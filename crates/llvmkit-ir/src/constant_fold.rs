//! Target-independent constant folding.
//!
//! Mirrors the pure-constant portions of `llvm/lib/IR/ConstantFold.cpp`.

use super::ap_float::{ApFloatCmpResult, ApFloatSemantics, ApFloatSign, NanPayload};
use super::ap_int::{ApInt, ApIntDivRem, ApIntSignedness};
use super::cmp_predicate::{CmpPredicate, FloatPredicate, IntPredicate};
use super::constant::{Constant, ConstantData};
use super::constants::{ConstantFloatValue, ConstantIntValue};
use super::derived_types::{ArrayType, FloatType, IntType, StructType, VectorType};
use super::float_kind::FloatDyn;
use super::instr_types::{BinaryOpcode, CastOpcode, POISON_MASK_ELEM, UnaryOpcode};
use super::instruction::state::InstructionState;
use super::instruction::{Instruction, InstructionKindData};
use super::int_width::IntDyn;
use super::module::{ModuleBrand, ModuleView};
use super::r#type::Type;
use super::value::{Value, ValueId, ValueKindData};
use super::{IrResult, RoundingMode};

/// Fold an instruction whose operands are constants.
///
/// Mirrors the target-independent dispatch layer of
/// `llvm/Analysis/ConstantFolding.h::ConstantFoldInstruction`; callers get
/// `Ok(None)` when any required operand is non-constant or the opcode has no
/// target-independent fold.
pub fn constant_fold_instruction<'ctx, S, B>(
    instruction: &Instruction<'ctx, S, B>,
) -> IrResult<Option<Constant<'ctx, B>>>
where
    S: InstructionState,
    B: ModuleBrand + 'ctx,
{
    let value = instruction.as_value();
    let module = value.module();
    let ValueKindData::Instruction(data) = &value.data().kind else {
        return Ok(None);
    };

    if let Some((opcode, lhs, rhs, is_exact)) = binary_instruction_parts(&data.kind) {
        let Some(lhs) = constant_from_id(module, lhs) else {
            return Ok(None);
        };
        let Some(rhs) = constant_from_id(module, rhs) else {
            return Ok(None);
        };
        return constant_fold_exact_binary_instruction(opcode, lhs, rhs, is_exact);
    }

    match &data.kind {
        InstructionKindData::Cast(cast) => {
            let Some(src) = constant_from_id(module, cast.src.get()) else {
                return Ok(None);
            };
            constant_fold_cast_instruction(cast.kind, src, value.ty())
        }
        InstructionKindData::ICmp(cmp) => {
            let Some(lhs) = constant_from_id(module, cmp.lhs.get()) else {
                return Ok(None);
            };
            let Some(rhs) = constant_from_id(module, cmp.rhs.get()) else {
                return Ok(None);
            };
            constant_fold_compare_instruction(CmpPredicate::Int(cmp.predicate), lhs, rhs)
        }
        InstructionKindData::FCmp(cmp) => {
            let Some(lhs) = constant_from_id(module, cmp.lhs.get()) else {
                return Ok(None);
            };
            let Some(rhs) = constant_from_id(module, cmp.rhs.get()) else {
                return Ok(None);
            };
            constant_fold_compare_instruction(CmpPredicate::Float(cmp.predicate), lhs, rhs)
        }
        InstructionKindData::FNeg(fneg) => {
            let Some(src) = constant_from_id(module, fneg.src.get()) else {
                return Ok(None);
            };
            constant_fold_unary_instruction(UnaryOpcode::FNeg, src)
        }
        InstructionKindData::Select(select) => {
            let Some(cond) = constant_from_id(module, select.cond.get()) else {
                return Ok(None);
            };
            let Some(true_value) = constant_from_id(module, select.true_val.get()) else {
                return Ok(None);
            };
            let Some(false_value) = constant_from_id(module, select.false_val.get()) else {
                return Ok(None);
            };
            constant_fold_select_instruction(cond, true_value, false_value)
        }
        InstructionKindData::ExtractElement(extract) => {
            let Some(vector) = constant_from_id(module, extract.vector.get()) else {
                return Ok(None);
            };
            let Some(index) = constant_from_id(module, extract.index.get()) else {
                return Ok(None);
            };
            constant_fold_extract_element_instruction(vector, index)
        }
        InstructionKindData::InsertElement(insert) => {
            let Some(vector) = constant_from_id(module, insert.vector.get()) else {
                return Ok(None);
            };
            let Some(element) = constant_from_id(module, insert.value.get()) else {
                return Ok(None);
            };
            let Some(index) = constant_from_id(module, insert.index.get()) else {
                return Ok(None);
            };
            constant_fold_insert_element_instruction(vector, element, index)
        }
        InstructionKindData::ShuffleVector(shuffle) => {
            let Some(lhs) = constant_from_id(module, shuffle.lhs.get()) else {
                return Ok(None);
            };
            let Some(rhs) = constant_from_id(module, shuffle.rhs.get()) else {
                return Ok(None);
            };
            constant_fold_shuffle_vector_instruction(lhs, rhs, &shuffle.mask)
        }
        InstructionKindData::ExtractValue(extract) => {
            let Some(aggregate) = constant_from_id(module, extract.aggregate.get()) else {
                return Ok(None);
            };
            constant_fold_extract_value_instruction(aggregate, &extract.indices)
        }
        InstructionKindData::InsertValue(insert) => {
            let Some(aggregate) = constant_from_id(module, insert.aggregate.get()) else {
                return Ok(None);
            };
            let Some(inserted) = constant_from_id(module, insert.value.get()) else {
                return Ok(None);
            };
            constant_fold_insert_value_instruction(aggregate, inserted, &insert.indices)
        }
        InstructionKindData::Gep(gep) => {
            let Some(pointer) = constant_from_id(module, gep.ptr.get()) else {
                return Ok(None);
            };
            let Some(indices) = constants_from_ids(module, gep.indices.iter().map(|id| id.get()))
            else {
                return Ok(None);
            };
            constant_fold_get_element_ptr(Type::new(gep.source_ty, module), pointer, &indices)
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
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Call(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::Freeze(_)
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
        | InstructionKindData::Ret(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::Unreachable(_) => Ok(None),
    }
}

fn binary_instruction_parts(
    kind: &InstructionKindData,
) -> Option<(BinaryOpcode, ValueId, ValueId, bool)> {
    let (opcode, data) = match kind {
        InstructionKindData::Add(data) => (BinaryOpcode::Add, data),
        InstructionKindData::Sub(data) => (BinaryOpcode::Sub, data),
        InstructionKindData::Mul(data) => (BinaryOpcode::Mul, data),
        InstructionKindData::UDiv(data) => (BinaryOpcode::UDiv, data),
        InstructionKindData::SDiv(data) => (BinaryOpcode::SDiv, data),
        InstructionKindData::URem(data) => (BinaryOpcode::URem, data),
        InstructionKindData::SRem(data) => (BinaryOpcode::SRem, data),
        InstructionKindData::Shl(data) => (BinaryOpcode::Shl, data),
        InstructionKindData::LShr(data) => (BinaryOpcode::LShr, data),
        InstructionKindData::AShr(data) => (BinaryOpcode::AShr, data),
        InstructionKindData::And(data) => (BinaryOpcode::And, data),
        InstructionKindData::Or(data) => (BinaryOpcode::Or, data),
        InstructionKindData::Xor(data) => (BinaryOpcode::Xor, data),
        InstructionKindData::FAdd(data) => (BinaryOpcode::FAdd, data),
        InstructionKindData::FSub(data) => (BinaryOpcode::FSub, data),
        InstructionKindData::FMul(data) => (BinaryOpcode::FMul, data),
        InstructionKindData::FDiv(data) => (BinaryOpcode::FDiv, data),
        InstructionKindData::FRem(data) => (BinaryOpcode::FRem, data),
        InstructionKindData::FCmp(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Gep(_)
        | InstructionKindData::Call(_)
        | InstructionKindData::Select(_)
        | InstructionKindData::Cast(_)
        | InstructionKindData::ICmp(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::FNeg(_)
        | InstructionKindData::Freeze(_)
        | InstructionKindData::VAArg(_)
        | InstructionKindData::ExtractValue(_)
        | InstructionKindData::InsertValue(_)
        | InstructionKindData::ExtractElement(_)
        | InstructionKindData::InsertElement(_)
        | InstructionKindData::ShuffleVector(_)
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
        | InstructionKindData::Ret(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::Unreachable(_) => return None,
    };
    Some((opcode, data.lhs.get(), data.rhs.get(), data.is_exact))
}

fn constant_from_id<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    id: ValueId,
) -> Option<Constant<'ctx, B>> {
    let data = module.context().value_data(id);
    match &data.kind {
        ValueKindData::Constant(_) => {
            Some(Constant::from_parts(Value::from_parts(id, module, data.ty)))
        }
        ValueKindData::Argument { .. }
        | ValueKindData::BasicBlock(_)
        | ValueKindData::Function(_)
        | ValueKindData::Instruction(_)
        | ValueKindData::GlobalAlias(_)
        | ValueKindData::GlobalIFunc(_)
        | ValueKindData::GlobalVariable(_)
        | ValueKindData::MetadataAsValue(_)
        | ValueKindData::InlineAsm(_) => None,
    }
}

fn constants_from_ids<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    ids: impl IntoIterator<Item = ValueId>,
) -> Option<Vec<Constant<'ctx, B>>> {
    let mut constants = Vec::new();
    for id in ids {
        constants.push(constant_from_id(module, id)?);
    }
    Some(constants)
}

/// Fold a unary instruction with constant operands.
pub fn constant_fold_unary_instruction<'ctx, B: ModuleBrand + 'ctx>(
    opcode: UnaryOpcode,
    operand: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if is_poison(operand) {
        return Ok(Some(poison_for(operand.ty())));
    }

    match opcode {
        UnaryOpcode::FNeg => {
            let Ok(value) = ConstantFloatValue::<FloatDyn, B>::try_from(operand) else {
                return Ok(None);
            };
            let negated = value.ap_float().change_sign();
            Ok(Some(value.ty().const_ap_float(&negated)?.as_constant()))
        }
    }
}

/// Fold a binary instruction with constant operands.
pub fn constant_fold_binary_instruction<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if lhs.ty() != rhs.ty() {
        return Ok(None);
    }

    if is_poison(lhs) || is_poison(rhs) {
        return Ok(Some(poison_for(lhs.ty())));
    }

    if let Some(folded) = fold_undef_int_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }
    if let Some(folded) = fold_undef_float_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }

    if let Some(folded) = fold_int_identity_binary(opcode, lhs, rhs) {
        return Ok(Some(folded));
    }

    match opcode {
        BinaryOpcode::Add
        | BinaryOpcode::Sub
        | BinaryOpcode::Mul
        | BinaryOpcode::UDiv
        | BinaryOpcode::SDiv
        | BinaryOpcode::URem
        | BinaryOpcode::SRem
        | BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::Xor => fold_int_binary(opcode, lhs, rhs),
        BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => fold_float_binary(opcode, lhs, rhs),
    }
}

/// Fold an exact-capable binary instruction with constant operands.
pub(super) fn constant_fold_exact_binary_instruction<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
    is_exact: bool,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if !is_exact || !is_exact_capable_binop(opcode) {
        return constant_fold_binary_instruction(opcode, lhs, rhs);
    }

    if lhs.ty() != rhs.ty() {
        return Ok(None);
    }
    if is_poison(lhs) || is_poison(rhs) {
        return Ok(Some(poison_for(lhs.ty())));
    }
    if let Some(folded) = fold_undef_int_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }

    fold_exact_int_binary(opcode, lhs, rhs)
}

/// Fold a cast instruction with a constant operand.
pub fn constant_fold_cast_instruction<'ctx, B: ModuleBrand + 'ctx>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if is_poison(operand) {
        return Ok(Some(poison_for(dest_ty)));
    }
    if is_undef(operand) {
        return match opcode {
            CastOpcode::ZExt | CastOpcode::SExt | CastOpcode::UIToFp | CastOpcode::SIToFp => {
                null_constant_for_type(dest_ty)
            }
            CastOpcode::Trunc
            | CastOpcode::FpTrunc
            | CastOpcode::FpExt
            | CastOpcode::FpToUI
            | CastOpcode::FpToSI
            | CastOpcode::BitCast
            | CastOpcode::PtrToAddr
            | CastOpcode::PtrToInt
            | CastOpcode::IntToPtr
            | CastOpcode::AddrSpaceCast => Ok(Some(dest_ty.get_undef().as_constant())),
        };
    }

    match opcode {
        CastOpcode::Trunc | CastOpcode::ZExt | CastOpcode::SExt => {
            let Ok(src) = ConstantIntValue::<IntDyn, B>::try_from(operand) else {
                return Ok(None);
            };
            let Ok(dst_ty) = IntType::<IntDyn, B>::try_from(dest_ty) else {
                return Ok(None);
            };
            let src_ap = src.ap_int();
            let dst_bits = dst_ty.bit_width();
            let result = match opcode {
                CastOpcode::Trunc => src_ap.trunc(dst_bits),
                CastOpcode::ZExt => src_ap.zext(dst_bits),
                CastOpcode::SExt => src_ap.sext(dst_bits),
                _ => None,
            };
            let Some(result) = result else {
                return Ok(None);
            };
            Ok(Some(dst_ty.const_ap_int(&result)?.as_constant()))
        }
        CastOpcode::FpTrunc | CastOpcode::FpExt => {
            let Ok(src) = ConstantFloatValue::<FloatDyn, B>::try_from(operand) else {
                return Ok(None);
            };
            let Ok(dst_ty) = FloatType::<FloatDyn, B>::try_from(dest_ty) else {
                return Ok(None);
            };
            let (result, _, _) = src
                .ap_float()
                .convert(dst_ty.semantics(), RoundingMode::NearestTiesToEven);
            Ok(Some(dst_ty.const_ap_float(&result)?.as_constant()))
        }
        CastOpcode::FpToUI | CastOpcode::FpToSI => {
            let Ok(src) = ConstantFloatValue::<FloatDyn, B>::try_from(operand) else {
                return Ok(None);
            };
            let Ok(dst_ty) = IntType::<IntDyn, B>::try_from(dest_ty) else {
                return Ok(None);
            };
            let signedness = match opcode {
                CastOpcode::FpToSI => ApIntSignedness::Signed,
                _ => ApIntSignedness::Unsigned,
            };
            let (result, status, _) = src.ap_float().convert_to_integer(
                dst_ty.bit_width(),
                signedness,
                RoundingMode::TowardZero,
            );
            if status.contains(crate::ApFloatStatus::INVALID_OP) {
                return Ok(Some(poison_for(dst_ty.as_type())));
            }
            Ok(Some(dst_ty.const_ap_int(&result)?.as_constant()))
        }
        CastOpcode::UIToFp | CastOpcode::SIToFp => {
            let Ok(src) = ConstantIntValue::<IntDyn, B>::try_from(operand) else {
                return Ok(None);
            };
            let Ok(dst_ty) = FloatType::<FloatDyn, B>::try_from(dest_ty) else {
                return Ok(None);
            };
            let signedness = match opcode {
                CastOpcode::SIToFp => ApIntSignedness::Signed,
                _ => ApIntSignedness::Unsigned,
            };
            let (result, _) = crate::ApFloat::convert_from_ap_int(
                dst_ty.semantics(),
                &src.ap_int(),
                signedness,
                RoundingMode::NearestTiesToEven,
            );
            Ok(Some(dst_ty.const_ap_float(&result)?.as_constant()))
        }
        CastOpcode::BitCast => fold_bitcast(operand, dest_ty),
        CastOpcode::PtrToAddr
        | CastOpcode::PtrToInt
        | CastOpcode::IntToPtr
        | CastOpcode::AddrSpaceCast => Ok(None),
    }
}

/// Fold an integer or floating-point compare instruction.
pub fn constant_fold_compare_instruction<'ctx, B: ModuleBrand + 'ctx>(
    predicate: CmpPredicate,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if lhs.ty() != rhs.ty() {
        return Ok(None);
    }

    let result_ty = compare_result_type(lhs.ty());
    if matches!(predicate, CmpPredicate::Float(FloatPredicate::False)) {
        return bool_constant_for_type(result_ty, false);
    }
    if matches!(predicate, CmpPredicate::Float(FloatPredicate::True)) {
        return bool_constant_for_type(result_ty, true);
    }
    if is_poison(lhs) || is_poison(rhs) {
        return Ok(Some(poison_for(result_ty)));
    }
    if is_undef(lhs) || is_undef(rhs) {
        return fold_undef_compare(predicate, result_ty, lhs, rhs);
    }

    if let Some((_, lanes, scalable)) = lhs.ty().data().as_vector() {
        if scalable {
            return Ok(None);
        }
        let Some(lhs_elements) = aggregate_elements(lhs) else {
            return Ok(None);
        };
        let Some(rhs_elements) = aggregate_elements(rhs) else {
            return Ok(None);
        };
        let Ok(lane_count) = usize::try_from(lanes) else {
            return Ok(None);
        };
        if lhs_elements.len() != lane_count || rhs_elements.len() != lane_count {
            return Ok(None);
        }
        let mut result = Vec::with_capacity(lane_count);
        for (lhs_element, rhs_element) in lhs_elements.into_iter().zip(rhs_elements) {
            let Some(folded) =
                constant_fold_compare_instruction(predicate, lhs_element, rhs_element)?
            else {
                return Ok(None);
            };
            result.push(folded);
        }
        return constant_aggregate_from_elements(result_ty, result);
    }

    match predicate {
        CmpPredicate::Int(pred) => {
            let Ok(lhs) = ConstantIntValue::<IntDyn, B>::try_from(lhs) else {
                return Ok(None);
            };
            let Ok(rhs) = ConstantIntValue::<IntDyn, B>::try_from(rhs) else {
                return Ok(None);
            };
            let result = fold_int_predicate(pred, &lhs.ap_int(), &rhs.ap_int());
            bool_constant_for_type(result_ty, result)
        }
        CmpPredicate::Float(pred) => {
            let Ok(lhs) = ConstantFloatValue::<FloatDyn, B>::try_from(lhs) else {
                return Ok(None);
            };
            let Ok(rhs) = ConstantFloatValue::<FloatDyn, B>::try_from(rhs) else {
                return Ok(None);
            };
            let result = fold_float_predicate(pred, lhs.ap_float().compare(&rhs.ap_float()));
            bool_constant_for_type(result_ty, result)
        }
    }
}

/// Fold a `select` with constant condition/arms.
pub fn constant_fold_select_instruction<'ctx, B: ModuleBrand + 'ctx>(
    condition: Constant<'ctx, B>,
    true_value: Constant<'ctx, B>,
    false_value: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if true_value.ty() != false_value.ty() {
        return Ok(None);
    }

    if let Some(condition) = bool_constant_value(condition) {
        return Ok(Some(if condition { true_value } else { false_value }));
    }

    if let Some((_, lanes, scalable)) = condition.ty().data().as_vector()
        && !scalable
        && let Some(condition_elements) = aggregate_elements(condition)
    {
        let Some((value_element_ty, value_lanes, value_scalable)) =
            true_value.ty().data().as_vector()
        else {
            return Ok(None);
        };
        if value_lanes != lanes || value_scalable {
            return Ok(None);
        }
        let Ok(lane_count) = usize::try_from(lanes) else {
            return Ok(None);
        };
        if condition_elements.len() != lane_count {
            return Ok(None);
        }
        let value_element_ty = Type::new(value_element_ty, true_value.as_value().module());
        let Some(true_elements) =
            fixed_vector_elements_for_rebuild(true_value, lanes, value_element_ty)?
        else {
            return Ok(None);
        };
        let Some(false_elements) =
            fixed_vector_elements_for_rebuild(false_value, lanes, value_element_ty)?
        else {
            return Ok(None);
        };
        let mut result = Vec::with_capacity(lane_count);
        for ((condition, true_element), false_element) in condition_elements
            .into_iter()
            .zip(true_elements)
            .zip(false_elements)
        {
            let selected = if is_poison(condition) {
                poison_for(value_element_ty)
            } else if true_element == false_element {
                true_element
            } else if is_undef(condition) {
                if is_undef(true_element) {
                    true_element
                } else {
                    false_element
                }
            } else {
                let Some(condition) = bool_constant_value(condition) else {
                    return Ok(None);
                };
                if condition {
                    true_element
                } else {
                    false_element
                }
            };
            result.push(selected);
        }
        return constant_aggregate_from_elements(true_value.ty(), result);
    }

    if is_poison(condition) {
        return Ok(Some(poison_for(true_value.ty())));
    }
    if is_undef(condition) {
        return Ok(Some(if is_undef(true_value) {
            true_value
        } else {
            false_value
        }));
    }
    if true_value == false_value {
        return Ok(Some(true_value));
    }
    if is_poison(true_value) {
        return Ok(Some(false_value));
    }
    if is_poison(false_value) {
        return Ok(Some(true_value));
    }
    if is_undef(true_value) && is_not_poison_for_select(false_value) {
        return Ok(Some(false_value));
    }
    if is_undef(false_value) && is_not_poison_for_select(true_value) {
        return Ok(Some(true_value));
    }
    Ok(None)
}

/// Fold an `extractelement` with constant vector and index operands.
pub fn constant_fold_extract_element_instruction<'ctx, B: ModuleBrand + 'ctx>(
    vector: Constant<'ctx, B>,
    index: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some((element_ty, lanes, scalable)) = vector.ty().data().as_vector() else {
        return Ok(None);
    };
    let element_ty = Type::new(element_ty, vector.as_value().module());
    if is_poison(vector) || is_undef(index) {
        return Ok(Some(poison_for(element_ty)));
    }
    if is_undef(vector) {
        return Ok(Some(element_ty.get_undef().as_constant()));
    }
    let Ok(index) = ConstantIntValue::<IntDyn, B>::try_from(index) else {
        return Ok(None);
    };
    let index_ap = index.ap_int();
    let raw_index = if scalable {
        let Some(raw_index) = index_ap.try_zext_u64() else {
            return Ok(None);
        };
        raw_index
    } else {
        let limit = u64::from(lanes);
        let raw_index = index_ap.limited_value(limit);
        if raw_index == limit {
            return Ok(Some(poison_for(element_ty)));
        }
        raw_index
    };
    let Ok(index) = usize::try_from(raw_index) else {
        return Ok(None);
    };
    let Some(elements) = aggregate_elements(vector) else {
        return Ok(None);
    };
    let Some(element) = elements.get(index).copied() else {
        return Ok(None);
    };
    Ok(Some(element))
}

/// Fold an `insertelement` with constant operands.
pub fn constant_fold_insert_element_instruction<'ctx, B: ModuleBrand + 'ctx>(
    vector: Constant<'ctx, B>,
    value: Constant<'ctx, B>,
    index: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some((element_ty, lanes, scalable)) = vector.ty().data().as_vector() else {
        return Ok(None);
    };
    if value.ty().id() != element_ty {
        return Ok(None);
    }
    if is_undef(index) || is_poison(index) {
        return Ok(Some(poison_for(vector.ty())));
    }
    if scalable {
        return Ok(None);
    }
    let Ok(index) = ConstantIntValue::<IntDyn, B>::try_from(index) else {
        return Ok(None);
    };
    let raw_index = index.ap_int().limited_value(u64::from(lanes));
    if raw_index == u64::from(lanes) {
        return Ok(Some(poison_for(vector.ty())));
    }
    let Ok(index) = usize::try_from(raw_index) else {
        return Ok(None);
    };
    let element_ty = Type::new(element_ty, vector.as_value().module());
    let Some(mut elements) = fixed_vector_elements_for_rebuild(vector, lanes, element_ty)? else {
        return Ok(None);
    };
    let Some(slot) = elements.get_mut(index) else {
        return Ok(None);
    };
    *slot = value;
    constant_aggregate_from_elements(vector.ty(), elements)
}

/// Fold a `shufflevector` with constant operands.
pub fn constant_fold_shuffle_vector_instruction<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
    mask: &[i32],
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some((element_ty, lanes, scalable)) = lhs.ty().data().as_vector() else {
        return Ok(None);
    };
    if rhs.ty() != lhs.ty() || scalable {
        return Ok(None);
    }
    let element_ty = Type::new(element_ty, lhs.as_value().module());
    let Ok(result_lanes) = u32::try_from(mask.len()) else {
        return Ok(None);
    };
    let result_ty = lhs
        .as_value()
        .module()
        .vector_type(element_ty, result_lanes, false)
        .as_type();
    if mask.iter().all(|element| *element == POISON_MASK_ELEM) {
        return Ok(Some(poison_for(result_ty)));
    }
    let Some(lhs_elements) = fixed_vector_elements_for_rebuild(lhs, lanes, element_ty)? else {
        return Ok(None);
    };
    let Some(rhs_elements) = fixed_vector_elements_for_rebuild(rhs, lanes, element_ty)? else {
        return Ok(None);
    };
    let mut result = Vec::with_capacity(mask.len());
    for &element in mask {
        if element == POISON_MASK_ELEM {
            result.push(element_ty.get_poison().as_constant());
            continue;
        }
        let Ok(element) = u32::try_from(element) else {
            return Ok(None);
        };
        let source = if element < lanes {
            usize::try_from(element)
                .ok()
                .and_then(|index| lhs_elements.get(index).copied())
        } else {
            let rhs_index = element - lanes;
            if rhs_index >= lanes {
                Some(element_ty.get_undef().as_constant())
            } else {
                usize::try_from(rhs_index)
                    .ok()
                    .and_then(|index| rhs_elements.get(index).copied())
            }
        };
        let Some(source) = source else {
            return Ok(None);
        };
        result.push(source);
    }
    constant_aggregate_from_elements(result_ty, result)
}
/// Decode the vector constant operand spelling used by constant-expression
/// `shufflevector` into the instruction-style mask list.
///
/// Mirrors `ConstantFoldShuffleVectorInstruction`: `undef` mask elements become
/// poison mask lanes, while integer elements select from the concatenated
/// `lhs` / `rhs` lane space.
pub(super) fn shufflevector_mask_from_constant<'ctx, B: ModuleBrand + 'ctx>(
    mask: Constant<'ctx, B>,
) -> Option<Vec<i32>> {
    let (_, lanes, scalable) = mask.ty().data().as_vector()?;
    if scalable {
        return None;
    }
    let lane_count = usize::try_from(lanes).ok()?;
    if is_undef(mask) {
        return Some(vec![POISON_MASK_ELEM; lane_count]);
    }
    let elements = aggregate_elements(mask)?;
    if elements.len() != lane_count {
        return None;
    }
    elements
        .into_iter()
        .map(|element| {
            if is_undef(element) {
                return Some(POISON_MASK_ELEM);
            }
            let int = ConstantIntValue::<IntDyn, B>::try_from(element).ok()?;
            let value = int.ap_int().try_zext_u64()?;
            i32::try_from(value).ok()
        })
        .collect()
}

/// Fold an `extractvalue` with a constant aggregate operand.
pub fn constant_fold_extract_value_instruction<'ctx, B: ModuleBrand + 'ctx>(
    aggregate: Constant<'ctx, B>,
    indices: &[u32],
) -> IrResult<Option<Constant<'ctx, B>>> {
    let mut current = aggregate;
    for index in indices.iter().copied() {
        let Some(elements) = aggregate_elements(current) else {
            return Ok(None);
        };
        let Ok(index) = usize::try_from(index) else {
            return Ok(None);
        };
        let Some(next) = elements.get(index).copied() else {
            return Ok(None);
        };
        current = next;
    }
    Ok(Some(current))
}

/// Fold an `insertvalue` with constant operands.
pub fn constant_fold_insert_value_instruction<'ctx, B: ModuleBrand + 'ctx>(
    aggregate: Constant<'ctx, B>,
    value: Constant<'ctx, B>,
    indices: &[u32],
) -> IrResult<Option<Constant<'ctx, B>>> {
    if indices.is_empty() {
        return Ok(Some(value));
    }
    let Some(elements) = aggregate_elements_for_rebuild(aggregate)? else {
        return Ok(None);
    };
    let Ok(index) = usize::try_from(indices[0]) else {
        return Ok(None);
    };
    let Some(current) = elements.get(index).copied() else {
        return Ok(None);
    };
    let Some(updated) = constant_fold_insert_value_instruction(current, value, &indices[1..])?
    else {
        return Ok(None);
    };
    let mut elements = elements;
    elements[index] = updated;
    constant_aggregate_from_elements(aggregate.ty(), elements)
}

/// Fold a `getelementptr` with constant operands.
pub fn constant_fold_get_element_ptr<'ctx, B: ModuleBrand + 'ctx>(
    _source_ty: Type<'ctx, B>,
    pointer: Constant<'ctx, B>,
    indices: &[Constant<'ctx, B>],
) -> IrResult<Option<Constant<'ctx, B>>> {
    if indices.is_empty() {
        return Ok(Some(pointer));
    }
    Ok(None)
}

fn fold_undef_int_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let lhs_undef = is_undef(lhs);
    let rhs_undef = is_undef(rhs);
    if !lhs_undef && !rhs_undef {
        return Ok(None);
    }
    let Ok(ty) = IntType::<IntDyn, B>::try_from(lhs.ty()) else {
        return Ok(None);
    };
    let undef = || lhs.ty().get_undef().as_constant();
    let zero = || ty.const_zero().as_constant();
    let all_ones = || ty.const_all_ones().as_constant();
    let poison = || poison_for(lhs.ty());

    let folded = match opcode {
        BinaryOpcode::Xor if lhs_undef && rhs_undef => zero(),
        BinaryOpcode::Add | BinaryOpcode::Sub | BinaryOpcode::Xor => undef(),
        BinaryOpcode::And if lhs_undef && rhs_undef => undef(),
        BinaryOpcode::And => zero(),
        BinaryOpcode::Mul if lhs_undef && rhs_undef => undef(),
        BinaryOpcode::Mul => {
            let known = if lhs_undef { rhs } else { lhs };
            if let Ok(known) = ConstantIntValue::<IntDyn, B>::try_from(known)
                && known.ap_int().is_one_bit_set(0)
            {
                undef()
            } else {
                zero()
            }
        }
        BinaryOpcode::UDiv | BinaryOpcode::SDiv | BinaryOpcode::URem | BinaryOpcode::SRem
            if rhs_undef || is_zero_int_constant(rhs) =>
        {
            poison()
        }
        BinaryOpcode::UDiv | BinaryOpcode::SDiv | BinaryOpcode::URem | BinaryOpcode::SRem => zero(),
        BinaryOpcode::Or if lhs_undef && rhs_undef => undef(),
        BinaryOpcode::Or => all_ones(),
        BinaryOpcode::LShr | BinaryOpcode::AShr | BinaryOpcode::Shl if rhs_undef => poison(),
        BinaryOpcode::LShr | BinaryOpcode::AShr | BinaryOpcode::Shl => zero(),
        BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => return Ok(None),
    };
    Ok(Some(folded))
}

fn fold_int_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Ok(lhs) = ConstantIntValue::<IntDyn, B>::try_from(lhs) else {
        return Ok(None);
    };
    let Ok(rhs) = ConstantIntValue::<IntDyn, B>::try_from(rhs) else {
        return Ok(None);
    };
    if lhs.bit_width() != rhs.bit_width() {
        return Ok(None);
    }

    let lhs_ap = lhs.ap_int();
    let rhs_ap = rhs.ap_int();
    let result = match opcode {
        BinaryOpcode::Add => Some(lhs_ap.wrapping_add(&rhs_ap)),
        BinaryOpcode::Sub => Some(lhs_ap.wrapping_sub(&rhs_ap)),
        BinaryOpcode::Mul => Some(lhs_ap.wrapping_mul(&rhs_ap)),
        BinaryOpcode::And => Some(lhs_ap.bitand(&rhs_ap)),
        BinaryOpcode::Or => Some(lhs_ap.bitor(&rhs_ap)),
        BinaryOpcode::Xor => Some(lhs_ap.bitxor(&rhs_ap)),
        BinaryOpcode::Shl => {
            fold_shift(&lhs_ap, &rhs_ap, |value, amount| value.checked_shl(amount))
        }
        BinaryOpcode::LShr => {
            fold_shift(&lhs_ap, &rhs_ap, |value, amount| value.checked_lshr(amount))
        }
        BinaryOpcode::AShr => {
            fold_shift(&lhs_ap, &rhs_ap, |value, amount| value.checked_ashr(amount))
        }
        BinaryOpcode::UDiv => lhs_ap.checked_udiv(&rhs_ap),
        BinaryOpcode::SDiv => lhs_ap.checked_sdiv(&rhs_ap),
        BinaryOpcode::URem => lhs_ap.checked_urem(&rhs_ap),
        BinaryOpcode::SRem => lhs_ap.checked_srem(&rhs_ap),
        BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => return Ok(None),
    };

    let Some(result) = result else {
        return Ok(Some(poison_for(lhs.ty().as_type())));
    };
    Ok(Some(lhs.ty().const_ap_int(&result)?.as_constant()))
}

fn fold_exact_int_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Ok(lhs) = ConstantIntValue::<IntDyn, B>::try_from(lhs) else {
        return Ok(None);
    };
    let Ok(rhs) = ConstantIntValue::<IntDyn, B>::try_from(rhs) else {
        return Ok(None);
    };
    if lhs.bit_width() != rhs.bit_width() {
        return Ok(None);
    }

    let lhs_ap = lhs.ap_int();
    let rhs_ap = rhs.ap_int();
    let result = match opcode {
        BinaryOpcode::UDiv => exact_div_result(lhs_ap.udivrem(&rhs_ap)),
        BinaryOpcode::SDiv => exact_div_result(lhs_ap.sdivrem(&rhs_ap)),
        BinaryOpcode::LShr => {
            exact_shift_result(&lhs_ap, &rhs_ap, |value, amount| value.checked_lshr(amount))
        }
        BinaryOpcode::AShr => {
            exact_shift_result(&lhs_ap, &rhs_ap, |value, amount| value.checked_ashr(amount))
        }
        BinaryOpcode::Add
        | BinaryOpcode::Sub
        | BinaryOpcode::Mul
        | BinaryOpcode::URem
        | BinaryOpcode::SRem
        | BinaryOpcode::Shl
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::Xor
        | BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => return Ok(None),
    };

    let Some(result) = result else {
        return Ok(Some(poison_for(lhs.ty().as_type())));
    };
    Ok(Some(lhs.ty().const_ap_int(&result)?.as_constant()))
}

fn exact_div_result(qr: Option<ApIntDivRem>) -> Option<ApInt> {
    let (quotient, remainder) = qr?.into_parts();
    remainder.is_zero().then_some(quotient)
}

fn exact_shift_result(
    lhs: &ApInt,
    rhs: &ApInt,
    f: impl FnOnce(&ApInt, u32) -> Option<ApInt>,
) -> Option<ApInt> {
    let amount = rhs.try_zext_u64()?;
    let amount = u32::try_from(amount).ok()?;
    if amount >= lhs.bit_width() || !shifted_out_bits_are_zero(lhs, amount) {
        return None;
    }
    f(lhs, amount)
}

fn shifted_out_bits_are_zero(value: &ApInt, amount: u32) -> bool {
    value
        .bitand(&ApInt::low_bits_set(value.bit_width(), amount))
        .is_zero()
}

fn is_exact_capable_binop(opcode: BinaryOpcode) -> bool {
    matches!(
        opcode,
        BinaryOpcode::UDiv | BinaryOpcode::SDiv | BinaryOpcode::LShr | BinaryOpcode::AShr
    )
}

fn fold_shift(
    lhs: &ApInt,
    rhs: &ApInt,
    f: impl FnOnce(&ApInt, u32) -> Option<ApInt>,
) -> Option<ApInt> {
    let amount = rhs.try_zext_u64()?;
    let amount = u32::try_from(amount).ok()?;
    f(lhs, amount)
}

fn fold_float_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Ok(lhs) = ConstantFloatValue::<FloatDyn, B>::try_from(lhs) else {
        return Ok(None);
    };
    let Ok(rhs) = ConstantFloatValue::<FloatDyn, B>::try_from(rhs) else {
        return Ok(None);
    };
    let lhs_ap = lhs.ap_float();
    let rhs_ap = rhs.ap_float();
    if lhs_ap.semantics() != rhs_ap.semantics() {
        return Ok(None);
    }

    let (result, _) = match opcode {
        BinaryOpcode::FAdd => lhs_ap.add(&rhs_ap, RoundingMode::NearestTiesToEven),
        BinaryOpcode::FSub => lhs_ap.subtract(&rhs_ap, RoundingMode::NearestTiesToEven),
        BinaryOpcode::FMul => lhs_ap.multiply(&rhs_ap, RoundingMode::NearestTiesToEven),
        BinaryOpcode::FDiv => lhs_ap.divide(&rhs_ap, RoundingMode::NearestTiesToEven),
        BinaryOpcode::FRem => lhs_ap.modulo(&rhs_ap),
        BinaryOpcode::Add
        | BinaryOpcode::Sub
        | BinaryOpcode::Mul
        | BinaryOpcode::UDiv
        | BinaryOpcode::SDiv
        | BinaryOpcode::URem
        | BinaryOpcode::SRem
        | BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::Xor => return Ok(None),
    };
    Ok(Some(lhs.ty().const_ap_float(&result)?.as_constant()))
}

fn fold_bitcast<'ctx, B: ModuleBrand + 'ctx>(
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if operand.ty() == dest_ty {
        return Ok(Some(operand));
    }
    if let Ok(src) = ConstantIntValue::<IntDyn, B>::try_from(operand)
        && let Ok(dst_ty) = IntType::<IntDyn, B>::try_from(dest_ty)
        && src.bit_width() == dst_ty.bit_width()
    {
        return Ok(Some(dst_ty.const_ap_int(&src.ap_int())?.as_constant()));
    }
    if let Ok(src) = ConstantIntValue::<IntDyn, B>::try_from(operand)
        && let Ok(dst_ty) = FloatType::<FloatDyn, B>::try_from(dest_ty)
        && dst_ty.semantics() != ApFloatSemantics::PpcDoubleDouble
        && src.bit_width() == dst_ty.semantics().bit_width()
    {
        let fp = crate::ApFloat::from_bits(dst_ty.semantics(), &src.ap_int())?;
        return Ok(Some(dst_ty.const_ap_float(&fp)?.as_constant()));
    }
    if let Ok(src) = ConstantFloatValue::<FloatDyn, B>::try_from(operand)
        && let Ok(dst_ty) = IntType::<IntDyn, B>::try_from(dest_ty)
        && src.ty().semantics() != ApFloatSemantics::PpcDoubleDouble
        && src.ty().semantics().bit_width() == dst_ty.bit_width()
    {
        return Ok(Some(
            dst_ty
                .const_ap_int(&src.ap_float().to_bits())?
                .as_constant(),
        ));
    }
    Ok(None)
}

fn fold_int_predicate(predicate: IntPredicate, lhs: &ApInt, rhs: &ApInt) -> bool {
    match predicate {
        IntPredicate::Eq => lhs.eq_ap_int(rhs),
        IntPredicate::Ne => !lhs.eq_ap_int(rhs),
        IntPredicate::Ugt => lhs.ugt(rhs),
        IntPredicate::Uge => lhs.uge(rhs),
        IntPredicate::Ult => lhs.ult(rhs),
        IntPredicate::Ule => lhs.ule(rhs),
        IntPredicate::Sgt => lhs.sgt(rhs),
        IntPredicate::Sge => lhs.sge(rhs),
        IntPredicate::Slt => lhs.slt(rhs),
        IntPredicate::Sle => lhs.sle(rhs),
    }
}

fn fold_float_predicate(predicate: FloatPredicate, cmp: ApFloatCmpResult) -> bool {
    let unordered = matches!(cmp, ApFloatCmpResult::Unordered);
    match predicate {
        FloatPredicate::False => false,
        FloatPredicate::Oeq => matches!(cmp, ApFloatCmpResult::Equal),
        FloatPredicate::Ogt => matches!(cmp, ApFloatCmpResult::GreaterThan),
        FloatPredicate::Oge => {
            matches!(cmp, ApFloatCmpResult::GreaterThan | ApFloatCmpResult::Equal)
        }
        FloatPredicate::Olt => matches!(cmp, ApFloatCmpResult::LessThan),
        FloatPredicate::Ole => matches!(cmp, ApFloatCmpResult::LessThan | ApFloatCmpResult::Equal),
        FloatPredicate::One => matches!(
            cmp,
            ApFloatCmpResult::LessThan | ApFloatCmpResult::GreaterThan
        ),
        FloatPredicate::Ord => !unordered,
        FloatPredicate::Uno => unordered,
        FloatPredicate::Ueq => unordered || matches!(cmp, ApFloatCmpResult::Equal),
        FloatPredicate::Ugt => unordered || matches!(cmp, ApFloatCmpResult::GreaterThan),
        FloatPredicate::Uge => {
            unordered || matches!(cmp, ApFloatCmpResult::GreaterThan | ApFloatCmpResult::Equal)
        }
        FloatPredicate::Ult => unordered || matches!(cmp, ApFloatCmpResult::LessThan),
        FloatPredicate::Ule => {
            unordered || matches!(cmp, ApFloatCmpResult::LessThan | ApFloatCmpResult::Equal)
        }
        FloatPredicate::Une => {
            unordered
                || matches!(
                    cmp,
                    ApFloatCmpResult::LessThan | ApFloatCmpResult::GreaterThan
                )
        }
        FloatPredicate::True => true,
    }
}

fn null_constant_for_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(int_ty) = IntType::<IntDyn, B>::try_from(ty) {
        return Ok(Some(int_ty.const_zero().as_constant()));
    }
    if let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(ty) {
        let zero = crate::ApFloat::zero(float_ty.semantics(), ApFloatSign::Positive);
        return Ok(Some(float_ty.const_ap_float(&zero)?.as_constant()));
    }
    if let Some((element_ty, lanes, scalable)) = ty.data().as_vector() {
        let module = ty.module();
        let element_ty = Type::new(element_ty, module);
        let Some(element) = null_constant_for_type(element_ty)? else {
            return Ok(None);
        };
        let Ok(lane_count) = usize::try_from(lanes) else {
            return Ok(None);
        };
        let vector_ty = module.vector_type(element_ty, lanes, scalable);
        return vector_ty
            .const_vector((0..lane_count).map(|_| element))
            .map(|constant| Some(constant.as_constant()));
    }
    Ok(None)
}

fn fold_undef_float_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let lhs_undef = is_undef(lhs);
    let rhs_undef = is_undef(rhs);
    if !lhs_undef && !rhs_undef {
        return Ok(None);
    }
    let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(lhs.ty()) else {
        return Ok(None);
    };
    let undef = || lhs.ty().get_undef().as_constant();
    let nan = || {
        let value = crate::ApFloat::qnan(
            float_ty.semantics(),
            ApFloatSign::Positive,
            NanPayload::Absent,
        );
        float_ty
            .const_ap_float(&value)
            .map(|value| value.as_constant())
    };

    let folded = match opcode {
        BinaryOpcode::FSub
            if rhs_undef
                && ConstantFloatValue::<FloatDyn, B>::try_from(lhs)
                    .is_ok_and(|value| value.ap_float().is_neg_zero()) =>
        {
            undef()
        }
        BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem
            if lhs_undef && rhs_undef =>
        {
            undef()
        }
        BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => nan()?,
        BinaryOpcode::Add
        | BinaryOpcode::Sub
        | BinaryOpcode::Mul
        | BinaryOpcode::UDiv
        | BinaryOpcode::SDiv
        | BinaryOpcode::URem
        | BinaryOpcode::SRem
        | BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::Xor => return Ok(None),
    };
    Ok(Some(folded))
}

fn compare_result_type<'ctx, B: ModuleBrand + 'ctx>(operand_ty: Type<'ctx, B>) -> Type<'ctx, B> {
    let module = operand_ty.module();
    let bool_ty: IntType<'ctx, bool, B> = IntType::new(module.context().int_type(1), module);
    if let Some((_, lanes, scalable)) = operand_ty.data().as_vector() {
        module
            .vector_type(bool_ty.as_type(), lanes, scalable)
            .as_type()
    } else {
        bool_ty.as_type()
    }
}

fn fold_undef_compare<'ctx, B: ModuleBrand + 'ctx>(
    predicate: CmpPredicate,
    result_ty: Type<'ctx, B>,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if is_equality_predicate(predicate)
        || (matches!(predicate, CmpPredicate::Int(_)) && is_undef(lhs) && is_undef(rhs))
    {
        return Ok(Some(result_ty.get_undef().as_constant()));
    }
    match predicate {
        CmpPredicate::Int(predicate) => {
            bool_constant_for_type(result_ty, int_predicate_true_when_equal(predicate))
        }
        CmpPredicate::Float(predicate) => {
            bool_constant_for_type(result_ty, float_predicate_is_unordered(predicate))
        }
    }
}

fn bool_constant_for_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    value: bool,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(bool_ty) = IntType::<bool, B>::try_from(ty) {
        return Ok(Some(bool_ty.const_int(value).as_constant()));
    }
    let Some((element_ty, lanes, _scalable)) = ty.data().as_vector() else {
        return Ok(None);
    };
    let module = ty.module();
    let bool_ty: IntType<'ctx, bool, B> = IntType::new(module.context().int_type(1), module);
    if element_ty != bool_ty.as_type().id() {
        return Ok(None);
    }
    let Ok(lane_count) = usize::try_from(lanes) else {
        return Ok(None);
    };
    let scalar = bool_ty.const_int(value).as_constant();
    let vector_ty = VectorType::try_from(ty)?;
    vector_ty
        .const_vector((0..lane_count).map(|_| scalar))
        .map(|constant| Some(constant.as_constant()))
}

fn is_equality_predicate(predicate: CmpPredicate) -> bool {
    matches!(
        predicate,
        CmpPredicate::Int(IntPredicate::Eq | IntPredicate::Ne)
            | CmpPredicate::Float(
                FloatPredicate::Oeq
                    | FloatPredicate::One
                    | FloatPredicate::Ueq
                    | FloatPredicate::Une,
            )
    )
}

fn int_predicate_true_when_equal(predicate: IntPredicate) -> bool {
    matches!(
        predicate,
        IntPredicate::Eq
            | IntPredicate::Uge
            | IntPredicate::Ule
            | IntPredicate::Sge
            | IntPredicate::Sle
    )
}

fn float_predicate_is_unordered(predicate: FloatPredicate) -> bool {
    matches!(
        predicate,
        FloatPredicate::Uno
            | FloatPredicate::Ueq
            | FloatPredicate::Ugt
            | FloatPredicate::Uge
            | FloatPredicate::Ult
            | FloatPredicate::Ule
            | FloatPredicate::Une
            | FloatPredicate::True
    )
}

fn bool_constant_value<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> Option<bool> {
    let Ok(constant) = ConstantIntValue::<IntDyn, B>::try_from(constant) else {
        return None;
    };
    (constant.bit_width() == 1).then(|| !constant.ap_int().is_zero())
}

fn fixed_vector_elements_for_rebuild<'ctx, B: ModuleBrand + 'ctx>(
    vector: Constant<'ctx, B>,
    lanes: u32,
    element_ty: Type<'ctx, B>,
) -> IrResult<Option<Vec<Constant<'ctx, B>>>> {
    if let Some(elements) = aggregate_elements(vector) {
        let Ok(lane_count) = usize::try_from(lanes) else {
            return Ok(None);
        };
        if elements.len() == lane_count {
            return Ok(Some(elements));
        }
        return Ok(None);
    }

    let fill = if is_undef(vector) {
        element_ty.get_undef().as_constant()
    } else if is_poison(vector) {
        poison_for(element_ty)
    } else {
        return Ok(None);
    };
    let Ok(lane_count) = usize::try_from(lanes) else {
        return Ok(None);
    };
    Ok(Some((0..lane_count).map(|_| fill).collect()))
}

fn aggregate_elements_for_rebuild<'ctx, B: ModuleBrand + 'ctx>(
    aggregate: Constant<'ctx, B>,
) -> IrResult<Option<Vec<Constant<'ctx, B>>>> {
    if let Some(elements) = aggregate_elements(aggregate) {
        return Ok(Some(elements));
    }
    let fill_for = |ty: Type<'ctx, B>| {
        if is_undef(aggregate) {
            Some(ty.get_undef().as_constant())
        } else if is_poison(aggregate) {
            Some(poison_for(ty))
        } else {
            None
        }
    };
    if let Ok(array_ty) = ArrayType::try_from(aggregate.ty()) {
        let Some(fill) = fill_for(array_ty.element()) else {
            return Ok(None);
        };
        let Ok(count) = usize::try_from(array_ty.len()) else {
            return Ok(None);
        };
        return Ok(Some((0..count).map(|_| fill).collect()));
    }
    if let Ok(struct_ty) = StructType::try_from(aggregate.ty()) {
        let mut elements = Vec::with_capacity(struct_ty.field_count());
        for index in 0..struct_ty.field_count() {
            let Some(field_ty) = struct_ty.field_type(index) else {
                return Ok(None);
            };
            let Some(fill) = fill_for(field_ty) else {
                return Ok(None);
            };
            elements.push(fill);
        }
        return Ok(Some(elements));
    }
    Ok(None)
}

fn is_not_poison_for_select<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    match &constant.as_value().data().kind {
        ValueKindData::Constant(
            ConstantData::Int(_)
            | ConstantData::Float(_)
            | ConstantData::GlobalValueRef { .. }
            | ConstantData::PointerNull,
        ) => true,
        ValueKindData::Constant(ConstantData::Aggregate(elements))
            if constant.ty().data().as_vector().is_some() =>
        {
            let module = constant.as_value().module();
            elements.iter().all(|element| {
                !matches!(
                    &module.context().value_data(*element).kind,
                    ValueKindData::Constant(ConstantData::Poison | ConstantData::Expr(_))
                )
            })
        }
        _ => false,
    }
}

fn constant_aggregate_from_elements<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    elements: Vec<Constant<'ctx, B>>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(vector_ty) = VectorType::try_from(ty) {
        return vector_ty
            .const_vector::<Constant<'ctx, B>, _>(elements)
            .map(|aggregate| Some(aggregate.as_constant()));
    }
    if let Ok(array_ty) = ArrayType::try_from(ty) {
        return array_ty
            .const_array::<Constant<'ctx, B>, _>(elements)
            .map(|aggregate| Some(aggregate.as_constant()));
    }
    if let Ok(struct_ty) = StructType::try_from(ty) {
        return struct_ty
            .const_struct::<Constant<'ctx, B>, _>(elements)
            .map(|aggregate| Some(aggregate.as_constant()));
    }
    Ok(None)
}

fn aggregate_elements<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> Option<Vec<Constant<'ctx, B>>> {
    let ValueKindData::Constant(ConstantData::Aggregate(elements)) =
        &constant.as_value().data().kind
    else {
        return None;
    };
    let module = constant.as_value().module();
    Some(
        elements
            .iter()
            .map(|id| {
                let data = module.context().value_data(*id);
                Constant::from_parts(crate::Value::from_parts(*id, module, data.ty))
            })
            .collect(),
    )
}

fn is_poison<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    matches!(
        &constant.as_value().data().kind,
        ValueKindData::Constant(ConstantData::Poison)
    )
}

fn is_undef<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    matches!(
        &constant.as_value().data().kind,
        ValueKindData::Constant(ConstantData::Undef)
    )
}

fn fold_int_identity_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> Option<Constant<'ctx, B>> {
    match opcode {
        BinaryOpcode::Add | BinaryOpcode::Xor if is_zero_int_constant(rhs) => Some(lhs),
        BinaryOpcode::Add | BinaryOpcode::Xor if is_zero_int_constant(lhs) => Some(rhs),
        BinaryOpcode::Sub if is_zero_int_constant(rhs) => Some(lhs),
        _ => None,
    }
}

fn is_zero_int_constant<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    ConstantIntValue::<IntDyn, B>::try_from(constant).is_ok_and(|value| value.ap_int().is_zero())
}

fn poison_for<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Constant<'ctx, B> {
    ty.get_poison().as_constant()
}
