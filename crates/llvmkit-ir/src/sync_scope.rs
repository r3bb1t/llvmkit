//! Synchronization-scope tag for atomic ops. Mirrors
//! `llvm/include/llvm/IR/LLVMContext.h::SyncScope`.

use core::fmt;

/// Synchronization scope for atomic ops. Mirrors
/// `namespace SyncScope` in `IR/LLVMContext.h`. The two well-known
/// scope IDs (`SingleThread = 0`, `System = 1`) get their own variants;
/// any other named scope (`workgroup`, `wavefront`, target-specific
/// scopes, ...) is carried as a [`Named`](Self::Named) variant.
///
/// The IR text form omits the `syncscope("system")` qualifier when the
/// scope is the default ([`System`](Self::System)) and prints
/// `syncscope("<name>")` otherwise. The bare keyword `singlethread`
/// has no `syncscope(...)` wrapping --- it is an alias spelled
/// `syncscope("singlethread")` in canonical IR text. Mirrors the
/// printer in `lib/IR/AsmWriter.cpp::writeAtomic`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SyncScope {
    /// Synchronized only with respect to signal handlers in the same
    /// thread (`SyncScope::SingleThread = 0`).
    SingleThread,
    /// Synchronized with respect to all concurrently executing threads.
    /// The default (`SyncScope::System = 1`).
    System,
    /// Target-specific named scope (e.g. `workgroup`, `wavefront`).
    /// Mirrors the LangRef `syncscope("<name>")` form.
    Named(String),
}

impl SyncScope {
    /// `true` if this is the default `system` scope (no
    /// `syncscope(...)` qualifier in IR text).
    pub fn is_default(&self) -> bool {
        matches!(self, Self::System)
    }
}

impl fmt::Display for SyncScope {
    /// Pretty-print the IR text form. Mirrors the
    /// `syncscope("<name>")` shape from `lib/IR/AsmWriter.cpp::writeAtomic`.
    /// `System` prints as the empty string (caller skips); `SingleThread`
    /// and `Named(s)` produce `syncscope("singlethread")` /
    /// `syncscope("<s>")`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System => Ok(()),
            Self::SingleThread => f.write_str("syncscope(\"singlethread\")"),
            Self::Named(s) => write!(f, "syncscope({s:?})"),
        }
    }
}
