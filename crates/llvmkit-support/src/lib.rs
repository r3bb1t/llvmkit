#![forbid(unsafe_code)]
//! Shared support utilities for the `llvmkit` family.
//!
//! Exposes [`Span`], [`Spanned`], and [`SourceMap`] — the source-location
//! vocabulary shared by the lexer, the parser, and IR diagnostics. It has no
//! dependencies of its own.
//!
//! Deliberately narrow: this crate holds only what more than one sibling crate
//! needs. `ApInt` / `ApFloat` are **not** here — they model LLVM IR values and
//! live in `llvmkit-ir` alongside the types that use them.

pub mod source_map;
pub mod span;

pub use source_map::SourceMap;
pub use span::{Span, Spanned};
