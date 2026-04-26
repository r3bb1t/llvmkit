#![forbid(unsafe_code)]
//! `llvmkit` is the public umbrella for the workspace.
//!
//! The crate is intentionally thin: it re-exports the implementation crates
//! so downstream users only need a single dependency.
//!
//! | Module        | Source                                  |
//! |---------------|-----------------------------------------|
//! | [`support`]   | [`llvmkit-support`](::llvmkit_support)  |
//! | [`asmparser`] | [`llvmkit-asmparser`](::llvmkit_asmparser) |
//! | [`ir`]        | [`llvmkit-ir`](::llvmkit_ir)            |

pub use llvmkit_asmparser as asmparser;
pub use llvmkit_ir as ir;
pub use llvmkit_support as support;
