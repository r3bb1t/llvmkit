//! Shared shape for module-level globals (functions, variables,
//! aliases, ifuncs). Mirrors `llvm/include/llvm/IR/GlobalValue.h`.
//!
//! Phase D ships only the linkage axis and only what
//! [`Function`](crate::function::FunctionValue) needs from it. Every
//! other field on the upstream `GlobalValue` (visibility, DLL storage
//! class, thread-local mode, unnamed-addr, comdat, section,
//! sanitizer-metadata, partition, etc.) is deferred until proper
//! globals land.

use core::fmt;
use core::str::FromStr;

use crate::error::IrError;

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
    LinkOnceOdr,
    /// Keep one copy; weak.
    WeakAny,
    /// Keep one copy; weak; ODR asserted.
    WeakOdr,
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
    /// Every variant, in declaration order. Exists so
    /// [`FromStr`] can invert [`keyword`](Self::keyword) by searching this
    /// list instead of carrying a second copy of the spelling table; keep it
    /// in step with the enum (the in-file drift-lock test's exhaustive
    /// `match` is the tripwire).
    pub const VARIANTS: [Self; 11] = [
        Self::External,
        Self::AvailableExternally,
        Self::LinkOnceAny,
        Self::LinkOnceOdr,
        Self::WeakAny,
        Self::WeakOdr,
        Self::Appending,
        Self::Internal,
        Self::Private,
        Self::ExternalWeak,
        Self::Common,
    ];

    /// `.ll` keyword for this linkage, or `""` for `External` (which
    /// has no explicit keyword in textual IR).
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::External => "",
            Self::AvailableExternally => "available_externally",
            Self::LinkOnceAny => "linkonce",
            Self::LinkOnceOdr => "linkonce_odr",
            Self::WeakAny => "weak",
            Self::WeakOdr => "weak_odr",
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

impl FromStr for Linkage {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) / [`Linkage::keyword`], found by
    /// searching [`VARIANTS`](Self::VARIANTS) — the spellings live only in
    /// `keyword`, so the two directions cannot drift. `""` resolves to
    /// [`External`](Self::External), whose textual form *is* the absence of a
    /// keyword, so `display` then `parse` is the identity on every variant.
    /// The lexer half upstream is the linkage keyword block in `LLLexer.cpp`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|linkage| linkage.keyword() == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "linkage",
                keyword: s.to_string(),
            })
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
    /// Every variant, in declaration order. See
    /// [`Linkage::VARIANTS`] for why it exists.
    pub const VARIANTS: [Self; 3] = [Self::Default, Self::Hidden, Self::Protected];

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

impl FromStr for Visibility {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) / [`Visibility::keyword`] over
    /// [`VARIANTS`](Self::VARIANTS). The keyword-less
    /// [`Default`](Self::Default) is spelled `""`, matching what `Display`
    /// writes for it, so `display` then `parse` is the identity on every
    /// variant. Mirrors the visibility keyword block in `LLLexer.cpp`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|visibility| visibility.keyword().unwrap_or("") == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "visibility",
                keyword: s.to_string(),
            })
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
    /// Every variant, in declaration order. See
    /// [`Linkage::VARIANTS`] for why it exists.
    pub const VARIANTS: [Self; 3] = [Self::Default, Self::DllImport, Self::DllExport];

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

impl FromStr for DllStorageClass {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) /
    /// [`DllStorageClass::keyword`] over [`VARIANTS`](Self::VARIANTS); `""`
    /// resolves to the keyword-less [`Default`](Self::Default). Mirrors the
    /// `dllimport` / `dllexport` keyword block in `LLLexer.cpp`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|class| class.keyword().unwrap_or("") == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "dll storage class",
                keyword: s.to_string(),
            })
    }
}

/// DSO locality marker. Mirrors `GlobalValue::DSOLocalEquivalent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DsoLocality {
    /// No explicit DSO-locality marker.
    #[default]
    Default,
    /// `dso_local`.
    Local,
    /// `dso_preemptable`.
    Preemptable,
}

impl DsoLocality {
    /// Every variant, in declaration order. See
    /// [`Linkage::VARIANTS`] for why it exists.
    pub const VARIANTS: [Self; 3] = [Self::Default, Self::Local, Self::Preemptable];

    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Local => Some("dso_local"),
            Self::Preemptable => Some("dso_preemptable"),
        }
    }
}

impl fmt::Display for DsoLocality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.keyword() {
            Some(s) => f.write_str(s),
            None => Ok(()),
        }
    }
}

impl FromStr for DsoLocality {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) / [`DsoLocality::keyword`] over
    /// [`VARIANTS`](Self::VARIANTS); `""` resolves to the keyword-less
    /// [`Default`](Self::Default). Mirrors the `dso_local` /
    /// `dso_preemptable` keyword block in `LLLexer.cpp`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|locality| locality.keyword().unwrap_or("") == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "dso locality",
                keyword: s.to_string(),
            })
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
    /// Every variant, in declaration order. See
    /// [`Linkage::VARIANTS`] for why it exists.
    pub const VARIANTS: [Self; 5] = [
        Self::NotThreadLocal,
        Self::GeneralDynamic,
        Self::LocalDynamic,
        Self::InitialExec,
        Self::LocalExec,
    ];

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

impl FromStr for ThreadLocalMode {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) / [`ThreadLocalMode::keyword`]
    /// over [`VARIANTS`](Self::VARIANTS); `""` resolves to the keyword-less
    /// [`NotThreadLocal`](Self::NotThreadLocal). Note the accepted spellings
    /// are the *whole* printed forms — `thread_local(localdynamic)`, not the
    /// bare model name — because that is what
    /// [`keyword`](Self::keyword) emits (`printThreadLocalModel`,
    /// `lib/IR/AsmWriter.cpp`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|mode| mode.keyword().unwrap_or("") == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "thread-local mode",
                keyword: s.to_string(),
            })
    }
}

/// Upstream provenance: the enums mirror `GlobalValue::LinkageTypes`,
/// `VisibilityTypes`, `DLLStorageClassTypes` and `ThreadLocalMode` from
/// `include/llvm/IR/GlobalValue.h`; the spellings are the AsmWriter's
/// (`lib/IR/AsmWriter.cpp`) and the lexer's (`lib/AsmParser/LLLexer.cpp`).
///
/// The tests below are **llvmkit-specific**: they are the `Display`/`FromStr`
/// drift lock, the analogue of `attribute_td_drift.rs`. Upstream cannot have
/// this bug — `LLLexer.cpp` and `AsmWriter.cpp` read the same enum, and the
/// keyword tables are generated — so there is nothing to port. llvmkit spells
/// each keyword exactly once, in the enum's own `keyword()`, and these pin
/// that parsing the printed form recovers the value for *every* variant.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: Display/FromStr drift lock for [`Linkage`]. The
    /// exhaustive `match` makes a new variant a compile error here, which is
    /// the prompt to extend `VARIANTS`; the count assertion then fails until
    /// it is extended.
    #[test]
    fn linkage_display_and_from_str_round_trip() {
        for linkage in Linkage::VARIANTS {
            match linkage {
                Linkage::External
                | Linkage::AvailableExternally
                | Linkage::LinkOnceAny
                | Linkage::LinkOnceOdr
                | Linkage::WeakAny
                | Linkage::WeakOdr
                | Linkage::Appending
                | Linkage::Internal
                | Linkage::Private
                | Linkage::ExternalWeak
                | Linkage::Common => {}
            }
            assert_eq!(linkage.to_string().parse::<Linkage>(), Ok(linkage));
        }
        // One entry per arm above.
        assert_eq!(Linkage::VARIANTS.len(), 11);
        // The keyword-less variant is reachable through the empty spelling.
        assert_eq!("".parse::<Linkage>(), Ok(Linkage::External));
    }

    /// llvmkit-specific: Display/FromStr drift lock for [`Visibility`].
    #[test]
    fn visibility_display_and_from_str_round_trip() {
        for visibility in Visibility::VARIANTS {
            match visibility {
                Visibility::Default | Visibility::Hidden | Visibility::Protected => {}
            }
            assert_eq!(visibility.to_string().parse::<Visibility>(), Ok(visibility));
        }
        // One entry per arm above.
        assert_eq!(Visibility::VARIANTS.len(), 3);
        assert_eq!("".parse::<Visibility>(), Ok(Visibility::Default));
    }

    /// llvmkit-specific: Display/FromStr drift lock for [`DllStorageClass`].
    #[test]
    fn dll_storage_class_display_and_from_str_round_trip() {
        for class in DllStorageClass::VARIANTS {
            match class {
                DllStorageClass::Default
                | DllStorageClass::DllImport
                | DllStorageClass::DllExport => {}
            }
            assert_eq!(class.to_string().parse::<DllStorageClass>(), Ok(class));
        }
        // One entry per arm above.
        assert_eq!(DllStorageClass::VARIANTS.len(), 3);
        assert_eq!("".parse::<DllStorageClass>(), Ok(DllStorageClass::Default));
    }

    /// llvmkit-specific: Display/FromStr drift lock for [`DsoLocality`].
    #[test]
    fn dso_locality_display_and_from_str_round_trip() {
        for locality in DsoLocality::VARIANTS {
            match locality {
                DsoLocality::Default | DsoLocality::Local | DsoLocality::Preemptable => {}
            }
            assert_eq!(locality.to_string().parse::<DsoLocality>(), Ok(locality));
        }
        // One entry per arm above.
        assert_eq!(DsoLocality::VARIANTS.len(), 3);
        assert_eq!("".parse::<DsoLocality>(), Ok(DsoLocality::Default));
    }

    /// llvmkit-specific: Display/FromStr drift lock for [`ThreadLocalMode`].
    /// The parenthesised models round-trip whole, since that is the form
    /// `printThreadLocalModel` prints.
    #[test]
    fn thread_local_mode_display_and_from_str_round_trip() {
        for mode in ThreadLocalMode::VARIANTS {
            match mode {
                ThreadLocalMode::NotThreadLocal
                | ThreadLocalMode::GeneralDynamic
                | ThreadLocalMode::LocalDynamic
                | ThreadLocalMode::InitialExec
                | ThreadLocalMode::LocalExec => {}
            }
            assert_eq!(mode.to_string().parse::<ThreadLocalMode>(), Ok(mode));
        }
        // One entry per arm above.
        assert_eq!(ThreadLocalMode::VARIANTS.len(), 5);
        assert_eq!(
            "".parse::<ThreadLocalMode>(),
            Ok(ThreadLocalMode::NotThreadLocal)
        );
        assert_eq!(
            "thread_local(initialexec)".parse::<ThreadLocalMode>(),
            Ok(ThreadLocalMode::InitialExec)
        );
    }

    /// llvmkit-specific: the negative half of the drift lock. An unknown
    /// keyword is an error for every member of the family, never a silent
    /// fallback to the default variant. Closest upstream: the `LLParser`
    /// entry points that reject an unrecognised keyword outright.
    #[test]
    fn unknown_global_value_keywords_are_rejected() {
        assert_eq!(
            "nosuchlinkage".parse::<Linkage>(),
            Err(IrError::InvalidKeyword {
                target: "linkage",
                keyword: "nosuchlinkage".to_string(),
            })
        );
        assert!("hiddenish".parse::<Visibility>().is_err());
        assert!("dllmaybe".parse::<DllStorageClass>().is_err());
        assert!("dso_whatever".parse::<DsoLocality>().is_err());
        // The bare model name is not a printed form, so it is not accepted.
        assert!("localdynamic".parse::<ThreadLocalMode>().is_err());
    }
}
