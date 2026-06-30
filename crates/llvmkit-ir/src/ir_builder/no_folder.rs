//! No-op IR-builder folder. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
//!
//! Every method declines to fold; the builder always emits a real
//! instruction.

use super::folder::IRBuilderFolder;
use super::{
    BinaryIntrinsic, BinaryOpcode, CastOpcode, CmpPredicate, Constant, FastMathFlags,
    GepNoWrapFlags, InstructionView, IrResult, ModuleBrand, Type, UnaryOpcode, Value,
};

/// Folder that never folds.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFolder;

impl<'ctx, B: ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B> for NoFolder {
    fn fold_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_exact_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        _is_exact: bool,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_no_wrap_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        _has_nuw: bool,
        _has_nsw: bool,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_bin_op_fmf(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_un_op_fmf(
        &self,
        _opcode: UnaryOpcode,
        _value: Value<'ctx, B>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_cmp(
        &self,
        _predicate: CmpPredicate,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_gep(
        &self,
        _source_ty: Type<'ctx, B>,
        _ptr: Value<'ctx, B>,
        _indices: &[Value<'ctx, B>],
        _no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_select(
        &self,
        _cond: Value<'ctx, B>,
        _true_value: Value<'ctx, B>,
        _false_value: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_extract_value(
        &self,
        _aggregate: Value<'ctx, B>,
        _indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_insert_value(
        &self,
        _aggregate: Value<'ctx, B>,
        _value: Value<'ctx, B>,
        _indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_extract_element(
        &self,
        _vector: Value<'ctx, B>,
        _index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_insert_element(
        &self,
        _vector: Value<'ctx, B>,
        _new_element: Value<'ctx, B>,
        _index: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_shuffle_vector(
        &self,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        _mask: &[i32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_cast(
        &self,
        _opcode: CastOpcode,
        _value: Value<'ctx, B>,
        _dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn fold_binary_intrinsic(
        &self,
        _id: BinaryIntrinsic,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        _ty: Type<'ctx, B>,
        _fmf_source: Option<&InstructionView<'ctx, B>>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn create_pointer_cast(
        &self,
        _value: Constant<'ctx, B>,
        _dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        _value: Constant<'ctx, B>,
        _dest_ty: Type<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        Ok(None)
    }
}
