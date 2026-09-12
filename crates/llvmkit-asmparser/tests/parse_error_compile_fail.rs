//! Compile-fail locks on the parser's error surface.
//!
//! Each fixture in `tests/compile_fail/` pins a property that would otherwise
//! be guarded only by a grep — and a grep goes stale the moment someone adds
//! back what it was watching for.

/// llvmkit-specific (**no upstream counterpart**): LLVM's `SMDiagnostic`
/// deliberately spans I/O and parse failures, because `parseAssemblyFile`
/// opens the file itself. llvmkit keeps the read in the caller, so
/// `ParseError` carries no I/O variant and no `From<std::io::Error>`, and `?`
/// on an `io::Result` inside a `ParseResult` function has no conversion to
/// reach for.
///
/// `cargo check` is enough here: the fixture's primary error is a missing
/// `From` impl (`E0277`), which type-checking reaches. No `t.pass(...)` case
/// is registered, unlike `llvmkit-ir`'s harness, which needs one to force
/// `cargo build` for a monomorphisation-time `const { assert!(...) }`.
#[test]
fn parse_error_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/parse_error_is_not_an_io_error.rs");
}
