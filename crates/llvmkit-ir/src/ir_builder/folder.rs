//! Folder trait for the IR builder. Mirrors
//! `llvm/include/llvm/IR/IRBuilderFolder.h`.
//!
//! Upstream returns `Value *` from every hook, with `nullptr` meaning
//! "no fold". The Rust analog uses `IrResult<Option<Value<'ctx, B>>>`:
//! `Ok(None)` declines to fold, `Ok(Some(_))` returns an existing value
//! or constant, and `Err(_)` reports a malformed custom-folder result.

use super::{
    BinaryIntrinsic, BinaryOpcode, Brand, CastOpcode, CmpPredicate, Constant, FastMathFlags,
    GepNoWrapFlags, InstructionView, IrResult, ModuleBrand, Type, UnaryOpcode, Value,
};
use crate::cmp_predicate::{FloatPredicate, IntPredicate};
use crate::derived_types::{FloatType, IntType};
use crate::float_kind::FloatKind;
use crate::instr_types::OverflowFlags;
use crate::int_width::IntWidth;
use crate::value::{FloatValue, IntValue, Typed};

/// Strategy for folding values during builder calls.
///
/// Every hook has two flavors:
///
/// - The erased `*_dyn` hooks (this section) take and return
///   type-erased [`Value`]. They mirror upstream's `Value *`-in,
///   `Value *`-out signatures directly and are what the parser and the
///   `_dyn` builder methods call. Every hook has a default
///   "decline to fold" body (`Ok(None)`), so a folder that overrides
///   only a handful of hooks (e.g. [`super::constant_folder::ConstantFolder`]'s
///   predecessors before this trait grew) still compiles.
/// - The typed hooks (below) are called by the statically-typed
///   `build_*` paths; results are typed handles the builder accepts
///   without a runtime re-check for static markers. Defaults delegate
///   to the matching `_dyn` hook and re-narrow by `TypeId`, so a folder
///   that only overrides the erased surface keeps today's semantics.
///   Pointer-, vector-, and aggregate-result folds (`fold_gep_dyn`,
///   `fold_select_dyn`, ...) deliberately stay erased: `PointerValue`
///   does not statically pin the address space and vector element
///   typing is deferred (T4).
pub trait IRBuilderFolder<'ctx, B: ModuleBrand + 'ctx = Brand<'ctx>> {
    fn fold_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (opcode, lhs, rhs);
        Ok(None)
    }

    /// The builder only ever calls this with exactness implied by the
    /// method (there is no non-exact caller), so unlike upstream's
    /// `FoldExactBinOp(Instruction::BinaryOps, Value *, Value *, bool
    /// IsExact)` there is no `is_exact` parameter to thread.
    fn fold_exact_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (opcode, lhs, rhs);
        Ok(None)
    }

    fn fold_no_wrap_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        flags: OverflowFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (opcode, lhs, rhs, flags);
        Ok(None)
    }

    fn fold_bin_op_fmf_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (opcode, lhs, rhs, fmf);
        Ok(None)
    }

    fn fold_un_op_fmf_dyn(
        &self,
        opcode: UnaryOpcode,
        value: Value<'ctx, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (opcode, value, fmf);
        Ok(None)
    }

    fn fold_cmp_dyn(
        &self,
        predicate: CmpPredicate,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (predicate, lhs, rhs);
        Ok(None)
    }

    fn fold_gep_dyn(
        &self,
        source_ty: Type<'ctx, B>,
        ptr: Value<'ctx, B>,
        indices: &[Value<'ctx, B>],
        no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (source_ty, ptr, indices, no_wrap);
        Ok(None)
    }

    fn fold_select_dyn(
        &self,
        cond: Value<'ctx, B>,
        true_value: Value<'ctx, B>,
        false_value: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (cond, true_value, false_value);
        Ok(None)
    }

    fn fold_extract_value_dyn(
        &self,
        aggregate: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (aggregate, indices);
        Ok(None)
    }

    fn fold_insert_value_dyn(
        &self,
        aggregate: Value<'ctx, B>,
        value: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (aggregate, value, indices);
        Ok(None)
    }

    fn fold_extract_element_dyn(
        &self,
        vector: Value<'ctx, B>,
        index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (vector, index);
        Ok(None)
    }

    fn fold_insert_element_dyn(
        &self,
        vector: Value<'ctx, B>,
        new_element: Value<'ctx, B>,
        index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (vector, new_element, index);
        Ok(None)
    }

    fn fold_shuffle_vector_dyn(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        mask: &[i32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (lhs, rhs, mask);
        Ok(None)
    }

    fn fold_cast_dyn(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (opcode, value, dest_ty);
        Ok(None)
    }

    fn fold_binary_intrinsic_dyn(
        &self,
        id: BinaryIntrinsic,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (id, lhs, rhs, ty);
        Ok(None)
    }

    fn fold_binary_intrinsic_with_fmf_source_dyn(
        &self,
        id: BinaryIntrinsic,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        ty: Type<'ctx, B>,
        fmf_source: &InstructionView<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = fmf_source;
        self.fold_binary_intrinsic_dyn(id, lhs, rhs, ty)
    }

    fn create_pointer_cast(
        &self,
        value: Constant<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (value, dest_ty);
        Ok(None)
    }

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        value: Constant<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (value, dest_ty);
        Ok(None)
    }

    // ---- Typed hooks. Called by the statically-typed build_* paths;
    //      results are typed handles the builder accepts without a
    //      runtime re-check for static markers. Defaults delegate to
    //      the matching _dyn hook and re-narrow by TypeId, so a folder
    //      that only overrides the erased surface keeps today's
    //      semantics. Pointer-, vector-, and aggregate-result folds
    //      (fold_gep_dyn, fold_select_dyn, ...) deliberately stay
    //      erased: PointerValue does not statically pin the address
    //      space and vector element typing is deferred (T4). ----

    fn fold_int_bin_op<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())?;
        narrow_folded_int(folded, lhs)
    }

    fn fold_int_bin_op_no_wrap<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
        flags: OverflowFlags,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_no_wrap_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value(), flags)?;
        narrow_folded_int(folded, lhs)
    }

    fn fold_int_bin_op_exact<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_exact_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())?;
        narrow_folded_int(folded, lhs)
    }

    fn fold_fp_bin_op<K: FloatKind>(
        &self,
        opcode: BinaryOpcode,
        lhs: FloatValue<'ctx, K, B>,
        rhs: FloatValue<'ctx, K, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        let folded = self.fold_bin_op_fmf_dyn(opcode, lhs.as_value(), rhs.as_value(), fmf)?;
        narrow_folded_fp(folded, lhs)
    }

    fn fold_fp_un_op<K: FloatKind>(
        &self,
        opcode: UnaryOpcode,
        value: FloatValue<'ctx, K, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        let folded = self.fold_un_op_fmf_dyn(opcode, value.as_value(), fmf)?;
        narrow_folded_fp(folded, value)
    }

    fn fold_int_cmp<W: IntWidth>(
        &self,
        predicate: IntPredicate,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, bool, B>>> {
        let folded = self.fold_cmp_dyn(predicate.into(), lhs.as_value(), rhs.as_value())?;
        narrow_folded_bool(folded)
    }

    fn fold_fp_cmp<K: FloatKind>(
        &self,
        predicate: FloatPredicate,
        lhs: FloatValue<'ctx, K, B>,
        rhs: FloatValue<'ctx, K, B>,
    ) -> IrResult<Option<IntValue<'ctx, bool, B>>> {
        let folded = self.fold_cmp_dyn(predicate.into(), lhs.as_value(), rhs.as_value())?;
        narrow_folded_bool(folded)
    }

    fn fold_cast_to_int<W: IntWidth>(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: IntType<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_cast_dyn(opcode, value, dest_ty.as_type())?;
        narrow_folded_cast_int(folded, dest_ty)
    }

    fn fold_cast_to_fp<K: FloatKind>(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: FloatType<'ctx, K, B>,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        let folded = self.fold_cast_dyn(opcode, value, dest_ty.as_type())?;
        narrow_folded_cast_fp(folded, dest_ty)
    }
}

/// Re-narrow an erased fold result to the operand's int width by TypeId
/// equality. Used by the typed hooks' delegating default bodies; native
/// typed overrides (ConstantFolder) skip this entirely.
///
/// Compares against `like`'s *runtime* type, never against `W` — see
/// [`Type::require_match`] for why that distinction is load-bearing, and
/// [`narrow_folded_bool`] for the one seam where narrowing to the marker
/// is in fact correct.
pub(super) fn narrow_folded_int<'ctx, W, B>(
    folded: Option<Value<'ctx, B>>,
    like: IntValue<'ctx, W, B>,
) -> IrResult<Option<IntValue<'ctx, W, B>>>
where
    W: IntWidth,
    B: ModuleBrand + 'ctx,
{
    let Some(v) = folded else { return Ok(None) };
    like.as_value().ty().require_match(v.ty())?;
    Ok(Some(IntValue::from_value_unchecked(v)))
}

/// Re-narrow an erased fold result to the operand's float kind by TypeId
/// equality. Mirrors [`narrow_folded_int`] for the float-kind family.
pub(super) fn narrow_folded_fp<'ctx, K, B>(
    folded: Option<Value<'ctx, B>>,
    like: FloatValue<'ctx, K, B>,
) -> IrResult<Option<FloatValue<'ctx, K, B>>>
where
    K: FloatKind,
    B: ModuleBrand + 'ctx,
{
    let Some(v) = folded else { return Ok(None) };
    Typed::ty(like).require_match(v.ty())?;
    Ok(Some(FloatValue::from_value_unchecked(v)))
}

/// Re-narrow an erased fold result to `i1` (the fixed result type of every
/// comparison), for the icmp/fcmp typed hooks.
///
/// The one seam in this family that narrows to a *marker* rather than
/// comparing against a runtime type, and the only one where that is sound:
/// every comparison yields `i1` by definition, so the expected type is a
/// compile-time constant rather than an operand's. `bool` is a static
/// marker naming width 1, so `bool::narrow` — unlike `IntDyn::narrow`,
/// which would accept any integer — checks exactly what the hand-rolled
/// `TypeData::Integer { bits: 1 }` match checked, and reports width drift
/// more precisely (see [`Type::require_match`] on why `i1`-vs-`i32` must
/// not read "expected integer, got integer").
pub(super) fn narrow_folded_bool<'ctx, B>(
    folded: Option<Value<'ctx, B>>,
) -> IrResult<Option<IntValue<'ctx, bool, B>>>
where
    B: ModuleBrand + 'ctx,
{
    let Some(v) = folded else { return Ok(None) };
    <bool as IntWidth>::narrow(v).map(Some)
}

/// Re-narrow an erased cast fold result to the destination int type by
/// TypeId equality. Mirrors [`narrow_folded_int`] but compares against
/// `dest_ty` instead of an operand (casts have no same-type operand).
pub(super) fn narrow_folded_cast_int<'ctx, W, B>(
    folded: Option<Value<'ctx, B>>,
    dest_ty: IntType<'ctx, W, B>,
) -> IrResult<Option<IntValue<'ctx, W, B>>>
where
    W: IntWidth,
    B: ModuleBrand + 'ctx,
{
    let Some(v) = folded else { return Ok(None) };
    dest_ty.as_type().require_match(v.ty())?;
    Ok(Some(IntValue::from_value_unchecked(v)))
}

/// Re-narrow an erased cast fold result to the destination float type by
/// TypeId equality. Mirrors [`narrow_folded_cast_int`] for float kinds.
pub(super) fn narrow_folded_cast_fp<'ctx, K, B>(
    folded: Option<Value<'ctx, B>>,
    dest_ty: FloatType<'ctx, K, B>,
) -> IrResult<Option<FloatValue<'ctx, K, B>>>
where
    K: FloatKind,
    B: ModuleBrand + 'ctx,
{
    let Some(v) = folded else { return Ok(None) };
    dest_ty.as_type().require_match(v.ty())?;
    Ok(Some(FloatValue::from_value_unchecked(v)))
}
