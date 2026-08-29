//! Module verifier. Mirrors `llvm/include/llvm/IR/Verifier.h` and
//! `llvm/lib/IR/Verifier.cpp` for the constructive subset of opcodes
//! modeled by `llvmkit-ir`: arithmetic, casts, compares, memory, GEP,
//! calls, select, PHI, terminators, aggregate/vector operations, atomics,
//! EH / funclet pads, and the parser-era terminator families.
//!
//! The verifier walks every function, block, and instruction in
//! declaration order, applying per-opcode invariants. Each rule cites
//! its `Verifier::visit*` method in the upstream C++.
//!
//! ## Public surface
//!
//! - [`Module::verify_borrowed`](crate::Module::verify_borrowed) — borrow-only
//!   diagnostic check.
//! - [`Module::verify`](crate::Module::verify) — consumes the module and returns
//!   `Module<B, Verified>`, which the pass manager requires for
//!   module pipelines that assume well-formed IR.
//!
//! ## Coverage gaps (deferred)
//!
//! - Metadata / debug-info verifier rules are not fully ported yet.
//! - `Verifier::visitIntrinsicCall`'s preamble and its per-intrinsic `switch`
//!   are unported: `check_intrinsic_call` checks the signature, the `immarg`
//!   operands and the funclet token, and nothing else. See
//!   `docs/divergences.md`.
//! - GEP index-walks-the-aggregate-type checks are deferred; today the
//!   verifier checks that every GEP index is integer-typed and that the
//!   source type is sized.
//! - `Verifier::verifyFunctionAttrs` and `Verifier::verifyParameterAttrs` —
//!   the per-function and per-parameter attribute coherence rules (`sret`,
//!   `byval`, `nest`, `returned`, `Cannot have multiple 'swifterror'
//!   parameters!`, …) — have no counterpart here. See `docs/divergences.md`.
//!
//! `Verifier::verifyInlineAsmCall` is ported whole (both the per-operand
//! `elementtype` loop and the label-constraint tail), as is the EH pad
//! chapter: `visitEHPadPredecessors`, `visitFuncletPadInst`,
//! `verifySiblingFuncletUnwinds` and the per-opcode `visit*Inst` routines
//! that call them.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};

use super::cfg::FunctionCfg;
use super::constant::{Constant, ConstantData};
use super::eh_personalities::{
    classify_eh_personality, color_eh_funclets, first_non_phi_kind, is_funclet_pad_kind,
    is_scoped_eh_personality,
};
use super::global_value::Linkage;
use super::global_variable::GlobalVariable;
use super::inline_asm::{ConstraintKind, InlineAsm};
use super::instr_types::{
    AllocaInstData, AtomicCmpXchgInstData, AtomicRmwInstData, CallAttributeData, CallBrInstData,
    CallInstData, CatchPadInstData, CatchReturnInstData, CatchSwitchInstData, CleanupPadInstData,
    CleanupReturnInstData, ExtractElementInstData, ExtractValueInstData, FenceInstData,
    FnegInstData, FreezeInstData, IndirectBrInstData, InsertElementInstData, InsertValueInstData,
    InvokeInstData, LandingPadClauseKind, LandingPadInstData, LoadInstData, OperandBundleTag,
    ResumeInstData, SelectInstData, ShuffleVectorInstData, StoreInstData, SwitchInstData,
    VaArgInstData,
};
use super::instruction::{InstructionKind, TerminatorKind};
use super::instructions::ShuffleVectorInst;
use super::intrinsics::{IntrinsicId, IntrinsicNameResolution};
use super::module::ModuleRef;
use super::value::Value;
use crate::attributes::{AttrIndex, AttrKind, AttributeStorage, AttributeStored};
use crate::basic_block::BasicBlock;
use crate::block_state::Unterminated;
use crate::constant_range::{ConstantRange, metadata_constant_int};
use crate::derived_types::SizedType;
use crate::dominator_tree::DominatorTree;
use crate::error::{IrError, IrResult, VerifierRule};
use crate::function::FunctionValue;
use crate::instr_types::{
    BinaryOpData, BranchInstData, BranchKind, CastOpData, CastOpcode, CmpInstData, FcmpInstData,
    GepInstData, PhiData, ReturnOpData,
};
use crate::instruction::{InstructionKindData, InstructionView};
use crate::marker::Dyn;
use crate::metadata::{
    MetadataAttachmentKind, MetadataId, MetadataKind, MetadataSlot, MetadataStore, StoredBrand,
};
use crate::module::{Invariant, ModuleBrand, ModuleCore, ModuleView};
use crate::module_flags::{ModuleFlagBehavior, module_flag_tuple, resolve_metadata_ref};
use crate::named_md_node::NamedMetadataName;
use crate::phi_check::{PhiViolation, check_phi_incoming};
use crate::r#type::{Type, TypeData, TypeSlot};
use crate::value::{IsValue, ValueKindData, ValueSlot};

// --------------------------------------------------------------------------
// Verifier
// --------------------------------------------------------------------------

/// CFG context built once per function and threaded through every
/// per-block / per-instruction visit. Mirrors LLVM's transient
/// per-function state inside `Verifier::visit*`.
struct FunctionContext<'a> {
    /// Predecessor multiset per block id.
    predecessors: &'a HashMap<ValueSlot, Vec<ValueSlot>>,
    /// Declaration-order index of every block in the parent function.
    block_index: &'a HashMap<ValueSlot, usize>,
    /// Recomputed dominator tree for cross-block SSA dominance checks.
    dom_tree: &'a DominatorTree,
    /// `Verifier::BlockEHFuncletColors`: the EH funclet colouring, built on
    /// demand by the first intrinsic call that needs it and shared by the rest
    /// of the function. Upstream clears the map per function; here it lives and
    /// dies with this context.
    eh_funclet_colors: &'a OnceCell<HashMap<ValueSlot, Vec<ValueSlot>>>,
    /// `Verifier::SiblingFuncletInfo`: the cleanup-sibling unwind edges
    /// `visitFuncletPadInst` and `visitCatchSwitchInst` record, consumed by
    /// `verifySiblingFuncletUnwinds` once the function's instructions have
    /// been visited. Upstream's `MapVector` is insertion-ordered and that
    /// routine iterates it in insertion order, so this is a `Vec` of pairs and
    /// not a `HashMap`. Upstream clears it per function; here it lives and
    /// dies with this context.
    sibling_funclet_info: &'a RefCell<Vec<(ValueSlot, ValueSlot)>>,
    /// `Verifier::LandingPadResultTy`: the result type the function's first
    /// `landingpad` or `resume` established, which every later one must agree
    /// with. Upstream resets it to null per function.
    landing_pad_result_ty: &'a Cell<Option<TypeSlot>>,
}

/// The four `CallBase` fields the shared `visitCallBase` / `visitIntrinsicCall`
/// halves read, projected out of a `call`, `invoke` or `callbr` payload — the
/// slice of `CallBase` those routines take.
#[derive(Clone, Copy)]
struct CallBaseParts<'a> {
    /// `CallBase::getCalledOperand()`.
    callee: ValueSlot,
    /// `CallBase::getFunctionType()`.
    fn_ty: TypeSlot,
    /// `CallBase::args()`.
    args: &'a [core::cell::Cell<ValueSlot>],
    /// `CallBase::getAttributes()` plus the operand bundles.
    attrs: &'a CallAttributeData,
}

/// Where an instruction sits in its block: the position plus the block's
/// instruction list. Together they are the `BasicBlock::iterator` that
/// `Verifier::verifyMustTailCall` advances with `++BBI` to find the `bitcast`
/// and `ret` that must follow a `musttail call`.
#[derive(Clone, Copy)]
struct BlockPosition<'a, 'ctx, B: ModuleBrand> {
    index: usize,
    instructions: &'a [InstructionView<'ctx, B>],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeLikeMetadataKind {
    Range,
    AbsoluteSymbol,
}

/// Recursion cap for `Verifier::metadata_structurally_equal`. Module-flag
/// values are shallow — upstream's are scalars, strings, or small tuples —
/// so the cap exists only to make a `metadata_reserve`/`metadata_set`
/// self-referential tuple terminate (answering unequal) instead of
/// recursing forever.
const METADATA_EQUALITY_DEPTH_LIMIT: u32 = 32;

/// Module verifier. Stateless apart from the per-function CFG cache
/// it builds during a [`Self::run`] traversal.
pub(crate) struct Verifier<'ctx, B: ModuleBrand> {
    module: &'ctx ModuleCore,
    _brand: Invariant<B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Verifier<'ctx, B> {
    pub(crate) fn new(module: ModuleView<'ctx, B>) -> Self {
        Self {
            module: module.core_ref(),
            _brand: core::marker::PhantomData,
        }
    }

    /// Verify every function in the module. Returns the first
    /// invariant violation encountered. Stops on first error to keep
    /// `IrError` single-shot; a later revision can add a multi-error
    /// collecting variant if pass infrastructure needs it.
    pub(crate) fn run(&self) -> IrResult<()> {
        for g in self.module.iter_globals() {
            self.visit_global_variable(g)?;
        }
        for i in self.module.iter_ifuncs::<B>() {
            self.visit_global_ifunc(i)?;
        }
        // Mirrors `Verifier::verify`'s module-level `visitModuleFlags()`
        // step between the global-value walk and the function walk.
        self.visit_module_flags()?;
        for f in self.module.iter_functions() {
            self.visit_function(f)?;
        }
        Ok(())
    }

    /// Mirrors `Verifier::visitGlobalVariable` for the constructive
    /// subset shipped today (initializer type/sized, common-linkage
    /// invariants, scalable-type rejection). The intrinsic-globals
    /// (`llvm.global_ctors` / `llvm.used` / etc.) and metadata
    /// attachment rules are deferred -- they need the metadata layer.
    /// Mirrors the linkage arm of `Verifier::visitGlobalIFunc`.
    ///
    /// It lives here, not in the parser, because upstream's parser has no such
    /// check: `parseAliasOrIFunc` guards `isValidLinkage` with
    /// `if (IsAlias && ...)`, so an ifunc with a bad linkage *parses* and is
    /// caught at verify time. llvmkit used to reject it at parse time and
    /// again in `GlobalIfuncBuilder::build`, which made the upstream
    /// diagnostic unreachable.
    fn visit_global_ifunc(&self, i: crate::GlobalIfunc<'ctx, B>) -> IrResult<()> {
        if !crate::global_ifunc::is_valid_ifunc_linkage(i.linkage()) {
            return Err(IrError::VerifierFailure {
                rule: VerifierRule::IfuncInvalidLinkage,
                function: Some(format!("@{}", i.name())),
                block: None,
                message: "IFunc should have private, internal, linkonce, weak, linkonce_odr, \
                          weak_odr, or external linkage!"
                    .to_owned(),
            });
        }
        Ok(())
    }

    fn visit_global_variable(&self, g: GlobalVariable<'ctx, B>) -> IrResult<()> {
        let value_ty = g.value_type();

        if type_contains_scalable(self.module, value_ty.id()) {
            return Err(self.fail_global(
                g,
                VerifierRule::GlobalScalableType,
                format!("Globals cannot contain scalable types (@{})", g.name()),
            ));
        }

        if let Some(init) = g.initializer() {
            if init.ty() != value_ty {
                return Err(self.fail_global(
                    g,
                    VerifierRule::GlobalInitializerTypeMismatch,
                    format!(
                        "Global variable initializer type does not match global variable type! (@{}: initializer type {}, value type {})",
                        g.name(),
                        init.ty().kind_label(),
                        value_ty.kind_label(),
                    ),
                ));
            }
            if !value_ty.is_sized() {
                return Err(self.fail_global(
                    g,
                    VerifierRule::GlobalInitializerUnsized,
                    format!("Global variable initializer must be sized (@{})", g.name()),
                ));
            }
            if g.linkage() == Linkage::Common {
                // Upstream's `if (GV.hasCommonLinkage())` block is three
                // separate `Check`s with three literals, and is three checks
                // here for the same reason.
                //
                // The first asks `GV.getInitializer()->isNullValue()`, which is
                // true of a zero *aggregate* too — `common global [10 x T]
                // zeroinitializer` is the shape clang emits. Recognising only
                // scalar zeros rejected it.
                if !crate::constants::constant_id_is_null_value(self.module, init.slot()) {
                    return Err(self.fail_global(
                        g,
                        VerifierRule::CommonLinkageInvariantViolated,
                        format!(
                            "'common' global must have a zero initializer! (@{})",
                            g.name()
                        ),
                    ));
                }
                if g.is_constant() {
                    return Err(self.fail_global(
                        g,
                        VerifierRule::CommonLinkageInvariantViolated,
                        format!(
                            "'common' global may not be marked constant! (@{})",
                            g.name()
                        ),
                    ));
                }
                if g.comdat().is_some() {
                    return Err(self.fail_global(
                        g,
                        VerifierRule::CommonLinkageInvariantViolated,
                        format!("'common' global may not be in a Comdat! (@{})", g.name()),
                    ));
                }
            }
            self.verify_constant_tree(init)?;
        }
        if let Some(range_id) = g
            .metadata_stored()
            .get(&MetadataAttachmentKind::AbsoluteSymbol)
        {
            let pointer_width = self
                .module
                .data_layout()
                .pointer_size_in_bits(g.address_space());
            let pointer_int_ty = self.module.context().int_type(pointer_width);
            self.verify_range_like_metadata_global(
                g,
                range_id.slot(),
                pointer_int_ty,
                RangeLikeMetadataKind::AbsoluteSymbol,
            )?;
        }

        Ok(())
    }

    fn verify_constant_tree(&self, constant: Constant<'ctx, B>) -> IrResult<()> {
        let value_data = self.module.context().value_data(constant.slot());
        let ValueKindData::Constant(data) = &value_data.kind else {
            return Ok(());
        };
        match data {
            ConstantData::Expr(expr) => {
                crate::constants::verify_constant_expr_data(self.module, expr)?;
                for operand in expr.operands.iter() {
                    let operand_data = self.module.context().value_data(*operand);
                    if matches!(operand_data.kind, ValueKindData::Constant(_)) {
                        self.verify_constant_tree(Constant::try_from(Value::from_parts(
                            *operand,
                            self.module,
                            operand_data.ty,
                        ))?)?;
                    }
                }
            }
            ConstantData::BlockAddress { function, block } => {
                let block = BasicBlock::<'ctx, Dyn, Unterminated, B>::from_parts(
                    *block,
                    self.module,
                    self.module.label_type::<B>().as_type().id(),
                );
                if block.parent_function().map(|f| f.slot()) != Some(*function) {
                    return Err(IrError::InvalidOperation {
                        message: "blockaddress block must belong to referenced function",
                    });
                }
            }
            ConstantData::DsoLocalEquivalent { function } => {
                let value =
                    Value::<B>::from_parts(*function, self.module, self.value_type(*function));
                match &value.data().kind {
                    ValueKindData::Function(_) => {}
                    ValueKindData::GlobalAlias(_) => {
                        if !crate::GlobalAlias::try_from(value)?
                            .value_type()
                            .is_function()
                        {
                            return Err(IrError::InvalidOperation {
                                message: "dso_local_equivalent expects a function, alias to function, or ifunc",
                            });
                        }
                    }
                    ValueKindData::GlobalIfunc(_) => {
                        if !crate::GlobalIfunc::try_from(value)?
                            .value_type()
                            .is_function()
                        {
                            return Err(IrError::InvalidOperation {
                                message: "dso_local_equivalent expects a function, alias to function, or ifunc",
                            });
                        }
                    }
                    _ => {
                        return Err(IrError::InvalidOperation {
                            message: "dso_local_equivalent expects a function, alias to function, or ifunc",
                        });
                    }
                }
            }
            ConstantData::NoCfi { function } => {
                let value =
                    Value::<B>::from_parts(*function, self.module, self.value_type(*function));
                match &value.data().kind {
                    ValueKindData::Function(_)
                    | ValueKindData::GlobalVariable(_)
                    | ValueKindData::GlobalAlias(_)
                    | ValueKindData::GlobalIfunc(_) => {}
                    _ => {
                        return Err(IrError::InvalidOperation {
                            message: "no_cfi expects a global value",
                        });
                    }
                }
            }
            ConstantData::TokenNone => {
                if !constant.ty().is_token() {
                    return Err(IrError::InvalidOperation {
                        message: "token none must have token type",
                    });
                }
            }
            ConstantData::TargetExtNone => {
                if !constant.ty().is_target_ext() {
                    return Err(IrError::InvalidOperation {
                        message: "target extension none must have target extension type",
                    });
                }
            }
            ConstantData::PtrAuth {
                pointer,
                key,
                discriminator,
                addr_discriminator,
                deactivation_symbol,
            } => {
                let pointer = Value::from_parts(*pointer, self.module, self.value_type(*pointer));
                let key = Value::<B>::from_parts(*key, self.module, self.value_type(*key));
                let discriminator = Value::<B>::from_parts(
                    *discriminator,
                    self.module,
                    self.value_type(*discriminator),
                );
                let addr_discriminator = Value::<B>::from_parts(
                    *addr_discriminator,
                    self.module,
                    self.value_type(*addr_discriminator),
                );
                let deactivation_symbol = Value::<B>::from_parts(
                    *deactivation_symbol,
                    self.module,
                    self.value_type(*deactivation_symbol),
                );
                if !pointer.ty().is_pointer()
                    || !addr_discriminator.ty().is_pointer()
                    || !deactivation_symbol.ty().is_pointer()
                    || key.ty() != self.module.i32_type().as_type()
                    || discriminator.ty() != self.module.i64_type().as_type()
                    || constant.ty() != pointer.ty()
                    || !matches!(
                        &self
                            .module
                            .context()
                            .value_data(deactivation_symbol.id)
                            .kind,
                        ValueKindData::Constant(
                            ConstantData::GlobalValueRef { .. } | ConstantData::PointerNull
                        )
                    )
                {
                    return Err(IrError::InvalidOperation {
                        message: "invalid ptrauth constant",
                    });
                }
            }
            ConstantData::Aggregate(ids) => {
                for id in ids.iter() {
                    let operand_data = self.module.context().value_data(*id);
                    if matches!(operand_data.kind, ValueKindData::Constant(_)) {
                        self.verify_constant_tree(Constant::try_from(Value::from_parts(
                            *id,
                            self.module,
                            operand_data.ty,
                        ))?)?;
                    }
                }
            }
            ConstantData::ForwardRefPlaceholder => {
                return Err(IrError::InvalidOperation {
                    message: "unresolved forward-reference placeholder",
                });
            }
            ConstantData::GlobalValueRef { .. }
            | ConstantData::PointerNull
            | ConstantData::GepOffset { .. }
            | ConstantData::SymbolDelta { .. }
            | ConstantData::SymbolDeltaPlus { .. }
            | ConstantData::Int(_)
            | ConstantData::Float(_)
            | ConstantData::Undef
            | ConstantData::Poison => {}
        }
        Ok(())
    }

    fn fail_global(
        &self,
        g: GlobalVariable<'ctx, B>,
        rule: VerifierRule,
        message: String,
    ) -> IrError {
        IrError::VerifierFailure {
            rule,
            function: Some(format!("@{}", g.name())),
            block: None,
            message,
        }
    }

    /// Module-level failure with no function/block context — the shape of
    /// `Verifier::CheckFailed` for module-flag violations.
    fn fail_module_flags(&self, rule: VerifierRule, message: String) -> IrError {
        IrError::VerifierFailure {
            rule,
            function: None,
            block: None,
            message,
        }
    }

    // ------------------------------------------------------------------
    // Module flags
    // ------------------------------------------------------------------

    /// Mirrors `Verifier::visitModuleFlags` (`lib/IR/Verifier.cpp`): walks
    /// every `llvm.module.flags` operand through
    /// [`Self::visit_module_flag`], tracks the `aarch64-elf-pauthabi-*`
    /// pairing, then validates the collected `require` entries against the
    /// seen flags. Single-shot like the rest of this verifier — the first
    /// violation returns where upstream's `CheckFailed` accumulates.
    fn visit_module_flags(&self) -> IrResult<()> {
        let Some(flags_id) = self.module.named_metadata(&NamedMetadataName::ModuleFlags) else {
            return Ok(());
        };
        let operands: Vec<MetadataId<StoredBrand>> = {
            let nmd = self.module.named_metadata_list();
            let node = nmd.get(flags_id.slot().0).unwrap_or_else(|| {
                unreachable!("a stored NamedMetadataId always names a node in the append-only list")
            });
            node.operands().to_vec()
        };

        let store = self.module.metadata_store();
        // Upstream: `DenseMap<const MDString*, const MDNode*> SeenIDs` — key
        // to flag; the requirement pass reads the flag's value operand, so
        // the map carries that operand directly.
        let mut seen: HashMap<String, MetadataId<StoredBrand>> = HashMap::new();
        let mut requirements: Vec<MetadataSlot> = Vec::new();
        // Upstream tracks these as `uint64_t(-1)` sentinels; `Option` spells
        // the same "no constant-integer value seen" state.
        let mut pauth_abi_platform: Option<u64> = None;
        let mut pauth_abi_version: Option<u64> = None;
        for op in operands {
            self.visit_module_flag(&store, op, &mut seen, &mut requirements)?;
            let Some([_, key_id, value_id]) = module_flag_tuple(&store, op) else {
                // Upstream: `if (MDN->getNumOperands() != 3) continue;`.
                continue;
            };
            let Some(MetadataKind::String(name)) =
                resolve_metadata_ref(&store, key_id.slot()).and_then(|slot| store.get(slot))
            else {
                continue;
            };
            let constant_value = || {
                resolve_metadata_ref(&store, value_id.slot())
                    .and_then(|slot| metadata_constant_int(self.module, &store, slot))
                    .map(|(_, value)| value.limited_value(u64::MAX))
            };
            if name == "aarch64-elf-pauthabi-platform" {
                pauth_abi_platform = constant_value();
            } else if name == "aarch64-elf-pauthabi-version" {
                pauth_abi_version = constant_value();
            }
        }

        if pauth_abi_platform.is_some() != pauth_abi_version.is_some() {
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagPauthAbiPairing,
                "either both or no 'aarch64-elf-pauthabi-platform' and \
                 'aarch64-elf-pauthabi-version' module flags must be present"
                    .to_string(),
            ));
        }

        // Validate that the requirements in the module are valid.
        for requirement in requirements {
            let Some(MetadataKind::Tuple { operands: pair, .. }) = store.get(requirement) else {
                unreachable!("a collected requirement was validated as a metadata pair")
            };
            let Some(MetadataKind::String(flag_name)) =
                resolve_metadata_ref(&store, pair[0].slot()).and_then(|slot| store.get(slot))
            else {
                unreachable!("a collected requirement's first operand was validated as a string")
            };
            let required_value = pair[1];
            match seen.get(flag_name) {
                None => {
                    return Err(self.fail_module_flags(
                        VerifierRule::ModuleFlagInvalidRequirement,
                        format!(
                            "invalid requirement on flag, flag is not present in module: !\"{flag_name}\""
                        ),
                    ));
                }
                Some(actual_value) => {
                    if !self.metadata_structurally_equal(
                        &store,
                        actual_value.slot(),
                        required_value.slot(),
                        METADATA_EQUALITY_DEPTH_LIMIT,
                    ) {
                        return Err(self.fail_module_flags(
                            VerifierRule::ModuleFlagInvalidRequirement,
                            format!(
                                "invalid requirement on flag, flag does not have the required value: !\"{flag_name}\""
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Mirrors `Verifier::visitModuleFlag` (`lib/IR/Verifier.cpp`): the
    /// three-operand tuple shape, the behavior operand (via
    /// [`ModuleFlagBehavior::from_raw`], the range check of
    /// `Module::isValidModFlagBehavior`), the `MDString` ID, the
    /// per-behavior value constraints, ID uniqueness outside `require`, and
    /// the per-key `wchar_size` / `Linker Options` / `SemanticInterposition`
    /// / `CG Profile` checks.
    fn visit_module_flag(
        &self,
        store: &MetadataStore,
        op: MetadataId<StoredBrand>,
        seen: &mut HashMap<String, MetadataId<StoredBrand>>,
        requirements: &mut Vec<MetadataSlot>,
    ) -> IrResult<()> {
        // Upstream a `NamedMDNode` operand is an `MDNode *` by type, so the
        // only shape failure is the operand count; llvmkit's named-metadata
        // operands are any metadata id, and a non-tuple operand lands in the
        // same arm.
        let operands = match resolve_metadata_ref(store, op.slot()).and_then(|slot| store.get(slot))
        {
            Some(MetadataKind::Tuple { operands, .. }) if operands.len() == 3 => operands.clone(),
            _ => {
                return Err(self.fail_module_flags(
                    VerifierRule::ModuleFlagInvalidOperandCount,
                    "incorrect number of operands in module flag".to_string(),
                ));
            }
        };

        // Behavior operand: a constant integer inside `1..=8`.
        let Some((_, behavior_value)) = resolve_metadata_ref(store, operands[0].slot())
            .and_then(|slot| metadata_constant_int(self.module, store, slot))
        else {
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagInvalidBehavior,
                "invalid behavior operand in module flag (expected constant integer)".to_string(),
            ));
        };
        let Some(behavior) = ModuleFlagBehavior::from_raw(behavior_value.limited_value(u64::MAX))
        else {
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagInvalidBehavior,
                "invalid behavior operand in module flag (unexpected constant)".to_string(),
            ));
        };

        // ID operand: a metadata string.
        let key = match resolve_metadata_ref(store, operands[1].slot())
            .and_then(|slot| store.get(slot))
        {
            Some(MetadataKind::String(s)) => s.clone(),
            _ => {
                return Err(self.fail_module_flags(
                    VerifierRule::ModuleFlagInvalidId,
                    "invalid ID operand in module flag (expected metadata string)".to_string(),
                ));
            }
        };

        // Check the values for behaviors with additional requirements.
        let value_id = operands[2];
        let value_slot = resolve_metadata_ref(store, value_id.slot());
        let value_constant_int =
            || value_slot.and_then(|slot| metadata_constant_int(self.module, store, slot));
        match behavior {
            // These behavior types accept any value.
            ModuleFlagBehavior::Error
            | ModuleFlagBehavior::Warning
            | ModuleFlagBehavior::Override => {}
            ModuleFlagBehavior::Min => {
                if !value_constant_int().is_some_and(|(_, value)| value.is_non_negative()) {
                    return Err(self.fail_module_flags(
                        VerifierRule::ModuleFlagInvalidValue,
                        "invalid value for 'min' module flag (expected constant non-negative \
                         integer)"
                            .to_string(),
                    ));
                }
            }
            ModuleFlagBehavior::Max => {
                if value_constant_int().is_none() {
                    return Err(self.fail_module_flags(
                        VerifierRule::ModuleFlagInvalidValue,
                        "invalid value for 'max' module flag (expected constant integer)"
                            .to_string(),
                    ));
                }
            }
            ModuleFlagBehavior::Require => {
                // The value should itself be a node with two operands: a
                // flag ID string and a value.
                let pair = match value_slot.and_then(|slot| store.get(slot)) {
                    Some(MetadataKind::Tuple { operands, .. }) if operands.len() == 2 => {
                        operands.clone()
                    }
                    _ => {
                        return Err(self.fail_module_flags(
                            VerifierRule::ModuleFlagInvalidValue,
                            "invalid value for 'require' module flag (expected metadata pair)"
                                .to_string(),
                        ));
                    }
                };
                let first_is_string = matches!(
                    resolve_metadata_ref(store, pair[0].slot()).and_then(|slot| store.get(slot)),
                    Some(MetadataKind::String(_))
                );
                if !first_is_string {
                    return Err(self.fail_module_flags(
                        VerifierRule::ModuleFlagInvalidValue,
                        "invalid value for 'require' module flag (first value operand should be \
                         a string)"
                            .to_string(),
                    ));
                }
                // Append it to the list of requirements, to check once all
                // module flags are scanned.
                requirements.push(value_slot.unwrap_or_else(|| {
                    unreachable!("a decoded metadata pair resolved to a node slot")
                }));
            }
            ModuleFlagBehavior::Append | ModuleFlagBehavior::AppendUnique => {
                // These behavior types require the operand be a metadata
                // node (upstream `isa<MDNode>` — a tuple or a specialized
                // node here; strings, constants, and null are not nodes).
                let is_node = matches!(
                    value_slot.and_then(|slot| store.get(slot)),
                    Some(MetadataKind::Tuple { .. } | MetadataKind::Specialized(_))
                );
                if !is_node {
                    return Err(self.fail_module_flags(
                        VerifierRule::ModuleFlagInvalidValue,
                        "invalid value for 'append'-type module flag (expected a metadata node)"
                            .to_string(),
                    ));
                }
            }
        }

        // Unless this is a "requires" flag, check the ID is unique.
        if behavior != ModuleFlagBehavior::Require && seen.insert(key.clone(), value_id).is_some() {
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagDuplicateId,
                format!(
                    "module flag identifiers must be unique (or of 'require' type): !\"{key}\""
                ),
            ));
        }

        if key == "wchar_size" && value_constant_int().is_none() {
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagInvalidValue,
                "wchar_size metadata requires constant integer argument".to_string(),
            ));
        }

        if key == "Linker Options"
            && self
                .module
                .named_metadata(&NamedMetadataName::LinkerOptions)
                .is_none()
        {
            // If the llvm.linker.options named metadata exists, the flag was
            // upgraded by the bitcode reader; otherwise it was created by a
            // client directly and is no longer supported.
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagLinkerOptionsUnsupported,
                "'Linker Options' named metadata no longer supported".to_string(),
            ));
        }

        if key == "SemanticInterposition" && value_constant_int().is_none() {
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagInvalidValue,
                "SemanticInterposition metadata requires constant integer argument".to_string(),
            ));
        }

        if key == "CG Profile" {
            // Upstream's `cast<MDNode>(Op->getOperand(2))` assumes a
            // node-valued flag — guaranteed when the behavior is `append`
            // (checked above), an assertion failure otherwise. llvmkit
            // iterates the entries only when the value is a tuple.
            if let Some(MetadataKind::Tuple {
                operands: entries, ..
            }) = value_slot.and_then(|slot| store.get(slot))
            {
                for entry in entries.clone() {
                    self.visit_module_flag_cg_profile_entry(store, entry)?;
                }
            }
        }

        Ok(())
    }

    /// Mirrors `Verifier::visitModuleFlagCGProfileEntry`
    /// (`lib/IR/Verifier.cpp`): each `CG Profile` entry is a three-operand
    /// node of caller, callee, and count, where caller/callee are functions
    /// or null and the count is a constant integer.
    fn visit_module_flag_cg_profile_entry(
        &self,
        store: &MetadataStore,
        entry: MetadataId<StoredBrand>,
    ) -> IrResult<()> {
        let triple = match resolve_metadata_ref(store, entry.slot())
            .and_then(|slot| store.get(slot))
        {
            Some(MetadataKind::Tuple { operands, .. }) if operands.len() == 3 => operands.clone(),
            _ => {
                return Err(self.fail_module_flags(
                    VerifierRule::ModuleFlagCgProfileMalformed,
                    "expected a MDNode triple".to_string(),
                ));
            }
        };
        for function_operand in [triple[0], triple[1]] {
            if !self.cg_profile_operand_is_function_or_null(store, function_operand) {
                return Err(self.fail_module_flags(
                    VerifierRule::ModuleFlagCgProfileMalformed,
                    "expected a Function or null".to_string(),
                ));
            }
        }
        let count_is_integer = resolve_metadata_ref(store, triple[2].slot())
            .and_then(|slot| metadata_constant_int(self.module, store, slot))
            .is_some();
        if !count_is_integer {
            return Err(self.fail_module_flags(
                VerifierRule::ModuleFlagCgProfileMalformed,
                "expected an integer constant".to_string(),
            ));
        }
        Ok(())
    }

    /// The `CheckFunction` lambda of `Verifier::visitModuleFlagCGProfileEntry`:
    /// null passes; otherwise the operand must be a value-as-metadata whose
    /// value strips to a `Function`. llvmkit's expressible spellings of such
    /// a value are a `GlobalValueRef` constant naming a function (`ptr @f`)
    /// or the function value itself.
    fn cg_profile_operand_is_function_or_null(
        &self,
        store: &MetadataStore,
        op: MetadataId<StoredBrand>,
    ) -> bool {
        let Some(slot) = resolve_metadata_ref(store, op.slot()) else {
            return false;
        };
        match store.get(slot) {
            Some(MetadataKind::Null) => true,
            Some(MetadataKind::Constant(value_id)) => {
                let data = self.module.context().value_data(value_id.slot());
                match &data.kind {
                    ValueKindData::Function(_) => true,
                    ValueKindData::Constant(ConstantData::GlobalValueRef { value }) => matches!(
                        self.module.context().value_data(*value).kind,
                        ValueKindData::Function(_)
                    ),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Metadata equality for the `require` value comparison.
    ///
    /// Upstream compares `Op->getOperand(2) != ReqValue` — pointer identity,
    /// which metadata uniquing makes structural for the uniqued kinds.
    /// llvmkit interns strings and global-value-ref constants but not (yet)
    /// tuples or integer constants (the constant-uniquing layer is recorded
    /// future work), so identity alone would wrongly reject
    /// `!{!"flag-1", i32 55}` against a flag whose value is a *different*
    /// arena node spelling the same `i32 55`. This helper therefore compares
    /// structurally, keeping upstream's identity semantics where uniquing
    /// *is* identity upstream: `distinct` tuples and specialized nodes are
    /// equal only as the same slot. Depth-capped so a
    /// `metadata_reserve`/`metadata_set` cycle terminates (answering
    /// `false`), which no well-formed flag value reaches.
    fn metadata_structurally_equal(
        &self,
        store: &MetadataStore,
        a: MetadataSlot,
        b: MetadataSlot,
        depth: u32,
    ) -> bool {
        if depth == 0 {
            return false;
        }
        let (Some(a), Some(b)) = (
            resolve_metadata_ref(store, a),
            resolve_metadata_ref(store, b),
        ) else {
            return false;
        };
        if a == b {
            return true;
        }
        let (Some(node_a), Some(node_b)) = (store.get(a), store.get(b)) else {
            return false;
        };
        match (node_a, node_b) {
            (MetadataKind::Null, MetadataKind::Null) => true,
            (MetadataKind::String(x), MetadataKind::String(y)) => x == y,
            (MetadataKind::Constant(x), MetadataKind::Constant(y)) => {
                let data_x = self.module.context().value_data(x.slot());
                let data_y = self.module.context().value_data(y.slot());
                if data_x.ty != data_y.ty {
                    return false;
                }
                match (&data_x.kind, &data_y.kind) {
                    (ValueKindData::Constant(constant_x), ValueKindData::Constant(constant_y)) => {
                        constant_x == constant_y
                    }
                    _ => false,
                }
            }
            (
                MetadataKind::Tuple {
                    distinct: false,
                    operands: x,
                },
                MetadataKind::Tuple {
                    distinct: false,
                    operands: y,
                },
            ) => {
                x.len() == y.len()
                    && x.iter().zip(y.iter()).all(|(x, y)| {
                        self.metadata_structurally_equal(store, x.slot(), y.slot(), depth - 1)
                    })
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------------
    // Per-function walk
    // ------------------------------------------------------------------

    fn visit_function(&self, f: FunctionValue<'ctx, Dyn, B>) -> IrResult<()> {
        self.verify_intrinsic_function(f)?;
        // Build a CFG predecessor map for this function so phi-validation
        // and use-before-def checks can consult it without re-walking
        // every terminator. Mirrors `Verifier::predecessorMultiset`
        // in `Verifier.cpp`.
        let predecessors = build_predecessors(f);
        // Collect block ids in declaration order so use-before-def
        // can check forward references between blocks (cross-block
        // checks are conservative -- see deferred-coverage note).
        let block_ids: Vec<ValueSlot> = f.basic_blocks().map(|bb| bb.slot()).collect();
        let block_index: HashMap<ValueSlot, usize> = block_ids
            .iter()
            .copied()
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect();

        let dom_tree = DominatorTree::new(f);
        let eh_funclet_colors = OnceCell::new();
        let sibling_funclet_info = RefCell::new(Vec::new());
        let landing_pad_result_ty = Cell::new(None);
        let cx = FunctionContext {
            predecessors: &predecessors,
            block_index: &block_index,
            dom_tree: &dom_tree,
            eh_funclet_colors: &eh_funclet_colors,
            sibling_funclet_info: &sibling_funclet_info,
            landing_pad_result_ty: &landing_pad_result_ty,
        };
        // `visitFunction`'s argument loop: "Check that swifterror argument is
        // only used by loads and stores." —
        // `if (Attrs.hasParamAttr(i, Attribute::SwiftError))
        //    verifySwiftErrorValue(&Arg);`
        //
        // The loop runs before the block walk, as upstream's does. A
        // declaration has no block to anchor a diagnostic at and no users
        // inside one either, so the walk simply finds nothing.
        {
            let attrs = f.data().attributes.borrow();
            // A declaration has no block to anchor a diagnostic at, and none
            // of its arguments can have a user inside one either, so the walk
            // would raise nothing.
            let entry = f
                .basic_blocks()
                .next()
                .map(BasicBlock::retag_termination::<Unterminated>);
            for argument in f.params() {
                if !attrs.has_kind(AttrIndex::Param(argument.slot()), AttrKind::SwiftError) {
                    continue;
                }
                let Some(entry) = entry.as_ref() else {
                    continue;
                };
                self.verify_swift_error_value(f, entry, argument.as_erased().slot())?;
            }
        }
        for bb in f.basic_blocks() {
            let bb = bb.retag_termination::<Unterminated>();
            self.visit_block(f, &bb, &cx)?;
        }
        // `visit(const_cast<Function &>(F)); verifySiblingFuncletUnwinds();` —
        // `Verifier::verify(const Function &)` runs it immediately after the
        // instruction walk that fills `SiblingFuncletInfo`.
        self.verify_sibling_funclet_unwinds(f, &cx)?;
        Ok(())
    }

    fn verify_intrinsic_function(&self, f: FunctionValue<'ctx, Dyn, B>) -> IrResult<()> {
        let name = f.name();
        match crate::intrinsics::resolve_intrinsic_name(name) {
            IntrinsicNameResolution::NonIntrinsic => return Ok(()),
            IntrinsicNameResolution::UnknownIntrinsic => {
                return Err(IrError::UnknownIntrinsic {
                    name: name.to_owned(),
                });
            }
            IntrinsicNameResolution::Known(_) => {}
        }
        let descriptor =
            self.module
                .intrinsic_descriptor_from_signature::<B>(name, f.signature())
                .map_err(|err| match err {
                    IrError::UnknownIntrinsic { .. }
                    | IrError::IntrinsicSignatureMismatch { .. } => err,
                    _ => IrError::IntrinsicSignatureMismatch {
                        name: name.to_owned(),
                    },
                })?;
        if f.is_intrinsic() && f.intrinsic_descriptor().as_ref() != Some(&descriptor) {
            return Err(IrError::IntrinsicSignatureMismatch {
                name: name.to_owned(),
            });
        }
        // An intrinsic with a body used to be rejected here, as an
        // `InvalidOperation` reading `intrinsic functions should never be
        // defined`. That is upstream's rule with upstream's wording lost:
        // `Verifier::visitIntrinsicCall`'s first statement is
        // `Check(IF->isDeclaration(), "Intrinsic functions should never be
        // defined!", IF)`, raised per *call site* and carrying the capital and
        // the `!`. `check_intrinsic_call` carries it now, and this site — which
        // upstream's `visitFunction` has no counterpart for — is gone.
        let expected_attrs = descriptor
            .declaration_attributes(f.signature())
            .map_err(|err| match err {
                IrError::UnknownIntrinsic { .. } | IrError::IntrinsicSignatureMismatch { .. } => {
                    err
                }
                _ => IrError::IntrinsicSignatureMismatch {
                    name: name.to_owned(),
                },
            })?;
        let Some(actual_attrs) = self.function_attrs_with_groups(f) else {
            return Err(IrError::InvalidOperation {
                message: "intrinsic declaration modifier",
            });
        };
        if !expected_attrs.is_subset_of(&actual_attrs) {
            return Err(IrError::InvalidOperation {
                message: "intrinsic declaration modifier",
            });
        }
        let intrinsic_value = f.as_erased();
        for user in intrinsic_value.users() {
            let used_as_callee = match user.kind() {
                Some(InstructionKind::Call(call)) => call.callee().slot() == intrinsic_value.slot(),
                _ => match user.terminator_kind() {
                    Some(TerminatorKind::Invoke(invoke)) => {
                        invoke.callee().slot() == intrinsic_value.slot()
                    }
                    Some(TerminatorKind::CallBr(callbr)) => {
                        callbr.callee().slot() == intrinsic_value.slot()
                    }
                    _ => false,
                },
            };
            if !used_as_callee {
                return Err(IrError::InvalidOperation {
                    message: "intrinsic can only be used as callee",
                });
            }
        }
        Ok(())
    }

    fn function_attrs_with_groups(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
    ) -> Option<AttributeStorage> {
        let mut attrs = f.data().attributes.borrow().clone();
        for group in f.function_attr_groups() {
            let group_attrs = self.module.attribute_group(group)?;
            attrs.merge_from(&group_attrs);
        }
        Some(attrs)
    }

    fn visit_block(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        let instructions: Vec<InstructionView<'ctx, B>> = bb.instructions().collect();

        // Empty block is malformed (LLVM accepts `unreachable` as the
        // sole instruction; an empty list has no terminator at all).
        let Some(last) = instructions.last() else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::MissingTerminator,
                format!(
                    "Basic Block does not have terminator! (block {:?} has no instructions)",
                    bb.name().as_deref().unwrap_or("<anon>")
                ),
            ));
        };

        // Terminator placement: every non-last instruction must be a
        // non-terminator, the last instruction must be a terminator.
        // Mirrors the prologue of `Verifier::visitInstruction`.
        for (idx, inst) in instructions.iter().enumerate() {
            let is_last = idx + 1 == instructions.len();
            if inst.is_terminator() && !is_last {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MisplacedTerminator,
                    "Terminator found in the middle of a basic block!".into(),
                ));
            }
            if !inst.is_terminator() && is_last {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MissingTerminator,
                    "Basic Block does not have terminator!".into(),
                ));
            }
        }
        // Independently: there must be a terminator at the end. The
        // pair of checks above covers it but the explicit assertion
        // makes the intent obvious to readers and mirrors
        // `Verifier::visitBasicBlock`.
        let _ = last;

        // PHI grouping rule: phi nodes must come before any non-phi
        // instruction. Mirrors `Verifier::visitPHINode`'s
        // "PHI nodes not grouped at top of block" assertion.
        let mut seen_non_phi = false;
        for inst in &instructions {
            let is_phi = matches!(inst.kind(), Some(crate::InstructionKind::Phi(_)));
            if is_phi && seen_non_phi {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiNotAtTop,
                    "PHI nodes not grouped at top of basic block!".into(),
                ));
            }
            if !is_phi {
                seen_non_phi = true;
            }
        }

        // Per-instruction rules.
        for (idx, inst) in instructions.iter().enumerate() {
            self.visit_instruction(f, bb, inst, idx, &instructions, cx)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Per-instruction dispatch
    // ------------------------------------------------------------------

    fn visit_instruction(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        index_in_block: usize,
        block_instructions: &[InstructionView<'ctx, B>],
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // Per-opcode dispatch. Reaches into the storage payload
        // directly because every typed handle re-narrows the same
        // payload anyway; one match arm per opcode keeps the dispatch
        // table local.
        let kind = match &inst.as_erased().data().kind {
            ValueKindData::Instruction(i) => &i.kind,
            // Instruction's invariant (asserted at handle construction)
            // is that the value-kind is Instruction.
            _ => unreachable!("instruction handle invariant: value kind is Instruction"),
        };
        let opcode_result = match kind {
            // `Verifier::visitBinaryOperator`'s `switch (B.getOpcode())` has
            // four arms, each with its own pair of `Check` literals. The
            // integer arms share one routine here and differ only in which
            // pair they hand it.
            InstructionKindData::Add(b)
            | InstructionKindData::Sub(b)
            | InstructionKindData::Mul(b)
            | InstructionKindData::Udiv(b)
            | InstructionKindData::Sdiv(b)
            | InstructionKindData::Urem(b)
            | InstructionKindData::Srem(b) => self.check_int_binary(
                f,
                bb,
                inst,
                b,
                "Integer arithmetic operators only work with integral types!",
                "Integer arithmetic operators must have same type for operands and result!",
            ),
            InstructionKindData::Shl(b)
            | InstructionKindData::Lshr(b)
            | InstructionKindData::Ashr(b) => self.check_int_binary(
                f,
                bb,
                inst,
                b,
                "Shifts only work with integral types!",
                "Shift return type must be same as operands!",
            ),
            InstructionKindData::And(b)
            | InstructionKindData::Or(b)
            | InstructionKindData::Xor(b) => self.check_int_binary(
                f,
                bb,
                inst,
                b,
                "Logical operators only work with integral types!",
                "Logical operators must have same type for operands and result!",
            ),
            InstructionKindData::Fadd(b)
            | InstructionKindData::Fsub(b)
            | InstructionKindData::Fmul(b)
            | InstructionKindData::Fdiv(b)
            | InstructionKindData::Frem(b) => self.check_float_binary(f, bb, inst, b),
            InstructionKindData::Icmp(c) => self.check_icmp(f, bb, inst, c),
            InstructionKindData::Fcmp(c) => self.check_fcmp(f, bb, inst, c),
            InstructionKindData::Cast(c) => self.check_cast(f, bb, inst, c),
            InstructionKindData::Alloca(a) => self.check_alloca(f, bb, inst, a),
            InstructionKindData::Load(l) => self.check_load(f, bb, inst, l),
            InstructionKindData::Store(s) => self.check_store(f, bb, inst, s),
            InstructionKindData::Gep(g) => self.check_gep(f, bb, inst, g),
            InstructionKindData::Call(c) => self.check_call(
                f,
                bb,
                inst,
                c,
                BlockPosition {
                    index: index_in_block,
                    instructions: block_instructions,
                },
                cx,
            ),
            InstructionKindData::Select(s) => self.check_select(f, bb, inst, s),
            InstructionKindData::Phi(p) => self.check_phi(f, bb, inst, p, cx.predecessors),
            InstructionKindData::Ret(r) => self.check_ret(f, bb, inst, r),
            InstructionKindData::Br(b) => self.check_br(f, bb, inst, b, cx.block_index),
            InstructionKindData::Fneg(u) => self.check_fneg(f, bb, inst, u),
            InstructionKindData::Freeze(u) => self.check_freeze(f, bb, inst, u),
            InstructionKindData::VaArg(u) => self.check_va_arg(f, bb, inst, u),
            InstructionKindData::ExtractValue(d) => self.check_extract_value(f, bb, inst, d),
            InstructionKindData::InsertValue(d) => self.check_insert_value(f, bb, inst, d),
            InstructionKindData::ExtractElement(d) => self.check_extract_element(f, bb, inst, d),
            InstructionKindData::InsertElement(d) => self.check_insert_element(f, bb, inst, d),
            InstructionKindData::ShuffleVector(d) => self.check_shuffle_vector(f, bb, inst, d),
            InstructionKindData::Fence(d) => self.check_fence(f, bb, inst, d),
            InstructionKindData::AtomicCmpXchg(d) => self.check_cmpxchg(f, bb, inst, d),
            InstructionKindData::AtomicRmw(d) => self.check_atomicrmw(f, bb, inst, d),
            InstructionKindData::Switch(d) => self.check_switch(f, bb, inst, d, cx.block_index),
            InstructionKindData::IndirectBr(d) => {
                self.check_indirectbr(f, bb, inst, d, cx.block_index)
            }
            InstructionKindData::Invoke(d) => self.check_invoke(f, bb, inst, d, cx),
            InstructionKindData::CallBr(d) => self.check_callbr(f, bb, inst, d, cx),
            InstructionKindData::LandingPad(d) => self.check_landing_pad(f, bb, inst, d, cx),
            InstructionKindData::Resume(d) => self.check_resume(f, bb, d, cx),
            InstructionKindData::CleanupPad(d) => self.check_cleanup_pad(f, bb, inst, d, cx),
            InstructionKindData::CatchPad(d) => self.check_catch_pad(f, bb, inst, d, cx),
            InstructionKindData::CatchReturn(d) => self.check_catch_return(f, bb, d),
            InstructionKindData::CleanupReturn(d) => self.check_cleanup_return(f, bb, d),
            InstructionKindData::CatchSwitch(d) => self.check_catch_switch(f, bb, inst, d, cx),
            InstructionKindData::Unreachable(_) => Ok(()),
        };
        opcode_result?;

        // `visitInstruction(I)` is the **last** statement of every
        // `Verifier::visit*` method, not a prologue, so its universal
        // invariants are raised after the opcode's own:
        //   1. Self-reference -- only PHI may reference its own value.
        //   2. In-block use-before-def -- an operand whose defining
        //      instruction lives in the same block AND comes after
        //      the use is malformed.
        // The PHI exception lives where the storage payload is read
        // (we know the kind here, and PHI's "incoming" pairs are
        // semantically uses on predecessor edges, not at the phi).
        //
        // The order is observable here and not upstream: `CheckFailed`
        // accumulates, so upstream reports both a bad `deopt` bundle and the
        // dominance failure `test/Verifier/operand-bundles.ll`'s `@f_deopt`
        // carries, while llvmkit reports whichever comes first.
        self.check_self_reference_and_in_block_dom(
            f,
            bb,
            inst,
            index_in_block,
            block_instructions,
        )?;
        self.check_dominates_uses(f, bb, inst, cx.dom_tree)?;

        self.check_instruction_metadata(f, bb, inst, kind)
    }

    fn check_instruction_metadata(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        kind: &InstructionKindData,
    ) -> IrResult<()> {
        let Some(range_id) = inst.metadata_stored().get(&MetadataAttachmentKind::Range) else {
            return Ok(());
        };
        if !matches!(
            kind,
            InstructionKindData::Load(_)
                | InstructionKindData::Call(_)
                | InstructionKindData::Invoke(_)
        ) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::RangeMetadataInvalidAttachment,
                "Ranges are only for loads, calls and invokes!".to_string(),
            ));
        }
        self.verify_range_like_metadata_inst(
            f,
            bb,
            inst,
            range_id.slot(),
            scalar_type_id(self.module, inst.ty().id),
            RangeLikeMetadataKind::Range,
        )
    }

    fn verify_range_like_metadata_inst(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        id: MetadataSlot,
        expected_scalar_ty: TypeSlot,
        kind: RangeLikeMetadataKind,
    ) -> IrResult<()> {
        self.verify_range_like_metadata(id, expected_scalar_ty, kind, |rule, message| {
            self.fail(f, bb, rule, message)
        })
    }

    fn verify_range_like_metadata_global(
        &self,
        g: GlobalVariable<'ctx, B>,
        id: MetadataSlot,
        expected_scalar_ty: TypeSlot,
        kind: RangeLikeMetadataKind,
    ) -> IrResult<()> {
        self.verify_range_like_metadata(id, expected_scalar_ty, kind, |rule, message| {
            self.fail_global(g, rule, message)
        })
    }

    fn verify_range_like_metadata<F>(
        &self,
        id: MetadataSlot,
        expected_scalar_ty: TypeSlot,
        kind: RangeLikeMetadataKind,
        mut fail: F,
    ) -> IrResult<()>
    where
        F: FnMut(VerifierRule, String) -> IrError,
    {
        let store = self.module.metadata_store();
        // No upstream `Check` literal: `verifyRangeLikeMetadata` takes an
        // `MDNode *` and reads `getNumOperands()` directly, so a non-tuple
        // cannot reach it. llvmkit's store can hold one, so this guard exists
        // and keeps llvmkit's own wording.
        let Some(MetadataKind::Tuple { operands, .. }) = store.get(id) else {
            return Err(fail(
                VerifierRule::RangeMetadataMalformed,
                "range metadata must be a tuple".to_string(),
            ));
        };
        if operands.len() % 2 != 0 {
            return Err(fail(
                VerifierRule::RangeMetadataMalformed,
                "Unfinished range!".to_string(),
            ));
        }
        let num_ranges = operands.len() / 2;
        if num_ranges == 0 {
            return Err(fail(
                VerifierRule::RangeMetadataMalformed,
                "It should have at least one range!".to_string(),
            ));
        }

        let mut first_range = None;
        let mut last_range = None;
        for (idx, pair) in operands.chunks_exact(2).enumerate() {
            let Some((low_ty, low)) = metadata_constant_int(self.module, &store, pair[0].slot())
            else {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "The lower limit must be an integer!".to_string(),
                ));
            };
            let Some((high_ty, high)) = metadata_constant_int(self.module, &store, pair[1].slot())
            else {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "The upper limit must be an integer!".to_string(),
                ));
            };
            if high_ty != low_ty {
                return Err(fail(
                    VerifierRule::RangeMetadataTypeMismatch,
                    "Range pair types must match!".to_string(),
                ));
            }
            if high_ty != expected_scalar_ty {
                return Err(fail(
                    VerifierRule::RangeMetadataTypeMismatch,
                    "Range types must match instruction type!".to_string(),
                ));
            }
            if low.eq_ap_int(&high) && !low.is_max_value() && !low.is_min_value() {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "The upper and lower limits cannot be the same value".to_string(),
                ));
            }
            // No upstream `Check` literal: `ConstantRange`'s constructor
            // asserts equal bit widths, which the two type checks above have
            // already established, so this arm carries `ConstantRange`'s own
            // error text rather than a `Verifier` string.
            let range = ConstantRange::new(low.clone(), high.clone())
                .map_err(|err| fail(VerifierRule::RangeMetadataTypeMismatch, err.to_string()))?;
            if range.is_empty_set() || (kind == RangeLikeMetadataKind::Range && range.is_full_set())
            {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "Range must not be empty!".to_string(),
                ));
            }
            if let Some(prev) = &last_range {
                if range.intersects_with(prev) {
                    return Err(fail(
                        VerifierRule::RangeMetadataOverlapping,
                        "Intervals are overlapping".to_string(),
                    ));
                }
                if !low.sgt(prev.lower()) {
                    return Err(fail(
                        VerifierRule::RangeMetadataOutOfOrder,
                        "Intervals are not in order".to_string(),
                    ));
                }
                if range.is_contiguous_with(prev) {
                    return Err(fail(
                        VerifierRule::RangeMetadataContiguous,
                        "Intervals are contiguous".to_string(),
                    ));
                }
            }
            if idx == 0 {
                first_range = Some(range.clone());
            }
            last_range = Some(range);
        }
        if num_ranges > 2
            && let (Some(first), Some(last)) = (&first_range, &last_range)
        {
            if first.intersects_with(last) {
                return Err(fail(
                    VerifierRule::RangeMetadataOverlapping,
                    "Intervals are overlapping".to_string(),
                ));
            }
            if first.is_contiguous_with(last) {
                return Err(fail(
                    VerifierRule::RangeMetadataContiguous,
                    "Intervals are contiguous".to_string(),
                ));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Per-opcode checks
    // ------------------------------------------------------------------

    /// `Verifier::visitBinaryOperator` -- integer flavor.
    /// `add`/`sub`/`mul`/`udiv`/`sdiv`/`urem`/`srem`/`shl`/`lshr`/`ashr`/
    /// `and`/`or`/`xor`.
    ///
    /// `operand_kind_message` and `same_type_message` are the two `Check`
    /// literals of the caller's arm of upstream's `switch (B.getOpcode())`;
    /// see [`Self::visit_instruction`]'s dispatch.
    fn check_int_binary(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        b: &BinaryOpData,
        operand_kind_message: &str,
        same_type_message: &str,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(b.lhs.get());
        let rhs_ty = self.value_type(b.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryOperandsTypeMismatch,
                format!(
                    "Both operands to a binary operator are not of the same type! (lhs is {} but rhs is {})",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_int_or_int_vector(self.module, lhs_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IntegerOpNonIntegerOperand,
                format!(
                    "{operand_kind_message} (operand type {})",
                    self.type_label(lhs_ty)
                ),
            ));
        }
        if inst.ty().id != lhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryResultTypeMismatch,
                format!(
                    "{same_type_message} (result {} != operand {})",
                    self.type_label(inst.ty().id),
                    self.type_label(lhs_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitBinaryOperator` -- float flavor.
    /// `fadd`/`fsub`/`fmul`/`fdiv`/`frem`.
    fn check_float_binary(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        b: &BinaryOpData,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(b.lhs.get());
        let rhs_ty = self.value_type(b.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryOperandsTypeMismatch,
                format!(
                    "Both operands to a binary operator are not of the same type! (lhs is {} but rhs is {})",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_fp_or_fp_vector(self.module, lhs_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FloatOpNonFloatOperand,
                format!(
                    "Floating-point arithmetic operators only work with floating-point types! (operand type {})",
                    self.type_label(lhs_ty)
                ),
            ));
        }
        if inst.ty().id != lhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryResultTypeMismatch,
                format!(
                    "Floating-point arithmetic operators must have same type for operands and result! (result {} != operand {})",
                    self.type_label(inst.ty().id),
                    self.type_label(lhs_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitUnaryOperator`, whose only opcode is `fneg`: the
    /// same-type `Check` runs *before* the `switch`, so it is first here too.
    ///
    /// `Unary operators must have same type foroperands and result!` is
    /// upstream's literal, missing space and all — the two adjacent string
    /// literals it concatenates have no separator.
    fn check_fneg(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        u: &FnegInstData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(u.src.get());
        if inst.ty().id != src_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FnegTypeMismatch,
                format!(
                    "Unary operators must have same type foroperands and result! (result {} != operand {})",
                    self.type_label(inst.ty().id),
                    self.type_label(src_ty)
                ),
            ));
        }
        if !is_fp_or_fp_vector(self.module, src_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FnegTypeMismatch,
                format!(
                    "FNeg operator only works with float types! (operand type {})",
                    self.type_label(src_ty)
                ),
            ));
        }
        Ok(())
    }

    /// The result type must match the operand type. Operand type is otherwise
    /// unconstrained (LangRef permits any first-class type except aggregates
    /// of tokens).
    ///
    /// **No upstream counterpart.** `Verifier` has no `visitFreeze`, so this
    /// rule has no `Check` literal to carry and keeps llvmkit's own wording —
    /// `grep -c 'Freeze' llvm/lib/IR/Verifier.cpp` is 0 at `llvmorg-22.1.4`.
    fn check_freeze(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        u: &FreezeInstData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(u.src.get());
        if inst.ty().id != src_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FreezeTypeMismatch,
                format!(
                    "result {} != operand {}",
                    self.type_label(inst.ty().id),
                    self.type_label(src_ty)
                ),
            ));
        }
        Ok(())
    }

    /// The source operand must be a pointer to a `va_list`; the destination
    /// type is independent.
    ///
    /// **No upstream counterpart.** `Verifier::visitVAArgInst` is declared
    /// inline as `{ visitInstruction(VAA); }` and carries no `Check`, so this
    /// rule has no literal to reproduce and keeps llvmkit's own wording.
    fn check_va_arg(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        u: &VaArgInstData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(u.src.get());
        if !self.module.context().type_data(src_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VaArgNonPointerOperand,
                format!("va_arg source {} is not a pointer", self.type_label(src_ty)),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitExtractValueInst`. Walks the aggregate type by
    /// the index list and checks the leaf matches the result.
    fn check_extract_value(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &ExtractValueInstData,
    ) -> IrResult<()> {
        let agg_ty = self.value_type(d.aggregate.get());
        let leaf_ty =
            walk_aggregate_path(self.module, agg_ty, &d.indices).map_err(|e| match e {
                AggWalkErr::NotAggregate(at) => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateOpNonAggregate,
                    format!(
                        "Invalid ExtractValueInst operands! (operand type {} is not aggregate)",
                        self.type_label(at)
                    ),
                ),
                AggWalkErr::OutOfRange { idx, count } => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateIndexOutOfRange,
                    format!("Invalid ExtractValueInst operands! (index {idx} >= {count})"),
                ),
            })?;
        if inst.ty().id != leaf_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AggregateOpNonAggregate,
                format!(
                    "Invalid ExtractValueInst operands! (result {} != leaf {})",
                    self.type_label(inst.ty().id),
                    self.type_label(leaf_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitInsertValueInst`.
    fn check_insert_value(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &InsertValueInstData,
    ) -> IrResult<()> {
        let agg_ty = self.value_type(d.aggregate.get());
        let val_ty = self.value_type(d.value.get());
        let leaf_ty =
            walk_aggregate_path(self.module, agg_ty, &d.indices).map_err(|e| match e {
                AggWalkErr::NotAggregate(at) => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateOpNonAggregate,
                    format!(
                        "Invalid InsertValueInst operands! (operand type {} is not aggregate)",
                        self.type_label(at)
                    ),
                ),
                AggWalkErr::OutOfRange { idx, count } => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateIndexOutOfRange,
                    format!("Invalid InsertValueInst operands! (index {idx} >= {count})"),
                ),
            })?;
        if val_ty != leaf_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::InsertValueLeafTypeMismatch,
                format!(
                    "Invalid InsertValueInst operands! (inserted value {} != leaf {})",
                    self.type_label(val_ty),
                    self.type_label(leaf_ty)
                ),
            ));
        }
        if inst.ty().id != agg_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::InsertValueLeafTypeMismatch,
                format!(
                    "Invalid InsertValueInst operands! (result {} != aggregate {})",
                    self.type_label(inst.ty().id),
                    self.type_label(agg_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitExtractElementInst`. Vector operand element type
    /// must equal the result type; the index must be integer-typed.
    fn check_extract_element(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &ExtractElementInstData,
    ) -> IrResult<()> {
        let vec_ty = self.value_type(d.vector.get());
        let idx_ty = self.value_type(d.index.get());
        let elem = match self.module.context().type_data(vec_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::VectorElementOpTypeMismatch,
                    format!(
                        "Invalid extractelement operands! (vector operand {} is not a vector)",
                        self.type_label(vec_ty)
                    ),
                ));
            }
        };
        if self
            .module
            .context()
            .type_data(idx_ty)
            .as_integer()
            .is_none()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "Invalid extractelement operands! (index {} is not an integer)",
                    self.type_label(idx_ty)
                ),
            ));
        }
        if inst.ty().id != elem {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "Invalid extractelement operands! (result {} != element {})",
                    self.type_label(inst.ty().id),
                    self.type_label(elem)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitInsertElementInst`.
    fn check_insert_element(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &InsertElementInstData,
    ) -> IrResult<()> {
        let vec_ty = self.value_type(d.vector.get());
        let val_ty = self.value_type(d.value.get());
        let idx_ty = self.value_type(d.index.get());
        let elem = match self.module.context().type_data(vec_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::VectorElementOpTypeMismatch,
                    format!(
                        "Invalid insertelement operands! (vector operand {} is not a vector)",
                        self.type_label(vec_ty)
                    ),
                ));
            }
        };
        if val_ty != elem {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "Invalid insertelement operands! (inserted value {} != element {})",
                    self.type_label(val_ty),
                    self.type_label(elem)
                ),
            ));
        }
        if self
            .module
            .context()
            .type_data(idx_ty)
            .as_integer()
            .is_none()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "Invalid insertelement operands! (index {} is not an integer)",
                    self.type_label(idx_ty)
                ),
            ));
        }
        if inst.ty().id != vec_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "Invalid insertelement operands! (result {} != vector {})",
                    self.type_label(inst.ty().id),
                    self.type_label(vec_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitShuffleVectorInst`, whose entire body is one
    /// `Check(ShuffleVectorInst::isValidOperands(SV.getOperand(0),
    /// SV.getOperand(1), SV.getShuffleMask()), "Invalid shufflevector
    /// operands!", &SV)`.
    ///
    /// The checks after it have no upstream counterpart and are defence in
    /// depth: upstream's result type is computed by the constructor and cannot
    /// disagree with the operands, while llvmkit's arena can hold an
    /// instruction whose recorded result type does.
    fn check_shuffle_vector(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &ShuffleVectorInstData,
    ) -> IrResult<()> {
        let lhs: Value<'ctx, B> =
            Value::from_parts(d.lhs.get(), self.module, self.value_type(d.lhs.get()));
        let rhs: Value<'ctx, B> =
            Value::from_parts(d.rhs.get(), self.module, self.value_type(d.rhs.get()));
        if !ShuffleVectorInst::is_valid_operands(lhs, rhs, &d.mask) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::ShuffleVectorTypeMismatch,
                "Invalid shufflevector operands!".to_string(),
            ));
        }

        let l_ty = self.value_type(d.lhs.get());
        let r_ty = self.value_type(d.rhs.get());
        let l_elem = match self.module.context().type_data(l_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::ShuffleVectorTypeMismatch,
                    format!(
                        "Invalid shufflevector operands! (lhs {} is not a vector)",
                        self.type_label(l_ty)
                    ),
                ));
            }
        };
        let r_elem = match self.module.context().type_data(r_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::ShuffleVectorTypeMismatch,
                    format!(
                        "Invalid shufflevector operands! (rhs {} is not a vector)",
                        self.type_label(r_ty)
                    ),
                ));
            }
        };
        if l_elem != r_elem {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::ShuffleVectorTypeMismatch,
                format!(
                    "Invalid shufflevector operands! (lhs element {} != rhs element {})",
                    self.type_label(l_elem),
                    self.type_label(r_elem)
                ),
            ));
        }
        // Result type element should equal the operand element; result
        // length should equal mask length. We compare via vector data.
        match self.module.context().type_data(inst.ty().id).as_vector() {
            Some((re, n, _)) => {
                let Ok(result_len) = usize::try_from(n) else {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::ShuffleVectorTypeMismatch,
                        "Invalid shufflevector operands! (result vector length does not fit \
                         this host)"
                            .to_string(),
                    ));
                };
                if re != l_elem || result_len != d.mask.len() {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::ShuffleVectorTypeMismatch,
                        "Invalid shufflevector operands! (result vector shape disagrees with \
                         operands or mask length)"
                            .to_string(),
                    ));
                }
            }
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::ShuffleVectorTypeMismatch,
                    format!(
                        "Invalid shufflevector operands! (result {} is not a vector)",
                        self.type_label(inst.ty().id)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// `Verifier::visitFenceInst`. The ordering must be one of
    /// `acquire`/`release`/`acq_rel`/`seq_cst`.
    fn check_fence(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        d: &FenceInstData,
    ) -> IrResult<()> {
        use crate::atomic_ordering::AtomicOrdering as AO;
        if !matches!(
            d.ordering,
            AO::Acquire | AO::Release | AO::AcquireRelease | AO::SequentiallyConsistent
        ) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!(
                    "fence instructions may only have acquire, release, acq_rel, or seq_cst \
                     ordering. (got {})",
                    d.ordering
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitAtomicCmpXchgInst`, preceded by the `AtomicCmpXchgInst`
    /// construction-time `assert`s llvmkit raises as verifier failures.
    ///
    /// The first four checks — pointer operand is a pointer, cmp / new value
    /// types match, both orderings are at least monotonic, the failure
    /// ordering is not Release / AcqRel — have **no upstream `Check`
    /// literal**: they are `assert`s inside `AtomicCmpXchgInst::Init` and
    /// `setFailureOrdering`, which run at construction and so precede
    /// `visitAtomicCmpXchgInst`. Production paths here do not panic, so they
    /// keep llvmkit's own wording.
    ///
    /// The two statements of `visitAtomicCmpXchgInst` itself follow, in its
    /// order: `Check(ElTy->isIntOrPtrTy(), …)` on `getOperand(1)` — the `cmp`
    /// operand — then `checkAtomicMemAccessSize(ElTy, &CXI)`.
    fn check_cmpxchg(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        d: &AtomicCmpXchgInstData,
    ) -> IrResult<()> {
        use crate::atomic_ordering::AtomicOrdering as AO;
        let ptr_ty = self.value_type(d.ptr.get());
        if !self.module.context().type_data(ptr_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicNonPointerOperand,
                format!(
                    "cmpxchg pointer {} is not a pointer",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let cmp_ty = self.value_type(d.cmp.get());
        let new_ty = self.value_type(d.new_val.get());
        if cmp_ty != new_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicRmwOperandTypeMismatch,
                format!(
                    "cmpxchg cmp {} != new {}",
                    self.type_label(cmp_ty),
                    self.type_label(new_ty)
                ),
            ));
        }
        let strong_enough = |o: AO| {
            matches!(
                o,
                AO::Monotonic
                    | AO::Acquire
                    | AO::Release
                    | AO::AcquireRelease
                    | AO::SequentiallyConsistent
            )
        };
        if !strong_enough(d.success_ordering) || !strong_enough(d.failure_ordering) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!(
                    "cmpxchg orderings ({}, {}) must be at least monotonic",
                    d.success_ordering, d.failure_ordering
                ),
            ));
        }
        if matches!(d.failure_ordering, AO::Release | AO::AcquireRelease) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!(
                    "cmpxchg failure ordering {} cannot be Release/AcqRel",
                    d.failure_ordering
                ),
            ));
        }
        // `Type *ElTy = CXI.getOperand(1)->getType();` — operand 1 is `cmp`.
        let el_ty = cmp_ty;
        // `Check(ElTy->isIntOrPtrTy(), "cmpxchg operand must have integer or
        //  pointer type", ElTy, &CXI);`
        //
        // `Type::isIntOrPtrTy` is `isIntegerTy() || isPointerTy()` — scalars
        // only, which is narrower than `check_atomic_access_type`'s
        // load/store predicate (that one also admits floating-point and
        // vectors).
        let el_data = self.module.context().type_data(el_ty);
        if !(el_data.as_integer().is_some() || el_data.is_pointer_data()) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicCmpXchgInvalidOperandType,
                format!(
                    "cmpxchg operand must have integer or pointer type (got {})",
                    self.type_label(el_ty)
                ),
            ));
        }
        // `checkAtomicMemAccessSize(ElTy, &CXI);`
        self.check_atomic_access_size(f, bb, el_ty)?;
        Ok(())
    }

    /// `Verifier::visitAtomicRMWInst`.
    ///
    /// The pointer and result-type checks have no upstream `Check` literal —
    /// `AtomicRMWInst::Init` asserts them — so they keep llvmkit's wording.
    fn check_atomicrmw(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &AtomicRmwInstData,
    ) -> IrResult<()> {
        use crate::atomic_ordering::AtomicOrdering as AO;
        let ptr_ty = self.value_type(d.ptr.get());
        if !self.module.context().type_data(ptr_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicNonPointerOperand,
                format!(
                    "atomicrmw pointer {} is not a pointer",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let val_ty = self.value_type(d.value.get());
        if d.op.is_fp_operation() && !is_fp_or_fp_vector(self.module, val_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicRmwOperandTypeMismatch,
                format!(
                    "atomicrmw {} operand must have floating-point or fixed vector of \
                     floating-point type! (got {})",
                    d.op.keyword(),
                    self.type_label(val_ty)
                ),
            ));
        }
        if !matches!(
            d.ordering,
            AO::Monotonic
                | AO::Acquire
                | AO::Release
                | AO::AcquireRelease
                | AO::SequentiallyConsistent
        ) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!(
                    "atomicrmw instructions cannot be unordered. (got {})",
                    d.ordering
                ),
            ));
        }
        if inst.ty().id != val_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicRmwOperandTypeMismatch,
                format!(
                    "atomicrmw result {} != value {}",
                    self.type_label(inst.ty().id),
                    self.type_label(val_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitICmpInst`.
    fn check_icmp(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        c: &CmpInstData,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(c.lhs.get());
        let rhs_ty = self.value_type(c.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IcmpOperandTypeMismatch,
                format!(
                    "Both operands to ICmp instruction are not of the same type! (lhs {} differs from rhs {})",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_int_or_int_vector(self.module, lhs_ty)
            && !is_pointer_or_pointer_vector(self.module, lhs_ty)
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IcmpOperandTypeMismatch,
                format!(
                    "Invalid operand types for ICmp instruction (operand type {} is neither integer nor pointer)",
                    self.type_label(lhs_ty)
                ),
            ));
        }
        // Result type must be i1 (or vector of i1 for vector compares).
        // Predicate is statically a valid IntPredicate; nothing extra
        // to assert beyond the type-level guarantee.
        //
        // No upstream `Check` literal: `CmpInst::Create` builds the result
        // type, so upstream has nothing to verify and llvmkit's arena-level
        // guard keeps its own wording. The same holds in `check_fcmp`.
        let _ = c.predicate;
        let res = inst.ty();
        let res_ok = is_i1(self.module, res.id) || is_i1_vector(self.module, res.id);
        if !res_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IcmpOperandTypeMismatch,
                format!("icmp result type {} is not i1", self.type_label(res.id)),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitFCmpInst`.
    fn check_fcmp(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        c: &FcmpInstData,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(c.lhs.get());
        let rhs_ty = self.value_type(c.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FcmpOperandTypeMismatch,
                format!(
                    "Both operands to FCmp instruction are not of the same type! (lhs {} differs from rhs {})",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_fp_or_fp_vector(self.module, lhs_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FcmpOperandTypeMismatch,
                format!(
                    "Invalid operand types for FCmp instruction (operand type {} is not floating-point)",
                    self.type_label(lhs_ty)
                ),
            ));
        }
        let res_ok = is_i1(self.module, inst.ty().id) || is_i1_vector(self.module, inst.ty().id);
        if !res_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FcmpOperandTypeMismatch,
                format!(
                    "fcmp result type {} is not i1",
                    self.type_label(inst.ty().id)
                ),
            ));
        }
        Ok(())
    }

    /// Cast opcodes. Mirrors the per-opcode `Verifier::visit{Cast}Inst`
    /// family.
    fn check_cast(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        c: &CastOpData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(c.src.get());
        let dst_ty = inst.ty().id;
        match c.kind {
            CastOpcode::Trunc | CastOpcode::Zext | CastOpcode::Sext => {
                // `CastInst::castIsValid` compares `getScalarSizeInBits`, so a
                // vector is checked on its element and separately on its
                // shape: both sides vectors of equal element count, or both
                // scalars.
                //
                // The four literals are `visitTruncInst` / `visitZExtInst` /
                // `visitSExtInst`'s, in their order. The `produces` verb is
                // upstream's own: `Trunc only produces integer` has no article
                // where the other two say `an integer`.
                let (source_message, result_message, shape_message, width_message) = match c.kind {
                    CastOpcode::Trunc => (
                        "Trunc only operates on integer",
                        "Trunc only produces integer",
                        "trunc source and destination must both be a vector or neither",
                        "DestTy too big for Trunc",
                    ),
                    CastOpcode::Zext => (
                        "ZExt only operates on integer",
                        "ZExt only produces an integer",
                        "zext source and destination must both be a vector or neither",
                        "Type too small for ZExt",
                    ),
                    _ => (
                        "SExt only operates on integer",
                        "SExt only produces an integer",
                        "sext source and destination must both be a vector or neither",
                        "Type too small for SExt",
                    ),
                };
                let src_w = self.scalar_int_width_or_err(f, bb, src_ty, source_message)?;
                let dst_w = self.scalar_int_width_or_err(f, bb, dst_ty, result_message)?;
                let src_shape = vector_shape(self.module, src_ty);
                let dst_shape = vector_shape(self.module, dst_ty);
                if src_shape != dst_shape {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "{shape_message} ({} from {} to {})",
                            c.kind.keyword(),
                            self.type_label(src_ty),
                            self.type_label(dst_ty)
                        ),
                    ));
                }
                let ok = match c.kind {
                    CastOpcode::Trunc => dst_w < src_w,
                    _ => dst_w > src_w,
                };
                if !ok {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastWidthMismatch,
                        format!(
                            "{width_message} ({} from {} to {})",
                            c.kind.keyword(),
                            self.type_label(src_ty),
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
            CastOpcode::FpTrunc | CastOpcode::FpExt => {
                let (source_message, result_message, width_message) = match c.kind {
                    CastOpcode::FpTrunc => (
                        "FPTrunc only operates on FP",
                        "FPTrunc only produces an FP",
                        "DestTy too big for FPTrunc",
                    ),
                    _ => (
                        "FPExt only operates on FP",
                        "FPExt only produces an FP",
                        "DestTy too small for FPExt",
                    ),
                };
                let Some(s) = fp_rank(self.module, src_ty) else {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{source_message} (got {})", self.type_label(src_ty)),
                    ));
                };
                let Some(d) = fp_rank(self.module, dst_ty) else {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{result_message} (got {})", self.type_label(dst_ty)),
                    ));
                };
                let ok = match c.kind {
                    CastOpcode::FpTrunc => d < s,
                    _ => d > s,
                };
                if !ok {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastWidthMismatch,
                        format!(
                            "{width_message} ({} from {} to {})",
                            c.kind.keyword(),
                            self.type_label(src_ty),
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
            CastOpcode::FpToUi | CastOpcode::FpToSi => {
                let (source_message, result_message) = match c.kind {
                    CastOpcode::FpToUi => (
                        "FPToUI source must be FP or FP vector",
                        "FPToUI result must be integer or integer vector",
                    ),
                    _ => (
                        "FPToSI source must be FP or FP vector",
                        "FPToSI result must be integer or integer vector",
                    ),
                };
                if !is_fp_or_fp_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{source_message} (got {})", self.type_label(src_ty)),
                    ));
                }
                if !is_int_or_int_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{result_message} (got {})", self.type_label(dst_ty)),
                    ));
                }
            }
            CastOpcode::UiToFp | CastOpcode::SiToFp => {
                let (source_message, result_message) = match c.kind {
                    CastOpcode::UiToFp => (
                        "UIToFP source must be integer or integer vector",
                        "UIToFP result must be FP or FP vector",
                    ),
                    _ => (
                        "SIToFP source must be integer or integer vector",
                        "SIToFP result must be FP or FP vector",
                    ),
                };
                if !is_int_or_int_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{source_message} (got {})", self.type_label(src_ty)),
                    ));
                }
                if !is_fp_or_fp_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{result_message} (got {})", self.type_label(dst_ty)),
                    ));
                }
            }
            CastOpcode::PtrToAddr | CastOpcode::PtrToInt => {
                let (source_message, result_message) = match c.kind {
                    CastOpcode::PtrToAddr => (
                        "PtrToAddr source must be pointer",
                        "PtrToAddr result must be integral",
                    ),
                    _ => (
                        "PtrToInt source must be pointer",
                        "PtrToInt result must be integral",
                    ),
                };
                if !is_pointer_or_pointer_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{source_message} (got {})", self.type_label(src_ty)),
                    ));
                }
                if !is_int_or_int_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!("{result_message} (got {})", self.type_label(dst_ty)),
                    ));
                }
                if c.kind == CastOpcode::PtrToAddr {
                    let Some((addr_space, src_shape)) = pointer_source_shape(self.module, src_ty)
                    else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            "PtrToAddr source must be pointer".to_owned(),
                        ));
                    };
                    let Some((dst_bits, dst_shape)) = integer_result_shape(self.module, dst_ty)
                    else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            "PtrToAddr result must be integral".to_owned(),
                        ));
                    };
                    let index_bits = self.module.data_layout().index_size_in_bits(addr_space);
                    if dst_bits != index_bits || src_shape != dst_shape {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            "PtrToAddr result must be address width".to_owned(),
                        ));
                    }
                }
            }
            CastOpcode::IntToPtr => {
                if !is_int_or_int_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "IntToPtr source must be an integral (got {})",
                            self.type_label(src_ty)
                        ),
                    ));
                }
                if !is_pointer_or_pointer_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "IntToPtr result must be a pointer (got {})",
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
            CastOpcode::BitCast => {
                // Bitcast must preserve bit width. Pointer-to-pointer
                // bitcasts in the same address space are identity in
                // LLVM 17+ (opaque pointers); LangRef accepts them.
                // Cross-address-space pointer reinterpretation must use
                // `addrspacecast` instead.
                let src_data = self.module.context().type_data(src_ty);
                let dst_data = self.module.context().type_data(dst_ty);
                if src_ty == dst_ty {
                    // Identity is always fine.
                } else if let (Some(src_as), Some(dst_as)) =
                    (src_data.as_pointer(), dst_data.as_pointer())
                {
                    if src_as != dst_as {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            format!(
                                "Invalid bitcast (across address spaces {src_as} -> {dst_as}; use addrspacecast)"
                            ),
                        ));
                    }
                } else {
                    let src_bits = type_bit_width(self.module, src_ty);
                    let dst_bits = type_bit_width(self.module, dst_ty);
                    match (src_bits, dst_bits) {
                        (Some(s), Some(d)) if s == d => {}
                        (Some(s), Some(d)) => {
                            return Err(self.fail(
                                f,
                                bb,
                                VerifierRule::BitCastSizeMismatch,
                                format!("Invalid bitcast ({s}-bit -> {d}-bit)"),
                            ));
                        }
                        _ => {
                            return Err(self.fail(
                                f,
                                bb,
                                VerifierRule::CastTypeMismatch,
                                format!(
                                    "Invalid bitcast (requires sized scalar/vector/pointer types; got {} -> {})",
                                    self.type_label(src_ty),
                                    self.type_label(dst_ty)
                                ),
                            ));
                        }
                    }
                }
            }
            CastOpcode::AddrSpaceCast => {
                if !is_pointer_or_pointer_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "AddrSpaceCast source must be a pointer (got {})",
                            self.type_label(src_ty)
                        ),
                    ));
                }
                if !is_pointer_or_pointer_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "AddrSpaceCast result must be a pointer (got {})",
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// `Verifier::visitAllocaInst`.
    fn check_alloca(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        a: &AllocaInstData,
    ) -> IrResult<()> {
        let allocated = Type::<B>::new(a.allocated_ty, self.module);
        if SizedType::try_from(allocated).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AllocaUnsizedType,
                format!(
                    "Cannot allocate unsized type (allocated type {})",
                    self.type_label(a.allocated_ty)
                ),
            ));
        }
        if let Some(count_id) = a.num_elements.get() {
            let count_ty = self.value_type(count_id);
            if !is_int_or_int_vector(self.module, count_ty) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::AllocaNonIntegerCount,
                    format!(
                        "Alloca array size must have integer type (got {})",
                        self.type_label(count_ty)
                    ),
                ));
            }
        }
        // Verifier.cpp `visitAllocaInst`: a swifterror alloca must have
        // pointer type and must not be an array allocation.
        if a.flags.is_swifterror() {
            if !self
                .module
                .context()
                .type_data(a.allocated_ty)
                .is_pointer_data()
            {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SwiftErrorAlloca,
                    format!(
                        "swifterror alloca must have pointer type (got {})",
                        self.type_label(a.allocated_ty)
                    ),
                ));
            }
            // `isArrayAllocation()` is false for an omitted size or a
            // constant-`1` size, so those are permitted.
            if a.num_elements
                .get()
                .is_some_and(|count| !crate::constants::is_constant_int_one(self.module, count))
            {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SwiftErrorAlloca,
                    "swifterror alloca must not be array allocation".to_owned(),
                ));
            }
            // `verifySwiftErrorValue(&AI);`
            self.verify_swift_error_value(f, bb, inst.slot())?;
        }
        // Result type must be a pointer; the IrBuilder construction
        // path always emits one, but assert it for parsed/foreign IR.
        //
        // No upstream `Check` literal: `AllocaInst`'s result type is built by
        // its constructor, so this guard is llvmkit's own and keeps its own
        // wording.
        if !self
            .module
            .context()
            .type_data(inst.ty().id)
            .is_pointer_data()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AllocaUnsizedType,
                format!(
                    "alloca result type {} is not a pointer",
                    self.type_label(inst.ty().id)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitLoadInst`.
    fn check_load(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        l: &LoadInstData,
    ) -> IrResult<()> {
        let ptr_ty = self.value_type(l.ptr.get());
        if !is_pointer_or_pointer_vector(self.module, ptr_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::LoadNonPointer,
                format!(
                    "Load operand must be a pointer. (got {})",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let pointee = Type::<B>::new(l.pointee_ty, self.module);
        if SizedType::try_from(pointee).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::LoadUnsizedType,
                format!(
                    "loading unsized types is not allowed (pointee type {})",
                    self.type_label(l.pointee_ty)
                ),
            ));
        }
        // Result type must equal pointee type. No upstream `Check` literal:
        // `LoadInst`'s result type *is* the pointee upstream, so there is
        // nothing to compare; this guard is llvmkit's own.
        if inst.ty().id != l.pointee_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::LoadUnsizedType,
                format!(
                    "load result type {} != pointee {}",
                    self.type_label(inst.ty().id),
                    self.type_label(l.pointee_ty)
                ),
            ));
        }
        // Atomic-specific rules. Mirrors `Verifier::visitLoadInst`.
        if l.is_atomic() {
            use crate::atomic_ordering::AtomicOrdering;
            if matches!(
                l.ordering,
                AtomicOrdering::Release | AtomicOrdering::AcquireRelease
            ) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::AtomicLoadInvalidOrdering,
                    format!("Load cannot have Release ordering (got {})", l.ordering),
                ));
            }
            self.check_atomic_access_type(
                f,
                bb,
                l.pointee_ty,
                "atomic load operand must have integer, pointer, floating point, or vector type!",
            )?;
            self.check_atomic_access_size(f, bb, l.pointee_ty)?;
        } else if !l.sync_scope.is_default() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::NonAtomicWithSyncScope,
                "Non-atomic load cannot have SynchronizationScope specified".to_string(),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitStoreInst`.
    fn check_store(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        s: &StoreInstData,
    ) -> IrResult<()> {
        let ptr_ty = self.value_type(s.ptr.get());
        if !is_pointer_or_pointer_vector(self.module, ptr_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::StoreNonPointer,
                format!(
                    "Store operand must be a pointer. (got {})",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let val_ty = self.value_type(s.value.get());
        if SizedType::try_from(Type::<B>::new(val_ty, self.module)).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::StoreUnsizedType,
                format!(
                    "storing unsized types is not allowed (value type {})",
                    self.type_label(val_ty)
                ),
            ));
        }
        // Atomic-specific rules. Mirrors `Verifier::visitStoreInst`.
        if s.is_atomic() {
            use crate::atomic_ordering::AtomicOrdering;
            if matches!(
                s.ordering,
                AtomicOrdering::Acquire | AtomicOrdering::AcquireRelease
            ) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::AtomicStoreInvalidOrdering,
                    format!("Store cannot have Acquire ordering (got {})", s.ordering),
                ));
            }
            self.check_atomic_access_type(
                f,
                bb,
                val_ty,
                "atomic store operand must have integer, pointer, floating point, or vector type!",
            )?;
            self.check_atomic_access_size(f, bb, val_ty)?;
        } else if !s.sync_scope.is_default() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::NonAtomicWithSyncScope,
                "Non-atomic store cannot have SynchronizationScope specified".to_string(),
            ));
        }
        Ok(())
    }

    /// Mirrors `Verifier::visitLoadInst` / `visitStoreInst` operand-type
    /// branch: atomic load/store operands must be integer, pointer,
    /// floating-point, or a vector thereof.
    ///
    /// `message` is the caller's `Check` literal — the two differ only in the
    /// `load` / `store` noun.
    fn check_atomic_access_type(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        ty: TypeSlot,
        message: &str,
    ) -> IrResult<()> {
        if is_int_or_int_vector(self.module, ty)
            || is_fp_or_fp_vector(self.module, ty)
            || is_pointer_or_pointer_vector(self.module, ty)
        {
            return Ok(());
        }
        Err(self.fail(
            f,
            bb,
            VerifierRule::AtomicLoadStoreInvalidType,
            format!("{message} (got {})", self.type_label(ty)),
        ))
    }

    /// Mirrors `Verifier::checkAtomicMemAccessSize` in `lib/IR/Verifier.cpp`,
    /// whose two `Check`s are separate and are separate here: byte-sized
    /// first, then power-of-two.
    fn check_atomic_access_size(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        ty: TypeSlot,
    ) -> IrResult<()> {
        let Some(bits) = type_bit_width(self.module, ty) else {
            // Pointers (no statically-known bit width) are accepted by
            // upstream because the data layout decides; we have no
            // DataLayout yet, so accept silently.
            return Ok(());
        };
        if bits < 8 {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicLoadStoreInvalidSize,
                format!("atomic memory access' size must be byte-sized (got {bits} bits)"),
            ));
        }
        if (bits & (bits - 1)) != 0 {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicLoadStoreInvalidSize,
                format!(
                    "atomic memory access' operand must have a power-of-two size (got {bits} bits)"
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitGetElementPtrInst`, arm for arm and in upstream's order.
    ///
    /// Two of upstream's `Check`s are not emitted, and each says so at the
    /// point it would have stood: the `getResultElementType() == ElTy` half of
    /// "GEP is not of right type for indices!", and the trailing address-space
    /// `Check`. A third, the `isIntOrIntVectorTy` re-check inside the vector
    /// loop, is upstream's own duplicate of the earlier one. See
    /// `docs/divergences.md` entry 120.
    fn check_gep(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        g: &GepInstData,
    ) -> IrResult<()> {
        let base_ty = self.value_type(g.ptr.get());
        if !is_pointer_or_pointer_vector(self.module, base_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepNonPointerBase,
                format!(
                    "GEP base pointer is not a vector or a vector of pointers (base operand has type {})",
                    self.type_label(base_ty)
                ),
            ));
        }
        let source = Type::<B>::new(g.source_ty, self.module);
        if SizedType::try_from(source).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepUnsizedSourceType,
                format!(
                    "GEP into unsized type! (source element type {})",
                    self.type_label(g.source_ty)
                ),
            ));
        }
        // `if (auto *STy = dyn_cast<StructType>(GEP.getSourceElementType()))
        //      Check(!STy->isScalableTy(), "getelementptr cannot target
        //      structure that contains scalable vector" "type", &GEP);`
        // `Type::is_scalable` is the port of `Type::isScalableTy`, so a struct
        // that merely *contains* one at any depth is rejected, as upstream's
        // is. `LLParser::parseGetElementPtr` carries the same rule, so a
        // parsed module is answered before this runs; a builder-constructed
        // one reaches it here.
        if source.is_struct() && source.is_scalable() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepScalableStructSource,
                format!(
                    "getelementptr cannot target structure that contains scalable vectortype (source type {})",
                    self.type_label(g.source_ty)
                ),
            ));
        }
        for (slot, idx_id) in g.indices.iter().map(|c| c.get()).enumerate() {
            let idx_ty = self.value_type(idx_id);
            if !is_int_or_int_vector(self.module, idx_ty) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::GepNonIntegerIndex,
                    format!(
                        "GEP indexes must be integers (index #{slot} has type {})",
                        self.type_label(idx_ty)
                    ),
                ));
            }
        }
        // The index sequence must index into the source element type
        // (`Verifier::visitGetElementPtrInst` checks
        // `GetElementPtrInst::getIndexedType(SourceTy, Idxs)` is non-null).
        let idx_ids: Vec<_> = g.indices.iter().map(|c| c.get()).collect();
        if crate::constants::gep_indexed_type(self.module, g.source_ty, &idx_ids).is_none() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepInvalidIndices,
                format!(
                    "Invalid indices for GEP pointer type! (indices do not index into source type {})",
                    self.type_label(g.source_ty)
                ),
            ));
        }
        // `PointerType *PtrTy = dyn_cast<PointerType>(GEP.getType()->getScalarType());`
        // and the `PtrTy` half of `Check(PtrTy && GEP.getResultElementType()
        // == ElTy, "GEP is not of right type for indices!")`. The second half
        // has no counterpart: `GepInstData` stores no result element type, so
        // there is nothing to disagree with `ElTy` (`docs/divergences.md`
        // entry 120).
        let result_ty = inst.ty().id;
        if !self
            .module
            .context()
            .type_data(scalar_type_id(self.module, result_ty))
            .is_pointer_data()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepNonPointerResult,
                format!(
                    "GEP is not of right type for indices! (result type {} is not a pointer)",
                    self.type_label(result_ty)
                ),
            ));
        }
        // "Additional checks for vector GEPs."
        if let Some(gep_width) = vector_shape(self.module, result_ty) {
            if let Some(base_width) = vector_shape(self.module, base_ty)
                && base_width != gep_width
            {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::GepVectorWidthMismatch,
                    "Vector GEP result width doesn't match operand's".to_string(),
                ));
            }
            for (position, idx_id) in g.indices.iter().map(|c| c.get()).enumerate() {
                // Upstream re-asks `IndexTy->isIntOrIntVectorTy()` inside this
                // loop; the `GepNonIntegerIndex` loop above already answered it
                // and returned on the first failure, so a second copy is dead.
                if let Some(index_width) = vector_shape(self.module, self.value_type(idx_id))
                    && index_width != gep_width
                {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::GepVectorWidthMismatch,
                        format!("Invalid GEP index vector width (index #{position})"),
                    ));
                }
            }
        }
        // `Check(GEP.getAddressSpace() == PtrTy->getAddressSpace(), "GEP
        // address space doesn't match type", &GEP);` stands here upstream and
        // is not emitted: `GetElementPtrInst::getAddressSpace` is the *pointer
        // operand's* address space (its own comment says "this is always the
        // same as the pointer operand's"), and both `IrBuilder::gep_inner` and
        // `IrBuilder::gep_erased` derive the result type from that operand via
        // `getGEPReturnType`, so the two are the same interned slot by
        // construction (`docs/divergences.md` entry 120).
        Ok(())
    }

    /// `Verifier::visitCallBase`.
    fn check_call(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        c: &CallInstData,
        position: BlockPosition<'_, 'ctx, B>,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // Callee must be a function value, OR a pointer of address
        // space 0 with a separately-tracked function-type (LLVM 17+
        // opaque-pointer model). The IrBuilder always emits a
        // function-typed callee; we accept either function or pointer
        // here so future indirect-call construction does not require
        // a verifier change.
        let callee_ty = self.value_type(c.callee.get());
        let callee_ok = self
            .module
            .context()
            .type_data(callee_ty)
            .is_function_data()
            || self.module.context().type_data(callee_ty).is_pointer_data();
        if !callee_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallNonFunction,
                format!(
                    "Called function must be a pointer! (callee has type {})",
                    self.type_label(callee_ty)
                ),
            ));
        }
        // Argument count and types must match `c.fn_ty`.
        //
        // The `as_function` guard has no upstream `Check` literal:
        // `CallBase::getFunctionType()` returns a `FunctionType *` by
        // construction, so upstream has nothing to reject here.
        let fn_ty_data = self.module.context().type_data(c.fn_ty);
        let Some((_ret, params, is_var_arg)) = fn_ty_data.as_function() else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallNonFunction,
                format!(
                    "call fn_ty {} is not a function type",
                    self.type_label(c.fn_ty)
                ),
            ));
        };
        let n_args = c.args.len();
        let n_params = params.len();
        if (is_var_arg && n_args < n_params) || (!is_var_arg && n_args != n_params) {
            // `visitCallBase`'s `if (FTy->isVarArg()) … else …`: the two arms
            // carry different literals.
            let message = if is_var_arg {
                "Called function requires more parameters than were provided!"
            } else {
                "Incorrect number of arguments passed to called function!"
            };
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallArgCountMismatch,
                format!("{message} (passes {n_args} args, signature expects {n_params})"),
            ));
        }
        for (slot, (arg_cell, &param_ty)) in c.args.iter().zip(params.iter()).enumerate() {
            let arg_ty = self.value_type(arg_cell.get());
            if arg_ty != param_ty {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgTypeMismatch,
                    format!(
                        "Call parameter type does not match function signature! (arg #{slot} has type {} but signature expects {})",
                        self.type_label(arg_ty),
                        self.type_label(param_ty)
                    ),
                ));
            }
        }
        let call = CallBaseParts {
            callee: c.callee.get(),
            fn_ty: c.fn_ty,
            args: &c.args,
            attrs: &c.attrs,
        };
        // `visitCallBase`'s `swifterror` loop, which sits between the
        // parameter-type loop above and the operand-bundle loop below.
        self.verify_call_swift_error_arguments(f, bb, call)?;
        self.check_intrinsic_call(f, bb, inst.id, call, cx)?;
        self.visit_call_base_operand_bundles(f, bb, call)?;
        // `if (Call.isInlineAsm()) verifyInlineAsmCall(Call);` — the tail of
        // `visitCallBase`, after the operand-bundle loop.
        if let ValueKindData::InlineAsm(_) = &self.module.context().value_data(c.callee.get()).kind
        {
            self.verify_inline_asm_call(f, bb, call, None)?;
        }

        // `void Verifier::visitCallInst(CallInst &CI) { visitCallBase(CI);
        //  if (CI.isMustTailCall()) verifyMustTailCall(CI); }`
        if matches!(c.tail_kind, crate::instr_types::TailCallKind::MustTail) {
            self.verify_must_tail_call(f, bb, inst, c, position)?;
        }

        Ok(())
    }

    /// `Verifier::verifySwiftErrorCall` (`lib/IR/Verifier.cpp`): "Check that
    /// SwiftErrorVal is used as a swifterror argument in CS."
    fn verify_swift_error_call(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        call: CallBaseParts<'_>,
        swift_error_val: ValueSlot,
    ) -> IrResult<()> {
        // `for (const auto &I : llvm::enumerate(Call.args()))`
        for (index, arg) in call.args.iter().enumerate() {
            if arg.get() != swift_error_val {
                continue;
            }
            // `Check(Call.paramHasAttr(I.index(), Attribute::SwiftError),
            //  "swifterror value when used in a callsite should be marked with
            //   swifterror attribute", SwiftErrorVal, Call);`
            self.verifier_check(
                f,
                bb,
                call.attrs
                    .arg_attrs()
                    .get(index)
                    .is_some_and(|set| set.has_kind(AttrIndex::Param(0), AttrKind::SwiftError)),
                VerifierRule::SwiftErrorValueUse,
                &format!(
                    "swifterror value when used in a callsite should be marked with swifterror attribute (argument #{index})"
                ),
            )?;
        }
        Ok(())
    }

    /// `Verifier::verifySwiftErrorValue` (`lib/IR/Verifier.cpp`): "Check that
    /// swifterror value is only used by loads, stores, or as a swifterror
    /// argument."
    ///
    /// `report_bb` is the block the diagnostic is anchored at when the
    /// offending user's own block cannot be resolved — upstream anchors on the
    /// two *values* (`SwiftErrorVal, U`) and prints no block at all.
    fn verify_swift_error_value(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        report_bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        swift_error_val: ValueSlot,
    ) -> IrResult<()> {
        let value = Value::<B>::from_parts(
            swift_error_val,
            self.module,
            self.value_type(swift_error_val),
        );
        // `for (const User *U : SwiftErrorVal->users())`
        for user in value.users() {
            let user_slot = user.slot();
            let user_block = self.block_of(f, user_slot);
            let at = user_block.as_ref().unwrap_or(report_bb);
            let ValueKindData::Instruction(instruction) =
                &self.module.context().value_data(user_slot).kind
            else {
                unreachable!("Value::users yields instructions");
            };
            // `Check(isa<LoadInst>(U) || isa<StoreInst>(U) || isa<CallInst>(U)
            //  || isa<InvokeInst>(U), "swifterror value can only be loaded and
            //  stored from, or as a swifterror argument!", SwiftErrorVal, U);`
            self.verifier_check(
                f,
                at,
                matches!(
                    instruction.kind,
                    InstructionKindData::Load(_)
                        | InstructionKindData::Store(_)
                        | InstructionKindData::Call(_)
                        | InstructionKindData::Invoke(_)
                ),
                VerifierRule::SwiftErrorValueUse,
                "swifterror value can only be loaded and stored from, or as a swifterror argument!",
            )?;
            // `if (auto StoreI = dyn_cast<StoreInst>(U))
            //    Check(StoreI->getOperand(1) == SwiftErrorVal, "swifterror
            //    value should be the second operand when used by stores",
            //    SwiftErrorVal, U);`
            if let InstructionKindData::Store(store) = &instruction.kind {
                self.verifier_check(
                    f,
                    at,
                    store.ptr.get() == swift_error_val,
                    VerifierRule::SwiftErrorValueUse,
                    "swifterror value should be the second operand when used by stores",
                )?;
            }
            // `if (auto *Call = dyn_cast<CallBase>(U))
            //    verifySwiftErrorCall(*const_cast<CallBase *>(Call),
            //    SwiftErrorVal);` — a `callbr` user has already failed the
            // first `Check` above, so only `call` and `invoke` reach here.
            let call = match &instruction.kind {
                InstructionKindData::Call(c) => Some(CallBaseParts {
                    callee: c.callee.get(),
                    fn_ty: c.fn_ty,
                    args: &c.args,
                    attrs: &c.attrs,
                }),
                InstructionKindData::Invoke(i) => Some(CallBaseParts {
                    callee: i.callee.get(),
                    fn_ty: i.fn_ty,
                    args: &i.args,
                    attrs: &i.attrs,
                }),
                _ => None,
            };
            if let Some(call) = call {
                self.verify_swift_error_call(f, at, call, swift_error_val)?;
            }
        }
        Ok(())
    }

    /// The `swifterror` loop of `Verifier::visitCallBase`: "For each argument
    /// of the callsite, if it has the swifterror argument, make sure the
    /// underlying alloca/parameter it comes from has a swifterror as well."
    fn verify_call_swift_error_arguments(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        call: CallBaseParts<'_>,
    ) -> IrResult<()> {
        // `FTy = Call.getFunctionType()`; a payload whose recorded type is not
        // a function type is rejected by the caller's own guard.
        let Some((_ret, params, _var_arg)) =
            self.module.context().type_data(call.fn_ty).as_function()
        else {
            return Ok(());
        };
        // `for (unsigned i = 0, e = FTy->getNumParams(); i != e; ++i)`
        for index in 0..params.len() {
            // `if (Call.paramHasAttr(i, Attribute::SwiftError))`
            if !call
                .attrs
                .arg_attrs()
                .get(index)
                .is_some_and(|set| set.has_kind(AttrIndex::Param(0), AttrKind::SwiftError))
            {
                continue;
            }
            // `Value *SwiftErrorArg = Call.getArgOperand(i);`
            let Some(swift_error_arg) = call.args.get(index) else {
                continue;
            };
            let swift_error_arg = Value::<B>::from_parts(
                swift_error_arg.get(),
                self.module,
                self.value_type(swift_error_arg.get()),
            );
            // `if (auto AI = dyn_cast<AllocaInst>(
            //      SwiftErrorArg->stripInBoundsOffsets())) {
            //    Check(AI->isSwiftError(), "swifterror argument for call has
            //    mismatched alloca", AI, Call); continue; }`
            let stripped = crate::pointer_analysis::strip_in_bounds_offsets(swift_error_arg);
            if let ValueKindData::Instruction(instruction) = &stripped.data().kind
                && let InstructionKindData::Alloca(alloca) = &instruction.kind
            {
                self.verifier_check(
                    f,
                    bb,
                    alloca.flags.is_swifterror(),
                    VerifierRule::SwiftErrorCallArgument,
                    &format!(
                        "swifterror argument for call has mismatched alloca (argument #{index})"
                    ),
                )?;
                continue;
            }
            // `auto ArgI = dyn_cast<Argument>(SwiftErrorArg);` — the
            // *unstripped* operand, deliberately: upstream strips only for the
            // alloca cast above.
            let ValueKindData::Argument { parent_fn, slot } = &swift_error_arg.data().kind else {
                // `Check(ArgI, "swifterror argument should come from an alloca
                //  or parameter", SwiftErrorArg, Call);`
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SwiftErrorCallArgument,
                    format!(
                        "swifterror argument should come from an alloca or parameter (argument #{index})"
                    ),
                ));
            };
            // `Check(ArgI->hasSwiftErrorAttr(), "swifterror argument for call
            //  has mismatched parameter", ArgI, Call);`
            let parent_attrs = match &self.module.context().value_data(*parent_fn).kind {
                ValueKindData::Function(data) => data.attributes.borrow().clone(),
                _ => AttributeStorage::new(),
            };
            self.verifier_check(
                f,
                bb,
                parent_attrs.has_kind(AttrIndex::Param(*slot), AttrKind::SwiftError),
                VerifierRule::SwiftErrorCallArgument,
                &format!(
                    "swifterror argument for call has mismatched parameter (argument #{index})"
                ),
            )?;
        }
        Ok(())
    }

    /// The block handle for the block `instruction` sits in, when it is an
    /// instruction of `f`. `IrError::VerifierFailure` carries a block name
    /// where `Verifier::CheckFailed` prints the offending value itself.
    fn block_of(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        instruction: ValueSlot,
    ) -> Option<BasicBlock<'ctx, Dyn, Unterminated, B>> {
        let ValueKindData::Instruction(data) = &self.module.context().value_data(instruction).kind
        else {
            return None;
        };
        let parent = data.parent.get();
        f.basic_blocks()
            .find(|bb| bb.slot() == parent)
            .map(BasicBlock::retag_termination::<Unterminated>)
    }

    /// `Verifier::verifyInlineAsmCall` (`lib/IR/Verifier.cpp`), whole.
    ///
    /// Upstream has one routine and two call sites: the tail of
    /// `visitCallBase` (`if (Call.isInlineAsm()) verifyInlineAsmCall(Call);`),
    /// which is where a `call` and an `invoke` reach it, and
    /// `visitCallBrInst`'s inline-asm arm. llvmkit carried two hand-rolled
    /// copies of the routine's *tail* — one in [`Self::check_call`], one in
    /// [`Self::check_callbr`] — and none in [`Self::check_invoke`], so an
    /// inline-asm `invoke` carrying a label constraint verified clean. This is
    /// now one routine with three call sites, matching upstream's two.
    ///
    /// `indirect_dest_count` stands for upstream's
    /// `dyn_cast<CallBrInst>(&Call)`: `Some(n)` is a `callbr` with `n`
    /// indirect destinations, `None` a `call` or an `invoke`.
    fn verify_inline_asm_call(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        call: CallBaseParts<'_>,
        indirect_dest_count: Option<usize>,
    ) -> IrResult<()> {
        let CallBaseParts {
            callee,
            args,
            attrs,
            ..
        } = call;
        // `const InlineAsm *IA = cast<InlineAsm>(Call.getCalledOperand());`
        let inline_asm = InlineAsm::<B>::from_parts(callee, self.module, self.value_type(callee));
        // `unsigned ArgNo = 0; unsigned LabelNo = 0;`
        let mut arg_no = 0usize;
        let mut label_no = 0usize;
        // `for (const InlineAsm::ConstraintInfo &CI : IA->ParseConstraints())`
        for constraint in inline_asm.constraint_info() {
            // `if (CI.Type == InlineAsm::isLabel) { ++LabelNo; continue; }`
            if constraint.kind == ConstraintKind::Label {
                label_no += 1;
                continue;
            }
            // `if (!CI.hasArg()) continue;` — "Only deal with constraints that
            // correspond to call arguments."
            if !constraint.has_arg() {
                continue;
            }
            // Upstream indexes `Call.getArgOperand(ArgNo)` directly, behind
            // `InlineAsm::verify`'s constraint/argument count check at
            // construction. llvmkit stops rather than indexing past the end:
            // production paths do not panic, and a count mismatch has its own
            // diagnostic before this routine runs.
            let Some(arg) = args.get(arg_no) else {
                break;
            };
            let arg_ty = self.value_type(arg.get());
            let arg_attrs = attrs.arg_attrs().get(arg_no);
            if constraint.is_indirect {
                // `Check(Arg->getType()->isPointerTy(), "Operand for indirect
                //  constraint must have pointer type", &Call);`
                self.verifier_check(
                    f,
                    bb,
                    self.module.context().type_data(arg_ty).is_pointer_data(),
                    VerifierRule::InlineAsmConstraintOperand,
                    &format!(
                        "Operand for indirect constraint must have pointer type (argument #{arg_no} has type {})",
                        self.type_label(arg_ty)
                    ),
                )?;
                // `Check(Call.getParamElementType(ArgNo), "Operand for
                //  indirect constraint must have elementtype attribute",
                //  &Call);`
                let element_type = arg_attrs
                    .and_then(|set| set.type_value(AttrIndex::Param(0), AttrKind::ElementType));
                self.verifier_check(
                    f,
                    bb,
                    element_type.is_some(),
                    VerifierRule::InlineAsmConstraintOperand,
                    &format!(
                        "Operand for indirect constraint must have elementtype attribute (argument #{arg_no})"
                    ),
                )?;
            } else {
                // `Check(!Call.paramHasAttr(ArgNo, Attribute::ElementType),
                //  "Elementtype attribute can only be applied for indirect
                //  constraints", &Call);`
                let has_element_type = arg_attrs
                    .is_some_and(|set| set.has_kind(AttrIndex::Param(0), AttrKind::ElementType));
                self.verifier_check(
                    f,
                    bb,
                    !has_element_type,
                    VerifierRule::InlineAsmConstraintOperand,
                    &format!(
                        "Elementtype attribute can only be applied for indirect constraints (argument #{arg_no})"
                    ),
                )?;
            }
            // `ArgNo++;`
            arg_no += 1;
        }

        // `if (auto *CallBr = dyn_cast<CallBrInst>(&Call)) { … } else { … }`
        match indirect_dest_count {
            Some(dests) => self.verifier_check(
                f,
                bb,
                label_no == dests,
                VerifierRule::InlineAsmLabelConstraint,
                &format!(
                    "Number of label constraints does not match number of callbr dests ({label_no} label constraints, {dests} indirect destinations)"
                ),
            ),
            None => self.verifier_check(
                f,
                bb,
                label_no == 0,
                VerifierRule::InlineAsmLabelConstraint,
                &format!(
                    "Label constraints can only be used with callbr ({label_no} label constraints)"
                ),
            ),
        }
    }

    /// `Check(C, Msg, Call)` as it expands inside a `Verifier::visit*` method:
    /// on a false condition, record the failure and leave the routine. llvmkit
    /// leaves it by returning the `Err`, which is why every caller writes `?`.
    /// Named for the macro, not for any one rule — the funclet-token arm uses
    /// it as well as the operand-bundle loop.
    fn verifier_check(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        condition: bool,
        rule: VerifierRule,
        message: &str,
    ) -> IrResult<()> {
        if condition {
            Ok(())
        } else {
            Err(self.fail(f, bb, rule, message.to_owned()))
        }
    }

    /// Mirrors the operand-bundle half of `Verifier::visitCallBase`
    /// (`lib/IR/Verifier.cpp`): the `for` over `Call.getOperandBundleAt(i)`
    /// with its `if` / `else if` chain on `BU.getTagID()`, then the single
    /// bundle `Check` that sits *after* the loop — `Direct call cannot have a
    /// ptrauth bundle`.
    ///
    /// Reached from [`Self::check_call`] and [`Self::check_invoke`] and from
    /// nowhere else, because `visitCallInst` and `visitInvokeInst` are
    /// upstream's only two callers of `visitCallBase`. `visitCallBrInst` does
    /// **not** call it — its non-inline-asm arm forbids operand bundles on a
    /// `callbr` outright, a different rule that llvmkit does not carry
    /// (`docs/divergences.md`).
    ///
    /// The `_` arm is the implicit `else` closing upstream's chain. What
    /// reaches it: `"convergencectrl"` (upstream verifies that one in
    /// `ConvergenceVerifier`, not here), `"align"`,
    /// `"deactivation-symbol"`, and every unregistered tag, which
    /// `LLVMContext::getOperandBundleTagID` gives an id no arm tests. None of
    /// them carries a rule in this routine.
    ///
    /// Single-shot, and faithfully so: upstream's `Check` macro `return`s out
    /// of `visitCallBase`, so at most one of these diagnostics is reported for
    /// one call site. Across *different* call sites upstream keeps going and
    /// llvmkit stops, which is the house difference the file header records.
    fn visit_call_base_operand_bundles(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        call: CallBaseParts<'_>,
    ) -> IrResult<()> {
        let CallBaseParts { callee, attrs, .. } = call;
        // `bool FoundDeoptBundle = false, FoundFuncletBundle = false, …;`
        let mut found_deopt = false;
        let mut found_funclet = false;
        let mut found_gc_transition = false;
        let mut found_cf_guard_target = false;
        let mut found_preallocated = false;
        let mut found_gc_live = false;
        let mut found_ptrauth = false;
        let mut found_kcfi = false;
        let mut found_attached_call = false;

        for bundle in attrs.operand_bundles_slice() {
            let inputs: Vec<ValueSlot> = bundle.inputs().collect();
            match bundle.tag() {
                OperandBundleTag::Deopt => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_deopt,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple deopt operand bundles",
                    )?;
                    found_deopt = true;
                }
                OperandBundleTag::GcTransition => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_gc_transition,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple gc-transition operand bundles",
                    )?;
                    found_gc_transition = true;
                }
                OperandBundleTag::Funclet => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_funclet,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple funclet operand bundles",
                    )?;
                    found_funclet = true;
                    // `Check(BU.Inputs.size() == 1, …)` followed by
                    // `Check(isa<FuncletPadInst>(BU.Inputs.front()), …)`;
                    // `front()` is only reached once the arity `Check` has
                    // passed, which the slice pattern spells directly.
                    let [input] = inputs.as_slice() else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CallOperandBundleOperandCount,
                            "Expected exactly one funclet bundle operand".to_owned(),
                        ));
                    };
                    self.verifier_check(
                        f,
                        bb,
                        self.is_funclet_pad(*input),
                        VerifierRule::CallFuncletBundleOperand,
                        "Funclet bundle operands should correspond to a FuncletPadInst",
                    )?;
                }
                OperandBundleTag::CfGuardTarget => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_cf_guard_target,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple CFGuardTarget operand bundles",
                    )?;
                    found_cf_guard_target = true;
                    self.verifier_check(
                        f,
                        bb,
                        inputs.len() == 1,
                        VerifierRule::CallOperandBundleOperandCount,
                        "Expected exactly one cfguardtarget bundle operand",
                    )?;
                }
                OperandBundleTag::PtrAuth => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_ptrauth,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple ptrauth operand bundles",
                    )?;
                    found_ptrauth = true;
                    let [key, discriminator] = inputs.as_slice() else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CallOperandBundleOperandCount,
                            "Expected exactly two ptrauth bundle operands".to_owned(),
                        ));
                    };
                    self.verifier_check(
                        f,
                        bb,
                        self.is_constant_int_of_width(*key, 32),
                        VerifierRule::CallPtrauthBundleOperand,
                        "Ptrauth bundle key operand must be an i32 constant",
                    )?;
                    self.verifier_check(
                        f,
                        bb,
                        self.is_integer_of_width(*discriminator, 64),
                        VerifierRule::CallPtrauthBundleOperand,
                        "Ptrauth bundle discriminator operand must be an i64",
                    )?;
                }
                OperandBundleTag::Kcfi => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_kcfi,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple kcfi operand bundles",
                    )?;
                    found_kcfi = true;
                    let [operand] = inputs.as_slice() else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CallOperandBundleOperandCount,
                            "Expected exactly one kcfi bundle operand".to_owned(),
                        ));
                    };
                    self.verifier_check(
                        f,
                        bb,
                        self.is_constant_int_of_width(*operand, 32),
                        VerifierRule::CallKcfiBundleOperand,
                        "Kcfi bundle operand must be an i32 constant",
                    )?;
                }
                OperandBundleTag::Preallocated => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_preallocated,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple preallocated operand bundles",
                    )?;
                    found_preallocated = true;
                    let [input] = inputs.as_slice() else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CallOperandBundleOperandCount,
                            "Expected exactly one preallocated bundle operand".to_owned(),
                        ));
                    };
                    // `auto Input = dyn_cast<IntrinsicInst>(BU.Inputs.front());
                    //  Check(Input && Input->getIntrinsicID() ==
                    //        Intrinsic::call_preallocated_setup, …)`
                    self.verifier_check(
                        f,
                        bb,
                        self.is_intrinsic_call_to(*input, IntrinsicId::CALL_PREALLOCATED_SETUP),
                        VerifierRule::CallPreallocatedBundleOperand,
                        "\"preallocated\" argument must be a token from \
                         llvm.call.preallocated.setup",
                    )?;
                }
                OperandBundleTag::GcLive => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_gc_live,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple gc-live operand bundles",
                    )?;
                    found_gc_live = true;
                }
                OperandBundleTag::ClangArcAttachedCall => {
                    self.verifier_check(
                        f,
                        bb,
                        !found_attached_call,
                        VerifierRule::CallDuplicateOperandBundle,
                        "Multiple \"clang.arc.attachedcall\" operand bundles",
                    )?;
                    found_attached_call = true;
                    self.verify_attached_call_bundle(f, bb, call, &inputs)?;
                }
                OperandBundleTag::ConvergenceCtrl
                | OperandBundleTag::Align
                | OperandBundleTag::DeactivationSymbol
                | OperandBundleTag::Custom(_) => {}
            }
        }

        // `Check(!(Call.getCalledFunction() && FoundPtrauthBundle),
        //        "Direct call cannot have a ptrauth bundle", Call);`
        // `CallBase::getCalledFunction` is a plain `dyn_cast_or_null<Function>`
        // on the callee operand — no `stripPointerCasts` — so "direct" here is
        // exactly "the callee value is a function".
        let direct_call = matches!(
            self.module.context().value_data(callee).kind,
            ValueKindData::Function(_)
        );
        self.verifier_check(
            f,
            bb,
            !(direct_call && found_ptrauth),
            VerifierRule::CallDirectPtrauthBundle,
            "Direct call cannot have a ptrauth bundle",
        )
    }

    /// Mirrors `Verifier::verifyAttachedCallBundle` (`lib/IR/Verifier.cpp`),
    /// `Check` for `Check` in its own order.
    fn verify_attached_call_bundle(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        call: CallBaseParts<'_>,
        inputs: &[ValueSlot],
    ) -> IrResult<()> {
        let CallBaseParts {
            callee,
            fn_ty,
            attrs,
            ..
        } = call;
        // `FunctionType *FTy = Call.getFunctionType();`
        let fn_ty_data = self.module.context().type_data(fn_ty);
        let Some((return_ty, _, _)) = fn_ty_data.as_function() else {
            // A call's `fn_ty` is a `FunctionType` by construction upstream;
            // `check_call` has already rejected anything else, and `check_invoke`
            // reports rather than panics for the same reason.
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallNonFunction,
                format!(
                    "call fn_ty {} is not a function type",
                    self.type_label(fn_ty)
                ),
            ));
        };
        let return_ty_data = self.module.context().type_data(return_ty);

        // `Check((FTy->getReturnType()->isPointerTy() ||
        //         (Call.doesNotReturn() && FTy->getReturnType()->isVoidTy())), …)`
        //
        // `CallBase::doesNotReturn()` is `hasFnAttr(Attribute::NoReturn)`,
        // already ported as `call_site_has_fn_attr`. Its first argument is an
        // anchor used only to recover the module, so the callee value serves.
        let callee_data = self.module.context().value_data(callee);
        let anchor = Value::<B>::from_parts(callee, self.module, callee_data.ty);
        let does_not_return =
            crate::speculation::call_site_has_fn_attr(anchor, callee, attrs, AttrKind::NoReturn);
        self.verifier_check(
            f,
            bb,
            matches!(return_ty_data, TypeData::Pointer { .. })
                || (does_not_return && matches!(return_ty_data, TypeData::Void)),
            VerifierRule::CallAttachedCallBundle,
            "a call with operand bundle \"clang.arc.attachedcall\" must call a \
             function returning a pointer or a non-returning function that has a \
             void return type",
        )?;

        // `Check(BU.Inputs.size() == 1 && isa<Function>(BU.Inputs.front()), …)`
        // and the `cast<Function>` immediately after it, which is why the
        // function-ness test and the binding are one pattern here.
        let [input] = inputs else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallAttachedCallBundle,
                "operand bundle \"clang.arc.attachedcall\" requires one function as \
                 an argument"
                    .to_owned(),
            ));
        };
        let input_data = self.module.context().value_data(*input);
        let ValueKindData::Function(input_function) = &input_data.kind else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallAttachedCallBundle,
                "operand bundle \"clang.arc.attachedcall\" requires one function as \
                 an argument"
                    .to_owned(),
            ));
        };

        // `Intrinsic::ID IID = Fn->getIntrinsicID(); if (IID) … else …`
        let intrinsic_id = crate::intrinsics::descriptor_for_callee(Value::<B>::from_parts(
            *input,
            self.module,
            input_data.ty,
        ))
        .map(|descriptor| descriptor.id());
        match intrinsic_id {
            Some(id) => self.verifier_check(
                f,
                bb,
                id == IntrinsicId::OBJC_RETAINAUTORELEASEDRETURNVALUE
                    || id == IntrinsicId::OBJC_CLAIMAUTORELEASEDRETURNVALUE
                    || id == IntrinsicId::OBJC_UNSAFECLAIMAUTORELEASEDRETURNVALUE,
                VerifierRule::CallAttachedCallBundle,
                "invalid function argument",
            ),
            None => {
                let name = input_function.name.as_str();
                self.verifier_check(
                    f,
                    bb,
                    name == "objc_retainAutoreleasedReturnValue"
                        || name == "objc_claimAutoreleasedReturnValue"
                        || name == "objc_unsafeClaimAutoreleasedReturnValue",
                    VerifierRule::CallAttachedCallBundle,
                    "invalid function argument",
                )
            }
        }
    }

    /// `isa<FuncletPadInst>(V)` — a `catchpad` or a `cleanuppad`, the two
    /// `FuncletPadInst` subclasses.
    fn is_funclet_pad(&self, slot: ValueSlot) -> bool {
        let ValueKindData::Instruction(instruction) = &self.module.context().value_data(slot).kind
        else {
            return false;
        };
        matches!(
            instruction.kind,
            InstructionKindData::CleanupPad(_) | InstructionKindData::CatchPad(_)
        )
    }

    /// `V->getType()->isIntegerTy(bits)`.
    fn is_integer_of_width(&self, slot: ValueSlot, bits: u32) -> bool {
        self.module
            .context()
            .type_data(self.value_type(slot))
            .as_integer()
            == Some(bits)
    }

    /// `isa<ConstantInt>(V) && V->getType()->isIntegerTy(bits)`.
    fn is_constant_int_of_width(&self, slot: ValueSlot, bits: u32) -> bool {
        matches!(
            self.module.context().value_data(slot).kind,
            ValueKindData::Constant(ConstantData::Int(_))
        ) && self.is_integer_of_width(slot, bits)
    }

    /// `dyn_cast<IntrinsicInst>(V)` followed by `getIntrinsicID() == id`.
    /// `IntrinsicInst` derives from `CallInst`, so an `invoke` of the same
    /// intrinsic is deliberately not one.
    fn is_intrinsic_call_to(&self, slot: ValueSlot, id: IntrinsicId) -> bool {
        let ValueKindData::Instruction(instruction) = &self.module.context().value_data(slot).kind
        else {
            return false;
        };
        let InstructionKindData::Call(call) = &instruction.kind else {
            return false;
        };
        let callee_data = self.module.context().value_data(call.callee.get());
        let ValueKindData::Function(_) = &callee_data.kind else {
            return false;
        };
        crate::intrinsics::descriptor_for_callee(Value::<B>::from_parts(
            call.callee.get(),
            self.module,
            callee_data.ty,
        ))
        .is_some_and(|descriptor| descriptor.id() == id)
    }

    /// Mirrors `Verifier::verifyMustTailCall`, `Check` for `Check` in its own
    /// order, including the `swifttailcc` / `tailcc` arm's early `return` and
    /// the intrinsic exemption on the prototype comparison.
    ///
    /// One house difference, shared with every rule in this file: upstream's
    /// `Check` macro leaves `verifyMustTailCall` on the first failure just as
    /// this does, but its `CheckFailed` only *records* the message and the
    /// `Verifier` carries on to the next instruction, so one bad module can
    /// produce several diagnostics; here the first failure ends the run.
    ///
    /// Driven by `test/Verifier/musttail-invalid.ll`,
    /// `test/Verifier/tailcc-musttail.ll`,
    /// `test/Verifier/swifttailcc-musttail.ll` and the two positives
    /// `test/Verifier/musttail-valid.ll` /
    /// `test/Verifier/swifttailcc-musttail-valid.ll`, all vendored under
    /// `crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/`.
    fn verify_must_tail_call(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        c: &CallInstData,
        position: BlockPosition<'_, 'ctx, B>,
    ) -> IrResult<()> {
        let BlockPosition {
            index: index_in_block,
            instructions: block_instructions,
        } = position;
        // `Check(!CI.isInlineAsm(), "cannot use musttail call with inline
        //  asm", &CI);`
        if matches!(
            self.module.context().value_data(c.callee.get()).kind,
            ValueKindData::InlineAsm(_)
        ) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::MustTailCallInlineAsm,
                "cannot use musttail call with inline asm".to_owned(),
            ));
        }

        // `Function *F = CI.getParent()->getParent();
        //  FunctionType *CallerTy = F->getFunctionType();
        //  FunctionType *CalleeTy = CI.getFunctionType();`
        let caller_ty_slot = f.data().signature;
        let callee_ty_data = self.module.context().type_data(c.fn_ty);
        let Some((callee_ret, callee_params, callee_var_arg)) = callee_ty_data.as_function() else {
            // `CI.getFunctionType()` is a `FunctionType` by construction; a
            // non-function `fn_ty` has already been rejected by the
            // `visitCallBase` half above, so this arm is unreachable from a
            // parsed module and reports rather than panics.
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallNonFunction,
                format!(
                    "call fn_ty {} is not a function type",
                    self.type_label(c.fn_ty)
                ),
            ));
        };
        let caller_ty_data = self.module.context().type_data(caller_ty_slot);
        let Some((caller_ret, caller_params, caller_var_arg)) = caller_ty_data.as_function() else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallNonFunction,
                format!(
                    "function signature {} is not a function type",
                    self.type_label(caller_ty_slot)
                ),
            ));
        };

        // `Check(CallerTy->isVarArg() == CalleeTy->isVarArg(), "cannot
        //  guarantee tail call due to mismatched varargs", &CI);`
        if caller_var_arg != callee_var_arg {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::MustTailCallVarArgsMismatch,
                "cannot guarantee tail call due to mismatched varargs".to_owned(),
            ));
        }
        // `Check(isTypeCongruent(CallerTy->getReturnType(),
        //  CalleeTy->getReturnType()), "cannot guarantee tail call due to
        //  mismatched return types", &CI);`
        if !self.is_type_congruent(caller_ret, callee_ret) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::MustTailCallReturnTypeMismatch,
                "cannot guarantee tail call due to mismatched return types".to_owned(),
            ));
        }

        // "- The calling conventions of the caller and callee must match."
        if f.calling_conv() != c.calling_conv {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::MustTailCallCallingConvMismatch,
                "cannot guarantee tail call due to mismatched calling conv".to_owned(),
            ));
        }

        // "- The call must immediately precede a ret instruction, or a
        //  pointer bitcast followed by a ret instruction.
        //  - The ret instruction must return the (possibly bitcasted) value
        //  produced by the call or void."
        //
        // `Value *RetVal = &CI; Instruction *Next = CI.getNextNode();`
        let mut ret_val = inst.as_erased().slot();
        let mut next = block_instructions.get(index_in_block + 1);

        // "Handle the optional bitcast."
        if let Some(bitcast) = next.and_then(|n| Self::bitcast_source(n)) {
            let (bitcast_inst, source) = bitcast;
            // `Check(BI->getOperand(0) == RetVal, "bitcast following musttail
            //  call must use the call", BI);`
            if source != ret_val {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MustTailCallBitcastMustUseCall,
                    "bitcast following musttail call must use the call".to_owned(),
                ));
            }
            ret_val = bitcast_inst;
            next = block_instructions.get(index_in_block + 2);
        }

        // "Check the return."
        // `ReturnInst *Ret = dyn_cast_or_null<ReturnInst>(Next);
        //  Check(Ret, "musttail call must precede a ret with an optional
        //  bitcast", &CI);`
        let Some(returned) = next.and_then(Self::return_value_of) else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::MustTailCallNotInTailPosition,
                "musttail call must precede a ret with an optional bitcast".to_owned(),
            ));
        };
        // `Check(!Ret->getReturnValue() || Ret->getReturnValue() == RetVal ||
        //  isa<UndefValue>(Ret->getReturnValue()), "musttail call result must
        //  be returned", Ret);`
        if let Some(returned) = returned {
            // `isa<UndefValue>` — `PoisonValue` derives from `UndefValue`
            // (`Constants.h`), so `ret ptr poison` satisfies the guard too.
            let is_undef = matches!(
                self.module.context().value_data(returned).kind,
                ValueKindData::Constant(ConstantData::Undef | ConstantData::Poison)
            );
            if returned != ret_val && !is_undef {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MustTailCallResultNotReturned,
                    "musttail call result must be returned".to_owned(),
                ));
            }
        }

        // `AttributeList CallerAttrs = F->getAttributes();
        //  AttributeList CalleeAttrs = CI.getAttributes();`
        let caller_attrs = f.data().attributes.borrow();
        if c.calling_conv == crate::CallingConv::SWIFT_TAIL
            || c.calling_conv == crate::CallingConv::TAIL
        {
            // `StringRef CCName = CI.getCallingConv() == CallingConv::Tail ?
            //  "tailcc" : "swifttailcc";`
            let cc_name = if c.calling_conv == crate::CallingConv::TAIL {
                "tailcc"
            } else {
                "swifttailcc"
            };
            // "- Only sret, byval, swiftself, and swiftasync ABI-impacting
            //  attributes are allowed in swifttailcc call"
            for index in 0..caller_params.len() {
                let abi_attrs = parameter_abi_attributes_of_function(&caller_attrs, index);
                self.verify_tail_cc_must_tail_attrs(
                    f,
                    bb,
                    &abi_attrs,
                    &format!("{cc_name} musttail caller"),
                )?;
            }
            for index in 0..callee_params.len() {
                let abi_attrs = parameter_abi_attributes_of_call_site(&c.attrs, index);
                self.verify_tail_cc_must_tail_attrs(
                    f,
                    bb,
                    &abi_attrs,
                    &format!("{cc_name} musttail callee"),
                )?;
            }
            // "- Varargs functions are not allowed"
            if caller_var_arg {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::TailCcMustTailVarArgsFunction,
                    format!("cannot guarantee {cc_name} tail call for varargs function"),
                ));
            }
            return Ok(());
        }

        // "- The caller and callee prototypes must match.  Pointer types of
        //  parameters or return types may differ in pointee type, but not
        //  address space."
        //
        // `if (!CI.getIntrinsicID()) { … }` — an intrinsic callee is exempt
        // from the prototype comparison, not from the attribute one below.
        if !self.callee_is_intrinsic(c.callee.get()) {
            if caller_params.len() != callee_params.len() {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MustTailCallParamCountMismatch,
                    "cannot guarantee tail call due to mismatched parameter counts".to_owned(),
                ));
            }
            for (caller_param, callee_param) in caller_params.iter().zip(callee_params.iter()) {
                if !self.is_type_congruent(*caller_param, *callee_param) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::MustTailCallParamTypeMismatch,
                        "cannot guarantee tail call due to mismatched parameter types".to_owned(),
                    ));
                }
            }
        }

        // "- All ABI-impacting function attributes, such as sret, byval,
        //  inreg, returned, preallocated, and inalloca, must match."
        for index in 0..caller_params.len() {
            let caller_abi_attrs = parameter_abi_attributes_of_function(&caller_attrs, index);
            let callee_abi_attrs = parameter_abi_attributes_of_call_site(&c.attrs, index);
            if !caller_abi_attrs.index_has_same_attributes(&callee_abi_attrs, AttrIndex::Param(0)) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MustTailCallAbiAttributeMismatch,
                    "cannot guarantee tail call due to mismatched ABI impacting function \
                     attributes"
                        .to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Mirrors `Verifier::verifyTailCCMustTailAttrs`.
    fn verify_tail_cc_must_tail_attrs(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        attrs: &AttributeStorage,
        context: &str,
    ) -> IrResult<()> {
        for (kind, keyword) in [
            (AttrKind::InAlloca, "inalloca"),
            (AttrKind::InReg, "inreg"),
            (AttrKind::SwiftError, "swifterror"),
            (AttrKind::Preallocated, "preallocated"),
            (AttrKind::ByRef, "byref"),
        ] {
            if attrs.has_kind(AttrIndex::Param(0), kind) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::TailCcMustTailForbiddenAttribute,
                    format!("{keyword} attribute not allowed in {context}"),
                ));
            }
        }
        Ok(())
    }

    /// Mirrors the file-local `isTypeCongruent` in `lib/IR/Verifier.cpp`.
    fn is_type_congruent(&self, l: TypeSlot, r: TypeSlot) -> bool {
        if l == r {
            return true;
        }
        let context = self.module.context();
        match (context.type_data(l), context.type_data(r)) {
            (TypeData::Pointer { addr_space: left }, TypeData::Pointer { addr_space: right }) => {
                left == right
            }
            _ => false,
        }
    }

    /// `dyn_cast_or_null<BitCastInst>(Next)`, projected to `(the bitcast,
    /// its operand 0)`.
    fn bitcast_source(inst: &InstructionView<'ctx, B>) -> Option<(ValueSlot, ValueSlot)> {
        let ValueKindData::Instruction(i) = &inst.as_erased().data().kind else {
            return None;
        };
        match &i.kind {
            InstructionKindData::Cast(cast) if cast.kind == CastOpcode::BitCast => {
                Some((inst.as_erased().slot(), cast.src.get()))
            }
            _ => None,
        }
    }

    /// `dyn_cast_or_null<ReturnInst>(Next)` followed by `Ret->getReturnValue()`
    /// — `None` when the instruction is not a `ret`, `Some(None)` for
    /// `ret void`.
    fn return_value_of(inst: &InstructionView<'ctx, B>) -> Option<Option<ValueSlot>> {
        let ValueKindData::Instruction(i) = &inst.as_erased().data().kind else {
            return None;
        };
        match &i.kind {
            InstructionKindData::Ret(r) => Some(r.value.get()),
            _ => None,
        }
    }

    /// `CallBase::getIntrinsicID()` — non-zero only for a direct call to an
    /// intrinsic declaration.
    fn callee_is_intrinsic(&self, callee: ValueSlot) -> bool {
        let callee_data = self.module.context().value_data(callee);
        let ValueKindData::Function(_) = &callee_data.kind else {
            return false;
        };
        crate::intrinsics::descriptor_for_callee(Value::<B>::from_parts(
            callee,
            self.module,
            callee_data.ty,
        ))
        .is_some()
    }

    fn check_intrinsic_call(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        instruction: ValueSlot,
        call: CallBaseParts<'_>,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        let CallBaseParts {
            callee: callee_id,
            fn_ty,
            args,
            attrs,
        } = call;
        let callee_data = self.module.context().value_data(callee_id);
        let ValueKindData::Function(callee_function) = &callee_data.kind else {
            return Ok(());
        };
        let callee = Value::<B>::from_parts(callee_id, self.module, callee_data.ty);
        let Some(descriptor) = crate::intrinsics::descriptor_for_callee(callee) else {
            return Ok(());
        };
        // `Check(IF->isDeclaration(), "Intrinsic functions should never be
        //  defined!", IF);` — `Function::isDeclaration()` is
        //  `BasicBlocks.empty()` for a non-materialisable function.
        self.verifier_check(
            f,
            bb,
            callee_function.basic_blocks.borrow().is_empty(),
            VerifierRule::IntrinsicDefined,
            "Intrinsic functions should never be defined!",
        )?;
        let expected = descriptor
            .function_type_ref(ModuleRef::new(self.module))
            .map_err(|_| {
                self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgTypeMismatch,
                    "Intrinsic called with incompatible signature".to_string(),
                )
            })?;
        if expected.as_type().id() != fn_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallArgTypeMismatch,
                "Intrinsic called with incompatible signature".to_string(),
            ));
        }
        let descriptor_id = descriptor.id();
        // `Check(ExpectedName == IF->getName(), "Intrinsic name not mangled
        //  correctly for type arguments! Should be: " + ExpectedName, IF);`
        // is **not** ported, because nothing here can reach it:
        // `intrinsics::descriptor_for_callee` derives the descriptor from the
        // callee's own name and then requires the resulting `FunctionType` to
        // equal the callee's signature, so a name that disagrees with its types
        // answers `None` and this routine has already returned. Upstream has no
        // such gate — `Function::getIntrinsicID()` is looked up from the name
        // alone — which is the same reason the three `matchIntrinsicSignature`
        // messages cannot fire here. `docs/divergences.md` entry 132 names
        // both.
        //
        // `for (Value *V : Call.args()) {
        //    if (auto *MD = dyn_cast<MetadataAsValue>(V))
        //      visitMetadataAsValue(*MD, Call.getCaller());
        //    if (auto *Const = dyn_cast<Constant>(V))
        //      Check(!Const->getType()->isX86_AMXTy(),
        //            "const x86_amx is not allowed in argument!"); }`
        //
        // The `visitMetadataAsValue` half is unported; entry 132 of
        // `docs/divergences.md` names it.
        for arg in args {
            let arg_data = self.module.context().value_data(arg.get());
            if matches!(arg_data.kind, ValueKindData::Constant(_)) {
                self.verifier_check(
                    f,
                    bb,
                    !matches!(
                        self.module.context().type_data(arg_data.ty),
                        TypeData::X86Amx
                    ),
                    VerifierRule::ConstX86AmxArgument,
                    "const x86_amx is not allowed in argument!",
                )?;
            }
        }
        // `switch (ID) { ... }`. Only the `Intrinsic::callbr_landingpad` arm is
        // ported; `docs/divergences.md` entry 132 names the rest.
        if descriptor_id == IntrinsicId::CALLBR_LANDINGPAD {
            self.check_callbr_landingpad_intrinsic(f, bb, instruction, args, cx)?;
        }
        for index in descriptor.immarg_operand_indices() {
            let Some(arg) = args.get(index) else {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgCountMismatch,
                    "Intrinsic called with incompatible signature".to_string(),
                ));
            };
            if !matches!(
                self.module.context().value_data(arg.get()).kind,
                ValueKindData::Constant(ConstantData::Int(_) | ConstantData::Float(_))
            ) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgTypeMismatch,
                    "immarg operand has non-immediate parameter".to_string(),
                ));
            }
        }
        self.verify_funclet_token(f, bb, descriptor_id, attrs, cx)
    }

    /// `Verifier::visitIntrinsicCall`'s `case Intrinsic::callbr_landingpad:`
    /// arm (`lib/IR/Verifier.cpp`), statement for statement — including the two
    /// `CheckFailed` + `break` pairs, which stop the arm and not the function,
    /// and upstream's `intrinstic` typo, which is contractual.
    fn check_callbr_landingpad_intrinsic(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        instruction: ValueSlot,
        args: &[core::cell::Cell<ValueSlot>],
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `const auto *CBR = dyn_cast<CallBrInst>(Call.getOperand(0));
        //  Check(CBR, "intrinstic requires callbr operand", &Call);
        //  if (!CBR) break;`
        let callbr = args
            .first()
            .map(core::cell::Cell::get)
            .filter(|slot| self.is_callbr_instruction(*slot));
        let Some(callbr) = callbr else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallbrLandingPadPlacement,
                "intrinstic requires callbr operand".to_owned(),
            ));
        };
        // `const BasicBlock *LandingPadBB = Call.getParent();
        //  const BasicBlock *PredBB = LandingPadBB->getUniquePredecessor();
        //  if (!PredBB) { CheckFailed("Intrinsic in block must have 1 unique
        //  predecessor", &Call); break; }`
        //
        // `getUniquePredecessor` is "exactly one *distinct* predecessor block",
        // not "exactly one incoming edge" — a block one predecessor reaches
        // twice still has a unique predecessor. `cx.predecessors` is the
        // multiset, so the distinct count is what is taken here.
        let landing_pad_bb = bb.slot();
        let predecessors = cx
            .predecessors
            .get(&landing_pad_bb)
            .map_or(&[][..], Vec::as_slice);
        let mut unique_predecessor = None;
        for predecessor in predecessors {
            match unique_predecessor {
                None => unique_predecessor = Some(*predecessor),
                Some(seen) if seen == *predecessor => {}
                Some(_) => {
                    unique_predecessor = None;
                    break;
                }
            }
        }
        let Some(predecessor) = unique_predecessor else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallbrLandingPadPlacement,
                "Intrinsic in block must have 1 unique predecessor".to_owned(),
            ));
        };
        // `if (!isa<CallBrInst>(PredBB->getTerminator())) {
        //    CheckFailed("Intrinsic must have corresponding callbr in
        //    predecessor", &Call); break; }`
        if !self
            .block_terminator_slot(predecessor)
            .is_some_and(|slot| self.is_callbr_instruction(slot))
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallbrLandingPadPlacement,
                "Intrinsic must have corresponding callbr in predecessor".to_owned(),
            ));
        }
        // `Check(llvm::is_contained(CBR->getIndirectDests(), LandingPadBB),
        //   "Intrinsic's corresponding callbr must have intrinsic's parent
        //    basic block in indirect destination list", &Call);`
        let in_indirect_dests = match &self.module.context().value_data(callbr).kind {
            ValueKindData::Instruction(data) => match &data.kind {
                InstructionKindData::CallBr(callbr_data) => callbr_data
                    .indirect_dests
                    .iter()
                    .any(|dest| dest.get() == landing_pad_bb),
                _ => false,
            },
            _ => false,
        };
        self.verifier_check(
            f,
            bb,
            in_indirect_dests,
            VerifierRule::CallbrLandingPadPlacement,
            "Intrinsic's corresponding callbr must have intrinsic's parent basic block in \
             indirect destination list",
        )?;
        // `const Instruction &First = *LandingPadBB->begin();
        //  Check(&First == &Call, "No other instructions may proceed intrinsic",
        //        &Call);`
        self.verifier_check(
            f,
            bb,
            self.first_instruction_in_block(landing_pad_bb) == Some(instruction),
            VerifierRule::CallbrLandingPadPlacement,
            "No other instructions may proceed intrinsic",
        )
    }

    /// `isa<CallBrInst>(V)`.
    fn is_callbr_instruction(&self, slot: ValueSlot) -> bool {
        match &self.module.context().value_data(slot).kind {
            ValueKindData::Instruction(data) => {
                matches!(data.kind, InstructionKindData::CallBr(_))
            }
            _ => false,
        }
    }

    /// `BasicBlock::getTerminator()` — the last instruction of the block, or
    /// `None` where upstream answers null because the block is unterminated.
    fn block_terminator_slot(&self, block: ValueSlot) -> Option<ValueSlot> {
        let ValueKindData::BasicBlock(data) = &self.module.context().value_data(block).kind else {
            return None;
        };
        let instructions = data.instructions.borrow();
        instructions.last().copied()
    }

    /// Mirrors the tail of `Verifier::visitIntrinsicCall`
    /// (`lib/IR/Verifier.cpp`), the block under "Verify that there aren't any
    /// unmediated control transfers between funclets": an intrinsic that may
    /// lower to a real call, sitting inside an EH funclet of a scoped-EH
    /// function, must name the funclet it belongs to.
    ///
    /// Runs after the per-intrinsic `switch`, which is where upstream puts it.
    ///
    /// **One hardening, at the point upstream asserts.** Upstream reads the
    /// colour vector with `BlockEHFuncletColors.find(CallBB)->second` behind
    /// `assert(CV.size() > 0 && "Uncolored block")`. `colorEHFunclets` walks
    /// forward from the entry block, so a block unreachable from entry has no
    /// entry at all and that lookup is a dangling dereference in a release
    /// build. llvmkit reads a missing entry as "not in a funclet" and raises
    /// nothing, which is the answer the colouring would have given had the
    /// block been reachable through no funclet.
    fn verify_funclet_token(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        id: IntrinsicId,
        attrs: &CallAttributeData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `if (IntrinsicInst::mayLowerToFunctionCall(ID)) {`
        if !crate::intrinsic_inst::may_lower_to_function_call(id) {
            return Ok(());
        }
        // `Function *F = Call.getParent()->getParent();
        //  if (F->hasPersonalityFn() &&
        //      isScopedEHPersonality(classifyEHPersonality(F->getPersonalityFn())))`
        let Some(personality) = f.personality_fn() else {
            return Ok(());
        };
        if !is_scoped_eh_personality(classify_eh_personality(personality.as_erased())) {
            return Ok(());
        }

        // `if (BlockEHFuncletColors.empty())
        //    BlockEHFuncletColors = colorEHFunclets(*F);`
        // The `OnceCell` is `FunctionContext`'s, so it is built at most once
        // per function and dropped with it — upstream clears the map in
        // `visitFunction` for the same reason.
        let colors = cx.eh_funclet_colors.get_or_init(|| color_eh_funclets(f));

        // `bool InEHFunclet = false;
        //  for (BasicBlock *ColorFirstBB : CV)
        //    if (auto It = ColorFirstBB->getFirstNonPHIIt(); It != ColorFirstBB->end())
        //      if (isa_and_nonnull<FuncletPadInst>(&*It)) InEHFunclet = true;`
        let mut in_eh_funclet = false;
        let anchor = f.as_erased();
        for color_first_bb in colors.get(&bb.slot()).map_or(&[][..], Vec::as_slice) {
            if first_non_phi_kind(anchor, *color_first_bb).is_some_and(is_funclet_pad_kind) {
                in_eh_funclet = true;
            }
        }

        // `bool HasToken = false;
        //  for (…) if (…getTagID() == LLVMContext::OB_funclet) HasToken = true;`
        let mut has_token = false;
        for bundle in attrs.operand_bundles_slice() {
            if matches!(bundle.tag(), OperandBundleTag::Funclet) {
                has_token = true;
            }
        }

        // `if (InEHFunclet)
        //    Check(HasToken, "Missing funclet token on intrinsic call", &Call);`
        if in_eh_funclet {
            self.verifier_check(
                f,
                bb,
                has_token,
                VerifierRule::MissingFuncletToken,
                "Missing funclet token on intrinsic call",
            )?;
        }
        Ok(())
    }

    /// `Verifier::visitSelectInst`.
    fn check_select(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        s: &SelectInstData,
    ) -> IrResult<()> {
        let cond_ty = self.value_type(s.cond.get());
        let result_ty = inst.ty().id;
        let true_ty = self.value_type(s.true_val.get());
        let false_ty = self.value_type(s.false_val.get());
        // Condition must be i1 or <N x i1>; if vector, its element
        // count must match the result-vector element count.
        let cond_ok = if is_i1(self.module, cond_ty) {
            true
        } else if let Some((cond_elem, cond_n, _)) =
            self.module.context().type_data(cond_ty).as_vector()
            && is_i1_data(self.module.context().type_data(cond_elem))
        {
            // Result must also be a vector with the same length.
            if let Some((_, res_n, _)) = self.module.context().type_data(result_ty).as_vector() {
                cond_n == res_n
            } else {
                false
            }
        } else {
            false
        };
        if !cond_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::SelectConditionNotI1,
                format!(
                    "Invalid operands for select instruction! (condition has type {})",
                    self.type_label(cond_ty)
                ),
            ));
        }
        if true_ty != false_ty || true_ty != result_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::SelectArmTypeMismatch,
                format!(
                    "Select values must have same type as select instruction! (arms have types {}/{}, result {})",
                    self.type_label(true_ty),
                    self.type_label(false_ty),
                    self.type_label(result_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitPHINode`.
    fn check_phi(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        p: &PhiData,
        predecessors: &HashMap<ValueSlot, Vec<ValueSlot>>,
    ) -> IrResult<()> {
        let result_ty = inst.ty().id;

        // The phi result type must be a first-class *data* type. `is_first_class`
        // is not a sufficient gate — it admits `label`/`metadata`/`token` — so
        // enumerate the valid kinds exactly as the `.ll` parser's `parse_phi`
        // whitelist does (int / float / pointer / vector / array / struct), with
        // the `is_first_class` conjunct on struct excluding opaque structs. This
        // runs before the coherence delegation so an invalid result type is
        // rejected regardless of incoming coherence.
        let rty = Type::<B>::new(result_ty, self.module);
        let valid_result = rty.is_integer()
            || rty.is_floating_point()
            || rty.is_pointer()
            || rty.is_typed_pointer()
            || rty.is_vector()
            || rty.is_array()
            || (rty.is_struct() && rty.is_first_class());
        if !valid_result {
            // Two upstream `Check`s answer this between them, and which one
            // fires depends on the type: `visitPHINode`'s
            // `Check(!PN.getType()->isTokenLikeTy(), …)` for a token, and
            // `visitInstruction`'s `Check(I.getType()->isFirstClassType(), …)`
            // for everything else llvmkit rejects here.
            let message = if rty.is_token() {
                "PHI nodes cannot have token type!"
            } else {
                "Instruction returns a non-scalar type!"
            };
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiInvalidResultType,
                format!("{message} (phi result type {})", self.type_label(result_ty)),
            ));
        }

        let preds = predecessors
            .get(&bb.slot())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        // Snapshot the (value, predecessor-block) pairs and delegate to
        // the shared coherence core so the parser (which runs the same
        // helper) cannot drift from the verifier. Each `PhiViolation`
        // maps back to the verifier's existing byte-identical diagnostic.
        let incoming: Vec<(ValueSlot, ValueSlot)> = p
            .incoming
            .borrow()
            .iter()
            .map(|(v, b)| (v.get(), *b))
            .collect();

        // There is deliberately no separate "empty phi" rule here. Upstream's
        // only length guard on a phi is `visitBasicBlock`'s
        // `Check(PN.getNumIncomingValues() == Preds.size(), …)`, which
        // `check_phi_incoming` carries below; a zero-incoming phi in a
        // zero-predecessor block passes it on `0 == 0`, and nothing else in
        // `Verifier` rejects it. `visitPHINode` itself checks only phi
        // grouping, the token-like result type and per-incoming type
        // agreement, then defers with "All other PHI node constraints are
        // checked in the visitBasicBlock method."
        //
        // llvmkit used to pre-empt that with a stricter reachability-gated
        // rule, justified on round-trip grounds. The justification was wrong:
        // `AsmWriter`'s phi arm emits the type and then an empty
        // `ListSeparator` loop, and both `LLParser::parsePHI` and llvmkit's
        // `parse_phi` open their pair loop with "if the next token is not `[`,
        // stop" — so `%p = phi i32` prints and re-parses on both sides.
        let value_ty_of = |id: ValueSlot| self.value_type(id);
        match check_phi_incoming(result_ty, &incoming, preds, &value_ty_of) {
            Ok(()) => Ok(()),
            Err(PhiViolation::CountMismatch { entries, preds }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                format!(
                    "PHINode should have one entry for each predecessor of its parent basic block! ({entries} incoming entries, {preds} predecessors)"
                ),
            )),
            Err(PhiViolation::NotAPredecessor { block }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                format!(
                    "PHI node entries do not match predecessors! (incoming block %{} is not a predecessor)",
                    slot_label(f, block)
                ),
            )),
            Err(PhiViolation::TooManyFromBlock { block }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                format!(
                    "PHI node entries do not match predecessors! (too many incoming entries from block %{})",
                    slot_label(f, block)
                ),
            )),
            Err(PhiViolation::AmbiguousValues { block }) => Err(self.fail(
                f,
                bb,
                VerifierRule::AmbiguousPhi,
                format!(
                    "PHI node has multiple entries for the same basic block with different incoming values! (block %{})",
                    slot_label(f, block)
                ),
            )),
            Err(PhiViolation::IncomingTypeMismatch { block, value_ty }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiIncomingTypeMismatch,
                format!(
                    "PHI node operands are not the same type as the result! (expects {} but incoming from %{} is {})",
                    self.type_label(result_ty),
                    slot_label(f, block),
                    self.type_label(value_ty)
                ),
            )),
        }
    }

    /// `Verifier::visitReturnInst`.
    fn check_ret(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        r: &ReturnOpData,
    ) -> IrResult<()> {
        let expected = f.return_type();
        match (r.value.get(), expected.is_void()) {
            (None, true) => Ok(()),
            // `visitReturnInst`'s `if (F->getReturnType()->isVoidTy())` picks
            // which of two literals a bad `ret` gets: the void-function arm
            // has its own, and every other shape is the operand-type one.
            (None, false) => Err(self.fail(
                f,
                bb,
                VerifierRule::ReturnTypeMismatch,
                format!(
                    "Function return type does not match operand type of return inst! (ret has no operand but function returns {})",
                    expected.kind_label()
                ),
            )),
            (Some(_), true) => Err(self.fail(
                f,
                bb,
                VerifierRule::ReturnTypeMismatch,
                "Found return instr that returns non-void in Function of void return type!".into(),
            )),
            (Some(v), false) => {
                let actual = self.value_type(v);
                if actual == expected.id {
                    Ok(())
                } else {
                    Err(self.fail(
                        f,
                        bb,
                        VerifierRule::ReturnTypeMismatch,
                        format!(
                            "Function return type does not match operand type of return inst! (operand has type {} but function returns {})",
                            self.type_label(actual),
                            expected.kind_label()
                        ),
                    ))
                }
            }
        }
    }

    /// `Verifier::visitSwitchInst`. The condition must be an integer
    /// type; every case value must share that type; every successor
    /// (default + cases) must belong to the parent function.
    fn check_switch(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        d: &SwitchInstData,
        block_index: &HashMap<ValueSlot, usize>,
    ) -> IrResult<()> {
        let cond_ty = self.value_type(d.cond.get());
        if self
            .module
            .context()
            .type_data(cond_ty)
            .as_integer()
            .is_none()
        {
            // No upstream `Check` literal: `SwitchInst::init` asserts the
            // condition is integral, so `visitSwitchInst` never restates it.
            return Err(self.fail(
                f,
                bb,
                VerifierRule::SwitchOperandTypeMismatch,
                format!(
                    "switch condition {} is not integer",
                    self.type_label(cond_ty)
                ),
            ));
        }
        if !block_index.contains_key(&d.default_bb.get()) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                "Referring to a basic block in another function! (switch default target)".into(),
            ));
        }
        for (case_v, case_bb) in d.cases.borrow().iter() {
            let v_ty = self.value_type(case_v.get());
            if v_ty != cond_ty {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SwitchOperandTypeMismatch,
                    format!(
                        "Switch constants must all be same type as switch value! (case value {} != condition {})",
                        self.type_label(v_ty),
                        self.type_label(cond_ty)
                    ),
                ));
            }
            if !block_index.contains_key(case_bb) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiPredecessorMismatch,
                    "Referring to a basic block in another function! (switch case target)".into(),
                ));
            }
        }
        Ok(())
    }

    /// `Verifier::visitIndirectBrInst`. The address operand must be a
    /// pointer; every destination must belong to the parent function.
    fn check_indirectbr(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        d: &IndirectBrInstData,
        block_index: &HashMap<ValueSlot, usize>,
    ) -> IrResult<()> {
        let addr_ty = self.value_type(d.addr.get());
        if !self.module.context().type_data(addr_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IndirectBrNonPointerAddress,
                format!(
                    "Indirectbr operand must have pointer type! (got {})",
                    self.type_label(addr_ty)
                ),
            ));
        }
        for &dest in d.destinations.borrow().iter() {
            if !block_index.contains_key(&dest) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiPredecessorMismatch,
                    "Referring to a basic block in another function! (indirectbr destination)"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// `Verifier::visitInvokeInst`. Constructive subset: every
    /// destination is a basic block of the parent function. Callee /
    /// arg type checks reuse the same logic as [`Self::check_call`]
    /// but specialised inline since the storage payload differs.
    fn check_invoke(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &InvokeInstData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        let block_index = cx.block_index;
        if !block_index.contains_key(&d.normal_dest.get())
            || !block_index.contains_key(&d.unwind_dest.get())
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                "Referring to a basic block in another function! (invoke destination)".into(),
            ));
        }
        let call = CallBaseParts {
            callee: d.callee.get(),
            fn_ty: d.fn_ty,
            args: &d.args,
            attrs: &d.attrs,
        };
        self.verify_call_swift_error_arguments(f, bb, call)?;
        self.check_intrinsic_call(f, bb, inst.id, call, cx)?;
        self.visit_call_base_operand_bundles(f, bb, call)?;
        // `if (Call.isInlineAsm()) verifyInlineAsmCall(Call);` — the same tail
        // of `visitCallBase` that `check_call` runs; `visitInvokeInst` calls
        // `visitCallBase` too, so an inline-asm `invoke` is checked here.
        if let ValueKindData::InlineAsm(_) = &self.module.context().value_data(d.callee.get()).kind
        {
            self.verify_inline_asm_call(f, bb, call, None)?;
        }
        // "Verify that the first non-PHI instruction of the unwind destination
        //  is an exception handling instruction." —
        // `Check(II.getUnwindDest()->isEHPad(), "The unwind destination does
        //  not have an exception handling instruction!", &II);`
        self.verifier_check(
            f,
            bb,
            self.first_non_phi_in_block(f, d.unwind_dest.get())
                .is_some_and(|slot| self.is_eh_pad_instruction(slot)),
            VerifierRule::EhPadInvalidStructure,
            "The unwind destination does not have an exception handling instruction!",
        )?;
        Ok(())
    }

    /// `Verifier::visitCallBrInst`, both arms, in upstream's order.
    ///
    /// The routine splits on `CBI.isInlineAsm()` and **never calls
    /// `visitCallBase`** — `grep -n "visitCallBase(" lib/IR/Verifier.cpp`
    /// prints the declaration, the definition and two call sites, those two
    /// being `visitCallInst` and `visitInvokeInst`. So no operand-bundle loop
    /// runs for a `callbr`; the non-asm arm forbids bundles outright instead.
    ///
    /// Two deliberate spellings:
    ///
    /// * upstream's `default:` arm is a bare `CheckFailed`, not a `Check`, so
    ///   it records the failure and falls through to `visitIntrinsicCall`.
    ///   llvmkit reports the first failure and returns, which is the house
    ///   single-error model the module header records; the *first* message is
    ///   upstream's first, which is what a `CHECK` line reads.
    /// * `visitTerminator(CBI)` closes the routine, and the block-membership
    ///   checks that stand for `visitInstruction`'s `Referring to a basic
    ///   block in another function!` therefore run **last**, not first as they
    ///   did before this port.
    fn check_callbr(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &CallBrInstData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        let call = CallBaseParts {
            callee: d.callee.get(),
            fn_ty: d.fn_ty,
            args: &d.args,
            attrs: &d.attrs,
        };
        let callee_data = self.module.context().value_data(d.callee.get());
        // `if (!CBI.isInlineAsm()) { … } else { … }`
        if let ValueKindData::InlineAsm(_) = &callee_data.kind {
            // `const InlineAsm *IA = cast<InlineAsm>(CBI.getCalledOperand());
            //  Check(!IA->canThrow(), "Unwinding from Callbr is not allowed");`
            let inline_asm =
                InlineAsm::<B>::from_parts(d.callee.get(), self.module, callee_data.ty);
            self.verifier_check(
                f,
                bb,
                !inline_asm.can_unwind(),
                VerifierRule::CallBrInlineAsmUnwinds,
                "Unwinding from Callbr is not allowed",
            )?;
            // `verifyInlineAsmCall(CBI);`
            self.verify_inline_asm_call(f, bb, call, Some(d.indirect_dests.len()))?;
        } else {
            // `Check(CBI.getCalledFunction(), "Callbr: indirect function /
            //  invalid signature");` — `getCalledFunction` is
            // `dyn_cast_or_null<Function>(getCalledOperand())`, so anything
            // that is not a function value fails here.
            let ValueKindData::Function(_) = &callee_data.kind else {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallNonFunction,
                    format!(
                        "Callbr: indirect function / invalid signature (callee has type {})",
                        self.type_label(callee_data.ty)
                    ),
                ));
            };
            // `Check(!CBI.hasOperandBundles(), "Callbr for intrinsics
            //  currently doesn't support operand bundles");`
            self.verifier_check(
                f,
                bb,
                d.attrs.operand_bundles_slice().is_empty(),
                VerifierRule::CallBrOperandBundle,
                "Callbr for intrinsics currently doesn't support operand bundles",
            )?;
            // `switch (CBI.getIntrinsicID())` — one case, plus `default:`.
            let intrinsic_id = crate::intrinsics::descriptor_for_callee(Value::<B>::from_parts(
                d.callee.get(),
                self.module,
                callee_data.ty,
            ))
            .map(|descriptor| descriptor.id());
            if intrinsic_id == Some(IntrinsicId::AMDGCN_KILL) {
                // `Check(CBI.getNumIndirectDests() == 1, "Callbr amdgcn_kill
                //  only supports one indirect dest");`
                self.verifier_check(
                    f,
                    bb,
                    d.indirect_dests.len() == 1,
                    VerifierRule::CallBrUnsupportedIntrinsic,
                    &format!(
                        "Callbr amdgcn_kill only supports one indirect dest (has {})",
                        d.indirect_dests.len()
                    ),
                )?;
                // `bool Unreachable = isa<UnreachableInst>(
                //      CBI.getIndirectDest(0)->begin());
                //  CallInst *Call = dyn_cast<CallInst>(
                //      CBI.getIndirectDest(0)->begin());`
                //
                // `begin()`, not `getFirstNonPHIIt()`. An empty destination
                // block has no `begin()` to dereference; upstream would read
                // past the end there, llvmkit answers `None` and the `Check`
                // below fails — the block is separately rejected for having no
                // terminator.
                let first = d
                    .indirect_dests
                    .first()
                    .and_then(|dest| self.first_instruction_in_block(dest.get()));
                let unreachable = first.is_some_and(|slot| {
                    matches!(
                        &self.module.context().value_data(slot).kind,
                        ValueKindData::Instruction(instruction)
                            if matches!(instruction.kind, InstructionKindData::Unreachable(_))
                    )
                });
                let calls_amdgcn_unreachable = first.is_some_and(|slot| {
                    self.is_intrinsic_call_to(slot, IntrinsicId::AMDGCN_UNREACHABLE)
                });
                // `Check(Unreachable || (Call && Call->getIntrinsicID() ==
                //  Intrinsic::amdgcn_unreachable), "Callbr amdgcn_kill
                //  indirect dest needs to be unreachable");`
                self.verifier_check(
                    f,
                    bb,
                    unreachable || calls_amdgcn_unreachable,
                    VerifierRule::CallBrUnsupportedIntrinsic,
                    "Callbr amdgcn_kill indirect dest needs to be unreachable",
                )?;
            } else {
                // `default: CheckFailed("Callbr currently only supports
                //  asm-goto and selected intrinsics");`
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallBrUnsupportedIntrinsic,
                    "Callbr currently only supports asm-goto and selected intrinsics".to_owned(),
                ));
            }
            // `visitIntrinsicCall(CBI.getIntrinsicID(), CBI);`
            self.check_intrinsic_call(f, bb, inst.id, call, cx)?;
        }

        // `visitTerminator(CBI);` — llvmkit's spelling of the successor half
        // of `visitInstruction`'s operand walk.
        let block_index = cx.block_index;
        if !block_index.contains_key(&d.default_dest.get()) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                "Referring to a basic block in another function! (callbr default destination)"
                    .into(),
            ));
        }
        for ic in d.indirect_dests.iter() {
            if !block_index.contains_key(&ic.get()) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiPredecessorMismatch,
                    "Referring to a basic block in another function! (callbr indirect destination)"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// `BasicBlock::begin()` projected to a value id — the block's *first*
    /// instruction, phis included, or `None` for an empty block (upstream's
    /// `begin() == end()`).
    fn first_instruction_in_block(&self, block: ValueSlot) -> Option<ValueSlot> {
        let ValueKindData::BasicBlock(data) = &self.module.context().value_data(block).kind else {
            return None;
        };
        let instructions = data.instructions.borrow();
        instructions.first().copied()
    }

    /// `Verifier::visitBranchInst`.
    fn check_br(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        _inst: &InstructionView<'ctx, B>,
        b: &BranchInstData,
        block_index: &HashMap<ValueSlot, usize>,
    ) -> IrResult<()> {
        match &*b.kind.borrow() {
            BranchKind::Unconditional(target) => {
                if !block_index.contains_key(target) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::PhiPredecessorMismatch,
                        "Referring to a basic block in another function! (br target)".into(),
                    ));
                }
            }
            BranchKind::Conditional {
                cond,
                then_bb,
                else_bb,
            } => {
                let cond_ty = self.value_type(cond.get());
                if !is_i1(self.module, cond_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::BranchConditionNotI1,
                        format!(
                            "Branch condition is not 'i1' type! (got {})",
                            self.type_label(cond_ty)
                        ),
                    ));
                }
                if !block_index.contains_key(then_bb) || !block_index.contains_key(else_bb) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::PhiPredecessorMismatch,
                        "Referring to a basic block in another function! (br target)".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Exception handling — `Verifier`'s EH pad chapter
    // ------------------------------------------------------------------

    /// A pad operand as upstream compares them, with `None` for
    /// `ConstantTokenNone`.
    ///
    /// llvmkit spells "no pad" two ways — a `parent_pad` field holding `None`
    /// (`cleanuppad within none`), and an explicit `token none` operand
    /// (`cleanupret from none`), which parses to a `ConstantData::TokenNone`
    /// value. Upstream has one uniqued `ConstantTokenNone` per context, so the
    /// two are pointer-equal there and must compare equal here.
    fn pad_ref(&self, slot: ValueSlot) -> Option<ValueSlot> {
        match &self.module.context().value_data(slot).kind {
            ValueKindData::Constant(ConstantData::TokenNone) => None,
            _ => Some(slot),
        }
    }

    /// `getParentPad` (`lib/IR/Verifier.cpp`, file-local):
    /// `FuncletPadInst::getParentPad()` or `CatchSwitchInst::getParentPad()`,
    /// with `None` for `ConstantTokenNone`.
    ///
    /// Upstream's `cast<CatchSwitchInst>` asserts on anything that is neither a
    /// funclet pad nor a `catchswitch`. Every caller here reaches it only past
    /// the `Parent pad must be catchpad/cleanuppad/catchswitch` `Check`, which
    /// is what upstream's own comment says that `Check` is for ("We need the
    /// extra check here to make sure getParentPad() works"), so the `None`
    /// this answers for a non-pad stands for an unreachable assertion rather
    /// than a divergence.
    fn parent_pad(&self, pad: ValueSlot) -> Option<ValueSlot> {
        let ValueKindData::Instruction(instruction) = &self.module.context().value_data(pad).kind
        else {
            return None;
        };
        let field = match &instruction.kind {
            InstructionKindData::CatchPad(p) => p.parent_pad.get(),
            InstructionKindData::CleanupPad(p) => p.parent_pad.get(),
            InstructionKindData::CatchSwitch(s) => s.parent_pad.get(),
            _ => None,
        };
        field.and_then(|slot| self.pad_ref(slot))
    }

    /// `isa<FuncletPadInst>(V) || isa<CatchSwitchInst>(V)`.
    fn is_pad_or_catch_switch(&self, slot: ValueSlot) -> bool {
        let ValueKindData::Instruction(instruction) = &self.module.context().value_data(slot).kind
        else {
            return false;
        };
        matches!(
            instruction.kind,
            InstructionKindData::CatchPad(_)
                | InstructionKindData::CleanupPad(_)
                | InstructionKindData::CatchSwitch(_)
        )
    }

    /// `Instruction::isEHPad()` — `landingpad`, `catchpad`, `cleanuppad`,
    /// `catchswitch`.
    fn is_eh_pad_instruction(&self, slot: ValueSlot) -> bool {
        let ValueKindData::Instruction(instruction) = &self.module.context().value_data(slot).kind
        else {
            return false;
        };
        matches!(
            instruction.kind,
            InstructionKindData::LandingPad(_)
                | InstructionKindData::CatchPad(_)
                | InstructionKindData::CleanupPad(_)
                | InstructionKindData::CatchSwitch(_)
        )
    }

    /// `isa<LandingPadInst>(V)`.
    fn is_landing_pad(&self, slot: ValueSlot) -> bool {
        matches!(
            &self.module.context().value_data(slot).kind,
            ValueKindData::Instruction(instruction)
                if matches!(instruction.kind, InstructionKindData::LandingPad(_))
        )
    }

    /// `BasicBlock::getFirstNonPHIIt()` as a value id.
    fn first_non_phi_in_block(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        block: ValueSlot,
    ) -> Option<ValueSlot> {
        crate::eh_personalities::first_non_phi_slot(f.as_erased(), block)
    }

    /// `BasicBlock::getTerminator()` as a value id — `None` when the block's
    /// last instruction is not one, which `visit_block` rejects separately.
    fn block_terminator(&self, block: ValueSlot) -> Option<ValueSlot> {
        let ValueKindData::BasicBlock(data) = &self.module.context().value_data(block).kind else {
            return None;
        };
        let last = *data.instructions.borrow().last()?;
        let ValueKindData::Instruction(instruction) = &self.module.context().value_data(last).kind
        else {
            return None;
        };
        instruction.kind.is_terminator().then_some(last)
    }

    /// `Instruction::getParent()` as a value id.
    fn parent_block_of(&self, instruction: ValueSlot) -> Option<ValueSlot> {
        match &self.module.context().value_data(instruction).kind {
            ValueKindData::Instruction(data) => Some(data.parent.get()),
            _ => None,
        }
    }

    /// The unwind destination of an `invoke`, a `catchswitch` or a
    /// `cleanupret` — the three terminators that have one. The outer `Option`
    /// is "this terminator has no unwind destination field at all"; the inner
    /// is upstream's null `BasicBlock *` (`unwind to caller`).
    fn terminator_unwind_dest(&self, terminator: ValueSlot) -> Option<Option<ValueSlot>> {
        let ValueKindData::Instruction(instruction) =
            &self.module.context().value_data(terminator).kind
        else {
            return None;
        };
        match &instruction.kind {
            InstructionKindData::Invoke(i) => Some(Some(i.unwind_dest.get())),
            InstructionKindData::CatchSwitch(s) => Some(s.unwind_dest.get()),
            InstructionKindData::CleanupReturn(r) => Some(r.unwind_dest),
            _ => None,
        }
    }

    /// `getSuccPad` (`lib/IR/Verifier.cpp`, file-local): the first non-phi
    /// instruction of `terminator`'s unwind destination.
    ///
    /// Upstream's final `cast<CleanupReturnInst>` asserts on any other
    /// terminator and its `UnwindDest->getFirstNonPHIIt()` dereferences the
    /// `end()` iterator of an empty block. Both are unreachable through
    /// `SiblingFuncletInfo`, whose only writers store an `invoke`, a
    /// `catchswitch` or a `cleanupret` whose unwind destination has already
    /// been shown to start with an EH pad; `None` stands for those two.
    fn succ_pad(&self, f: FunctionValue<'ctx, Dyn, B>, terminator: ValueSlot) -> Option<ValueSlot> {
        let unwind_dest = self.terminator_unwind_dest(terminator)??;
        self.first_non_phi_in_block(f, unwind_dest)
    }

    /// `CallBase::doesNotThrow()` — `hasFnAttr(Attribute::NoUnwind)`, which
    /// reads the call site's own function attributes and then the called
    /// function's (`CallBase::hasFnAttrOnCalledFunction`).
    fn call_does_not_throw(&self, callee: ValueSlot, attrs: &CallAttributeData) -> bool {
        if attrs
            .function_attrs()
            .has_kind(AttrIndex::Function, AttrKind::NoUnwind)
        {
            return true;
        }
        match &self.module.context().value_data(callee).kind {
            ValueKindData::Function(data) => data
                .attributes
                .borrow()
                .has_kind(AttrIndex::Function, AttrKind::NoUnwind),
            _ => false,
        }
    }

    /// `Verifier::visitEHPadPredecessors` (`lib/IR/Verifier.cpp`), whole.
    ///
    /// `pad` is upstream's `Instruction &I`, and `bb` its parent block.
    fn visit_eh_pad_predecessors(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        pad: ValueSlot,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        let block = bb.slot();
        let no_predecessors: Vec<ValueSlot> = Vec::new();
        let predecessors = cx.predecessors.get(&block).unwrap_or(&no_predecessors);

        // `Check(BB != &F->getEntryBlock(), "EH pad cannot be in entry
        //  block.", &I);`
        let is_entry_block = f
            .basic_blocks()
            .next()
            .is_some_and(|entry| entry.slot() == block);
        self.verifier_check(
            f,
            bb,
            !is_entry_block,
            VerifierRule::EhPadPredecessorEdge,
            "EH pad cannot be in entry block.",
        )?;

        let ValueKindData::Instruction(pad_instruction) =
            &self.module.context().value_data(pad).kind
        else {
            unreachable!("visitEHPadPredecessors is only reached from an EH pad instruction");
        };

        // `if (auto *LPI = dyn_cast<LandingPadInst>(&I)) { … return; }`
        if matches!(pad_instruction.kind, InstructionKindData::LandingPad(_)) {
            for &predecessor in predecessors {
                let is_unwind_edge_of_invoke = self
                    .block_terminator(predecessor)
                    .and_then(|terminator| {
                        match &self.module.context().value_data(terminator).kind {
                            ValueKindData::Instruction(instruction) => Some(&instruction.kind),
                            _ => None,
                        }
                    })
                    .is_some_and(|kind| match kind {
                        InstructionKindData::Invoke(invoke) => {
                            invoke.unwind_dest.get() == block && invoke.normal_dest.get() != block
                        }
                        _ => false,
                    });
                // `Check(II && II->getUnwindDest() == BB &&
                //  II->getNormalDest() != BB, "Block containing LandingPadInst
                //  must be jumped to only by the unwind edge of an invoke.",
                //  LPI);`
                self.verifier_check(
                    f,
                    bb,
                    is_unwind_edge_of_invoke,
                    VerifierRule::EhPadPredecessorEdge,
                    "Block containing LandingPadInst must be jumped to only by the unwind edge of an invoke.",
                )?;
            }
            return Ok(());
        }

        // `if (auto *CPI = dyn_cast<CatchPadInst>(&I)) { … return; }`
        if let InstructionKindData::CatchPad(catch_pad) = &pad_instruction.kind {
            // `CPI->getCatchSwitch()` is `cast<CatchSwitchInst>(getParentPad())`,
            // which `visitCatchPadInst`'s `CatchPadInst needs to be directly
            // nested in a CatchSwitchInst.` `Check` has already established —
            // it runs before this routine.
            let Some(catch_switch) = catch_pad.parent_pad.get() else {
                unreachable!(
                    "visitCatchPadInst rejects a catchpad whose parent is not a catchswitch"
                )
            };
            // `if (!pred_empty(BB))
            //    Check(BB->getUniquePredecessor() ==
            //          CPI->getCatchSwitch()->getParent(), "Block containg
            //          CatchPadInst must be jumped to only by its
            //          catchswitch.", CPI);`
            if !predecessors.is_empty() {
                // `BasicBlock::getUniquePredecessor` answers the single
                // *distinct* predecessor, so a block that branches to `BB`
                // twice still has one.
                let first = predecessors[0];
                let unique_predecessor = predecessors
                    .iter()
                    .all(|&predecessor| predecessor == first)
                    .then_some(first);
                self.verifier_check(
                    f,
                    bb,
                    unique_predecessor.is_some()
                        && unique_predecessor == self.parent_block_of(catch_switch),
                    VerifierRule::EhPadPredecessorEdge,
                    "Block containg CatchPadInst must be jumped to only by its catchswitch.",
                )?;
            }
            // `Check(BB != CPI->getCatchSwitch()->getUnwindDest(),
            //  "Catchswitch cannot unwind to one of its catchpads",
            //  CPI->getCatchSwitch(), CPI);`
            let catch_switch_unwind_dest =
                self.terminator_unwind_dest(catch_switch).unwrap_or(None);
            self.verifier_check(
                f,
                bb,
                catch_switch_unwind_dest != Some(block),
                VerifierRule::EhPadPredecessorEdge,
                "Catchswitch cannot unwind to one of its catchpads",
            )?;
            return Ok(());
        }

        // "Verify that each pred has a legal terminator with a legal to/from
        //  EH pad relationship."
        //
        // `Instruction *ToPad = &I; Value *ToPadParent = getParentPad(ToPad);`
        let to_pad = pad;
        let to_pad_parent = self.parent_pad(to_pad);
        for &predecessor in predecessors {
            let Some(terminator) = self.block_terminator(predecessor) else {
                // A predecessor with no terminator is rejected by
                // `visit_block`'s `Basic Block does not have terminator!`.
                continue;
            };
            let ValueKindData::Instruction(terminator_instruction) =
                &self.module.context().value_data(terminator).kind
            else {
                unreachable!("block_terminator answers an instruction")
            };
            let from_pad: Option<ValueSlot> = match &terminator_instruction.kind {
                // `if (auto *II = dyn_cast<InvokeInst>(TI))`
                InstructionKindData::Invoke(invoke) => {
                    // `Check(II->getUnwindDest() == BB && II->getNormalDest()
                    //  != BB, "EH pad must be jumped to via an unwind edge",
                    //  ToPad, II);`
                    self.verifier_check(
                        f,
                        bb,
                        invoke.unwind_dest.get() == block && invoke.normal_dest.get() != block,
                        VerifierRule::EhPadPredecessorEdge,
                        "EH pad must be jumped to via an unwind edge",
                    )?;
                    // `auto *CalledFn = dyn_cast<Function>(
                    //      II->getCalledOperand()->stripPointerCasts());
                    //  if (CalledFn && CalledFn->isIntrinsic() &&
                    //      II->doesNotThrow() &&
                    //      !IntrinsicInst::mayLowerToFunctionCall(
                    //          CalledFn->getIntrinsicID()))
                    //    continue;`
                    let callee = invoke.callee.get();
                    let callee_value =
                        Value::<B>::from_parts(callee, self.module, self.value_type(callee));
                    let stripped = crate::pointer_analysis::strip_pointer_casts(callee_value);
                    let intrinsic_id = crate::intrinsics::descriptor_for_callee(stripped)
                        .map(|descriptor| descriptor.id());
                    if let Some(id) = intrinsic_id
                        && self.call_does_not_throw(stripped.slot(), &invoke.attrs)
                        && !crate::intrinsic_inst::may_lower_to_function_call(id)
                    {
                        continue;
                    }
                    // `if (auto Bundle = II->getOperandBundle(
                    //      LLVMContext::OB_funclet))
                    //    FromPad = Bundle->Inputs[0];
                    //  else FromPad = ConstantTokenNone::get(II->getContext());`
                    invoke
                        .attrs
                        .operand_bundles_slice()
                        .iter()
                        .find(|bundle| *bundle.tag() == OperandBundleTag::Funclet)
                        .and_then(|bundle| bundle.inputs().next())
                        .and_then(|input| self.pad_ref(input))
                }
                // `else if (auto *CRI = dyn_cast<CleanupReturnInst>(TI))`
                InstructionKindData::CleanupReturn(cleanup_return) => {
                    // `FromPad = CRI->getOperand(0);
                    //  Check(FromPad != ToPadParent, "A cleanupret must exit
                    //  its cleanup", CRI);`
                    let from_pad = self.pad_ref(cleanup_return.cleanup_pad.get());
                    self.verifier_check(
                        f,
                        bb,
                        from_pad != to_pad_parent,
                        VerifierRule::EhPadPredecessorEdge,
                        "A cleanupret must exit its cleanup",
                    )?;
                    from_pad
                }
                // `else if (auto *CSI = dyn_cast<CatchSwitchInst>(TI))
                //    FromPad = CSI;`
                InstructionKindData::CatchSwitch(_) => Some(terminator),
                // `else Check(false, "EH pad must be jumped to via an unwind
                //  edge", ToPad, TI);`
                _ => {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::EhPadPredecessorEdge,
                        "EH pad must be jumped to via an unwind edge".to_owned(),
                    ));
                }
            };

            // "The edge may exit from zero or more nested pads."
            // `SmallPtrSet<Value *, 8> Seen;
            //  for (;; FromPad = getParentPad(FromPad)) { … }`
            let mut seen: HashSet<ValueSlot> = HashSet::new();
            let mut from_pad = from_pad;
            loop {
                // `Check(FromPad != ToPad, "EH pad cannot handle exceptions
                //  raised within it", FromPad, TI);`
                self.verifier_check(
                    f,
                    bb,
                    from_pad != Some(to_pad),
                    VerifierRule::EhPadPredecessorEdge,
                    "EH pad cannot handle exceptions raised within it",
                )?;
                // `if (FromPad == ToPadParent) break;` — a legal unwind edge.
                if from_pad == to_pad_parent {
                    break;
                }
                // `Check(!isa<ConstantTokenNone>(FromPad), "A single unwind
                //  edge may only enter one EH pad", TI);`
                self.verifier_check(
                    f,
                    bb,
                    from_pad.is_some(),
                    VerifierRule::EhPadPredecessorEdge,
                    "A single unwind edge may only enter one EH pad",
                )?;
                let Some(current) = from_pad else {
                    unreachable!("the Check above returns when from_pad is token none")
                };
                // `Check(Seen.insert(FromPad).second, "EH pad jumps through a
                //  cycle of pads", FromPad);`
                self.verifier_check(
                    f,
                    bb,
                    seen.insert(current),
                    VerifierRule::EhPadPredecessorEdge,
                    "EH pad jumps through a cycle of pads",
                )?;
                // `Check(isa<FuncletPadInst>(FromPad) ||
                //  isa<CatchSwitchInst>(FromPad), "Parent pad must be
                //  catchpad/cleanuppad/catchswitch", TI);` — upstream's own
                // comment: "This will be diagnosed on the corresponding
                // instruction already. We need the extra check here to make
                // sure getParentPad() works."
                self.verifier_check(
                    f,
                    bb,
                    self.is_pad_or_catch_switch(current),
                    VerifierRule::EhPadPredecessorEdge,
                    "Parent pad must be catchpad/cleanuppad/catchswitch",
                )?;
                from_pad = self.parent_pad(current);
            }
        }
        Ok(())
    }

    /// `Verifier::visitLandingPadInst`.
    fn check_landing_pad(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &LandingPadInstData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        let clauses = d.clauses.borrow();
        // `Check(LPI.getNumClauses() > 0 || LPI.isCleanup(), "LandingPadInst
        //  needs at least one clause or to be a cleanup.", &LPI);`
        self.verifier_check(
            f,
            bb,
            !clauses.is_empty() || d.cleanup.get(),
            VerifierRule::EhPadInvalidStructure,
            "LandingPadInst needs at least one clause or to be a cleanup.",
        )?;

        // `visitEHPadPredecessors(LPI);`
        self.visit_eh_pad_predecessors(f, bb, inst.slot(), cx)?;

        // `if (!LandingPadResultTy) LandingPadResultTy = LPI.getType();
        //  else Check(LandingPadResultTy == LPI.getType(), "The landingpad
        //  instruction should have a consistent result type inside a
        //  function.", &LPI);`
        let result_ty = self.value_type(inst.slot());
        match cx.landing_pad_result_ty.get() {
            None => cx.landing_pad_result_ty.set(Some(result_ty)),
            Some(established) => self.verifier_check(
                f,
                bb,
                established == result_ty,
                VerifierRule::EhPadInvalidStructure,
                "The landingpad instruction should have a consistent result type inside a function.",
            )?,
        }

        // `Check(F->hasPersonalityFn(), "LandingPadInst needs to be in a
        //  function with a personality.", &LPI);`
        self.verifier_check(
            f,
            bb,
            f.personality_fn().is_some(),
            VerifierRule::EhPadMissingPersonality,
            "LandingPadInst needs to be in a function with a personality.",
        )?;

        // `Check(LPI.getParent()->getLandingPadInst() == &LPI, "LandingPadInst
        //  not the first non-PHI instruction in the block.", &LPI);` —
        // `BasicBlock::getLandingPadInst` is `dyn_cast<LandingPadInst>(
        // getFirstNonPHIIt())`, so this is the first-non-phi test its three
        // sibling routines spell directly.
        self.verifier_check(
            f,
            bb,
            self.first_non_phi_in_block(f, bb.slot()) == Some(inst.slot()),
            VerifierRule::EhPadInvalidStructure,
            "LandingPadInst not the first non-PHI instruction in the block.",
        )?;

        // `for (unsigned i = 0, e = LPI.getNumClauses(); i < e; ++i)`
        for (kind, clause) in clauses.iter() {
            let clause = clause.get();
            match kind {
                // `if (LPI.isCatch(i)) Check(isa<PointerType>(
                //  Clause->getType()), "Catch operand does not have pointer
                //  type!", &LPI);`
                LandingPadClauseKind::Catch => self.verifier_check(
                    f,
                    bb,
                    self.module
                        .context()
                        .type_data(self.value_type(clause))
                        .is_pointer_data(),
                    VerifierRule::EhPadInvalidStructure,
                    "Catch operand does not have pointer type!",
                )?,
                // `else { Check(LPI.isFilter(i), "Clause is neither catch nor
                //  filter!", &LPI); Check(isa<ConstantArray>(Clause) ||
                //  isa<ConstantAggregateZero>(Clause), "Filter operand is not
                //  an array of constants!", &LPI); }`
                //
                // The first of those two has no counterpart: upstream's
                // `ClauseType` is a two-valued enum stored per clause and
                // `isCatch`/`isFilter` are its two readers, so the `else` arm
                // *is* the filter arm and the `Check` can only fail on a
                // corrupted `LandingPadInst`. llvmkit stores the same two-valued
                // enum, so the branch is unrepresentable rather than merely
                // untaken.
                //
                // `isa<ConstantArray> || isa<ConstantAggregateZero>` collapses
                // to one test here: llvmkit spells a `zeroinitializer` as an
                // aggregate of zeros, so both are `ConstantData::Aggregate`.
                LandingPadClauseKind::Filter => self.verifier_check(
                    f,
                    bb,
                    matches!(
                        &self.module.context().value_data(clause).kind,
                        ValueKindData::Constant(ConstantData::Aggregate(_))
                    ),
                    VerifierRule::EhPadInvalidStructure,
                    "Filter operand is not an array of constants!",
                )?,
            }
        }
        Ok(())
    }

    /// `Verifier::visitResumeInst`.
    fn check_resume(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        d: &ResumeInstData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `Check(RI.getFunction()->hasPersonalityFn(), "ResumeInst needs to be
        //  in a function with a personality.", &RI);`
        self.verifier_check(
            f,
            bb,
            f.personality_fn().is_some(),
            VerifierRule::EhPadMissingPersonality,
            "ResumeInst needs to be in a function with a personality.",
        )?;
        // `if (!LandingPadResultTy) LandingPadResultTy =
        //  RI.getValue()->getType(); else Check(LandingPadResultTy ==
        //  RI.getValue()->getType(), "The resume instruction should have a
        //  consistent result type inside a function.", &RI);`
        let value_ty = self.value_type(d.value.get());
        match cx.landing_pad_result_ty.get() {
            None => cx.landing_pad_result_ty.set(Some(value_ty)),
            Some(established) => self.verifier_check(
                f,
                bb,
                established == value_ty,
                VerifierRule::EhPadInvalidStructure,
                "The resume instruction should have a consistent result type inside a function.",
            )?,
        }
        Ok(())
    }

    /// `Verifier::visitCatchPadInst`.
    fn check_catch_pad(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &CatchPadInstData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `Check(F->hasPersonalityFn(), "CatchPadInst needs to be in a
        //  function with a personality.", &CPI);`
        self.verifier_check(
            f,
            bb,
            f.personality_fn().is_some(),
            VerifierRule::EhPadMissingPersonality,
            "CatchPadInst needs to be in a function with a personality.",
        )?;
        // `Check(isa<CatchSwitchInst>(CPI.getParentPad()), "CatchPadInst needs
        //  to be directly nested in a CatchSwitchInst.", CPI.getParentPad());`
        let parent_is_catch_switch = d.parent_pad.get().is_some_and(|parent| {
            matches!(
                &self.module.context().value_data(parent).kind,
                ValueKindData::Instruction(instruction)
                    if matches!(instruction.kind, InstructionKindData::CatchSwitch(_))
            )
        });
        self.verifier_check(
            f,
            bb,
            parent_is_catch_switch,
            VerifierRule::EhPadInvalidStructure,
            "CatchPadInst needs to be directly nested in a CatchSwitchInst.",
        )?;
        // `Check(&*BB->getFirstNonPHIIt() == &CPI, "CatchPadInst not the first
        //  non-PHI instruction in the block.", &CPI);`
        self.verifier_check(
            f,
            bb,
            self.first_non_phi_in_block(f, bb.slot()) == Some(inst.slot()),
            VerifierRule::EhPadInvalidStructure,
            "CatchPadInst not the first non-PHI instruction in the block.",
        )?;
        // `visitEHPadPredecessors(CPI); visitFuncletPadInst(CPI);`
        self.visit_eh_pad_predecessors(f, bb, inst.slot(), cx)?;
        self.visit_funclet_pad(f, bb, inst.slot(), cx)
    }

    /// `Verifier::visitCatchReturnInst`.
    fn check_catch_return(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        d: &CatchReturnInstData,
    ) -> IrResult<()> {
        // `Check(isa<CatchPadInst>(CatchReturn.getOperand(0)),
        //  "CatchReturnInst needs to be provided a CatchPad", &CatchReturn,
        //  CatchReturn.getOperand(0));`
        let is_catch_pad = matches!(
            &self.module.context().value_data(d.catch_pad.get()).kind,
            ValueKindData::Instruction(instruction)
                if matches!(instruction.kind, InstructionKindData::CatchPad(_))
        );
        self.verifier_check(
            f,
            bb,
            is_catch_pad,
            VerifierRule::EhPadInvalidStructure,
            "CatchReturnInst needs to be provided a CatchPad",
        )
    }

    /// `Verifier::visitCleanupPadInst`.
    fn check_cleanup_pad(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &CleanupPadInstData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `Check(F->hasPersonalityFn(), "CleanupPadInst needs to be in a
        //  function with a personality.", &CPI);`
        self.verifier_check(
            f,
            bb,
            f.personality_fn().is_some(),
            VerifierRule::EhPadMissingPersonality,
            "CleanupPadInst needs to be in a function with a personality.",
        )?;
        // `Check(&*BB->getFirstNonPHIIt() == &CPI, "CleanupPadInst not the
        //  first non-PHI instruction in the block.", &CPI);`
        self.verifier_check(
            f,
            bb,
            self.first_non_phi_in_block(f, bb.slot()) == Some(inst.slot()),
            VerifierRule::EhPadInvalidStructure,
            "CleanupPadInst not the first non-PHI instruction in the block.",
        )?;
        // `auto *ParentPad = CPI.getParentPad();
        //  Check(isa<ConstantTokenNone>(ParentPad) ||
        //  isa<FuncletPadInst>(ParentPad), "CleanupPadInst has an invalid
        //  parent.", &CPI);`
        let parent_pad = d.parent_pad.get().and_then(|slot| self.pad_ref(slot));
        let parent_is_valid = match parent_pad {
            None => true,
            Some(parent) => matches!(
                &self.module.context().value_data(parent).kind,
                ValueKindData::Instruction(instruction)
                    if is_funclet_pad_kind(&instruction.kind)
            ),
        };
        self.verifier_check(
            f,
            bb,
            parent_is_valid,
            VerifierRule::EhPadInvalidStructure,
            "CleanupPadInst has an invalid parent.",
        )?;
        // `visitEHPadPredecessors(CPI); visitFuncletPadInst(CPI);`
        self.visit_eh_pad_predecessors(f, bb, inst.slot(), cx)?;
        self.visit_funclet_pad(f, bb, inst.slot(), cx)
    }

    /// `Verifier::visitCatchSwitchInst`.
    fn check_catch_switch(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        d: &CatchSwitchInstData,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `Check(F->hasPersonalityFn(), "CatchSwitchInst needs to be in a
        //  function with a personality.", &CatchSwitch);`
        self.verifier_check(
            f,
            bb,
            f.personality_fn().is_some(),
            VerifierRule::EhPadMissingPersonality,
            "CatchSwitchInst needs to be in a function with a personality.",
        )?;
        // `Check(&*BB->getFirstNonPHIIt() == &CatchSwitch, "CatchSwitchInst
        //  not the first non-PHI instruction in the block.", &CatchSwitch);`
        self.verifier_check(
            f,
            bb,
            self.first_non_phi_in_block(f, bb.slot()) == Some(inst.slot()),
            VerifierRule::EhPadInvalidStructure,
            "CatchSwitchInst not the first non-PHI instruction in the block.",
        )?;
        // `auto *ParentPad = CatchSwitch.getParentPad();
        //  Check(isa<ConstantTokenNone>(ParentPad) ||
        //  isa<FuncletPadInst>(ParentPad), "CatchSwitchInst has an invalid
        //  parent.", ParentPad);`
        let parent_pad = d.parent_pad.get().and_then(|slot| self.pad_ref(slot));
        let parent_is_valid = match parent_pad {
            None => true,
            Some(parent) => matches!(
                &self.module.context().value_data(parent).kind,
                ValueKindData::Instruction(instruction)
                    if is_funclet_pad_kind(&instruction.kind)
            ),
        };
        self.verifier_check(
            f,
            bb,
            parent_is_valid,
            VerifierRule::EhPadInvalidStructure,
            "CatchSwitchInst has an invalid parent.",
        )?;

        // `if (BasicBlock *UnwindDest = CatchSwitch.getUnwindDest()) { … }`
        if let Some(unwind_dest) = d.unwind_dest.get() {
            let first_non_phi = self.first_non_phi_in_block(f, unwind_dest);
            // `Check(I->isEHPad() && !isa<LandingPadInst>(I), "CatchSwitchInst
            //  must unwind to an EH block which is not a landingpad.",
            //  &CatchSwitch);`
            self.verifier_check(
                f,
                bb,
                first_non_phi.is_some_and(|slot| {
                    self.is_eh_pad_instruction(slot) && !self.is_landing_pad(slot)
                }),
                VerifierRule::EhPadInvalidStructure,
                "CatchSwitchInst must unwind to an EH block which is not a landingpad.",
            )?;
            // "Record catchswitch sibling unwinds for
            //  verifySiblingFuncletUnwinds" —
            // `if (getParentPad(&*I) == ParentPad)
            //    SiblingFuncletInfo[&CatchSwitch] = &CatchSwitch;`
            if let Some(unwind_pad) = first_non_phi
                && self.parent_pad(unwind_pad) == parent_pad
            {
                self.record_sibling_funclet(cx, inst.slot(), inst.slot());
            }
        }

        let handlers = d.handlers.borrow();
        // `Check(CatchSwitch.getNumHandlers() != 0, "CatchSwitchInst cannot
        //  have empty handler list", &CatchSwitch);`
        self.verifier_check(
            f,
            bb,
            !handlers.is_empty(),
            VerifierRule::EhPadInvalidStructure,
            "CatchSwitchInst cannot have empty handler list",
        )?;
        // `for (BasicBlock *Handler : CatchSwitch.handlers())
        //    Check(isa<CatchPadInst>(Handler->getFirstNonPHIIt()),
        //    "CatchSwitchInst handlers must be catchpads", &CatchSwitch,
        //    Handler);`
        for &handler in handlers.iter() {
            let handler_is_catch_pad =
                self.first_non_phi_in_block(f, handler).is_some_and(|slot| {
                    matches!(
                        &self.module.context().value_data(slot).kind,
                        ValueKindData::Instruction(instruction)
                            if matches!(instruction.kind, InstructionKindData::CatchPad(_))
                    )
                });
            self.verifier_check(
                f,
                bb,
                handler_is_catch_pad,
                VerifierRule::EhPadInvalidStructure,
                "CatchSwitchInst handlers must be catchpads",
            )?;
        }
        drop(handlers);

        // `visitEHPadPredecessors(CatchSwitch); visitTerminator(CatchSwitch);`
        self.visit_eh_pad_predecessors(f, bb, inst.slot(), cx)
    }

    /// `Verifier::visitCleanupReturnInst`.
    fn check_cleanup_return(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        d: &CleanupReturnInstData,
    ) -> IrResult<()> {
        // `Check(isa<CleanupPadInst>(CRI.getOperand(0)), "CleanupReturnInst
        //  needs to be provided a CleanupPad", &CRI, CRI.getOperand(0));`
        let is_cleanup_pad = matches!(
            &self.module.context().value_data(d.cleanup_pad.get()).kind,
            ValueKindData::Instruction(instruction)
                if matches!(instruction.kind, InstructionKindData::CleanupPad(_))
        );
        self.verifier_check(
            f,
            bb,
            is_cleanup_pad,
            VerifierRule::EhPadInvalidStructure,
            "CleanupReturnInst needs to be provided a CleanupPad",
        )?;
        // `if (BasicBlock *UnwindDest = CRI.getUnwindDest()) { … }`
        if let Some(unwind_dest) = d.unwind_dest {
            // `Check(I->isEHPad() && !isa<LandingPadInst>(I),
            //  "CleanupReturnInst must unwind to an EH block which is not a
            //  landingpad.", &CRI);`
            self.verifier_check(
                f,
                bb,
                self.first_non_phi_in_block(f, unwind_dest)
                    .is_some_and(|slot| {
                        self.is_eh_pad_instruction(slot) && !self.is_landing_pad(slot)
                    }),
                VerifierRule::EhPadInvalidStructure,
                "CleanupReturnInst must unwind to an EH block which is not a landingpad.",
            )?;
        }
        Ok(())
    }

    /// `SiblingFuncletInfo[pad] = terminator`, on the insertion-ordered `Vec`
    /// standing for upstream's `MapVector`.
    fn record_sibling_funclet(
        &self,
        cx: &FunctionContext<'_>,
        pad: ValueSlot,
        terminator: ValueSlot,
    ) {
        let mut info = cx.sibling_funclet_info.borrow_mut();
        match info.iter_mut().find(|(key, _)| *key == pad) {
            Some(entry) => entry.1 = terminator,
            None => info.push((pad, terminator)),
        }
    }

    /// `Verifier::visitFuncletPadInst` (`lib/IR/Verifier.cpp`), whole.
    ///
    /// The pad references upstream keeps in `Value *` are `Option<ValueSlot>`
    /// here, `None` for `ConstantTokenNone`; `UnresolvedAncestorPad` is a
    /// `Value *` that is separately *nullable*, so it is
    /// `Option<Option<ValueSlot>>` — the outer layer is upstream's null, the
    /// inner its token-none.
    fn visit_funclet_pad(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        fpi: ValueSlot,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `User *FirstUser = nullptr; Value *FirstUnwindPad = nullptr;
        //  SmallVector<FuncletPadInst *, 8> Worklist({&FPI});
        //  SmallPtrSet<FuncletPadInst *, 8> Seen;`
        let mut first_user: Option<ValueSlot> = None;
        let mut first_unwind_pad: Option<Option<ValueSlot>> = None;
        let mut worklist: Vec<ValueSlot> = vec![fpi];
        let mut seen: HashSet<ValueSlot> = HashSet::new();

        // `while (!Worklist.empty()) { FuncletPadInst *CurrentPad =
        //  Worklist.pop_back_val(); … }`
        while let Some(current_pad) = worklist.pop() {
            // `Check(Seen.insert(CurrentPad).second, "FuncletPadInst must not
            //  be nested within itself", CurrentPad);`
            self.verifier_check(
                f,
                bb,
                seen.insert(current_pad),
                VerifierRule::FuncletPadNesting,
                "FuncletPadInst must not be nested within itself",
            )?;
            // `Value *UnresolvedAncestorPad = nullptr;`
            let mut unresolved_ancestor_pad: Option<Option<ValueSlot>> = None;
            let current_pad_value =
                Value::<B>::from_parts(current_pad, self.module, self.value_type(current_pad));
            // `for (User *U : CurrentPad->users())`
            for user in current_pad_value.users() {
                let u = user.slot();
                let ValueKindData::Instruction(user_instruction) =
                    &self.module.context().value_data(u).kind
                else {
                    unreachable!("Value::users yields instructions")
                };
                // `BasicBlock *UnwindDest;` — the `if`/`else if` chain.
                let unwind_dest: Option<ValueSlot> = match &user_instruction.kind {
                    // `if (auto *CRI = dyn_cast<CleanupReturnInst>(U))
                    //    UnwindDest = CRI->getUnwindDest();`
                    InstructionKindData::CleanupReturn(cri) => cri.unwind_dest,
                    // `else if (auto *CSI = dyn_cast<CatchSwitchInst>(U)) {
                    //    if (CSI->unwindsToCaller()) continue;
                    //    UnwindDest = CSI->getUnwindDest(); }`
                    InstructionKindData::CatchSwitch(csi) => match csi.unwind_dest.get() {
                        None => continue,
                        dest => dest,
                    },
                    // `else if (auto *II = dyn_cast<InvokeInst>(U))
                    //    UnwindDest = II->getUnwindDest();`
                    InstructionKindData::Invoke(ii) => Some(ii.unwind_dest.get()),
                    // `else if (isa<CallInst>(U)) continue;`
                    InstructionKindData::Call(_) => continue,
                    // `else if (auto *CPI = dyn_cast<CleanupPadInst>(U)) {
                    //    Worklist.push_back(CPI); continue; }`
                    InstructionKindData::CleanupPad(_) => {
                        worklist.push(u);
                        continue;
                    }
                    // `else { Check(isa<CatchReturnInst>(U), "Bogus funclet pad
                    //  use", U); continue; }`
                    other => {
                        self.verifier_check(
                            f,
                            bb,
                            matches!(other, InstructionKindData::CatchReturn(_)),
                            VerifierRule::FuncletPadNesting,
                            "Bogus funclet pad use",
                        )?;
                        continue;
                    }
                };

                let unwind_pad: Option<ValueSlot>;
                let exits_fpi: bool;
                if let Some(unwind_dest) = unwind_dest {
                    // `UnwindPad = &*UnwindDest->getFirstNonPHIIt();
                    //  if (!cast<Instruction>(UnwindPad)->isEHPad()) continue;`
                    let Some(pad) = self.first_non_phi_in_block(f, unwind_dest) else {
                        continue;
                    };
                    if !self.is_eh_pad_instruction(pad) {
                        continue;
                    }
                    unwind_pad = Some(pad);
                    // `Value *UnwindParent = getParentPad(UnwindPad);
                    //  if (UnwindParent == CurrentPad) continue;`
                    let unwind_parent = self.parent_pad(pad);
                    if unwind_parent == Some(current_pad) {
                        continue;
                    }
                    // `Value *ExitedPad = CurrentPad; ExitsFPI = false;
                    //  do { … } while (!isa<ConstantTokenNone>(ExitedPad));`
                    let mut exited_pad: Option<ValueSlot> = Some(current_pad);
                    let mut exits = false;
                    loop {
                        // `if (ExitedPad == &FPI) { ExitsFPI = true;
                        //    UnresolvedAncestorPad = &FPI; break; }`
                        if exited_pad == Some(fpi) {
                            exits = true;
                            unresolved_ancestor_pad = Some(Some(fpi));
                            break;
                        }
                        // `Value *ExitedParent = getParentPad(ExitedPad);`
                        let Some(current_exited) = exited_pad else {
                            unreachable!("the loop condition below stops at token none")
                        };
                        let exited_parent = self.parent_pad(current_exited);
                        // `if (ExitedParent == UnwindParent) {
                        //    UnresolvedAncestorPad = ExitedParent; break; }`
                        if exited_parent == unwind_parent {
                            unresolved_ancestor_pad = Some(exited_parent);
                            break;
                        }
                        exited_pad = exited_parent;
                        if exited_pad.is_none() {
                            break;
                        }
                    }
                    exits_fpi = exits;
                } else {
                    // "Unwinding to caller exits all pads."
                    // `UnwindPad = ConstantTokenNone::get(FPI.getContext());
                    //  ExitsFPI = true; UnresolvedAncestorPad = &FPI;`
                    unwind_pad = None;
                    exits_fpi = true;
                    unresolved_ancestor_pad = Some(Some(fpi));
                }

                if exits_fpi {
                    // "This unwind edge exits FPI. Make sure it agrees with
                    //  other such edges."
                    if first_user.is_some() {
                        // `Check(UnwindPad == FirstUnwindPad, "Unwind edges out
                        //  of a funclet pad must have the same unwind dest",
                        //  &FPI, U, FirstUser);`
                        self.verifier_check(
                            f,
                            bb,
                            first_unwind_pad == Some(unwind_pad),
                            VerifierRule::FuncletPadNesting,
                            "Unwind edges out of a funclet pad must have the same unwind dest",
                        )?;
                    } else {
                        first_user = Some(u);
                        first_unwind_pad = Some(unwind_pad);
                        // "Record cleanup sibling unwinds for
                        //  verifySiblingFuncletUnwinds" —
                        // `if (isa<CleanupPadInst>(&FPI) &&
                        //     !isa<ConstantTokenNone>(UnwindPad) &&
                        //     getParentPad(UnwindPad) == getParentPad(&FPI))
                        //   SiblingFuncletInfo[&FPI] = cast<Instruction>(U);`
                        let fpi_is_cleanup_pad = matches!(
                            &self.module.context().value_data(fpi).kind,
                            ValueKindData::Instruction(instruction)
                                if matches!(instruction.kind, InstructionKindData::CleanupPad(_))
                        );
                        if fpi_is_cleanup_pad
                            && let Some(unwind_pad) = unwind_pad
                            && self.parent_pad(unwind_pad) == self.parent_pad(fpi)
                        {
                            self.record_sibling_funclet(cx, fpi, u);
                        }
                    }
                }
                // "Make sure we visit all uses of FPI, but for nested pads stop
                //  as soon as we know where they unwind to."
                // `if (CurrentPad != &FPI) break;`
                if current_pad != fpi {
                    break;
                }
            }

            // `if (UnresolvedAncestorPad) { … }`
            if let Some(unresolved_ancestor_pad) = unresolved_ancestor_pad {
                // `if (CurrentPad == UnresolvedAncestorPad) { assert(CurrentPad
                //  == &FPI); continue; }` — "When CurrentPad is FPI itself, we
                // don't mark it as resolved even if we've found an unwind edge
                // that exits it, because we need to verify all direct uses of
                // FPI."
                if unresolved_ancestor_pad == Some(current_pad) {
                    continue;
                }
                // "Pop off the worklist any nested pads that we've found an
                //  unwind destination for."
                let mut resolved_pad: Option<ValueSlot> = Some(current_pad);
                while let Some(&uncle_pad) = worklist.last() {
                    let ancestor_pad = self.parent_pad(uncle_pad);
                    // "Walk ResolvedPad up the ancestor list until we either
                    //  find the uncle's parent or the last resolved ancestor."
                    while resolved_pad != ancestor_pad {
                        // Upstream's `getParentPad(ResolvedPad)` asserts if the
                        // walk ever reaches `ConstantTokenNone`; it cannot,
                        // because `UnresolvedAncestorPad` is an ancestor of
                        // `ResolvedPad`. Stopping is llvmkit's answer at the
                        // point upstream asserts.
                        let Some(current_resolved) = resolved_pad else {
                            break;
                        };
                        let resolved_parent = self.parent_pad(current_resolved);
                        if resolved_parent == unresolved_ancestor_pad {
                            break;
                        }
                        resolved_pad = resolved_parent;
                    }
                    // "If the resolved ancestor search didn't find the uncle's
                    //  parent, then the uncle is not yet resolved."
                    if resolved_pad != ancestor_pad {
                        break;
                    }
                    worklist.pop();
                }
            }
        }

        // `if (FirstUnwindPad) { if (auto *CatchSwitch =
        //  dyn_cast<CatchSwitchInst>(FPI.getParentPad())) { … } }` — the guard
        // is on the `Value *` being non-null, which a `ConstantTokenNone`
        // still is, so it tests "an exiting edge was seen at all".
        if let Some(first_unwind_pad) = first_unwind_pad {
            let parent_pad = self.parent_pad(fpi);
            let parent_is_catch_switch = parent_pad.is_some_and(|parent| {
                matches!(
                    &self.module.context().value_data(parent).kind,
                    ValueKindData::Instruction(instruction)
                        if matches!(instruction.kind, InstructionKindData::CatchSwitch(_))
                )
            });
            if parent_is_catch_switch {
                let Some(catch_switch) = parent_pad else {
                    unreachable!("parent_is_catch_switch implies a parent")
                };
                // `BasicBlock *SwitchUnwindDest = CatchSwitch->getUnwindDest();
                //  Value *SwitchUnwindPad = SwitchUnwindDest ?
                //    &*SwitchUnwindDest->getFirstNonPHIIt() :
                //    ConstantTokenNone::get(...);`
                let switch_unwind_pad = self
                    .terminator_unwind_dest(catch_switch)
                    .unwrap_or(None)
                    .and_then(|dest| self.first_non_phi_in_block(f, dest));
                // `Check(SwitchUnwindPad == FirstUnwindPad, "Unwind edges out
                //  of a catch must have the same unwind dest as the parent
                //  catchswitch", &FPI, FirstUser, CatchSwitch);`
                self.verifier_check(
                    f,
                    bb,
                    switch_unwind_pad == first_unwind_pad,
                    VerifierRule::FuncletPadNesting,
                    "Unwind edges out of a catch must have the same unwind dest as the parent catchswitch",
                )?;
            }
        }
        Ok(())
    }

    /// `Verifier::verifySiblingFuncletUnwinds` (`lib/IR/Verifier.cpp`).
    ///
    /// Upstream's `CycleNodes` walk is not reproduced: it exists only to build
    /// the value list `CheckFailed` prints beside the message, and
    /// `IrError::VerifierFailure` has no field for one. The cycle *detection*
    /// — the `Active` set — is unchanged.
    fn verify_sibling_funclet_unwinds(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // `SmallPtrSet<Instruction *, 8> Visited; SmallPtrSet<Instruction *, 8>
        //  Active;`
        let mut visited: HashSet<ValueSlot> = HashSet::new();
        let mut active: HashSet<ValueSlot> = HashSet::new();
        let info = cx.sibling_funclet_info.borrow().clone();
        // `for (const auto &Pair : SiblingFuncletInfo)`
        for (pred_pad, terminator) in &info {
            // `if (Visited.count(PredPad)) continue; Active.insert(PredPad);`
            if visited.contains(pred_pad) {
                continue;
            }
            active.insert(*pred_pad);
            let mut terminator = *terminator;
            // `do { … } while (true);`
            //
            // `getSuccPad` is upstream's first statement in the body and
            // cannot fail there; `None` here stands for the two shapes its
            // `cast` and its `getFirstNonPHIIt()` would assert on, so the
            // `while let` is upstream's `do { … } while (true)` with that
            // assertion spelled as an exit.
            while let Some(succ_pad) = self.succ_pad(f, terminator) {
                // `if (Active.count(SuccPad)) { … Check(false, "EH pads can't
                //  handle each other's exceptions", CycleNodes); }`
                if active.contains(&succ_pad) {
                    let report_at = self
                        .block_of(f, succ_pad)
                        .or_else(|| self.block_of(f, terminator));
                    let Some(report_at) = report_at else {
                        break;
                    };
                    return Err(self.fail(
                        f,
                        &report_at,
                        VerifierRule::FuncletPadNesting,
                        "EH pads can't handle each other's exceptions".to_owned(),
                    ));
                }
                // "Don't re-walk a node we've already checked" —
                // `if (!Visited.insert(SuccPad).second) break;`
                if !visited.insert(succ_pad) {
                    break;
                }
                // "Walk to this successor if it has a map entry."
                let Some((_, next_terminator)) = info.iter().find(|(key, _)| *key == succ_pad)
                else {
                    break;
                };
                terminator = *next_terminator;
                active.insert(succ_pad);
            }
            // "Each node only has one successor, so we've walked all the active
            //  nodes' successors."
            active.clear();
        }
        Ok(())
    }

    /// Cross-block SSA dominance. Mirrors `Verifier::verifyDominatesUse`,
    /// using `DominatorTree` directly rather than the analysis manager.
    fn check_dominates_uses(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        dom_tree: &DominatorTree,
    ) -> IrResult<()> {
        let operands = inst.operand_ids();
        for (index, op_id) in operands.into_iter().enumerate() {
            let op_data = self.module.context().value_data(op_id);
            let operand = crate::Value::from_parts(op_id, self.module, op_data.ty);
            let index = u32::try_from(index)
                .unwrap_or_else(|_| unreachable!("instruction operand index exceeds u32::MAX"));
            let use_edge = crate::Use::new(inst.as_erased(), operand, index);
            if !dom_tree.dominates_use(operand, use_edge) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::UseBeforeDef,
                    format!(
                        "Instruction does not dominate all uses! (operand %{} does not dominate its use in block %{})",
                        slot_label(f, op_id),
                        slot_label(f, bb.slot())
                    ),
                ));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Universal in-block invariants
    // ------------------------------------------------------------------

    /// Self-reference + in-block use-before-def. PHI is exempt because
    /// its incoming-pair operands are semantically uses on the
    /// predecessor edge, not at the phi's own slot.
    fn check_self_reference_and_in_block_dom(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        inst: &InstructionView<'ctx, B>,
        index_in_block: usize,
        block_instructions: &[InstructionView<'ctx, B>],
    ) -> IrResult<()> {
        let is_phi = matches!(inst.kind(), Some(crate::InstructionKind::Phi(_)));
        if is_phi {
            return Ok(());
        }
        let kind = match &inst.as_erased().data().kind {
            ValueKindData::Instruction(i) => &i.kind,
            _ => unreachable!("instruction handle invariant: value kind is Instruction"),
        };
        for op_id in kind.operand_ids() {
            // Self-reference (`Verifier/SelfReferential.ll`).
            if op_id == inst.slot() {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SelfReference,
                    "Only PHI nodes may reference their own value!".into(),
                ));
            }
            // In-block use-before-def. For operands that are themselves
            // instructions in the same block: the operand's index
            // must be strictly less than `index_in_block`.
            if let ValueKindData::Instruction(op_inst) =
                &self.module.context().value_data(op_id).kind
                && op_inst.parent.get() == bb.slot()
            {
                // Find op_id's index in block.
                if let Some(op_idx) = block_instructions.iter().position(|i| i.slot() == op_id)
                    && op_idx >= index_in_block
                {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::UseBeforeDef,
                        "Instruction does not dominate all uses! (operand defined after its \
                         use within the same block)"
                            .into(),
                    ));
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Diagnostic helpers
    // ------------------------------------------------------------------

    fn fail(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        rule: VerifierRule,
        message: String,
    ) -> IrError {
        IrError::VerifierFailure {
            rule,
            function: Some(f.name().to_owned()),
            block: bb.name(),
            message,
        }
    }

    fn value_type(&self, id: ValueSlot) -> TypeSlot {
        self.module.context().value_data(id).ty
    }

    fn type_label(&self, id: TypeSlot) -> String {
        format!("{}", Type::<B>::new(id, self.module))
    }

    /// Read the width of `ty`'s scalar integer type — `ty` itself when it is a
    /// scalar, its element when it is a vector — erroring with `message` if
    /// neither is an integer. Callers pass the `Check` literal of the
    /// per-opcode `Verifier::visit*Inst` they stand in for, so the diagnostic
    /// says `Trunc only operates on integer` where upstream does.
    ///
    /// Mirrors `Type::getScalarSizeInBits`, which is what
    /// `CastInst::castIsValid` compares for the integer casts. The caller must
    /// check the vector shapes separately; this deliberately says nothing
    /// about element counts.
    fn scalar_int_width_or_err(
        &self,
        f: FunctionValue<'ctx, Dyn, B>,
        bb: &BasicBlock<'ctx, Dyn, Unterminated, B>,
        ty: TypeSlot,
        message: &str,
    ) -> IrResult<u32> {
        let data = self.module.context().type_data(ty);
        let scalar = match data.as_vector() {
            Some((elem, _, _)) => self.module.context().type_data(elem),
            None => data,
        };
        match scalar.as_integer() {
            Some(bits) => Ok(bits),
            None => Err(self.fail(
                f,
                bb,
                VerifierRule::CastTypeMismatch,
                format!("{message} (got {})", self.type_label(ty)),
            )),
        }
    }
}

// --------------------------------------------------------------------------
// Predecessor map
// --------------------------------------------------------------------------

/// CFG predecessor map for one function. Mirrors LLVM's `pred_iterator`
/// exposed via `BasicBlock::pred_begin`.
///
/// Read straight off [`crate::cfg::FunctionCfg`] rather than re-derived by
/// transposing its edge list: the edge list is in block order and
/// `pred_iterator` is a use-list view, and re-deriving here is what let the
/// two disagree unnoticed.
/// The ten kinds `getParameterABIAttributes` copies, in its own order.
const PARAMETER_ABI_ATTR_KINDS: [AttrKind; 10] = [
    AttrKind::StructRet,
    AttrKind::ByVal,
    AttrKind::InAlloca,
    AttrKind::InReg,
    AttrKind::StackAlignment,
    AttrKind::SwiftSelf,
    AttrKind::SwiftAsync,
    AttrKind::SwiftError,
    AttrKind::Preallocated,
    AttrKind::ByRef,
];

/// Mirrors the file-local `getParameterABIAttributes` in
/// `lib/IR/Verifier.cpp`, reading the attributes recorded at `index`.
///
/// The result is keyed at `AttrIndex::Param(0)` whatever `index` was, so that
/// a *function*'s parameter set and a *call site*'s argument set — which
/// llvmkit stores one per argument, each at `Param(0)` — compare directly.
/// Upstream compares two `AttrBuilder`s, which carry no index at all.
fn parameter_abi_attributes(source: &AttributeStorage, index: AttrIndex) -> AttributeStorage {
    let mut copy = AttributeStorage::new();
    for kind in PARAMETER_ABI_ATTR_KINDS {
        // `Attribute Attr = Attrs.getParamAttrs(I).getAttribute(AK);
        //  if (Attr.isValid()) Copy.addAttribute(Attr);`
        if let Some(attr) = source
            .get(index)
            .and_then(|attrs| attrs.iter().find(|attr| attr.kind() == Some(kind)))
        {
            copy.add_stored(AttrIndex::Param(0), attr.clone());
        }
    }
    // "`align` is ABI-affecting only in combination with `byval` or `byref`."
    if source.has_kind(index, AttrKind::Alignment)
        && (source.has_kind(index, AttrKind::ByVal) || source.has_kind(index, AttrKind::ByRef))
        && let Some(align) = source.int_value(index, AttrKind::Alignment)
    {
        copy.add_stored(
            AttrIndex::Param(0),
            AttributeStored::Int(AttrKind::Alignment, align),
        );
    }
    copy
}

/// `getParameterABIAttributes(C, I, F->getAttributes())`.
fn parameter_abi_attributes_of_function(
    attrs: &AttributeStorage,
    index: usize,
) -> AttributeStorage {
    parameter_abi_attributes(
        attrs,
        AttrIndex::Param(u32::try_from(index).unwrap_or(u32::MAX)),
    )
}

/// `getParameterABIAttributes(C, I, CI.getAttributes())`. A call site's
/// per-argument attributes are stored one `AttributeStorage` per argument,
/// each keyed at `Param(0)`; an argument past the end carries none, which is
/// upstream's empty `AttributeSet` for an absent index.
fn parameter_abi_attributes_of_call_site(
    attrs: &crate::instr_types::CallAttributeData,
    index: usize,
) -> AttributeStorage {
    match attrs.arg_attrs().get(index) {
        Some(storage) => parameter_abi_attributes(storage, AttrIndex::Param(0)),
        None => AttributeStorage::new(),
    }
}

fn build_predecessors<B: ModuleBrand>(
    f: FunctionValue<'_, Dyn, B>,
) -> HashMap<ValueSlot, Vec<ValueSlot>> {
    let cfg = FunctionCfg::new(f);
    f.basic_blocks()
        .map(|bb| {
            (
                bb.slot(),
                cfg.predecessors(&bb.as_dyn())
                    .map(|pred| pred.slot())
                    .collect(),
            )
        })
        .collect()
}

// --------------------------------------------------------------------------
// Type predicates (lifetime-free, operate on TypeSlot via the context)
// --------------------------------------------------------------------------

/// Recursively detects whether a type contains any scalable vector.
/// Mirrors `Type::isScalableTy` in `llvm/lib/IR/Type.cpp`.
fn type_contains_scalable(m: &ModuleCore, ty: TypeSlot) -> bool {
    match m.context().type_data(ty) {
        TypeData::ScalableVector { .. } => true,
        TypeData::FixedVector { elem, .. } | TypeData::Array { elem, .. } => {
            type_contains_scalable(m, *elem)
        }
        TypeData::Struct(s) => match s.body.borrow().as_ref() {
            None => false,
            Some(body) => body.elements.iter().any(|e| type_contains_scalable(m, *e)),
        },
        _ => false,
    }
}

fn scalar_type_id(m: &ModuleCore, ty: TypeSlot) -> TypeSlot {
    match m.context().type_data(ty) {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => *elem,
        _ => ty,
    }
}

/// The element count and scalability of `ty`, or `None` when it is a scalar.
///
/// Two types agree in shape when this answers equal for both, which is how
/// `CastInst::castIsValid` phrases its vector rule: both operands vectors of
/// the same element count, or neither a vector.
fn vector_shape(m: &ModuleCore, ty: TypeSlot) -> Option<(u32, bool)> {
    m.context()
        .type_data(ty)
        .as_vector()
        .map(|(_, count, scalable)| (count, scalable))
}

fn is_int_or_int_vector(m: &ModuleCore, ty: TypeSlot) -> bool {
    let d = m.context().type_data(ty);
    if d.as_integer().is_some() {
        return true;
    }
    if let Some((elem, _, _)) = d.as_vector()
        && m.context().type_data(elem).as_integer().is_some()
    {
        return true;
    }
    false
}

enum AggWalkErr {
    NotAggregate(TypeSlot),
    OutOfRange { idx: u32, count: u32 },
}

fn walk_aggregate_path(
    m: &ModuleCore,
    root: TypeSlot,
    indices: &[u32],
) -> Result<TypeSlot, AggWalkErr> {
    let mut cur = root;
    for &idx in indices {
        let d = m.context().type_data(cur);
        match d {
            TypeData::Array { elem, n } => {
                let n_u32 = u32::try_from(*n).unwrap_or(u32::MAX);
                if idx >= n_u32 {
                    return Err(AggWalkErr::OutOfRange { idx, count: n_u32 });
                }
                cur = *elem;
            }
            TypeData::Struct(s) => {
                let body = s.body.borrow();
                match body.as_ref() {
                    Some(b) => {
                        let count = u32::try_from(b.elements.len()).unwrap_or(u32::MAX);
                        if idx >= count {
                            return Err(AggWalkErr::OutOfRange { idx, count });
                        }
                        let Ok(field_index) = usize::try_from(idx) else {
                            return Err(AggWalkErr::OutOfRange { idx, count });
                        };
                        cur = b.elements[field_index];
                    }
                    None => return Err(AggWalkErr::NotAggregate(cur)),
                }
            }
            _ => return Err(AggWalkErr::NotAggregate(cur)),
        }
    }
    Ok(cur)
}

fn is_fp_or_fp_vector(m: &ModuleCore, ty: TypeSlot) -> bool {
    let d = m.context().type_data(ty);
    if is_fp_data(d) {
        return true;
    }
    if let Some((elem, _, _)) = d.as_vector()
        && is_fp_data(m.context().type_data(elem))
    {
        return true;
    }
    false
}

fn is_pointer_or_pointer_vector(m: &ModuleCore, ty: TypeSlot) -> bool {
    let d = m.context().type_data(ty);
    if d.is_pointer_data() {
        return true;
    }
    if let Some((elem, _, _)) = d.as_vector()
        && m.context().type_data(elem).is_pointer_data()
    {
        return true;
    }
    false
}
fn pointer_source_shape(m: &ModuleCore, ty: TypeSlot) -> Option<(u32, Option<(u32, bool)>)> {
    match m.context().type_data(ty) {
        TypeData::Pointer { addr_space } => Some((*addr_space, None)),
        TypeData::FixedVector { elem, n } => match m.context().type_data(*elem) {
            TypeData::Pointer { addr_space } => Some((*addr_space, Some((*n, false)))),
            _ => None,
        },
        TypeData::ScalableVector { elem, min } => match m.context().type_data(*elem) {
            TypeData::Pointer { addr_space } => Some((*addr_space, Some((*min, true)))),
            _ => None,
        },
        _ => None,
    }
}

fn integer_result_shape(m: &ModuleCore, ty: TypeSlot) -> Option<(u32, Option<(u32, bool)>)> {
    match m.context().type_data(ty) {
        TypeData::Integer { bits } => Some((*bits, None)),
        TypeData::FixedVector { elem, n } => match m.context().type_data(*elem) {
            TypeData::Integer { bits } => Some((*bits, Some((*n, false)))),
            _ => None,
        },
        TypeData::ScalableVector { elem, min } => match m.context().type_data(*elem) {
            TypeData::Integer { bits } => Some((*bits, Some((*min, true)))),
            _ => None,
        },
        _ => None,
    }
}

fn is_i1(m: &ModuleCore, ty: TypeSlot) -> bool {
    matches!(m.context().type_data(ty).as_integer(), Some(1))
}

fn is_i1_vector(m: &ModuleCore, ty: TypeSlot) -> bool {
    if let Some((elem, _, _)) = m.context().type_data(ty).as_vector() {
        is_i1(m, elem)
    } else {
        false
    }
}

fn is_i1_data(d: &TypeData) -> bool {
    matches!(d.as_integer(), Some(1))
}

fn is_fp_data(d: &TypeData) -> bool {
    matches!(
        d,
        TypeData::Half
            | TypeData::Bfloat
            | TypeData::Float
            | TypeData::Double
            | TypeData::Fp128
            | TypeData::X86Fp80
            | TypeData::PpcFp128
    )
}

/// Floating-point precision rank for `fpext` / `fptrunc` ordering.
/// Mirrors LLVM's `Type::getFPMantissaWidth`-driven comparison.
/// `bfloat` and `half` share a width but bfloat has fewer mantissa
/// bits; LangRef accepts conversions in either direction so long as
/// they are not the identity, which the per-opcode width check
/// (`s != d`) catches separately.
fn fp_rank(m: &ModuleCore, ty: TypeSlot) -> Option<u32> {
    match m.context().type_data(ty) {
        TypeData::Half => Some(16),
        TypeData::Bfloat => Some(16),
        TypeData::Float => Some(32),
        TypeData::Double => Some(64),
        TypeData::X86Fp80 => Some(80),
        TypeData::Fp128 => Some(128),
        TypeData::PpcFp128 => Some(128),
        _ => None,
    }
}

/// Bit width of a value-bearing type, or `None` if it has no defined
/// width (function/void/label/...). Mirrors `Type::getPrimitiveSizeInBits`
/// for the cases bitcast cares about.
fn type_bit_width(m: &ModuleCore, ty: TypeSlot) -> Option<u32> {
    match m.context().type_data(ty) {
        TypeData::Integer { bits } => Some(*bits),
        TypeData::Half | TypeData::Bfloat => Some(16),
        TypeData::Float => Some(32),
        TypeData::Double => Some(64),
        TypeData::X86Fp80 => Some(80),
        TypeData::Fp128 | TypeData::PpcFp128 => Some(128),
        // Pointers don't have a portable bit-width here; LLVM uses the
        // data-layout. We don't ship a DataLayout yet, so two opaque
        // pointers in the same address space round-trip as bitcast
        // identity (caught by source==dest equality before width).
        TypeData::Pointer { .. } => None,
        TypeData::FixedVector { elem, n } => type_bit_width(m, *elem).map(|w| w * *n),
        _ => None,
    }
}

// --------------------------------------------------------------------------
// Slot label helper
// --------------------------------------------------------------------------

/// The text `AsmWriter` prints after the `%` for `id` inside `f`: its written
/// name, or the `SlotTracker` number an unnamed value or block is given.
///
/// Mirrors how `Verifier::CheckFailed` renders a `Value` — through
/// `WriteAsOperand`, which asks the module's `SlotTracker` for the number and
/// so always names something the reader can find in the printed IR. The
/// previous fallback was `format!("{:?}", block_id)`, the `Debug` of an
/// internal arena handle, which named nothing in the source or the output.
fn slot_label<B: ModuleBrand>(f: FunctionValue<'_, Dyn, B>, id: ValueSlot) -> String {
    let module = f.module();
    if let Some(name) = module.context().value_data(id).name.borrow().as_ref() {
        return name.clone();
    }
    let slots = crate::asm_writer::SlotTracker::for_function(f);
    match slots.local(id).or_else(|| slots.block(id)) {
        Some(number) => number.to_string(),
        // `AsmWriter`'s own spelling for a value it cannot number.
        None => "<badref>".to_owned(),
    }
}

// --------------------------------------------------------------------------
// TypeData crate-private helper trait
// --------------------------------------------------------------------------

/// Crate-private projections used only by the verifier. Live here so
/// `TypeData` does not grow new pub(crate) helpers that the rest of
/// the IR layer would not benefit from.
trait TypeDataExt {
    fn is_pointer_data(&self) -> bool;
    fn is_function_data(&self) -> bool;
}

impl TypeDataExt for TypeData {
    fn is_pointer_data(&self) -> bool {
        matches!(self, TypeData::Pointer { .. })
    }
    fn is_function_data(&self) -> bool {
        matches!(self, TypeData::Function { .. })
    }
}

// --------------------------------------------------------------------------
// Negative tests
// --------------------------------------------------------------------------
//
// The IrBuilder is sufficiently type-safe that most invalid IR shapes
// are unrepresentable through its public API. To exercise each
// `VerifierRule` we fabricate pathological IR by reaching into the
// crate-internal value arena directly. Each test cites the upstream
// `test/Verifier/<file>.ll` fixture whose CHECK rule it ports.

/// Upstream provenance: per-rule negative tests for `class Verifier` in
/// `lib/IR/Verifier.cpp`. Each `#[test]` ports a CHECK rule from
/// `test/Verifier/*.ll` (or the equivalent `Verifier::visit*` rule), with
/// the per-test doc comments naming the specific upstream fixture or
/// member function.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Linkage;
    use crate::constant::ConstantData;
    use crate::data_layout::DataLayout;
    use crate::function::FunctionValue;
    use crate::instr_types::{BinaryOpData, BranchInstData, BranchKind, PhiData, ReturnOpData};
    use crate::instruction::{InstructionKindData, build_instruction_value};
    use crate::marker::Dyn;
    use crate::module::Module;
    use crate::value::{ValueData, ValueKindData, ValueSlot};

    /// Append a fabricated instruction to a block, bypassing the
    /// IrBuilder's typestate. Returns the new instruction's value id.
    fn fabricate_instruction<B: crate::ModuleBrand>(
        m: &Module<B>,
        bb_id: ValueSlot,
        result_ty: TypeSlot,
        kind: InstructionKindData,
    ) -> ValueSlot {
        let m = m.core_ref();
        let v = build_instruction_value(result_ty, bb_id, kind, None);
        // `IrBuilder::append_instruction`'s use registration, verbatim —
        // `operand_ids()` extended with `block_operand_ids()`. Fabricating
        // without it left the block use-lists empty, so a `br` built here was
        // no predecessor at all to `predecessors(BB)` and to
        // `AssemblyWriter::printBasicBlock`'s `; preds = …`.
        let operand_ids = match &v.kind {
            ValueKindData::Instruction(i) => {
                let mut ids = i.kind.operand_ids();
                ids.extend(i.kind.block_operand_ids());
                ids
            }
            _ => Vec::new(),
        };
        let id = m.context().push_value(v);
        for op in operand_ids {
            m.context()
                .value_data(op)
                .add_use(crate::value::ValueUse::Instruction(id));
        }
        let bb_data = match &m.context().value_data(bb_id).kind {
            ValueKindData::BasicBlock(b) => b,
            _ => panic!("fabricate_instruction: bb_id is not a basic block"),
        };
        bb_data.instructions.borrow_mut().push(id);
        id
    }

    /// Push a fresh constant-int value of the given type.
    fn fab_const_int_id<B: crate::ModuleBrand>(
        m: &Module<B>,
        ty: TypeSlot,
        value: u64,
    ) -> ValueSlot {
        let m = m.core_ref();
        m.context().push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Int(Box::new([value]))),
            use_list: core::cell::RefCell::new(Vec::new()),
        })
    }

    /// Push a fresh `ptr null` value.
    fn fab_null_ptr_id<B: crate::ModuleBrand>(m: &Module<B>, ptr_ty: TypeSlot) -> ValueSlot {
        let m = m.core_ref();
        m.context().push_value(ValueData {
            ty: ptr_ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::PointerNull),
            use_list: core::cell::RefCell::new(Vec::new()),
        })
    }
    fn skeleton<'ctx, B: crate::ModuleBrand + 'ctx>(
        m: &Module<B>,
        ret_ty: crate::Type<'ctx, B>,
        params: &[crate::Type<'ctx, B>],
        name: &str,
    ) -> (ValueSlot, ValueSlot) {
        let fn_ty = m.function_type(ret_ty, params.iter().copied());
        let f = m.add_function_dyn(name, fn_ty, Linkage::External).unwrap();
        let bb = m.view(f).append_basic_block(m, "entry");
        // Reach the value-id pair without leaking the return marker.
        let f_id = {
            // FunctionValue<Dyn> has a private id field; widen via as_dyn.
            m.view(f).as_dyn().slot()
        };
        let bb_id = bb.as_dyn().slot();
        (f_id, bb_id)
    }

    /// Append a `ret void` to a block via direct fabrication.
    fn append_ret_void<B: crate::ModuleBrand>(m: &Module<B>, bb_id: ValueSlot) {
        fabricate_instruction(
            m,
            bb_id,
            m.void_type().as_type().id(),
            InstructionKindData::Ret(ReturnOpData::new(None)),
        );
    }

    fn assert_rule(err: &IrError, expected: VerifierRule) {
        match err {
            IrError::VerifierFailure { rule, .. } if *rule == expected => {}
            _ => panic!("expected VerifierRule::{expected:?}, got {err:?}"),
        }
    }

    /// [`assert_rule`] plus the `CHECK` text of the upstream fixture the test
    /// cites.
    ///
    /// The rule alone says nothing about the message, and the message is the
    /// half a `llvm/test/Verifier/*.ll` fixture is written against. Asserting
    /// only the rule is exactly what let every verifier diagnostic drift from
    /// `Verifier::CheckFailed`'s literal without a single test noticing.
    fn assert_rule_and_check_line(err: &IrError, expected: VerifierRule, check_line: &str) {
        assert_rule(err, expected);
        match err {
            IrError::VerifierFailure { message, .. } => assert!(
                message.contains(check_line),
                "message {message:?} lacks the fixture's CHECK text {check_line:?}"
            ),
            other => panic!("expected VerifierRule::{expected:?}, got {other:?}"),
        }
    }

    /// Mirrors `Verifier::visitFunction`: generated intrinsic declarations
    /// must carry the generated declaration attributes, not a subset with
    /// silently missing `immarg` / memory attributes.
    #[test]
    fn intrinsic_declaration_missing_generated_attrs_is_rejected() {
        let err = {
            let m = crate::module_new!("intrinsic-missing-attrs").expect("fresh module");
            let f = m
                .get_or_insert_intrinsic_declaration_by_name("llvm.abs.i32")
                .expect("intrinsic declaration");
            *m.view(f).data().attributes.borrow_mut() = AttributeStorage::new();
            m.verify_borrowed()
                .expect_err("missing generated attrs rejected")
        };

        match err {
            IrError::InvalidOperation { message } => {
                assert_eq!(message, "intrinsic declaration modifier")
            }
            other => panic!("unexpected verifier error: {other:?}"),
        }
    }

    /// Mirrors `Verifier::visitFunction`: intrinsic declaration attribute
    /// groups must resolve before generated attributes can be checked.
    #[test]
    fn intrinsic_declaration_extra_attr_group_is_rejected() {
        let err = {
            let m = crate::module_new!("intrinsic-extra-group").expect("fresh module");
            let f = m
                .get_or_insert_intrinsic_declaration_by_name("llvm.bswap.i32")
                .expect("intrinsic declaration");
            m.view(f).data().function_attr_groups.borrow_mut().push(0);
            m.verify_borrowed().expect_err("extra attr group rejected")
        };

        match err {
            IrError::InvalidOperation { message } => {
                assert_eq!(message, "intrinsic declaration modifier")
            }
            other => panic!("unexpected verifier error: {other:?}"),
        }
    }

    /// `test/Verifier/2002-04-13-RetTypes.ll` -- ret operand type
    /// (ptr) does not match function return type (i32).
    #[test]
    fn ret_type_mismatch_ptr_in_i32_function() {
        let m = crate::module_new!("t").expect("fresh module");
        let i32_ty = m.i32_type().as_type();
        let ptr_ty = m.ptr_type(0).as_type();
        let (_, bb_id) = skeleton(&m, i32_ty, &[], "f");
        let null_id = fab_null_ptr_id(&m, ptr_ty.id());
        fabricate_instruction(
            &m,
            bb_id,
            m.void_type().as_type().id(),
            InstructionKindData::Ret(ReturnOpData::new(Some(null_id))),
        );
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::ReturnTypeMismatch);
    }

    /// `test/Verifier/2008-11-15-RetVoid.ll` -- void function with a
    /// returned operand.
    #[test]
    fn ret_value_in_void_function() {
        let m = crate::module_new!("t").expect("fresh module");
        let void_ty = m.void_type().as_type();
        let i32_ty = m.i32_type().as_type();
        let (_, bb_id) = skeleton(&m, void_ty, &[], "f");
        let zero_id = fab_const_int_id(&m, i32_ty.id(), 0);
        fabricate_instruction(
            &m,
            bb_id,
            void_ty.id(),
            InstructionKindData::Ret(ReturnOpData::new(Some(zero_id))),
        );
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::ReturnTypeMismatch);
    }

    /// Binary operands have differing types: `add i32 %a, i64 %b`.
    /// Mirrors `Verifier::visitBinaryOperator` operand-equality rule.
    #[test]
    fn binary_operand_type_mismatch() {
        let m = crate::module_new!("t").expect("fresh module");
        let i32_ty = m.i32_type().as_type();
        let i64_ty = m.i64_type().as_type();
        let void_ty = m.void_type().as_type();
        let (f_id, bb_id) = skeleton(&m, void_ty, &[i32_ty, i64_ty], "f");
        let f = FunctionValue::<'_, Dyn, _>::from_parts_unchecked(f_id, m.as_view());
        let p0 = f.param(0).unwrap();
        let p1 = f.param(1).unwrap();
        fabricate_instruction(
            &m,
            bb_id,
            i32_ty.id(),
            InstructionKindData::Add(BinaryOpData::new(IsValue::slot(p0), IsValue::slot(p1))),
        );
        append_ret_void(&m, bb_id);
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::BinaryOperandsTypeMismatch);
    }

    /// Conditional branch with non-i1 condition.
    /// Mirrors `Verifier::visitBranchInst`.
    #[test]
    fn br_condition_not_i1() {
        let m = crate::module_new!("t").expect("fresh module");
        let void_ty = m.void_type().as_type();
        let i32_ty = m.i32_type().as_type();
        let (f_id, entry_id) = skeleton(&m, void_ty, &[i32_ty], "f");
        let f = FunctionValue::<'_, Dyn, _>::from_parts_unchecked(f_id, m.as_view());
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        append_ret_void(&m, then_bb.slot());
        append_ret_void(&m, else_bb.slot());
        let p0 = f.param(0).unwrap();
        fabricate_instruction(
            &m,
            entry_id,
            void_ty.id(),
            InstructionKindData::Br(BranchInstData {
                kind: core::cell::RefCell::new(BranchKind::Conditional {
                    cond: core::cell::Cell::new(IsValue::slot(p0)),
                    then_bb: then_bb.slot(),
                    else_bb: else_bb.slot(),
                }),
            }),
        );
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::BranchConditionNotI1);
    }

    /// Two terminators in a row -- second one is misplaced.
    /// Mirrors `Verifier::visitInstruction` terminator-position rule.
    #[test]
    fn misplaced_terminator() {
        let m = crate::module_new!("t").expect("fresh module");
        let void_ty = m.void_type().as_type();
        let (_, bb_id) = skeleton(&m, void_ty, &[], "f");
        for _ in 0..2 {
            append_ret_void(&m, bb_id);
        }
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::MisplacedTerminator);
    }

    /// `test/Verifier/PhiGrouping.ll` -- phi appears after a non-phi.
    ///
    /// The fixture itself is vendored at
    /// `crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/PhiGrouping.ll`
    /// but cannot be driven through the parser (`docs/divergences.md` entry
    /// 26), so its `CHECK` text is asserted here, on a block built in the
    /// arena.
    #[test]
    fn phi_not_at_top() {
        let m = crate::module_new!("t").expect("fresh module");
        let i32_ty = m.i32_type().as_type();
        let void_ty = m.void_type().as_type();
        let (f_id, entry_id) = skeleton(&m, void_ty, &[i32_ty, i32_ty], "f");
        let f = FunctionValue::<'_, Dyn, _>::from_parts_unchecked(f_id, m.as_view());
        let p0 = f.param(0).unwrap();
        let p1 = f.param(1).unwrap();
        fabricate_instruction(
            &m,
            entry_id,
            i32_ty.id(),
            InstructionKindData::Add(BinaryOpData::new(IsValue::slot(p0), IsValue::slot(p1))),
        );
        fabricate_instruction(
            &m,
            entry_id,
            i32_ty.id(),
            InstructionKindData::Phi(PhiData::new()),
        );
        append_ret_void(&m, entry_id);
        let err = m.verify_borrowed().unwrap_err();
        assert_rule_and_check_line(
            &err,
            VerifierRule::PhiNotAtTop,
            "PHI nodes not grouped at top",
        );
    }

    /// `test/Verifier/SelfReferential.ll` -- non-phi instruction whose
    /// operand is itself.
    #[test]
    fn self_reference_in_non_phi() {
        let m = crate::module_new!("t").expect("fresh module");
        let i32_ty = m.i32_type().as_type();
        let void_ty = m.void_type().as_type();
        let (_, bb_id) = skeleton(&m, void_ty, &[], "f");
        // Predict the next value-id by pushing a probe and reading
        // its arena index.
        let probe = fab_const_int_id(&m, i32_ty.id(), 0);
        let next_index = probe.arena_index() + 1;
        let next_id = ValueSlot::from_index(next_index);
        // Push an `add i32 next_id, probe` -- next_id IS this add's id.
        let pushed = fabricate_instruction(
            &m,
            bb_id,
            i32_ty.id(),
            InstructionKindData::Add(BinaryOpData::new(next_id, probe)),
        );
        assert_eq!(pushed, next_id, "id prediction must match arena order");
        append_ret_void(&m, bb_id);
        let err = m.verify_borrowed().unwrap_err();
        assert_rule_and_check_line(
            &err,
            VerifierRule::SelfReference,
            "Only PHI nodes may reference their own value",
        );
    }

    /// `Verifier::visitPHINode` -- "PHI nodes cannot have token type", plus the
    /// general rule that a phi result must be a first-class data type. The
    /// verifier now rejects an invalid phi *result* type before any coherence
    /// check, mirroring the `.ll` parser's parse-time rejection so the
    /// guarantee holds regardless of construction path (the raw phi builders
    /// are internal, but `phi_dyn`/`make_phi_in_block` still take an
    /// erased type).
    #[test]
    fn phi_with_invalid_result_type_rejected() {
        // `token`: LLVM's explicit "PHI nodes cannot have token type".
        {
            let m = crate::module_new!("t").expect("fresh module");
            let void_ty = m.void_type().as_type();
            let token_ty = m.token_type().as_type();
            let (_f_id, entry_id) = skeleton(&m, void_ty, &[], "f");
            fabricate_instruction(
                &m,
                entry_id,
                token_ty.id(),
                InstructionKindData::Phi(PhiData::new()),
            );
            append_ret_void(&m, entry_id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::PhiInvalidResultType);
        }
        // `void`: not a first-class type, so also not a valid phi result.
        let m = crate::module_new!("t").expect("fresh module");
        let void_ty = m.void_type().as_type();
        let (_f_id, entry_id) = skeleton(&m, void_ty, &[], "f");
        fabricate_instruction(
            &m,
            entry_id,
            void_ty.id(),
            InstructionKindData::Phi(PhiData::new()),
        );
        append_ret_void(&m, entry_id);
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::PhiInvalidResultType);
    }

    /// The result-type rule must NOT reject a *typed* pointer (`i32*`, the legacy
    /// `TypedPointerType`): it is a first-class data type and a valid phi result,
    /// distinct from the opaque `ptr` that `Type::is_pointer` matches. Regression
    /// guard — the first cut of this rule enumerated only `is_pointer()`, so it
    /// rejected `phi i32*`, IR that verified clean before.
    ///
    /// The phi is fabricated in a predecessor-less block so the incoming count
    /// matches the predecessor count on `0 == 0` and the case isolates the
    /// result-type gate, which runs ahead of the count delegation.
    #[test]
    fn phi_with_typed_pointer_result_type_verifies() {
        let m = crate::module_new!("t").expect("fresh module");
        let i32_ty = m.i32_type().as_type();
        let void_ty = m.void_type().as_type();
        let tptr_ty = m.typed_pointer_type(i32_ty, 0).as_type();
        let (f_id, entry_id) = skeleton(&m, void_ty, &[], "f");
        let f = FunctionValue::<'_, Dyn, _>::from_parts_unchecked(f_id, m.as_view());
        let dead = f.append_basic_block(&m, "dead");
        let dead_id = dead.slot();
        fabricate_instruction(
            &m,
            dead_id,
            tptr_ty.id(),
            InstructionKindData::Phi(PhiData::new()),
        );
        append_ret_void(&m, dead_id);
        append_ret_void(&m, entry_id);
        m.verify_borrowed()
            .expect("a typed-pointer phi result must remain valid");
    }

    /// `test/Verifier/AmbiguousPhi.ll` -- duplicate predecessor with
    /// differing values.
    ///
    /// The fixture itself is vendored at
    /// `crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/AmbiguousPhi.ll`
    /// but cannot be driven through the parser (`docs/divergences.md` entry
    /// 130), so its `CHECK` text is asserted here, on a phi built in the arena.
    #[test]
    fn ambiguous_phi_duplicate_predecessor() {
        let m = crate::module_new!("t").expect("fresh module");
        let i1_ty = m.bool_type().as_type();
        let i32_ty = m.i32_type().as_type();
        let void_ty = m.void_type().as_type();
        let (f_id, entry_id) = skeleton(&m, void_ty, &[i1_ty], "f");
        let f = FunctionValue::<'_, Dyn, _>::from_parts_unchecked(f_id, m.as_view());
        let target = f.append_basic_block(&m, "target");
        let cond_id = IsValue::slot(f.param(0).unwrap());
        fabricate_instruction(
            &m,
            entry_id,
            void_ty.id(),
            InstructionKindData::Br(BranchInstData {
                kind: core::cell::RefCell::new(BranchKind::Conditional {
                    cond: core::cell::Cell::new(cond_id),
                    then_bb: target.slot(),
                    else_bb: target.slot(),
                }),
            }),
        );
        let one = fab_const_int_id(&m, i32_ty.id(), 1);
        let two = fab_const_int_id(&m, i32_ty.id(), 2);
        let phi = PhiData::new();
        phi.incoming
            .borrow_mut()
            .push((core::cell::Cell::new(one), entry_id));
        phi.incoming
            .borrow_mut()
            .push((core::cell::Cell::new(two), entry_id));
        fabricate_instruction(
            &m,
            target.slot(),
            i32_ty.id(),
            InstructionKindData::Phi(phi),
        );
        append_ret_void(&m, target.slot());
        let err = m.verify_borrowed().unwrap_err();
        assert_rule_and_check_line(
            &err,
            VerifierRule::AmbiguousPhi,
            "multiple entries for the same basic block",
        );
    }

    /// Phi references a block that is not a CFG predecessor.
    #[test]
    fn phi_predecessor_mismatch() {
        let m = crate::module_new!("t").expect("fresh module");
        let i32_ty = m.i32_type().as_type();
        let void_ty = m.void_type().as_type();
        let (f_id, entry_id) = skeleton(&m, void_ty, &[], "f");
        let f = FunctionValue::<'_, Dyn, _>::from_parts_unchecked(f_id, m.as_view());
        let target = f.append_basic_block(&m, "target");
        let unrelated = f.append_basic_block(&m, "unrelated");
        fabricate_instruction(
            &m,
            entry_id,
            void_ty.id(),
            InstructionKindData::Br(BranchInstData {
                kind: core::cell::RefCell::new(BranchKind::Unconditional(target.slot())),
            }),
        );
        append_ret_void(&m, unrelated.slot());
        let bogus = fab_const_int_id(&m, i32_ty.id(), 7);
        let phi = PhiData::new();
        phi.incoming
            .borrow_mut()
            .push((core::cell::Cell::new(bogus), unrelated.slot()));
        fabricate_instruction(
            &m,
            target.slot(),
            i32_ty.id(),
            InstructionKindData::Phi(phi),
        );
        append_ret_void(&m, target.slot());
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::PhiPredecessorMismatch);
    }

    /// Call argument count mismatch -- non-vararg callee with wrong
    /// argc. Mirrors `Verifier::visitCallBase`.
    #[test]
    fn call_arg_count_mismatch() {
        let m = crate::module_new!("t").expect("fresh module");
        let i32_ty = m.i32_type().as_type();
        let void_ty = m.void_type().as_type();
        // Callee: `define i32 @callee(i32, i32)` -- empty body, terminator
        // fabricated to make it valid.
        let callee_fn_ty = m.function_type(i32_ty, [i32_ty, i32_ty]);
        let callee = m
            .add_function_dyn("callee", callee_fn_ty, Linkage::External)
            .unwrap();
        let cb = m.view(callee).append_basic_block(&m, "entry");
        let zero = fab_const_int_id(&m, i32_ty.id(), 0);
        fabricate_instruction(
            &m,
            cb.slot(),
            void_ty.id(),
            InstructionKindData::Ret(ReturnOpData::new(Some(zero))),
        );
        // Caller: passes only ONE arg.
        let caller_fn_ty = m.function_type(void_ty, [i32_ty]);
        let caller = m
            .add_function_dyn("caller", caller_fn_ty, Linkage::External)
            .unwrap();
        let entry = m.view(caller).append_basic_block(&m, "entry");
        let arg_id = IsValue::slot(m.view(caller).param(0).unwrap());
        fabricate_instruction(
            &m,
            entry.slot(),
            i32_ty.id(),
            InstructionKindData::Call(CallInstData::new(
                m.view(callee).slot(),
                callee_fn_ty.as_type().id(),
                [arg_id],
                crate::CallingConv::default(),
                crate::instr_types::TailCallKind::None,
            )),
        );
        append_ret_void(&m, entry.slot());
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::CallArgCountMismatch);
    }
    /// Mirrors `Verifier::visitPtrToAddrInst`: result integer width must match
    /// the `DataLayout` index width for the source pointer address space.
    #[test]
    fn ptrtoaddr_result_uses_index_width() {
        let m = crate::module_new!("t").expect("fresh module");
        m.set_data_layout(DataLayout::parse("p1:64:64:64:32").unwrap());
        let void_ty = m.void_type().as_type();
        let i64_ty = m.i64_type().as_type();
        let ptr1_ty = m.ptr_type(1).as_type();
        let (_f_id, bb_id) = skeleton(&m, void_ty, &[], "f");
        let ptr = fab_null_ptr_id(&m, ptr1_ty.id());
        fabricate_instruction(
            &m,
            bb_id,
            i64_ty.id(),
            InstructionKindData::Cast(CastOpData::new(CastOpcode::PtrToAddr, ptr)),
        );
        append_ret_void(&m, bb_id);
        let err = m.verify_borrowed().unwrap_err();
        assert_rule(&err, VerifierRule::CastTypeMismatch);
        match err {
            IrError::VerifierFailure { message, .. } => {
                assert!(message.contains("PtrToAddr result must be address width"));
            }
            _ => panic!("expected verifier failure"),
        }
    }
}
