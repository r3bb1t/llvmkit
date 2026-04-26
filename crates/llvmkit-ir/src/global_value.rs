//! Shared shape for module-level globals (functions, variables,
//! aliases, ifuncs). Mirrors `llvm/include/llvm/IR/GlobalValue.h`.
//!
//! Phase D ships only the linkage axis and only what
//! [`Function`](crate::function::FunctionValue) needs from it. Every
//! other field on the upstream `GlobalValue` (visibility, DLL storage
//! class, thread-local mode, unnamed-addr, comdat, section,
//! sanitizer-metadata, partition, etc.) is deferred to a session that
//! lands proper globals.

use core::fmt;

/// Linkage type. Mirrors `GlobalValue::LinkageTypes`. The discriminant
/// values are not stable across LLVM bitcode versions, so we encode
/// only the symbolic set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Linkage {
    /// Externally visible function/variable.
    External,
    /// Available for inspection but not emission.
    AvailableExternally,
    /// Keep one copy; ODR not asserted.
    LinkOnceAny,
    /// Keep one copy; ODR asserted.
    LinkOnceODR,
    /// Keep one copy; weak.
    WeakAny,
    /// Keep one copy; weak; ODR asserted.
    WeakODR,
    /// Special-case linkage for `@llvm.global_ctors` / similar.
    Appending,
    /// Internal to the translation unit.
    Internal,
    /// Like `Internal` but renamed at link time to avoid conflicts.
    Private,
    /// External, but only referenced if defined elsewhere.
    ExternalWeak,
    /// Tentative definitions (Common-style C linkage).
    Common,
}

impl Default for Linkage {
    /// LLVM's default linkage for `Function` / `GlobalVariable` is
    /// `External`.
    #[inline]
    fn default() -> Self {
        Self::External
    }
}

impl Linkage {
    /// `.ll` keyword for this linkage, or `""` for `External` (which
    /// has no explicit keyword in textual IR).
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::External => "",
            Self::AvailableExternally => "available_externally",
            Self::LinkOnceAny => "linkonce",
            Self::LinkOnceODR => "linkonce_odr",
            Self::WeakAny => "weak",
            Self::WeakODR => "weak_odr",
            Self::Appending => "appending",
            Self::Internal => "internal",
            Self::Private => "private",
            Self::ExternalWeak => "extern_weak",
            Self::Common => "common",
        }
    }
}

impl fmt::Display for Linkage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}
