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
        | InstructionKindData::Phi(_) => true,
        InstructionKindData::Ret(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::Unreachable(_) => false,
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

fn fmt_aggregate_constant(
    f: &mut fmt::Formatter<'_>,
    host: Value<'_>,
    elem_ids: &[ValueId],
) -> fmt::Result {
    let module = host.module();
    let ty = host.ty();
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
        InstructionKindData::Br(b) => fmt_br(f, inst, b, slots),
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
    let module = inst.module();
    let lhs_data = module.context().value_data(c.lhs.get());
    let lhs = Value::from_parts(c.lhs.get(), module, lhs_data.ty);
    write!(f, "fcmp {} {} ", c.predicate.name(), lhs.ty())?;
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
    let module = inst.module();
    let pointee = crate::r#type::Type::new(l.pointee_ty, module);
    f.write_str("load ")?;
    if l.volatile {
        f.write_str("volatile ")?;
    }
    write!(f, "{}, ", pointee)?;
    let pd = module.context().value_data(l.ptr.get());
    let pv = Value::from_parts(l.ptr.get(), module, pd.ty);
    write!(f, "{} ", pv.ty())?;
    fmt_operand_ref(f, pv, Some(slots))?;
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
    let module = inst.module();
    f.write_str("store ")?;
    if s.volatile {
        f.write_str("volatile ")?;
    }
    let vd = module.context().value_data(s.value.get());
    let vv = Value::from_parts(s.value.get(), module, vd.ty);
    write!(f, "{} ", vv.ty())?;
    fmt_operand_ref(f, vv, Some(slots))?;
    f.write_str(", ")?;
    let pd = module.context().value_data(s.ptr.get());
    let pv = Value::from_parts(s.ptr.get(), module, pd.ty);
    write!(f, "{} ", pv.ty())?;
    fmt_operand_ref(f, pv, Some(slots))?;
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
    let mut first = true;
    for func in m.iter_functions() {
        if !first {
            f.write_str("\n")?;
        }
        first = false;
        fmt_function(f, func)?;
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
