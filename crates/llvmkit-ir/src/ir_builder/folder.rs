//! Folder trait for the IR builder. Mirrors
//! `llvm/include/llvm/IR/IRBuilderFolder.h`.
//!
//! Upstream returns `Value *` from every hook, with `nullptr` meaning
//! "no fold". The Rust analog uses `IrResult<Option<Value<'ctx, B>>>`:
//! `Ok(None)` declines to fold, `Ok(Some(_))` returns an existing value
//! or constant, and `Err(_)` reports a malformed custom-folder result.

use super::{
    BinaryOpcode, Brand, CastOpcode, CmpPredicate, Constant, FastMathFlags, GepNoWrapFlags,
    InstructionView, IntrinsicId, IrResult, ModuleBrand, Type, UnaryOpcode, Value,
};

/// Strategy for folding values during builder calls.
pub trait IRBuilderFolder<'ctx, B: ModuleBrand = Brand<'ctx>> {
    fn fold_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_exact_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        is_exact: bool,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_no_wrap_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        has_nuw: bool,
        has_nsw: bool,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_bin_op_fmf(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_un_op_fmf(
        &self,
        opcode: UnaryOpcode,
        value: Value<'ctx, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_cmp(
        &self,
        predicate: CmpPredicate,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_gep(
        &self,
        source_ty: Type<'ctx, B>,
        ptr: Value<'ctx, B>,
        indices: &[Value<'ctx, B>],
        no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_select(
        &self,
        cond: Value<'ctx, B>,
        true_value: Value<'ctx, B>,
        false_value: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_extract_value(
        &self,
        aggregate: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_insert_value(
        &self,
        aggregate: Value<'ctx, B>,
        value: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_extract_element(
        &self,
        vector: Value<'ctx, B>,
        index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_insert_element(
        &self,
        vector: Value<'ctx, B>,
        new_element: Value<'ctx, B>,
        index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_shuffle_vector(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        mask: &[i32],
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_cast(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn fold_binary_intrinsic(
        &self,
        id: IntrinsicId,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        ty: Type<'ctx, B>,
        fmf_source: Option<&InstructionView<'ctx, B>>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn create_pointer_cast(
        &self,
        value: Constant<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        value: Constant<'ctx, B>,
        dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>>;
}
