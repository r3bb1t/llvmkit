//! The `llvmkit-tblgen` command-line entry point.
//!
//! Mirrors the `llvm-tblgen` binary: the same generator `llvmkit-ir`'s build
//! script drives, exposed for manual regeneration and for `--check`, which
//! reports a stale generated file without rewriting it.
//!
//! ```text
//! llvmkit-tblgen [--check] <generated-file>
//! llvmkit-tblgen [--check] <llvm-root> <generated-file>
//! ```

fn main() {
    llvmkit_tablegen::table_gen_main();
}
