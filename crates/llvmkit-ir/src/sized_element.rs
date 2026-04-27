//! Sealed-trait scaffolding for valid [`crate::ArrayType`] element
//! types. Mirrors `ArrayType::isValidElementType` in `lib/IR/Type.cpp`,
//! which rejects `void`, `label`, `metadata`, `function`, and `token`
//! (the "sized" predicate).
//!
//! Like [`crate::vector_element`], this module is the marker scaffold
//! Doctrine D6's aggregate-element parameterisation will key off when
//! `ArrayType<'ctx, E, const N>` lands. Today the array element type
//! is still tracked at runtime by [`crate::ArrayType<'ctx>`].

use crate::value::sealed;

/// Sealed: types accepted as an `ArrayType` element. Implemented for
/// every integer width, float kind, pointer, and (transitively) every
/// nested aggregate.
pub trait SizedElement: sealed::Sealed {}

/// Runtime-checked array-element marker.
#[derive(Debug, Clone, Copy)]
pub struct ArrayDyn;

impl sealed::Sealed for ArrayDyn {}
impl SizedElement for ArrayDyn {}

// Reuse the per-kind sealed impls from [`vector_element`]:
// [`crate::IntType`], [`crate::FloatType`], and [`crate::PointerType`]
// are already `sealed::Sealed`; we add the `SizedElement` marker.
impl<'ctx, W: crate::int_width::IntWidth> SizedElement for crate::IntType<'ctx, W> {}
impl<'ctx, K: crate::float_kind::FloatKind> SizedElement for crate::FloatType<'ctx, K> {}
impl<'ctx> SizedElement for crate::PointerType<'ctx> {}
