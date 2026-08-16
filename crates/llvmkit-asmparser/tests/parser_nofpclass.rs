//! The `nofpclass` parameter and return attribute.
//!
//! Sources: `llvm/lib/AsmParser/LLParser.cpp::parseNoFPClassAttr` and
//! `keywordToFPClassTest` for the grammar, and
//! `llvm/lib/Support/FloatingPointMode.cpp::operator<<(raw_ostream &,
//! FPClassTest)` with its `NoFPClassName` table for the printing.
//!
//! **The print order is not the parse order**, which is the whole reason these
//! are round-trip tests rather than parse tests. `keywordToFPClassTest` lists
//! `norm` before `sub` before `zero`; `NoFPClassName` prints `zero`, then
//! `sub`, then `norm`, consuming bits greedily so an aliasing name never
//! reprints a bit a wider name already covered. Printing in parse order would
//! produce text that re-parses to the same mask but does not match what
//! upstream emits.

use llvmkit_asmparser::parser;

/// Parse, verify, print, and require the text back byte-identically.
fn round_trip(source: &str) {
    let module = match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("parses: {error:?}\n--- source ---\n{source}"),
    };
    let verified = module
        .verify()
        .unwrap_or_else(|e| panic!("verifies: {e:?}\n--- source ---\n{source}"));
    let printed = format!("{verified}");
    assert!(
        printed.contains(source.trim()),
        "round trip changed the text\n--- printed ---\n{printed}\n--- source ---\n{source}"
    );
}

fn rejects(source: &str, what: &str) {
    assert!(
        parser::parse_dynamic(source).is_err(),
        "expected {what} to be rejected, but it parsed\n--- source ---\n{source}"
    );
}

/// Every component keyword `keywordToFPClassTest` accepts, one per parameter,
/// each printing back as itself.
///
/// `ninf` and `sub` are the two that share a token with something else — a
/// fast-math flag and the instruction — so they are the ones most likely to
/// break, and they are here for that reason.
#[test]
fn every_nofpclass_component_round_trips() {
    for component in [
        "all", "nan", "snan", "qnan", "inf", "ninf", "pinf", "norm", "nnorm", "pnorm", "sub",
        "nsub", "psub", "zero", "nzero", "pzero",
    ] {
        round_trip(&format!("declare void @f(float nofpclass({component}) %x)"));
    }
}

/// A multi-class mask prints in `NoFPClassName` order, not in the order the
/// source listed the classes.
///
/// The mask is the same either way; only the text differs, which is what makes
/// this the case a parse-only test would miss.
#[test]
fn a_multi_class_mask_prints_in_upstream_order() {
    let module =
        parser::parse_dynamic("declare void @f(float nofpclass(zero nan) %x)").expect("parses");
    let printed = format!("{}", module.verify().expect("verifies"));
    assert!(
        printed.contains("nofpclass(nan zero)"),
        "expected the mask to print in NoFPClassName order\n{printed}"
    );
}

/// Upstream's `nofpclass(nan inf)` spelling from `ComputeKnownFPClassTest`,
/// on a definition rather than a declaration, in both the parameter and the
/// return position.
///
/// The signature line is compared rather than the whole function, because the
/// printer materialises the entry block's implicit `0:` label that the source
/// omits — the same reason `parser_vector_binops.rs` compares body lines.
#[test]
fn nofpclass_round_trips_on_parameters_and_returns() {
    let signature = "define nofpclass(nan) float @test(float nofpclass(nan inf) %nnan.ninf, float nofpclass(nan) %nnan, float nofpclass(qnan) %no.qnan, float %unknown) {";
    let source = format!("{signature}\n  ret float %nnan\n}}\n");
    let module = parser::parse_dynamic(&source).expect("parses");
    let printed = format!("{}", module.verify().expect("verifies"));
    assert!(
        printed.contains(signature),
        "signature did not round trip\n--- printed ---\n{printed}"
    );
}

/// The integer spelling, which `parseNoFPClassAttr` accepts only as the first
/// token, and its two rejections: zero, and a value with bits outside
/// `fcAllFlags`.
///
/// It prints back as class names, because that is the only form
/// `operator<<(raw_ostream &, FPClassTest)` emits.
#[test]
fn the_integer_mask_spelling_parses_and_prints_as_names() {
    // 3 == fcSNan | fcQNan == fcNan.
    let module = parser::parse_dynamic("declare void @f(float nofpclass(3) %x)").expect("parses");
    let printed = format!("{}", module.verify().expect("verifies"));
    assert!(
        printed.contains("nofpclass(nan)"),
        "expected the integer mask to print as class names\n{printed}"
    );

    rejects(
        "declare void @f(float nofpclass(0) %x)",
        "a zero nofpclass mask",
    );
    rejects(
        "declare void @f(float nofpclass(4294967295) %x)",
        "a nofpclass mask with bits outside fcAllFlags",
    );
}

/// An empty or malformed list is rejected rather than silently accepted.
#[test]
fn a_malformed_nofpclass_is_rejected() {
    rejects("declare void @f(float nofpclass() %x)", "an empty mask");
    rejects(
        "declare void @f(float nofpclass(bogus) %x)",
        "an unknown class name",
    );
    rejects(
        "declare void @f(float nofpclass nan %x)",
        "a mask with no parentheses",
    );
}
