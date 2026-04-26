//! Source-location markers attached to IR values. Mirrors the public
//! face of `llvm/include/llvm/IR/DebugLoc.h`.
//!
//! The full debug-info subsystem (DI metadata) is part of Phase F. For
//! the value-layer foundation we expose [`DebugLoc`] only as an opaque
//! handle and reserve the optional slot on every value. Phase F will
//! introduce constructors and accessors; until then the field stays
//! `None` for every value the IR builder produces, and pattern matches
//! ignore it.

use core::num::NonZeroU32;

/// Opaque debug-location handle.
///
/// `NonZeroU32` so `Option<DebugLoc>` is the same size as `DebugLoc`
/// itself. The internal numeric meaning will be defined by the metadata
/// system; until then no value is reachable from safe code, so every
/// `Option<DebugLoc>` field stays `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DebugLoc(NonZeroU32);
