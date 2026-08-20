//! Parser integration tests for S3.3 EH/funclet opcodes.
//!
//! Mixed provenance, stated per test. The `resume` / `invoke` cases and
//! `cleanuppad_cleanupret_round_trips` are hand-narrowed subsets: the
//! `test/Assembler/*.ll` fixture names they were first cited against are not
//! present in LLVM 22.1.4, and `UPSTREAM.md` now points them at the
//! `test/Bitcode/compatibility.ll` functions they were shaped after.
//! `landingpad_round_trips` and four of the five `catchswitch` cases reproduce
//! a whole upstream fixture or function verbatim, loaded through `include_str!`
//! from a checked-in copy under `tests/fixtures/upstream/` as `UPSTREAM.md`'s
//! audit rule requires of a `mirror` row; the fifth,
//! `catchswitch_print_reparse_is_stable`, has no upstream counterpart and says
//! so in its own doc comment.
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

/// `opt -passes=verify` — the whole contract of an upstream `-valid` fixture.
/// Parses, verifies, and returns the printed module.
fn parse_verify_and_print(src: &str) -> String {
    let module = Module::dynamic("test");
    let _ = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    module
        .verify_borrowed()
        .expect("test/Verifier/preallocated-valid.ll: `opt -passes=verify` accepts this module");
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

/// One directive of a ported fixture's `CHECK` block.
enum Check<'a> {
    /// `; CHECK: <needle>` — `Pattern::match` searches the *remaining buffer*
    /// from the byte cursor, and `FileCheckString::Check` resumes at
    /// `MatchPos + MatchLen`, a byte position still inside the matched line.
    /// Two `CHECK:` directives may therefore match one output line.
    Line(&'a str),
    /// `; CHECK-NEXT: <needle>` — matches like `Check::Line`, then
    /// `FileCheckString::CheckNext` requires exactly one newline in the
    /// skipped region.
    Next(&'a str),
}

/// Mirrors `FileCheck::CanonicalizeFile`, both halves of its loop body: drop
/// the `\r` of a `\r\n` pair (`if (Ptr <= End - 2 && Ptr[0] == '\r' && Ptr[1]
/// == '\n') continue;`, so a lone trailing `\r` survives), then collapse each
/// run of ' ' / '\t' to a single ' '. Upstream applies it to the check file
/// *and* the input file whenever `--strict-whitespace` is absent, which is the
/// case for every fixture ported here.
fn canonicalize_horizontal_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        // Eliminate trailing dosish `\r`.
        if c == '\r' && chars.peek() == Some(&'\n') {
            continue;
        }
        if c != ' ' && c != '\t' {
            out.push(c);
            continue;
        }
        // Otherwise, add one space and advance over neighboring space.
        out.push(' ');
        while let Some(' ' | '\t') = chars.peek() {
            chars.next();
        }
    }
    out
}

/// Mirrors `CountNumNewlinesBetween`: a `\r\n` or `\n\r` pair is one newline.
fn count_newlines_between(region: &str) -> usize {
    let bytes = region.as_bytes();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' || bytes[i] == b'\r' {
            count += 1;
            if i + 1 < bytes.len()
                && (bytes[i + 1] == b'\n' || bytes[i + 1] == b'\r')
                && bytes[i + 1] != bytes[i]
            {
                i += 1;
            }
        }
        i += 1;
    }
    count
}

/// Run a fixture's own `CHECK` / `CHECK-NEXT` directives against `text`.
///
/// A faithful two-directive subset of FileCheck, not a paraphrase:
///
/// * `FileCheck::CanonicalizeFile` collapses horizontal-whitespace runs in both
///   buffers; `FileCheck::readCheckFile` ltrims a pattern and
///   `Pattern::parsePattern` rtrims it. Both are reproduced here.
/// * `Pattern::match` is `Buffer.find(FixedStr)` over the *remaining buffer*, and
///   `FileCheckString::Check` resumes at `MatchPos + MatchLen` — a byte position
///   still inside the matched line, so two `CHECK:` directives may match one line.
/// * `FileCheckString::CheckNext` counts newlines in the skipped region with
///   `CountNumNewlinesBetween` and errors "is on the same line as previous match"
///   at zero, "is not on the line after the previous match" otherwise.
/// * `FileCheck::readCheckFile` rejects a leading `-NEXT` directive.
///
/// The byte cursor is load-bearing for `@instructions.win_eh.2`, whose
/// `CHECK: cleanuppad within none []` appears twice and must match two
/// different lines.
///
/// **Only `CHECK` and `CHECK-NEXT` are implemented.** Everything else FileCheck
/// understands is *unimplemented here*, and a fixture needing any of it is
/// *unported*, not narrowed — do not trim the fixture to fit this.
///
/// Two of those gaps can be stated exactly, because each is an enum. Every
/// other user-spellable `Check::FileCheckKind` in
/// `include/llvm/FileCheck/FileCheck.h` is missing — `CHECK-SAME`, `CHECK-NOT`,
/// `CHECK-DAG`, `CHECK-LABEL`, `CHECK-EMPTY`, `CHECK-COUNT-<n>` and `COM:`;
/// that enum's remaining values (`CheckNone`, `CheckMisspelled`, `CheckEOF`,
/// `CheckBadNot`, `CheckBadCount`) are internal markers, not spellings. So is
/// the sole `FileCheckKindModifier`, `{LITERAL}`.
///
/// Past those two enums, assume nothing is honoured: no `FileCheckRequest`
/// option is, and no pattern syntax beyond a fixed substring is. The names
/// here are examples, not an inventory — `{{regex}}`, `[[var]]`,
/// `[[#numeric]]`, `--implicit-check-not`, `--strict-whitespace`,
/// `--ignore-case`, `-D VAR=VALUE`, `--check-prefix` / `--check-prefixes`,
/// `--comment-prefixes`, `--match-full-lines`.
///
/// Three of those are live in the vendored corpus rather than hypothetical:
/// `test/Assembler/block-labels.ll` carries `--match-full-lines` on its RUN
/// line, and `-check-prefix` and `-DFILE=%s` appear on fixtures under
/// `tests/fixtures/upstream/assembler-corpus/`. Those fixtures are driven by
/// `parser_corpus.rs`'s manifest rather than by this harness, which is why they
/// are not a defect today — and why lifting a needle out of one of them into a
/// `check_directives` call would silently drop the option it depends on.
fn check_directives(text: &str, checks: &[Check<'_>]) {
    // `FileCheck::readCheckFile`'s wording, reproduced including its unbalanced
    // quote: `"found '" + UsedPrefix + "-" + Type + "' without previous '" +
    // UsedPrefix + ": line"`.
    assert!(
        !matches!(checks.first(), Some(Check::Next(_))),
        "found 'CHECK-NEXT' without previous 'CHECK: line"
    );
    let haystack = canonicalize_horizontal_whitespace(text);
    let mut cursor = 0usize;
    for check in checks {
        let (raw, is_next) = match check {
            Check::Line(needle) => (needle, false),
            Check::Next(needle) => (needle, true),
        };
        let canonical = canonicalize_horizontal_whitespace(raw);
        let needle = canonical.trim_matches(|c| c == ' ' || c == '\t');
        let found = haystack[cursor..].find(needle).unwrap_or_else(|| {
            let kind = if is_next { "CHECK-NEXT" } else { "CHECK" };
            panic!("{kind}: {needle:?} not found after byte {cursor} in:\n{text}")
        });
        if is_next {
            let newlines = count_newlines_between(&haystack[cursor..cursor + found]);
            assert!(
                newlines != 0,
                "CHECK-NEXT: is on the same line as previous match ({needle:?}) in:\n{text}"
            );
            assert!(
                newlines == 1,
                "CHECK-NEXT: is not on the line after the previous match \
                 ({needle:?}) in:\n{text}"
            );
        }
        cursor += found + needle.len();
    }
}

// ── landingpad / resume ───────────────────────────────────────────────────────

/// `test/Bitcode/compatibility.ll` `@instructions.landingpad`, verbatim, with
/// the `declare void @llvm.donothing()` it invokes. Covers all four clause
/// shapes upstream writes: `cleanup` alone (`catch1`), `cleanup` + one `catch`
/// (`catch2`), `cleanup` + two `catch`es (`catch3`), and a `filter`
/// (`catch4`). Accepted via `LLParser::parseLandingPad`.
///
/// The fixture's `RUN` line is `llvm-as | llvm-dis | llvm-as | llvm-dis |
/// FileCheck`, so its `CHECK` lines are matched against a second `llvm-dis`.
/// All **eleven** of the function's `CHECK` lines are asserted below, in
/// order, through [`check_directives`]. The declaration's own `CHECK`
/// (`declare void @llvm.donothing() #35`) names a file-wide attribute-group
/// number and is not part of this excerpt; the four `invoke` lines and the
/// `br`/`ret` lines carry no `CHECK` upstream and are pinned by the
/// round-trip assertion instead.
#[test]
fn landingpad_round_trips() {
    const FIXTURE: &str =
        include_str!("fixtures/upstream/compatibility/instructions_landingpad.ll");

    let (first, second) = parse_print_reparse(FIXTURE);
    // The function's own eleven CHECK lines, verbatim and in order.
    check_directives(
        &first,
        &[
            // catch1
            Check::Line("landingpad i32"),
            Check::Line("cleanup"),
            // catch2
            Check::Line("landingpad i32"),
            Check::Line("cleanup"),
            Check::Line("catch ptr null"),
            // catch3
            Check::Line("landingpad i32"),
            Check::Line("cleanup"),
            Check::Line("catch ptr null"),
            Check::Line("catch ptr null"),
            // catch4
            Check::Line("landingpad i32"),
            Check::Line("filter [2 x i32] zeroinitializer"),
        ],
    );
    assert_eq!(first, second, "print/re-parse is not idempotent");
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
/// below, in order, through [`check_directives`]. Upstream writes no `CHECK`
/// for the three `catchswitch` lines themselves — the `RUN` line's second
/// `llvm-as` is what pins those, so they are pinned here by the round-trip
/// assertion plus three explicit checks.
#[test]
fn catchswitch_handlers_and_unwind_forms() {
    const FIXTURE: &str = include_str!("fixtures/upstream/compatibility/instructions_win_eh_1.ll");

    let (first, second) = parse_print_reparse(FIXTURE);
    // The function's own eight CHECK lines, verbatim and in order.
    check_directives(
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
    // line's second `llvm-as` does, by requiring them to re-parse. This is a
    // deliberate *second* assertion group, not a continuation of the one
    // above — the byte cursor restarts at 0, the way a second `FileCheck`
    // invocation over the same input would.
    check_directives(
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
    const FIXTURE: &str = include_str!("fixtures/upstream/compatibility/instructions_win_eh_2.ll");

    let (first, second) = parse_print_reparse(FIXTURE);
    // The function's own nine CHECK lines, verbatim and in order.
    check_directives(
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
    check_directives(
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
/// parse half is ported and that one `CHECK` is **not** asserted. llvmkit
/// verifies this module clean where upstream rejects it — divergence **112**
/// in `docs/divergences.md`. The input is the vendored fixture, whole rather
/// than trimmed; what is left out is the Verifier check, not any part of the
/// fixture's IR. The three assertions below are llvmkit's own, pinning that
/// the numbered results survive printing.
#[test]
fn catchswitch_numbered_result() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/operand-bundles-wineh.ll");

    let text = parse_snippet(FIXTURE);
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

/// **llvmkit-specific divergence lock — no upstream counterpart**, because it
/// pins the *absence* of a rule upstream has. `test/Verifier/operand-bundles-wineh.ll`
/// runs `not opt -passes=verify` and its one `CHECK` is
/// `Missing funclet token on intrinsic call`, so upstream **rejects** this
/// module; llvmkit's `Module::verify_borrowed` answers `Ok(())` because
/// `verifier.rs` has no funclet-token rule. That is divergence **112** in
/// `docs/divergences.md`, and this test is its live evidence rather than a
/// probe quoted in prose: when the rule is ported this assertion flips, which
/// is the signal to retire entry 112 and regrade `catchswitch_numbered_result`
/// from `mirror (partial)` to `mirror`.
#[test]
fn wineh_missing_funclet_token_is_not_diagnosed() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/operand-bundles-wineh.ll");

    let module = Module::dynamic("test");
    let _ = Parser::new(FIXTURE.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    assert!(
        module.verify_borrowed().is_ok(),
        "divergence 112 assumes llvmkit accepts this module; it no longer does: {:?}",
        module.verify_borrowed().err()
    );
}

/// `test/Verifier/preallocated-valid.ll`, verbatim (the whole file).
/// `@preallocated_teardown_invoke` is the reason it is here:
/// `%s = catchswitch within none [label %catch] unwind to caller` with
/// `%p = catchpad within %s []`.
///
/// Upstream's `RUN` line is `opt -S %s -passes=verify` with no `FileCheck`, so
/// the fixture carries no `CHECK` lines at all: **it asserts only that the
/// module verifies**, and that is the oracle run here — `verify_borrowed`
/// inside [`parse_verify_and_print`]. llvmkit has no `preallocated` and no
/// funclet rule, but a missing rule can only make llvmkit more permissive, so
/// it can never turn this fixture's contract into a false failure; what the
/// oracle does cover is `Verifier::check_call` and `check_invoke` over the
/// rest of the file. The two `contains` assertions below are llvmkit's own,
/// pinning the `catchswitch`/`catchpad` spelling on top. The file is the
/// vendored fixture, whole rather than reduced to the one function.
///
/// Deliberately **not** a round trip. `@preallocated_indirect`'s
/// `call void %f(ptr preallocated(i32) %x) ["preallocated"(token %cs)]` prints
/// back as `call void %f(ptr %x)`: llvmkit drops parameter attributes and
/// operand bundles on an *indirect* call, where the direct-callee form in
/// `@preallocated` round-trips correctly. That is divergence **108** in
/// `docs/divergences.md` and is not fixed here; when it is, this test can gain
/// the round trip.
#[test]
fn catchswitch_in_preallocated_teardown() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/preallocated-valid.ll");

    let text = parse_verify_and_print(FIXTURE);
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
