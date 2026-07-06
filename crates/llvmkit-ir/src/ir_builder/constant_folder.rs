//! Default IR-builder folder. Mirrors
//! `llvm/include/llvm/IR/ConstantFolder.h`.
//!
//! The default strategy folds only all-constant inputs. It delegates target-
//! independent arithmetic to [`crate::constant_fold`] and uses the supported
//! `ConstantExpr` constructors for LLVM's desirable constant-expression groups.

use super::constant_fold::{
    constant_fold_binary_instruction, constant_fold_cast_instruction,
    constant_fold_compare_instruction, constant_fold_extract_element_instruction,
    constant_fold_extract_value_instruction, constant_fold_get_element_ptr,
    constant_fold_insert_element_instruction, constant_fold_insert_value_instruction,
    constant_fold_select_instruction, constant_fold_shuffle_vector_instruction,
    constant_fold_unary_instruction, gep_result_type,
};
use super::folder::IRBuilderFolder;
use super::{
    BinaryIntrinsic, BinaryOpcode, CastOpcode, CmpPredicate, Constant, ConstantExprFlags,
    ConstantExprOpcode, ConstantExprOptions, FastMathFlags, FloatType, GepNoWrapFlags, IntType,
    IrError, IrResult, ModuleBrand, ModuleRef, ModuleView, POISON_MASK_ELEM, Type, TypeData,
    UnaryOpcode, Value,
};
use crate::cmp_predicate::{FloatPredicate, IntPredicate};
use crate::float_kind::FloatKind;
use crate::instr_types::OverflowFlags;
use crate::int_width::IntWidth;
use crate::value::{FloatValue, IntValue};

/// Default fold strategy: fold target-independent constant-on-constant
/// operations and decline non-constant inputs.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConstantFolder;

impl<'ctx, B: ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B> for ConstantFolder {
    fn fold_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        fold_binary(opcode, lhs, rhs, ConstantExprFlags::none())
    }

    fn fold_exact_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        fold_exact_binary(opcode, lhs, rhs)
    }

    fn fold_no_wrap_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        flags: OverflowFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let flags = if matches!(opcode, BinaryOpcode::Add | BinaryOpcode::Sub) {
            ConstantExprFlags::overflowing(flags.has_nuw(), flags.has_nsw())
        } else {
            ConstantExprFlags::none()
        };
        fold_binary(opcode, lhs, rhs, flags)
    }

    fn fold_bin_op_fmf_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        self.fold_bin_op_dyn(opcode, lhs, rhs)
    }

    fn fold_un_op_fmf_dyn(
        &self,
        opcode: UnaryOpcode,
        value: Value<'ctx, B>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let value = match Constant::try_from(value) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        constant_fold_unary_instruction(opcode, value).map(|folded| folded.map(Constant::as_value))
    }

    fn fold_cmp_dyn(
        &self,
        predicate: CmpPredicate,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let (lhs, rhs) = match constants2(lhs, rhs) {
            Some(values) => values,
            None => return Ok(None),
        };
        constant_fold_compare_instruction(predicate, lhs, rhs)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_gep_dyn(
        &self,
        source_ty: Type<'ctx, B>,
        ptr: Value<'ctx, B>,
        indices: &[Value<'ctx, B>],
        no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
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
        if let Some(folded) = constant_fold_get_element_ptr(source_ty, ptr, &index_constants, None)?
        {
            return Ok(Some(folded.as_value()));
        }
        let result_ty = gep_result_type(ptr.ty(), &index_constants)?;
        let module = ptr.as_value().module().core_ref();
        module
            .constant_expr_with_options(
                result_ty,
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

    fn fold_select_dyn(
        &self,
        cond: Value<'ctx, B>,
        true_value: Value<'ctx, B>,
        false_value: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
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

    fn fold_extract_value_dyn(
        &self,
        aggregate: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let aggregate = match Constant::try_from(aggregate) {
            Ok(aggregate) => aggregate,
            Err(_) => return Ok(None),
        };
        constant_fold_extract_value_instruction(aggregate, indices)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_insert_value_dyn(
        &self,
        aggregate: Value<'ctx, B>,
        value: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let (aggregate, value) = match constants2(aggregate, value) {
            Some(values) => values,
            None => return Ok(None),
        };
        constant_fold_insert_value_instruction(aggregate, value, indices)
            .map(|folded| folded.map(Constant::as_value))
    }

    fn fold_extract_element_dyn(
        &self,
        vector: Value<'ctx, B>,
        index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
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

    fn fold_insert_element_dyn(
        &self,
        vector: Value<'ctx, B>,
        new_element: Value<'ctx, B>,
        index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
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

    fn fold_shuffle_vector_dyn(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        mask: &[i32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let (lhs, rhs) = match constants2(lhs, rhs) {
            Some(values) => values,
            None => return Ok(None),
        };
        if let Some(folded) = constant_fold_shuffle_vector_instruction(lhs, rhs, mask)? {
            return Ok(Some(folded.as_value()));
        }
        let module: ModuleRef<'ctx, B> = lhs.as_value().module().into();
        let Some(result_ty) = shuffle_result_type(lhs.ty(), mask)? else {
            return Ok(None);
        };
        let mask_constant = shuffle_mask_constant(
            module,
            mask,
            result_ty
                .data()
                .as_vector()
                .is_some_and(|(_, _, scalable)| scalable),
        )?;
        module
            .module()
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

    fn fold_cast_dyn(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
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

    fn fold_binary_intrinsic_dyn(
        &self,
        _id: BinaryIntrinsic,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        _ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        // Mirrors ConstantFolder.h: use TargetFolder or InstSimplifyFolder instead.
        Ok(None)
    }

    fn create_pointer_cast(
        &self,
        value: Constant<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let opcode = pointer_cast_opcode(value.ty(), dest_ty)?;
        self.fold_cast_dyn(opcode, value.as_value(), dest_ty)
    }

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        value: Constant<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let opcode = pointer_bitcast_or_addrspace_cast_opcode(value.ty(), dest_ty)?;
        self.fold_cast_dyn(opcode, value.as_value(), dest_ty)
    }

    // ---- Typed hooks: native overrides under audited kernel invariants.
    //      Each override is preceded by the one-line invariant that lets the
    //      unchecked `from_value_unchecked` wrap skip `narrow_folded_*`'s
    //      TypeId re-check. See the folder-rewrite kernel audit (task-5
    //      report) for the full trace through constant_fold.rs. ----

    fn fold_int_bin_op<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        // Kernel invariant: constant_fold_binary_instruction (constant_fold.rs)
        // rejects lhs.ty() != rhs.ty() up front, and every fold arm it can
        // reach (binop_identity, poison/undef folds, fold_vector_binary's
        // rebuild, fold_int_binary/fold_float_binary, the associative-expr
        // recursion, the global-pointer-mask fold) constructs its result at
        // lhs.ty() -- confirmed by reading each arm in constant_fold.rs. The
        // ConstantExpr fallback (build_binary_constant_or_expr) also passes
        // `lhs.ty()` as the explicit result type. So a folded result's type
        // always equals the operand type; the unchecked wrap cannot mistype.
        Ok(self
            .fold_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())?
            .map(IntValue::from_value_unchecked))
    }

    fn fold_int_bin_op_no_wrap<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
        flags: OverflowFlags,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        // Kernel invariant: fold_no_wrap_bin_op_dyn funnels into the same
        // fold_binary(..., lhs.ty()-pinned ConstantExprFlags) path as
        // fold_bin_op_dyn for the Add/Sub overflow-flag case (and plain
        // fold_binary otherwise) -- same lhs.ty()-preservation argument as
        // fold_int_bin_op above.
        Ok(self
            .fold_no_wrap_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value(), flags)?
            .map(IntValue::from_value_unchecked))
    }

    fn fold_int_bin_op_exact<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        // Kernel invariant: fold_exact_bin_op_dyn -> fold_exact_binary ->
        // fold_binary_constants, the same lhs.ty()-pinned path fold_bin_op_dyn
        // uses (constant_fold_binary_instruction / the ConstantExpr
        // constructor called with lhs.ty()).
        Ok(self
            .fold_exact_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())?
            .map(IntValue::from_value_unchecked))
    }

    fn fold_fp_bin_op<K: FloatKind>(
        &self,
        opcode: BinaryOpcode,
        lhs: FloatValue<'ctx, K, B>,
        rhs: FloatValue<'ctx, K, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        // Kernel invariant: fold_bin_op_fmf_dyn ignores fmf and delegates to
        // fold_bin_op_dyn (ConstantFolder.h: FoldBinOpFMF drops the flags for
        // the default folder), so the same lhs.ty()-preservation argument
        // applies transitively.
        Ok(self
            .fold_bin_op_fmf_dyn(opcode, lhs.as_value(), rhs.as_value(), fmf)?
            .map(FloatValue::from_value_unchecked))
    }

    fn fold_fp_un_op<K: FloatKind>(
        &self,
        opcode: UnaryOpcode,
        value: FloatValue<'ctx, K, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        // Kernel invariant: constant_fold_unary_instruction's only opcode
        // (FNeg) returns `operand` itself (poison_for(operand.ty()),
        // returning `operand` unchanged for undef, or
        // value.ty().const_ap_float(...)`) -- always operand.ty(), so the
        // result type equals the input FloatValue's type.
        Ok(self
            .fold_un_op_fmf_dyn(opcode, value.as_value(), fmf)?
            .map(FloatValue::from_value_unchecked))
    }

    fn fold_int_cmp<W: IntWidth>(
        &self,
        predicate: IntPredicate,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, bool, B>>> {
        // Kernel invariant: constant_fold_compare_instruction computes
        // result_ty = compare_result_type(lhs.ty()) once lhs.ty() == rhs.ty()
        // holds; compare_result_type maps a scalar operand type to scalar i1
        // (only the vector-operand branch produces `<N x i1>`, and IntValue's
        // IntWidth marker is scalar-only -- IntoIntValue rejects integer
        // vectors, see ir_builder.rs's erased-integer-binop comment). Every
        // fold arm on the scalar path (bool_constant_for_type, poison_for,
        // fold_undef_compare, the i1-Eq/Ne Xor rewrite, the direct APInt
        // compare) is built at result_ty, i.e. scalar i1. Safe to wrap as
        // IntValue<bool, B> without a re-check.
        Ok(self
            .fold_cmp_dyn(predicate.into(), lhs.as_value(), rhs.as_value())?
            .map(IntValue::<bool, B>::from_value_unchecked))
    }

    fn fold_fp_cmp<K: FloatKind>(
        &self,
        predicate: FloatPredicate,
        lhs: FloatValue<'ctx, K, B>,
        rhs: FloatValue<'ctx, K, B>,
    ) -> IrResult<Option<IntValue<'ctx, bool, B>>> {
        // Kernel invariant: same constant_fold_compare_instruction path as
        // fold_int_cmp above; FloatValue's FloatKind marker is likewise
        // scalar-only, so the scalar result_ty == i1 argument applies
        // identically for float predicates.
        Ok(self
            .fold_cmp_dyn(predicate.into(), lhs.as_value(), rhs.as_value())?
            .map(IntValue::<bool, B>::from_value_unchecked))
    }

    fn fold_cast_to_int<W: IntWidth>(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: IntType<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        // Kernel invariant: constant_fold_cast_instruction builds every
        // result at exactly `dest_ty` (poison_for(dest_ty), dest_ty.get_undef(),
        // null_constant_for_type(dest_ty), dst_ty.const_ap_int/const_ap_float
        // where dst_ty is dest_ty narrowed, fold_bitcast(operand, dest_ty)
        // which itself only ever returns dest_ty-typed constants) and the
        // desirable-ConstantExpr fallback (fold_cast_dyn's
        // `value.module().constant_expr(dest_ty, ...)`) is explicitly
        // dest_ty-typed too. Confirmed by reading every match arm in
        // constant_fold_cast_instruction / fold_bitcast / fold_constant_cast_pair.
        Ok(self
            .fold_cast_dyn(opcode, value, dest_ty.as_type())?
            .map(IntValue::from_value_unchecked))
    }

    fn fold_cast_to_fp<K: FloatKind>(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: FloatType<'ctx, K, B>,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        // Kernel invariant: same constant_fold_cast_instruction /
        // fold_cast_dyn dest_ty-pinning argument as fold_cast_to_int above,
        // for the float-destination opcodes (FpTrunc/FpExt/UIToFp/SIToFp/
        // BitCast).
        Ok(self
            .fold_cast_dyn(opcode, value, dest_ty.as_type())?
            .map(FloatValue::from_value_unchecked))
    }
}

fn fold_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    flags: ConstantExprFlags,
) -> IrResult<Option<Value<'ctx, B>>> {
    let (lhs, rhs) = match constants2(lhs, rhs) {
        Some(values) => values,
        None => return Ok(None),
    };
    fold_binary_constants(opcode, lhs, rhs, flags)
}

fn fold_binary_constants<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B>,
    rhs: Constant<'ctx, B>,
    flags: ConstantExprFlags,
) -> IrResult<Option<Value<'ctx, B>>> {
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

fn fold_exact_binary<'ctx, B: ModuleBrand + 'ctx>(
    opcode: BinaryOpcode,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
) -> IrResult<Option<Value<'ctx, B>>> {
    let (lhs, rhs) = match constants2(lhs, rhs) {
        Some(values) => values,
        None => return Ok(None),
    };
    fold_binary_constants(opcode, lhs, rhs, ConstantExprFlags::none())
}

fn constants2<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
) -> Option<(Constant<'ctx, B>, Constant<'ctx, B>)> {
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

fn pointer_cast_opcode<B: ModuleBrand>(
    source_ty: Type<'_, B>,
    dest_ty: Type<'_, B>,
) -> IrResult<CastOpcode> {
    let Some(source) = ptr_or_ptr_vector_address_space(source_ty) else {
        return invalid_pointer_cast();
    };
    if is_int_or_int_vector(dest_ty) {
        if !lane_shape_matches(source_ty, dest_ty) {
            return invalid_pointer_cast();
        }
        return Ok(CastOpcode::PtrToInt);
    }
    let Some(dest) = ptr_or_ptr_vector_address_space(dest_ty) else {
        return invalid_pointer_cast();
    };
    if source != dest {
        if !lane_shape_matches(source_ty, dest_ty) {
            return invalid_pointer_cast();
        }
        Ok(CastOpcode::AddrSpaceCast)
    } else if pointer_bitcast_shape_matches(source_ty, dest_ty) {
        Ok(CastOpcode::BitCast)
    } else {
        invalid_pointer_cast()
    }
}

fn pointer_bitcast_or_addrspace_cast_opcode<B: ModuleBrand>(
    source_ty: Type<'_, B>,
    dest_ty: Type<'_, B>,
) -> IrResult<CastOpcode> {
    let Some(source) = ptr_or_ptr_vector_address_space(source_ty) else {
        return invalid_pointer_cast();
    };
    let Some(dest) = ptr_or_ptr_vector_address_space(dest_ty) else {
        return invalid_pointer_cast();
    };
    if source != dest {
        if !lane_shape_matches(source_ty, dest_ty) {
            return invalid_pointer_cast();
        }
        Ok(CastOpcode::AddrSpaceCast)
    } else if pointer_bitcast_shape_matches(source_ty, dest_ty) {
        Ok(CastOpcode::BitCast)
    } else {
        invalid_pointer_cast()
    }
}

fn ptr_or_ptr_vector_address_space<B: ModuleBrand>(ty: Type<'_, B>) -> Option<u32> {
    match ty.data() {
        TypeData::Pointer { addr_space } | TypeData::TypedPointer { addr_space, .. } => {
            Some(*addr_space)
        }
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            ptr_or_ptr_vector_address_space(Type::new(*elem, ty.module()))
        }
        _ => None,
    }
}

fn is_int_or_int_vector<B: ModuleBrand>(ty: Type<'_, B>) -> bool {
    match ty.data() {
        TypeData::Integer { .. } => true,
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            matches!(
                Type::new(*elem, ty.module()).data(),
                TypeData::Integer { .. }
            )
        }
        _ => false,
    }
}

fn lane_shape_matches<B: ModuleBrand>(lhs: Type<'_, B>, rhs: Type<'_, B>) -> bool {
    vector_shape(lhs) == vector_shape(rhs)
}

fn pointer_bitcast_shape_matches<B: ModuleBrand>(lhs: Type<'_, B>, rhs: Type<'_, B>) -> bool {
    match (vector_shape(lhs), vector_shape(rhs)) {
        (None, None) => true,
        (Some(lhs_shape), Some(rhs_shape)) => lhs_shape == rhs_shape,
        (None, Some((1, false))) | (Some((1, false)), None) => true,
        _ => false,
    }
}

fn vector_shape<B: ModuleBrand>(ty: Type<'_, B>) -> Option<(u32, bool)> {
    ty.data()
        .as_vector()
        .map(|(_, lanes, scalable)| (lanes, scalable))
}

fn invalid_pointer_cast<T>() -> IrResult<T> {
    Err(IrError::InvalidOperation {
        message: "invalid pointer cast constant expression",
    })
}

fn vector_element_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<Type<'ctx, B>> {
    let (elem, _, _) = ty.data().as_vector()?;
    Some(Type::new(elem, ty.module()))
}

fn shuffle_result_type<'ctx, B: ModuleBrand + 'ctx>(
    lhs_ty: Type<'ctx, B>,
    mask: &[i32],
) -> IrResult<Option<Type<'ctx, B>>> {
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
            .vector_type(elem_ty, lanes, scalable)
            .as_type(),
    ))
}

fn shuffle_mask_constant<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleRef<'ctx, B>,
    mask: &[i32],
    scalable: bool,
) -> IrResult<Constant<'ctx, B>> {
    let i32_ty = IntType::<i32, B>::new(module.module().i32_type().as_type().id(), module);
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
    ModuleView::<B>::new(module.module())
        .vector_type(i32_ty.as_type(), lanes, scalable)
        .const_vector(elements)
        .map(|constant| constant.as_constant())
}

fn type_contains_scalable_vector<B: ModuleBrand>(ty: Type<'_, B>) -> bool {
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
        TypeData::TargetExt(data) => match data.name.as_str() {
            "aarch64.svcount" => true,
            "riscv.vector.tuple" => data.type_params.first().is_some_and(|elem| {
                matches!(
                    Type::new(*elem, ty.module()).data(),
                    TypeData::ScalableVector { .. }
                )
            }),
            _ => false,
        },
        _ => false,
    }
}
