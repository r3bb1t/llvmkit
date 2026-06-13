#![forbid(unsafe_code)]
//! `llvmkit` is the public umbrella for the workspace.
//!
//! The crate is intentionally thin: it groups the implementation crates under
//! stable module names so downstream users only need a single dependency.
//!
//! Start with [`ir`] for typed LLVM IR construction and verification,
//! [`asmparser`] for textual `.ll` parsing, and [`support`] for shared source
//! location utilities.

pub mod asmparser {
    pub use llvmkit_asmparser::*;
}

pub mod ir {
    pub use llvmkit_ir::*;
}

pub mod support {
    pub use llvmkit_support::*;
}
