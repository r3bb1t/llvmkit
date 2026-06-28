//! Textual `.ll` printer. Mirrors a slice of `llvm/lib/IR/AsmWriter.cpp`.
//!
//! Public surface is just the [`Display`](core::fmt::Display) impls on the IR
//! handles ([`Module`](crate::Module), [`FunctionValue`], [`BasicBlock`],
//! [`Instruction`](crate::Instruction), [`InstructionView`], [`Value`]). The slot-tracking and per-construct printers
//! stay `pub(super)` because consumers should reach for `format!("{module}")`
//! (or [`std::io::Write`] via `write!`) rather than poking at the internals.
//!
//! ## What's shipped
//!
//! - Modules, named structs, target directives, module asm, globals, aliases,
//!   ifuncs, functions, and COMDATs.
//! - Basic blocks with every shipped terminator family.
//! - Instructions across arithmetic, casts, compares, memory, GEP, calls,
//!   select, PHI, aggregate/vector operations, atomics, and EH / funclet pads.
//! - Constants: integer, float, undef, poison, null pointer, aggregates,
//!   global references, block addresses, inline asm, and constant expressions.
//! - Operand printing via slot numbering for unnamed values.
//! - Function/global attributes and calling conventions.

use core::fmt;
use core::fmt::Write as _;
use std::collections::HashMap;

use super::attributes::{AttributeStorage, AttributeStored};
use super::basic_block::BasicBlock;
use super::block_state::BlockSealState;
use super::constant::{ConstantData, ConstantExprData, ConstantExprFlags, ConstantExprOpcode};
use super::function::FunctionValue;
use super::global_alias::GlobalAlias;
use super::global_ifunc::GlobalIFunc;
use super::instr_types::{
    BinaryOpData, BranchInstData, BranchKind, CastOpData, CastOpcode, CmpInstData, PhiData,
    ReturnOpData,
};
use super::instruction::{InstructionKindData, InstructionView};
use super::marker::Dyn;
use super::module::{
    ModuleBrand, ModuleCore, ModuleView, UseListOrderBBRecord, UseListOrderRecord,
};
use super::r#type::{StructBody, Type, TypeData, TypeId};
use super::value::{Value, ValueId, ValueKindData};
use super::{ApInt, ApIntSignedness, AttrIndex};

// --------------------------------------------------------------------------
// SlotTracker
// --------------------------------------------------------------------------

/// Per-function slot map. Mirrors the private `SlotTracker` inside
/// `AsmWriter.cpp`. Walks values in declaration order, assigning a
/// 0-based slot to every *unnamed* one.
pub(super) struct SlotTracker {
    /// Local-scope slots: function arguments + instructions that
    /// produce a non-void result and lack a name.
    local: HashMap<ValueId, u32>,
    /// Basic-block slots: unnamed blocks get `; <label>:N`.
    blocks: HashMap<ValueId, u32>,
}

impl SlotTracker {
    /// Empty tracker for orphan IR (e.g. a [`BasicBlock`] not yet
    /// attached to a function).
    pub(super) fn empty() -> Self {
        Self {
            local: HashMap::new(),
            blocks: HashMap::new(),
        }
    }

    /// Build a slot tracker for a single function. Arguments come
    /// first, then each basic block (header counts as a value), then
    /// every instruction in program order.
    pub(super) fn for_function<B: ModuleBrand>(f: FunctionValue<'_, Dyn, B>) -> Self {
        let mut local = HashMap::new();
        let mut blocks = HashMap::new();
        let mut next: u32 = 0;

        for arg in f.params() {
            if arg.name().is_none() {
                local.insert(arg.as_value().id, next);
                next += 1;
            }
        }

        for bb in f.basic_blocks() {
            if bb.name().is_none() {
                blocks.insert(bb.as_value().id, next);
                next += 1;
            }
            for inst in bb.instructions() {
                if produces_named_result(&inst) && inst.name().is_none() {
                    local.insert(inst.as_value().id, next);
                    next += 1;
                }
            }
        }

        Self { local, blocks }
    }

    pub(super) fn local(&self, id: ValueId) -> Option<u32> {
        self.local.get(&id).copied()
    }

    pub(super) fn block(&self, id: ValueId) -> Option<u32> {
        self.blocks.get(&id).copied()
    }
}

/// `true` if `inst` produces a result that gets a textual name (or
/// slot). Terminators and stores don't.
fn produces_named_result(inst: &InstructionView<'_, impl ModuleBrand>) -> bool {
    match inst_kind_data(inst) {
        InstructionKindData::Add(_)
        | InstructionKindData::Sub(_)
        | InstructionKindData::Mul(_)
        | InstructionKindData::UDiv(_)
        | InstructionKindData::SDiv(_)
        | InstructionKindData::URem(_)
        | InstructionKindData::SRem(_)
        | InstructionKindData::Shl(_)
        | InstructionKindData::LShr(_)
        | InstructionKindData::AShr(_)
        | InstructionKindData::And(_)
        | InstructionKindData::Or(_)
        | InstructionKindData::Xor(_)
        | InstructionKindData::FAdd(_)
        | InstructionKindData::FSub(_)
        | InstructionKindData::FMul(_)
        | InstructionKindData::FDiv(_)
        | InstructionKindData::FRem(_)
        | InstructionKindData::FCmp(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Gep(_)
        | InstructionKindData::Select(_)
        | InstructionKindData::Cast(_)
        | InstructionKindData::ICmp(_)
        | InstructionKindData::FNeg(_)
        | InstructionKindData::Freeze(_)
        | InstructionKindData::VAArg(_)
        | InstructionKindData::ExtractValue(_)
        | InstructionKindData::InsertValue(_)
        | InstructionKindData::ExtractElement(_)
        | InstructionKindData::InsertElement(_)
        | InstructionKindData::ShuffleVector(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRMW(_)
        | InstructionKindData::Phi(_) => true,
        InstructionKindData::Fence(_) => false,
        InstructionKindData::Ret(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::Switch(_)
        | InstructionKindData::IndirectBr(_)
        | InstructionKindData::Resume(_)
        | InstructionKindData::CatchReturn(_)
        | InstructionKindData::CleanupReturn(_)
        | InstructionKindData::Unreachable(_) => false,
        InstructionKindData::Invoke(_)
        | InstructionKindData::Call(_)
        | InstructionKindData::CallBr(_) => !matches!(inst.ty().data(), TypeData::Void),
        InstructionKindData::CleanupPad(_) => true,
        InstructionKindData::CatchPad(_) => true,
        InstructionKindData::CatchSwitch(_) => true,
        InstructionKindData::LandingPad(_) => true,
    }
}

fn inst_kind_data<'ctx, B: ModuleBrand + 'ctx>(
    inst: &InstructionView<'ctx, B>,
) -> &'ctx InstructionKindData {
    match &inst.as_value().data().kind {
        ValueKindData::Instruction(i) => &i.kind,
        _ => unreachable!("Instruction handle invariant: kind is Instruction"),
    }
}

// --------------------------------------------------------------------------
// Operand printing
// --------------------------------------------------------------------------

/// Print a value as an operand: `<type> <ref>`, where `<ref>` is
/// `%name`, `@name`, `%slot`, or a constant literal.
pub(super) fn fmt_operand<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    v: Value<'ctx, B>,
    slots: Option<&SlotTracker>,
) -> fmt::Result {
    write!(f, "{} ", v.ty())?;
    fmt_operand_ref(f, v, slots)
}

/// Print just the SSA reference part: `%name` / `@name` / `%slot` /
/// constant body.
pub(super) fn fmt_operand_ref<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    v: Value<'ctx, B>,
    slots: Option<&SlotTracker>,
) -> fmt::Result {
    let data = v.data();
    match &data.kind {
        ValueKindData::Function(_) => fmt_global_value_ref(f, v),
        ValueKindData::BasicBlock(_) => match v.name() {
            Some(n) => fmt_llvm_name(f, "%", &n),
            None => match slots.and_then(|s| s.block(v.id)) {
                Some(slot) => write!(f, "%{slot}"),
                None => f.write_str("%<unnumbered>"),
            },
        },
        ValueKindData::Argument { .. } | ValueKindData::Instruction(_) => match v.name() {
            Some(n) => fmt_llvm_name(f, "%", &n),
            None => match slots.and_then(|s| s.local(v.id)) {
                Some(slot) => write!(f, "%{slot}"),
                None => f.write_str("%<unnumbered>"),
            },
        },
        ValueKindData::GlobalVariable(_)
        | ValueKindData::GlobalAlias(_)
        | ValueKindData::GlobalIFunc(_) => fmt_global_value_ref(f, v),
        ValueKindData::Constant(c) => fmt_constant(f, v, c),
        // `MetadataAsValue` delegates to the metadata printer. MDStrings
        // print inline as `!"..."`; MDNodes print as their numbered slot.
        ValueKindData::MetadataAsValue(id) => {
            let module_view = v.module();
            let module = module_view.core_ref();
            let md = module_view.metadata_store();
            fmt_metadata_operand(f, *id, module, &md, &metadata_slot_map(md.nodes()))
        }
        // An inline-asm value only ever appears as a `call` callee, where
        // `fmt_call` short-circuits to the `asm "...", "..."` form before
        // reaching here. If one is reached as a bare operand (it should
        // not be), print the `asm` body so the output is still
        // self-describing rather than panicking.
        ValueKindData::InlineAsm(d) => fmt_inline_asm(f, d),
    }
}
fn fmt_indexes(f: &mut fmt::Formatter<'_>, indexes: &[u32]) -> fmt::Result {
    f.write_str("{ ")?;
    for (i, idx) in indexes.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{idx}")?;
    }
    f.write_str(" }")
}

fn fmt_use_list_order(
    f: &mut fmt::Formatter<'_>,
    m: &ModuleCore,
    record: &UseListOrderRecord,
    slots: Option<&SlotTracker>,
) -> fmt::Result {
    let value_ty = record.value_type();
    write!(f, "uselistorder {} ", Type::new(value_ty, m))?;
    let value = Value::from_parts(record.value(), m, value_ty);
    fmt_operand_ref(f, value, slots)?;
    f.write_str(", ")?;
    fmt_indexes(f, record.indexes())
}

fn fmt_use_list_order_bb(
    f: &mut fmt::Formatter<'_>,
    m: &ModuleCore,
    record: &UseListOrderBBRecord,
) -> fmt::Result {
    let function_id = record.function();
    let function_data = m.context().value_data(function_id);
    let function_ty = function_data.ty;
    let block_ty = m.label_type().as_type().id();
    let function = Value::from_parts(function_id, m, function_ty);
    let block = Value::from_parts(record.block(), m, block_ty);
    let slots = match &function_data.kind {
        ValueKindData::Function(_) => Some(SlotTracker::for_function(
            FunctionValue::<Dyn>::from_parts_unchecked(function_id, m),
        )),
        _ => None,
    };
    f.write_str("uselistorder_bb ")?;
    fmt_operand_ref(f, function, None)?;
    f.write_str(", ")?;
    fmt_operand_ref(f, block, slots.as_ref())?;
    f.write_str(", ")?;
    fmt_indexes(f, record.indexes())
}

/// Print the `asm`-callee body shared by `fmt_operand_ref` and
/// `fmt_call`:
/// `asm [sideeffect ][alignstack ][inteldialect ]"<asm>", "<constraints>"`.
/// The leading `asm` token and the keyword set mirror
/// `AssemblyWriter::writeOperand`'s `InlineAsm` arm in
/// `lib/IR/AsmWriter.cpp`; the strings are escaped exactly like a
/// `module asm` line (see [`print_escaped_string`]).
fn fmt_inline_asm(f: &mut fmt::Formatter<'_>, d: &crate::inline_asm::InlineAsmData) -> fmt::Result {
    f.write_str("asm ")?;
    if d.has_side_effects {
        f.write_str("sideeffect ")?;
    }
    if d.is_align_stack {
        f.write_str("alignstack ")?;
    }
    if matches!(d.dialect, crate::inline_asm::AsmDialect::Intel) {
        f.write_str("inteldialect ")?;
    }
    if d.can_unwind {
        f.write_str("unwind ")?;
    }
    f.write_str("\"")?;
    print_escaped_string(f, d.asm_string.as_bytes())?;
    f.write_str("\", \"")?;
    print_escaped_string(f, d.constraint_string.as_bytes())?;
    f.write_str("\"")
}

// --------------------------------------------------------------------------
// Constant printing
// --------------------------------------------------------------------------

pub(super) fn fmt_constant<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    host: Value<'ctx, B>,
    c: &ConstantData,
) -> fmt::Result {
    match c {
        ConstantData::Int(words) => fmt_int_constant(f, host.ty(), words),
        ConstantData::Expr(expr) => fmt_constant_expr(f, host, expr),
        ConstantData::GlobalValueRef { value } => {
            let module = host.module;
            let global = Value::from_parts(*value, module, module.value_data(*value).ty);
            fmt_operand_ref(f, global, None)
        }
        ConstantData::Float(bits) => fmt_float_constant(f, host.ty(), *bits),
        ConstantData::PointerNull => f.write_str("null"),
        ConstantData::BlockAddressPlaceholder => f.write_str("<forward blockaddress>"),
        ConstantData::Undef => f.write_str("undef"),
        ConstantData::Poison => f.write_str("poison"),
        ConstantData::Aggregate(elems) => fmt_aggregate_constant(f, host, elems),
        ConstantData::BlockAddress { function, block } => {
            let module = host.module.module();
            let fval =
                Value::from_parts(*function, module, module.context().value_data(*function).ty);
            let bval = Value::from_parts(*block, module, module.context().value_data(*block).ty);
            f.write_str("blockaddress(")?;
            fmt_operand_ref(f, fval, None)?;
            f.write_str(", ")?;
            fmt_operand_ref(f, bval, None)?;
            f.write_str(")")
        }
        ConstantData::DSOLocalEquivalent { function } => {
            let module = host.module.module();
            let fval =
                Value::from_parts(*function, module, module.context().value_data(*function).ty);
            f.write_str("dso_local_equivalent ")?;
            fmt_operand_ref(f, fval, None)
        }
        ConstantData::NoCfi { function } => {
            let module = host.module.module();
            let fval =
                Value::from_parts(*function, module, module.context().value_data(*function).ty);
            f.write_str("no_cfi ")?;
            fmt_operand_ref(f, fval, None)
        }
        ConstantData::TokenNone => f.write_str("none"),
        ConstantData::TargetExtNone => f.write_str("zeroinitializer"),
        ConstantData::PtrAuth {
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        } => {
            let module = host.module.module();
            let disc_is_default = is_zero_int_constant(module, *discriminator);
            let addr_is_default = is_null_pointer_constant(module, *addr_discriminator);
            let ds_is_default = is_null_pointer_constant(module, *deactivation_symbol);
            let ids = [
                *pointer,
                *key,
                *discriminator,
                *addr_discriminator,
                *deactivation_symbol,
            ];
            let count = if disc_is_default && addr_is_default && ds_is_default {
                2
            } else if addr_is_default && ds_is_default {
                3
            } else if ds_is_default {
                4
            } else {
                5
            };
            f.write_str("ptrauth (")?;
            for (i, id) in ids[..count].iter().enumerate() {
                if i != 0 {
                    f.write_str(", ")?;
                }
                let data = module.context().value_data(*id);
                let value = Value::from_parts(*id, module, data.ty);
                fmt_operand(f, value, None)?;
            }
            f.write_str(")")
        }
        ConstantData::GepOffset { base_id, off } => {
            // `getelementptr inbounds (i8, <ptr-ty> @<base>, i64 <off>)`.
            // Mirrors `writeConstantInternal` printing each ConstantExpr
            // operand with its true type.
            let module = host.module.module();
            let base =
                Value::from_parts(*base_id, module, module.context().value_data(*base_id).ty);
            write!(
                f,
                "getelementptr inbounds (i8, {} ",
                constant_ptr_operand_type(base)
            )?;
            fmt_operand_ref(f, base, None)?;
            write!(f, ", i64 {off})")
        }
        ConstantData::SymbolDelta { hi_id, lo_id } => {
            let module = host.module.module();
            let hi = Value::from_parts(*hi_id, module, module.context().value_data(*hi_id).ty);
            let lo = Value::from_parts(*lo_id, module, module.context().value_data(*lo_id).ty);
            write!(f, "sub (i64 ptrtoint ({} ", constant_ptr_operand_type(hi))?;
            fmt_operand_ref(f, hi, None)?;
            write!(
                f,
                " to i64), i64 ptrtoint ({} ",
                constant_ptr_operand_type(lo)
            )?;
            fmt_operand_ref(f, lo, None)?;
            f.write_str(" to i64))")
        }
        ConstantData::SymbolDeltaPlus {
            hi_id,
            lo_id,
            addend,
        } => {
            let module = host.module.module();
            let hi = Value::from_parts(*hi_id, module, module.context().value_data(*hi_id).ty);
            let lo = Value::from_parts(*lo_id, module, module.context().value_data(*lo_id).ty);
            write!(
                f,
                "add (i64 sub (i64 ptrtoint ({} ",
                constant_ptr_operand_type(hi)
            )?;
            fmt_operand_ref(f, hi, None)?;
            write!(
                f,
                " to i64), i64 ptrtoint ({} ",
                constant_ptr_operand_type(lo)
            )?;
            fmt_operand_ref(f, lo, None)?;
            write!(f, " to i64)), i64 {addend})")
        }
    }
}

fn is_zero_int_constant(module: &ModuleCore, id: ValueId) -> bool {
    matches!(
        &module.context().value_data(id).kind,
        ValueKindData::Constant(ConstantData::Int(words)) if words.iter().all(|word| *word == 0)
    )
}

fn is_null_pointer_constant(module: &ModuleCore, id: ValueId) -> bool {
    matches!(
        &module.context().value_data(id).kind,
        ValueKindData::Constant(ConstantData::PointerNull)
    )
}

fn fmt_apint_signed(f: &mut fmt::Formatter<'_>, words: &[u64], bit_width: u32) -> fmt::Result {
    let value = ApInt::from_words(bit_width, words);
    f.write_str(&value.to_string_radix(10, ApIntSignedness::Signed))
}

fn fmt_constant_expr<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    host: Value<'ctx, B>,
    expr: &ConstantExprData,
) -> fmt::Result {
    let module = host.module();
    f.write_str(expr.opcode.keyword())?;
    match &expr.flags {
        ConstantExprFlags::None => {}
        ConstantExprFlags::Overflowing(flags) => {
            if flags.nuw() {
                f.write_str(" nuw")?;
            }
            if flags.nsw() {
                f.write_str(" nsw")?;
            }
        }
        ConstantExprFlags::Gep(flags) => {
            let no_wrap = flags.no_wrap();
            if !no_wrap.is_empty() {
                write!(f, " {}", no_wrap)?;
            }
            if let Some(in_range) = flags.in_range() {
                f.write_str(" inrange(")?;
                fmt_apint_signed(f, in_range.start(), in_range.bit_width())?;
                f.write_str(", ")?;
                fmt_apint_signed(f, in_range.end(), in_range.bit_width())?;
                f.write_str(")")?;
            }
        }
    }
    f.write_str(" (")?;
    if matches!(expr.opcode, ConstantExprOpcode::GetElementPtr) {
        let source_ty = expr
            .source_ty
            .unwrap_or_else(|| infer_gep_source_ty(module.core_ref(), expr));
        write!(f, "{}, ", Type::new(source_ty, module))?;
    }
    let mut first = true;
    for op_id in expr.operands.iter() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let data = module.context().value_data(*op_id);
        let value = Value::from_parts(*op_id, module, data.ty);
        fmt_operand(f, value, None)?;
    }
    if expr.opcode.is_cast() {
        write!(f, " to {}", Type::new(expr.result_ty, module))?;
    }
    if matches!(expr.opcode, ConstantExprOpcode::ShuffleVector) && !expr.mask.is_empty() {
        f.write_str(", <")?;
        for (i, m) in expr.mask.iter().enumerate() {
            if i != 0 {
                f.write_str(", ")?;
            }
            write!(f, "i32 {m}")?;
        }
        f.write_str(">")?;
    }
    f.write_str(")")
}
fn infer_gep_source_ty(module: &ModuleCore, expr: &ConstantExprData) -> TypeId {
    let Some(first) = expr.operands.first() else {
        return expr.result_ty;
    };
    let first_data = module.context().value_data(*first);
    if let ValueKindData::Constant(ConstantData::GlobalValueRef { value }) = &first_data.kind {
        return module.context().value_data(*value).ty;
    }
    expr.result_ty
}

fn constant_ptr_operand_type<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Type<'ctx, B> {
    match &value.data().kind {
        ValueKindData::Function(_) => value.module().ptr_type(0).as_type(),
        ValueKindData::GlobalAlias(_) | ValueKindData::GlobalIFunc(_) => value.ty(),
        _ => value.ty(),
    }
}

fn fmt_int_constant<B: ModuleBrand>(
    f: &mut fmt::Formatter<'_>,
    ty: Type<'_, B>,
    words: &[u64],
) -> fmt::Result {
    let bits = match ty.data() {
        TypeData::Integer { bits } => *bits,
        _ => unreachable!("integer-constant ty invariant"),
    };
    if bits == 1 {
        let v = words.first().copied().unwrap_or(0) & 1;
        return f.write_str(if v == 0 { "false" } else { "true" });
    }
    fmt_apint_signed(f, words, bits)
}

struct FloatDecimalBuffer {
    bytes: [u8; 32],
    len: usize,
}

impl FloatDecimalBuffer {
    fn new() -> Self {
        Self {
            bytes: [0; 32],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        match core::str::from_utf8(&self.bytes[..self.len]) {
            Ok(s) => s,
            Err(_) => unreachable!("float decimal buffer contains formatter output"),
        }
    }
}

impl fmt::Write for FloatDecimalBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let Some(end) = self.len.checked_add(s.len()) else {
            return Err(fmt::Error);
        };
        if end > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

// Mirrors `AsmWriter.cpp::writeAPFloatInternal`: finite single/double
// constants try the six-digit scientific spelling and keep it only when
// parsing that spelling returns the same `double` value.
fn try_write_finite_float_decimal(
    f: &mut fmt::Formatter<'_>,
    value: f64,
) -> Result<bool, fmt::Error> {
    if !value.is_finite() {
        return Ok(false);
    }

    let mut candidate = FloatDecimalBuffer::new();
    write!(&mut candidate, "{value:.5e}")?;
    let text = candidate.as_str();
    let Ok(reparsed) = text.parse::<f64>() else {
        return Ok(false);
    };
    if reparsed != value {
        return Ok(false);
    }

    write_llvm_float_decimal(f, text)?;
    Ok(true)
}

fn write_llvm_float_decimal(f: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    let Some(exp_index) = text.as_bytes().iter().position(|&b| b == b'e') else {
        return f.write_str(text);
    };
    f.write_str(&text[..exp_index])?;
    f.write_str("0e")?;

    let rest = &text[exp_index + 1..];
    let (sign, digits) = match rest.as_bytes().first() {
        Some(b'-') => ("-", &rest[1..]),
        Some(b'+') => ("+", &rest[1..]),
        _ => ("+", rest),
    };
    f.write_str(sign)?;
    for _ in digits.len()..2 {
        f.write_str("0")?;
    }
    f.write_str(digits)
}

fn low_u16(bits: u128) -> u16 {
    let bytes = bits.to_le_bytes();
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn low_u32(bits: u128) -> u32 {
    let bytes = bits.to_le_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn low_u64(bits: u128) -> u64 {
    let bytes = bits.to_le_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn fmt_float_constant<B: ModuleBrand>(
    f: &mut fmt::Formatter<'_>,
    ty: Type<'_, B>,
    bits: u128,
) -> fmt::Result {
    match ty.data() {
        TypeData::Half => write!(f, "0xH{:04X}", low_u16(bits)),
        TypeData::BFloat => write!(f, "0xR{:04X}", low_u16(bits)),
        TypeData::Float => {
            let value = f32::from_bits(low_u32(bits));
            if value.is_finite() && try_write_finite_float_decimal(f, f64::from(value))? {
                return Ok(());
            }
            let as_double_bits = f64::from(value).to_bits();
            write!(f, "0x{as_double_bits:016x}")
        }
        TypeData::Double => {
            let value = f64::from_bits(low_u64(bits));
            if value.is_finite() && try_write_finite_float_decimal(f, value)? {
                return Ok(());
            }
            write!(f, "0x{:016x}", value.to_bits())
        }
        TypeData::X86Fp80 => {
            let lo = low_u64(bits);
            let hi = low_u16(bits >> 64);
            write!(f, "0xK{hi:04X}{lo:016X}")
        }
        TypeData::Fp128 => {
            let lo = low_u64(bits);
            let hi = low_u64(bits >> 64);
            write!(f, "0xL{lo:016X}{hi:016X}")
        }
        TypeData::PpcFp128 => {
            let lo = low_u64(bits);
            let hi = low_u64(bits >> 64);
            write!(f, "0xM{lo:016X}{hi:016X}")
        }
        _ => unreachable!("float-constant ty invariant"),
    }
}

/// Mirrors `llvm::printEscapedString` in
/// `lib/Support/StringExtras.cpp`. Used both for c-string array
/// constants and for `section`/`partition` attributes on globals.
fn print_escaped_string(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for &c in bytes {
        if c == b'\\' {
            f.write_str("\\\\")?;
        } else if (0x20..=0x7e).contains(&c) && c != b'"' {
            f.write_str(
                core::str::from_utf8(&[c])
                    .unwrap_or_else(|_| unreachable!("printable ASCII is valid UTF-8")),
            )?;
        } else {
            write!(f, "\\{:02x}", c)?;
        }
    }
    Ok(())
}

fn fmt_llvm_name(f: &mut fmt::Formatter<'_>, prefix: &str, name: &str) -> fmt::Result {
    f.write_str(prefix)?;
    fmt_llvm_name_without_prefix(f, name)
}

fn fmt_llvm_name_without_prefix(f: &mut fmt::Formatter<'_>, name: &str) -> fmt::Result {
    let bytes = name.as_bytes();
    let needs_quotes = bytes.first().is_some_and(u8::is_ascii_digit)
        || bytes
            .iter()
            .any(|c| !c.is_ascii_alphanumeric() && !matches!(*c, b'-' | b'.' | b'_' | b'$'));
    if !needs_quotes {
        return f.write_str(name);
    }
    f.write_str("\"")?;
    print_escaped_string(f, bytes)?;
    f.write_str("\"")
}

fn fmt_global_value_ref<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    v: Value<'ctx, B>,
) -> fmt::Result {
    match v.name() {
        Some(name) => fmt_llvm_name(f, "@", &name),
        None => match module_global_slot(v.module().core_ref(), v.id) {
            Some(slot) => write!(f, "@{slot}"),
            None => f.write_str("@<unnumbered>"),
        },
    }
}

fn module_global_slot(module: &ModuleCore, id: ValueId) -> Option<u32> {
    let mut next = 0_u32;
    for global in module.iter_globals::<crate::module::Brand<'_>>() {
        if global.as_value().name().is_none() {
            if global.as_value().id == id {
                return Some(next);
            }
            next = next.saturating_add(1);
        }
    }
    for alias in module.iter_aliases::<crate::module::Brand<'_>>() {
        if alias.as_value().name().is_none() {
            if alias.as_value().id == id {
                return Some(next);
            }
            next = next.saturating_add(1);
        }
    }
    for ifunc in module.iter_ifuncs::<crate::module::Brand<'_>>() {
        if ifunc.as_value().name().is_none() {
            if ifunc.as_value().id == id {
                return Some(next);
            }
            next = next.saturating_add(1);
        }
    }
    for function in module.iter_functions::<crate::module::Brand<'_>>() {
        if function.as_value().name().is_none() {
            if function.as_value().id == id {
                return Some(next);
            }
            next = next.saturating_add(1);
        }
    }
    None
}

/// If the aggregate is `[N x i8]` and every element is a
/// `ConstantInt`, return the underlying byte sequence; else `None`.
/// Mirrors `ConstantDataArray::isString` (in C++ this is a runtime
/// downcast plus a per-element check).
fn collect_byte_string<B: ModuleBrand>(
    module: &crate::module::ModuleCore,
    ty: Type<'_, B>,
    elem_ids: &[ValueId],
) -> Option<Vec<u8>> {
    match ty.data() {
        TypeData::Array { elem, .. } => match module.context().type_data(*elem) {
            TypeData::Integer { bits: 8 } => {
                let mut bytes = Vec::with_capacity(elem_ids.len());
                for id in elem_ids {
                    let data = module.context().value_data(*id);
                    match &data.kind {
                        ValueKindData::Constant(ConstantData::Int(words)) => {
                            let v = words.first().copied().unwrap_or(0) & 0xff;
                            let Ok(byte) = u8::try_from(v) else {
                                return None;
                            };
                            bytes.push(byte);
                        }
                        _ => return None,
                    }
                }
                Some(bytes)
            }
            _ => None,
        },
        _ => None,
    }
}

fn is_zero_initializer_value(module: &ModuleCore, id: ValueId) -> bool {
    let data = module.context().value_data(id);
    match &data.kind {
        ValueKindData::Constant(ConstantData::Int(words)) => words.iter().all(|word| *word == 0),
        ValueKindData::Constant(ConstantData::Float(bits)) => *bits == 0,
        ValueKindData::Constant(ConstantData::PointerNull) => true,
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => elements
            .iter()
            .all(|element| is_zero_initializer_value(module, *element)),
        _ => false,
    }
}

fn aggregate_splat_id(elem_ids: &[ValueId]) -> Option<ValueId> {
    let first = elem_ids.first().copied()?;
    elem_ids.iter().all(|id| *id == first).then_some(first)
}

fn is_int_or_fp_splat_value(module: &ModuleCore, id: ValueId) -> bool {
    matches!(
        module.context().value_data(id).kind,
        ValueKindData::Constant(ConstantData::Int(_) | ConstantData::Float(_))
    )
}

fn fmt_aggregate_constant<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    host: Value<'ctx, B>,
    elem_ids: &[ValueId],
) -> fmt::Result {
    let module = host.module();
    let ty = host.ty();
    if let Some(bytes) = collect_byte_string(module.core_ref(), ty, elem_ids) {
        f.write_str("c\"")?;
        print_escaped_string(f, &bytes)?;
        return f.write_str("\"");
    }
    if elem_ids.is_empty() {
        return f.write_str("zeroinitializer");
    }
    if elem_ids
        .iter()
        .all(|id| is_zero_initializer_value(module.core_ref(), *id))
    {
        return f.write_str("zeroinitializer");
    }
    if matches!(
        ty.data(),
        TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }
    ) && let Some(splat) = aggregate_splat_id(elem_ids)
        && is_int_or_fp_splat_value(module.core_ref(), splat)
    {
        let data = module.context().value_data(splat);
        let value = Value::from_parts(splat, module, data.ty);
        f.write_str("splat (")?;
        fmt_operand(f, value, None)?;
        return f.write_str(")");
    }
    let (open, close) = match ty.data() {
        TypeData::Array { .. } => ("[", "]"),
        TypeData::Struct(s) => {
            let body = s.body.borrow();
            match body.as_ref() {
                Some(b) if b.packed => ("<{ ", " }>"),
                _ => ("{ ", " }"),
            }
        }
        TypeData::FixedVector { .. } | TypeData::ScalableVector { .. } => ("<", ">"),
        _ => unreachable!("aggregate constant ty invariant"),
    };
    f.write_str(open)?;
    let mut first = true;
    for id in elem_ids.iter() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let data = module.context().value_data(*id);
        let v = Value::from_parts(*id, module, data.ty);
        fmt_operand(f, v, None)?;
    }
    f.write_str(close)
}

// --------------------------------------------------------------------------
// Instruction printing
// --------------------------------------------------------------------------

pub(super) fn fmt_instruction(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    slots: &SlotTracker,
) -> fmt::Result {
    f.write_str("  ")?;
    let kind = inst_kind_data(inst);
    if produces_named_result(inst) {
        match inst.name() {
            Some(n) => {
                fmt_llvm_name(f, "%", &n)?;
                f.write_str(" = ")?;
            }
            None => match slots.local(inst.as_value().id) {
                Some(slot) => write!(f, "%{slot} = ")?,
                None => f.write_str("%<unnumbered> = ")?,
            },
        }
    }
    match kind {
        InstructionKindData::Add(b) => fmt_binop(f, "add", inst, b, slots),
        InstructionKindData::Sub(b) => fmt_binop(f, "sub", inst, b, slots),
        InstructionKindData::Mul(b) => fmt_binop(f, "mul", inst, b, slots),
        InstructionKindData::UDiv(b) => fmt_binop(f, "udiv", inst, b, slots),
        InstructionKindData::SDiv(b) => fmt_binop(f, "sdiv", inst, b, slots),
        InstructionKindData::URem(b) => fmt_binop(f, "urem", inst, b, slots),
        InstructionKindData::SRem(b) => fmt_binop(f, "srem", inst, b, slots),
        InstructionKindData::Shl(b) => fmt_binop(f, "shl", inst, b, slots),
        InstructionKindData::LShr(b) => fmt_binop(f, "lshr", inst, b, slots),
        InstructionKindData::AShr(b) => fmt_binop(f, "ashr", inst, b, slots),
        InstructionKindData::And(b) => fmt_binop(f, "and", inst, b, slots),
        InstructionKindData::Or(b) => fmt_binop(f, "or", inst, b, slots),
        InstructionKindData::Xor(b) => fmt_binop(f, "xor", inst, b, slots),
        InstructionKindData::FAdd(b) => fmt_binop(f, "fadd", inst, b, slots),
        InstructionKindData::FSub(b) => fmt_binop(f, "fsub", inst, b, slots),
        InstructionKindData::FMul(b) => fmt_binop(f, "fmul", inst, b, slots),
        InstructionKindData::FDiv(b) => fmt_binop(f, "fdiv", inst, b, slots),
        InstructionKindData::FRem(b) => fmt_binop(f, "frem", inst, b, slots),
        InstructionKindData::FCmp(c) => fmt_fcmp(f, inst, c, slots),
        InstructionKindData::Alloca(a) => fmt_alloca(f, inst, a, slots),
        InstructionKindData::Load(l) => fmt_load(f, inst, l, slots),
        InstructionKindData::Store(s) => fmt_store(f, inst, s, slots),
        InstructionKindData::Gep(g) => fmt_gep(f, inst, g, slots),
        InstructionKindData::Call(c) => fmt_call(f, inst, c, slots),
        InstructionKindData::Select(s) => fmt_select(f, inst, s, slots),
        InstructionKindData::Cast(c) => fmt_cast(f, inst, c, slots),
        InstructionKindData::ICmp(c) => fmt_icmp(f, inst, c, slots),
        InstructionKindData::Phi(p) => fmt_phi(f, inst, p, slots),
        InstructionKindData::Switch(d) => fmt_switch(f, inst, d, slots),
        InstructionKindData::IndirectBr(d) => fmt_indirectbr(f, inst, d, slots),
        InstructionKindData::Invoke(d) => fmt_invoke(f, inst, d, slots),
        InstructionKindData::CallBr(d) => fmt_callbr(f, inst, d, slots),
        InstructionKindData::LandingPad(d) => fmt_landingpad(f, inst, d, slots),
        InstructionKindData::Resume(d) => fmt_resume(f, inst, d, slots),
        InstructionKindData::CleanupPad(d) => {
            fmt_funclet_pad(f, inst, "cleanuppad", &d.parent_pad, &d.args, slots)
        }
        InstructionKindData::CatchPad(d) => {
            fmt_funclet_pad(f, inst, "catchpad", &d.parent_pad, &d.args, slots)
        }
        InstructionKindData::CatchReturn(d) => fmt_catchret(f, inst, d, slots),
        InstructionKindData::CleanupReturn(d) => fmt_cleanupret(f, inst, d, slots),
        InstructionKindData::CatchSwitch(d) => fmt_catchswitch(f, inst, d, slots),
        InstructionKindData::Br(b) => fmt_br(f, inst, b, slots),
        InstructionKindData::FNeg(u) => fmt_fneg(f, inst, u, slots),
        InstructionKindData::Freeze(u) => fmt_freeze(f, inst, u, slots),
        InstructionKindData::VAArg(u) => fmt_va_arg(f, inst, u, slots),
        InstructionKindData::ExtractValue(d) => fmt_extract_value(f, inst, d, slots),
        InstructionKindData::InsertValue(d) => fmt_insert_value(f, inst, d, slots),
        InstructionKindData::ExtractElement(d) => fmt_extract_element(f, inst, d, slots),
        InstructionKindData::InsertElement(d) => fmt_insert_element(f, inst, d, slots),
        InstructionKindData::ShuffleVector(d) => fmt_shuffle_vector(f, inst, d, slots),
        InstructionKindData::Fence(d) => fmt_fence(f, d),
        InstructionKindData::AtomicCmpXchg(d) => fmt_cmpxchg(f, inst, d, slots),
        InstructionKindData::AtomicRMW(d) => fmt_atomicrmw(f, inst, d, slots),
        InstructionKindData::Unreachable(_) => f.write_str("unreachable"),
        InstructionKindData::Ret(r) => fmt_ret(f, inst, r, slots),
    }?;
    let module_view = inst.module();
    let md = module_view.metadata_store();
    let md_slots = metadata_slot_map(md.nodes());
    fmt_metadata_attachments(f, &inst.metadata(), module_view.core_ref(), &md, &md_slots)
}

fn fmt_binop(
    f: &mut fmt::Formatter<'_>,
    opcode: &str,
    inst: &InstructionView<'_, impl ModuleBrand>,
    b: &BinaryOpData,
    slots: &SlotTracker,
) -> fmt::Result {
    f.write_str(opcode)?;
    if b.no_unsigned_wrap {
        f.write_str(" nuw")?;
    }
    if b.no_signed_wrap {
        f.write_str(" nsw")?;
    }
    if b.is_exact {
        f.write_str(" exact")?;
    }
    if b.disjoint {
        f.write_str(" disjoint")?;
    }
    // Mirrors `writeOptimizationInfo` in `lib/IR/AsmWriter.cpp`: an
    // `FPMathOperator` prints its FMF (` <flags>`) before the operands.
    if !b.fmf.is_empty() {
        write!(f, " {}", b.fmf)?;
    }
    f.write_str(" ")?;
    let module = inst.module();
    let lhs_data = module.context().value_data(b.lhs.get());
    let lhs = Value::from_parts(b.lhs.get(), module, lhs_data.ty);
    write!(f, "{} ", lhs.ty())?;
    fmt_operand_ref(f, lhs, Some(slots))?;
    f.write_str(", ")?;
    let rhs_data = module.context().value_data(b.rhs.get());
    let rhs = Value::from_parts(b.rhs.get(), module, rhs_data.ty);
    fmt_operand_ref(f, rhs, Some(slots))
}

fn fmt_cast(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    c: &CastOpData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `<keyword> <src-ty> <src-ref> to <dst-ty>`
    f.write_str(c.kind.keyword())?;
    match c.kind {
        CastOpcode::Trunc => {
            if c.nuw.get() {
                f.write_str(" nuw")?;
            }
            if c.nsw.get() {
                f.write_str(" nsw")?;
            }
        }
        CastOpcode::ZExt | CastOpcode::UIToFp if c.nneg.get() => {
            f.write_str(" nneg")?;
        }
        _ => {}
    }
    f.write_str(" ")?;
    let module = inst.module();
    let src_data = module.context().value_data(c.src.get());
    let src = Value::from_parts(c.src.get(), module, src_data.ty);
    write!(f, "{} ", src.ty())?;
    fmt_operand_ref(f, src, Some(slots))?;
    write!(f, " to {}", inst.ty())
}

fn fmt_fneg(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    u: &crate::instr_types::FNegInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `fneg [<fmf>] <ty> <src>` --- mirrors `printInstruction` /
    // `writeOptimizationInfo` in `lib/IR/AsmWriter.cpp`.
    f.write_str("fneg")?;
    if !u.fmf.is_empty() {
        f.write_str(" ")?;
        write!(f, "{}", u.fmf)?;
    }
    let module = inst.module();
    let src_data = module.context().value_data(u.src.get());
    let src = Value::from_parts(u.src.get(), module, src_data.ty);
    write!(f, " {} ", src.ty())?;
    fmt_operand_ref(f, src, Some(slots))
}

fn fmt_freeze(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    u: &crate::instr_types::FreezeInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `freeze <ty> <src>`
    f.write_str("freeze ")?;
    let module = inst.module();
    let src_data = module.context().value_data(u.src.get());
    let src = Value::from_parts(u.src.get(), module, src_data.ty);
    write!(f, "{} ", src.ty())?;
    fmt_operand_ref(f, src, Some(slots))
}

fn fmt_va_arg(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    u: &crate::instr_types::VAArgInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `va_arg <list-ty> <list-val>, <result-ty>`
    f.write_str("va_arg ")?;
    let module = inst.module();
    let src_data = module.context().value_data(u.src.get());
    let src = Value::from_parts(u.src.get(), module, src_data.ty);
    write!(f, "{} ", src.ty())?;
    fmt_operand_ref(f, src, Some(slots))?;
    write!(f, ", {}", inst.ty())
}

fn fmt_extract_value(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::ExtractValueInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `extractvalue <agg-ty> <agg>, idx0, idx1, ...`
    // Mirrors the `dyn_cast<ExtractValueInst>` branch of
    // `printInstruction` in `lib/IR/AsmWriter.cpp`.
    f.write_str("extractvalue ")?;
    let module = inst.module();
    let agg_id = d.aggregate.get();
    let agg_data = module.context().value_data(agg_id);
    let agg = Value::from_parts(agg_id, module, agg_data.ty);
    write!(f, "{} ", agg.ty())?;
    fmt_operand_ref(f, agg, Some(slots))?;
    for idx in d.indices.iter() {
        write!(f, ", {idx}")?;
    }
    Ok(())
}

fn fmt_insert_value(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::InsertValueInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, idx0, idx1, ...`
    f.write_str("insertvalue ")?;
    let module = inst.module();
    let agg_id = d.aggregate.get();
    let agg_data = module.context().value_data(agg_id);
    let agg = Value::from_parts(agg_id, module, agg_data.ty);
    write!(f, "{} ", agg.ty())?;
    fmt_operand_ref(f, agg, Some(slots))?;
    f.write_str(", ")?;
    let val_id = d.value.get();
    let val_data = module.context().value_data(val_id);
    let val = Value::from_parts(val_id, module, val_data.ty);
    write!(f, "{} ", val.ty())?;
    fmt_operand_ref(f, val, Some(slots))?;
    for idx in d.indices.iter() {
        write!(f, ", {idx}")?;
    }
    Ok(())
}

fn fmt_extract_element(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::ExtractElementInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `extractelement <vec-ty> <vec>, <idx-ty> <idx>`
    // Falls through `printInstruction`'s default branch with
    // PrintAllTypes=true (vector type != index type).
    f.write_str("extractelement ")?;
    let module = inst.module();
    let vec_id = d.vector.get();
    let vec_data = module.context().value_data(vec_id);
    let vec = Value::from_parts(vec_id, module, vec_data.ty);
    write!(f, "{} ", vec.ty())?;
    fmt_operand_ref(f, vec, Some(slots))?;
    f.write_str(", ")?;
    let idx_id = d.index.get();
    let idx_data = module.context().value_data(idx_id);
    let idx = Value::from_parts(idx_id, module, idx_data.ty);
    write!(f, "{} ", idx.ty())?;
    fmt_operand_ref(f, idx, Some(slots))
}

fn fmt_insert_element(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::InsertElementInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `insertelement <vec-ty> <vec>, <elt-ty> <elt>, <idx-ty> <idx>`
    f.write_str("insertelement ")?;
    let module = inst.module();
    let vec_id = d.vector.get();
    let vec_data = module.context().value_data(vec_id);
    let vec = Value::from_parts(vec_id, module, vec_data.ty);
    write!(f, "{} ", vec.ty())?;
    fmt_operand_ref(f, vec, Some(slots))?;
    f.write_str(", ")?;
    let val_id = d.value.get();
    let val_data = module.context().value_data(val_id);
    let val = Value::from_parts(val_id, module, val_data.ty);
    write!(f, "{} ", val.ty())?;
    fmt_operand_ref(f, val, Some(slots))?;
    f.write_str(", ")?;
    let idx_id = d.index.get();
    let idx_data = module.context().value_data(idx_id);
    let idx = Value::from_parts(idx_id, module, idx_data.ty);
    write!(f, "{} ", idx.ty())?;
    fmt_operand_ref(f, idx, Some(slots))
}

fn fmt_shuffle_vector(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::ShuffleVectorInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `shufflevector <ty> <v1>, <ty> <v2>, <mask>` --- the mask is
    // emitted via `printShuffleMask` in `lib/IR/AsmWriter.cpp`. The
    // result type is `<N x i32>` where N == mask length.
    f.write_str("shufflevector ")?;
    let module = inst.module();
    let l_id = d.lhs.get();
    let l_data = module.context().value_data(l_id);
    let l = Value::from_parts(l_id, module, l_data.ty);
    write!(f, "{} ", l.ty())?;
    fmt_operand_ref(f, l, Some(slots))?;
    f.write_str(", ")?;
    let r_id = d.rhs.get();
    let r_data = module.context().value_data(r_id);
    let r = Value::from_parts(r_id, module, r_data.ty);
    write!(f, "{} ", r.ty())?;
    fmt_operand_ref(f, r, Some(slots))?;
    print_shuffle_mask(f, inst.ty(), &d.mask)
}

fn print_shuffle_mask<B: ModuleBrand>(
    f: &mut fmt::Formatter<'_>,
    result_ty: Type<'_, B>,
    mask: &[i32],
) -> fmt::Result {
    // Mirrors `printShuffleMask` in `lib/IR/AsmWriter.cpp`.
    f.write_str(", <")?;
    if matches!(result_ty.data(), TypeData::ScalableVector { .. }) {
        f.write_str("vscale x ")?;
    }
    write!(f, "{} x i32> ", mask.len())?;
    let all_zero = !mask.is_empty() && mask.iter().all(|&e| e == 0);
    let all_poison = !mask.is_empty()
        && mask
            .iter()
            .all(|&e| e == crate::instr_types::POISON_MASK_ELEM);
    if all_zero {
        f.write_str("zeroinitializer")?;
    } else if all_poison {
        f.write_str("poison")?;
    } else {
        f.write_str("<")?;
        for (i, &e) in mask.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str("i32 ")?;
            if e == crate::instr_types::POISON_MASK_ELEM {
                f.write_str("poison")?;
            } else {
                write!(f, "{e}")?;
            }
        }
        f.write_str(">")?;
    }
    Ok(())
}

fn write_atomic_suffix(
    f: &mut fmt::Formatter<'_>,
    ordering: crate::atomic_ordering::AtomicOrdering,
    sync_scope: &crate::sync_scope::SyncScope,
) -> fmt::Result {
    // Mirrors `AssemblyWriter::writeAtomic` in `lib/IR/AsmWriter.cpp`:
    //   ` syncscope("...") <ordering>` (system scope omits the qualifier).
    if matches!(ordering, crate::atomic_ordering::AtomicOrdering::NotAtomic) {
        return Ok(());
    }
    if !sync_scope.is_default() {
        write!(f, " {sync_scope}")?;
    }
    write!(f, " {ordering}")
}

fn fmt_fence(f: &mut fmt::Formatter<'_>, d: &crate::instr_types::FenceInstData) -> fmt::Result {
    // `fence [syncscope("...")] <ordering>`
    f.write_str("fence")?;
    write_atomic_suffix(f, d.ordering, &d.sync_scope)
}

fn fmt_cmpxchg(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::AtomicCmpXchgInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `cmpxchg [weak] [volatile] <ptr-ty> <ptr>, <cmp-ty> <cmp>, <new-ty> <new>
    //          [syncscope("...")] <success-ord> <failure-ord>, align N`
    f.write_str("cmpxchg")?;
    if d.weak {
        f.write_str(" weak")?;
    }
    if d.volatile {
        f.write_str(" volatile")?;
    }
    let module = inst.module();
    let ptr_id = d.ptr.get();
    let ptr_data = module.context().value_data(ptr_id);
    let ptr = Value::from_parts(ptr_id, module, ptr_data.ty);
    write!(f, " {} ", ptr.ty())?;
    fmt_operand_ref(f, ptr, Some(slots))?;
    f.write_str(", ")?;
    let cmp_id = d.cmp.get();
    let cmp_data = module.context().value_data(cmp_id);
    let cmp = Value::from_parts(cmp_id, module, cmp_data.ty);
    write!(f, "{} ", cmp.ty())?;
    fmt_operand_ref(f, cmp, Some(slots))?;
    f.write_str(", ")?;
    let new_id = d.new_val.get();
    let new_data = module.context().value_data(new_id);
    let new_v = Value::from_parts(new_id, module, new_data.ty);
    write!(f, "{} ", new_v.ty())?;
    fmt_operand_ref(f, new_v, Some(slots))?;
    if !d.sync_scope.is_default() {
        write!(f, " {}", d.sync_scope)?;
    }
    write!(f, " {} {}", d.success_ordering, d.failure_ordering)?;
    if let Some(a) = d.align.align() {
        write!(f, ", align {}", a.value())?;
    }
    Ok(())
}

fn fmt_atomicrmw(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::AtomicRMWInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `atomicrmw [volatile] <op> <ptr-ty> <ptr>, <val-ty> <val>
    //           [syncscope("...")] <ordering>, align N`
    f.write_str("atomicrmw")?;
    if d.volatile {
        f.write_str(" volatile")?;
    }
    write!(f, " {} ", d.op)?;
    let module = inst.module();
    let ptr_id = d.ptr.get();
    let ptr_data = module.context().value_data(ptr_id);
    let ptr = Value::from_parts(ptr_id, module, ptr_data.ty);
    write!(f, "{} ", ptr.ty())?;
    fmt_operand_ref(f, ptr, Some(slots))?;
    f.write_str(", ")?;
    let val_id = d.value.get();
    let val_data = module.context().value_data(val_id);
    let val = Value::from_parts(val_id, module, val_data.ty);
    write!(f, "{} ", val.ty())?;
    fmt_operand_ref(f, val, Some(slots))?;
    write_atomic_suffix(f, d.ordering, &d.sync_scope)?;
    if let Some(a) = d.align.align() {
        write!(f, ", align {}", a.value())?;
    }
    Ok(())
}

fn fmt_ret(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    r: &ReturnOpData,
    slots: &SlotTracker,
) -> fmt::Result {
    match r.value.get() {
        None => f.write_str("ret void"),
        Some(id) => {
            let module = inst.module();
            let data = module.context().value_data(id);
            let v = Value::from_parts(id, module, data.ty);
            f.write_str("ret ")?;
            fmt_operand(f, v, Some(slots))
        }
    }
}

fn fmt_icmp(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    c: &CmpInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    let lhs_data = module.context().value_data(c.lhs.get());
    let lhs = Value::from_parts(c.lhs.get(), module, lhs_data.ty);
    if c.samesign {
        write!(f, "icmp samesign {} {} ", c.predicate.name(), lhs.ty())?;
    } else {
        write!(f, "icmp {} {} ", c.predicate.name(), lhs.ty())?;
    }
    fmt_operand_ref(f, lhs, Some(slots))?;
    f.write_str(", ")?;
    let rhs_data = module.context().value_data(c.rhs.get());
    let rhs = Value::from_parts(c.rhs.get(), module, rhs_data.ty);
    fmt_operand_ref(f, rhs, Some(slots))
}
fn fmt_fcmp(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    c: &crate::instr_types::FCmpInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `fcmp [<fmf>] <pred> <ty> <lhs>, <rhs>`. The optional FMF block
    // mirrors `writeOptimizationInfo` in `lib/IR/AsmWriter.cpp`.
    let module = inst.module();
    let lhs_data = module.context().value_data(c.lhs.get());
    let lhs = Value::from_parts(c.lhs.get(), module, lhs_data.ty);
    f.write_str("fcmp")?;
    if !c.fmf.is_empty() {
        write!(f, " {}", c.fmf)?;
    }
    write!(f, " {} {} ", c.predicate.name(), lhs.ty())?;
    fmt_operand_ref(f, lhs, Some(slots))?;
    f.write_str(", ")?;
    let rhs_data = module.context().value_data(c.rhs.get());
    let rhs = Value::from_parts(c.rhs.get(), module, rhs_data.ty);
    fmt_operand_ref(f, rhs, Some(slots))
}
fn fmt_alloca(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    a: &crate::instr_types::AllocaInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    let allocated = Type::new(a.allocated_ty, module);
    write!(f, "alloca {}", allocated)?;
    if let Some(num_id) = a.num_elements.get() {
        let nd = module.context().value_data(num_id);
        let nv = Value::from_parts(num_id, module, nd.ty);
        write!(f, ", {} ", nv.ty())?;
        fmt_operand_ref(f, nv, Some(slots))?;
    }
    if let Some(al) = a.align.align() {
        write!(f, ", align {}", al.value())?;
    }
    Ok(())
}

fn fmt_load(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    l: &crate::instr_types::LoadInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // Mirrors `AssemblyWriter::printInstruction` LoadInst arm in
    // `lib/IR/AsmWriter.cpp`: `load [atomic] [volatile] <ty>, <ptrty> <ptr>
    // [syncscope("...")] <ordering>, align N`.
    let module = inst.module();
    let pointee = Type::new(l.pointee_ty, module);
    f.write_str("load")?;
    if l.is_atomic() {
        f.write_str(" atomic")?;
    }
    if l.volatile {
        f.write_str(" volatile")?;
    }
    write!(f, " {}, ", pointee)?;
    let pd = module.context().value_data(l.ptr.get());
    let pv = Value::from_parts(l.ptr.get(), module, pd.ty);
    write!(f, "{} ", pv.ty())?;
    fmt_operand_ref(f, pv, Some(slots))?;
    write_atomic_suffix(f, l.ordering, &l.sync_scope)?;
    if let Some(al) = l.align.align() {
        write!(f, ", align {}", al.value())?;
    }
    Ok(())
}

fn fmt_store(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    s: &crate::instr_types::StoreInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // Mirrors `AssemblyWriter::printInstruction` StoreInst arm in
    // `lib/IR/AsmWriter.cpp`: `store [atomic] [volatile] <valty> <val>,
    // <ptrty> <ptr> [syncscope("...")] <ordering>, align N`.
    let module = inst.module();
    f.write_str("store")?;
    if s.is_atomic() {
        f.write_str(" atomic")?;
    }
    if s.volatile {
        f.write_str(" volatile")?;
    }
    f.write_str(" ")?;
    let vd = module.context().value_data(s.value.get());
    let vv = Value::from_parts(s.value.get(), module, vd.ty);
    write!(f, "{} ", vv.ty())?;
    fmt_operand_ref(f, vv, Some(slots))?;
    f.write_str(", ")?;
    let pd = module.context().value_data(s.ptr.get());
    let pv = Value::from_parts(s.ptr.get(), module, pd.ty);
    write!(f, "{} ", pv.ty())?;
    fmt_operand_ref(f, pv, Some(slots))?;
    write_atomic_suffix(f, s.ordering, &s.sync_scope)?;
    if let Some(al) = s.align.align() {
        write!(f, ", align {}", al.value())?;
    }
    Ok(())
}

fn fmt_gep(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    g: &crate::instr_types::GepInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    f.write_str("getelementptr ")?;
    let flags_str = format!("{}", g.flags);
    if !flags_str.is_empty() {
        write!(f, "{} ", flags_str)?;
    }
    let source = Type::new(g.source_ty, module);
    write!(f, "{}, ", source)?;
    let pd = module.context().value_data(g.ptr.get());
    let pv = Value::from_parts(g.ptr.get(), module, pd.ty);
    write!(f, "{} ", pv.ty())?;
    fmt_operand_ref(f, pv, Some(slots))?;
    for idx_cell in g.indices.iter() {
        let iid = idx_cell.get();
        let id_data = module.context().value_data(iid);
        let iv = Value::from_parts(iid, module, id_data.ty);
        f.write_str(", ")?;
        write!(f, "{} ", iv.ty())?;
        fmt_operand_ref(f, iv, Some(slots))?;
    }
    Ok(())
}

fn fmt_call(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    c: &crate::instr_types::CallInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    if let Some(kw) = c.tail_kind.keyword() {
        write!(f, "{} ", kw)?;
    }
    f.write_str("call ")?;
    if c.calling_conv != crate::CallingConv::C {
        write!(f, "{} ", c.calling_conv)?;
    }
    let module = inst.module();
    fmt_attribute_set(f, c.attrs.return_attrs(), AttrIndex::Return, false, module)?;
    if c.attrs.return_attrs().get(AttrIndex::Return).is_some() {
        f.write_str(" ")?;
    }
    // LLVM prints the callee function type for varargs call sites so the
    // fixed parameter prefix is preserved (`call i32 (ptr, ...) @printf(...)`).
    // Non-varargs direct calls keep the compact result-type spelling.
    if module
        .context()
        .type_data(c.fn_ty)
        .as_function()
        .is_some_and(|(_, _, is_var_arg)| is_var_arg)
    {
        write!(f, "{} ", Type::new(c.fn_ty, module))?;
    } else {
        write!(f, "{} ", inst.ty())?;
    }
    let cd = module.context().value_data(c.callee.get());
    // An inline-asm callee prints the `asm "...", "..."` form in place of
    // an `@name` / SSA operand. Mirrors `AssemblyWriter`'s `CallInst`
    // path, which routes an `InlineAsm` callee through `writeOperand`'s
    // `asm` printer rather than emitting a symbolic callee.
    match &cd.kind {
        ValueKindData::InlineAsm(d) => fmt_inline_asm(f, d)?,
        _ => {
            let callee = Value::from_parts(c.callee.get(), module, cd.ty);
            fmt_operand_ref(f, callee, Some(slots))?;
        }
    }
    f.write_str("(")?;
    let mut first = true;
    for (idx, arg_cell) in c.args.iter().enumerate() {
        let aid = arg_cell.get();
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let ad = module.context().value_data(aid);
        let av = Value::from_parts(aid, module, ad.ty);
        write!(f, "{} ", av.ty())?;
        if let Some(arg_attr) = c.attrs.arg_attrs().get(idx) {
            fmt_attribute_set(f, arg_attr, AttrIndex::Param(0), false, module)?;
            if arg_attr.get(AttrIndex::Param(0)).is_some() {
                f.write_str(" ")?;
            }
        }
        fmt_operand_ref(f, av, Some(slots))?;
    }
    f.write_str(")")?;
    fmt_attribute_set(
        f,
        c.attrs.function_attrs(),
        AttrIndex::Function,
        true,
        module,
    )?;
    for group in c.attrs.function_attr_groups_slice() {
        write!(f, " #{group}")?;
    }
    fmt_operand_bundles(f, c.attrs.operand_bundles_slice(), module.core_ref(), slots)
}

fn operand_bundle_tag_name(tag: &crate::instr_types::OperandBundleTag) -> &str {
    match tag {
        crate::instr_types::OperandBundleTag::Deopt => "deopt",
        crate::instr_types::OperandBundleTag::Funclet => "funclet",
        crate::instr_types::OperandBundleTag::GcTransition => "gc-transition",
        crate::instr_types::OperandBundleTag::CfGuardTarget => "cfguardtarget",
        crate::instr_types::OperandBundleTag::Preallocated => "preallocated",
        crate::instr_types::OperandBundleTag::GcLive => "gc-live",
        crate::instr_types::OperandBundleTag::ClangArcAttachedCall => "clang.arc.attachedcall",
        crate::instr_types::OperandBundleTag::PtrAuth => "ptrauth",
        crate::instr_types::OperandBundleTag::Kcfi => "kcfi",
        crate::instr_types::OperandBundleTag::ConvergenceCtrl => "convergencectrl",
        crate::instr_types::OperandBundleTag::Align => "align",
        crate::instr_types::OperandBundleTag::DeactivationSymbol => "deactivation",
        crate::instr_types::OperandBundleTag::Custom(name) => name.as_str(),
    }
}

fn fmt_operand_bundles(
    f: &mut fmt::Formatter<'_>,
    bundles: &[crate::instr_types::OperandBundleData],
    module: &crate::module::ModuleCore,
    slots: &SlotTracker,
) -> fmt::Result {
    if bundles.is_empty() {
        return Ok(());
    }
    f.write_str(" [")?;
    for (idx, bundle) in bundles.iter().enumerate() {
        if idx != 0 {
            f.write_str(", ")?;
        }
        f.write_str("\"")?;
        print_escaped_string(f, operand_bundle_tag_name(bundle.tag()).as_bytes())?;
        f.write_str("\"(")?;
        for (input_idx, id) in bundle.inputs().enumerate() {
            if input_idx != 0 {
                f.write_str(", ")?;
            }
            let data = module.context().value_data(id);
            let value = Value::from_parts(id, module, data.ty);
            fmt_operand(f, value, Some(slots))?;
        }
        f.write_str(")")?;
    }
    f.write_str("]")
}

fn fmt_landingpad(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::LandingPadInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // Mirrors `printInstruction`'s `LandingPadInst` arm:
    //   `landingpad <ty>`
    //   followed by `\n          cleanup` and `\n          catch <ty> <val>` /
    //   `\n          filter <ty> <val>` lines.
    f.write_str("landingpad ")?;
    write!(f, "{}", inst.ty())?;
    let cleanup = d.cleanup.get();
    let clauses = d.clauses.borrow();
    if cleanup || !clauses.is_empty() {
        f.write_str("\n")?;
    }
    if cleanup {
        f.write_str("          cleanup")?;
    }
    let module = inst.module();
    for (i, (kind, op_cell)) in clauses.iter().enumerate() {
        if i != 0 || cleanup {
            f.write_str("\n")?;
        }
        let kw = match kind {
            crate::instr_types::LandingPadClauseKind::Catch => "          catch ",
            crate::instr_types::LandingPadClauseKind::Filter => "          filter ",
        };
        f.write_str(kw)?;
        let op_id = op_cell.get();
        let op_data = module.context().value_data(op_id);
        let op_v = Value::from_parts(op_id, module, op_data.ty);
        write!(f, "{} ", op_v.ty())?;
        fmt_operand_ref(f, op_v, Some(slots))?;
    }
    Ok(())
}

fn fmt_resume(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::ResumeInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `resume <ty> <value>`
    f.write_str("resume ")?;
    let module = inst.module();
    let v_id = d.value.get();
    let v_data = module.context().value_data(v_id);
    let v = Value::from_parts(v_id, module, v_data.ty);
    write!(f, "{} ", v.ty())?;
    fmt_operand_ref(f, v, Some(slots))
}

fn fmt_funclet_pad(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    keyword: &str,
    parent_pad: &core::cell::Cell<Option<crate::value::ValueId>>,
    args: &[core::cell::Cell<crate::value::ValueId>],
    slots: &SlotTracker,
) -> fmt::Result {
    // `<keyword> within <parent> [<arg-ty> <arg>, ...]`
    f.write_str(keyword)?;
    f.write_str(" within ")?;
    let module = inst.module();
    match parent_pad.get() {
        None => f.write_str("none")?,
        Some(id) => {
            let pd = module.context().value_data(id);
            let pv = Value::from_parts(id, module, pd.ty);
            fmt_operand_ref(f, pv, Some(slots))?;
        }
    }
    f.write_str(" [")?;
    let mut first = true;
    for arg_cell in args.iter() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let aid = arg_cell.get();
        let ad = module.context().value_data(aid);
        let av = Value::from_parts(aid, module, ad.ty);
        write!(f, "{} ", av.ty())?;
        fmt_operand_ref(f, av, Some(slots))?;
    }
    f.write_str("]")
}

fn fmt_catchret(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::CatchReturnInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `catchret from <catchpad> to label <bb>`
    f.write_str("catchret from ")?;
    let module = inst.module();
    let cp_id = d.catch_pad.get();
    let cp_data = module.context().value_data(cp_id);
    let cp = Value::from_parts(cp_id, module, cp_data.ty);
    fmt_operand_ref(f, cp, Some(slots))?;
    f.write_str(" to ")?;
    let bb_data = module.context().value_data(d.target_bb);
    let bb = Value::from_parts(d.target_bb, module, bb_data.ty);
    write!(f, "{} ", bb.ty())?;
    fmt_operand_ref(f, bb, Some(slots))
}

fn fmt_cleanupret(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::CleanupReturnInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `cleanupret from <cleanuppad> unwind [to caller | label <bb>]`
    f.write_str("cleanupret from ")?;
    let module = inst.module();
    let cp_id = d.cleanup_pad.get();
    let cp_data = module.context().value_data(cp_id);
    let cp = Value::from_parts(cp_id, module, cp_data.ty);
    fmt_operand_ref(f, cp, Some(slots))?;
    f.write_str(" unwind ")?;
    match d.unwind_dest {
        None => f.write_str("to caller"),
        Some(bb_id) => {
            let bb_data = module.context().value_data(bb_id);
            let bb = Value::from_parts(bb_id, module, bb_data.ty);
            write!(f, "{} ", bb.ty())?;
            fmt_operand_ref(f, bb, Some(slots))
        }
    }
}

fn fmt_catchswitch(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::CatchSwitchInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `catchswitch within <parent> [label <h1>, label <h2>, ...] unwind [to caller | label <bb>]`
    f.write_str("catchswitch within ")?;
    let module = inst.module();
    match d.parent_pad.get() {
        None => f.write_str("none")?,
        Some(id) => {
            let pd = module.context().value_data(id);
            let pv = Value::from_parts(id, module, pd.ty);
            fmt_operand_ref(f, pv, Some(slots))?;
        }
    }
    f.write_str(" [")?;
    let handlers = d.handlers.borrow();
    let mut first = true;
    for &h_id in handlers.iter() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let hd = module.context().value_data(h_id);
        let hv = Value::from_parts(h_id, module, hd.ty);
        write!(f, "{} ", hv.ty())?;
        fmt_operand_ref(f, hv, Some(slots))?;
    }
    f.write_str("] unwind ")?;
    match d.unwind_dest.get() {
        None => f.write_str("to caller"),
        Some(bb_id) => {
            let bb_data = module.context().value_data(bb_id);
            let bb = Value::from_parts(bb_id, module, bb_data.ty);
            write!(f, "{} ", bb.ty())?;
            fmt_operand_ref(f, bb, Some(slots))
        }
    }
}

fn fmt_invoke(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::InvokeInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `invoke [<cc>] <ret-ty> <callee>(<args>)\n          to label %normal unwind label %unwind`
    f.write_str("invoke ")?;
    if d.calling_conv != crate::CallingConv::C {
        write!(f, "{} ", d.calling_conv)?;
    }
    let module = inst.module();
    fmt_attribute_set(f, d.attrs.return_attrs(), AttrIndex::Return, false, module)?;
    if d.attrs.return_attrs().get(AttrIndex::Return).is_some() {
        f.write_str(" ")?;
    }
    write!(f, "{} ", inst.ty())?;
    let callee_data = module.context().value_data(d.callee.get());
    match &callee_data.kind {
        ValueKindData::InlineAsm(data) => fmt_inline_asm(f, data)?,
        _ => {
            let callee = Value::from_parts(d.callee.get(), module, callee_data.ty);
            fmt_operand_ref(f, callee, Some(slots))?;
        }
    }
    f.write_str("(")?;
    let mut first = true;
    for (idx, arg_cell) in d.args.iter().enumerate() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let aid = arg_cell.get();
        let ad = module.context().value_data(aid);
        let av = Value::from_parts(aid, module, ad.ty);
        write!(f, "{} ", av.ty())?;
        if let Some(arg_attr) = d.attrs.arg_attrs().get(idx) {
            fmt_attribute_set(f, arg_attr, AttrIndex::Param(0), false, module)?;
            if arg_attr.get(AttrIndex::Param(0)).is_some() {
                f.write_str(" ")?;
            }
        }
        fmt_operand_ref(f, av, Some(slots))?;
    }
    f.write_str(")")?;
    fmt_attribute_set(
        f,
        d.attrs.function_attrs(),
        AttrIndex::Function,
        true,
        module,
    )?;
    for group in d.attrs.function_attr_groups_slice() {
        write!(f, " #{group}")?;
    }
    fmt_operand_bundles(f, d.attrs.operand_bundles_slice(), module.core_ref(), slots)?;
    f.write_str("\n          to ")?;
    let nd = module.context().value_data(d.normal_dest.get());
    let nbb = Value::from_parts(d.normal_dest.get(), module, nd.ty);
    write!(f, "{} ", nbb.ty())?;
    fmt_operand_ref(f, nbb, Some(slots))?;
    f.write_str(" unwind ")?;
    let ud = module.context().value_data(d.unwind_dest.get());
    let ubb = Value::from_parts(d.unwind_dest.get(), module, ud.ty);
    write!(f, "{} ", ubb.ty())?;
    fmt_operand_ref(f, ubb, Some(slots))
}

fn fmt_callbr(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::CallBrInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `callbr [<cc>] <ret-ty> <callee>(<args>)\n          to label %default [label %indirect1, ...]`
    f.write_str("callbr ")?;
    if d.calling_conv != crate::CallingConv::C {
        write!(f, "{} ", d.calling_conv)?;
    }
    let module = inst.module();
    fmt_attribute_set(f, d.attrs.return_attrs(), AttrIndex::Return, false, module)?;
    if d.attrs.return_attrs().get(AttrIndex::Return).is_some() {
        f.write_str(" ")?;
    }
    write!(f, "{} ", inst.ty())?;
    let callee_data = module.context().value_data(d.callee.get());
    match &callee_data.kind {
        ValueKindData::InlineAsm(data) => fmt_inline_asm(f, data)?,
        _ => {
            let callee = Value::from_parts(d.callee.get(), module, callee_data.ty);
            fmt_operand_ref(f, callee, Some(slots))?;
        }
    }
    f.write_str("(")?;
    let mut first = true;
    for (idx, arg_cell) in d.args.iter().enumerate() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let aid = arg_cell.get();
        let ad = module.context().value_data(aid);
        let av = Value::from_parts(aid, module, ad.ty);
        write!(f, "{} ", av.ty())?;
        if let Some(arg_attr) = d.attrs.arg_attrs().get(idx) {
            fmt_attribute_set(f, arg_attr, AttrIndex::Param(0), false, module)?;
            if arg_attr.get(AttrIndex::Param(0)).is_some() {
                f.write_str(" ")?;
            }
        }
        fmt_operand_ref(f, av, Some(slots))?;
    }
    f.write_str(")")?;
    fmt_attribute_set(
        f,
        d.attrs.function_attrs(),
        AttrIndex::Function,
        true,
        module,
    )?;
    for group in d.attrs.function_attr_groups_slice() {
        write!(f, " #{group}")?;
    }
    fmt_operand_bundles(f, d.attrs.operand_bundles_slice(), module.core_ref(), slots)?;
    f.write_str("\n          to ")?;
    let dd = module.context().value_data(d.default_dest.get());
    let dbb = Value::from_parts(d.default_dest.get(), module, dd.ty);
    write!(f, "{} ", dbb.ty())?;
    fmt_operand_ref(f, dbb, Some(slots))?;
    f.write_str(" [")?;
    let mut first = true;
    for ic in d.indirect_dests.iter() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let ind_id = ic.get();
        let ind_data = module.context().value_data(ind_id);
        let ibb = Value::from_parts(ind_id, module, ind_data.ty);
        write!(f, "{} ", ibb.ty())?;
        fmt_operand_ref(f, ibb, Some(slots))?;
    }
    f.write_str("]")
}

fn fmt_phi(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    p: &PhiData,
    slots: &SlotTracker,
) -> fmt::Result {
    write!(f, "phi {} ", inst.ty())?;
    let module = inst.module();
    let mut first = true;
    for (vid_cell, bid) in p.incoming.borrow().iter() {
        let vid = vid_cell.get();
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let v_data = module.context().value_data(vid);
        let v = Value::from_parts(vid, module, v_data.ty);
        f.write_str("[ ")?;
        fmt_operand_ref(f, v, Some(slots))?;
        f.write_str(", ")?;
        let b_data = module.context().value_data(*bid);
        let b = Value::from_parts(*bid, module, b_data.ty);
        fmt_operand_ref(f, b, Some(slots))?;
        f.write_str(" ]")?;
    }
    Ok(())
}

fn fmt_switch(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::SwitchInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // Mirrors the `SwitchInst` arm of `printInstruction`
    // (`lib/IR/AsmWriter.cpp`):
    //   `switch <cond-ty> <cond>, label <default> [\n    <case-ty> <val>, label <bb> ...\n  ]`
    f.write_str("switch ")?;
    let module = inst.module();
    let cond_id = d.cond.get();
    let cond_data = module.context().value_data(cond_id);
    let cond = Value::from_parts(cond_id, module, cond_data.ty);
    write!(f, "{} ", cond.ty())?;
    fmt_operand_ref(f, cond, Some(slots))?;
    f.write_str(", ")?;
    let default_id = d.default_bb.get();
    let default_data = module.context().value_data(default_id);
    let default = Value::from_parts(default_id, module, default_data.ty);
    write!(f, "{} ", default.ty())?;
    fmt_operand_ref(f, default, Some(slots))?;
    f.write_str(" [")?;
    for (case_v, case_bb) in d.cases.borrow().iter() {
        f.write_str("\n    ")?;
        let v_id = case_v.get();
        let v_data = module.context().value_data(v_id);
        let v = Value::from_parts(v_id, module, v_data.ty);
        write!(f, "{} ", v.ty())?;
        fmt_operand_ref(f, v, Some(slots))?;
        f.write_str(", ")?;
        let bb_data = module.context().value_data(*case_bb);
        let bb_v = Value::from_parts(*case_bb, module, bb_data.ty);
        write!(f, "{} ", bb_v.ty())?;
        fmt_operand_ref(f, bb_v, Some(slots))?;
    }
    f.write_str("\n  ]")
}

fn fmt_indirectbr(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    d: &crate::instr_types::IndirectBrInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `indirectbr <addr-ty> <addr>, [label <bb1>, label <bb2>, ...]`
    f.write_str("indirectbr ")?;
    let module = inst.module();
    let addr_id = d.addr.get();
    let addr_data = module.context().value_data(addr_id);
    let addr = Value::from_parts(addr_id, module, addr_data.ty);
    write!(f, "{} ", addr.ty())?;
    fmt_operand_ref(f, addr, Some(slots))?;
    f.write_str(", [")?;
    let dests = d.destinations.borrow();
    for (i, &bb_id) in dests.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        let bb_data = module.context().value_data(bb_id);
        let bb_v = Value::from_parts(bb_id, module, bb_data.ty);
        write!(f, "{} ", bb_v.ty())?;
        fmt_operand_ref(f, bb_v, Some(slots))?;
    }
    f.write_str("]")
}

fn fmt_br(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    b: &BranchInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    match &b.kind {
        BranchKind::Unconditional(target) => {
            let data = module.context().value_data(*target);
            let v = Value::from_parts(*target, module, data.ty);
            f.write_str("br label ")?;
            fmt_operand_ref(f, v, Some(slots))
        }
        BranchKind::Conditional {
            cond,
            then_bb,
            else_bb,
        } => {
            let cid = cond.get();
            let c_data = module.context().value_data(cid);
            let cv = Value::from_parts(cid, module, c_data.ty);
            f.write_str("br ")?;
            fmt_operand(f, cv, Some(slots))?;
            f.write_str(", label ")?;
            let t_data = module.context().value_data(*then_bb);
            let t = Value::from_parts(*then_bb, module, t_data.ty);
            fmt_operand_ref(f, t, Some(slots))?;
            f.write_str(", label ")?;
            let e_data = module.context().value_data(*else_bb);
            let e = Value::from_parts(*else_bb, module, e_data.ty);
            fmt_operand_ref(f, e, Some(slots))
        }
    }
}

// --------------------------------------------------------------------------
// Attribute printing helpers
// --------------------------------------------------------------------------

/// Render every attribute in `set` as a space-separated list, in
/// insertion order. Returns `Ok(true)` if at least one attribute was
/// printed (so callers can decide whether to add a separator before
/// the next token), `Ok(false)` if the set was empty.
fn fmt_attribute_set<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    storage: &AttributeStorage,
    index: AttrIndex,
    leading_space: bool,
    module: ModuleView<'ctx, B>,
) -> fmt::Result {
    let Some(stored) = storage.get(index) else {
        return Ok(());
    };
    let mut first = !leading_space;
    for attr in stored {
        if first {
            first = false;
        } else {
            f.write_str(" ")?;
        }
        fmt_attribute_stored(f, attr, module)?;
    }
    Ok(())
}

fn fmt_attribute_stored<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    attr: &AttributeStored,
    module: ModuleView<'ctx, B>,
) -> fmt::Result {
    match attr {
        AttributeStored::Enum(k) => f.write_str(k.name()),
        AttributeStored::Int(k, v) => write!(f, "{}({v})", k.name()),
        AttributeStored::Type(k, ty_id) => write!(f, "{}({})", k.name(), Type::new(*ty_id, module)),
        AttributeStored::Range { ty, lower, upper } => write!(
            f,
            "range({} {}, {})",
            Type::new(*ty, module),
            lower.to_string_radix(10, ApIntSignedness::Signed),
            upper.to_string_radix(10, ApIntSignedness::Signed)
        ),
        AttributeStored::String { key, value } if value.is_empty() => write!(f, "\"{key}\""),
        AttributeStored::String { key, value } => write!(f, "\"{key}\"=\"{value}\""),
    }
}

// --------------------------------------------------------------------------
// BasicBlock + Function + Module printing
// --------------------------------------------------------------------------

fn fmt_debug_metadata_operand(
    f: &mut fmt::Formatter<'_>,
    operand: crate::metadata::DebugMetadataOperand,
    module: &ModuleCore,
    store: &crate::metadata::MetadataStore,
    md_slots: &[Option<usize>],
    slots: &SlotTracker,
) -> fmt::Result {
    match operand {
        crate::metadata::DebugMetadataOperand::Metadata(md) => {
            fmt_metadata_operand(f, md.0, module, store, md_slots)
        }
        crate::metadata::DebugMetadataOperand::Value(id) => {
            let data = module.context().value_data(id);
            fmt_operand(f, Value::from_parts(id, module, data.ty), Some(slots))
        }
    }
}

fn fmt_debug_record(
    f: &mut fmt::Formatter<'_>,
    record: &crate::metadata::DebugRecord,
    module: &ModuleCore,
    store: &crate::metadata::MetadataStore,
    md_slots: &[Option<usize>],
    slots: &SlotTracker,
) -> fmt::Result {
    f.write_str("  ")?;
    match record {
        crate::metadata::DebugRecord::Variable(record) => {
            write!(f, "#dbg_{}(", record.kind().name())?;
            fmt_debug_metadata_operand(f, record.location(), module, store, md_slots, slots)?;
            f.write_str(", ")?;
            fmt_metadata_operand(f, record.variable(), module, store, md_slots)?;
            f.write_str(", ")?;
            fmt_metadata_operand(f, record.expression(), module, store, md_slots)?;
            f.write_str(", ")?;
            if let Some(assign_id) = record.assign_id() {
                fmt_metadata_operand(f, assign_id, module, store, md_slots)?;
                f.write_str(", ")?;
            }
            if let Some(address_location) = record.address_location() {
                fmt_debug_metadata_operand(f, address_location, module, store, md_slots, slots)?;
                f.write_str(", ")?;
            }
            if let Some(address_expression) = record.address_expression() {
                fmt_metadata_operand(f, address_expression, module, store, md_slots)?;
                f.write_str(", ")?;
            }
            fmt_metadata_operand(f, record.debug_loc(), module, store, md_slots)?;
            f.write_str(")")
        }
        crate::metadata::DebugRecord::Label { label, debug_loc } => {
            f.write_str("#dbg_label(")?;
            fmt_metadata_operand(f, *label, module, store, md_slots)?;
            f.write_str(", ")?;
            fmt_metadata_operand(f, *debug_loc, module, store, md_slots)?;
            f.write_str(")")
        }
    }
}

pub(super) fn fmt_basic_block<S: BlockSealState>(
    f: &mut fmt::Formatter<'_>,
    bb: BasicBlock<'_, Dyn, S, impl ModuleBrand>,
    slots: &SlotTracker,
    is_first: bool,
) -> fmt::Result {
    if !is_first {
        f.write_str("\n")?;
    }
    if let Some(name) = bb.name() {
        fmt_llvm_name_without_prefix(f, &name)?;
        f.write_str(":")?;
    } else if let Some(slot) = slots.block(bb.as_value().id) {
        write!(f, "{slot}:")?;
    } else {
        f.write_str("<unnamed>:")?;
    }
    f.write_str("\n")?;
    let module_view = bb.module();
    let md = module_view.metadata_store();
    let md_slots = metadata_slot_map(md.nodes());
    for inst in bb.instructions() {
        for record in inst.debug_records().iter() {
            fmt_debug_record(f, record, module_view.core_ref(), &md, &md_slots, slots)?;
            f.write_str("\n")?;
        }
        fmt_instruction(f, &inst, slots)?;
        f.write_str("\n")?;
    }
    Ok(())
}

pub(super) fn fmt_function<B: ModuleBrand>(
    f: &mut fmt::Formatter<'_>,
    func: FunctionValue<'_, Dyn, B>,
) -> fmt::Result {
    let slots = SlotTracker::for_function(func);
    let sig = func.signature();
    let linkage = func.linkage();
    let attrs = func.data().attributes.borrow();
    let header = if func.basic_blocks().count() == 0 {
        "declare"
    } else {
        "define"
    };
    write!(f, "{header}")?;
    // Print non-default linkage between header and return type.
    let linkage_str = linkage.keyword();
    if !linkage_str.is_empty() {
        write!(f, " {linkage_str}")?;
    }
    if let Some(s) = func.visibility().keyword() {
        write!(f, " {s}")?;
    }
    if let Some(s) = func.dll_storage_class().keyword() {
        write!(f, " {s}")?;
    }
    if let Some(s) = func.dso_locality().keyword() {
        write!(f, " {s}")?;
    }
    if func.calling_conv() != crate::CallingConv::C {
        write!(f, " {}", func.calling_conv())?;
    }
    // Return-attribute slot: prints between `define` (or `declare`)
    // and the return type. Mirrors `define noundef i32 @main()`.
    f.write_str(" ")?;
    fmt_attribute_set(f, &attrs, AttrIndex::Return, false, func.module())?;
    if attrs.get(AttrIndex::Return).is_some() {
        f.write_str(" ")?;
    }
    write!(f, "{} ", sig.return_type())?;
    fmt_global_value_ref(f, func.as_value())?;
    f.write_str("(")?;
    let mut first = true;
    for arg in func.params() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        write!(f, "{}", arg.ty())?;
        // Per-parameter attribute slot.
        if attrs.get(AttrIndex::Param(arg.slot())).is_some() {
            f.write_str(" ")?;
            fmt_attribute_set(
                f,
                &attrs,
                AttrIndex::Param(arg.slot()),
                false,
                func.module(),
            )?;
        }
        f.write_str(" ")?;
        match arg.name() {
            Some(n) => fmt_llvm_name(f, "%", &n)?,
            None => match slots.local(arg.as_value().id) {
                Some(slot) => write!(f, "%{slot}")?,
                None => f.write_str("%<unnumbered>")?,
            },
        }
    }
    if sig.is_var_arg() {
        if !first {
            f.write_str(", ")?;
        }
        f.write_str("...")?;
    }
    f.write_str(")")?;
    if let Some(kw) = func.unnamed_addr().keyword() {
        write!(f, " {kw}")?;
    }
    if func.address_space() != 0 {
        write!(f, " addrspace({})", func.address_space())?;
    }
    for group in func.function_attr_groups() {
        write!(f, " #{group}")?;
    }
    fmt_attribute_set(f, &attrs, AttrIndex::Function, true, func.module())?;
    if let Some(section) = func.section() {
        f.write_str(" section \"")?;
        print_escaped_string(f, section.as_bytes())?;
        f.write_str("\"")?;
    }
    if let Some(partition) = func.partition() {
        f.write_str(" partition \"")?;
        print_escaped_string(f, partition.as_bytes())?;
        f.write_str("\"")?;
    }
    if let Some(c) = func.comdat() {
        f.write_str(" comdat")?;
        if c.name() != func.name() {
            f.write_str("(")?;
            fmt_llvm_name(f, "$", c.name())?;
            f.write_str(")")?;
        }
    }
    if let Some(a) = func.align().align() {
        write!(f, " align {}", a.value())?;
    }
    if let Some(gc) = func.gc() {
        f.write_str(" gc \"")?;
        print_escaped_string(f, gc.as_bytes())?;
        f.write_str("\"")?;
    }
    if let Some(prefix) = func.prefix_data() {
        f.write_str(" prefix ")?;
        fmt_operand(f, prefix.as_value(), None)?;
    }
    if let Some(prologue) = func.prologue_data() {
        f.write_str(" prologue ")?;
        fmt_operand(f, prologue.as_value(), None)?;
    }
    if let Some(personality) = func.personality_fn() {
        f.write_str(" personality ")?;
        fmt_operand(f, personality.as_value(), None)?;
    }
    {
        let module_view = func.module();
        let md = module_view.metadata_store();
        let md_slots = metadata_slot_map(md.nodes());
        fmt_metadata_attachments(f, &func.metadata(), module_view.core_ref(), &md, &md_slots)?;
    }
    if header == "declare" {
        return f.write_str("\n");
    }
    f.write_str(" {\n")?;
    let mut first_block = true;
    for bb in func.basic_blocks() {
        fmt_basic_block(f, bb, &slots, first_block)?;
        first_block = false;
    }
    for directive in func.use_list_orders() {
        f.write_str("  ")?;
        fmt_use_list_order(f, func.module().core_ref(), &directive, Some(&slots))?;
        f.write_str("\n")?;
    }
    f.write_str("}\n")
}

/// Print a struct body inline: `{ <elem>, ... }` (or `<{ ... }>` when
/// packed). Mirrors the literal-struct arm of `Type`'s `Display`
/// (`type.rs`), recursing into elements via `Type::new` — which renders a
/// *named* element as `%Name` and any other type structurally.
fn fmt_struct_body(f: &mut fmt::Formatter<'_>, body: &StructBody, m: &ModuleCore) -> fmt::Result {
    if body.packed {
        f.write_str("<{ ")?;
    } else {
        f.write_str("{ ")?;
    }
    let mut first = true;
    for e in body.elements.iter() {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        write!(f, "{}", Type::new(*e, m))?;
    }
    if body.packed {
        f.write_str(" }>")
    } else {
        f.write_str(" }")
    }
}

pub(super) fn fmt_module(f: &mut fmt::Formatter<'_>, m: &ModuleCore) -> fmt::Result {
    writeln!(f, "; ModuleID = '{}'", m.name())?;
    if let Some(source_filename) = m.source_filename() {
        f.write_str("source_filename = \"")?;
        print_escaped_string(f, source_filename.as_bytes())?;
        f.write_str("\"\n")?;
    }

    // `target datalayout = "..."`. Mirrors
    // `AssemblyWriter::printModule`'s `M->getDataLayout()` arm. Only
    // emitted when the directive is non-empty (matches upstream's
    // `if (!DL.empty())` guard).
    {
        let dl = m.data_layout();
        if !dl.is_default() {
            writeln!(f, "target datalayout = \"{}\"", dl)?;
        }
    }

    // `target triple = "..."`.
    if let Some(triple) = m.target_triple() {
        writeln!(f, "target triple = \"{triple}\"")?;
    }

    // Module-level inline assembly: one `module asm "<line>"` per
    // newline-split entry. Mirrors the `do { ... } while (!Asm.empty())`
    // loop in `printModule`.
    {
        let asm = m.module_asm();
        if !asm.is_empty() {
            for line in asm.split('\n') {
                if line.is_empty() {
                    continue;
                }
                f.write_str("module asm \"")?;
                print_escaped_string(f, line.as_bytes())?;
                f.write_str("\"\n")?;
            }
        }
    }

    // Comdats. Mirrors `AssemblyWriter::printModuleSummaryIndex`'s
    // comdat-emission loop in `lib/IR/AsmWriter.cpp` (the bare-module
    // path: a leading blank line if any comdats exist, then one line
    // per comdat).
    let mut comdats_iter = m.iter_comdats();
    if comdats_iter.len() > 0 {
        f.write_str("\n")?;
        for c in comdats_iter.by_ref() {
            fmt_comdat(f, c)?;
        }
    }

    // Named-struct type identities. Mirrors the `printTypeIdentities`
    // call in `AssemblyWriter::printModule`, emitted between comdats and
    // globals: a leading blank line if any exist, then one
    // `%Name = type {...}` (or `%Name = type opaque`) line per struct in
    // declaration order.
    let struct_ids = m.iter_named_struct_ids();
    let has_named_structs = !struct_ids.is_empty();
    if has_named_structs {
        f.write_str("\n")?;
        for id in struct_ids {
            let data = m.context().type_data(id);
            let s = data
                .as_struct()
                .expect("iter_named_struct_ids yields only struct ids");
            let name = s.name.as_ref().expect("named struct must have a name");
            fmt_llvm_name(f, "%", name)?;
            f.write_str(" = type ")?;
            match s.body.borrow().as_ref() {
                Some(body) => fmt_struct_body(f, body, m)?,
                // KirpiIR never creates opaque structs, but be faithful
                // to LLVM, which prints those as `%Name = type opaque`.
                None => f.write_str("opaque")?,
            }
            f.write_str("\n")?;
        }
    }

    // Globals. Mirrors the `for (const GlobalVariable &GV :
    // M->globals())` loop in `printModule`.
    if !m.global_empty() {
        f.write_str("\n")?;
        for g in m.iter_globals::<crate::module::Brand<'_>>() {
            fmt_global(f, g)?;
            f.write_str("\n")?;
        }
    }

    if !m.alias_empty() {
        f.write_str("\n")?;
        for a in m.iter_aliases::<crate::module::Brand<'_>>() {
            fmt_alias(f, a)?;
        }
    }

    if !m.ifunc_empty() {
        f.write_str("\n")?;
        for i in m.iter_ifuncs::<crate::module::Brand<'_>>() {
            fmt_ifunc(f, i)?;
        }
    }

    let mut first = true;
    for func in m.iter_functions::<crate::module::Brand<'_>>() {
        if !first || !m.global_empty() || !m.alias_empty() || !m.ifunc_empty() || has_named_structs
        {
            f.write_str("\n")?;
        }
        first = false;
        fmt_function(f, func)?;
    }
    {
        let mut groups = m.attribute_groups();
        groups.sort_by_key(|(slot, _)| *slot);
        if !groups.is_empty() {
            f.write_str("\n")?;
            for (slot, storage) in groups {
                write!(f, "attributes #{slot} = {{")?;
                if let Some(attrs) = storage.get(AttrIndex::Function)
                    && !attrs.is_empty()
                {
                    f.write_str(" ")?;
                    for (i, attr) in attrs.iter().enumerate() {
                        if i != 0 {
                            f.write_str(" ")?;
                        }
                        write!(f, "{attr}")?;
                    }
                    f.write_str(" ")?;
                }
                f.write_str("}\n")?;
            }
        }
    }

    for directive in m.iter_use_list_orders() {
        fmt_use_list_order(f, m, &directive, None)?;
        f.write_str("\n")?;
    }
    for directive in m.iter_use_list_order_bbs() {
        fmt_use_list_order_bb(f, m, &directive)?;
        f.write_str("\n")?;
    }

    // Numbered metadata nodes. Mirrors the
    // `for (const auto &[Slot, Node] : ...NumberedMetadata())`
    // loop in `printModule`. MDStrings are not numbered; they print inline
    // when referenced from MDNodes or MetadataAsValue operands.
    {
        let md = m.metadata_store();
        let nodes = md.nodes();
        let slots = metadata_slot_map(nodes);
        if slots.iter().any(Option::is_some) {
            f.write_str("\n")?;
            for (i, node) in nodes.iter().enumerate() {
                if let Some(slot) = slots[i] {
                    write!(f, "!{slot} = ")?;
                    fmt_metadata_node(f, node, m, &md, &slots)?;
                    f.write_str("\n")?;
                }
            }
        }
    }

    // Named metadata. Mirrors the `for (const NamedMDNode &NMD :
    // M->named_metadata())` loop in `printModule`.
    {
        let nmd = m.named_metadata_list();
        if !nmd.is_empty() {
            let md = m.metadata_store();
            let slots = metadata_slot_map(md.nodes());
            for node in nmd.iter() {
                write!(f, "!{} = !{{", node.name())?;
                for (j, op) in node.operands().iter().enumerate() {
                    if j > 0 {
                        f.write_str(", ")?;
                    }
                    fmt_metadata_operand(f, op.0, m, &md, &slots)?;
                }
                f.write_str("}\n")?;
            }
        }
    }

    Ok(())
}

/// Print an `MDString` body: `!"..."`. Shared by the standalone-node and
/// inline-operand paths.
fn fmt_md_string(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    f.write_str("!\"")?;
    print_escaped_string(f, s.as_bytes())?;
    f.write_str("\"")
}

/// Print one metadata node body. Mirrors `WriteMDNodeBodyInternal` in
/// `lib/IR/AsmWriter.cpp`.
///
/// Tuple MDString operands are printed *inline* (`!{!"rsp"}`) because LLVM
/// never assigns standalone metadata slots to `MDString`s.
fn fmt_metadata_node(
    f: &mut fmt::Formatter<'_>,
    node: &crate::metadata::MetadataKind,
    module: &ModuleCore,
    store: &crate::metadata::MetadataStore,
    slots: &[Option<usize>],
) -> fmt::Result {
    use super::metadata::MetadataKind;
    match node {
        MetadataKind::Null => f.write_str("null"),
        MetadataKind::String(s) => fmt_md_string(f, s),
        MetadataKind::Tuple { distinct, operands } => {
            if *distinct {
                f.write_str("distinct ")?;
            }
            f.write_str("!{")?;
            for (i, op) in operands.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                fmt_metadata_operand(f, op.0, module, store, slots)?;
            }
            f.write_str("}")
        }
        MetadataKind::Ref(id) => fmt_metadata_operand(f, *id, module, store, slots),
        MetadataKind::Specialized(node) => {
            fmt_specialized_metadata_node(f, node, module, store, slots)
        }
        MetadataKind::Constant(id) => {
            let data = module.context().value_data(*id);
            let value = Value::from_parts(*id, module, data.ty);
            fmt_operand(f, value, None)
        }
    }
}
fn fmt_specialized_metadata_node(
    f: &mut fmt::Formatter<'_>,
    node: &crate::metadata::SpecializedMetadataNode,
    module: &ModuleCore,
    store: &crate::metadata::MetadataStore,
    slots: &[Option<usize>],
) -> fmt::Result {
    use super::metadata::MetadataFieldValue;
    if node.is_distinct() {
        f.write_str("distinct ")?;
    }
    write!(f, "!{}(", node.kind().name())?;
    for (i, field) in node.fields().iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{}: ", field.name())?;
        match field.value() {
            MetadataFieldValue::Null => f.write_str("null")?,
            MetadataFieldValue::Bool(v) => f.write_str(if *v { "true" } else { "false" })?,
            MetadataFieldValue::Integer(v) => write!(f, "{v}")?,
            MetadataFieldValue::String(s) => {
                f.write_str("\"")?;
                print_escaped_string(f, s.as_bytes())?;
                f.write_str("\"")?;
            }
            MetadataFieldValue::Enum(s) => f.write_str(s)?,
            MetadataFieldValue::Metadata(md) => {
                fmt_metadata_operand(f, md.0, module, store, slots)?
            }
            MetadataFieldValue::MetadataList(items) => {
                f.write_str("!{")?;
                for (j, md) in items.iter().enumerate() {
                    if j > 0 {
                        f.write_str(", ")?;
                    }
                    fmt_metadata_operand(f, md.0, module, store, slots)?;
                }
                f.write_str("}")?;
            }
        }
    }
    f.write_str(")")
}

fn fmt_metadata_attachments(
    f: &mut fmt::Formatter<'_>,
    attachments: &crate::metadata::MetadataAttachmentSet,
    module: &ModuleCore,
    store: &crate::metadata::MetadataStore,
    slots: &[Option<usize>],
) -> fmt::Result {
    for (kind, id) in attachments.iter() {
        write!(f, ", !{} ", kind.name())?;
        fmt_metadata_operand(f, *id, module, store, slots)?;
    }
    Ok(())
}

/// Print a single metadata operand. `MDString`, `DIExpression`, and other
/// upstream-inline node families are printed inline; slotted MDNodes are
/// referenced by the module metadata slot map.
fn fmt_metadata_operand(
    f: &mut fmt::Formatter<'_>,
    id: crate::metadata::MetadataId,
    module: &ModuleCore,
    store: &crate::metadata::MetadataStore,
    slots: &[Option<usize>],
) -> fmt::Result {
    if let Some(node) = store.get(id) {
        if let crate::metadata::MetadataKind::String(s) = node {
            return fmt_md_string(f, s);
        }
        if is_inline_metadata_node(node) {
            return fmt_metadata_node(f, node, module, store, slots);
        }
    }
    match slots.get(id.index()).and_then(|slot| *slot) {
        Some(slot) => write!(f, "!{slot}"),
        None => write!(f, "!{}", id.index()),
    }
}

fn is_inline_metadata_node(node: &crate::metadata::MetadataKind) -> bool {
    matches!(
        node,
        crate::metadata::MetadataKind::Null | crate::metadata::MetadataKind::Constant(_)
    ) || matches!(
        node,
        crate::metadata::MetadataKind::Specialized(s)
            if s.kind() == crate::metadata::SpecializedMetadataKind::DIExpression
    )
}

fn metadata_slot_map(nodes: &[crate::metadata::MetadataKind]) -> Vec<Option<usize>> {
    let mut slots = vec![None; nodes.len()];
    let mut next = 0;
    for (i, node) in nodes.iter().enumerate() {
        if !matches!(node, crate::metadata::MetadataKind::String(_))
            && !is_inline_metadata_node(node)
        {
            slots[i] = Some(next);
            next += 1;
        }
    }
    slots
}

fn fmt_comdat(f: &mut fmt::Formatter<'_>, c: crate::comdat::ComdatRef<'_>) -> fmt::Result {
    // `$<name> = comdat <kind>\n`. Mirrors
    // `Comdat::print` in `lib/IR/AsmWriter.cpp`.
    fmt_llvm_name(f, "$", c.name())?;
    writeln!(f, " = comdat {}", c.selection_kind())
}

fn fmt_global<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    g: crate::global_variable::GlobalVariable<'ctx, B>,
) -> fmt::Result {
    // Mirrors `AssemblyWriter::printGlobal` in
    // `lib/IR/AsmWriter.cpp`.
    fmt_global_value_ref(f, g.as_value())?;
    f.write_str(" = ")?;

    // `external` keyword in front of decl-only globals with
    // External linkage. Mirrors the special case in `printGlobal`.
    if !g.has_initializer() && g.linkage() == crate::global_value::Linkage::External {
        f.write_str("external ")?;
    }

    // Linkage keyword (with trailing space) -- empty for External.
    let linkage_kw = g.linkage().keyword();
    if !linkage_kw.is_empty() {
        f.write_str(linkage_kw)?;
        f.write_str(" ")?;
    }

    // Visibility / DLL / TLS / unnamed-addr. Each prints with a
    // trailing space when present. Order mirrors `printGlobal`.
    if let Some(s) = g.visibility().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    if let Some(s) = g.dll_storage_class().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    if let Some(s) = g.thread_local_mode().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    if let Some(s) = g.unnamed_addr().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }

    // Address space. Mirrors `printAddressSpace(M, AS, Out, "",
    // " ")` for non-zero AS.
    if g.address_space() != 0 {
        write!(f, "addrspace({}) ", g.address_space())?;
    }

    if g.is_externally_initialized() {
        f.write_str("externally_initialized ")?;
    }
    f.write_str(if g.is_constant() {
        "constant "
    } else {
        "global "
    })?;
    write!(f, "{}", g.value_type())?;

    // Initializer.
    if let Some(init) = g.initializer() {
        f.write_str(" ")?;
        let v = init.as_value();
        fmt_operand_ref(f, v, None)?;
    }

    // Section.
    if let Some(section) = g.section() {
        f.write_str(", section \"")?;
        print_escaped_string(f, section.as_bytes())?;
        f.write_str("\"")?;
    }

    // Partition.
    if let Some(partition) = g.partition() {
        f.write_str(", partition \"")?;
        print_escaped_string(f, partition.as_bytes())?;
        f.write_str("\"")?;
    }
    if let Some(c) = g.comdat() {
        f.write_str(", comdat")?;
        if c.name() != g.name() {
            f.write_str("(")?;
            fmt_llvm_name(f, "$", c.name())?;
            f.write_str(")")?;
        }
    }

    // Alignment. Mirrors `if (MaybeAlign A = GV->getAlign()) Out
    // << ", align " << A->value();`.
    if let Some(a) = g.align().align() {
        write!(f, ", align {}", a.value())?;
    }
    let module_view = g.module();
    let md = module_view.metadata_store();
    let md_slots = metadata_slot_map(md.nodes());
    fmt_metadata_attachments(f, &g.metadata(), g.module().core_ref(), &md, &md_slots)
}

pub(super) fn fmt_alias<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    a: GlobalAlias<'ctx, B>,
) -> fmt::Result {
    fmt_global_value_ref(f, a.as_value())?;
    f.write_str(" = ")?;
    let linkage_kw = a.linkage().keyword();
    if !linkage_kw.is_empty() {
        f.write_str(linkage_kw)?;
        f.write_str(" ")?;
    }
    if let Some(s) = a.visibility().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    if let Some(s) = a.dll_storage_class().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    if let Some(s) = a.thread_local_mode().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    if let Some(s) = a.unnamed_addr().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    f.write_str("alias ")?;
    write!(f, "{}, ", a.value_type())?;
    fmt_operand(f, a.aliasee().as_value(), None)?;
    if let Some(partition) = a.partition() {
        f.write_str(", partition \"")?;
        print_escaped_string(f, partition.as_bytes())?;
        f.write_str("\"")?;
    }
    let module_view = a.module();
    let md = module_view.metadata_store();
    let md_slots = metadata_slot_map(md.nodes());
    fmt_metadata_attachments(f, &a.metadata(), a.module().core_ref(), &md, &md_slots)?;
    f.write_str("\n")
}

pub(super) fn fmt_ifunc<'ctx, B: ModuleBrand + 'ctx>(
    f: &mut fmt::Formatter<'_>,
    i: GlobalIFunc<'ctx, B>,
) -> fmt::Result {
    fmt_global_value_ref(f, i.as_value())?;
    f.write_str(" = ")?;
    let linkage_kw = i.linkage().keyword();
    if !linkage_kw.is_empty() {
        f.write_str(linkage_kw)?;
        f.write_str(" ")?;
    }
    if let Some(s) = i.visibility().keyword() {
        f.write_str(s)?;
        f.write_str(" ")?;
    }
    f.write_str("ifunc ")?;
    write!(f, "{}, ", i.value_type())?;
    fmt_operand(f, i.resolver().as_value(), None)?;
    if let Some(partition) = i.partition() {
        f.write_str(", partition \"")?;
        print_escaped_string(f, partition.as_bytes())?;
        f.write_str("\"")?;
    }
    let module_view = i.module();
    let md = module_view.metadata_store();
    let md_slots = metadata_slot_map(md.nodes());
    fmt_metadata_attachments(f, &i.metadata(), i.module().core_ref(), &md, &md_slots)?;
    f.write_str("\n")
}

fn fmt_select(
    f: &mut fmt::Formatter<'_>,
    inst: &InstructionView<'_, impl ModuleBrand>,
    s: &crate::instr_types::SelectInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    f.write_str("select ")?;
    let cd = module.context().value_data(s.cond.get());
    let cv = Value::from_parts(s.cond.get(), module, cd.ty);
    write!(f, "{} ", cv.ty())?;
    fmt_operand_ref(f, cv, Some(slots))?;
    f.write_str(", ")?;
    let td = module.context().value_data(s.true_val.get());
    let tv = Value::from_parts(s.true_val.get(), module, td.ty);
    write!(f, "{} ", tv.ty())?;
    fmt_operand_ref(f, tv, Some(slots))?;
    f.write_str(", ")?;
    let fd = module.context().value_data(s.false_val.get());
    let fv = Value::from_parts(s.false_val.get(), module, fd.ty);
    write!(f, "{} ", fv.ty())?;
    fmt_operand_ref(f, fv, Some(slots))
}
