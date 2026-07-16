//! Sealed marker types for a basic block's typed parameter list.
//!
//! The block analog of [`element`](crate::element): where that module
//! pins a typed vector/array element at the type level, this one pins the
//! parameter shape a basic-block handle carries. A block marked with a
//! typed parameter tuple is a different type from a block marked
//! [`BlockParamsDyn`], so mixing them at a branch/edge call site becomes
//! a compile error rather than a runtime shape mismatch.
//!
//! ## This slice ships the erased marker only
//!
//! This is the foundational, purely-additive slice of the typed
//! block-parameter cycle. [`BlockParamsDyn`] — "the type system does not
//! know this block's parameter shape" — is the only inhabitant for now
//! and the default for every handle's `Params` parameter, so the existing
//! surface keeps compiling unchanged. Typed tuple inhabitants (and the
//! `BlockCall` edge that consumes them) arrive in later slices.
//!
//! The base trait is **sealed** — the set of parameter-shape markers is
//! closed, not an extension point.

/// Base marker trait — the bound a basic-block handle's `Params`
/// parameter carries. The block-parameter analog of
/// [`VecElem`](crate::VecElem).
///
/// Implemented by [`BlockParamsDyn`] (and, in later slices, by typed
/// parameter tuples). Sealed: the parameter-shape set is part of the IR
/// shape, not an extension point.
pub trait BlockParams: sealed::Sealed + Copy + 'static + core::fmt::Debug {}

/// Parameter-erased marker. The handle carries no static promise about
/// its block's parameter shape; this marker only signals "the type system
/// does not know the parameters." The default for a block handle's
/// `Params` parameter, and the block analog of [`ElemDyn`](crate::ElemDyn).
///
/// A block label recovered from an untyped [`Value`](crate::Value) always
/// lands here — an erased value legitimately carries no static parameter
/// promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockParamsDyn;
impl sealed::Sealed for BlockParamsDyn {}
impl BlockParams for BlockParamsDyn {}

mod sealed {
    pub trait Sealed {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_params_dyn_is_copy_and_debug() {
        // The erased marker is a zero-size `Copy + Debug` tag, exactly as
        // the const-generic vector/array element markers are.
        let a = BlockParamsDyn;
        let b = a;
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    fn assert_block_params<P: BlockParams>() {}

    #[test]
    fn block_params_dyn_implements_the_bound() {
        assert_block_params::<BlockParamsDyn>();
    }
}
