//! Parser integration tests for S3.3 EH/funclet opcodes.
//!
//! These are llvmkit-specific parser acceptance subsets for EH/funclet opcodes.
//! The previously cited `test/Assembler/*.ll` fixture names are not present in
//! LLVM 22.1.4; rows in `UPSTREAM.md` cite the parser branches instead.
//!
//! Note: the parser does not require a `personality` attribute on `define`
//! to accept `landingpad`/`resume`; that constraint is left to the verifier.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_snippet(src: &str) -> String {
    Module::with_new("test", |module| {
        let _ = Parser::new(src.as_bytes(), &module)
            .expect("parse constructor")
            .parse_module()
            .expect("parse succeeded");
        format!("{module}")
    })
}

// ── landingpad / resume ───────────────────────────────────────────────────────

/// llvmkit-specific subset: `landingpad { ptr, i32 } catch ptr null`
/// accepted via `LLParser::parseLandingPad`.
#[test]
fn landingpad_round_trips() {
    let text = parse_snippet(
        r#"define void @f() {
entry:
  br label %lpad
lpad:
  %e = landingpad { ptr, i32 } catch ptr null
  resume { ptr, i32 } %e
}
"#,
    );
    assert!(
        text.contains("%e = landingpad { ptr, i32 }\n          catch ptr null\n"),
        "got: {text}"
    );
}

/// llvmkit-specific subset: `resume { ptr, i32 } %e` accepted via
/// `LLParser::parseResume`.
#[test]
fn resume_round_trips() {
    let text = parse_snippet(
        r#"define void @f() {
entry:
  br label %lpad
lpad:
  %e = landingpad { ptr, i32 } cleanup
  resume { ptr, i32 } %e
}
"#,
    );
    assert!(text.contains("resume { ptr, i32 } %e\n"), "got: {text}");
}

// ── invoke ────────────────────────────────────────────────────────────────────

/// llvmkit-specific subset: `invoke void @may_throw() to label %ok unwind
/// label %lpad` accepted via `LLParser::parseInvoke`.
#[test]
fn invoke_round_trips() {
    let text = parse_snippet(
        r#"declare void @may_throw()
define void @f() {
entry:
  invoke void @may_throw() to label %normal unwind label %lpad
normal:
  ret void
lpad:
  %e = landingpad { ptr, i32 } catch ptr null
  resume { ptr, i32 } %e
}
"#,
    );
    assert!(
        text.contains("invoke void @may_throw()\n          to label %normal unwind label %lpad\n"),
        "got: {text}"
    );
}

// ── cleanuppad / cleanupret ───────────────────────────────────────────────────

/// llvmkit-specific subset: `cleanuppad within none []` plus
/// `cleanupret from token %cp unwind to caller` parser acceptance.
#[test]
fn cleanuppad_cleanupret_round_trips() {
    let text = parse_snippet(
        r#"define void @f() {
entry:
  br label %pad_bb
pad_bb:
  %cp = cleanuppad within none []
  cleanupret from token %cp unwind to caller
}
"#,
    );
    assert!(
        text.contains("%cp = cleanuppad within none []\n"),
        "got: {text}"
    );
    assert!(
        text.contains("cleanupret from %cp unwind to caller\n"),
        "got: {text}"
    );
}
