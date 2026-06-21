//! No-op IR-builder folder. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
//!
//! Every method declines to fold; the builder always emits a real
//! instruction.

use crate::IrResult;
use crate::cmp_predicate::CmpPredicate;
use crate::constant::Constant;
use crate::fmf::FastMathFlags;
use crate::gep_no_wrap_flags::GepNoWrapFlags;
use crate::instr_types::{BinaryOpcode, CastOpcode, UnaryOpcode};
use crate::instruction::Instruction;
use crate::intrinsics::IntrinsicId;
use crate::ir_builder::folder::IRBuilderFolder;
use crate::r#type::Type;
use crate::value::Value;

/// Folder that never folds.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFolder;

impl<'ctx> IRBuilderFolder<'ctx> for NoFolder {
    fn fold_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_exact_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _is_exact: bool,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_no_wrap_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _has_nuw: bool,
        _has_nsw: bool,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_bin_op_fmf(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_un_op_fmf(
        &self,
        _opcode: UnaryOpcode,
        _value: Value<'ctx>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_cmp(
        &self,
        _predicate: CmpPredicate,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_gep(
        &self,
        _source_ty: Type<'ctx>,
        _ptr: Value<'ctx>,
        _indices: &[Value<'ctx>],
        _no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_select(
        &self,
        _cond: Value<'ctx>,
        _true_value: Value<'ctx>,
        _false_value: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_extract_value(
        &self,
        _aggregate: Value<'ctx>,
        _indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_insert_value(
        &self,
        _aggregate: Value<'ctx>,
        _value: Value<'ctx>,
        _indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_extract_element(
        &self,
        _vector: Value<'ctx>,
        _index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_insert_element(
        &self,
        _vector: Value<'ctx>,
        _new_element: Value<'ctx>,
        _index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_shuffle_vector(
        &self,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _mask: &[i32],
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_cast(
        &self,
        _opcode: CastOpcode,
        _value: Value<'ctx>,
        _dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn fold_binary_intrinsic(
        &self,
        _id: IntrinsicId,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _ty: Type<'ctx>,
        _fmf_source: Option<&Instruction<'ctx>>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn create_pointer_cast(
        &self,
        _value: Constant<'ctx>,
        _dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        _value: Constant<'ctx>,
        _dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        Ok(None)
    }
}
