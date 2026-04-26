//! Sealed [`User`] trait for any value that references other values.
//! Mirrors `llvm/include/llvm/IR/User.h`.
//!
//! Implementations live with their concrete handles ([`Instruction`]
//! today; more as new categories land). The trait is sealed so the set
//! of operand-bearing categories stays closed.
//!
//! [`Instruction`]: crate::instruction::Instruction

use crate::r#use::Use;
use crate::value::{Value, sealed};

/// Trait implemented by values that have operand edges. Inherits
/// the same `Sealed` bound used by every value-side trait so external
/// crates cannot widen the closed user set.
pub trait User<'ctx>: sealed::Sealed {
    /// Number of operand edges. Mirrors `User::getNumOperands`.
    fn operand_count(self) -> u32;

    /// Operand at `index`, or `None` if `index >= operand_count()`.
    /// Mirrors `User::getOperand`.
    fn operand(self, index: u32) -> Option<Value<'ctx>>;

    /// Materialize a transient [`Use`] view for the operand at
    /// `index`, or `None` if out of range. Mirrors the role of
    /// `User::getOperandUse` but builds the view on demand instead of
    /// linking into an intrusive list (see the [`Use`] type).
    fn operand_use(self, index: u32) -> Option<Use<'ctx>>;
}
