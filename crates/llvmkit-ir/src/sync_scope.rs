//! Synchronization-scope tag for atomic ops. Mirrors
//! `llvm/include/llvm/IR/LLVMContext.h::SyncScope`.

use core::fmt;
use core::str::FromStr;

use crate::error::IrError;

/// Synchronization scope for atomic ops. Mirrors
/// `namespace SyncScope` in `IR/LLVMContext.h`. The two well-known
/// scope IDs (`SingleThread = 0`, `System = 1`) get their own variants;
/// any other named scope (`workgroup`, `wavefront`, target-specific
/// scopes, ...) is carried as a [`Named`](Self::Named) variant.
///
/// [`System`](Self::System)'s canonical *name* is the empty string:
/// `LLVMContext::LLVMContext` seeds `getOrInsertSyncScopeID` with
/// `"singlethread"` and `""`, so its IR text form is the *absence* of a
/// `syncscope(...)` qualifier, and the literal spelling
/// `syncscope("system")` denotes an ordinary named scope distinct from
/// the default ([`Named`](Self::Named)`("system")`). The bare keyword
/// `singlethread` has no `syncscope(...)` wrapping --- it is an alias
/// spelled `syncscope("singlethread")` in canonical IR text. Mirrors
/// the printer in `lib/IR/AsmWriter.cpp::writeAtomic`.
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

impl FromStr for SyncScope {
    type Err = IrError;

    /// Resolve a **bare scope name** — the string inside `syncscope("…")`,
    /// not the wrapper. Inverse of `LLVMContext::getOrInsertSyncScopeID`
    /// (`lib/IR/LLVMContext.cpp`), which seeds `"singlethread"` and `""` as
    /// the two well-known IDs and interns everything else as a fresh named
    /// scope.
    ///
    /// This is deliberately **not** the inverse of
    /// [`Display`](fmt::Display), which prints the `syncscope("…")` wrapper
    /// (and nothing at all for [`System`](Self::System)) because that is what
    /// `AsmWriter::writeAtomic` emits at a use site. Feeding a printed scope
    /// back through `parse` would therefore not round-trip, and the
    /// drift-lock tests exclude this type for that reason.
    ///
    /// Never fails: any name that is not one of the two well-known ones is a
    /// legitimate target-specific scope. `Err = IrError` only because the
    /// trait demands an error type; [`Infallible`](core::convert::Infallible)
    /// would make the family's error types inconsistent for no gain.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "singlethread" => Self::SingleThread,
            // `""` is `System`'s canonical name upstream; `"system"` is an
            // ordinary named scope and stays one (see the type docs).
            "" => Self::System,
            name => Self::Named(name.to_string()),
        })
    }
}

/// Upstream provenance: mirrors `namespace SyncScope` in
/// `include/llvm/IR/LLVMContext.h` and the seeding in
/// `LLVMContext::LLVMContext` / `getOrInsertSyncScopeID`
/// (`lib/IR/LLVMContext.cpp`).
///
/// The test below is **llvmkit-specific**: no upstream unit test drives
/// `getOrInsertSyncScopeID`'s well-known-name partition directly.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: `FromStr` is the bare-name inverse of
    /// `LLVMContext::getOrInsertSyncScopeID`, not of `Display`. Only
    /// `"singlethread"` and `""` reach the well-known IDs; `"system"` is an
    /// ordinary named scope, exactly as upstream registers it.
    #[test]
    fn from_str_resolves_the_well_known_scope_names() {
        assert_eq!(
            "singlethread".parse::<SyncScope>(),
            Ok(SyncScope::SingleThread)
        );
        assert_eq!("".parse::<SyncScope>(), Ok(SyncScope::System));
        assert_eq!(
            "system".parse::<SyncScope>(),
            Ok(SyncScope::Named("system".to_string()))
        );
        assert_eq!(
            "workgroup".parse::<SyncScope>(),
            Ok(SyncScope::Named("workgroup".to_string()))
        );
        // The printed form is the wrapper, so it is *not* a scope name — this
        // is why `SyncScope` is excluded from the Display round-trip locks.
        assert_eq!(
            "syncscope(\"singlethread\")".parse::<SyncScope>(),
            Ok(SyncScope::Named("syncscope(\"singlethread\")".to_string()))
        );
    }
}
