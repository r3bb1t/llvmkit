#![forbid(unsafe_code)]

//! Expands the vendored TableGen tree into this crate's intrinsic tables.
//!
//! The generator itself is `llvmkit-tablegen`, which ports LLVM's
//! `lib/TableGen` front end and `utils/TableGen/Basic` intrinsic emitter. It is
//! a crate rather than a file included here, so that its own tests run under
//! `cargo test` and its modules can mirror the upstream files they port.

fn main() {
    llvmkit_tablegen::table_gen_main();
}
