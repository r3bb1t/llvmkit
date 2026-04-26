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

/// Unnamed-address marker. Mirrors `GlobalValue::UnnamedAddr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
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
