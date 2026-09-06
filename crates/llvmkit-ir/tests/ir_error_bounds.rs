//! `IrError`'s derives are load-bearing and its rustdoc says why. Both halves
//! are pinned here: the bound the compiler enforces, and the sentence that
//! explains it.
//!
//! **llvmkit-specific, no upstream counterpart.** LLVM has no error *value* to
//! port: `Verifier::CheckFailed`, `LLParser::error` and
//! `ConstantExpr::getGetElementPtr` report through `bool`, `const char *`,
//! `SMDiagnostic` and `assert`, none of which is hashable or comparable. The
//! `Hash + Eq` requirement is llvmkit's own, driven by pass drivers that
//! collect findings into a `HashSet`.

const ERROR_RS: &str = include_str!("../src/error.rs");

/// The bound the rustdoc promises is the bound the compiler checks, so a
/// payload that broke it fails here rather than at a downstream user.
#[test]
fn ir_error_keeps_the_bounds_its_rustdoc_promises() {
    fn requires<T: core::hash::Hash + Eq + Clone + Send + Sync + 'static>() {}
    requires::<llvmkit_ir::IrError>();
}

/// The rustdoc names the bound, not a payload list that has been wrong since
/// `TypeKindLabel`, `VerifierRule` and `BrandError` became payloads.
#[test]
fn the_derive_rationale_names_a_bound_and_not_a_payload_list() {
    assert!(
        !ERROR_RS.contains("or integer, so the derive is total"),
        "error.rs still claims every payload is a String, &'static str or \
         integer; TypeMismatch carries a TypeKindLabel, VerifierFailure a \
         VerifierRule and a VerifierSubject, Brand a BrandError"
    );
    assert!(
        ERROR_RS.contains("Every payload is `Hash + Eq + Clone`"),
        "error.rs no longer states the bound a new payload has to satisfy"
    );
}
