//! Target-independent constant folding.
//!
//! Mirrors the pure-constant portions of `llvm/lib/IR/ConstantFold.cpp`.

use super::align::Align;
use super::ap_float::{ApFloatCmpResult, ApFloatSemantics, ApFloatSign, NanPayload};
use super::ap_int::{ApInt, ApIntDivRem, ApIntSignedness};
use super::array_len::ArrLenDyn;
use super::cmp_predicate::{CmpPredicate, FloatPredicate, IntPredicate};
use super::constant::{
    Constant, ConstantData, ConstantExprData, ConstantExprFlags, ConstantExprInRange,
    ConstantExprOpcode,
};
use super::constants::{ConstantExprOptions, ConstantFloatValue, ConstantIntValue};
use super::data_layout::FunctionPtrAlignType;
use super::derived_types::{ArrayType, FloatType, IntType, PointerType, StructType, VectorType};
use super::element::ElemDyn;
use super::float_kind::FloatDyn;
use super::gep_no_wrap_flags::GepNoWrapFlags;
use super::global_value::Linkage;
use super::instr_types::{BinaryOpcode, CastOpcode, POISON_MASK_ELEM, UnaryOpcode};
use super::instruction::{InstructionKindData, InstructionView};
use super::int_width::IntDyn;
use super::module::{ModuleBrand, ModuleRef, ModuleView};
use super::r#type::{Type, TypeData};
use super::unnamed_addr::UnnamedAddr;
use super::value::{Value, ValueId, ValueKindData};
use super::vec_len::LenDyn;
use super::{IrError, IrResult, RoundingMode};

/// Dispatch an instruction to the target-independent pure-constant folds
/// mirrored from `llvm/lib/IR/ConstantFold.cpp`.
///
/// Callers get `Ok(None)` when an operand is non-constant or the opcode has no
/// pure-constant fold. DataLayout / TLI analysis folds live in
/// [`crate::constant_folding`].
pub fn constant_fold_instruction<'ctx, B>(
    instruction: &InstructionView<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>>
where
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
            constant_fold_get_element_ptr(Type::new(gep.source_ty, module), pointer, &indices, None)
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
            if is_undef(operand) && !matches!(operand.ty().data(), TypeData::FixedVector { .. }) {
                return Ok(Some(operand));
            }
            if let Some((element_ty, lanes, scalable)) = operand.ty().data().as_vector() {
                let element_ty = Type::new(element_ty, operand.as_value().module());
                if let Some(splat) = constant_splat_value(operand) {
                    let Some(folded) = constant_fold_unary_instruction(opcode, splat)? else {
                        return Ok(None);
                    };
                    return vector_splat_constant(operand.ty(), folded);
                }
                if scalable {
                    return Ok(None);
                }
                let Some(elements) = fixed_vector_elements_for_rebuild(operand, lanes, element_ty)?
                else {
                    return Ok(None);
                };
                let Ok(lane_count) = usize::try_from(lanes) else {
                    return Ok(None);
                };
                let mut folded = Vec::with_capacity(lane_count);
                for element in elements {
                    let Some(result) = constant_fold_unary_instruction(opcode, element)? else {
                        return Ok(None);
                    };
                    folded.push(result);
                }
                return constant_aggregate_from_elements(operand.ty(), folded);
            }
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

    if let Some(folded) = binop_identity(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }

    if is_poison(lhs) || is_poison(rhs) {
        return Ok(Some(poison_for(lhs.ty())));
    }
    if matches!(
        opcode,
        BinaryOpcode::UDiv | BinaryOpcode::SDiv | BinaryOpcode::URem | BinaryOpcode::SRem
    ) && constant_is_null_value(rhs)
    {
        return Ok(Some(poison_for(lhs.ty())));
    }

    if let Some(folded) = fold_undef_int_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }
    if let Some(folded) = fold_undef_float_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }

    if let Some(folded) = binop_rhs_absorber(opcode, rhs)? {
        return Ok(Some(folded));
    }

    if let Some(folded) = fold_global_pointer_and_mask(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }

    if let Some(folded) = fold_vector_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }

    if let Some(folded) = fold_constant_expr_associative_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }

    if matches!(opcode, BinaryOpcode::URem | BinaryOpcode::SRem)
        && let Some(folded) = fold_i1_binary(opcode, lhs, rhs)?
    {
        return Ok(Some(folded));
    }

    let folded = match opcode {
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
        | BinaryOpcode::Xor => fold_int_binary(opcode, lhs, rhs)?,
        BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => fold_float_binary(opcode, lhs, rhs)?,
    };
    if folded.is_some() {
        return Ok(folded);
    }
    if let Some(folded) = fold_i1_binary(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }
    if let Some(folded) = binop_lhs_absorber(opcode, lhs)? {
        return Ok(Some(folded));
    }
    Ok(None)
}

fn fold_vector_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some((element_ty, lanes, scalable)) = lhs.ty().data().as_vector() else {
        return Ok(None);
    };
    let element_ty = Type::new(element_ty, lhs.as_value().module());
    if let (Some(lhs_splat), Some(rhs_splat)) =
        (constant_splat_value(lhs), constant_splat_value(rhs))
    {
        let Some(folded) = build_binary_constant_or_expr(opcode, lhs_splat, rhs_splat)? else {
            return Ok(None);
        };
        return vector_splat_constant(lhs.ty(), folded);
    }
    if scalable {
        return Ok(None);
    }
    let Some(lhs_elements) = fixed_vector_elements_for_rebuild(lhs, lanes, element_ty)? else {
        return Ok(None);
    };
    let Some(rhs_elements) = fixed_vector_elements_for_rebuild(rhs, lanes, element_ty)? else {
        return Ok(None);
    };
    let Ok(lane_count) = usize::try_from(lanes) else {
        return Ok(None);
    };
    if lhs_elements.len() != lane_count || rhs_elements.len() != lane_count {
        return Ok(None);
    }
    let mut folded = Vec::with_capacity(lane_count);
    for (lhs_element, rhs_element) in lhs_elements.into_iter().zip(rhs_elements) {
        let Some(element) = build_binary_constant_or_expr(opcode, lhs_element, rhs_element)? else {
            return Ok(None);
        };
        folded.push(element);
    }
    constant_aggregate_from_elements(lhs.ty(), folded)
}

fn fold_constant_expr_associative_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &lhs.as_value().data().kind else {
        if opcode.is_commutative()
            && matches!(
                &rhs.as_value().data().kind,
                ValueKindData::Constant(ConstantData::Expr(_))
            )
        {
            if ConstantIntValue::<IntDyn, B>::try_from(lhs).is_ok()
                && opcode.is_desirable_constant_expr()
            {
                return build_binary_constant_or_expr(opcode, rhs, lhs);
            }
            return constant_fold_binary_instruction(opcode, rhs, lhs);
        }
        return Ok(None);
    };
    if !opcode.is_associative() || constant_expr_binary_opcode(expr.opcode) != Some(opcode) {
        return Ok(None);
    }
    let [operand0, operand1] = expr.operands.as_ref() else {
        return Ok(None);
    };
    let module = lhs.as_value().module();
    let operand0_data = module.context().value_data(*operand0);
    let operand1_data = module.context().value_data(*operand1);
    let operand0 = Constant::try_from(Value::from_parts(*operand0, module, operand0_data.ty))?;
    let operand1 = Constant::try_from(Value::from_parts(*operand1, module, operand1_data.ty))?;
    let Some(nested) = build_binary_constant_or_expr(opcode, operand1, rhs)? else {
        return Ok(None);
    };
    if constant_expr_binary_opcode_of(nested) == Some(opcode) {
        return Ok(None);
    }
    build_binary_constant_or_expr(opcode, operand0, nested)
}

fn build_binary_constant_or_expr<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if opcode.is_desirable_constant_expr() {
        let Some(expr_opcode) = binary_constant_expr_opcode(opcode) else {
            return Ok(None);
        };
        return lhs
            .as_value()
            .module()
            .core_ref()
            .constant_expr(
                lhs.ty(),
                expr_opcode,
                [lhs.as_value(), rhs.as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )
            .map(Some);
    }
    constant_fold_binary_instruction(opcode, lhs, rhs)
}

fn constant_expr_binary_opcode_of<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> Option<BinaryOpcode> {
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &constant.as_value().data().kind else {
        return None;
    };
    constant_expr_binary_opcode(expr.opcode)
}

fn constant_expr_binary_opcode(opcode: ConstantExprOpcode) -> Option<BinaryOpcode> {
    match opcode {
        ConstantExprOpcode::Add => Some(BinaryOpcode::Add),
        ConstantExprOpcode::Sub => Some(BinaryOpcode::Sub),
        ConstantExprOpcode::Xor => Some(BinaryOpcode::Xor),
        ConstantExprOpcode::GetElementPtr
        | ConstantExprOpcode::ShuffleVector
        | ConstantExprOpcode::InsertElement
        | ConstantExprOpcode::ExtractElement
        | ConstantExprOpcode::Trunc
        | ConstantExprOpcode::PtrToAddr
        | ConstantExprOpcode::PtrToInt
        | ConstantExprOpcode::IntToPtr
        | ConstantExprOpcode::BitCast
        | ConstantExprOpcode::AddrSpaceCast => None,
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
    if constant_is_null_value(operand) && !matches!(opcode, CastOpcode::AddrSpaceCast) {
        return null_constant_for_type(dest_ty);
    }
    if let Some(folded) = fold_constant_cast_pair(opcode, operand, dest_ty)? {
        return Ok(Some(folded));
    }
    if let Some(folded) = fold_same_lane_vector_cast(opcode, operand, dest_ty)? {
        return Ok(Some(folded));
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

fn fold_constant_cast_pair<'ctx, B: ModuleBrand + 'ctx>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &operand.as_value().data().kind else {
        return Ok(None);
    };
    if !expr.opcode.is_cast() {
        return Ok(None);
    }
    let Some(first_opcode) = cast_opcode_from_constant_expr(expr.opcode) else {
        return Ok(None);
    };
    let [source_id] = expr.operands.as_ref() else {
        return Ok(None);
    };
    let module = operand.as_value().module();
    let source_data = module.context().value_data(*source_id);
    let source_value = Value::from_parts(*source_id, module, source_data.ty);
    let Ok(source) = Constant::try_from(source_value) else {
        return Ok(None);
    };
    let Some(new_opcode) =
        fold_constant_cast_pair_opcode(first_opcode, opcode, source.ty(), operand.ty(), dest_ty)
    else {
        return Ok(None);
    };
    fold_maybe_undesirable_cast(new_opcode, source, dest_ty)
}

fn fold_maybe_undesirable_cast<'ctx, B: ModuleBrand + 'ctx>(
    opcode: CastOpcode,
    value: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if opcode.is_desirable_constant_expr() {
        let Some(expr_opcode) = constant_expr_opcode_from_cast(opcode) else {
            return Ok(None);
        };
        return value
            .as_value()
            .module()
            .core_ref()
            .constant_expr(
                dest_ty,
                expr_opcode,
                [value.as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )
            .map(Some);
    }
    constant_fold_cast_instruction(opcode, value, dest_ty)
}

fn fold_constant_cast_pair_opcode<B: ModuleBrand>(
    first_opcode: CastOpcode,
    second_opcode: CastOpcode,
    source_ty: Type<'_, B>,
    middle_ty: Type<'_, B>,
    dest_ty: Type<'_, B>,
) -> Option<CastOpcode> {
    match (first_opcode, second_opcode) {
        (CastOpcode::Trunc, CastOpcode::Trunc) => Some(CastOpcode::Trunc),
        (CastOpcode::Trunc, CastOpcode::BitCast)
            if !type_is_vector(source_ty) && type_is_integer(dest_ty) =>
        {
            Some(CastOpcode::Trunc)
        }
        (CastOpcode::PtrToInt, CastOpcode::Trunc) => Some(CastOpcode::PtrToInt),
        (CastOpcode::PtrToInt, CastOpcode::BitCast)
            if !type_is_vector(source_ty) && type_is_integer(dest_ty) =>
        {
            Some(CastOpcode::PtrToInt)
        }
        (CastOpcode::PtrToAddr, CastOpcode::BitCast)
            if !type_is_vector(source_ty) && type_is_integer(dest_ty) =>
        {
            Some(CastOpcode::PtrToAddr)
        }
        (CastOpcode::IntToPtr, CastOpcode::BitCast)
            if pointer_address_space(middle_ty) == pointer_address_space(dest_ty) =>
        {
            Some(CastOpcode::IntToPtr)
        }
        (CastOpcode::BitCast, CastOpcode::Trunc) if type_is_integer(source_ty) => {
            Some(CastOpcode::Trunc)
        }
        (CastOpcode::BitCast, CastOpcode::PtrToAddr)
            if pointer_address_space(source_ty) == pointer_address_space(middle_ty)
                && type_is_integer(dest_ty) =>
        {
            Some(CastOpcode::PtrToAddr)
        }
        (CastOpcode::BitCast, CastOpcode::PtrToInt)
            if pointer_address_space(source_ty) == pointer_address_space(middle_ty)
                && type_is_integer(dest_ty) =>
        {
            Some(CastOpcode::PtrToInt)
        }
        (CastOpcode::BitCast, CastOpcode::IntToPtr) if type_is_integer(source_ty) => {
            Some(CastOpcode::IntToPtr)
        }
        (CastOpcode::BitCast, CastOpcode::BitCast) => Some(CastOpcode::BitCast),
        (CastOpcode::BitCast, CastOpcode::AddrSpaceCast) => Some(CastOpcode::AddrSpaceCast),
        (CastOpcode::AddrSpaceCast, CastOpcode::BitCast)
            if pointer_address_space(source_ty) != pointer_address_space(middle_ty)
                && pointer_address_space(middle_ty) == pointer_address_space(dest_ty) =>
        {
            Some(CastOpcode::AddrSpaceCast)
        }
        (CastOpcode::AddrSpaceCast, CastOpcode::AddrSpaceCast) => {
            if pointer_address_space(source_ty) == pointer_address_space(dest_ty) {
                Some(CastOpcode::BitCast)
            } else {
                Some(CastOpcode::AddrSpaceCast)
            }
        }
        _ => None,
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
        ConstantExprOpcode::Add
        | ConstantExprOpcode::Sub
        | ConstantExprOpcode::Xor
        | ConstantExprOpcode::GetElementPtr
        | ConstantExprOpcode::ShuffleVector
        | ConstantExprOpcode::InsertElement
        | ConstantExprOpcode::ExtractElement => None,
    }
}

fn constant_expr_opcode_from_cast(opcode: CastOpcode) -> Option<ConstantExprOpcode> {
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

fn type_is_vector<B: ModuleBrand>(ty: Type<'_, B>) -> bool {
    matches!(
        ty.data(),
        TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }
    )
}

fn type_is_integer<B: ModuleBrand>(ty: Type<'_, B>) -> bool {
    IntType::<IntDyn, B>::try_from(ty).is_ok()
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
    if let CmpPredicate::Int(pred) = predicate {
        if constant_is_null_value(rhs) {
            match pred {
                IntPredicate::Uge => return bool_constant_for_type(result_ty, true),
                IntPredicate::Ult => return bool_constant_for_type(result_ty, false),
                _ => {}
            }
        }
        if is_i1_or_i1_vector_type(lhs.ty()) {
            match pred {
                IntPredicate::Eq => {
                    let lhs_is_expr = matches!(
                        &lhs.as_value().data().kind,
                        ValueKindData::Constant(ConstantData::Expr(_))
                    );
                    let (lhs, rhs) = if lhs_is_expr {
                        let Some(rhs_not) = constant_not(rhs)? else {
                            return Ok(None);
                        };
                        (lhs, rhs_not)
                    } else {
                        let Some(lhs_not) = constant_not(lhs)? else {
                            return Ok(None);
                        };
                        (lhs_not, rhs)
                    };
                    return build_binary_constant_or_expr(BinaryOpcode::Xor, lhs, rhs);
                }
                IntPredicate::Ne => {
                    return build_binary_constant_or_expr(BinaryOpcode::Xor, lhs, rhs);
                }
                _ => {}
            }
        }
    }

    if lhs.ty().data().as_vector().is_some()
        && let (Some(lhs_splat), Some(rhs_splat)) =
            (constant_splat_value(lhs), constant_splat_value(rhs))
        && let Some(folded) = constant_fold_compare_instruction(predicate, lhs_splat, rhs_splat)?
    {
        return vector_splat_constant(result_ty, folded);
    }
    if let Some((element_ty, lanes, scalable)) = lhs.ty().data().as_vector() {
        if scalable {
            return Ok(None);
        }
        let element_ty = Type::new(element_ty, lhs.as_value().module());
        let Some(lhs_elements) = fixed_vector_elements_for_rebuild(lhs, lanes, element_ty)? else {
            return Ok(None);
        };
        let Some(rhs_elements) = fixed_vector_elements_for_rebuild(rhs, lanes, element_ty)? else {
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
            if let (Ok(lhs_int), Ok(rhs_int)) = (
                ConstantIntValue::<IntDyn, B>::try_from(lhs),
                ConstantIntValue::<IntDyn, B>::try_from(rhs),
            ) {
                let result = fold_int_predicate(pred, &lhs_int.ap_int(), &rhs_int.ap_int());
                return bool_constant_for_type(result_ty, result);
            }
            if let Some(relation) = evaluate_icmp_relation(lhs, rhs)
                && let Some(result) = relation_satisfies_predicate(relation, pred)
            {
                return bool_constant_for_type(result_ty, result);
            }
            let lhs_is_not_expr = !matches!(
                &lhs.as_value().data().kind,
                ValueKindData::Constant(ConstantData::Expr(_))
            );
            let rhs_is_expr = matches!(
                &rhs.as_value().data().kind,
                ValueKindData::Constant(ConstantData::Expr(_))
            );
            if (lhs_is_not_expr && rhs_is_expr)
                || (constant_is_null_value(lhs) && !constant_is_null_value(rhs))
            {
                return constant_fold_compare_instruction(
                    CmpPredicate::Int(swapped_int_predicate(pred)),
                    rhs,
                    lhs,
                );
            }
            Ok(None)
        }
        CmpPredicate::Float(pred) => {
            if lhs == rhs {
                match pred {
                    FloatPredicate::One => return bool_constant_for_type(result_ty, false),
                    FloatPredicate::Ueq => return bool_constant_for_type(result_ty, true),
                    _ => {}
                }
            }
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

fn evaluate_icmp_relation<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> Option<IntPredicate> {
    if lhs == rhs {
        return Some(IntPredicate::Eq);
    }
    if !matches!(lhs.ty().data(), TypeData::Pointer { .. }) {
        return None;
    }
    if constant_relation_complexity(lhs) < constant_relation_complexity(rhs) {
        return evaluate_icmp_relation(rhs, lhs).map(swapped_int_predicate);
    }
    if let Some(relation) = evaluate_gep_icmp_relation(lhs, rhs) {
        return Some(relation);
    }
    if let Some((lhs_function, _)) = block_address_info(lhs) {
        if let Some((rhs_function, _)) = block_address_info(rhs) {
            if lhs_function != rhs_function {
                return Some(IntPredicate::Ne);
            }
        } else if is_pointer_null(rhs) {
            return Some(IntPredicate::Ne);
        }
    }
    if let Some(lhs_global) = global_value_ref(lhs) {
        if let Some(rhs_global) = global_value_ref(rhs) {
            return are_globals_potentially_equal(lhs.as_value().module(), lhs_global, rhs_global);
        }
        if block_address_info(rhs).is_some() {
            return Some(IntPredicate::Ne);
        }
        if is_pointer_null(rhs)
            && !global_has_external_weak_linkage(lhs.as_value().module(), lhs_global)
            && !global_is_alias(lhs.as_value().module(), lhs_global)
        {
            return Some(IntPredicate::Ugt);
        }
    }
    None
}

fn evaluate_gep_icmp_relation<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> Option<IntPredicate> {
    let lhs_expr = gep_expr_data(lhs)?;
    let module = lhs.as_value().module();
    let lhs_base_global = gep_base_global(module, lhs_expr);

    if is_pointer_null(rhs) {
        if let Some(lhs_base_global) = lhs_base_global
            && !global_has_external_weak_linkage(module, lhs_base_global)
            && gep_expr_is_inbounds(lhs_expr)
        {
            return Some(IntPredicate::Ugt);
        }
        return None;
    }

    if let Some(rhs_global) = global_value_ref(rhs) {
        if let Some(lhs_global) = lhs_base_global
            && lhs_global != rhs_global
        {
            if gep_has_all_zero_indices(module, lhs_expr) {
                return are_globals_potentially_equal(module, lhs_global, rhs_global);
            }
            return None;
        }
        return None;
    }

    if let Some(rhs_expr) = gep_expr_data(rhs)
        && let (Some(lhs_global), Some(rhs_global)) =
            (lhs_base_global, gep_base_global(module, rhs_expr))
        && lhs_global != rhs_global
    {
        if gep_has_all_zero_indices(module, lhs_expr) && gep_has_all_zero_indices(module, rhs_expr)
        {
            return are_globals_potentially_equal(module, lhs_global, rhs_global);
        }
        return None;
    }

    None
}

fn gep_expr_data<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> Option<&'ctx ConstantExprData> {
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &constant.as_value().data().kind else {
        return None;
    };
    if expr.opcode == ConstantExprOpcode::GetElementPtr {
        Some(expr)
    } else {
        None
    }
}

fn gep_base_global<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    expr: &ConstantExprData,
) -> Option<ValueId> {
    let base = constant_from_id(module, *expr.operands.first()?)?;
    global_value_ref(base)
}

fn gep_has_all_zero_indices<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleView<'ctx, B>,
    expr: &ConstantExprData,
) -> bool {
    expr.operands
        .iter()
        .skip(1)
        .all(|id| constant_from_id(module, *id).is_some_and(|index| is_zero_int_constant(index)))
}

fn gep_expr_is_inbounds(expr: &ConstantExprData) -> bool {
    matches!(
        &expr.flags,
        ConstantExprFlags::Gep(flags) if flags.no_wrap().contains(GepNoWrapFlags::IN_BOUNDS)
    )
}

fn relation_satisfies_predicate(relation: IntPredicate, predicate: IntPredicate) -> Option<bool> {
    match relation {
        IntPredicate::Eq => Some(int_predicate_true_when_equal(predicate)),
        IntPredicate::Ne => match predicate {
            IntPredicate::Eq => Some(false),
            IntPredicate::Ne => Some(true),
            _ => None,
        },
        IntPredicate::Ult => match predicate {
            IntPredicate::Ult | IntPredicate::Ne | IntPredicate::Ule => Some(true),
            IntPredicate::Ugt | IntPredicate::Eq | IntPredicate::Uge => Some(false),
            _ => None,
        },
        IntPredicate::Ugt => match predicate {
            IntPredicate::Ugt | IntPredicate::Ne | IntPredicate::Uge => Some(true),
            IntPredicate::Ult | IntPredicate::Eq | IntPredicate::Ule => Some(false),
            _ => None,
        },
        IntPredicate::Ule => match predicate {
            IntPredicate::Ugt => Some(false),
            IntPredicate::Ult | IntPredicate::Ule => Some(true),
            _ => None,
        },
        IntPredicate::Uge => match predicate {
            IntPredicate::Ult => Some(false),
            IntPredicate::Ugt | IntPredicate::Uge => Some(true),
            _ => None,
        },
        IntPredicate::Slt => match predicate {
            IntPredicate::Slt | IntPredicate::Ne | IntPredicate::Sle => Some(true),
            IntPredicate::Sgt | IntPredicate::Eq | IntPredicate::Sge => Some(false),
            _ => None,
        },
        IntPredicate::Sgt => match predicate {
            IntPredicate::Sgt | IntPredicate::Ne | IntPredicate::Sge => Some(true),
            IntPredicate::Slt | IntPredicate::Eq | IntPredicate::Sle => Some(false),
            _ => None,
        },
        IntPredicate::Sle => match predicate {
            IntPredicate::Sgt => Some(false),
            IntPredicate::Slt | IntPredicate::Sle => Some(true),
            _ => None,
        },
        IntPredicate::Sge => match predicate {
            IntPredicate::Slt => Some(false),
            IntPredicate::Sgt | IntPredicate::Sge => Some(true),
            _ => None,
        },
    }
}

fn swapped_int_predicate(predicate: IntPredicate) -> IntPredicate {
    match predicate {
        IntPredicate::Eq => IntPredicate::Eq,
        IntPredicate::Ne => IntPredicate::Ne,
        IntPredicate::Ugt => IntPredicate::Ult,
        IntPredicate::Uge => IntPredicate::Ule,
        IntPredicate::Ult => IntPredicate::Ugt,
        IntPredicate::Ule => IntPredicate::Uge,
        IntPredicate::Sgt => IntPredicate::Slt,
        IntPredicate::Sge => IntPredicate::Sle,
        IntPredicate::Slt => IntPredicate::Sgt,
        IntPredicate::Sle => IntPredicate::Sge,
    }
}

fn constant_relation_complexity<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> u8 {
    match &constant.as_value().data().kind {
        ValueKindData::Constant(ConstantData::Expr(_)) => 3,
        ValueKindData::Constant(ConstantData::GlobalValueRef { .. })
        | ValueKindData::Function(_)
        | ValueKindData::GlobalAlias(_)
        | ValueKindData::GlobalIFunc(_)
        | ValueKindData::GlobalVariable(_) => 2,
        ValueKindData::Constant(ConstantData::BlockAddress { .. }) => 1,
        _ => 0,
    }
}

fn global_value_ref<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> Option<ValueId> {
    global_value_ref_from_id(constant.as_value().module(), constant.as_value().id())
}

fn global_value_ref_from_id<B: ModuleBrand>(
    module: ModuleView<'_, B>,
    id: ValueId,
) -> Option<ValueId> {
    match &module.context().value_data(id).kind {
        ValueKindData::Constant(ConstantData::GlobalValueRef { value }) => Some(*value),
        ValueKindData::Function(_)
        | ValueKindData::GlobalAlias(_)
        | ValueKindData::GlobalIFunc(_)
        | ValueKindData::GlobalVariable(_) => Some(id),
        _ => None,
    }
}

fn block_address_info<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> Option<(ValueId, ValueId)> {
    let ValueKindData::Constant(ConstantData::BlockAddress { function, block }) =
        &constant.as_value().data().kind
    else {
        return None;
    };
    Some((*function, *block))
}

fn is_pointer_null<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    matches!(
        &constant.as_value().data().kind,
        ValueKindData::Constant(ConstantData::PointerNull)
    )
}

fn are_globals_potentially_equal<B: ModuleBrand>(
    module: ModuleView<'_, B>,
    lhs: ValueId,
    rhs: ValueId,
) -> Option<IntPredicate> {
    if lhs == rhs {
        return Some(IntPredicate::Eq);
    }
    if global_is_alias(module, lhs) || global_is_alias(module, rhs) {
        return None;
    }
    if global_is_unsafe_for_equality(module, lhs) || global_is_unsafe_for_equality(module, rhs) {
        return None;
    }
    Some(IntPredicate::Ne)
}

fn global_is_unsafe_for_equality<B: ModuleBrand>(module: ModuleView<'_, B>, id: ValueId) -> bool {
    if global_linkage(module, id).is_some_and(linkage_is_interposable)
        || global_has_global_unnamed_addr(module, id)
    {
        return true;
    }
    match &module.context().value_data(id).kind {
        ValueKindData::GlobalVariable(data) => {
            !Type::new(data.value_type, module).is_sized()
                || type_is_empty(Type::new(data.value_type, module))
        }
        ValueKindData::GlobalAlias(_) => true,
        _ => false,
    }
}

pub(crate) fn linkage_is_interposable(linkage: Linkage) -> bool {
    matches!(
        linkage,
        Linkage::WeakAny | Linkage::LinkOnceAny | Linkage::Common | Linkage::ExternalWeak
    )
}

fn linkage_is_weak_for_linker(linkage: Linkage) -> bool {
    matches!(
        linkage,
        Linkage::WeakAny
            | Linkage::WeakODR
            | Linkage::LinkOnceAny
            | Linkage::LinkOnceODR
            | Linkage::Common
            | Linkage::ExternalWeak
    )
}

fn global_has_external_weak_linkage<B: ModuleBrand>(
    module: ModuleView<'_, B>,
    id: ValueId,
) -> bool {
    global_linkage(module, id).is_some_and(|linkage| linkage == Linkage::ExternalWeak)
}

fn global_linkage<B: ModuleBrand>(module: ModuleView<'_, B>, id: ValueId) -> Option<Linkage> {
    match &module.context().value_data(id).kind {
        ValueKindData::GlobalVariable(data) => Some(data.linkage.get()),
        ValueKindData::Function(data) => Some(*data.linkage.borrow()),
        ValueKindData::GlobalAlias(data) => Some(data.linkage.get()),
        ValueKindData::GlobalIFunc(data) => Some(data.linkage.get()),
        _ => None,
    }
}

fn global_has_global_unnamed_addr<B: ModuleBrand>(module: ModuleView<'_, B>, id: ValueId) -> bool {
    match &module.context().value_data(id).kind {
        ValueKindData::GlobalVariable(data) => data.unnamed_addr.get() == UnnamedAddr::Global,
        ValueKindData::Function(data) => *data.unnamed_addr.borrow() == UnnamedAddr::Global,
        ValueKindData::GlobalAlias(data) => data.unnamed_addr.get() == UnnamedAddr::Global,
        _ => false,
    }
}

fn global_is_alias<B: ModuleBrand>(module: ModuleView<'_, B>, id: ValueId) -> bool {
    matches!(
        &module.context().value_data(id).kind,
        ValueKindData::GlobalAlias(_)
    )
}

fn type_is_empty<B: ModuleBrand>(ty: Type<'_, B>) -> bool {
    match ty.data() {
        TypeData::Array { elem, n } => *n == 0 || type_is_empty(Type::new(*elem, ty.module())),
        TypeData::Struct(data) => data.body.borrow().as_ref().is_some_and(|body| {
            body.elements
                .iter()
                .all(|elem| type_is_empty(Type::new(*elem, ty.module())))
        }),
        _ => false,
    }
}

fn erase_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Type<'ctx> {
    Type::new(ty.id(), ModuleRef::new(ty.module().core_ref()))
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
    if constant_is_null_value(condition) {
        return Ok(Some(false_value));
    }
    if constant_is_all_ones_value(condition)? {
        return Ok(Some(true_value));
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
    if is_poison(vector) || is_undef_or_poison(index) {
        return Ok(Some(poison_for(element_ty)));
    }
    if is_undef(vector) {
        return Ok(Some(element_ty.get_undef().as_constant()));
    }
    let Ok(index_constant) = ConstantIntValue::<IntDyn, B>::try_from(index) else {
        return Ok(None);
    };
    let index_ap = index_constant.ap_int();
    let index_usize = if scalable {
        index_ap
            .try_zext_u64()
            .and_then(|raw_index| usize::try_from(raw_index).ok())
    } else {
        let limit = u64::from(lanes);
        let raw_index = index_ap.limited_value(limit);
        if raw_index == limit {
            return Ok(Some(poison_for(element_ty)));
        }
        Some(match usize::try_from(raw_index) {
            Ok(index) => index,
            Err(_) => return Ok(None),
        })
    };

    if let ValueKindData::Constant(ConstantData::Expr(expr)) = &vector.as_value().data().kind {
        match expr.opcode {
            ConstantExprOpcode::GetElementPtr => {
                let module = vector.as_value().module();
                let Some(source_ty) = expr.source_ty.map(|id| Type::new(id, module)) else {
                    return Ok(None);
                };
                let mut operands = Vec::with_capacity(expr.operands.len());
                for operand in expr.operands.iter().copied() {
                    let Some(operand) = constant_from_id(module, operand) else {
                        return Ok(None);
                    };
                    let operand = if operand.ty().data().as_vector().is_some() {
                        let Some(scalar) = constant_fold_extract_element_instruction(
                            operand,
                            index_constant.as_constant(),
                        )?
                        else {
                            return Ok(None);
                        };
                        scalar
                    } else {
                        operand
                    };
                    operands.push(operand.as_value());
                }
                return module
                    .core_ref()
                    .constant_expr_with_options(
                        element_ty,
                        ConstantExprOpcode::GetElementPtr,
                        operands,
                        [],
                        [],
                        ConstantExprOptions::new()
                            .source_ty(source_ty)
                            .flags(expr.flags.clone()),
                    )
                    .map(Some);
            }
            ConstantExprOpcode::InsertElement => {
                let [base, inserted, insert_index] = expr.operands.as_ref() else {
                    return Ok(None);
                };
                let module = vector.as_value().module();
                let Some(insert_index) = constant_from_id(module, *insert_index) else {
                    return Ok(None);
                };
                let Ok(insert_index) = ConstantIntValue::<IntDyn, B>::try_from(insert_index) else {
                    return Ok(None);
                };
                let Some(inserted) = constant_from_id(module, *inserted) else {
                    return Ok(None);
                };
                if constant_int_same_unsigned_value(insert_index, index_constant) {
                    return Ok(Some(inserted));
                }
                let Some(base) = constant_from_id(module, *base) else {
                    return Ok(None);
                };
                return base
                    .as_value()
                    .module()
                    .core_ref()
                    .constant_expr(
                        element_ty,
                        ConstantExprOpcode::ExtractElement,
                        [base.as_value(), index_constant.as_value()],
                        [],
                        [],
                        ConstantExprFlags::none(),
                    )
                    .map(Some);
            }
            _ => {}
        }
    }

    let Some(index_usize) = index_usize else {
        return Ok(None);
    };
    let Some(elements) = aggregate_elements(vector) else {
        return Ok(None);
    };
    let Some(element) = elements.get(index_usize).copied() else {
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
    if is_undef_or_poison(index) {
        return Ok(Some(poison_for(vector.ty())));
    }
    if constant_is_null_value(vector) && constant_is_null_value(value) {
        return Ok(Some(vector));
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
    if rhs.ty() != lhs.ty() {
        return Ok(None);
    }
    let element_ty = Type::new(element_ty, lhs.as_value().module());
    let Ok(result_lanes) = u32::try_from(mask.len()) else {
        return Ok(None);
    };
    let result_ty = lhs
        .as_value()
        .module()
        .vector_type(element_ty, result_lanes, scalable)
        .as_type();
    if mask.iter().all(|element| *element == POISON_MASK_ELEM) {
        return Ok(Some(poison_for(result_ty)));
    }
    if mask.iter().all(|element| *element == 0) {
        let index = lhs
            .as_value()
            .module()
            .i32_type()
            .const_zero()
            .as_constant();
        if let Some(element) = constant_fold_extract_element_instruction(lhs, index)?
            && (!scalable || constant_is_null_value(element) || is_undef_or_poison(element))
        {
            return vector_splat_constant(result_ty, element);
        }
    }
    if scalable {
        return Ok(None);
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
            result.push(element_ty.get_undef().as_constant());
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
pub fn shufflevector_mask_from_constant<'ctx, B: ModuleBrand + 'ctx>(
    mask: Constant<'ctx, B>,
) -> Option<Vec<i32>> {
    let (_, lanes, scalable) = mask.ty().data().as_vector()?;
    let lane_count = usize::try_from(lanes).ok()?;
    if is_undef_or_poison(mask) {
        return Some(vec![POISON_MASK_ELEM; lane_count]);
    }
    if constant_is_null_value(mask) {
        return Some(vec![0; lane_count]);
    }
    if scalable {
        return None;
    }
    let elements = aggregate_elements(mask)?;
    if elements.len() != lane_count {
        return None;
    }
    elements
        .into_iter()
        .map(|element| {
            if is_undef_or_poison(element) {
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
        let Some(elements) = aggregate_elements_for_rebuild(current)? else {
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
    in_range: Option<&ConstantExprInRange>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if indices.is_empty() {
        return Ok(Some(pointer));
    }

    let result_ty = match gep_result_type(pointer.ty(), indices) {
        Ok(ty) => ty,
        Err(IrError::InvalidOperation { .. }) => return Ok(None),
        Err(err) => return Err(err),
    };

    if is_poison(pointer) {
        return Ok(Some(poison_for(result_ty)));
    }
    if is_undef(pointer) {
        return Ok(Some(result_ty.get_undef().as_constant()));
    }

    let is_noop = in_range.is_none()
        && indices
            .iter()
            .copied()
            .all(|index| constant_is_null_value(index) || is_undef_or_poison(index));
    if is_noop {
        if result_ty.data().as_vector().is_some() && pointer.ty().data().as_vector().is_none() {
            return vector_splat_constant(result_ty, pointer);
        }
        return Ok(Some(pointer));
    }

    Ok(None)
}

pub(crate) fn gep_result_type<'ctx, B: ModuleBrand + 'ctx>(
    pointer_ty: Type<'ctx, B>,
    indices: &[Constant<'ctx, B>],
) -> IrResult<Type<'ctx, B>> {
    let Some(addr_space) = pointer_address_space(pointer_ty) else {
        return Err(IrError::InvalidOperation {
            message: "invalid getelementptr constant expression",
        });
    };
    let scalar_ptr_ty = pointer_ty.module().ptr_type(addr_space).as_type();
    let Some((lanes, scalable)) = gep_operand_vector_shape(pointer_ty, indices)? else {
        return Ok(scalar_ptr_ty);
    };
    Ok(pointer_ty
        .module()
        .vector_type(scalar_ptr_ty, lanes, scalable)
        .as_type())
}

fn gep_operand_vector_shape<'ctx, B: ModuleBrand + 'ctx>(
    pointer_ty: Type<'ctx, B>,
    indices: &[Constant<'ctx, B>],
) -> IrResult<Option<(u32, bool)>> {
    let mut shape = pointer_ty
        .data()
        .as_vector()
        .map(|(_, lanes, scalable)| (lanes, scalable));
    for index in indices {
        let Some(index_shape) = index
            .ty()
            .data()
            .as_vector()
            .map(|(_, lanes, scalable)| (lanes, scalable))
        else {
            continue;
        };
        match shape {
            Some(current) if current != index_shape => {
                return Err(IrError::InvalidOperation {
                    message: "invalid getelementptr constant expression",
                });
            }
            Some(_) => {}
            None => shape = Some(index_shape),
        }
    }
    Ok(shape)
}

fn pointer_address_space<B: ModuleBrand>(ty: Type<'_, B>) -> Option<u32> {
    match ty.data() {
        TypeData::Pointer { addr_space } => Some(*addr_space),
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            pointer_address_space(Type::new(*elem, ty.module()))
        }
        _ => None,
    }
}

fn constant_is_null_value<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    match &constant.as_value().data().kind {
        ValueKindData::Constant(ConstantData::Int(_)) => is_zero_int_constant(constant),
        ValueKindData::Constant(ConstantData::Float(_)) => {
            ConstantFloatValue::<FloatDyn, B>::try_from(constant)
                .is_ok_and(|value| value.ap_float().is_pos_zero())
        }
        ValueKindData::Constant(ConstantData::PointerNull) => true,
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            let module = constant.as_value().module();
            elements.iter().all(|id| {
                let data = module.context().value_data(*id);
                constant_is_null_value(Constant::from_parts(Value::from_parts(
                    *id, module, data.ty,
                )))
            })
        }
        _ => false,
    }
}

fn vector_splat_constant<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    scalar: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Ok(vector_ty) = VectorType::<ElemDyn, LenDyn, _>::try_from(ty) else {
        return Ok(None);
    };
    if vector_ty.element() != scalar.ty() {
        return Ok(None);
    }
    let Ok(lane_count) = usize::try_from(vector_ty.min_len()) else {
        return Ok(None);
    };
    vector_ty
        .const_vector((0..lane_count).map(|_| scalar))
        .map(|aggregate| Some(aggregate.as_constant()))
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
    if !is_scalar_int_or_scalable_int_vector_type(lhs.ty()) {
        return Ok(None);
    }
    let Some(zero_constant) = null_constant_for_type(lhs.ty())? else {
        return Ok(None);
    };
    let Some(all_ones_constant) = all_ones_constant_for_type(lhs.ty())? else {
        return Ok(None);
    };
    let undef = || lhs.ty().get_undef().as_constant();
    let zero = || zero_constant;
    let all_ones = || all_ones_constant;
    let poison = || poison_for(lhs.ty());

    let folded = match opcode {
        BinaryOpcode::Xor if lhs_undef && rhs_undef => zero(),
        BinaryOpcode::Add | BinaryOpcode::Sub | BinaryOpcode::Xor => undef(),
        BinaryOpcode::And if lhs_undef && rhs_undef => undef(),
        BinaryOpcode::And => zero(),
        BinaryOpcode::Mul if lhs_undef && rhs_undef => undef(),
        BinaryOpcode::Mul => {
            let known = if lhs_undef { rhs } else { lhs };
            if constant_is_one_bit_set_low(known) {
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

fn is_scalar_int_or_scalable_int_vector_type<B: ModuleBrand>(ty: Type<'_, B>) -> bool {
    if IntType::<IntDyn, B>::try_from(ty).is_ok() {
        return true;
    }
    let TypeData::ScalableVector { elem, .. } = ty.data() else {
        return false;
    };
    IntType::<IntDyn, B>::try_from(Type::new(*elem, ty.module())).is_ok()
}

fn is_i1_or_i1_vector_type<B: ModuleBrand>(ty: Type<'_, B>) -> bool {
    match ty.data() {
        TypeData::Integer { bits } => *bits == 1,
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            matches!(
                Type::new(*elem, ty.module()).data(),
                TypeData::Integer { bits } if *bits == 1
            )
        }
        _ => false,
    }
}

fn constant_not<'ctx, B: ModuleBrand + 'ctx>(
    value: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some(all_ones) = all_ones_constant_for_type(value.ty())? else {
        return Ok(None);
    };
    build_binary_constant_or_expr(BinaryOpcode::Xor, value, all_ones)
}

fn constant_is_one_bit_set_low<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    if let Ok(value) = ConstantIntValue::<IntDyn, B>::try_from(constant) {
        return value.ap_int().is_one_bit_set(0);
    }
    aggregate_elements(constant).is_some_and(|elements| {
        !elements.is_empty()
            && elements
                .iter()
                .all(|element| constant_is_one_bit_set_low(*element))
    })
}

fn fold_i1_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if !matches!(lhs.ty().data(), TypeData::Integer { bits: 1 }) {
        return Ok(None);
    }
    match opcode {
        BinaryOpcode::Add | BinaryOpcode::Sub => {
            build_binary_constant_or_expr(BinaryOpcode::Xor, lhs, rhs)
        }
        BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::UDiv
        | BinaryOpcode::SDiv => Ok(Some(lhs)),
        BinaryOpcode::URem | BinaryOpcode::SRem => null_constant_for_type(lhs.ty()),
        BinaryOpcode::Mul
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::Xor
        | BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => Ok(None),
    }
}

fn fold_global_pointer_and_mask<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if !matches!(opcode, BinaryOpcode::And) {
        return Ok(None);
    }
    let (pointer, mask) = if let Ok(mask) = ConstantIntValue::<IntDyn, B>::try_from(rhs)
        && ptr_to_int_global_operand(lhs).is_some()
    {
        (lhs, mask)
    } else if let Ok(mask) = ConstantIntValue::<IntDyn, B>::try_from(lhs)
        && ptr_to_int_global_operand(rhs).is_some()
    {
        (rhs, mask)
    } else {
        return Ok(None);
    };
    let Some(global_id) = ptr_to_int_global_operand(pointer) else {
        return Ok(None);
    };
    let Some(align) = global_pointer_alignment(pointer.as_value().module(), global_id) else {
        return Ok(None);
    };
    if align.value() <= 1 {
        return Ok(None);
    }
    let dst_width = mask.bit_width();
    let src_width = dst_width.min(align.value().trailing_zeros());
    let bits_not_set = ApInt::low_bits_set(dst_width, src_width);
    if mask
        .ap_int()
        .bitand(&bits_not_set)
        .eq_ap_int(&mask.ap_int())
    {
        return Ok(Some(mask.ty().const_zero().as_constant()));
    }
    Ok(None)
}

fn ptr_to_int_global_operand<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> Option<ValueId> {
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &constant.as_value().data().kind else {
        return None;
    };
    if !matches!(
        expr.opcode,
        ConstantExprOpcode::PtrToInt | ConstantExprOpcode::PtrToAddr
    ) {
        return None;
    }
    let [operand] = expr.operands.as_ref() else {
        return None;
    };
    let module = constant.as_value().module();
    global_value_ref_from_id(module, *operand)
}

fn global_pointer_alignment<B: ModuleBrand>(
    module: ModuleView<'_, B>,
    global_id: ValueId,
) -> Option<Align> {
    match &module.context().value_data(global_id).kind {
        ValueKindData::Function(data) => {
            let explicit = data.align.borrow().align().unwrap_or(Align::ONE);
            let dl = module.data_layout();
            if let Some(function_ptr_align) = dl.function_ptr_align() {
                return Some(match dl.function_ptr_align_type() {
                    FunctionPtrAlignType::Independent => function_ptr_align,
                    FunctionPtrAlignType::MultipleOfFunctionAlign => {
                        function_ptr_align.max(explicit)
                    }
                });
            }
            Align::new(4).ok()
        }
        ValueKindData::GlobalVariable(data) => {
            if let Some(explicit) = data.align.get().align() {
                return Some(explicit);
            }
            let value_ty = Type::new(data.value_type, module);
            if !value_ty.is_sized() {
                return Some(Align::ONE);
            }
            let dl = module.data_layout();
            let linkage = data.linkage.get();
            let has_initializer = data.initializer.get().is_some();
            if has_initializer
                && linkage != Linkage::AvailableExternally
                && !linkage_is_weak_for_linker(linkage)
            {
                let mut alignment = dl.pref_type_align(erase_type(value_ty));
                let large_alignment = Align::new(16).ok()?;
                if alignment < large_alignment && dl.type_size_in_bits(erase_type(value_ty)) > 128 {
                    alignment = large_alignment;
                }
                Some(alignment)
            } else {
                Some(dl.abi_type_align(erase_type(value_ty)))
            }
        }
        _ => None,
    }
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

fn fold_same_lane_vector_cast<'ctx, B: ModuleBrand + 'ctx>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Some((src_element_ty, lanes, src_scalable)) = operand.ty().data().as_vector() else {
        return Ok(None);
    };
    let Some((dest_element_ty, dest_lanes, dest_scalable)) = dest_ty.data().as_vector() else {
        return Ok(None);
    };
    if lanes != dest_lanes || src_scalable != dest_scalable {
        return Ok(None);
    }
    let src_element_ty = Type::new(src_element_ty, operand.as_value().module());
    let dest_element_ty = Type::new(dest_element_ty, operand.as_value().module());
    if let Some(splat) = constant_splat_value(operand) {
        let Some(folded) = fold_maybe_undesirable_cast(opcode, splat, dest_element_ty)? else {
            return Ok(None);
        };
        return vector_splat_constant(dest_ty, folded);
    }
    if src_scalable {
        return Ok(None);
    }
    let Some(elements) = fixed_vector_elements_for_rebuild(operand, lanes, src_element_ty)? else {
        return Ok(None);
    };
    let Ok(lane_count) = usize::try_from(lanes) else {
        return Ok(None);
    };
    let mut folded = Vec::with_capacity(lane_count);
    for element in elements {
        let Some(result) = fold_maybe_undesirable_cast(opcode, element, dest_element_ty)? else {
            return Ok(None);
        };
        folded.push(result);
    }
    constant_aggregate_from_elements(dest_ty, folded)
}

fn fold_bitcast<'ctx, B: ModuleBrand + 'ctx>(
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if operand.ty() == dest_ty {
        return Ok(Some(operand));
    }
    if constant_is_all_ones_value(operand)?
        && let Some(all_ones) = all_ones_constant_for_type(dest_ty)?
    {
        return Ok(Some(all_ones));
    }
    if dest_ty.data().as_vector().is_some()
        && operand.ty().data().as_vector().is_none()
        && (ConstantIntValue::<IntDyn, B>::try_from(operand).is_ok()
            || ConstantFloatValue::<FloatDyn, B>::try_from(operand).is_ok())
    {
        let vector_ty = operand
            .as_value()
            .module()
            .vector_type(operand.ty(), 1, false);
        let vector = vector_ty.const_vector::<Constant<'ctx, B>, _>([operand])?;
        return match operand.as_value().module().core_ref().constant_expr(
            dest_ty,
            ConstantExprOpcode::BitCast,
            [vector.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        ) {
            Ok(folded) => Ok(Some(folded)),
            Err(IrError::InvalidOperation { .. }) => Ok(None),
            Err(err) => Err(err),
        };
    }
    if let Some(folded) = fold_vector_bitcast_splat(operand, dest_ty)? {
        return Ok(Some(folded));
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

fn fold_vector_bitcast_splat<'ctx, B: ModuleBrand + 'ctx>(
    operand: Constant<'ctx, B>,
    dest_ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let Ok(dst_vec_ty) = VectorType::<ElemDyn, LenDyn, _>::try_from(dest_ty) else {
        return Ok(None);
    };
    let dst_elem_ty = dst_vec_ty.element();
    let Some(elements) = aggregate_elements(operand) else {
        return Ok(None);
    };
    let mut pattern = None;
    let mut source_bits = 0_u64;
    for element in elements {
        let Ok(int_value) = ConstantIntValue::<IntDyn, B>::try_from(element) else {
            return Ok(None);
        };
        source_bits = source_bits.saturating_add(u64::from(int_value.bit_width()));
        let element_pattern = if int_value.ap_int().is_zero() {
            VectorBitcastSplat::Zero
        } else if int_value.ap_int().is_all_ones() {
            VectorBitcastSplat::AllOnes
        } else {
            return Ok(None);
        };
        match pattern {
            Some(current) if current != element_pattern => return Ok(None),
            Some(_) => {}
            None => pattern = Some(element_pattern),
        }
    }
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    let Some(dst_elem_bits) = type_bit_width_for_bitcast(dst_elem_ty) else {
        return Ok(None);
    };
    if source_bits != u64::from(dst_elem_bits) * u64::from(dst_vec_ty.min_len()) {
        return Ok(None);
    }
    let Some(scalar) = (match pattern {
        VectorBitcastSplat::Zero => null_constant_for_type(dst_elem_ty)?,
        VectorBitcastSplat::AllOnes => all_ones_constant_for_type(dst_elem_ty)?,
    }) else {
        return Ok(None);
    };
    let Ok(lane_count) = usize::try_from(dst_vec_ty.min_len()) else {
        return Ok(None);
    };
    dst_vec_ty
        .const_vector((0..lane_count).map(|_| scalar))
        .map(|aggregate| Some(aggregate.as_constant()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VectorBitcastSplat {
    Zero,
    AllOnes,
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
    if let Ok(ptr_ty) = PointerType::try_from(ty) {
        return Ok(Some(ptr_ty.const_null().as_constant()));
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
    let (float_ty, vector_ty) = if let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(lhs.ty()) {
        (float_ty, None)
    } else {
        let TypeData::ScalableVector { elem, .. } = lhs.ty().data() else {
            return Ok(None);
        };
        let element_ty = Type::new(*elem, lhs.as_value().module());
        let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(element_ty) else {
            return Ok(None);
        };
        (float_ty, Some(lhs.ty()))
    };
    let undef = || lhs.ty().get_undef().as_constant();
    let nan_scalar = || {
        let value = crate::ApFloat::qnan(
            float_ty.semantics(),
            ApFloatSign::Positive,
            NanPayload::Absent,
        );
        float_ty
            .const_ap_float(&value)
            .map(|value| value.as_constant())
    };
    let nan = || {
        let scalar = nan_scalar()?;
        match vector_ty {
            Some(ty) => vector_splat_constant(ty, scalar)?.ok_or(IrError::InvalidOperation {
                message: "invalid vector NaN constant",
            }),
            None => Ok(scalar),
        }
    };

    let folded = match opcode {
        BinaryOpcode::FSub if rhs_undef && constant_matches_negative_zero_fp_pattern(lhs) => {
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
    let vector_ty = VectorType::<ElemDyn, LenDyn, _>::try_from(ty)?;
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

    let Ok(lane_count) = usize::try_from(lanes) else {
        return Ok(None);
    };
    let fill = if is_undef(vector) {
        Some(element_ty.get_undef().as_constant())
    } else if is_poison(vector) {
        Some(poison_for(element_ty))
    } else {
        None
    };
    if let Some(fill) = fill {
        return Ok(Some((0..lane_count).map(|_| fill).collect()));
    }

    let i32_ty = vector.as_value().module().i32_type();
    let mut elements = Vec::with_capacity(lane_count);
    for index in 0..lanes {
        let Ok(index) = i32::try_from(index) else {
            return Ok(None);
        };
        let index = i32_ty.const_int(index).as_constant();
        if let Some(folded) = constant_fold_extract_element_instruction(vector, index)? {
            elements.push(folded);
        } else {
            let expr = vector.as_value().module().core_ref().constant_expr(
                element_ty,
                ConstantExprOpcode::ExtractElement,
                [vector.as_value(), index.as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )?;
            elements.push(expr);
        }
    }
    Ok(Some(elements))
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
    if let Ok(array_ty) = ArrayType::<ElemDyn, ArrLenDyn, _>::try_from(aggregate.ty()) {
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
fn type_bit_width_for_bitcast<B: ModuleBrand>(ty: Type<'_, B>) -> Option<u32> {
    if let Ok(int_ty) = IntType::<IntDyn, B>::try_from(ty) {
        return Some(int_ty.bit_width());
    }
    if let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(ty) {
        return Some(float_ty.semantics().bit_width());
    }
    None
}

fn all_ones_constant_for_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(int_ty) = IntType::<IntDyn, B>::try_from(ty) {
        return Ok(Some(int_ty.const_all_ones().as_constant()));
    }
    if let Ok(float_ty) = FloatType::<FloatDyn, B>::try_from(ty) {
        let bits = ApInt::all_ones(float_ty.semantics().bit_width());
        let value = crate::ApFloat::from_bits(float_ty.semantics(), &bits)?;
        return Ok(Some(float_ty.const_ap_float(&value)?.as_constant()));
    }
    if let Some((element_ty, lanes, scalable)) = ty.data().as_vector() {
        let module = ty.module();
        let element_ty = Type::new(element_ty, module);
        let Some(element) = all_ones_constant_for_type(element_ty)? else {
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

fn is_not_poison_for_select<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    match &constant.as_value().data().kind {
        ValueKindData::Constant(
            ConstantData::Int(_)
            | ConstantData::Float(_)
            | ConstantData::GlobalValueRef { .. }
            | ConstantData::PointerNull,
        )
        | ValueKindData::Function(_)
        | ValueKindData::GlobalVariable(_) => true,
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
fn constant_splat_value<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> Option<Constant<'ctx, B>> {
    let elements = aggregate_elements(constant)?;
    let first = elements.first().copied()?;
    elements
        .iter()
        .all(|element| *element == first)
        .then_some(first)
}

fn constant_aggregate_from_elements<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    elements: Vec<Constant<'ctx, B>>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if let Ok(vector_ty) = VectorType::<ElemDyn, LenDyn, _>::try_from(ty) {
        return vector_ty
            .const_vector::<Constant<'ctx, B>, _>(elements)
            .map(|aggregate| Some(aggregate.as_constant()));
    }
    if let Ok(array_ty) = ArrayType::<ElemDyn, ArrLenDyn, _>::try_from(ty) {
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

fn constant_int_same_unsigned_value<'ctx, B: ModuleBrand + 'ctx>(
    lhs: ConstantIntValue<'ctx, IntDyn, B>,
    rhs: ConstantIntValue<'ctx, IntDyn, B>,
) -> bool {
    let width = lhs.bit_width().max(rhs.bit_width());
    lhs.ap_int()
        .zext_or_trunc(width)
        .eq_ap_int(&rhs.ap_int().zext_or_trunc(width))
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

fn is_undef_or_poison<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    is_undef(constant) || is_poison(constant)
}

fn binop_identity<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    let folded = match opcode {
        BinaryOpcode::Add | BinaryOpcode::Or | BinaryOpcode::Xor => {
            if constant_is_null_value(lhs) {
                Some(rhs)
            } else if constant_is_null_value(rhs) {
                Some(lhs)
            } else {
                None
            }
        }
        BinaryOpcode::Mul => {
            if constant_is_one_value(lhs)? {
                Some(rhs)
            } else if constant_is_one_value(rhs)? {
                Some(lhs)
            } else {
                None
            }
        }
        BinaryOpcode::And => {
            if constant_is_all_ones_value(lhs)? {
                Some(rhs)
            } else if constant_is_all_ones_value(rhs)? {
                Some(lhs)
            } else {
                None
            }
        }
        BinaryOpcode::FAdd => {
            if constant_is_float_negative_zero(lhs) {
                Some(rhs)
            } else if constant_is_float_negative_zero(rhs) {
                Some(lhs)
            } else {
                None
            }
        }
        BinaryOpcode::FMul => {
            if constant_is_one_value(lhs)? {
                Some(rhs)
            } else if constant_is_one_value(rhs)? {
                Some(lhs)
            } else {
                None
            }
        }
        BinaryOpcode::Sub
        | BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::FSub
            if constant_is_null_value(rhs) =>
        {
            Some(lhs)
        }
        BinaryOpcode::UDiv | BinaryOpcode::SDiv | BinaryOpcode::FDiv
            if constant_is_one_value(rhs)? =>
        {
            Some(lhs)
        }
        BinaryOpcode::URem | BinaryOpcode::SRem | BinaryOpcode::FRem => None,
        BinaryOpcode::Sub
        | BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::FSub
        | BinaryOpcode::UDiv
        | BinaryOpcode::SDiv
        | BinaryOpcode::FDiv => None,
    };
    Ok(folded)
}

fn binop_rhs_absorber<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    rhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if ConstantIntValue::<IntDyn, B>::try_from(rhs).is_err() {
        return Ok(None);
    }
    let folded = match opcode {
        BinaryOpcode::Or if constant_is_all_ones_value(rhs)? => Some(rhs),
        BinaryOpcode::And | BinaryOpcode::Mul if constant_is_null_value(rhs) => Some(rhs),
        _ => None,
    };
    Ok(folded)
}

fn binop_lhs_absorber<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
) -> IrResult<Option<Constant<'ctx, B>>> {
    if ConstantIntValue::<IntDyn, B>::try_from(lhs).is_err() {
        return Ok(None);
    }
    let folded = match opcode {
        BinaryOpcode::Or if constant_is_all_ones_value(lhs)? => Some(lhs),
        BinaryOpcode::And
        | BinaryOpcode::Mul
        | BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::SDiv
        | BinaryOpcode::UDiv
        | BinaryOpcode::URem
        | BinaryOpcode::SRem
            if constant_is_null_value(lhs) =>
        {
            Some(lhs)
        }
        _ => None,
    };
    Ok(folded)
}

fn constant_is_one_value<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> IrResult<bool> {
    if let Ok(value) = ConstantIntValue::<IntDyn, B>::try_from(constant) {
        return Ok(value.ap_int().is_one());
    }
    if let Ok(value) = ConstantFloatValue::<FloatDyn, B>::try_from(constant) {
        let one = crate::ApFloat::one(value.ap_float().semantics(), ApFloatSign::Positive);
        return Ok(matches!(
            value.ap_float().compare(&one),
            ApFloatCmpResult::Equal
        ));
    }
    let Some(elements) = aggregate_elements(constant) else {
        return Ok(false);
    };
    for element in elements {
        if !constant_is_one_value(element)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn constant_is_all_ones_value<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> IrResult<bool> {
    if let Ok(value) = ConstantIntValue::<IntDyn, B>::try_from(constant) {
        return Ok(value.ap_int().is_all_ones());
    }
    if let Ok(value) = ConstantFloatValue::<FloatDyn, B>::try_from(constant) {
        return Ok(value.ap_float().to_bits().is_all_ones());
    }
    let Some(elements) = aggregate_elements(constant) else {
        return Ok(false);
    };
    for element in elements {
        if !constant_is_all_ones_value(element)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn constant_is_float_negative_zero<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> bool {
    if let Ok(value) = ConstantFloatValue::<FloatDyn, B>::try_from(constant) {
        return value.ap_float().is_neg_zero();
    }
    aggregate_elements(constant).is_some_and(|elements| {
        !elements.is_empty()
            && elements
                .iter()
                .copied()
                .all(constant_is_float_negative_zero)
    })
}

fn constant_matches_negative_zero_fp_pattern<'ctx, B: ModuleBrand + 'ctx>(
    constant: Constant<'ctx, B>,
) -> bool {
    if let Ok(value) = ConstantFloatValue::<FloatDyn, B>::try_from(constant) {
        return value.ap_float().is_neg_zero();
    }
    let Some(elements) = aggregate_elements(constant) else {
        return false;
    };
    let mut has_negative_zero = false;
    for element in elements {
        if is_poison(element) {
            continue;
        }
        if !constant_matches_negative_zero_fp_pattern(element) {
            return false;
        }
        has_negative_zero = true;
    }
    has_negative_zero
}

fn is_zero_int_constant<'ctx, B: ModuleBrand + 'ctx>(constant: Constant<'ctx, B>) -> bool {
    ConstantIntValue::<IntDyn, B>::try_from(constant).is_ok_and(|value| value.ap_int().is_zero())
}

fn poison_for<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Constant<'ctx, B> {
    ty.get_poison().as_constant()
}
