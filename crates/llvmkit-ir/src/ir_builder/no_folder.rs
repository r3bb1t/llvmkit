//! No-op IR-builder folder. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
//!
//! Every method declines to fold; the builder always emits a real
//! instruction. Used when constant folding interferes with analysis
//! (e.g. when callers want to inspect the literal arithmetic chain).

use crate::constant::Constant;
use crate::ir_builder::folder::IRBuilderFolder;
use crate::value::Value;

/// Folder that never folds.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFolder;

impl<'ctx> IRBuilderFolder<'ctx> for NoFolder {
    #[inline]
    fn fold_int_add(&self, _lhs: Value<'ctx>, _rhs: Value<'ctx>) -> Option<Constant<'ctx>> {
        None
    }
    #[inline]
    fn fold_int_sub(&self, _lhs: Value<'ctx>, _rhs: Value<'ctx>) -> Option<Constant<'ctx>> {
        None
    }
    #[inline]
    fn fold_int_mul(&self, _lhs: Value<'ctx>, _rhs: Value<'ctx>) -> Option<Constant<'ctx>> {
        None
    }
}
