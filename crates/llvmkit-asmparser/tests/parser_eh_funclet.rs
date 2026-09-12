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

pub mod support;

use support::{Check, check_directives};

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
/// diagnostic, asserted by [`wineh_missing_funclet_token_is_diagnosed`]. This
/// is the parse half: the input is the vendored fixture, whole rather than
/// trimmed, and the three assertions below are llvmkit's own, pinning that the
/// numbered results survive printing.
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

/// `test/Verifier/operand-bundles-wineh.ll`'s verdict half: upstream's `RUN`
/// line is `not opt -passes=verify` and the file's one `CHECK` is
/// `Missing funclet token on intrinsic call`, the tail of
/// `Verifier::visitIntrinsicCall`.
///
/// Every ingredient of that rule is exercised by this one module:
/// `@llvm.objc.retain` is in `IntrinsicInst::mayLowerToFunctionCall`'s
/// `switch`, `ptr @__CxxFrameHandler3` classifies as `MSVC_CXX` which
/// `isScopedEHPersonality` accepts, and `colorEHFunclets` has to walk
/// entry → `catch.dispatch` (a `catchswitch`) → `catch` (a `catchpad`, so the
/// colour changes there) → `catch.cont` for the call's block to come out
/// coloured by a funclet at all.
///
/// `contains` is the comparison because a `CHECK` directive is a substring
/// match, which is FileCheck's own rule.
#[test]
fn wineh_missing_funclet_token_is_diagnosed() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/operand-bundles-wineh.ll");

    let module = Module::dynamic("test");
    let _ = Parser::new(FIXTURE.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    let message = match module.verify_borrowed() {
        Ok(()) => panic!("upstream's RUN line is `not opt -passes=verify`"),
        Err(llvmkit_ir::IrError::VerifierFailure { message, .. }) => message,
        Err(other) => panic!("expected a verifier failure, got {other:?}"),
    };
    assert!(
        message.contains("Missing funclet token on intrinsic call"),
        "{message:?}"
    );
}

/// **No upstream counterpart** — the negative half of
/// [`wineh_missing_funclet_token_is_diagnosed`], built by editing that
/// fixture's one offending call.
///
/// Two ways upstream's rule stays silent, neither of which the fixture itself
/// exercises: naming the funclet, and leaving the function's personality off.
/// Without them, a rule that fired unconditionally would pass the fixture.
///
/// The two halves assert different things, and the reason is upstream's own:
/// a `funclet` bundle makes the module **valid**, while removing the
/// `personality` makes it invalid for a *different* reason —
/// `Verifier::visitCatchSwitchInst`'s `CatchSwitchInst needs to be in a
/// function with a personality.`, which llvmkit reports since the EH chapter
/// was ported. So the second half asserts that the rejection is that one and
/// not `Missing funclet token on intrinsic call`, which is exactly the claim
/// the case exists to make. It used to assert `is_ok()`, which held only
/// because the personality `Check` was unported.
#[test]
fn wineh_funclet_token_and_missing_personality_both_silence_the_rule() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/operand-bundles-wineh.ll");

    // `Check(HasToken, …)` — a `"funclet"` bundle naming the enclosing pad.
    let with_token = FIXTURE.replace(
        "call ptr @llvm.objc.retain(ptr null)",
        "call ptr @llvm.objc.retain(ptr null) [ \"funclet\"(token %1) ]",
    );
    assert_ne!(with_token, FIXTURE, "fixture text moved");

    // `if (F->hasPersonalityFn() && isScopedEHPersonality(…))` — no
    // personality, so the funclet colouring never runs.
    let without_personality = FIXTURE.replace(" personality ptr @__CxxFrameHandler3", "");
    assert_ne!(without_personality, FIXTURE, "fixture text moved");

    for (name, source, expected) in [
        ("funclet token supplied", with_token, None),
        (
            "no personality function",
            without_personality,
            Some("CatchSwitchInst needs to be in a function with a personality."),
        ),
    ] {
        let module = Module::dynamic("test");
        Parser::new(source.as_bytes(), &module)
            .expect("parse constructor")
            .parse_module()
            .unwrap_or_else(|e| {
                panic!(
                    "{name} parses: {e}
{source}"
                )
            });
        match (module.verify_borrowed(), expected) {
            (Ok(()), None) => {}
            (Ok(()), Some(expected)) => {
                panic!("{name}: verified clean, want {expected:?}\n{source}")
            }
            (Err(e), None) => panic!("{name}: {e:?}\n{source}"),
            (Err(llvmkit_ir::IrError::VerifierFailure { message, .. }), Some(expected)) => {
                assert!(
                    message.contains(expected),
                    "{name}: {message:?} does not contain {expected:?}"
                );
                assert!(
                    !message.contains("Missing funclet token on intrinsic call"),
                    "{name}: the funclet-token rule must stay silent, got {message:?}"
                );
            }
            (Err(other), Some(_)) => panic!("{name}: expected a verifier failure, got {other:?}"),
        }
    }
}

/// `test/Verifier/preallocated-valid.ll`, verbatim (the whole file).
/// `@preallocated_teardown_invoke` is the reason it is here:
/// `%s = catchswitch within none [label %catch] unwind to caller` with
/// `%p = catchpad within %s []`.
///
/// Upstream's `RUN` line is `opt -S %s -passes=verify` with no `FileCheck`, so
/// the fixture carries no `CHECK` lines at all: **it asserts only that the
/// module verifies**, and that is the oracle run here — `verify_borrowed`
/// inside [`parse_verify_and_print`]. llvmkit has no `preallocated`
/// intrinsic rules, but a missing rule can only make llvmkit more permissive, so
/// it can never turn this fixture's contract into a false failure; what the
/// oracle does cover is `Verifier::check_call` and `check_invoke` over the
/// rest of the file. The three `contains` assertions below are llvmkit's own,
/// pinning the `catchswitch`/`catchpad` spelling on top. The file is the
/// vendored fixture, whole rather than reduced to the one function.
#[test]
fn catchswitch_in_preallocated_teardown() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/preallocated-valid.ll");

    let text = parse_verify_and_print(FIXTURE);
    assert!(
        text.contains("%s = catchswitch within none [label %catch] unwind to caller"),
        "got:\n{text}"
    );
    assert!(text.contains("%p = catchpad within %s []"), "got:\n{text}");
    // `@preallocated_indirect` keeps its parameter attribute and its operand
    // bundle now that `parse_call` builds every callee shape through one
    // `CallInst::Create`. It used to print `call void %f(ptr %x)`.
    assert!(
        text.contains(r#"call void %f(ptr preallocated(i32) %x) [ "preallocated"(token %cs) ]"#),
        "got:\n{text}"
    );
}

/// **No upstream counterpart.** This pins the parser/printer contract that the
/// missing named-result dispatch broke: `crates/llvmkit-ir/src/asm_writer.rs`
/// marks `catchswitch` as printing a result name, so every `catchswitch`
/// llvmkit emits carries a `%name =` — text llvmkit's own parser rejected until
/// its instruction dispatch stripped the result name once, ahead of the opcode,
/// the way `LLParser::parseBasicBlock` does. The IR is written here the way
/// `crates/llvmkit-ir/tests/builder_funclet.rs` builds it, printed, and fed
/// straight back to the parser.
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

/// `llvm/test/Verifier/invalid-eh.ll`, vendored verbatim, run the way its own
/// `RUN` lines run it: twenty-six `sed`-selected cases, each its own module,
/// each asserted against the `CHECK<n>` line the fixture writes for it.
///
/// This is `Verifier`'s EH chapter end to end — `visitEHPadPredecessors`,
/// `visitFuncletPadInst` and `verifySiblingFuncletUnwinds`, plus the per-opcode
/// `visit*Inst` routines that call them and `visitInvokeInst`'s
/// unwind-destination `Check`.
///
/// One message is asserted per case, because upstream's `Verifier` accumulates
/// where `Module::verify_borrowed` reports the first failure. For twenty-four
/// of the twenty-six that message is the case's own `CHECK<n>` line; for the
/// two exceptions it is the other upstream `Check` literal the same module
/// raises first, and the table says which. The `CHECK<n>-NEXT` lines are
/// `CheckFailed`'s value list, which `IrError::VerifierFailure` has no field
/// for.
#[test]
fn upstream_invalid_eh_fixture_messages_match() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/invalid-eh.ll");
    let cases: [(u32, &str); 26] = [
        (1, "CatchReturnInst needs to be provided a CatchPad"),
        // T2 and T4 spell `define void @f()` with no `personality`, and both
        // carry a pad. Upstream emits `CHECK2` / `CHECK4` *and* the personality
        // `Check` of `visitCleanupPadInst` / `visitCatchSwitchInst`, in that
        // order; `Module::verify_borrowed` reports only the first. The
        // expectation is therefore upstream's other literal for the same
        // module, not a llvmkit wording.
        (
            2,
            "CleanupPadInst needs to be in a function with a personality.",
        ),
        (3, "CleanupReturnInst needs to be provided a CleanupPad"),
        (
            4,
            "CatchSwitchInst needs to be in a function with a personality.",
        ),
        (5, "CleanupPadInst has an invalid parent"),
        (
            6,
            "Block containg CatchPadInst must be jumped to only by its catchswitch",
        ),
        (7, "CatchSwitchInst has an invalid parent"),
        (8, "CatchSwitchInst handlers must be catchpads"),
        (9, "EH pad cannot handle exceptions raised within it"),
        (10, "EH pad cannot handle exceptions raised within it"),
        (11, "A single unwind edge may only enter one EH pad"),
        (12, "A cleanupret must exit its cleanup"),
        (13, "EH pad cannot handle exceptions raised within it"),
        (
            14,
            "Unwind edges out of a funclet pad must have the same unwind dest",
        ),
        (
            15,
            "Unwind edges out of a funclet pad must have the same unwind dest",
        ),
        (
            16,
            "Unwind edges out of a catch must have the same unwind dest as the parent catchswitch",
        ),
        (
            17,
            "Unwind edges out of a catch must have the same unwind dest as the parent catchswitch",
        ),
        (18, "EH pads can't handle each other's exceptions"),
        (19, "EH pads can't handle each other's exceptions"),
        (20, "Catchswitch cannot unwind to one of its catchpads"),
        (21, "Catchswitch cannot unwind to one of its catchpads"),
        (
            22,
            "The unwind destination does not have an exception handling instruction!",
        ),
        (
            23,
            "CatchPadInst needs to be directly nested in a CatchSwitchInst.",
        ),
        (24, "A single unwind edge may only enter one EH pad"),
        (25, "EH pad jumps through a cycle of pads"),
        (26, "A cleanupret must exit its cleanup"),
    ];
    let mut mismatches: Vec<String> = Vec::new();
    for (case, expected) in cases {
        let source = invalid_eh_case(FIXTURE, case);
        let module = Module::dynamic("invalid_eh");
        let parsed = Parser::new(source.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module();
        if let Err(e) = parsed {
            mismatches.push(format!("T{case}: parse failed: {e}"));
            continue;
        }
        let message = match module.verify_borrowed() {
            Ok(()) => {
                mismatches.push(format!("T{case}: verified clean, want {expected:?}"));
                continue;
            }
            Err(llvmkit_ir::IrError::VerifierFailure { message, .. }) => message,
            Err(other) => panic!("T{case}: expected a verifier failure, got {other:?}"),
        };
        if !message.contains(expected) {
            mismatches.push(format!("T{case}: got {message:?}, want {expected:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// `sed -e s/.T<case>://` — the transformation each of `invalid-eh.ll`'s `RUN`
/// lines applies before handing the file to `llvm-as` / `opt`. It uncomments
/// the selected case's lines and leaves every other `;T<n>:` line as the
/// comment it already is.
fn invalid_eh_case(fixture: &str, case: u32) -> String {
    let marker = format!(";T{case}:");
    let mut out: String = fixture
        .lines()
        .map(|line| line.replacen(&marker, "", 1))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

/// `llvm/test/Verifier/invalid-cleanuppad-chain.ll`, vendored verbatim.
///
/// Its first `CHECK` is asserted. The second — `Parent pad must be
/// catchpad/cleanuppad/catchswitch`, from `visitEHPadPredecessors` walking
/// `bb2`'s `cleanuppad` back through the `cleanupret from undef` that reaches
/// it — is upstream's second accumulated failure, and
/// `Module::verify_borrowed` stops at the first.
#[test]
fn upstream_invalid_cleanuppad_chain_fixture_message_matches() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/invalid-cleanuppad-chain.ll");
    let module = Module::dynamic("invalid_cleanuppad_chain");
    Parser::new(FIXTURE.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("the fixture parses; the verifier is the layer that rejects it");
    let message = match module.verify_borrowed() {
        Ok(()) => panic!("upstream's RUN line is `not llvm-as`"),
        Err(llvmkit_ir::IrError::VerifierFailure { message, .. }) => message,
        Err(other) => panic!("expected a verifier failure, got {other:?}"),
    };
    assert!(
        message.contains("CleanupReturnInst needs to be provided a CleanupPad"),
        "{message:?}"
    );
}
