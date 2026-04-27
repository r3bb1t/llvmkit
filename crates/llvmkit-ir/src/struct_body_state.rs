//! Body-state typestate for [`crate::StructType`] (Doctrine D1 --
//! make invalid states unrepresentable).
//!
//! LLVM's `StructType::setBody` (`lib/IR/Type.cpp`) asserts at runtime
//! that the named struct has not already received a body. llvmkit's
//! [`crate::Module::set_struct_body`] consumes a `StructType<Opaque>`
//! and produces a `StructType<BodySet>`; calling `set_struct_body`
//! twice is a compile error because the second call cannot acquire
//! another `StructType<Opaque>` for the same id.
//!
//! [`StructBodyDyn`] is the runtime-checked default used by parsed
//! modules and existing single-shot constructors that leave the
//! body-set determination to runtime.

use crate::value::sealed;

/// Sealed marker trait for the [`crate::StructType`] body-state
/// type parameter. The three implementors are [`Opaque`],
/// [`BodySet`], and [`StructBodyDyn`]; external crates cannot invent
/// new states.
pub trait StructBodyState: sealed::Sealed + 'static {}

/// Marker: the named struct has been declared but no field list has
/// been set yet.
#[derive(Debug, Clone, Copy)]
pub struct Opaque;

/// Marker: the struct has a fixed field list. Mirrors LLVM's
/// `StructType::isOpaque() == false`.
#[derive(Debug, Clone, Copy)]
pub struct BodySet;

/// Runtime-checked marker for parsed / legacy code paths. Serves as
/// the default so existing `StructType<'ctx>` references continue to
/// compile.
#[derive(Debug, Clone, Copy)]
pub struct StructBodyDyn;

impl sealed::Sealed for Opaque {}
impl sealed::Sealed for BodySet {}
impl sealed::Sealed for StructBodyDyn {}

impl StructBodyState for Opaque {}
impl StructBodyState for BodySet {}
impl StructBodyState for StructBodyDyn {}
