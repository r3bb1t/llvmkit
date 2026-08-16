//! Auto-upgrade of legacy IR spellings. Mirrors `llvm/lib/IR/AutoUpgrade.cpp`,
//! and lives here rather than in the parser crate because that is upstream's
//! own layering: `AutoUpgrade.cpp` is `lib/IR`, and both the textual parser
//! (`LLParser::validateEndOfModule`) and the bitcode reader call into it.
//!
//! **Scope.** `AutoUpgrade.cpp` is dominated by target-specific intrinsic
//! rewrites (x86, ARM/AArch64, AMDGPU, NVVM, RISC-V, WebAssembly). Those, and
//! the intrinsic-upgrade framework they hang off, are a separate milestone —
//! see `docs/future-work.md`. What is ported here is the module-level,
//! target-independent half that `LLParser::validateEndOfModule` reaches for
//! every module it parses:
//!
//! | upstream | here |
//! |---|---|
//! | `llvm::UpgradeModuleFlags` | [`upgrade_module_flags`] |
//! | `llvm::UpgradeSectionAttributes` | [`upgrade_section_attributes`] |
//! | `llvm::UpgradeTBAANode` | [`upgrade_tbaa_node`] |
//!
//! Each is a mechanical translation of its upstream counterpart: same guards,
//! same order, same rebuilt node contents. Where a C++ shape has no Rust
//! spelling (an `unsigned` truncation, an `assert` on a malformed operand) the
//! difference is called out at the site.

use crate::error::IrResult;
use crate::metadata::MetadataId;
use crate::module::{Module, ModuleBrand, Unverified};
use crate::module_flags::ModuleFlagBehavior;
use crate::named_md_node::NamedMetadataName;

/// Upgrade the module-flag tuples of `llvm.module.flags` in place. `true` when
/// anything changed. Mirrors `llvm::UpgradeModuleFlags`
/// (`llvm/lib/IR/AutoUpgrade.cpp`).
///
/// The tuples are rewritten positionally — `NamedMDNode::setOperand(I, ...)`,
/// not "the first flag with this key" — so a module that repeats a key (legal
/// IR the verifier does not reject) upgrades every copy, exactly as upstream
/// does.
pub fn upgrade_module_flags<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
) -> bool {
    let Some(flags) = module.named_metadata(&NamedMetadataName::ModuleFlags) else {
        return false;
    };

    let mut has_objc_flag = false;
    let mut has_class_properties = false;
    let mut changed = false;
    // `HasSwiftVersionFlag` plus the three values it guards, as one `Option`
    // rather than a `bool` and three uninitialised locals (D-ADT: an upstream
    // sentinel becomes the shape that cannot be read before it is set).
    let mut swift_version: Option<SwiftVersion> = None;

    let operand_count = module
        .named_metadata_get(flags)
        .map_or(0, |node| node.operand_count());

    for index in 0..operand_count {
        // Re-read the node each iteration: a rebuild below replaces it.
        // `ModFlags->getOperand(I)`.
        let Some(node) = module.named_metadata_get(flags) else {
            break;
        };
        let Some(&op) = node.operands().get(index) else {
            break;
        };
        // `if (Op->getNumOperands() != 3) continue;`
        let Some(operands) = module.metadata_tuple_operands(op) else {
            continue;
        };
        let &[behavior_op, key_op, value_op] = operands.as_slice() else {
            continue;
        };
        // `MDString *ID = dyn_cast_or_null<MDString>(Op->getOperand(1));`
        let Some(id) = module.metadata_string_value(key_op) else {
            continue;
        };

        if id == "Objective-C Image Info Version" {
            has_objc_flag = true;
        }
        if id == "Objective-C Class Properties" {
            has_class_properties = true;
        }

        // Upgrade PIC from Error/Max to Min.
        if id == "PIC Level"
            && let Some(behavior) = behavior_value(module, behavior_op)
            && (behavior == u64::from(ModuleFlagBehavior::Error.raw())
                || behavior == u64::from(ModuleFlagBehavior::Max.raw()))
        {
            set_behavior(module, index, ModuleFlagBehavior::Min, &id, value_op);
            changed = true;
        }

        // Upgrade "PIE Level" from Error to Max.
        if id == "PIE Level"
            && let Some(behavior) = behavior_value(module, behavior_op)
            && behavior == u64::from(ModuleFlagBehavior::Error.raw())
        {
            set_behavior(module, index, ModuleFlagBehavior::Max, &id, value_op);
            changed = true;
        }

        // Upgrade branch protection and return address signing module flags.
        // The module flag behavior for these fields were Error and now they
        // are Min.
        if (id == "branch-target-enforcement" || id.starts_with("sign-return-address"))
            && let Some(behavior) = behavior_value(module, behavior_op)
            && behavior == u64::from(ModuleFlagBehavior::Error.raw())
        {
            // Unlike `SetBehavior`, this arm reuses operand 1 verbatim rather
            // than re-interning the key string — the same node either way,
            // since `MDString` is uniqued.
            let min = behavior_metadata(module, ModuleFlagBehavior::Min);
            replace_flag(module, index, [min, key_op, value_op]);
            changed = true;
        }

        // Upgrade Objective-C Image Info Section. Removed the whitespce in the
        // section name so that llvm-lto will not complain about mismatching
        // module flags that is functionally the same.
        if id == "Objective-C Image Info Section"
            && let Some(value) = module.metadata_string_value(value_op)
        {
            // `Value->getString().split(ValueComp, " ")` keeps empty
            // components, and so does `str::split`, so both count at least
            // one; `!= 1` is "the value contains a space".
            if value.split(' ').count() != 1 {
                let new_value: String = value.split(' ').collect();
                let new_value = module.metadata_string(new_value);
                replace_flag(module, index, [behavior_op, key_op, new_value]);
                changed = true;
            }
        }

        // IRUpgrader turns a i32 type "Objective-C Garbage Collection" into i8
        // value. If the higher bits are set, it adds new module flag for swift
        // info.
        if id == "Objective-C Garbage Collection" {
            // Upstream reads a `ConstantAsMetadata` and then
            // `getUniqueInteger()`, which asserts for a constant that is not
            // an integer. There is no assertion to reproduce in a crate that
            // forbids runtime panics, so a non-integer constant is left alone
            // — the verifier is what rejects it.
            let Some((bits, value)) = module.metadata_constant_int_value(value_op) else {
                continue;
            };
            // `if (Type == Int8Ty) continue;`
            if bits == 8 {
                continue;
            }
            // `unsigned Val = ...getZExtValue()` truncates to 32 bits on the
            // assignment; the mask below is that truncation, spelled out.
            let val = value.limited_value(u64::MAX) & 0xffff_ffff;
            if (val & 0xff) != val {
                swift_version = Some(SwiftVersion {
                    abi: val_field(val, 0xff00, 8),
                    major: val_field(val, 0xff00_0000, 24),
                    minor: val_field(val, 0x00ff_0000, 16),
                });
            }
            let error = behavior_metadata(module, ModuleFlagBehavior::Error);
            let low_byte = module.i8_type().const_int(byte_of(val & 0xff));
            let low_byte = module
                .metadata_constant(low_byte)
                .unwrap_or_else(|_| unreachable!("constant interned in this module"));
            replace_flag(module, index, [error, key_op, low_byte]);
            changed = true;
        }

        if id == "amdgpu_code_object_version" {
            let key = module.metadata_string("amdhsa_code_object_version");
            replace_flag(module, index, [behavior_op, key, value_op]);
            changed = true;
        }
    }

    // "Objective-C Class Properties" is recently added for Objective-C. We
    // upgrade ObjC bitcodes to contain a "Objective-C Class Properties" module
    // flag of value 0, so we can correclty downgrade this flag when trying to
    // link an ObjC bitcode without this module flag with an ObjC bitcode with
    // this module flag.
    if has_objc_flag && !has_class_properties {
        add_int_flag(
            module,
            ModuleFlagBehavior::Override,
            "Objective-C Class Properties",
            0,
        );
        changed = true;
    }

    if let Some(swift) = swift_version {
        add_int_flag(
            module,
            ModuleFlagBehavior::Error,
            "Swift ABI Version",
            swift.abi,
        );
        add_byte_flag(
            module,
            ModuleFlagBehavior::Error,
            "Swift Major Version",
            swift.major,
        );
        add_byte_flag(
            module,
            ModuleFlagBehavior::Error,
            "Swift Minor Version",
            swift.minor,
        );
        changed = true;
    }

    changed
}

/// The three Swift fields `UpgradeModuleFlags` decodes out of a wide
/// `"Objective-C Garbage Collection"` value.
struct SwiftVersion {
    /// `SwiftABIVersion` — `uint32_t`, from bits 8..16.
    abi: u32,
    /// `SwiftMajorVersion` — `uint8_t`, from bits 24..32.
    major: u8,
    /// `SwiftMinorVersion` — `uint8_t`, from bits 16..24.
    minor: u8,
}

/// `(Val & mask) >> shift`, narrowed to the width upstream's local has. The
/// mask guarantees the narrowing, so the fallback branch is unreachable.
fn val_field<T>(val: u64, mask: u64, shift: u32) -> T
where
    T: TryFrom<u64>,
{
    T::try_from((val & mask) >> shift)
        .unwrap_or_else(|_| unreachable!("a masked byte/half-word always fits its field"))
}

/// `Val & 0xff` as the `uint8_t` the i8 constant takes.
fn byte_of(val: u64) -> u8 {
    u8::try_from(val).unwrap_or_else(|_| unreachable!("caller masked with 0xff"))
}

/// `mdconst::dyn_extract_or_null<ConstantInt>(Op->getOperand(0))` followed by
/// `getLimitedValue()`.
fn behavior_value<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    behavior_op: MetadataId<B>,
) -> Option<u64> {
    module
        .metadata_constant_int_value(behavior_op)
        .map(|(_, value)| value.limited_value(u64::MAX))
}

/// `ConstantAsMetadata::get(ConstantInt::get(Int32Ty, B))`.
fn behavior_metadata<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    behavior: ModuleFlagBehavior,
) -> MetadataId<B> {
    let constant = module.i32_type().const_int(behavior.raw());
    module
        .metadata_constant(constant)
        .unwrap_or_else(|_| unreachable!("constant interned in this module"))
}

/// `UpgradeModuleFlags`'s `SetBehavior` lambda: rebuild the flag at `index`
/// with a new behavior, re-interning the key and keeping the value operand.
fn set_behavior<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    index: usize,
    behavior: ModuleFlagBehavior,
    key: &str,
    value_op: MetadataId<B>,
) {
    let behavior = behavior_metadata(module, behavior);
    let key = module.metadata_string(key);
    replace_flag(module, index, [behavior, key, value_op]);
}

/// `ModFlags->setOperand(I, MDNode::get(Context, Ops))`.
fn replace_flag<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    index: usize,
    operands: [MetadataId<B>; 3],
) {
    let tuple = module
        .metadata_tuple(operands)
        .unwrap_or_else(|_| unreachable!("operands were read out of this module"));
    let replaced = module
        .set_module_flag_operand(index, tuple)
        .unwrap_or_else(|_| unreachable!("tuple was interned in this module"));
    if !replaced {
        // The caller read `index` out of the very node it is writing back to,
        // inside a loop that never shortens it, so the node exists and the
        // index is in range. Stating it beats swallowing a `false`.
        unreachable!("the flag index came from the node being rewritten");
    }
}

/// `M.addModuleFlag(Behavior, Key, (uint32_t)Val)`.
fn add_int_flag<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    behavior: ModuleFlagBehavior,
    key: &str,
    value: u32,
) {
    let constant = module.i32_type().const_int(value);
    add_flag(module, behavior, key, module.metadata_constant(constant));
}

/// `M.addModuleFlag(Behavior, Key, ConstantInt::get(Int8Ty, Val))`.
fn add_byte_flag<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    behavior: ModuleFlagBehavior,
    key: &str,
    value: u8,
) {
    let constant = module.i8_type().const_int(value);
    add_flag(module, behavior, key, module.metadata_constant(constant));
}

fn add_flag<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    behavior: ModuleFlagBehavior,
    key: &str,
    value: IrResult<MetadataId<B>>,
) {
    let value = value.unwrap_or_else(|_| unreachable!("constant interned in this module"));
    module
        .add_module_flag(behavior, key, value)
        .unwrap_or_else(|_| unreachable!("value was interned in this module"));
}

/// Canonicalise Objective-C category-list section names by removing the
/// spaces around their commas. Mirrors `llvm::UpgradeSectionAttributes`
/// (`llvm/lib/IR/AutoUpgrade.cpp`).
///
/// Upstream walks `M.globals()` only — functions carry sections too, and are
/// deliberately not visited.
pub fn upgrade_section_attributes<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
) {
    for global in module.globals() {
        // `if (!GV.hasSection()) continue;`
        let Some(section) = global.section() else {
            continue;
        };
        if !section.starts_with("__DATA, __objc_catlist") {
            continue;
        }
        // __DATA, __objc_catlist, regular, no_dead_strip
        // __DATA,__objc_catlist,regular,no_dead_strip
        global.set_section(module, trim_spaces(&section));
    }
}

/// `UpgradeSectionAttributes`'s `TrimSpaces` lambda: split on `,`, trim each
/// component, and rejoin with `,`.
///
/// `StringRef::trim()` strips the ASCII whitespace set `" \t\n\v\f\r"`;
/// `str::trim` strips Unicode whitespace, a superset that coincides on every
/// section name a Mach-O toolchain emits.
fn trim_spaces(section: &str) -> String {
    section
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(",")
}

/// Rewrite a scalar-format TBAA tag into the struct-path-aware format,
/// answering `tag` itself when it already is one (or is too malformed to
/// classify). Mirrors `llvm::UpgradeTBAANode` (`llvm/lib/IR/AutoUpgrade.cpp`).
///
/// The three shapes, in upstream's order:
///
/// * zero operands — invalid, returned unchanged so the verifier reports it;
/// * first operand is an `MDNode` and there are at least three operands —
///   already struct-path aware;
/// * three operands — `!{ !{name, parent}, !{name, parent}, i64 0, const }`;
/// * anything else — `!{ tag, tag, i64 0 }`.
pub fn upgrade_tbaa_node<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    tag: MetadataId<B>,
) -> MetadataId<B> {
    let Some(operands) = module.metadata_tuple_operands(tag) else {
        // Not a tuple at all: upstream's `MDNode &` cannot be reached with
        // one, so there is nothing to upgrade.
        return tag;
    };
    if operands.is_empty() {
        // Invalid, punt to a verifier error.
        return tag;
    }

    // Check if the tag uses struct-path aware TBAA format.
    if operands.len() >= 3 && module.metadata_tuple_operands(operands[0]).is_some() {
        return tag;
    }

    // `ConstantAsMetadata::get(Constant::getNullValue(Int64Ty))`.
    let zero = module.i64_type().const_zero();
    let zero = module
        .metadata_constant(zero)
        .unwrap_or_else(|_| unreachable!("constant interned in this module"));

    if operands.len() == 3 {
        let scalar_type = tuple(module, &[operands[0], operands[1]]);
        // Create a MDNode <ScalarType, ScalarType, offset 0, const>
        tuple(module, &[scalar_type, scalar_type, zero, operands[2]])
    } else {
        // Create a MDNode <MD, MD, offset 0>
        tuple(module, &[tag, tag, zero])
    }
}

fn tuple<'ctx, B: ModuleBrand + 'ctx>(
    module: &'ctx Module<B, Unverified>,
    operands: &[MetadataId<B>],
) -> MetadataId<B> {
    module
        .metadata_tuple(operands)
        .unwrap_or_else(|_| unreachable!("operands were read out of this module"))
}
