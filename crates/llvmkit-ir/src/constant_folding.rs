//! DataLayout-aware analysis constant folding.
//!
//! Mirrors the public routines in `llvm/include/llvm/Analysis/ConstantFolding.h`.
//! The target-independent subset lives in [`crate::constant_fold`]; this module
//! adds the folds that need `DataLayout`, denormal handling, or target-library
//! availability.

use super::ap_float::{ApFloat, ApFloatSemantics, ApFloatSign};
use super::cmp_predicate::CmpPredicate;
use super::constant::{
    Constant, ConstantData, ConstantExprData, ConstantExprFlags, ConstantExprOpcode,
};
use super::constant_fold::{
    constant_fold_binary_instruction, constant_fold_cast_instruction,
    constant_fold_compare_instruction, constant_fold_extract_element_instruction,
    constant_fold_extract_value_instruction, constant_fold_get_element_ptr,
    constant_fold_insert_element_instruction, constant_fold_insert_value_instruction,
    constant_fold_select_instruction, constant_fold_shuffle_vector_instruction,
    constant_fold_unary_instruction, shufflevector_mask_from_constant,
};
use super::constants::{ConstantFloatValue, ConstantIntValue};
use super::data_layout::DataLayout;
use super::denormal_mode::{DenormalMode, DenormalModeKind, DenormalModeSide};
use super::derived_types::{FloatType, IntType};
use super::float_kind::FloatDyn;
use super::global_variable::GlobalVariable;
use super::instr_types::{BinaryOpcode, CastOpcode, PhiData, UnaryOpcode};
use super::instruction::{InstructionKindData, InstructionView};
use super::int_width::IntDyn;
use super::intrinsics::BinaryIntrinsic;
use super::module::{Brand, ModuleBrand, ModuleRef, ModuleView};
use super::target_library_info::{LibFunc, TargetLibraryInfo};
use super::r#type::{MAX_INT_BITS, MIN_INT_BITS, Type, TypeData};
use super::value::{Value, ValueId, ValueKindData};
use super::{ApInt, Dyn, FunctionValue, IrError, IrResult};

/// Whether folds that depend on host/libm floating-point determinism are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FoldNonDeterminism {
    Allow,
    Deny,
}

/// Flags preserved while collapsing a pair of casts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PreservedCastFlags {
    non_negative: bool,
    no_unsigned_wrap: bool,
    no_signed_wrap: bool,
}

impl PreservedCastFlags {
    #[inline]
    pub const fn none() -> Self {
        Self {
            non_negative: false,
            no_unsigned_wrap: false,
            no_signed_wrap: false,
        }
    }

    #[inline]
    pub const fn non_negative(mut self) -> Self {
        self.non_negative = true;
        self
    }

    #[inline]
    pub const fn no_unsigned_wrap(mut self) -> Self {
        self.no_unsigned_wrap = true;
        self
    }

    #[inline]
    pub const fn no_signed_wrap(mut self) -> Self {
        self.no_signed_wrap = true;
        self
    }

    #[inline]
    pub const fn has_non_negative(self) -> bool {
        self.non_negative
    }

    #[inline]
    pub const fn has_no_unsigned_wrap(self) -> bool {
        self.no_unsigned_wrap
    }

    #[inline]
    pub const fn has_no_signed_wrap(self) -> bool {
        self.no_signed_wrap
    }
}

/// Constant pointer offset relative to one global object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConstantOffsetFromGlobal<'ctx, B: ModuleBrand = Brand<'ctx>> {
    global: GlobalVariable<'ctx, B>,
    offset: ApInt,
}

impl<'ctx, B: ModuleBrand + 'ctx> ConstantOffsetFromGlobal<'ctx, B> {
    #[inline]
    pub(super) fn new(global: GlobalVariable<'ctx, B>, offset: ApInt) -> Self {
        Self { global, offset }
    }

    /// Base global object.
    #[inline]
    pub fn global(&self) -> GlobalVariable<'ctx, B> {
        self.global
    }

    /// Byte offset from the start of [`Self::global`].
    #[inline]
    pub fn offset(&self) -> &ApInt {
        &self.offset
    }
}

/// True if the target library permits folding `lib_func`.
#[inline]
pub fn can_constant_fold_call_to(lib_func: LibFunc, tli: &TargetLibraryInfo) -> bool {
    tli.has(lib_func)
}

/// Flush one floating-point constant according to `mode`.
pub fn flush_fp_constant<'ctx, B: ModuleBrand + 'ctx>(
    operand: Constant<'ctx, B>,
    mode: DenormalMode,
    side: DenormalModeSide,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(fp) = ConstantFloatValue::<FloatDyn, B>::try_from(operand) {
        let apf = fp.ap_float();
        if !apf.is_denormal() {
            return Ok(Some(operand));
        }
        return flush_denormal_float(fp.ty(), &apf, mode.for_side(side));
    }

    if is_undef(operand) || is_poison(operand) {
        return Ok(Some(operand));
    }

    Ok(None)
}

/// Resolve a constant pointer to a global plus byte offset.
pub fn constant_offset_from_global<'ctx, B: ModuleBrand + 'ctx>(
    pointer: Constant<'ctx, B>,
    dl: &DataLayout,
) -> Option<ConstantOffsetFromGlobal<'ctx, B>> {
    constant_offset_from_global_with_offset(
        pointer,
        ApInt::zero(index_bits_for_constant_offset(pointer, dl)?),
        dl,
    )
}

/// Resolve a constant pointer to a global plus byte offset.
#[inline]
pub fn is_constant_offset_from_global<'ctx, B: ModuleBrand + 'ctx>(
    pointer: Constant<'ctx, B>,
    dl: &DataLayout,
) -> Option<ConstantOffsetFromGlobal<'ctx, B>> {
    constant_offset_from_global(pointer, dl)
}

/// Fold a load from a pointer to a constant global.
pub fn constant_fold_load_from_const_ptr<'ctx, B: ModuleBrand + 'ctx>(
    pointer: Constant<'ctx, B>,
    ty: Type<'ctx, B>,
    offset: ApInt,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some(resolved) = constant_offset_from_global_with_offset(pointer, offset, dl) else {
        return Ok(None);
    };
    let global = resolved.global();
    if !global.is_constant() || global.is_externally_initialized() {
        return Ok(None);
    }
    let Some(initializer) = global.initializer() else {
        return Ok(None);
    };
    constant_fold_load_from_const(initializer, ty, resolved.offset().clone(), dl)
}

/// Fold a load directly from a constant aggregate/scalar at byte `offset`.
pub fn constant_fold_load_from_const<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    ty: Type<'ctx, B>,
    offset: ApInt,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if offset.is_negative() {
        return Ok(None);
    }
    let Some(start) = offset.try_zext_u64() else {
        return Ok(None);
    };
    let alloc_size = dl.type_alloc_size(erase_type(constant.ty()));
    if start >= alloc_size {
        return Ok(Some(ty.get_poison().as_constant()));
    }
    let load_size = dl.type_store_size(erase_type(ty));
    let Some(end) = start.checked_add(load_size) else {
        return Ok(None);
    };
    // Mirrors `ConstantFoldLoadFromConst` in
    // `llvm/lib/Analysis/ConstantFolding.cpp`: first try the constant stored at
    // the byte offset, then reinterpret it through the requested load type.
    if let Some(at_offset) = constant_at_offset(constant, start, dl)?
        && let Some(folded) = constant_fold_load_through_bitcast(at_offset, ty, dl)?
    {
        return Ok(Some(folded));
    }
    let bytes = match constant_to_store_bytes(constant, dl)? {
        Some(bytes) => bytes,
        None => return fold_uniform_constant_load(constant, ty, dl),
    };
    let Ok(start_index) = usize::try_from(start) else {
        return Ok(None);
    };
    let Ok(end_index) = usize::try_from(end) else {
        return Ok(None);
    };
    if end_index > bytes.len() {
        return Ok(Some(ty.get_poison().as_constant()));
    }
    constant_from_store_bytes(ty, &bytes[start_index..end_index], dl)
}

/// If `constant` has a uniform bit pattern, materialise that value as `ty`.
pub fn constant_fold_load_from_uniform_value<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    fold_uniform_constant_load(constant, ty, dl)
}

/// Fold a cast using `DataLayout` where target-independent `ConstantFold.cpp`
/// cannot decide.
pub fn constant_fold_cast_operand<'ctx, B: ModuleBrand + 'ctx>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Some(folded) = constant_fold_cast_instruction(opcode, operand, dest_ty)? {
        return Ok(Some(folded));
    }

    match opcode {
        CastOpcode::PtrToInt | CastOpcode::PtrToAddr => {
            if let Some(folded) = fold_ptr_to_int_pair(opcode, operand, dest_ty, dl)? {
                return Ok(Some(folded));
            }
        }
        CastOpcode::IntToPtr => {
            if let Some(folded) = fold_int_to_ptr_pair(operand, dest_ty, dl)? {
                return Ok(Some(folded));
            }
        }
        CastOpcode::BitCast => return fold_bitcast_with_layout(operand, dest_ty, dl),
        CastOpcode::Trunc
        | CastOpcode::ZExt
        | CastOpcode::SExt
        | CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::FpToUI
        | CastOpcode::FpToSI
        | CastOpcode::UIToFp
        | CastOpcode::SIToFp
        | CastOpcode::AddrSpaceCast => {}
    }

    if opcode.is_desirable_constant_expr() {
        let Some(expr_opcode) = cast_constant_expr_opcode(opcode) else {
            return Ok(None);
        };
        let expr = operand.as_value().module().core_ref().constant_expr(
            erase_type(dest_ty),
            expr_opcode,
            [erase_value(operand.as_value())],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        return Ok(Some(rebrand_constant(expr, operand.as_value().module())));
    }

    Ok(None)
}

/// Constant fold a zext, sext or trunc according to source and destination width.
pub fn constant_fold_integer_cast<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    is_signed: bool,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    fold_integer_cast_constant(constant, dest_ty, is_signed, dl)
}

/// Fold an instruction using DataLayout-aware analysis rules.
pub fn constant_fold_instruction<'ctx, B>(
    instruction: &InstructionView<'ctx, B>,
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Option<Constant<'ctx, B>>>
where
    B: ModuleBrand + 'ctx,
{
    let value = instruction.as_value();
    let ValueKindData::Instruction(data) = &value.data().kind else {
        return Ok(None);
    };

    if let InstructionKindData::Phi(phi) = &data.kind {
        return fold_phi(value.ty(), value.module(), phi);
    }

    if let InstructionKindData::Call(call) = &data.kind {
        let Some(tli) = tli else {
            return Ok(None);
        };
        let callee_data = value.module().context().value_data(call.callee.get());
        let ValueKindData::Function(_) = &callee_data.kind else {
            return Ok(None);
        };
        let function =
            FunctionValue::<Dyn, B>::from_parts_unchecked(call.callee.get(), value.module());
        let Some(lib_func) = tli.lib_func_for_name(function.name()) else {
            return Ok(None);
        };
        let mut args = Vec::with_capacity(call.args.len());
        for arg in call.args.iter().map(|arg| arg.get()) {
            let Some(constant) = constant_from_id(value.module(), arg) else {
                return Ok(None);
            };
            args.push(constant_fold_constant(constant, dl, Some(tli))?);
        }
        return constant_fold_call(lib_func, &args, value.ty(), tli, FoldNonDeterminism::Allow);
    }

    let mut operands = Vec::new();
    for id in data.kind.operand_ids() {
        let Some(constant) = constant_from_id(value.module(), id) else {
            return Ok(None);
        };
        operands.push(constant_fold_constant(constant, dl, tli)?);
    }

    constant_fold_inst_operands(instruction, &operands, dl, tli, FoldNonDeterminism::Allow)
}

/// Fold a constant using DataLayout-aware analysis rules.
pub fn constant_fold_constant<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Constant<'ctx, B>> {
    let module = constant.as_value().module();
    match &constant.as_value().data().kind {
        ValueKindData::Constant(ConstantData::Expr(expr)) => {
            let Some(operands) =
                folded_constants_from_ids(module, expr.operands.iter().copied(), dl, tli)?
            else {
                return Ok(constant);
            };
            if let Some(folded) =
                constant_fold_constant_expr_operands(constant, expr, &operands, dl, tli)?
            {
                Ok(folded)
            } else {
                Ok(constant)
            }
        }
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            let mut changed = false;
            let mut folded_ids = Vec::with_capacity(elements.len());
            for id in elements.iter().copied() {
                let Some(element) = constant_from_id(module, id) else {
                    return Ok(constant);
                };
                let folded = constant_fold_constant(element, dl, tli)?;
                changed |= folded.as_value().id != id;
                folded_ids.push(folded.as_value().id);
            }
            if !changed {
                return Ok(constant);
            }
            let id = module
                .context()
                .intern_constant_aggregate(constant.ty().id(), folded_ids.into_boxed_slice());
            Ok(Constant::from_parts(Value::from_parts(
                id,
                module,
                constant.ty().id(),
            )))
        }
        _ => Ok(constant),
    }
}

/// Fold an instruction with caller-provided constant operands.
pub fn constant_fold_inst_operands<'ctx, B>(
    instruction: &InstructionView<'ctx, B>,
    operands: &[Constant<'ctx, B>],
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B>>>
where
    B: ModuleBrand + 'ctx,
{
    let value = instruction.as_value();
    let ValueKindData::Instruction(data) = &value.data().kind else {
        return Ok(None);
    };

    if let Some(opcode) = binary_opcode(&data.kind) {
        let [lhs, rhs] = operands else {
            return Ok(None);
        };
        return if matches!(
            opcode,
            BinaryOpcode::FAdd
                | BinaryOpcode::FSub
                | BinaryOpcode::FMul
                | BinaryOpcode::FDiv
                | BinaryOpcode::FRem
        ) {
            constant_fold_fp_inst_operands(
                opcode,
                *lhs,
                *rhs,
                dl,
                denormal_mode_for_instruction(data.parent.get(), value.module(), value.ty()),
                allow_non_deterministic,
            )
        } else {
            constant_fold_binary_op_operands(opcode, *lhs, *rhs, dl)
        };
    }

    match &data.kind {
        InstructionKindData::Cast(cast) => {
            let [operand] = operands else {
                return Ok(None);
            };
            constant_fold_cast_operand(cast.kind, *operand, value.ty(), dl)
        }
        InstructionKindData::ICmp(cmp) => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            constant_fold_compare_inst_operands(
                CmpPredicate::Int(cmp.predicate),
                *lhs,
                *rhs,
                dl,
                None,
            )
        }
        InstructionKindData::FCmp(cmp) => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            constant_fold_compare_inst_operands(
                CmpPredicate::Float(cmp.predicate),
                *lhs,
                *rhs,
                dl,
                Some(denormal_mode_for_instruction(
                    data.parent.get(),
                    value.module(),
                    lhs.ty(),
                )),
            )
        }
        InstructionKindData::FNeg(_) => {
            let [operand] = operands else {
                return Ok(None);
            };
            constant_fold_unary_op_operand(UnaryOpcode::FNeg, *operand, dl)
        }
        InstructionKindData::Select(_) => {
            let [condition, true_value, false_value] = operands else {
                return Ok(None);
            };
            constant_fold_select_instruction(*condition, *true_value, *false_value)
        }
        InstructionKindData::ExtractElement(_) => {
            let [vector, index] = operands else {
                return Ok(None);
            };
            constant_fold_extract_element_instruction(*vector, *index)
        }
        InstructionKindData::InsertElement(_) => {
            let [vector, value, index] = operands else {
                return Ok(None);
            };
            constant_fold_insert_element_instruction(*vector, *value, *index)
        }
        InstructionKindData::ShuffleVector(shuffle) => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            constant_fold_shuffle_vector_instruction(*lhs, *rhs, &shuffle.mask)
        }
        InstructionKindData::ExtractValue(extract) => {
            let [aggregate] = operands else {
                return Ok(None);
            };
            constant_fold_extract_value_instruction(*aggregate, &extract.indices)
        }
        InstructionKindData::InsertValue(insert) => {
            let [aggregate, value] = operands else {
                return Ok(None);
            };
            constant_fold_insert_value_instruction(*aggregate, *value, &insert.indices)
        }
        InstructionKindData::Gep(gep) => {
            let Some((pointer, indices)) = operands.split_first() else {
                return Ok(None);
            };
            constant_fold_get_element_ptr(
                Type::new(gep.source_ty, value.module()),
                *pointer,
                indices,
                None,
            )
        }
        InstructionKindData::Load(load) => {
            if load.volatile {
                return Ok(None);
            }
            let [pointer] = operands else {
                return Ok(None);
            };
            let Some(index_bits) = index_bits_for_pointer(pointer.ty(), dl) else {
                return Ok(None);
            };
            constant_fold_load_from_const_ptr(*pointer, value.ty(), ApInt::zero(index_bits), dl)
        }
        // Mirrors `ConstantFoldInstOperandsImpl`'s Freeze arm in
        // `llvm/lib/Analysis/ConstantFolding.cpp`: only forward operands proven
        // not to be undef or poison.
        InstructionKindData::Freeze(_) => {
            let [operand] = operands else {
                return Ok(None);
            };
            if is_guaranteed_not_to_be_undef_or_poison(*operand) {
                Ok(Some(*operand))
            } else {
                Ok(None)
            }
        }
        InstructionKindData::Call(call) => {
            let Some(tli) = tli else {
                return Ok(None);
            };
            let callee_data = value.module().context().value_data(call.callee.get());
            let ValueKindData::Function(_) = &callee_data.kind else {
                return Ok(None);
            };
            let function =
                FunctionValue::<Dyn, B>::from_parts_unchecked(call.callee.get(), value.module());
            let Some(lib_func) = tli.lib_func_for_name(function.name()) else {
                return Ok(None);
            };
            let args = if operands.len() == call.args.len() {
                operands
            } else if operands.len() == call.args.len().saturating_add(1) {
                &operands[1..]
            } else {
                return Ok(None);
            };
            constant_fold_call(lib_func, args, value.ty(), tli, allow_non_deterministic)
        }
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
        | InstructionKindData::Phi(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Ret(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::VAArg(_)
        | InstructionKindData::Fence(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRMW(_)
        | InstructionKindData::Switch(_)
        | InstructionKindData::IndirectBr(_)
        | InstructionKindData::Invoke(_)
        | InstructionKindData::CallBr(_)
        | InstructionKindData::LandingPad(_)
        | InstructionKindData::Resume(_)
        | InstructionKindData::CleanupPad(_)
        | InstructionKindData::CatchPad(_)
        | InstructionKindData::CatchReturn(_)
        | InstructionKindData::CleanupReturn(_)
        | InstructionKindData::CatchSwitch(_)
        | InstructionKindData::Unreachable(_) => Ok(None),
    }
}

/// Fold an integer or floating-point compare with caller-provided operands.
pub fn constant_fold_compare_inst_operands<'ctx, B: ModuleBrand + 'ctx>(
    predicate: CmpPredicate,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
    dl: &DataLayout,
    denormal_mode: Option<DenormalMode>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let lhs = if lhs.ty().is_floating_point() {
        if let Some(mode) = denormal_mode {
            let Some(flushed) = flush_fp_constant(lhs, mode, DenormalModeSide::Input)? else {
                return Ok(None);
            };
            flushed
        } else {
            lhs
        }
    } else {
        lhs
    };
    let rhs = if rhs.ty().is_floating_point() {
        if let Some(mode) = denormal_mode {
            let Some(flushed) = flush_fp_constant(rhs, mode, DenormalModeSide::Input)? else {
                return Ok(None);
            };
            flushed
        } else {
            rhs
        }
    } else {
        rhs
    };
    let _ = dl;
    constant_fold_compare_instruction(predicate, lhs, rhs)
}

/// Fold a unary operation with a caller-provided constant operand.
pub fn constant_fold_unary_op_operand<'ctx, B: ModuleBrand + 'ctx>(
    opcode: UnaryOpcode,
    operand: Constant<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let _ = dl;
    constant_fold_unary_instruction(opcode, operand)
}

/// Fold a binary operation with caller-provided constant operands.
pub fn constant_fold_binary_op_operands<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Some(folded) = constant_fold_binary_instruction(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }
    let _ = dl;
    if lhs.ty() != rhs.ty() || !opcode.is_supported_constant_expr() {
        return Ok(None);
    }
    let Some(expr_opcode) = binary_constant_expr_opcode(opcode) else {
        return Ok(None);
    };
    let module = lhs.as_value().module();
    let expr = module.core_ref().constant_expr(
        erase_type(lhs.ty()),
        expr_opcode,
        [erase_value(lhs.as_value()), erase_value(rhs.as_value())],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    Ok(Some(rebrand_constant(expr, module)))
}

/// Fold a floating-point binary operation with denormal handling.
pub fn constant_fold_fp_inst_operands<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
    dl: &DataLayout,
    denormal_mode: DenormalMode,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some(lhs) = flush_fp_constant(lhs, denormal_mode, DenormalModeSide::Input)? else {
        return Ok(None);
    };
    let Some(rhs) = flush_fp_constant(rhs, denormal_mode, DenormalModeSide::Input)? else {
        return Ok(None);
    };
    let Some(folded) = constant_fold_binary_op_operands(opcode, lhs, rhs, dl)? else {
        return Ok(None);
    };
    let Some(folded) = flush_fp_constant(folded, denormal_mode, DenormalModeSide::Output)? else {
        return Ok(None);
    };
    if matches!(allow_non_deterministic, FoldNonDeterminism::Deny) && constant_is_nan(folded) {
        return Ok(None);
    }
    Ok(Some(folded))
}

/// Fold the currently modelled binary intrinsic set.
pub fn constant_fold_binary_intrinsic<'ctx, B: ModuleBrand + 'ctx>(
    intrinsic: BinaryIntrinsic,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
    ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let _ = (intrinsic, lhs, rhs, ty, dl);
    Ok(None)
}

/// Reinterpret `constant` as though it were loaded through a bitcasted pointer.
pub fn constant_fold_load_through_bitcast<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let mut current = Some(constant);
    while let Some(value) = current {
        if value.ty() == dest_ty {
            return Ok(Some(value));
        }
        let src_size = dl.type_size_in_bits(erase_type(value.ty()));
        let dest_size = dl.type_size_in_bits(erase_type(dest_ty));
        if src_size < dest_size {
            return Ok(None);
        }
        if let Some(folded) = constant_fold_load_from_uniform_value(value, dest_ty, dl)? {
            return Ok(Some(folded));
        }
        // Mirrors `ConstantFoldLoadThroughBitcast`: the all-zero/uniform case
        // above may legally coerce non-integral pointers, but direct
        // pointer/int casts require both sides to agree on non-integrality.
        if src_size == dest_size
            && is_non_integral_pointer_type(value.ty(), dl)
                == is_non_integral_pointer_type(dest_ty, dl)
        {
            let opcode = load_through_bitcast_opcode(value.ty(), dest_ty);
            if let Some(folded) = constant_fold_cast_operand(opcode, value, dest_ty, dl)? {
                return Ok(Some(folded));
            }
        }
        current = aggregate_first_element(value, dl)?;
    }
    Ok(None)
}

/// Return an inverse cast if recasting it with `cast_op` losslessly recovers `constant`.
pub fn lossless_inv_cast<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    inv_cast_to: Type<'ctx, B>,
    cast_op: CastOpcode,
    dl: &DataLayout,
) -> IrResult<Option<(Constant<'ctx, B>, PreservedCastFlags)>> {
    match cast_op {
        CastOpcode::BitCast => {
            let Some(folded) =
                constant_fold_cast_operand(CastOpcode::BitCast, constant, inv_cast_to, dl)?
            else {
                return Ok(None);
            };
            Ok(Some((folded, PreservedCastFlags::none())))
        }
        CastOpcode::Trunc => {
            let Some(zext) =
                constant_fold_cast_operand(CastOpcode::ZExt, constant, inv_cast_to, dl)?
            else {
                return Ok(None);
            };
            let mut flags = PreservedCastFlags::none().no_unsigned_wrap();
            if constant_fold_cast_operand(CastOpcode::SExt, constant, inv_cast_to, dl)?
                == Some(zext)
            {
                flags = flags.no_signed_wrap();
            }
            Ok(Some((zext, flags)))
        }
        CastOpcode::ZExt | CastOpcode::SExt => {
            let Some(inv) =
                constant_fold_cast_operand(CastOpcode::Trunc, constant, inv_cast_to, dl)?
            else {
                return Ok(None);
            };
            let Some(recast) = constant_fold_cast_operand(cast_op, inv, constant.ty(), dl)? else {
                return Ok(None);
            };
            if recast != constant {
                return Ok(None);
            }
            let mut flags = PreservedCastFlags::none();
            if cast_op == CastOpcode::ZExt
                && constant_fold_cast_operand(CastOpcode::SExt, inv, constant.ty(), dl)?
                    == Some(recast)
            {
                flags = flags.non_negative();
            }
            Ok(Some((inv, flags)))
        }
        CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::FpToUI
        | CastOpcode::FpToSI
        | CastOpcode::UIToFp
        | CastOpcode::SIToFp
        | CastOpcode::PtrToInt
        | CastOpcode::PtrToAddr
        | CastOpcode::IntToPtr
        | CastOpcode::AddrSpaceCast => Ok(None),
    }
}

/// Return a truncation that can be losslessly unsigned-extended back to `constant`.
pub fn lossless_unsigned_trunc<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<(Constant<'ctx, B>, PreservedCastFlags)>> {
    lossless_inv_cast(constant, dest_ty, CastOpcode::ZExt, dl)
}

/// Return a truncation that can be losslessly signed-extended back to `constant`.
pub fn lossless_signed_trunc<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<(Constant<'ctx, B>, PreservedCastFlags)>> {
    lossless_inv_cast(constant, dest_ty, CastOpcode::SExt, dl)
}
/// Fold a known library call with constant operands.
pub fn constant_fold_call<'ctx, B: ModuleBrand + 'ctx>(
    lib_func: LibFunc,
    operands: &[Constant<'ctx, B>],
    result_ty: Type<'ctx, B>,
    tli: &TargetLibraryInfo,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if !can_constant_fold_call_to(lib_func, tli) {
        return Ok(None);
    }
    // Keep APFloat-native folds such as sqrt available when deterministic mode
    // denies host-libm-dependent folds; unported libm arms decline below.
    match lib_func {
        LibFunc::Sqrt | LibFunc::Sqrtf => fold_sqrt_call(operands, result_ty),
        _ if matches!(allow_non_deterministic, FoldNonDeterminism::Deny) => Ok(None),
        _ => Ok(None),
    }
}

fn fold_phi<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    module: ModuleView<'ctx, B>,
    phi: &PhiData,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let mut common = None;
    for (incoming, _) in phi.incoming.borrow().iter() {
        let Some(constant) = constant_from_id(module, incoming.get()) else {
            return Ok(None);
        };
        if is_undef(constant) {
            continue;
        }
        if let Some(previous) = common {
            if previous != constant {
                return Ok(None);
            }
        } else {
            common = Some(constant);
        }
    }
    Ok(Some(match common {
        Some(value) => value,
        None => ty.get_undef().as_constant(),
    }))
}

fn fold_sqrt_call<'ctx, B: ModuleBrand + 'ctx>(
    operands: &[Constant<'ctx, B>],
    result_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let [operand] = operands else {
        return Ok(None);
    };
    let Ok(src) = ConstantFloatValue::<FloatDyn, B>::try_from(*operand) else {
        return Ok(None);
    };
    if src.ty().as_type() != result_ty {
        return Ok(None);
    }

    let ap = src.ap_float();
    if ap.is_negative() && ap.is_finite() {
        return Ok(None);
    }

    match src.ty().semantics() {
        ApFloatSemantics::IeeeSingle => {
            let Some(bits) = ap.to_bits().try_zext_u64() else {
                return Ok(None);
            };
            let Ok(bits) = u32::try_from(bits) else {
                return Ok(None);
            };
            let result = f32::from_bits(bits).sqrt();
            let apf = ApFloat::from_bits(
                ApFloatSemantics::IeeeSingle,
                &ApInt::from_words(32, &[u64::from(result.to_bits())]),
            )?;
            Ok(Some(src.ty().const_ap_float(&apf)?.as_constant()))
        }
        ApFloatSemantics::IeeeDouble => {
            let Some(bits) = ap.to_bits().try_zext_u64() else {
                return Ok(None);
            };
            let result = f64::from_bits(bits).sqrt();
            let apf = ApFloat::from_bits(
                ApFloatSemantics::IeeeDouble,
                &ApInt::from_words(64, &[result.to_bits()]),
            )?;
            Ok(Some(src.ty().const_ap_float(&apf)?.as_constant()))
        }
        ApFloatSemantics::IeeeHalf
        | ApFloatSemantics::BFloat
        | ApFloatSemantics::IeeeQuad
        | ApFloatSemantics::X87DoubleExtended
        | ApFloatSemantics::PpcDoubleDouble => Ok(None),
    }
}

fn flush_denormal_float<'ctx, B: ModuleBrand + 'ctx>(
    ty: FloatType<'ctx, FloatDyn, B>,
    value: &ApFloat,
    mode: DenormalModeKind,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let folded = match mode {
        DenormalModeKind::Dynamic => return Ok(None),
        DenormalModeKind::Ieee => value.clone(),
        DenormalModeKind::PreserveSign => {
            let sign = if value.is_negative() {
                ApFloatSign::Negative
            } else {
                ApFloatSign::Positive
            };
            ApFloat::zero(ty.semantics(), sign)
        }
        DenormalModeKind::PositiveZero => ApFloat::zero(ty.semantics(), ApFloatSign::Positive),
    };
    Ok(Some(ty.const_ap_float(&folded)?.as_constant()))
}

fn constant_offset_from_global_with_offset<'ctx, B: ModuleBrand + 'ctx>(
    pointer: Constant<'ctx, B>,
    offset: ApInt,
    dl: &DataLayout,
) -> Option<ConstantOffsetFromGlobal<'ctx, B>> {
    let module = pointer.as_value().module();
    match &pointer.as_value().data().kind {
        ValueKindData::Constant(ConstantData::GlobalValueRef { value }) => {
            let global_value =
                Value::from_parts(*value, module, module.context().value_data(*value).ty);
            let Ok(global) = GlobalVariable::try_from(global_value) else {
                return None;
            };
            Some(ConstantOffsetFromGlobal::new(global, offset))
        }
        ValueKindData::Constant(ConstantData::GepOffset { base_id, off }) => {
            let global_value =
                Value::from_parts(*base_id, module, module.context().value_data(*base_id).ty);
            let Ok(global) = GlobalVariable::try_from(global_value) else {
                return None;
            };
            let index_bits = index_bits_for_pointer(pointer.ty(), dl)?;
            let magnitude = ApInt::from_words(index_bits, &[off.unsigned_abs()]);
            let addend = if *off < 0 {
                magnitude.negate()
            } else {
                magnitude
            };
            Some(ConstantOffsetFromGlobal::new(
                global,
                offset.sext_or_trunc(index_bits).wrapping_add(&addend),
            ))
        }
        ValueKindData::Constant(ConstantData::Expr(expr)) => match expr.opcode {
            // Mirrors `IsConstantOffsetFromGlobal` in
            // `llvm/lib/Analysis/ConstantFolding.cpp`: constant expressions make
            // this analysis recursive through ptr/int and ptr/ptr casts.
            ConstantExprOpcode::PtrToInt
            | ConstantExprOpcode::PtrToAddr
            | ConstantExprOpcode::BitCast
            | ConstantExprOpcode::AddrSpaceCast => {
                let [operand_id] = expr.operands.as_ref() else {
                    return None;
                };
                let operand = constant_from_id(module, *operand_id)?;
                let offset = offset.sext_or_trunc(index_bits_for_constant_offset(operand, dl)?);
                constant_offset_from_global_with_offset(operand, offset, dl)
            }
            ConstantExprOpcode::GetElementPtr => {
                let source_ty = Type::new(expr.source_ty?, module);
                let (base_id, index_ids) = expr.operands.split_first()?;
                let base = constant_from_id(module, *base_id)?;
                let index_bits = index_bits_for_constant_offset(pointer, dl)
                    .or_else(|| index_bits_for_constant_offset(base, dl))?;
                let gep_offset = constant_gep_offset(source_ty, index_ids, module, index_bits, dl)?;
                constant_offset_from_global_with_offset(
                    base,
                    offset.sext_or_trunc(index_bits).wrapping_add(&gep_offset),
                    dl,
                )
            }
            _ => None,
        },
        _ => None,
    }
}

fn constant_gep_offset<'ctx, B: ModuleBrand + 'ctx>(
    source_ty: Type<'ctx, B>,
    index_ids: &[ValueId],
    module: ModuleView<'ctx, B>,
    index_bits: u32,
    dl: &DataLayout,
) -> Option<ApInt> {
    let mut offset = ApInt::zero(index_bits);
    let mut current_ty = source_ty;
    for (position, index_id) in index_ids.iter().copied().enumerate() {
        let index = constant_gep_index(module, index_id, index_bits)?;
        if position == 0 {
            offset = offset.wrapping_add(&scaled_offset(
                &index,
                dl.type_alloc_size(erase_type(current_ty)),
                index_bits,
            ));
            continue;
        }

        match current_ty.data() {
            TypeData::Array { elem, .. } => {
                let elem_ty = Type::new(*elem, module);
                offset = offset.wrapping_add(&scaled_offset(
                    &index,
                    dl.type_alloc_size(erase_type(elem_ty)),
                    index_bits,
                ));
                current_ty = elem_ty;
            }
            TypeData::FixedVector { elem, .. } => {
                let elem_ty = Type::new(*elem, module);
                offset = offset.wrapping_add(&scaled_offset(
                    &index,
                    dl.type_alloc_size(erase_type(elem_ty)),
                    index_bits,
                ));
                current_ty = elem_ty;
            }
            TypeData::Struct(data) => {
                if index.is_negative() {
                    return None;
                }
                let field = usize::try_from(index.try_zext_u64()?).ok()?;
                let body = data.body.borrow();
                let elem = *body.as_ref()?.elements.get(field)?;
                let layout = dl.struct_layout(erase_type(current_ty));
                offset = offset.wrapping_add(&ApInt::from_words(
                    index_bits,
                    &[layout.element_offset(field)],
                ));
                current_ty = Type::new(elem, module);
            }
            TypeData::ScalableVector { .. } => return None,
            _ => return None,
        }
    }
    Some(offset)
}

fn constant_gep_index<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    id: ValueId,
    index_bits: u32,
) -> Option<ApInt> {
    let constant = constant_from_id(module, id)?;
    Some(
        ConstantIntValue::<IntDyn, B>::try_from(constant)
            .ok()?
            .ap_int()
            .sext_or_trunc(index_bits),
    )
}

fn scaled_offset(index: &ApInt, scale: u64, index_bits: u32) -> ApInt {
    index.wrapping_mul(&ApInt::from_words(index_bits, &[scale]))
}

fn constant_at_offset<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    offset: u64,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if offset == 0 {
        return Ok(Some(constant));
    }
    let ValueKindData::Constant(ConstantData::Aggregate(elements)) =
        &constant.as_value().data().kind
    else {
        return Ok(None);
    };
    let module = constant.as_value().module();
    match constant.ty().data() {
        TypeData::Array { elem, .. } => {
            let elem_ty = Type::new(*elem, module);
            let stride = dl.type_alloc_size(erase_type(elem_ty));
            constant_at_indexed_offset(elements, elem_ty, stride, module, offset, dl)
        }
        TypeData::FixedVector { elem, .. } => {
            let elem_ty = Type::new(*elem, module);
            if !dl.type_size_equals_store_size(erase_type(elem_ty)) {
                return Ok(None);
            }
            let stride = dl.type_store_size(erase_type(elem_ty));
            constant_at_indexed_offset(elements, elem_ty, stride, module, offset, dl)
        }
        TypeData::Struct(_) => {
            let layout = dl.struct_layout(erase_type(constant.ty()));
            for (index, id) in elements.iter().copied().enumerate() {
                let Some(element) = constant_from_id(module, id) else {
                    return Ok(None);
                };
                let start = layout.element_offset(index);
                let size = dl.type_alloc_size(erase_type(element.ty()));
                let Some(end) = start.checked_add(size) else {
                    return Ok(None);
                };
                if offset < start {
                    return Ok(None);
                }
                if offset < end {
                    return constant_at_offset(element, offset - start, dl);
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn constant_at_indexed_offset<'ctx, B: ModuleBrand + 'ctx>(
    elements: &[ValueId],
    elem_ty: Type<'ctx, B>,
    stride: u64,
    module: ModuleView<'ctx, B>,
    offset: u64,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if stride == 0 {
        return Ok(None);
    }
    let index = offset / stride;
    let Ok(index) = usize::try_from(index) else {
        return Ok(None);
    };
    let Some(id) = elements.get(index).copied() else {
        return Ok(None);
    };
    let Some(element) = constant_from_id(module, id) else {
        return Ok(None);
    };
    if element.ty() != elem_ty {
        return Ok(None);
    }
    constant_at_offset(element, offset % stride, dl)
}

fn fold_uniform_constant_load<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if is_poison(constant) {
        return Ok(Some(ty.get_poison().as_constant()));
    }
    if is_undef(constant) {
        return Ok(Some(ty.get_undef().as_constant()));
    }
    if !dl.type_size_equals_store_size(erase_type(constant.ty())) {
        return Ok(None);
    }
    let Some(bytes) = constant_to_store_bytes(constant, dl)? else {
        return Ok(None);
    };
    if bytes.iter().all(|byte| *byte == 0) {
        return zero_constant_for_type(ty);
    }
    if bytes.iter().all(|byte| *byte == u8::MAX) {
        return all_ones_constant_for_type(ty);
    }
    Ok(None)
}
fn zero_constant_for_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(int_ty) = IntType::<IntDyn, B>::try_from(ty) {
        return Ok(Some(int_ty.const_zero().as_constant()));
    }
    if let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(ty) {
        let zero = ApFloat::zero(float_ty.semantics(), ApFloatSign::Positive);
        return Ok(Some(float_ty.const_ap_float(&zero)?.as_constant()));
    }
    if matches!(ty.data(), TypeData::Pointer { .. }) {
        let module = ty.module();
        let id = module.context().intern_constant_null(ty.id());
        return Ok(Some(Constant::from_parts(Value::from_parts(
            id,
            module,
            ty.id(),
        ))));
    }
    Ok(None)
}

fn all_ones_constant_for_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(int_ty) = IntType::<IntDyn, B>::try_from(ty) {
        return Ok(Some(int_ty.const_all_ones().as_constant()));
    }
    if let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(ty) {
        let bits = ApInt::all_ones(float_ty.semantics().bit_width());
        let value = ApFloat::from_bits(float_ty.semantics(), &bits)?;
        return Ok(Some(float_ty.const_ap_float(&value)?.as_constant()));
    }
    Ok(None)
}

fn folded_constants_from_ids<'ctx, B>(
    module: ModuleView<'ctx, B>,
    ids: impl IntoIterator<Item = ValueId>,
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Option<Vec<Constant<'ctx, B>>>>
where
    B: ModuleBrand + 'ctx,
{
    let mut folded = Vec::new();
    for id in ids {
        let Some(constant) = constant_from_id(module, id) else {
            return Ok(None);
        };
        folded.push(constant_fold_constant(constant, dl, tli)?);
    }
    Ok(Some(folded))
}

fn constant_fold_constant_expr_operands<'ctx, B: ModuleBrand + 'ctx>(
    original: Constant<'ctx, B>,
    expr: &ConstantExprData,
    operands: &[Constant<'ctx, B>],
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let _ = tli;
    match expr.opcode {
        ConstantExprOpcode::Add | ConstantExprOpcode::Sub | ConstantExprOpcode::Xor => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            let Some(opcode) = binary_opcode_from_constant_expr(expr.opcode) else {
                return Ok(None);
            };
            constant_fold_binary_op_operands(opcode, *lhs, *rhs, dl)
        }
        ConstantExprOpcode::GetElementPtr => {
            let Some(source_ty) = expr.source_ty else {
                return Ok(None);
            };
            let Some((pointer, indices)) = operands.split_first() else {
                return Ok(None);
            };
            let in_range = match &expr.flags {
                ConstantExprFlags::Gep(flags) => flags.in_range(),
                _ => None,
            };
            constant_fold_get_element_ptr(
                Type::new(source_ty, original.as_value().module()),
                *pointer,
                indices,
                in_range,
            )
        }
        ConstantExprOpcode::ShuffleVector => {
            let [lhs, rhs, mask] = operands else {
                return Ok(None);
            };
            let Some(mask) = shufflevector_mask_from_constant(*mask) else {
                return Ok(None);
            };
            constant_fold_shuffle_vector_instruction(*lhs, *rhs, &mask)
        }
        ConstantExprOpcode::InsertElement => {
            let [vector, value, index] = operands else {
                return Ok(None);
            };
            constant_fold_insert_element_instruction(*vector, *value, *index)
        }
        ConstantExprOpcode::ExtractElement => {
            let [vector, index] = operands else {
                return Ok(None);
            };
            constant_fold_extract_element_instruction(*vector, *index)
        }
        ConstantExprOpcode::Trunc
        | ConstantExprOpcode::PtrToAddr
        | ConstantExprOpcode::PtrToInt
        | ConstantExprOpcode::IntToPtr
        | ConstantExprOpcode::BitCast
        | ConstantExprOpcode::AddrSpaceCast => {
            let [operand] = operands else {
                return Ok(None);
            };
            let Some(opcode) = cast_opcode_from_constant_expr(expr.opcode) else {
                return Ok(None);
            };
            constant_fold_cast_operand(
                opcode,
                *operand,
                Type::new(expr.result_ty, original.as_value().module()),
                dl,
            )
        }
    }
}

fn binary_opcode(kind: &InstructionKindData) -> Option<BinaryOpcode> {
    let opcode = match kind {
        InstructionKindData::Add(_) => BinaryOpcode::Add,
        InstructionKindData::Sub(_) => BinaryOpcode::Sub,
        InstructionKindData::Mul(_) => BinaryOpcode::Mul,
        InstructionKindData::UDiv(_) => BinaryOpcode::UDiv,
        InstructionKindData::SDiv(_) => BinaryOpcode::SDiv,
        InstructionKindData::URem(_) => BinaryOpcode::URem,
        InstructionKindData::SRem(_) => BinaryOpcode::SRem,
        InstructionKindData::Shl(_) => BinaryOpcode::Shl,
        InstructionKindData::LShr(_) => BinaryOpcode::LShr,
        InstructionKindData::AShr(_) => BinaryOpcode::AShr,
        InstructionKindData::And(_) => BinaryOpcode::And,
        InstructionKindData::Or(_) => BinaryOpcode::Or,
        InstructionKindData::Xor(_) => BinaryOpcode::Xor,
        InstructionKindData::FAdd(_) => BinaryOpcode::FAdd,
        InstructionKindData::FSub(_) => BinaryOpcode::FSub,
        InstructionKindData::FMul(_) => BinaryOpcode::FMul,
        InstructionKindData::FDiv(_) => BinaryOpcode::FDiv,
        InstructionKindData::FRem(_) => BinaryOpcode::FRem,
        _ => return None,
    };
    Some(opcode)
}

fn binary_opcode_from_constant_expr(opcode: ConstantExprOpcode) -> Option<BinaryOpcode> {
    match opcode {
        ConstantExprOpcode::Add => Some(BinaryOpcode::Add),
        ConstantExprOpcode::Sub => Some(BinaryOpcode::Sub),
        ConstantExprOpcode::Xor => Some(BinaryOpcode::Xor),
        _ => None,
    }
}

fn binary_constant_expr_opcode(opcode: BinaryOpcode) -> Option<ConstantExprOpcode> {
    match opcode {
        BinaryOpcode::Add => Some(ConstantExprOpcode::Add),
        BinaryOpcode::Sub => Some(ConstantExprOpcode::Sub),
        BinaryOpcode::Xor => Some(ConstantExprOpcode::Xor),
        BinaryOpcode::Mul
        | BinaryOpcode::UDiv
        | BinaryOpcode::SDiv
        | BinaryOpcode::URem
        | BinaryOpcode::SRem
        | BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => None,
    }
}

fn cast_opcode_from_constant_expr(opcode: ConstantExprOpcode) -> Option<CastOpcode> {
    match opcode {
        ConstantExprOpcode::Trunc => Some(CastOpcode::Trunc),
        ConstantExprOpcode::PtrToAddr => Some(CastOpcode::PtrToAddr),
        ConstantExprOpcode::PtrToInt => Some(CastOpcode::PtrToInt),
        ConstantExprOpcode::IntToPtr => Some(CastOpcode::IntToPtr),
        ConstantExprOpcode::BitCast => Some(CastOpcode::BitCast),
        ConstantExprOpcode::AddrSpaceCast => Some(CastOpcode::AddrSpaceCast),
        _ => None,
    }
}

fn load_through_bitcast_opcode<B: ModuleBrand>(
    src_ty: Type<'_, B>,
    dest_ty: Type<'_, B>,
) -> CastOpcode {
    if matches!(src_ty.data(), TypeData::Integer { .. }) && pointer_address_space(dest_ty).is_some()
    {
        CastOpcode::IntToPtr
    } else if pointer_address_space(src_ty).is_some()
        && matches!(dest_ty.data(), TypeData::Integer { .. })
    {
        CastOpcode::PtrToInt
    } else {
        CastOpcode::BitCast
    }
}

fn aggregate_first_element<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let ValueKindData::Constant(ConstantData::Aggregate(elements)) =
        &constant.as_value().data().kind
    else {
        return Ok(None);
    };
    let module = constant.as_value().module();
    match constant.ty().data() {
        TypeData::Struct(_) => {
            for id in elements.iter().copied() {
                let Some(element) = constant_from_id(module, id) else {
                    return Ok(None);
                };
                if dl.type_size_in_bits(erase_type(element.ty())) != 0 {
                    return Ok(Some(element));
                }
            }
            Ok(None)
        }
        TypeData::Array { .. } => {
            let Some(id) = elements.first().copied() else {
                return Ok(None);
            };
            Ok(constant_from_id(module, id))
        }
        TypeData::FixedVector { elem, .. } => {
            let elem_ty = Type::new(*elem, module);
            if !dl.type_size_equals_store_size(erase_type(elem_ty)) {
                return Ok(None);
            }
            let Some(id) = elements.first().copied() else {
                return Ok(None);
            };
            Ok(constant_from_id(module, id))
        }
        _ => Ok(None),
    }
}

fn constant_is_nan<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    ConstantFloatValue::<FloatDyn, B>::try_from(constant)
        .is_ok_and(|value| value.ap_float().is_nan())
}

fn constant_to_store_bytes<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Vec<u8>>> {
    match &constant.as_value().data().kind {
        ValueKindData::Constant(ConstantData::Int(words)) => {
            let Some(bits) = constant.ty().data().as_integer() else {
                return Ok(None);
            };
            let byte_len = usize_from_u64(dl.type_store_size(erase_type(constant.ty())))?;
            Ok(Some(apint_to_store_bytes(
                &ApInt::from_words(bits, words),
                byte_len,
                dl,
            )))
        }
        ValueKindData::Constant(ConstantData::Float(_)) => {
            let Ok(fp) = ConstantFloatValue::<FloatDyn, B>::try_from(constant) else {
                return Ok(None);
            };
            let byte_len = usize_from_u64(dl.type_store_size(erase_type(constant.ty())))?;
            Ok(Some(apint_to_store_bytes(
                &fp.ap_float().to_bits(),
                byte_len,
                dl,
            )))
        }
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            aggregate_to_store_bytes(constant.ty(), elements, constant.as_value().module(), dl)
        }
        ValueKindData::Constant(ConstantData::PointerNull) => {
            let Some(addr_space) = pointer_address_space(constant.ty()) else {
                return Ok(None);
            };
            let bits = dl.pointer_size_in_bits(addr_space);
            let byte_len = usize_from_u64(dl.type_store_size(erase_type(constant.ty())))?;
            Ok(Some(apint_to_store_bytes(&ApInt::zero(bits), byte_len, dl)))
        }
        ValueKindData::Constant(ConstantData::Poison)
        | ValueKindData::Constant(ConstantData::Undef) => Ok(None),
        _ => Ok(None),
    }
}

fn aggregate_to_store_bytes<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    elements: &[ValueId],
    module: ModuleView<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Vec<u8>>> {
    match ty.data() {
        TypeData::Array { elem, .. } => {
            let elem_ty = Type::new(*elem, module);
            let elem_alloc = usize_from_u64(dl.type_alloc_size(erase_type(elem_ty)))?;
            let mut out = Vec::new();
            for id in elements.iter().copied() {
                let data = module.context().value_data(id);
                let element = Constant::from_parts(Value::from_parts(id, module, data.ty));
                let Some(mut bytes) = constant_to_store_bytes(element, dl)? else {
                    return Ok(None);
                };
                bytes.resize(elem_alloc, 0);
                out.extend(bytes);
            }
            Ok(Some(out))
        }
        TypeData::FixedVector { elem, .. } => {
            let elem_ty = Type::new(*elem, module);
            let elem_store = usize_from_u64(dl.type_store_size(erase_type(elem_ty)))?;
            let mut out = Vec::new();
            for id in elements.iter().copied() {
                let data = module.context().value_data(id);
                let element = Constant::from_parts(Value::from_parts(id, module, data.ty));
                let Some(mut bytes) = constant_to_store_bytes(element, dl)? else {
                    return Ok(None);
                };
                bytes.resize(elem_store, 0);
                out.extend(bytes);
            }
            Ok(Some(out))
        }
        TypeData::Struct(_) => {
            let total = usize_from_u64(dl.type_alloc_size(erase_type(ty)))?;
            let mut out = vec![0; total];
            let layout = dl.struct_layout(erase_type(ty));
            for (index, id) in elements.iter().copied().enumerate() {
                let data = module.context().value_data(id);
                let element = Constant::from_parts(Value::from_parts(id, module, data.ty));
                let Some(bytes) = constant_to_store_bytes(element, dl)? else {
                    return Ok(None);
                };
                let start = usize_from_u64(layout.element_offset(index))?;
                let Some(end) = start.checked_add(bytes.len()) else {
                    return Ok(None);
                };
                if end > out.len() {
                    return Ok(None);
                }
                out[start..end].copy_from_slice(&bytes);
            }
            Ok(Some(out))
        }
        _ => Ok(None),
    }
}

fn constant_from_store_bytes<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    bytes: &[u8],
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(int_ty) = IntType::<IntDyn, B>::try_from(ty) {
        let ap = bytes_to_apint(int_ty.bit_width(), bytes, dl);
        return Ok(Some(int_ty.const_ap_int(&ap)?.as_constant()));
    }
    if let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(ty) {
        let ap = bytes_to_apint(float_ty.semantics().bit_width(), bytes, dl);
        let fp = ApFloat::from_bits(float_ty.semantics(), &ap)?;
        return Ok(Some(float_ty.const_ap_float(&fp)?.as_constant()));
    }
    Ok(None)
}

fn fold_ptr_to_int_pair<'ctx, B: ModuleBrand + 'ctx>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &operand.as_value().data().kind else {
        return Ok(None);
    };
    let [src] = expr.operands.as_ref() else {
        return Ok(None);
    };
    if expr.opcode != ConstantExprOpcode::IntToPtr {
        return Ok(None);
    }
    let module = operand.as_value().module();
    let source = Constant::from_parts(Value::from_parts(
        *src,
        module,
        module.context().value_data(*src).ty,
    ));
    let Some(addr_space) = pointer_address_space(operand.ty()) else {
        return Ok(None);
    };
    let mid_bits = match opcode {
        CastOpcode::PtrToInt => dl.index_size_in_bits(addr_space),
        CastOpcode::PtrToAddr => dl.pointer_size_in_bits(addr_space),
        _ => return Ok(None),
    };
    let mid_ty = int_type(module, mid_bits)?;
    let Some(mid) = fold_integer_cast_constant(source, mid_ty.as_type(), false, dl)? else {
        return Ok(None);
    };
    fold_integer_cast_constant(mid, dest_ty, false, dl)
}

fn fold_int_to_ptr_pair<'ctx, B: ModuleBrand + 'ctx>(
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &operand.as_value().data().kind else {
        return Ok(None);
    };
    if expr.opcode != ConstantExprOpcode::PtrToInt {
        return Ok(None);
    }
    let [src] = expr.operands.as_ref() else {
        return Ok(None);
    };
    let module = operand.as_value().module();
    let source = Constant::from_parts(Value::from_parts(
        *src,
        module,
        module.context().value_data(*src).ty,
    ));
    let Some(src_as) = pointer_address_space(source.ty()) else {
        return Ok(None);
    };
    let Some(dst_as) = pointer_address_space(dest_ty) else {
        return Ok(None);
    };
    let Ok(mid_ty) = IntType::<IntDyn, B>::try_from(operand.ty()) else {
        return Ok(None);
    };
    if mid_ty.bit_width() < dl.pointer_size_in_bits(src_as) || src_as != dst_as {
        return Ok(None);
    }
    fold_bitcast_with_layout(source, dest_ty, dl)
}

fn fold_integer_cast_constant<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    is_signed: bool,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Ok(src_ty) = IntType::<IntDyn, B>::try_from(constant.ty()) else {
        return Ok(None);
    };
    let Ok(dst_ty) = IntType::<IntDyn, B>::try_from(dest_ty) else {
        return Ok(None);
    };
    if src_ty.bit_width() == dst_ty.bit_width() {
        return Ok(Some(constant));
    }
    let opcode = if src_ty.bit_width() > dst_ty.bit_width() {
        CastOpcode::Trunc
    } else if is_signed {
        CastOpcode::SExt
    } else {
        CastOpcode::ZExt
    };
    constant_fold_cast_operand(opcode, constant, dest_ty, dl)
}

fn fold_bitcast_with_layout<'ctx, B: ModuleBrand + 'ctx>(
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Some(folded) = constant_fold_cast_instruction(CastOpcode::BitCast, operand, dest_ty)? {
        return Ok(Some(folded));
    }
    if dl.type_store_size(erase_type(operand.ty())) != dl.type_store_size(erase_type(dest_ty)) {
        return Ok(None);
    }
    let Some(bytes) = constant_to_store_bytes(operand, dl)? else {
        return Ok(None);
    };
    constant_from_store_bytes(dest_ty, &bytes, dl)
}

fn cast_constant_expr_opcode(opcode: CastOpcode) -> Option<ConstantExprOpcode> {
    match opcode {
        CastOpcode::Trunc => Some(ConstantExprOpcode::Trunc),
        CastOpcode::PtrToAddr => Some(ConstantExprOpcode::PtrToAddr),
        CastOpcode::PtrToInt => Some(ConstantExprOpcode::PtrToInt),
        CastOpcode::IntToPtr => Some(ConstantExprOpcode::IntToPtr),
        CastOpcode::BitCast => Some(ConstantExprOpcode::BitCast),
        CastOpcode::AddrSpaceCast => Some(ConstantExprOpcode::AddrSpaceCast),
        CastOpcode::ZExt
        | CastOpcode::SExt
        | CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::FpToUI
        | CastOpcode::FpToSI
        | CastOpcode::UIToFp
        | CastOpcode::SIToFp => None,
    }
}

fn constant_from_id<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    id: ValueId,
) -> Option<Constant<'ctx, B>> {
    let data = module.context().value_data(id);
    matches!(&data.kind, ValueKindData::Constant(_))
        .then(|| Constant::from_parts(Value::from_parts(id, module, data.ty)))
}

fn int_type<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    bits: u32,
) -> IrResult<IntType<'ctx, IntDyn, B>> {
    if !(MIN_INT_BITS..=MAX_INT_BITS).contains(&bits) {
        return Err(IrError::InvalidIntegerWidth { bits });
    }
    Ok(IntType::new(
        module.context().int_type(bits),
        ModuleRef::<B>::new(module.core_ref()),
    ))
}

fn pointer_address_space<B: ModuleBrand>(ty: Type<'_, B>) -> Option<u32> {
    match ty.data() {
        TypeData::Pointer { addr_space } | TypeData::TypedPointer { addr_space, .. } => {
            Some(*addr_space)
        }
        _ => None,
    }
}

fn index_bits_for_pointer<B: ModuleBrand>(ty: Type<'_, B>, dl: &DataLayout) -> Option<u32> {
    pointer_address_space(ty).map(|addr_space| dl.index_size_in_bits(addr_space))
}

fn index_bits_for_constant_offset<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
    dl: &DataLayout,
) -> Option<u32> {
    if let Some(bits) = index_bits_for_pointer(constant.ty(), dl) {
        return Some(bits);
    }
    let module = constant.as_value().module();
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &constant.as_value().data().kind else {
        return None;
    };
    match expr.opcode {
        ConstantExprOpcode::PtrToInt
        | ConstantExprOpcode::PtrToAddr
        | ConstantExprOpcode::BitCast
        | ConstantExprOpcode::AddrSpaceCast => {
            let [operand_id] = expr.operands.as_ref() else {
                return None;
            };
            let operand = constant_from_id(module, *operand_id)?;
            index_bits_for_constant_offset(operand, dl)
        }
        ConstantExprOpcode::GetElementPtr => {
            let (base_id, _) = expr.operands.split_first()?;
            let base = constant_from_id(module, *base_id)?;
            index_bits_for_constant_offset(base, dl)
        }
        _ => None,
    }
}

fn is_non_integral_pointer_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    dl: &DataLayout,
) -> bool {
    match ty.data() {
        TypeData::Pointer { addr_space } | TypeData::TypedPointer { addr_space, .. } => {
            dl.is_non_integral_address_space(*addr_space)
        }
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            is_non_integral_pointer_type(Type::new(*elem, ty.module()), dl)
        }
        _ => false,
    }
}

fn denormal_mode_for_instruction<'ctx, B: ModuleBrand + 'ctx>(
    parent_id: ValueId,
    module: ModuleView<'ctx, B>,
    ty: Type<'ctx, B>,
) -> DenormalMode {
    let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(ty) else {
        return DenormalMode::dynamic();
    };
    let parent = module.context().value_data(parent_id);
    let ValueKindData::BasicBlock(block) = &parent.kind else {
        return DenormalMode::dynamic();
    };
    let Some(function_id) = *block.parent.borrow() else {
        return DenormalMode::dynamic();
    };
    let ValueKindData::Function(_) = &module.context().value_data(function_id).kind else {
        return DenormalMode::dynamic();
    };
    let function = FunctionValue::<Dyn, B>::from_parts_unchecked(function_id, module);
    function.denormal_mode(float_ty.semantics())
}

fn apint_to_store_bytes(value: &ApInt, byte_len: usize, dl: &DataLayout) -> Vec<u8> {
    let mut out = vec![0; byte_len];
    for (word_index, word) in value.words().iter().copied().enumerate() {
        for byte_index in 0..8_usize {
            let out_index = word_index.saturating_mul(8).saturating_add(byte_index);
            if out_index >= out.len() {
                break;
            }
            let shift = byte_index.saturating_mul(8);
            let masked = (word >> shift) & 0xff;
            let Ok(byte) = u8::try_from(masked) else {
                continue;
            };
            out[out_index] = byte;
        }
    }
    if dl.is_big_endian() {
        out.reverse();
    }
    out
}

fn bytes_to_apint(bit_width: u32, bytes: &[u8], dl: &DataLayout) -> ApInt {
    let ordered: Vec<u8> = if dl.is_little_endian() {
        bytes.to_vec()
    } else {
        bytes.iter().rev().copied().collect()
    };
    let mut words = vec![0; ordered.len().saturating_add(7) / 8];
    for (index, byte) in ordered.iter().copied().enumerate() {
        let word_index = index / 8;
        let shift = (index % 8).saturating_mul(8);
        if let Some(word) = words.get_mut(word_index) {
            *word |= u64::from(byte) << shift;
        }
    }
    ApInt::from_words(bit_width, &words)
}

fn usize_from_u64(value: u64) -> IrResult<usize> {
    usize::try_from(value).map_err(|_| IrError::InvalidOperation {
        message: "constant fold size does not fit usize",
    })
}

fn is_undef<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    matches!(
        &constant.as_value().data().kind,
        ValueKindData::Constant(ConstantData::Undef)
    )
}

fn is_poison<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    matches!(
        &constant.as_value().data().kind,
        ValueKindData::Constant(ConstantData::Poison)
    )
}

fn is_guaranteed_not_to_be_undef_or_poison<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> bool {
    let module = constant.as_value().module();
    match &constant.as_value().data().kind {
        ValueKindData::Constant(ConstantData::Undef | ConstantData::Poison) => false,
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => elements
            .iter()
            .copied()
            .all(|id| constant_id_guaranteed_not_to_be_undef_or_poison(module, id)),
        ValueKindData::Constant(ConstantData::PtrAuth {
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        }) => [
            *pointer,
            *key,
            *discriminator,
            *addr_discriminator,
            *deactivation_symbol,
        ]
        .iter()
        .copied()
        .all(|id| constant_id_guaranteed_not_to_be_undef_or_poison(module, id)),
        ValueKindData::Constant(ConstantData::Expr(_) | ConstantData::BlockAddressPlaceholder) => {
            false
        }
        ValueKindData::Constant(
            ConstantData::Int(_)
            | ConstantData::Float(_)
            | ConstantData::GlobalValueRef { .. }
            | ConstantData::PointerNull
            | ConstantData::GepOffset { .. }
            | ConstantData::SymbolDelta { .. }
            | ConstantData::SymbolDeltaPlus { .. }
            | ConstantData::BlockAddress { .. }
            | ConstantData::DSOLocalEquivalent { .. }
            | ConstantData::NoCfi { .. }
            | ConstantData::TokenNone
            | ConstantData::TargetExtNone,
        ) => true,
        _ => false,
    }
}

fn constant_id_guaranteed_not_to_be_undef_or_poison<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    id: ValueId,
) -> bool {
    let data = module.context().value_data(id);
    let ValueKindData::Constant(_) = &data.kind else {
        return false;
    };
    is_guaranteed_not_to_be_undef_or_poison(Constant::from_parts(Value::from_parts(
        id, module, data.ty,
    )))
}

fn erase_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Type<'ctx> {
    Type::new(ty.id(), ModuleRef::new(ty.module().core_ref()))
}

fn erase_value<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Value<'ctx> {
    Value::from_parts(
        value.id,
        ModuleView::new(value.module().core_ref()),
        value.ty().id(),
    )
}

fn rebrand_constant<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx>,
    module: ModuleView<'ctx, B>,
) -> Constant<'ctx, B> {
    Constant::from_parts(Value::from_parts(
        constant.as_value().id,
        module,
        constant.ty().id(),
    ))
}
