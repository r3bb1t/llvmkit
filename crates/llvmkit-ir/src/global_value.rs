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

/// Visibility marker. Mirrors `GlobalValue::VisibilityTypes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Visibility {
    /// The GV is visible (default).
    #[default]
    Default,
    /// The GV is hidden.
    Hidden,
    /// The GV is protected.
    Protected,
}

impl Visibility {
    /// `.ll` keyword for this visibility, or `None` for
    /// [`Self::Default`] (no keyword in textual IR).
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Hidden => Some("hidden"),
            Self::Protected => Some("protected"),
        }
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.keyword() {
            Some(s) => f.write_str(s),
            None => Ok(()),
        }
    }
}

/// DLL storage class. Mirrors `GlobalValue::DLLStorageClassTypes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DllStorageClass {
    /// No DLL storage class (default).
    #[default]
    Default,
    /// `dllimport` -- to be imported from a DLL.
    DllImport,
    /// `dllexport` -- to be accessible from a DLL.
    DllExport,
}

impl DllStorageClass {
    /// `.ll` keyword for this DLL storage class, or `None` for
    /// [`Self::Default`] (no keyword in textual IR).
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::DllImport => Some("dllimport"),
            Self::DllExport => Some("dllexport"),
        }
    }
}

impl fmt::Display for DllStorageClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.keyword() {
            Some(s) => f.write_str(s),
            None => Ok(()),
        }
    }
}

/// Thread-local mode. Mirrors `GlobalValue::ThreadLocalMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ThreadLocalMode {
    /// Not thread-local (default).
    #[default]
    NotThreadLocal,
    /// `thread_local` -- general-dynamic TLS model.
    GeneralDynamic,
    /// `thread_local(localdynamic)`.
    LocalDynamic,
    /// `thread_local(initialexec)`.
    InitialExec,
    /// `thread_local(localexec)`.
    LocalExec,
}

impl ThreadLocalMode {
    /// `.ll` keyword for this TLS mode, or `None` for
    /// [`Self::NotThreadLocal`] (no keyword in textual IR). Mirrors
    /// `printThreadLocalModel` in `lib/IR/AsmWriter.cpp`.
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::NotThreadLocal => None,
            Self::GeneralDynamic => Some("thread_local"),
            Self::LocalDynamic => Some("thread_local(localdynamic)"),
            Self::InitialExec => Some("thread_local(initialexec)"),
            Self::LocalExec => Some("thread_local(localexec)"),
        }
    }

    /// Returns `true` if this is any flavour of thread-local. Mirrors
    /// `GlobalValue::isThreadLocal`.
    #[inline]
    pub const fn is_thread_local(self) -> bool {
        !matches!(self, Self::NotThreadLocal)
    }
}

impl fmt::Display for ThreadLocalMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.keyword() {
            Some(s) => f.write_str(s),
            None => Ok(()),
        }
    }
}
