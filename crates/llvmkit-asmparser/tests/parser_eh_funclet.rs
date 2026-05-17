//! Parser integration tests for S3.3 EH/funclet opcodes.
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.
//!
//! Note: the parser does not require a `personality` attribute on `define`
//! to accept `landingpad`/`resume`; that constraint is left to the verifier.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_snippet(src: &str) -> (Module<'_>, String) {
    let module = Module::new("test");
    let _ = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    let text = format!("{module}");
    (module, text)
}

// ── landingpad / resume ───────────────────────────────────────────────────────

/// `landingpad { ptr, i32 } catch ptr null` — non-terminator EH instruction.
/// Mirrors `test/Assembler/landingpad.ll`.
#[test]
fn landingpad_round_trips() {
    let (_, text) = parse_snippet(
        r#"define void @f() {
entry:
  br label %lpad
lpad:
  %e = landingpad { ptr, i32 } catch ptr null
  resume { ptr, i32 } %e
}
"#,
    );
    assert!(text.contains("landingpad"), "got: {text}");
}

/// `resume { ptr, i32 } %e` — re-raise an exception.
/// Mirrors `test/Assembler/resume.ll`.
#[test]
fn resume_round_trips() {
    let (_, text) = parse_snippet(
        r#"define void @f() {
entry:
  br label %lpad
lpad:
  %e = landingpad { ptr, i32 } cleanup
  resume { ptr, i32 } %e
}
"#,
    );
    assert!(text.contains("resume"), "got: {text}");
}

// ── invoke ────────────────────────────────────────────────────────────────────

/// `invoke void @may_throw() to label %ok unwind label %lpad` — EH call.
/// Mirrors `test/Assembler/invoke.ll`.
#[test]
fn invoke_round_trips() {
    let (_, text) = parse_snippet(
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
    assert!(text.contains("invoke"), "got: {text}");
}

// ── cleanuppad / cleanupret ───────────────────────────────────────────────────

/// `cleanuppad within none []` + `cleanupret from token %cp unwind to caller`.
/// Mirrors `test/Assembler/cleanuppad.ll` and `test/Assembler/cleanupret.ll`.
#[test]
fn cleanuppad_cleanupret_round_trips() {
    let (_, text) = parse_snippet(
        r#"define void @f() {
entry:
  br label %pad_bb
pad_bb:
  %cp = cleanuppad within none []
  cleanupret from token %cp unwind to caller
}
"#,
    );
    assert!(text.contains("cleanuppad"), "got: {text}");
    assert!(text.contains("cleanupret"), "got: {text}");
}
