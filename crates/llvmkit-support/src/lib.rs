#![forbid(unsafe_code)]
//! Shared support utilities for the `llvmkit` family.
//!
//! Currently exposes [`Span`], [`Spanned`], and [`SourceMap`]. Future helpers
//! (e.g. `APInt`, `APFloat`, diagnostic rendering) land here.

pub mod source_map;
pub mod span;

pub use source_map::SourceMap;
pub use span::{Span, Spanned};
