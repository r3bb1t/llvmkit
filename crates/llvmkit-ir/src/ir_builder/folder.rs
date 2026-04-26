//! Folder trait for the IR builder. Mirrors
//! `llvm/include/llvm/IR/IRBuilderFolder.h`.
//!
//! ## Trait shape
//!
//! Upstream `IRBuilderFolder` defines a (large) virtual interface
//! whose member functions return `Value *` (with `nullptr` meaning
//! "no fold"). The Rust analog returns `Option<Constant<'ctx>>` to
//! preserve the no-fold signal without a sentinel.
//!
//! Phase G ships the methods the slice needs; the trait grows as new
//! `IRBuilder::build_*` methods land. The trait is **not** sealed:
//! consumers may plug in their own folder strategies (`NoFolder`,
//! `LazyConstantFolder`, custom analyses, ...).

use crate::constant::Constant;
use crate::constants::ConstantIntValue;
use crate::int_width::BDyn;
use crate::value::Value;

/// Strategy for folding constant operands during builder calls.
pub trait IRBuilderFolder<'ctx> {
    /// Try to fold `lhs + rhs` to a constant. Returns `None` if either
    /// operand is non-constant (or if the folder declines to fold).
    fn fold_int_add(&self, lhs: Value<'ctx>, rhs: Value<'ctx>) -> Option<Constant<'ctx>>;
    /// Try to fold `lhs - rhs` to a constant.
    fn fold_int_sub(&self, lhs: Value<'ctx>, rhs: Value<'ctx>) -> Option<Constant<'ctx>>;
    /// Try to fold `lhs * rhs` to a constant.
    fn fold_int_mul(&self, lhs: Value<'ctx>, rhs: Value<'ctx>) -> Option<Constant<'ctx>>;
}

/// Helper: try to extract a `ConstantIntValue` from a `Value`.
/// Returns `None` if the value isn't a constant integer of a width
/// that fits losslessly in `u128` (the canonical fold representation
/// for the slice).
pub(crate) fn as_int_const<'ctx>(v: Value<'ctx>) -> Option<ConstantIntValue<'ctx, BDyn>> {
    let c = Constant::try_from(v).ok()?;
    ConstantIntValue::try_from(c).ok()
}
