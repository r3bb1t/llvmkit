//! Generator errors.
//!
//! Ports `llvm/lib/TableGen/Error.cpp`.

use crate::*;

pub(crate) type GenResult<T> = Result<T, TableGenError>;

/// An error from the TableGen front end or the intrinsic emitter.
///
/// Carries the rendered message; the source position, when one exists, is
/// already folded into the text, as `llvm/lib/TableGen/Error.cpp` does when it
/// prints against the `SourceMgr`.
#[derive(Debug, Clone)]
pub struct TableGenError {
    pub(crate) message: String,
}

impl TableGenError {
    pub(crate) fn new<M>(message: M) -> Self
    where
        M: Into<String>,
    {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TableGenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TableGenError {}
