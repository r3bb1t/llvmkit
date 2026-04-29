//! Textual `.ll` printer. Mirrors a slice of `llvm/lib/IR/AsmWriter.cpp`.
//!
//! Public surface is just the [`Display`](core::fmt::Display) impls on the IR handles
//! ([`Module`], [`FunctionValue`], [`BasicBlock`], [`Instruction`],
//! [`Value`]). The slot-tracking and per-construct printers stay
//! `pub(crate)` because consumers should reach for `format!("{module}")`
//! (or [`std::io::Write`] via `write!`) rather than poking at the
//! internals.
//!
//! ## What's shipped
//!
//! - Modules, named structs, function definitions.
//! - Basic blocks with terminators (`ret`).
//! - Instructions: `add`, `sub`, `mul`, `trunc`, `ret`.
//! - Constants: integer, float, undef, poison, null pointer, simple
//!   aggregates.
//! - Operand printing via slot numbering for unnamed values.
//! - Function-level `local_unnamed_addr` / `unnamed_addr`.
//! - Parameter and return attribute printing.
//!
//! Future opcodes hook into the per-instruction printer one match
//! arm at a time as their builders land.

use core::fmt;
use std::collections::HashMap;

use crate::AttrIndex;
use crate::attributes::{AttributeStorage, AttributeStored};
use crate::basic_block::BasicBlock;
use crate::constant::ConstantData;
use crate::function::FunctionValue;
use crate::instr_types::{
    BinaryOpData, BranchInstData, BranchKind, CastOpData, CmpInstData, PhiData, ReturnOpData,
};
use crate::instruction::{Instruction, InstructionKindData};
use crate::marker::Dyn;
use crate::module::Module;
use crate::r#type::{Type, TypeData};
use crate::value::{Value, ValueId, ValueKindData};

// --------------------------------------------------------------------------
// SlotTracker
// --------------------------------------------------------------------------

/// Per-function slot map. Mirrors the private `SlotTracker` inside
/// `AsmWriter.cpp`. Walks values in declaration order, assigning a
/// 0-based slot to every *unnamed* one.
pub(crate) struct SlotTracker {
    /// Local-scope slots: function arguments + instructions that
    /// produce a non-void result and lack a name.
    local: HashMap<ValueId, u32>,
    /// Basic-block slots: unnamed blocks get `; <label>:N`.
    blocks: HashMap<ValueId, u32>,
}

impl SlotTracker {
    /// Empty tracker for orphan IR (e.g. a [`BasicBlock`] not yet
    /// attached to a function).
    pub(crate) fn empty() -> Self {
        Self {
            local: HashMap::new(),
            blocks: HashMap::new(),
        }
    }

    /// Build a slot tracker for a single function. Arguments come
    /// first, then each basic block (header counts as a value), then
    /// every instruction in program order.
    pub(crate) fn for_function(f: FunctionValue<'_, Dyn>) -> Self {
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

    pub(crate) fn local(&self, id: ValueId) -> Option<u32> {
        self.local.get(&id).copied()
    }

    pub(crate) fn block(&self, id: ValueId) -> Option<u32> {
        self.blocks.get(&id).copied()
    }
}

/// `true` if `inst` produces a result that gets a textual name (or
/// slot). Terminators and stores don't.
fn produces_named_result(inst: &Instruction<'_>) -> bool {
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
        | InstructionKindData::CallBr(_)
        | InstructionKindData::Resume(_)
        | InstructionKindData::CatchReturn(_)
        | InstructionKindData::CleanupReturn(_)
        | InstructionKindData::Unreachable(_) => false,
        InstructionKindData::Invoke(_) => {
            !matches!(inst.ty().data(), crate::r#type::TypeData::Void)
        }
        InstructionKindData::CleanupPad(_) => true,
        InstructionKindData::CatchPad(_) => true,
        InstructionKindData::CatchSwitch(_) => true,
        InstructionKindData::LandingPad(_) => true,
        InstructionKindData::Call(_) => {
            // Void-returning calls don't get a `%name = ` prefix.
            !matches!(inst.ty().data(), crate::r#type::TypeData::Void)
        }
    }
}

fn inst_kind_data<'ctx>(inst: &Instruction<'ctx>) -> &'ctx InstructionKindData {
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
pub(crate) fn fmt_operand(
    f: &mut fmt::Formatter<'_>,
    v: Value<'_>,
    slots: Option<&SlotTracker>,
) -> fmt::Result {
    write!(f, "{} ", v.ty())?;
    fmt_operand_ref(f, v, slots)
}

/// Print just the SSA reference part: `%name` / `@name` / `%slot` /
/// constant body.
pub(crate) fn fmt_operand_ref(
    f: &mut fmt::Formatter<'_>,
    v: Value<'_>,
    slots: Option<&SlotTracker>,
) -> fmt::Result {
    let data = v.data();
    match &data.kind {
        ValueKindData::Function(_) => write!(f, "@{}", v.name().unwrap_or_default()),
        ValueKindData::BasicBlock(_) => match v.name() {
            Some(n) => write!(f, "%{n}"),
            None => match slots.and_then(|s| s.block(v.id)) {
                Some(slot) => write!(f, "%{slot}"),
                None => f.write_str("%<unnumbered>"),
            },
        },
        ValueKindData::Argument { .. } | ValueKindData::Instruction(_) => match v.name() {
            Some(n) => write!(f, "%{n}"),
            None => match slots.and_then(|s| s.local(v.id)) {
                Some(slot) => write!(f, "%{slot}"),
                None => f.write_str("%<unnumbered>"),
            },
        },
        ValueKindData::GlobalVariable(_) => {
            write!(f, "@{}", v.name().unwrap_or_default())
        }
        ValueKindData::Constant(c) => fmt_constant(f, v, c),
    }
}

// --------------------------------------------------------------------------
// Constant printing
// --------------------------------------------------------------------------

pub(crate) fn fmt_constant(
    f: &mut fmt::Formatter<'_>,
    host: Value<'_>,
    c: &ConstantData,
) -> fmt::Result {
    match c {
        ConstantData::Int(words) => fmt_int_constant(f, host.ty(), words),
        ConstantData::Float(bits) => fmt_float_constant(f, host.ty(), *bits),
        ConstantData::PointerNull => f.write_str("null"),
        ConstantData::Undef => f.write_str("undef"),
        ConstantData::Poison => f.write_str("poison"),
        ConstantData::Aggregate(elems) => fmt_aggregate_constant(f, host, elems),
    }
}

fn fmt_int_constant(f: &mut fmt::Formatter<'_>, ty: Type<'_>, words: &[u64]) -> fmt::Result {
    let bits = match ty.data() {
        TypeData::Integer { bits } => *bits,
        _ => unreachable!("integer-constant ty invariant"),
    };
    if bits == 1 {
        let v = words.first().copied().unwrap_or(0) & 1;
        return f.write_str(if v == 0 { "false" } else { "true" });
    }
    if bits <= 64 {
        // Print as a signed decimal: sign-extend the active bits.
        let raw = words.first().copied().unwrap_or(0);
        let active_mask: u64 = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let raw = raw & active_mask;
        let sign_bit: u64 = 1u64 << (bits - 1);
        let signed_value = if raw & sign_bit != 0 {
            // Sign-extend: subtract 2^bits.
            let two_n: i128 = 1i128 << bits;
            (raw as i128) - two_n
        } else {
            raw as i128
        };
        return write!(f, "{signed_value}");
    }
    // Wide integers: print as zero-padded hex magnitude with a `u`
    // prefix to mark unsigned. Mirrors LLVM's APInt textual fallback
    // for widths >64.
    f.write_str("u0x")?;
    for word in words.iter().rev() {
        write!(f, "{word:016x}")?;
    }
    Ok(())
}

fn fmt_float_constant(f: &mut fmt::Formatter<'_>, ty: Type<'_>, bits: u128) -> fmt::Result {
    match ty.data() {
        TypeData::Half => write!(f, "0xH{:04x}", bits as u16),
        TypeData::BFloat => write!(f, "0xR{:04x}", bits as u16),
        TypeData::Float | TypeData::Double => {
            // For both single and double, emit the IEEE 754 double-
            // precision hex representation. Mirrors AsmWriter.cpp's
            // `writeAPFloatInternal`.
            let as_double_bits: u64 = match ty.data() {
                TypeData::Float => f64::from(f32::from_bits(bits as u32)).to_bits(),
                TypeData::Double => bits as u64,
                _ => unreachable!(),
            };
            write!(f, "0x{as_double_bits:016x}")
        }
        TypeData::X86Fp80 => {
            let lo = bits as u64;
            let hi = (bits >> 64) as u16;
            write!(f, "0xK{hi:04x}{lo:016x}")
        }
        TypeData::Fp128 => {
            let lo = bits as u64;
            let hi = (bits >> 64) as u64;
            write!(f, "0xL{lo:016x}{hi:016x}")
        }
        TypeData::PpcFp128 => {
            let lo = bits as u64;
            let hi = (bits >> 64) as u64;
            write!(f, "0xM{lo:016x}{hi:016x}")
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

/// If the aggregate is `[N x i8]` and every element is a
/// `ConstantInt`, return the underlying byte sequence; else `None`.
/// Mirrors `ConstantDataArray::isString` (in C++ this is a runtime
/// downcast plus a per-element check).
fn collect_byte_string<'ctx>(
    module: &'ctx crate::module::Module<'ctx>,
    ty: Type<'_>,
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
                            let v = words.first().copied().unwrap_or(0);
                            bytes.push(v as u8);
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

fn fmt_aggregate_constant(
    f: &mut fmt::Formatter<'_>,
    host: Value<'_>,
    elem_ids: &[ValueId],
) -> fmt::Result {
    let module = host.module();
    let ty = host.ty();
    if let Some(bytes) = collect_byte_string(module, ty, elem_ids) {
        f.write_str("c\"")?;
        print_escaped_string(f, &bytes)?;
        return f.write_str("\"");
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

pub(crate) fn fmt_instruction(
    f: &mut fmt::Formatter<'_>,
    inst: &Instruction<'_>,
    slots: &SlotTracker,
) -> fmt::Result {
    f.write_str("  ")?;
    let kind = inst_kind_data(inst);
    if produces_named_result(inst) {
        match inst.name() {
            Some(n) => write!(f, "%{n} = ")?,
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
    }
}

fn fmt_binop(
    f: &mut fmt::Formatter<'_>,
    opcode: &str,
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
    c: &CastOpData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `<keyword> <src-ty> <src-ref> to <dst-ty>`
    f.write_str(c.kind.keyword())?;
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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

fn print_shuffle_mask(
    f: &mut fmt::Formatter<'_>,
    result_ty: crate::r#type::Type<'_>,
    mask: &[i32],
) -> fmt::Result {
    // Mirrors `printShuffleMask` in `lib/IR/AsmWriter.cpp`.
    f.write_str(", <")?;
    if matches!(
        result_ty.data(),
        crate::r#type::TypeData::ScalableVector { .. }
    ) {
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
    c: &CmpInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    let lhs_data = module.context().value_data(c.lhs.get());
    let lhs = Value::from_parts(c.lhs.get(), module, lhs_data.ty);
    write!(f, "icmp {} {} ", c.predicate.name(), lhs.ty())?;
    fmt_operand_ref(f, lhs, Some(slots))?;
    f.write_str(", ")?;
    let rhs_data = module.context().value_data(c.rhs.get());
    let rhs = Value::from_parts(c.rhs.get(), module, rhs_data.ty);
    fmt_operand_ref(f, rhs, Some(slots))
}
fn fmt_fcmp(
    f: &mut fmt::Formatter<'_>,
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
    a: &crate::instr_types::AllocaInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    let allocated = crate::r#type::Type::new(a.allocated_ty, module);
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
    inst: &Instruction<'_>,
    l: &crate::instr_types::LoadInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // Mirrors `AssemblyWriter::printInstruction` LoadInst arm in
    // `lib/IR/AsmWriter.cpp`: `load [atomic] [volatile] <ty>, <ptrty> <ptr>
    // [syncscope("...")] <ordering>, align N`.
    let module = inst.module();
    let pointee = crate::r#type::Type::new(l.pointee_ty, module);
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
    g: &crate::instr_types::GepInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    let module = inst.module();
    f.write_str("getelementptr ")?;
    let flags_str = format!("{}", g.flags);
    if !flags_str.is_empty() {
        write!(f, "{} ", flags_str)?;
    }
    let source = crate::r#type::Type::new(g.source_ty, module);
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
    inst: &Instruction<'_>,
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
    // Print the return type. Mirrors AsmWriter's
    // `printType(I.getType())`.
    write!(f, "{} ", inst.ty())?;
    let cd = module.context().value_data(c.callee.get());
    let callee = Value::from_parts(c.callee.get(), module, cd.ty);
    fmt_operand_ref(f, callee, Some(slots))?;
    f.write_str("(")?;
    let mut first = true;
    for arg_cell in c.args.iter() {
        let aid = arg_cell.get();
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let ad = module.context().value_data(aid);
        let av = Value::from_parts(aid, module, ad.ty);
        write!(f, "{} ", av.ty())?;
        fmt_operand_ref(f, av, Some(slots))?;
    }
    f.write_str(")")
}

fn fmt_landingpad(
    f: &mut fmt::Formatter<'_>,
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
    d: &crate::instr_types::InvokeInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `invoke [<cc>] <ret-ty> <callee>(<args>)\n          to label %normal unwind label %unwind`
    f.write_str("invoke ")?;
    if d.calling_conv != crate::CallingConv::C {
        write!(f, "{} ", d.calling_conv)?;
    }
    let module = inst.module();
    write!(f, "{} ", inst.ty())?;
    let callee_data = module.context().value_data(d.callee.get());
    let callee = Value::from_parts(d.callee.get(), module, callee_data.ty);
    fmt_operand_ref(f, callee, Some(slots))?;
    f.write_str("(")?;
    let mut first = true;
    for arg_cell in d.args.iter() {
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
    f.write_str(")\n          to ")?;
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
    inst: &Instruction<'_>,
    d: &crate::instr_types::CallBrInstData,
    slots: &SlotTracker,
) -> fmt::Result {
    // `callbr [<cc>] <ret-ty> <callee>(<args>)\n          to label %default [label %indirect1, ...]`
    f.write_str("callbr ")?;
    if d.calling_conv != crate::CallingConv::C {
        write!(f, "{} ", d.calling_conv)?;
    }
    let module = inst.module();
    write!(f, "{} ", inst.ty())?;
    let callee_data = module.context().value_data(d.callee.get());
    let callee = Value::from_parts(d.callee.get(), module, callee_data.ty);
    fmt_operand_ref(f, callee, Some(slots))?;
    f.write_str("(")?;
    let mut first = true;
    for arg_cell in d.args.iter() {
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
    f.write_str(")\n          to ")?;
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
    inst: &Instruction<'_>,
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
fn fmt_attribute_set(
    f: &mut fmt::Formatter<'_>,
    storage: &AttributeStorage,
    index: AttrIndex,
    leading_space: bool,
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
        fmt_attribute_stored(f, attr)?;
    }
    Ok(())
}

fn fmt_attribute_stored(f: &mut fmt::Formatter<'_>, attr: &AttributeStored) -> fmt::Result {
    match attr {
        AttributeStored::Enum(k) => f.write_str(k.name()),
        AttributeStored::Int(k, v) => write!(f, "{}({v})", k.name()),
        AttributeStored::Type(k, _ty_id) => {
            // Type-payload attributes are rare and need a Type<'ctx>
            // for printing; the storage form drops the lifetime so we
            // print the kind without its payload. The full path lives
            // when the IR builder wires a ModuleRef into print.
            f.write_str(k.name())
        }
        AttributeStored::String { key, value } if value.is_empty() => write!(f, "\"{key}\""),
        AttributeStored::String { key, value } => write!(f, "\"{key}\"=\"{value}\""),
    }
}

// --------------------------------------------------------------------------
// BasicBlock + Function + Module printing
// --------------------------------------------------------------------------

pub(crate) fn fmt_basic_block(
    f: &mut fmt::Formatter<'_>,
    bb: BasicBlock<'_, Dyn>,
    slots: &SlotTracker,
    is_first: bool,
) -> fmt::Result {
    if !is_first {
        f.write_str("\n")?;
    }
    if let Some(name) = bb.name() {
        write!(f, "{name}:")?;
    } else if let Some(slot) = slots.block(bb.as_value().id) {
        write!(f, "{slot}:")?;
    } else {
        f.write_str("<unnamed>:")?;
    }
    f.write_str("\n")?;
    for inst in bb.instructions() {
        fmt_instruction(f, &inst, slots)?;
        f.write_str("\n")?;
    }
    Ok(())
}

pub(crate) fn fmt_function(
    f: &mut fmt::Formatter<'_>,
    func: FunctionValue<'_, Dyn>,
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
    // Return-attribute slot: prints between `define` (or `declare`)
    // and the return type. Mirrors `define noundef i32 @main()`.
    f.write_str(" ")?;
    fmt_attribute_set(f, &attrs, AttrIndex::Return, false)?;
    if attrs.get(AttrIndex::Return).is_some() {
        f.write_str(" ")?;
    }
    write!(f, "{} @{}(", sig.return_type(), func.name())?;
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
            fmt_attribute_set(f, &attrs, AttrIndex::Param(arg.slot()), false)?;
        }
        f.write_str(" ")?;
        match arg.name() {
            Some(n) => write!(f, "%{n}")?,
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
    // Unnamed-address marker.
    if let Some(kw) = func.unnamed_addr().keyword() {
        write!(f, " {kw}")?;
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
    f.write_str("}\n")
}

pub(crate) fn fmt_module(f: &mut fmt::Formatter<'_>, m: &Module<'_>) -> fmt::Result {
    writeln!(f, "; ModuleID = '{}'", m.name())?;

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

    // Globals. Mirrors the `for (const GlobalVariable &GV :
    // M->globals())` loop in `printModule`.
    if !m.global_empty() {
        f.write_str("\n")?;
        for g in m.iter_globals() {
            fmt_global(f, g)?;
            f.write_str("\n")?;
        }
    }

    let mut first = true;
    for func in m.iter_functions() {
        if !first || !m.global_empty() || m.iter_comdats().len() > 0 {
            f.write_str("\n")?;
        }
        first = false;
        fmt_function(f, func)?;
    }
    Ok(())
}

fn fmt_comdat(f: &mut fmt::Formatter<'_>, c: crate::comdat::ComdatRef<'_>) -> fmt::Result {
    // `$<name> = comdat <kind>\n`. Mirrors
    // `Comdat::print` in `lib/IR/AsmWriter.cpp`.
    writeln!(f, "${} = comdat {}", c.name(), c.selection_kind())
}

fn fmt_global(
    f: &mut fmt::Formatter<'_>,
    g: crate::global_variable::GlobalVariable<'_>,
) -> fmt::Result {
    // Mirrors `AssemblyWriter::printGlobal` in
    // `lib/IR/AsmWriter.cpp`.
    write!(f, "@{} = ", g.name())?;

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

    // Comdat. Mirrors `maybePrintComdat` (with leading `,` for a
    // GlobalVariable host).
    if let Some(c) = g.comdat() {
        f.write_str(", comdat")?;
        if c.name() != g.name() {
            write!(f, "(${})", c.name())?;
        }
    }

    // Alignment. Mirrors `if (MaybeAlign A = GV->getAlign()) Out
    // << ", align " << A->value();`.
    if let Some(a) = g.align().align() {
        write!(f, ", align {}", a.value())?;
    }
    Ok(())
}

fn fmt_select(
    f: &mut fmt::Formatter<'_>,
    inst: &Instruction<'_>,
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
