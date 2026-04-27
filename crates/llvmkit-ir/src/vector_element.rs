//! Sealed-trait scaffolding for valid `VectorType` element types.
//! Mirrors `VectorType::isValidElementType` in `lib/IR/Type.cpp`,
//! which restricts element types to integer / float / pointer.
//!
//! The trait is used by Doctrine D6 (aggregate types parameterise
//! over element shape). Today llvmkit's [`crate::VectorType<'ctx>`]
//! still erases the element type at the value level; this module
//! ships the marker hierarchy that the future const-generic
//! parameterisation will key off (see `RLLVM_TYPE_SAFETY_SWEEP.md`
//! T4 for the staged plan).

use crate::value::sealed;

/// Sealed: types accepted as a `VectorType` element. Implemented for
/// every `IntType<'ctx, W>`, `FloatType<'ctx, K>`, and `PointerType`.
pub trait VectorElement: sealed::Sealed {}

/// Runtime-checked vector-element marker. Used as the default for the
/// existing erased `VectorType<'ctx>` API surface so adding the param
/// does not break consumer code.
#[derive(Debug, Clone, Copy)]
pub struct VectorDyn;

impl sealed::Sealed for VectorDyn {}
impl VectorElement for VectorDyn {}

// Concrete impls. The width / kind generics are explicit so coherence
// stays sane.
impl<'ctx, W: crate::int_width::IntWidth> sealed::Sealed for crate::IntType<'ctx, W> {}
impl<'ctx, W: crate::int_width::IntWidth> VectorElement for crate::IntType<'ctx, W> {}

impl<'ctx, K: crate::float_kind::FloatKind> sealed::Sealed for crate::FloatType<'ctx, K> {}
impl<'ctx, K: crate::float_kind::FloatKind> VectorElement for crate::FloatType<'ctx, K> {}

impl<'ctx> sealed::Sealed for crate::PointerType<'ctx> {}
impl<'ctx> VectorElement for crate::PointerType<'ctx> {}
