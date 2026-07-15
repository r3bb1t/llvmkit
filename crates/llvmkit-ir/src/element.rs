//! Sealed marker types for scalar vector / array element types.
//!
//! Mirrors the static-width split the integer side keeps between
//! [`IntWidth`](crate::IntWidth) (the bound a handle carries) and
//! [`StaticIntWidth`] (the projection subtrait that
//! can name a concrete [`IntType`](crate::IntType) from a module). Here the
//! same split describes the *element* of a typed vector or array.
//!
//! ## Rust scalars *are* the element markers
//!
//! A typed vector reads `VectorValue<i64, Len<4>>` and a typed array
//! `ArrayValue<f64, ArrLen<8>>` — the element type is spelled by the same
//! scalar marker the scalar handles use (`IntValue<i64>`,
//! `FloatValue<f64>`). The marker set is therefore exactly the int-width
//! scalar markers (`bool`, `i8`, `i16`, `i32`, `i64`, `i128`) plus the
//! float-kind markers (`f32`, `f64`, [`Half`],
//! [`BFloat`], [`Fp128`],
//! [`X86Fp80`], [`PpcFp128`]). Only
//! [`ElemDyn`] needs a dedicated struct, since "element unknown at compile
//! time" has no Rust counterpart.
//!
//! This slice ships leaf scalar elements only; nested aggregates (arrays of
//! arrays, vectors of pointers) are future work and deliberately absent
//! from the marker set.
//!
//! This module replaces the earlier unwired `vector_element` /
//! `sized_element` scaffolds, whose element-type-as-*type-handle* markers
//! (`VectorElement` / `SizedElement`) never got a const-generic consumer.
//!
//! The base trait is **sealed** — the set of leaf element markers is closed,
//! not an extension point.

use crate::float_kind::{BFloat, Fp128, Half, PpcFp128, StaticFloatKind, X86Fp80};
use crate::int_width::StaticIntWidth;
use crate::module::{ModuleBrand, ModuleRef};
use crate::r#type::Type;
use crate::value::{FloatValue, IntValue, IsValue, Value};

/// Base marker trait — the bound a typed vector/array handle's element
/// parameter carries. The element analog of [`IntWidth`](crate::IntWidth).
///
/// Implemented by the int-width scalar markers, the float-kind markers, and
/// [`ElemDyn`]. Sealed: the leaf element set is part of the IR shape, not an
/// extension point.
pub trait VecElem: sealed::Sealed + Copy + 'static + core::fmt::Debug {}

/// Element-erased marker. The handle still tracks its element type as
/// runtime data; this marker only signals "the type system does not know
/// the element." The default for a typed handle's element parameter, and
/// the element analog of [`IntDyn`](crate::IntDyn).
///
/// Used by parsed IR and APIs that genuinely cannot be statically typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElemDyn;
impl sealed::Sealed for ElemDyn {}
impl VecElem for ElemDyn {}

/// Projection subtrait — element markers whose element type is known at
/// compile time AND whose IR type / value handle can be projected from a
/// [`Module`](crate::Module) without an extra runtime parameter. The element
/// analog of [`StaticIntWidth`] /
/// [`StaticFloatKind`].
///
/// Parameterised over `'ctx, B` so the associated value type can name them.
/// Not implemented for [`ElemDyn`] — there is no single static element type
/// for an erased handle.
pub trait StaticVecElem<'ctx, B: ModuleBrand>: VecElem {
    /// The scalar value handle for this element: what `extractelement`
    /// returns and what `insertelement` / `splat` accept. `IntValue` for the
    /// int markers, `FloatValue` for the float markers.
    type Value: IsValue<'ctx, B> + Copy;

    /// Project the marker into the matching erased element [`Type`] from the
    /// caller's module.
    fn element_ir_type(module: ModuleRef<'ctx, B>) -> Type<'ctx, B>;

    /// Wrap a [`Value`] known-by-construction to have this element type into
    /// the typed scalar handle.
    fn wrap_value(v: Value<'ctx, B>) -> Self::Value;
}

// Int scalar markers. Concrete per-marker impls (not a blanket
// `impl<W: StaticIntWidth> VecElem for W`) so the float blanket below cannot
// collide on coherence.
macro_rules! impl_vec_elem_int {
    ($($ty:ty),+ $(,)?) => { $(
        impl sealed::Sealed for $ty {}
        impl VecElem for $ty {}
        impl<'ctx, B: ModuleBrand + 'ctx> StaticVecElem<'ctx, B> for $ty {
            type Value = IntValue<'ctx, $ty, B>;
            #[inline]
            fn element_ir_type(module: ModuleRef<'ctx, B>) -> Type<'ctx, B> {
                <$ty as StaticIntWidth>::ir_type(module).as_type()
            }
            #[inline]
            fn wrap_value(v: Value<'ctx, B>) -> Self::Value {
                IntValue::<$ty, B>::from_value_unchecked(v)
            }
        }
    )+ };
}
impl_vec_elem_int!(bool, i8, i16, i32, i64, i128);

// Float markers. Concrete per-marker impls for the same coherence reason.
macro_rules! impl_vec_elem_float {
    ($($ty:ty),+ $(,)?) => { $(
        impl sealed::Sealed for $ty {}
        impl VecElem for $ty {}
        impl<'ctx, B: ModuleBrand + 'ctx> StaticVecElem<'ctx, B> for $ty {
            type Value = FloatValue<'ctx, $ty, B>;
            #[inline]
            fn element_ir_type(module: ModuleRef<'ctx, B>) -> Type<'ctx, B> {
                <$ty as StaticFloatKind>::ir_type(module).as_type()
            }
            #[inline]
            fn wrap_value(v: Value<'ctx, B>) -> Self::Value {
                FloatValue::<$ty, B>::from_value_unchecked(v)
            }
        }
    )+ };
}
impl_vec_elem_float!(f32, f64, Half, BFloat, Fp128, X86Fp80, PpcFp128);

mod sealed {
    pub trait Sealed {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Module;

    #[test]
    fn static_vec_elem_projects_scalar_element_types() {
        Module::with_new("element-marker", |m| {
            let module = m.module_ref();
            assert_eq!(
                <i32 as StaticVecElem<'_, _>>::element_ir_type(module),
                m.i32_type().as_type(),
            );
            assert_eq!(
                <f32 as StaticVecElem<'_, _>>::element_ir_type(module),
                m.f32_type().as_type(),
            );
        });
    }
}
