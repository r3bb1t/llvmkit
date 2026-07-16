//! Sealed marker types for a basic block's typed parameter list.
//!
//! The block analog of [`element`](crate::element): where that module
//! pins a typed vector/array element at the type level, this one pins the
//! parameter shape a basic-block handle carries. A block marked with a
//! typed parameter tuple is a different type from a block marked
//! [`BlockParamsDyn`], so mixing them at a branch/edge call site becomes
//! a compile error rather than a runtime shape mismatch.
//!
//! ## Inhabitants
//!
//! [`BlockParamsDyn`] — "the type system does not know this block's
//! parameter shape" — is the erased inhabitant and the default for every
//! handle's `Params` parameter, so the parameter-erased surface keeps
//! compiling unchanged. Typed inhabitants are the same
//! [`FunctionParamList`](crate::FunctionParamList) tuples that describe a
//! function's parameter list: a block marked `(i32, Ptr)` yields typed
//! head-phi parameter handles from
//! [`IRBuilder::append_block_typed`](crate::IRBuilder::append_block_typed).
//! `BlockParamsDyn` and a parameter tuple are distinct types, so there is
//! no coherence conflict with the `FunctionParamList` tuple impls. (The
//! `BlockCall` edge that consumes typed block parameters arrives in a
//! later slice.)
//!
//! The base trait is **sealed** — the set of parameter-shape markers is
//! closed, not an extension point.

use crate::function_signature::FunctionParam;

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

// The unit tuple is the arity-0 typed inhabitant, mirroring
// `FunctionParamList for ()` — a block that statically promises *no*
// block-arguments, distinct from the erased `BlockParamsDyn`.
impl sealed::Sealed for () {}
impl BlockParams for () {}

/// Seal + implement [`BlockParams`] for each parameter tuple that is also
/// a [`FunctionParamList`](crate::FunctionParamList). Each element carries
/// `Copy + Debug` so the tuple satisfies [`BlockParams`]'s
/// `Copy + 'static + Debug` supertrait bound (`'static` comes from
/// [`FunctionParam`]); every real schema token (`i32`, `Ptr`, `Width<N>`,
/// the float markers, …) derives both, so this bound never turns a valid
/// `FunctionParamList` tuple away.
macro_rules! impl_block_params_tuple {
    ($($param:ident),+) => {
        impl<$($param),+> sealed::Sealed for ($($param,)+)
        where
            $($param: FunctionParam + Copy + core::fmt::Debug,)+
        {
        }

        impl<$($param),+> BlockParams for ($($param,)+)
        where
            $($param: FunctionParam + Copy + core::fmt::Debug,)+
        {
        }
    };
}

// Arities 1..=12 only. `BlockParams` requires `Debug`, and the standard
// library implements `Debug` for tuples up to arity 12; 13..=16 tuples are
// `Copy` but not `Debug`, so they cannot satisfy the `BlockParams`
// supertrait even though they are `FunctionParamList`. A block with more
// than twelve typed parameters is not expressible — such a block must use
// the erased `BlockParamsDyn` form.
impl_block_params_tuple!(A0);
impl_block_params_tuple!(A0, A1);
impl_block_params_tuple!(A0, A1, A2);
impl_block_params_tuple!(A0, A1, A2, A3);
impl_block_params_tuple!(A0, A1, A2, A3, A4);
impl_block_params_tuple!(A0, A1, A2, A3, A4, A5);
impl_block_params_tuple!(A0, A1, A2, A3, A4, A5, A6);
impl_block_params_tuple!(A0, A1, A2, A3, A4, A5, A6, A7);
impl_block_params_tuple!(A0, A1, A2, A3, A4, A5, A6, A7, A8);
impl_block_params_tuple!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9);
impl_block_params_tuple!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
impl_block_params_tuple!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);

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

    #[test]
    fn param_tuples_and_unit_implement_the_bound() {
        // The arity-0 typed inhabitant and the `FunctionParamList` parameter
        // tuples are all `BlockParams`, distinct from the erased marker.
        assert_block_params::<()>();
        assert_block_params::<(i32,)>();
        assert_block_params::<(i32, crate::Ptr)>();
        assert_block_params::<(i32, i64, crate::Ptr, f32)>();
        // Arity 12 — the largest tuple that satisfies the `Debug` supertrait.
        assert_block_params::<(i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32)>();
    }
}
