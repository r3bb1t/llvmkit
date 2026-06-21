//! Default IR-builder folder. Mirrors
//! `llvm/include/llvm/IR/ConstantFolder.h`.
//!
//! The default strategy folds only all-constant inputs. It delegates target-
//! independent arithmetic to [`crate::constant_fold`] and uses the supported
//! `ConstantExpr` constructors for LLVM's desirable constant-expression groups.

use crate::cmp_predicate::CmpPredicate;
use crate::constant::{Constant, ConstantExprFlags, ConstantExprOpcode};
use crate::constant_fold::{
    constant_fold_binary_instruction, constant_fold_cast_instruction,
    constant_fold_compare_instruction, constant_fold_exact_binary_instruction,
    constant_fold_extract_element_instruction, constant_fold_extract_value_instruction,
    constant_fold_get_element_ptr, constant_fold_insert_element_instruction,
    constant_fold_insert_value_instruction, constant_fold_select_instruction,
    constant_fold_shuffle_vector_instruction, constant_fold_unary_instruction,
};
use crate::constants::ConstantExprOptions;
use crate::fmf::FastMathFlags;
use crate::gep_no_wrap_flags::GepNoWrapFlags;
use crate::instr_types::{BinaryOpcode, CastOpcode, POISON_MASK_ELEM, UnaryOpcode};
use crate::instruction::Instruction;
use crate::intrinsics::IntrinsicId;
use crate::ir_builder::folder::IRBuilderFolder;
use crate::r#type::{Type, TypeData};
use crate::value::Value;
use crate::{IrError, IrResult};

/// Default fold strategy: fold target-independent constant-on-constant
/// operations and decline non-constant inputs.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConstantFolder;

impl<'ctx> IRBuilderFolder<'ctx> for ConstantFolder {
    fn fold_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        fold_binary(opcode, lhs, rhs, ConstantExprFlags::none())
    }

    fn fold_exact_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        is_exact: bool,
    ) -> IrResult<Option<Value<'ctx>>> {
        fold_exact_binary(opcode, lhs, rhs, is_exact)
    }

    fn fold_no_wrap_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        has_nuw: bool,
        has_nsw: bool,
    ) -> IrResult<Option<Value<'ctx>>> {
        if !matches!(
            opcode,
            BinaryOpcode::Add | BinaryOpcode::Sub | BinaryOpcode::Mul | BinaryOpcode::Shl
        ) {
            return Ok(None);
        }
        fold_binary(
            opcode,
            lhs,
            rhs,
            ConstantExprFlags::overflowing(has_nuw, has_nsw),
        )
    }

    fn fold_bin_op_fmf(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold_bin_op(opcode, lhs, rhs)
    }

    fn fold_un_op_fmf(
        &self,
        opcode: UnaryOpcode,
        value: Value<'ctx>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        let value = match Constant::try_from(value) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        constant_fold_unary_instruction(opcode, value).map(|folded| folded.map(Constant::as_value))
    }

    fn fold_cmp(
        &self,
        predicate: CmpPredicate,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        let (lhs, rhs) = match constants2(lhs, rhs) {
            Some(values) => values,
            None => return Ok(None),
        };
        constant_fold_compare_instruction(predicate, lhs, rhs)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_gep(
        &self,
        source_ty: Type<'ctx>,
        ptr: Value<'ctx>,
        indices: &[Value<'ctx>],
        no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        if type_contains_scalable_vector(source_ty) {
            return Ok(None);
        }
        let ptr = match Constant::try_from(ptr) {
            Ok(ptr) => ptr,
            Err(_) => return Ok(None),
        };
        let mut index_constants = Vec::with_capacity(indices.len());
        let mut operands = Vec::with_capacity(indices.len().saturating_add(1));
        operands.push(ptr.as_value());
        for index in indices {
            let index = match Constant::try_from(*index) {
                Ok(index) => index,
                Err(_) => return Ok(None),
            };
            operands.push(index.as_value());
            index_constants.push(index);
        }
        if let Some(folded) = constant_fold_get_element_ptr(source_ty, ptr, &index_constants)? {
            return Ok(Some(folded.as_value()));
        }
        let module = ptr.as_value().module().core_ref();
        module
            .constant_expr_with_options(
                ptr.ty(),
                ConstantExprOpcode::GetElementPtr,
                operands,
                [],
                [],
                ConstantExprOptions::new()
                    .source_ty(source_ty)
                    .flags(ConstantExprFlags::gep(no_wrap)),
            )
            .map(|folded| Some(folded.as_value()))
    }

    fn fold_select(
        &self,
        cond: Value<'ctx>,
        true_value: Value<'ctx>,
        false_value: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        let cond = match Constant::try_from(cond) {
            Ok(cond) => cond,
            Err(_) => return Ok(None),
        };
        let (true_value, false_value) = match constants2(true_value, false_value) {
            Some(values) => values,
            None => return Ok(None),
        };
        constant_fold_select_instruction(cond, true_value, false_value)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_extract_value(
        &self,
        aggregate: Value<'ctx>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>> {
        let aggregate = match Constant::try_from(aggregate) {
            Ok(aggregate) => aggregate,
            Err(_) => return Ok(None),
        };
        constant_fold_extract_value_instruction(aggregate, indices)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_insert_value(
        &self,
        aggregate: Value<'ctx>,
        value: Value<'ctx>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>> {
        let (aggregate, value) = match constants2(aggregate, value) {
            Some(values) => values,
            None => return Ok(None),
        };
        constant_fold_insert_value_instruction(aggregate, value, indices)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_extract_element(
        &self,
        vector: Value<'ctx>,
        index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        let (vector, index) = match constants2(vector, index) {
            Some(values) => values,
            None => return Ok(None),
        };
        if let Some(folded) = constant_fold_extract_element_instruction(vector, index)? {
            return Ok(Some(folded.as_value()));
        }
        let Some(result_ty) = vector_element_type(vector.ty()) else {
            return Ok(None);
        };
        vector
            .as_value()
            .module()
            .core_ref()
            .constant_expr(
                result_ty,
                ConstantExprOpcode::ExtractElement,
                [vector.as_value(), index.as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )
            .map(|folded| Some(folded.as_value()))
    }

    fn fold_insert_element(
        &self,
        vector: Value<'ctx>,
        new_element: Value<'ctx>,
        index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        let vector = match Constant::try_from(vector) {
            Ok(vector) => vector,
            Err(_) => return Ok(None),
        };
        let new_element = match Constant::try_from(new_element) {
            Ok(new_element) => new_element,
            Err(_) => return Ok(None),
        };
        let index = match Constant::try_from(index) {
            Ok(index) => index,
            Err(_) => return Ok(None),
        };
        if let Some(folded) = constant_fold_insert_element_instruction(vector, new_element, index)?
        {
            return Ok(Some(folded.as_value()));
        }
        vector
            .as_value()
            .module()
            .core_ref()
            .constant_expr(
                vector.ty(),
                ConstantExprOpcode::InsertElement,
                [vector.as_value(), new_element.as_value(), index.as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )
            .map(|folded| Some(folded.as_value()))
    }

    fn fold_shuffle_vector(
        &self,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        mask: &[i32],
    ) -> IrResult<Option<Value<'ctx>>> {
        let (lhs, rhs) = match constants2(lhs, rhs) {
            Some(values) => values,
            None => return Ok(None),
        };
        if let Some(folded) = constant_fold_shuffle_vector_instruction(lhs, rhs, mask)? {
            return Ok(Some(folded.as_value()));
        }
        let module = lhs.as_value().module().core_ref();
        let Some(result_ty) = shuffle_result_type(lhs.ty(), mask)? else {
            return Ok(None);
        };
        let mask_constant = shuffle_mask_constant(module, mask)?;
        module
            .constant_expr(
                result_ty,
                ConstantExprOpcode::ShuffleVector,
                [lhs.as_value(), rhs.as_value(), mask_constant.as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )
            .map(|folded| Some(folded.as_value()))
    }

    fn fold_cast(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx>,
        dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        let value = match Constant::try_from(value) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        if opcode.is_desirable_constant_expr() {
            let Some(expr_opcode) = cast_constant_expr_opcode(opcode) else {
                return constant_fold_cast_instruction(opcode, value, dest_ty)
                    .map(|folded| folded.map(Constant::as_value));
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
                .map(|folded| Some(folded.as_value()));
        }
        constant_fold_cast_instruction(opcode, value, dest_ty)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_binary_intrinsic(
        &self,
        _id: IntrinsicId,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _ty: Type<'ctx>,
        _fmf_source: Option<&Instruction<'ctx>>,
    ) -> IrResult<Option<Value<'ctx>>> {
        // Mirrors ConstantFolder.h: use TargetFolder or InstSimplifyFolder instead.
        Ok(None)
    }

    fn create_pointer_cast(
        &self,
        value: Constant<'ctx>,
        dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        let opcode = pointer_cast_opcode(value.ty(), dest_ty);
        self.fold_cast(opcode, value.as_value(), dest_ty)
    }

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        value: Constant<'ctx>,
        dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        let opcode = pointer_bitcast_or_addrspace_cast_opcode(value.ty(), dest_ty);
        self.fold_cast(opcode, value.as_value(), dest_ty)
    }
}

fn fold_binary<'ctx>(
    opcode: BinaryOpcode,
    lhs: Value<'ctx>,
    rhs: Value<'ctx>,
    flags: ConstantExprFlags,
) -> IrResult<Option<Value<'ctx>>> {
    let (lhs, rhs) = match constants2(lhs, rhs) {
        Some(values) => values,
        None => return Ok(None),
    };
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
                flags,
            )
            .map(|folded| Some(folded.as_value()));
    }
    constant_fold_binary_instruction(opcode, lhs, rhs).map(|folded| folded.map(Constant::as_value))
}

fn fold_exact_binary<'ctx>(
    opcode: BinaryOpcode,
    lhs: Value<'ctx>,
    rhs: Value<'ctx>,
    is_exact: bool,
) -> IrResult<Option<Value<'ctx>>> {
    if !is_exact {
        return fold_binary(opcode, lhs, rhs, ConstantExprFlags::none());
    }
    let (lhs, rhs) = match constants2(lhs, rhs) {
        Some(values) => values,
        None => return Ok(None),
    };
    constant_fold_exact_binary_instruction(opcode, lhs, rhs, true)
        .map(|folded| folded.map(Constant::as_value))
}

fn constants2<'ctx>(
    lhs: Value<'ctx>,
    rhs: Value<'ctx>,
) -> Option<(Constant<'ctx>, Constant<'ctx>)> {
    Some((Constant::try_from(lhs).ok()?, Constant::try_from(rhs).ok()?))
}

fn binary_constant_expr_opcode(opcode: BinaryOpcode) -> Option<ConstantExprOpcode> {
    match opcode {
        BinaryOpcode::Add => Some(ConstantExprOpcode::Add),
        BinaryOpcode::Sub => Some(ConstantExprOpcode::Sub),
        BinaryOpcode::Xor => Some(ConstantExprOpcode::Xor),
        _ => None,
    }
}

fn cast_constant_expr_opcode(opcode: CastOpcode) -> Option<ConstantExprOpcode> {
    match opcode {
        CastOpcode::Trunc => Some(ConstantExprOpcode::Trunc),
        CastOpcode::PtrToAddr => Some(ConstantExprOpcode::PtrToAddr),
        CastOpcode::PtrToInt => Some(ConstantExprOpcode::PtrToInt),
        CastOpcode::IntToPtr => Some(ConstantExprOpcode::IntToPtr),
        CastOpcode::BitCast => Some(ConstantExprOpcode::BitCast),
        CastOpcode::AddrSpaceCast => Some(ConstantExprOpcode::AddrSpaceCast),
        _ => None,
    }
}

fn pointer_cast_opcode(source_ty: Type<'_>, dest_ty: Type<'_>) -> CastOpcode {
    match (
        pointer_address_space(source_ty),
        pointer_address_space(dest_ty),
    ) {
        (Some(source), Some(dest)) if source != dest => CastOpcode::AddrSpaceCast,
        (Some(_), Some(_)) => CastOpcode::BitCast,
        (Some(_), None) if dest_ty.is_integer() => CastOpcode::PtrToInt,
        (None, Some(_)) if source_ty.is_integer() => CastOpcode::IntToPtr,
        _ => CastOpcode::BitCast,
    }
}

fn pointer_bitcast_or_addrspace_cast_opcode(source_ty: Type<'_>, dest_ty: Type<'_>) -> CastOpcode {
    match (
        pointer_address_space(source_ty),
        pointer_address_space(dest_ty),
    ) {
        (Some(source), Some(dest)) if source != dest => CastOpcode::AddrSpaceCast,
        _ => CastOpcode::BitCast,
    }
}

fn pointer_address_space(ty: Type<'_>) -> Option<u32> {
    match ty.data() {
        TypeData::Pointer { addr_space } | TypeData::TypedPointer { addr_space, .. } => {
            Some(*addr_space)
        }
        _ => None,
    }
}

fn vector_element_type<'ctx>(ty: Type<'ctx>) -> Option<Type<'ctx>> {
    let (elem, _, _) = ty.data().as_vector()?;
    Some(Type::new(elem, ty.module()))
}

fn shuffle_result_type<'ctx>(lhs_ty: Type<'ctx>, mask: &[i32]) -> IrResult<Option<Type<'ctx>>> {
    let Some((elem, _, scalable)) = lhs_ty.data().as_vector() else {
        return Ok(None);
    };
    let lanes = u32::try_from(mask.len()).map_err(|_| IrError::InvalidOperation {
        message: "shufflevector mask too large",
    })?;
    let elem_ty = Type::new(elem, lhs_ty.module());
    Ok(Some(
        lhs_ty
            .module()
            .core_ref()
            .vector_type(elem_ty, lanes, scalable)
            .as_type(),
    ))
}

fn shuffle_mask_constant<'ctx>(
    module: &'ctx crate::module::ModuleCore,
    mask: &[i32],
) -> IrResult<Constant<'ctx>> {
    let i32_ty = module.i32_type();
    let mut elements = Vec::with_capacity(mask.len());
    for element in mask {
        if *element == POISON_MASK_ELEM {
            elements.push(i32_ty.as_type().get_undef().as_constant());
        } else {
            elements.push(i32_ty.const_int(*element).as_constant());
        }
    }
    let lanes = u32::try_from(mask.len()).map_err(|_| IrError::InvalidOperation {
        message: "shufflevector mask too large",
    })?;
    module
        .vector_type(i32_ty.as_type(), lanes, false)
        .const_vector(elements)
        .map(|constant| constant.as_constant())
}

fn type_contains_scalable_vector(ty: Type<'_>) -> bool {
    match ty.data() {
        TypeData::ScalableVector { .. } => true,
        TypeData::Array { elem, .. } | TypeData::FixedVector { elem, .. } => {
            type_contains_scalable_vector(Type::new(*elem, ty.module()))
        }
        TypeData::Struct(data) => data.body.borrow().as_ref().is_some_and(|body| {
            body.elements
                .iter()
                .any(|elem| type_contains_scalable_vector(Type::new(*elem, ty.module())))
        }),
        _ => false,
    }
}
