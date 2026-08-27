//! Exception-handling personality classification and EH funclet colouring.
//!
//! Mirrors `llvm/include/llvm/IR/EHPersonalities.h` and
//! `llvm/lib/IR/EHPersonalities.cpp`, for the three entry points
//! `Verifier::visitIntrinsicCall`'s funclet-token arm reads:
//! `classifyEHPersonality`, `isScopedEHPersonality` and `colorEHFunclets`.
//! The rest of that file — `getEHPersonalityName`, `getDefaultEHPersonality`,
//! `isAsynchronousEHPersonality`, `isFuncletEHPersonality`,
//! `isNoOpWithoutInvoke`, `canSimplifyInvokeNoUnwind` — has no caller in
//! llvmkit today and is not ported.

use std::collections::HashMap;

use crate::constant::ConstantData;
use crate::function::FunctionValue;
use crate::instruction::InstructionKindData;
use crate::marker::Dyn;
use crate::module::ModuleBrand;
use crate::pointer_analysis::strip_pointer_casts;
use crate::r#type::TypeSlot;
use crate::value::{Value, ValueKindData, ValueSlot};

/// Mirrors `enum class EHPersonality`
/// (`llvm/include/llvm/IR/EHPersonalities.h`), in its own order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EhPersonality {
    Unknown,
    GnuAda,
    GnuC,
    GnuCSjLj,
    GnuCxx,
    GnuCxxSjLj,
    GnuObjC,
    MsvcX86Seh,
    MsvcTableSeh,
    MsvcCxx,
    CoreClr,
    Rust,
    WasmCxx,
    XlCxx,
    ZosCxx,
}

/// Mirrors `isScopedEHPersonality`
/// (`llvm/include/llvm/IR/EHPersonalities.h`): the personalities whose
/// exception handling is scoped, so that a call inside a funclet must name the
/// funclet it belongs to.
pub fn is_scoped_eh_personality(personality: EhPersonality) -> bool {
    matches!(
        personality,
        EhPersonality::MsvcCxx
            | EhPersonality::MsvcX86Seh
            | EhPersonality::MsvcTableSeh
            | EhPersonality::CoreClr
            | EhPersonality::WasmCxx
    )
}

/// The `(name, value type)` of a `GlobalValue`, or `None` for anything
/// `dyn_cast<GlobalValue>` would answer null for. `Function` spells its value
/// type `signature`; the other three spell it `value_type`.
///
/// Upstream a `GlobalValue` *is* the pointer-typed constant, so its `dyn_cast`
/// is direct. llvmkit interposes a [`ConstantData::GlobalValueRef`] standing
/// for `@name`, so the cast has to step through it first — a representation
/// difference, not a rule.
fn global_value_name_and_type<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(&'ctx str, TypeSlot)> {
    let value = match &value.data().kind {
        ValueKindData::Constant(ConstantData::GlobalValueRef { value: referent }) => {
            let module = value.module().core_ref();
            Value::from_parts(*referent, module, module.context().value_data(*referent).ty)
        }
        _ => value,
    };
    match &value.data().kind {
        ValueKindData::Function(data) => Some((data.name.as_str(), data.signature)),
        ValueKindData::GlobalVariable(data) => Some((data.name.as_str(), data.value_type)),
        ValueKindData::GlobalAlias(data) => Some((data.name.as_str(), data.value_type)),
        ValueKindData::GlobalIfunc(data) => Some((data.name.as_str(), data.value_type)),
        _ => None,
    }
}

/// Mirrors `classifyEHPersonality` (`llvm/lib/IR/EHPersonalities.cpp`).
///
/// Upstream's `Pers ? … : nullptr` guard is spelled by the caller here:
/// llvmkit's `FunctionValue::personality_fn` already answers `Option`, so a
/// function without a personality never reaches this routine.
pub fn classify_eh_personality<'ctx, B: ModuleBrand + 'ctx>(
    personality: Value<'ctx, B>,
) -> EhPersonality {
    // `const GlobalValue *F = dyn_cast<GlobalValue>(Pers->stripPointerCasts());
    //  if (!F || !F->getValueType() || !F->getValueType()->isFunctionTy())
    //    return EHPersonality::Unknown;`
    let stripped = strip_pointer_casts(personality);
    let Some((name, value_type)) = global_value_name_and_type(stripped) else {
        return EhPersonality::Unknown;
    };
    if stripped
        .module()
        .core_ref()
        .context()
        .type_data(value_type)
        .as_function()
        .is_none()
    {
        return EhPersonality::Unknown;
    }

    // `if (F->getParent()->getTargetTriple().isWindowsArm64EC())
    //    Name.consume_front("#");`
    //
    // llvmkit has no `Triple`, only the module's triple string.
    // `Triple::isWindowsArm64EC` is `getArch() == aarch64 && getSubArch() ==
    // AArch64SubArch_arm64ec`, and `arm64ec` is the single architecture
    // spelling `Triple::parseArch` / `parseSubArch` map to that pair, so the
    // architecture component alone decides it.
    let name = match stripped.module().core_ref().target_triple() {
        Some(triple) if triple.split('-').next() == Some("arm64ec") => {
            name.strip_prefix('#').unwrap_or(name)
        }
        _ => name,
    };

    // `StringSwitch<EHPersonality>(Name)`, case for case in upstream's order.
    match name {
        "__gnat_eh_personality" => EhPersonality::GnuAda,
        "__gxx_personality_v0" | "__gxx_personality_seh0" => EhPersonality::GnuCxx,
        "__gxx_personality_sj0" => EhPersonality::GnuCxxSjLj,
        "__gcc_personality_v0" | "__gcc_personality_seh0" => EhPersonality::GnuC,
        "__gcc_personality_sj0" => EhPersonality::GnuCSjLj,
        "__objc_personality_v0" => EhPersonality::GnuObjC,
        "_except_handler3" | "_except_handler4" => EhPersonality::MsvcX86Seh,
        "__C_specific_handler" => EhPersonality::MsvcTableSeh,
        "__CxxFrameHandler3" => EhPersonality::MsvcCxx,
        "ProcessCLRException" => EhPersonality::CoreClr,
        "__gxx_wasm_personality_v0" => EhPersonality::WasmCxx,
        "__xlcxx_personality_v1" => EhPersonality::XlCxx,
        "__zos_cxx_personality_v2" => EhPersonality::ZosCxx,
        // `.EndsWith("rust_eh_personality", EHPersonality::Rust)` — Rust
        // mangles its personality function, so upstream cannot test equality.
        // `StringSwitch` evaluates its cases in order and `EndsWith` is the
        // last one, so it is reached only after every `Case` has missed.
        _ if name.ends_with("rust_eh_personality") => EhPersonality::Rust,
        _ => EhPersonality::Unknown,
    }
}

/// `Instruction::isEHPad()` — `landingpad`, `catchpad`, `cleanuppad`,
/// `catchswitch`.
fn is_eh_pad(kind: &InstructionKindData) -> bool {
    matches!(
        kind,
        InstructionKindData::LandingPad(_)
            | InstructionKindData::CatchPad(_)
            | InstructionKindData::CleanupPad(_)
            | InstructionKindData::CatchSwitch(_)
    )
}

/// The instruction payload behind `slot`, or `None` when it is not one.
fn instruction_kind<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    slot: ValueSlot,
) -> Option<&'ctx InstructionKindData> {
    match &anchor.module().core_ref().context().value_data(slot).kind {
        ValueKindData::Instruction(instruction) => Some(&instruction.kind),
        _ => None,
    }
}

/// `isa<FuncletPadInst>` on an instruction payload — `catchpad` or
/// `cleanuppad`, the two `FuncletPadInst` subclasses.
pub(crate) fn is_funclet_pad_kind(kind: &InstructionKindData) -> bool {
    matches!(
        kind,
        InstructionKindData::CatchPad(_) | InstructionKindData::CleanupPad(_)
    )
}

/// `BasicBlock::getFirstNonPHIIt()`, projected to the instruction's payload —
/// `None` where upstream's iterator would be `end()`.
pub(crate) fn first_non_phi_kind<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Option<&'ctx InstructionKindData> {
    let ValueKindData::BasicBlock(data) =
        &anchor.module().core_ref().context().value_data(block).kind
    else {
        return None;
    };
    let instructions = data.instructions.borrow();
    instructions
        .iter()
        .copied()
        .find_map(|slot| match instruction_kind(anchor, slot) {
            Some(InstructionKindData::Phi(_)) => None,
            other => other.map(Some),
        })
        .flatten()
}

/// `Visiting->getTerminator()`, projected to its payload.
fn terminator_kind<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Option<&'ctx InstructionKindData> {
    let ValueKindData::BasicBlock(data) =
        &anchor.module().core_ref().context().value_data(block).kind
    else {
        return None;
    };
    let last = *data.instructions.borrow().last()?;
    instruction_kind(anchor, last)
}

/// `CatchReturnInst::getCatchSwitchParentPad()` —
/// `getCatchPad()->getCatchSwitch()->getParentPad()` — as `Option`, where
/// `None` is upstream's `ConstantTokenNone`.
///
/// Upstream's `cast<CatchSwitchInst>` on a `catchpad`'s parent pad asserts if
/// the pad is malformed. llvmkit answers `None` there instead, which routes the
/// caller to the entry block rather than crashing on IR the verifier is in the
/// middle of rejecting.
fn catch_switch_parent_pad<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    catch_pad: ValueSlot,
) -> Option<ValueSlot> {
    let Some(InstructionKindData::CatchPad(pad)) = instruction_kind(anchor, catch_pad) else {
        return None;
    };
    let Some(InstructionKindData::CatchSwitch(switch)) =
        instruction_kind(anchor, pad.parent_pad.get()?)
    else {
        return None;
    };
    switch.parent_pad.get()
}

/// The block an instruction belongs to — `cast<Instruction>(Pad)->getParent()`.
fn parent_block<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    slot: ValueSlot,
) -> Option<ValueSlot> {
    match &anchor.module().core_ref().context().value_data(slot).kind {
        ValueKindData::Instruction(instruction) => Some(instruction.parent.get()),
        _ => None,
    }
}

/// Mirrors `colorEHFunclets` (`llvm/lib/IR/EHPersonalities.cpp`).
///
/// Maps each block reachable from the entry block to the set of funclets that
/// must directly contain it — a "colour" being the first block of a funclet,
/// with the entry block standing for the function body itself. A catchswitch
/// counts as its own funclet, as upstream's comment says.
///
/// Only reachable blocks get an entry, exactly as upstream: the walk starts at
/// the entry block and follows successors. Upstream's caller then asserts on a
/// block with no colours; llvmkit's caller reads the missing entry as "not in a
/// funclet" instead — see `Verifier::check_intrinsic_call`.
pub fn color_eh_funclets<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, Dyn, B>,
) -> HashMap<ValueSlot, Vec<ValueSlot>> {
    let anchor = function.as_erased();
    let mut block_colors: HashMap<ValueSlot, Vec<ValueSlot>> = HashMap::new();
    // `BasicBlock *EntryBlock = &F->getEntryBlock();` — a declaration has none,
    // and upstream never colours one because `visitIntrinsicCall` only runs on
    // instructions inside a definition.
    let Some(entry_block) = function.basic_blocks().next() else {
        return block_colors;
    };
    let entry_block = entry_block.slot();

    let mut worklist: Vec<(ValueSlot, ValueSlot)> = vec![(entry_block, entry_block)];
    while let Some((visiting, color)) = worklist.pop() {
        // `BasicBlock::iterator VisitingHead = Visiting->getFirstNonPHIIt();
        //  if (VisitingHead->isEHPad()) Color = Visiting;`
        let color = match first_non_phi_kind(anchor, visiting) {
            Some(head) if is_eh_pad(head) => visiting,
            _ => color,
        };

        // `ColorVector &Colors = BlockColors[Visiting];
        //  if (!is_contained(Colors, Color)) Colors.push_back(Color);
        //  else continue;`
        let colors = block_colors.entry(visiting).or_default();
        if colors.contains(&color) {
            continue;
        }
        colors.push(color);

        // `BasicBlock *SuccColor = Color;
        //  Instruction *Terminator = Visiting->getTerminator();
        //  if (auto *CatchRet = dyn_cast<CatchReturnInst>(Terminator)) { … }`
        let terminator = terminator_kind(anchor, visiting);
        let mut successor_color = color;
        if let Some(InstructionKindData::CatchReturn(catch_return)) = terminator {
            successor_color = match catch_switch_parent_pad(anchor, catch_return.catch_pad.get()) {
                None => entry_block,
                Some(parent_pad) => parent_block(anchor, parent_pad).unwrap_or(entry_block),
            };
        }

        // `for (BasicBlock *Succ : successors(Visiting))
        //    Worklist.push_back({Succ, SuccColor});`
        if let Some(terminator) = terminator {
            for successor in crate::cfg::kind_successor_ids(terminator) {
                worklist.push((successor, successor_color));
            }
        }
    }
    block_colors
}
