//! Unnamed-address marker for module-level globals. Mirrors the
//! `GlobalValue::UnnamedAddr` enum in
//! `llvm/include/llvm/IR/GlobalValue.h`.
//!
//! Three states:
//!
//! - [`UnnamedAddr::None`] — the address is significant (default).
//! - [`UnnamedAddr::Local`] — `local_unnamed_addr`; address is unique
//!   within the module but the linker may merge across modules.
//! - [`UnnamedAddr::Global`] — `unnamed_addr`; address is unique
//!   globally at link time.

use core::fmt;
use core::str::FromStr;

use crate::error::IrError;

/// Unnamed-address marker. Mirrors `GlobalValue::UnnamedAddr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UnnamedAddr {
    /// Has a unique address (default).
    #[default]
    None,
    /// `local_unnamed_addr` — address is unique within the module.
    Local,
    /// `unnamed_addr` — address is unique globally (link-time).
    Global,
}

impl UnnamedAddr {
    /// Every variant, in declaration order. Exists so [`FromStr`] can invert
    /// [`keyword`](Self::keyword) by searching this list instead of carrying
    /// a second copy of the spelling table; keep it in step with the enum
    /// (the in-file drift-lock test's exhaustive `match` is the tripwire).
    pub const VARIANTS: [Self; 3] = [Self::None, Self::Local, Self::Global];

    /// `.ll` keyword for this marker, or `None` for [`Self::None`]
    /// (no keyword in textual IR).
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Local => Some("local_unnamed_addr"),
            Self::Global => Some("unnamed_addr"),
        }
    }
}

impl fmt::Display for UnnamedAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.keyword() {
            Some(s) => f.write_str(s),
            None => Ok(()),
        }
    }
}

impl FromStr for UnnamedAddr {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) / [`UnnamedAddr::keyword`] over
    /// [`VARIANTS`](Self::VARIANTS). `""` resolves to the keyword-less
    /// [`None`](Self::None), matching what `Display` writes for it, so
    /// `display` then `parse` is the identity on every variant. The lexer
    /// half upstream is the `unnamed_addr` / `local_unnamed_addr` keyword
    /// pair in `LLLexer.cpp`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|marker| marker.keyword().unwrap_or("") == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "unnamed-address marker",
                keyword: s.to_string(),
            })
    }
}

/// Upstream provenance: mirrors `GlobalValue::UnnamedAddr` in
/// `include/llvm/IR/GlobalValue.h`; the spellings are the AsmWriter's
/// (`lib/IR/AsmWriter.cpp`) and the lexer's (`lib/AsmParser/LLLexer.cpp`).
///
/// The test below is **llvmkit-specific**: it is the `Display`/`FromStr`
/// drift lock, the analogue of `attribute_td_drift.rs`. Upstream cannot have
/// this bug — printer and lexer read the same generated tables — so there is
/// nothing to port.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: Display/FromStr drift lock. The exhaustive `match`
    /// makes a new variant a compile error here, which is the prompt to
    /// extend `VARIANTS`; the count assertion then fails until it is
    /// extended.
    #[test]
    fn unnamed_addr_display_and_from_str_round_trip() {
        for marker in UnnamedAddr::VARIANTS {
            match marker {
                UnnamedAddr::None | UnnamedAddr::Local | UnnamedAddr::Global => {}
            }
            assert_eq!(marker.to_string().parse::<UnnamedAddr>(), Ok(marker));
        }
        // One entry per arm above.
        assert_eq!(UnnamedAddr::VARIANTS.len(), 3);
        assert_eq!("".parse::<UnnamedAddr>(), Ok(UnnamedAddr::None));
        assert_eq!(
            "nosuchmarker".parse::<UnnamedAddr>(),
            Err(IrError::InvalidKeyword {
                target: "unnamed-address marker",
                keyword: "nosuchmarker".to_string(),
            })
        );
    }
}
