//! Generator errors.
//!
//! Ports `llvm/lib/TableGen/Error.cpp`.

use crate::*;

pub(crate) type GenResult<T> = Result<T, GenError>;

#[derive(Debug, Clone)]
pub(crate) struct GenError {
    pub(crate) message: String,
}

impl GenError {
    pub(crate) fn new<M>(message: M) -> Self
    where
        M: Into<String>,
    {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GenError {}
