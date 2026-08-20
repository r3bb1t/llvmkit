//! Parser integration tests for S3.3 EH/funclet opcodes.
//!
//! Mixed provenance, stated per test. The `landingpad` / `resume` / `invoke`
//! cases and `cleanuppad_cleanupret_round_trips` are hand-narrowed subsets: the
//! `test/Assembler/*.ll` fixture names they were first cited against are not
//! present in LLVM 22.1.4, and `UPSTREAM.md` now points them at the
//! `test/Bitcode/compatibility.ll` functions they were shaped after. Four of
//! the five `catchswitch` cases reproduce a whole upstream fixture or function
//! verbatim; the fifth, `catchswitch_print_reparse_is_stable`, has no upstream
//! counterpart and says so in its own doc comment.
//!
//! Note: the parser does not require a `personality` attribute on `define`
//! to accept `landingpad`/`resume`; that constraint is left to the verifier.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_snippet(src: &str) -> String {
    let module = Module::dynamic("test");
    let _ = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    format!("{module}")
}

/// Parse, print, re-parse and re-print. `test/Bitcode/compatibility.ll` runs
/// `llvm-as | llvm-dis | llvm-as | llvm-dis | FileCheck`, so its `CHECK` lines
/// are matched against the *second* `llvm-dis`: every construct in it must
/// survive a full round trip, including the ones with no `CHECK` of their own.
/// This is the parser/printer half of that pipeline.
fn parse_print_reparse(src: &str) -> (String, String) {
    let first = parse_snippet(src);
    let second = parse_snippet(&first);
    (first, second)
}

/// One line of a ported fixture's `CHECK` block.
enum Check<'a> {
    /// `; CHECK: <needle>` — matches as a substring of some line at or after
    /// the scan cursor.
    Line(&'a str),
    /// `; CHECK-NEXT: <needle>` — must match the line immediately following
    /// the previous match.
    Next(&'a str),
}

/// Run a fixture's own `CHECK` / `CHECK-NEXT` lines against `text` the way
/// `FileCheck` does: needles match as substrings of a line, every check must
/// match, and they must match **in order**, each one at or after the previous
/// match. The ordering rule is load-bearing for `@instructions.win_eh.2`,
/// whose `CHECK: cleanuppad within none []` appears twice and must match two
/// different lines.
fn file_check(text: &str, checks: &[Check<'_>]) {
    let lines: Vec<&str> = text.lines().collect();
    let mut cursor = 0usize;
    for check in checks {
        match check {
            Check::Line(needle) => {
                let hit = (cursor..lines.len())
                    .find(|&i| lines[i].contains(needle))
                    .unwrap_or_else(|| {
                        panic!("CHECK: {needle:?} not found at or after line {cursor} in:\n{text}")
                    });
                cursor = hit + 1;
            }
            Check::Next(needle) => {
                assert!(
                    cursor < lines.len() && lines[cursor].contains(needle),
                    "CHECK-NEXT: {needle:?} did not match line {cursor} in:\n{text}"
                );
                cursor += 1;
            }
        }
    }
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

/// `cleanuppad within none []` plus `cleanupret from %cp unwind to caller`,
/// the spelling `LLParser::parseCleanupRet` accepts: its pad operand is read
/// with `parseValue(Type::getTokenTy(Context), …)`, so there is no `token`
/// keyword in the syntax. Shaped after `test/Bitcode/compatibility.ll`
/// `@instructions.win_eh.2`, narrowed to the two instructions under test.
#[test]
fn cleanuppad_cleanupret_round_trips() {
    let text = parse_snippet(
        r#"define void @f() {
entry:
  br label %pad_bb
pad_bb:
  %cp = cleanuppad within none []
  cleanupret from %cp unwind to caller
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

// ── catchswitch ───────────────────────────────────────────────────────────────

/// `test/Bitcode/compatibility.ll` `@instructions.win_eh.1`, verbatim, with the
/// `declare ccc void @f.ccc()` it calls. Covers a named `catchswitch` result
/// (`%cs1`/`%cs2`/`%cs3`), both unwind forms (`unwind to caller` and
/// `unwind label %cleanuppad1`), three distinct handlers, and catchpads with
/// zero, one and two arguments.
///
/// The fixture's `RUN` line is `llvm-as | llvm-dis | llvm-as | llvm-dis |
/// FileCheck`, so its `CHECK` lines are matched against a second `llvm-dis`.
/// All **eight** of the function's `CHECK`/`CHECK-NEXT` lines are asserted
/// below, in order, through [`file_check`]. Upstream writes no `CHECK` for the
/// three `catchswitch` lines themselves — the `RUN` line's second `llvm-as` is
/// what pins those, so they are pinned here by the round-trip assertion plus
/// three explicit checks.
#[test]
fn catchswitch_handlers_and_unwind_forms() {
    let (first, second) = parse_print_reparse(
        r#"declare ccc void @f.ccc()

define i32 @instructions.win_eh.1() personality i32 -3 {
entry:
  %arg1 = alloca i32
  %arg2 = alloca i32
  invoke void @f.ccc() to label %normal unwind label %catchswitch1
  invoke void @f.ccc() to label %normal unwind label %catchswitch2
  invoke void @f.ccc() to label %normal unwind label %catchswitch3

catchswitch1:
  %cs1 = catchswitch within none [label %catchpad1] unwind to caller

catchpad1:
  catchpad within %cs1 []
  br label %normal
  ; CHECK: catchpad within %cs1 []
  ; CHECK-NEXT: br label %normal

catchswitch2:
  %cs2 = catchswitch within none [label %catchpad2] unwind to caller

catchpad2:
  catchpad within %cs2 [ptr %arg1]
  br label %normal
  ; CHECK: catchpad within %cs2 [ptr %arg1]
  ; CHECK-NEXT: br label %normal

catchswitch3:
  %cs3 = catchswitch within none [label %catchpad3] unwind label %cleanuppad1

catchpad3:
  catchpad within %cs3 [ptr %arg1, ptr %arg2]
  br label %normal
  ; CHECK: catchpad within %cs3 [ptr %arg1, ptr %arg2]
  ; CHECK-NEXT: br label %normal

cleanuppad1:
  %clean.1 = cleanuppad within none []
  unreachable
  ; CHECK: %clean.1 = cleanuppad within none []
  ; CHECK-NEXT: unreachable

normal:
  ret i32 0
}
"#,
    );
    // The function's own eight CHECK lines, verbatim and in order.
    file_check(
        &first,
        &[
            Check::Line("catchpad within %cs1 []"),
            Check::Next("br label %normal"),
            Check::Line("catchpad within %cs2 [ptr %arg1]"),
            Check::Next("br label %normal"),
            Check::Line("catchpad within %cs3 [ptr %arg1, ptr %arg2]"),
            Check::Next("br label %normal"),
            Check::Line("%clean.1 = cleanuppad within none []"),
            Check::Next("unreachable"),
        ],
    );
    // No upstream CHECK covers the catchswitch lines themselves; the RUN
    // line's second `llvm-as` does, by requiring them to re-parse.
    file_check(
        &first,
        &[
            Check::Line("%cs1 = catchswitch within none [label %catchpad1] unwind to caller"),
            Check::Line("%cs2 = catchswitch within none [label %catchpad2] unwind to caller"),
            Check::Line(
                "%cs3 = catchswitch within none [label %catchpad3] unwind label %cleanuppad1",
            ),
        ],
    );
    assert_eq!(first, second, "print/re-parse is not idempotent");
}

/// `test/Bitcode/compatibility.ll` `@instructions.win_eh.2`, verbatim, with the
/// `declare ccc void @f.ccc()` it calls. Covers a named `catchswitch` with
/// `unwind label` to a `cleanuppad`, a *named* `catchpad` result,
/// `catchret from %catch to label %return`,
/// `cleanupret from %clean unwind to caller`, a nested `cleanuppad within
/// %catch []`, and a `"funclet"` operand bundle on an `invoke`.
///
/// Same `llvm-as | llvm-dis | llvm-as | llvm-dis` `RUN` line as
/// `@instructions.win_eh.1`. All **nine** of the function's `CHECK`/
/// `CHECK-NEXT` lines are asserted below, in order — the ordering matters
/// here, because `CHECK: cleanuppad within none []` appears twice and its
/// second occurrence must match `terminate:`'s instruction, not `cleanup:`'s.
/// The `catchswitch` line has no `CHECK` of its own upstream and is pinned by
/// the round trip plus one explicit check, as in `@instructions.win_eh.1`.
#[test]
fn catchswitch_nested_funclets_and_catchret() {
    let (first, second) = parse_print_reparse(
        r#"declare ccc void @f.ccc()

define i32 @instructions.win_eh.2() personality i32 -4 {
entry:
  invoke void @f.ccc() to label %invoke.cont unwind label %catchswitch

invoke.cont:
  invoke void @f.ccc() to label %continue unwind label %cleanup

cleanup:
  %clean = cleanuppad within none []
  ; CHECK: %clean = cleanuppad within none []
  cleanupret from %clean unwind to caller
  ; CHECK: cleanupret from %clean unwind to caller

catchswitch:
  %cs = catchswitch within none [label %catchpad] unwind label %terminate

catchpad:
  %catch = catchpad within %cs []
  br label %body
  ; CHECK: %catch = catchpad within %cs []
  ; CHECK-NEXT: br label %body

body:
  invoke void @f.ccc() [ "funclet"(token %catch) ]
    to label %continue unwind label %terminate.inner
  catchret from %catch to label %return
  ; CHECK: catchret from %catch to label %return

return:
  ret i32 0

terminate.inner:
  cleanuppad within %catch []
  unreachable
  ; CHECK: cleanuppad within %catch []
  ; CHECK-NEXT: unreachable

terminate:
  cleanuppad within none []
  unreachable
  ; CHECK: cleanuppad within none []
  ; CHECK-NEXT: unreachable

continue:
  ret i32 0
}
"#,
    );
    // The function's own nine CHECK lines, verbatim and in order.
    file_check(
        &first,
        &[
            Check::Line("%clean = cleanuppad within none []"),
            Check::Line("cleanupret from %clean unwind to caller"),
            Check::Line("%catch = catchpad within %cs []"),
            Check::Next("br label %body"),
            Check::Line("catchret from %catch to label %return"),
            Check::Line("cleanuppad within %catch []"),
            Check::Next("unreachable"),
            Check::Line("cleanuppad within none []"),
            Check::Next("unreachable"),
        ],
    );
    file_check(
        &first,
        &[Check::Line(
            "%cs = catchswitch within none [label %catchpad] unwind label %terminate",
        )],
    );
    assert_eq!(first, second, "print/re-parse is not idempotent");
}

/// `test/Verifier/operand-bundles-wineh.ll`, verbatim (the whole file) — the
/// *numbered* result spelling, `%0 = catchswitch within none [label %catch]
/// unwind to caller`, with `%1 = catchpad within %0 [ptr null, i32 0, ptr
/// null]` referring to it.
///
/// Upstream's `RUN` line is `not opt -passes=verify`, and the file's single
/// `CHECK` (`Missing funclet token on intrinsic call`) is a Verifier
/// diagnostic: llvmkit's `verifier.rs` has no funclet-token rule, so only the
/// parse half is ported and that one `CHECK` is **not** asserted. The input is
/// reproduced whole rather than trimmed; what is left out is the Verifier
/// check, not any part of the fixture's IR. The three assertions below are
/// llvmkit's own, pinning that the numbered results survive printing.
#[test]
fn catchswitch_numbered_result() {
    let text = parse_snippet(
        r#"define void @report_missing() personality ptr @__CxxFrameHandler3 {
entry:
  invoke void @may_throw() to label %eh.cont unwind label %catch.dispatch

catch.dispatch:
  %0 = catchswitch within none [label %catch] unwind to caller

catch:
  %1 = catchpad within %0 [ptr null, i32 0, ptr null]
  br label %catch.cont

catch.cont:
; CHECK: Missing funclet token on intrinsic call
  %2 = call ptr @llvm.objc.retain(ptr null)
  catchret from %1 to label %eh.cont

eh.cont:
  ret void
}

declare void @may_throw()
declare i32 @__CxxFrameHandler3(...)

declare ptr @llvm.objc.retain(ptr) #0

attributes #0 = { nounwind }
"#,
    );
    assert!(
        text.contains("%0 = catchswitch within none [label %catch] unwind to caller"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%1 = catchpad within %0 [ptr null, i32 0, ptr null]"),
        "got:\n{text}"
    );
    assert!(
        text.contains("catchret from %1 to label %eh.cont"),
        "got:\n{text}"
    );
}

/// `test/Verifier/preallocated-valid.ll`, verbatim (the whole file).
/// `@preallocated_teardown_invoke` is the reason it is here:
/// `%s = catchswitch within none [label %catch] unwind to caller` with
/// `%p = catchpad within %s []`.
///
/// Upstream's `RUN` line is `opt -S %s -passes=verify` with no `FileCheck`, so
/// the fixture carries no `CHECK` lines at all: it asserts only that the module
/// verifies. llvmkit's `verifier.rs` has no `preallocated` or funclet rule, so
/// only the parse half is ported and the two assertions below are llvmkit's
/// own. The file is reproduced whole rather than reduced to the one function.
///
/// Deliberately **not** a round trip. `@preallocated_indirect`'s
/// `call void %f(ptr preallocated(i32) %x) ["preallocated"(token %cs)]` prints
/// back as `call void %f(ptr %x)`: llvmkit drops parameter attributes and
/// operand bundles on an *indirect* call, where the direct-callee form in
/// `@preallocated` round-trips correctly. That defect is recorded separately
/// and is not fixed here; when it is, this test can gain the round trip.
#[test]
fn catchswitch_in_preallocated_teardown() {
    let text = parse_snippet(
        r#"declare token @llvm.call.preallocated.setup(i32)
declare ptr @llvm.call.preallocated.arg(token, i32)
declare void @llvm.call.preallocated.teardown(token)

declare i32 @__CxxFrameHandler3(...)

declare void @foo1(ptr preallocated(i32))
declare i64 @foo1_i64(ptr preallocated(i32))
declare void @foo2(ptr preallocated(i32), ptr, ptr preallocated(i32))

declare void @constructor(ptr)

define void @preallocated() {
    %cs = call token @llvm.call.preallocated.setup(i32 1)
    %x = call ptr @llvm.call.preallocated.arg(token %cs, i32 0) preallocated(i32)
    call void @foo1(ptr preallocated(i32) %x) ["preallocated"(token %cs)]
    ret void
}

define void @preallocated_indirect(ptr %f) {
    %cs = call token @llvm.call.preallocated.setup(i32 1)
    %x = call ptr @llvm.call.preallocated.arg(token %cs, i32 0) preallocated(i32)
    call void %f(ptr preallocated(i32) %x) ["preallocated"(token %cs)]
    ret void
}

define void @preallocated_setup_without_call() {
    %cs = call token @llvm.call.preallocated.setup(i32 1)
    %a0 = call ptr @llvm.call.preallocated.arg(token %cs, i32 0) preallocated(i32)
    ret void
}

define void @preallocated_num_args() {
    %cs = call token @llvm.call.preallocated.setup(i32 2)
    %x = call ptr @llvm.call.preallocated.arg(token %cs, i32 0) preallocated(i32)
    %y = call ptr @llvm.call.preallocated.arg(token %cs, i32 1) preallocated(i32)
    %a = inttoptr i32 0 to ptr
    call void @foo2(ptr preallocated(i32) %x, ptr %a, ptr preallocated(i32) %y) ["preallocated"(token %cs)]
    ret void
}

define void @preallocated_musttail(ptr preallocated(i32) %a) {
    musttail call void @foo1(ptr preallocated(i32) %a)
    ret void
}

define i64 @preallocated_musttail_i64(ptr preallocated(i32) %a) {
    %r = musttail call i64 @foo1_i64(ptr preallocated(i32) %a)
    ret i64 %r
}

define void @preallocated_teardown() {
    %cs = call token @llvm.call.preallocated.setup(i32 1)
    call void @llvm.call.preallocated.teardown(token %cs)
    ret void
}

define void @preallocated_teardown_invoke() personality ptr @__CxxFrameHandler3 {
    %cs = call token @llvm.call.preallocated.setup(i32 1)
    %x = call ptr @llvm.call.preallocated.arg(token %cs, i32 0) preallocated(i32)
    invoke void @constructor(ptr %x) to label %conta unwind label %contb
conta:
    call void @foo1(ptr preallocated(i32) %x) ["preallocated"(token %cs)]
    ret void
contb:
    %s = catchswitch within none [label %catch] unwind to caller
catch:
    %p = catchpad within %s []
    call void @llvm.call.preallocated.teardown(token %cs)
    ret void
}
"#,
    );
    assert!(
        text.contains("%s = catchswitch within none [label %catch] unwind to caller"),
        "got:\n{text}"
    );
    assert!(text.contains("%p = catchpad within %s []"), "got:\n{text}");
}

/// **No upstream counterpart.** This pins the parser/printer contract that the
/// missing named-result dispatch broke: `crates/llvmkit-ir/src/asm_writer.rs`
/// marks `catchswitch` as printing a result name, so every `catchswitch`
/// llvmkit emits carries a `%name =` — text llvmkit's own parser rejected until
/// the post-`parse_lhs_assignment` dispatch arm existed. The IR is written here
/// the way `crates/llvmkit-ir/tests/builder_funclet.rs` builds it, printed, and
/// fed straight back to the parser.
#[test]
fn catchswitch_print_reparse_is_stable() {
    let (first, second) = parse_print_reparse(
        r#"define void @f() {
entry:
  br label %catchswitch1
catchswitch1:
  %cs1 = catchswitch within none [label %catchpad1] unwind to caller
catchpad1:
  %cp1 = catchpad within %cs1 []
  catchret from %cp1 to label %entry
}
"#,
    );
    assert!(
        first.contains("%cs1 = catchswitch within none [label %catchpad1] unwind to caller"),
        "got:\n{first}"
    );
    assert_eq!(first, second, "print/re-parse is not idempotent");
}
