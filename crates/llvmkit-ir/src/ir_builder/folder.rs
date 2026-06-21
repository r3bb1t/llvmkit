//! Folder trait for the IR builder. Mirrors
//! `llvm/include/llvm/IR/IRBuilderFolder.h`.
//!
//! Upstream returns `Value *` from every hook, with `nullptr` meaning
//! "no fold". The Rust analog uses `IrResult<Option<Value<'ctx>>>`:
//! `Ok(None)` declines to fold, `Ok(Some(_))` returns an existing value
//! or constant, and `Err(_)` reports a malformed custom-folder result.

use crate::IrResult;
use crate::cmp_predicate::CmpPredicate;
use crate::constant::Constant;
use crate::fmf::FastMathFlags;
use crate::gep_no_wrap_flags::GepNoWrapFlags;
use crate::instr_types::{BinaryOpcode, CastOpcode, UnaryOpcode};
use crate::instruction::Instruction;
use crate::intrinsics::IntrinsicId;
use crate::r#type::Type;
use crate::value::Value;

/// Strategy for folding values during builder calls.
pub trait IRBuilderFolder<'ctx> {
    fn fold_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_exact_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        is_exact: bool,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_no_wrap_bin_op(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        has_nuw: bool,
        has_nsw: bool,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_bin_op_fmf(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_un_op_fmf(
        &self,
        opcode: UnaryOpcode,
        value: Value<'ctx>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_cmp(
        &self,
        predicate: CmpPredicate,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_gep(
        &self,
        source_ty: Type<'ctx>,
        ptr: Value<'ctx>,
        indices: &[Value<'ctx>],
        no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_select(
        &self,
        cond: Value<'ctx>,
        true_value: Value<'ctx>,
        false_value: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_extract_value(
        &self,
        aggregate: Value<'ctx>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_insert_value(
        &self,
        aggregate: Value<'ctx>,
        value: Value<'ctx>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_extract_element(
        &self,
        vector: Value<'ctx>,
        index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_insert_element(
        &self,
        vector: Value<'ctx>,
        new_element: Value<'ctx>,
        index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_shuffle_vector(
        &self,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        mask: &[i32],
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_cast(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx>,
        dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn fold_binary_intrinsic(
        &self,
        id: IntrinsicId,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        ty: Type<'ctx>,
        fmf_source: Option<&Instruction<'ctx>>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn create_pointer_cast(
        &self,
        value: Constant<'ctx>,
        dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        value: Constant<'ctx>,
        dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>>;
}
