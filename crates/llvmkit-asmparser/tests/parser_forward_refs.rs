//! Forward-reference and slot-numbering parser tests.
//!
//! Mirrors upstream assembler diagnostics around `LLParser::validateEndOfModule`
//! and `PerFunctionState::finishFunction`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;
use llvmkit_ir::module_new;

fn parse_and_render(src: &str) -> String {
    let module = Module::dynamic("forward_refs");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
}

fn parse_err(src: &str) -> ParseError {
    let module = Module::dynamic("forward_refs");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("parser rejects malformed input")
}

fn parse_ok(src: &str) {
    let module = Module::dynamic("forward_refs");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
}

/// Redefining a numbered slot is the same rule as going backwards, and
/// `LLParser::checkValueID` answers it with the same message: by the time
/// `%0` is seen a second time the frontier is already `%1`.
#[test]
fn skip_value_numbers_invalid_is_rejected() {
    let err = parse_err(
        "define i32 @f() {\nentry:\n  %0 = add i32 1, 2\n  %0 = add i32 3, 4\n  ret i32 %0\n}\n",
    );
    assert_eq!(
        err.to_string(),
        "instruction expected to be numbered '%1' or greater"
    );
}

/// Ports **all five** splits of
/// `test/Assembler/skip-value-numbers-invalid.ll`, whose CHECK lines pin
/// `LLParser::checkValueID`'s message for each `(Kind, Prefix)` pair. Note
/// the `label` form carries **no** sigil, as upstream spells it.
///
/// The rule is one-sided: a numbered slot may not go *backwards*.
///
/// The `function_smaller_id` split was unreachable until W8 wired
/// `parseFunctionHeader`'s own `checkValueID(NameLoc, "function", "@", ...)`
/// — a `@N` header used to take its number verbatim, so only three of the
/// five splits were ported.
#[test]
fn numbered_slots_may_not_go_backwards() {
    for (fixture, expected) in [
        (
            include_bytes!("fixtures/upstream/skip-value-numbers/instr_smaller_id.ll").as_slice(),
            "instruction expected to be numbered '%11' or greater",
        ),
        (
            include_bytes!("fixtures/upstream/skip-value-numbers/arg_smaller_id.ll").as_slice(),
            "argument expected to be numbered '%11' or greater",
        ),
        (
            include_bytes!("fixtures/upstream/skip-value-numbers/block_smaller_id.ll").as_slice(),
            "label expected to be numbered '11' or greater",
        ),
        (
            include_bytes!("fixtures/upstream/skip-value-numbers/global_smaller_id.ll").as_slice(),
            "global expected to be numbered '@11' or greater",
        ),
        (
            include_bytes!("fixtures/upstream/skip-value-numbers/function_smaller_id.ll")
                .as_slice(),
            "function expected to be numbered '@11' or greater",
        ),
    ] {
        let module = Module::dynamic("skip_value_numbers_invalid");
        let err = Parser::new(fixture, &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("fixture is rejected");
        assert_eq!(err.to_string(), expected);
    }
}

/// Ports `test/Assembler/skip-value-numbers.ll`: skipping *ahead* is legal
/// for both instruction results and block labels, and the writer renumbers
/// the gaps away — that fixture's own CHECK lines expect `%10, %20, %30` to
/// print as `%1, %2, %3`.
///
/// This is the half llvmkit got wrong: it required each numbered slot to
/// equal the frontier exactly, so every spelling in this fixture was
/// rejected even though `llvm-as` accepts all of them.
#[test]
fn numbered_slots_may_skip_ahead() {
    for fixture in [
        include_bytes!("fixtures/upstream/skip-value-numbers/instr_skip_ahead.ll").as_slice(),
        include_bytes!("fixtures/upstream/skip-value-numbers/blocks_skip_ahead.ll").as_slice(),
    ] {
        let module = Module::dynamic("skip_value_numbers");
        Parser::new(fixture, &module)
            .expect("lexer primes")
            .parse_module()
            .expect("upstream accepts skipped-ahead numbering");
        let printed = format!("{module}");
        assert!(
            !printed.contains("%10") && !printed.contains("%20") && !printed.contains("%30"),
            "gaps should be renumbered away on output, got:\n{printed}"
        );
    }
}

/// Ports `test/Assembler/2007-03-18-InvalidNumberedVar.ll`, whose CHECK line
/// pins `LLParser::checkValidVariableType`'s wording: `%0` is the `icmp`, so
/// the `ret i32 %0` asks for it at the wrong type. The fixture reaches this
/// through unnamed instruction results, which is why it also exercises
/// `setInstName`'s numbering path.
#[test]
fn invalid_numbered_var_reports_the_defining_type() {
    let module = Module::dynamic("invalid_numbered_var");
    let err = Parser::new(
        include_bytes!("fixtures/upstream/invalid-numbered-var/invalid_numbered_var.ll"),
        &module,
    )
    .expect("lexer primes")
    .parse_module()
    .expect_err("fixture is rejected");
    assert_eq!(
        err.to_string(),
        "'%0' defined with type 'i1' but expected 'i32'"
    );
}

/// A numbered local may be used before it is defined: `PerFunctionState::getVal`
/// mints a sentinel of the demanded type and `setInstName` replaces every use
/// of it when the definition arrives. llvmkit rejected this outright before
/// forward references landed.
///
/// No upstream `.ll` fixture asserts the *positive* case for a numbered local
/// on its own — `2009-02-01-UnnamedForwardRef.ll`, which llvmkit's earlier
/// test cited here, is a **global** forward reference (`@X = global ptr @0`)
/// and is ported with the module-level half.
#[test]
fn numbered_local_forward_reference_resolves() {
    let text = parse_and_render(
        "define i32 @f() {\nentry:\n  %0 = add i32 %1, 1\n  %1 = add i32 2, 3\n  ret i32 %0\n}\n",
    );
    assert!(text.contains("%0 = add i32 %1, 1"), "{text}");
    assert!(text.contains("%1 = add i32 2, 3"), "{text}");
}

/// The named half of the same rule (`ForwardRefVals` rather than
/// `ForwardRefValIDs`), resolved by `setInstName`'s named branch.
///
/// No upstream counterpart as a standalone fixture; the rule is the anchor
/// (D11) and `LLParser::PerFunctionState::getVal` is the symbol.
#[test]
fn named_local_forward_reference_resolves() {
    let text = parse_and_render(
        "define i32 @f() {\nentry:\n  %a = add i32 %b, 1\n  %b = add i32 2, 3\n  ret i32 %a\n}\n",
    );
    assert!(text.contains("%a = add i32 %b, 1"), "{text}");
    assert!(text.contains("%b = add i32 2, 3"), "{text}");
}

/// A forward reference that is never defined is `finishFunction`'s
/// `use of undefined value '%x'`, reported at the *first* use.
///
/// No upstream counterpart as a standalone fixture; the message is
/// `LLParser::PerFunctionState::finishFunction`'s, verbatim.
#[test]
fn unresolved_local_forward_reference_names_the_first_use() {
    let err = parse_err("define i32 @f() {\nentry:\n  %a = add i32 %b, 1\n  ret i32 %a\n}\n");
    assert_eq!(err.to_string(), "use of undefined value '%b'");
}

/// `setInstName` compares the definition against the type the forward
/// reference demanded and reports the *reference's* type when they disagree.
///
/// No upstream counterpart as a standalone fixture; the message is
/// `LLParser::PerFunctionState::setInstName`'s, verbatim.
#[test]
fn forward_reference_type_disagreement_names_the_reference_type() {
    let err = parse_err(
        "define i32 @f() {\nentry:\n  %0 = add i32 %1, 1\n  %1 = add i64 2, 3\n  ret i32 %0\n}\n",
    );
    assert_eq!(
        err.to_string(),
        "instruction forward referenced with type 'i32'"
    );
}

/// A loop-carried phi incoming is the commonest forward reference LLVM emits.
/// Upstream reads it with the general `parseValue` inside `parsePHI`, so it is
/// the same machinery as any other operand — no phi-specific deferral.
///
/// No upstream counterpart as a standalone fixture; the shape is `parsePHI`'s
/// own and appears in most `-O2` output.
#[test]
fn loop_carried_phi_incoming_resolves() {
    let text = parse_and_render(
        "define i32 @f(i32 %n) {\n\
         entry:\n  \
           br label %loop\n\
         loop:\n  \
           %acc = phi i32 [ 0, %entry ], [ %next, %loop ]\n  \
           %next = add i32 %acc, 1\n  \
           %c = icmp slt i32 %next, %n\n  \
           br i1 %c, label %loop, label %done\n\
         done:\n  \
           ret i32 %next\n\
         }\n",
    );
    assert!(
        text.contains("%acc = phi i32 [ 0, %entry ], [ %next, %loop ]"),
        "{text}"
    );
}

/// Mirrors `test/Assembler/2002-08-15-UnresolvedGlobalReference.ll` and
/// `2003-04-25-UnresolvedGlobalReference.ll`: unresolved globals fail.
#[test]
fn unresolved_global_reference_is_rejected() {
    let err = parse_err("define i32 @f() {\nentry:\n  %0 = call i32 @missing()\n  ret i32 %0\n}\n");
    assert!(matches!(err, ParseError::UndefinedSymbol { .. }));
}

/// Ports `test/Assembler/2009-02-01-UnnamedForwardRef.ll` (PR3372), whose RUN
/// line is `llvm-as | llvm-dis`: `@X`'s initializer names `@0` before `@0`
/// exists. `getGlobalVal` mints a stand-in and `validateEndOfModule` retires
/// it — the module-level twin of the function-local machinery above.
#[test]
fn unnamed_global_forward_reference_resolves() {
    let module = Module::dynamic("unnamed_forward_ref");
    Parser::new(
        include_bytes!("fixtures/upstream/unnamed-forward-ref/unnamed_forward_ref.ll"),
        &module,
    )
    .expect("lexer primes")
    .parse_module()
    .expect("upstream accepts a forward global reference");
    let text = format!("{module}");
    assert!(text.contains("@X = global ptr @0"), "{text}");
    assert!(text.contains("@0 = global i32 4"), "{text}");
}

/// The named twin: `@a = global ptr @b` with `@b` below it.
///
/// No upstream counterpart as a standalone fixture; the rule is
/// `LLParser::getGlobalVal`'s and the numbered form is pinned above.
#[test]
fn named_global_forward_reference_resolves() {
    let text = parse_and_render("@a = global ptr @b\n@b = global i32 7\n");
    assert!(text.contains("@a = global ptr @b"), "{text}");
    assert!(text.contains("@b = global i32 7"), "{text}");
}

/// A `@`-reference that no definition ever satisfies is
/// `validateEndOfModule`'s `use of undefined value '@x'` — the noun is
/// **value**, not `global`, which upstream reserves for redefinition.
///
/// No upstream counterpart as a standalone fixture; the message is
/// `LLParser::validateEndOfModule`'s, verbatim.
#[test]
fn unresolved_global_forward_reference_names_the_first_use() {
    let err = parse_err("@a = global ptr @missing\n");
    assert_eq!(err.to_string(), "use of undefined value '@missing'");
}

/// Mirrors `test/Assembler/2002-05-02-InvalidForwardRef.ll`: forward function
/// calls resolve to the later declaration when the signatures match.
#[test]
fn upstream_forward_global_reference_fixture_parses() {
    parse_ok(include_str!(
        "fixtures/upstream/Assembler/2002-05-02-InvalidForwardRef.ll"
    ));
}

/// llvmkit-specific regression for the forward-callee path exercised by
/// `test/Assembler/2002-05-02-InvalidForwardRef.ll`: repeated forward
/// references to one function each keep their OWN call-site type rather
/// than being silently rewritten to the first provisional signature. Under
/// opaque pointers a forward-referenced `@foo` is a bare `ptr` like any
/// other callee, so `LLParser::parseCall` lets each call carry its own
/// function type (`CallBase`); the second call stays `i64`, not rewritten
/// to the `i32` the first call provisionally created.
#[test]
fn forward_global_reference_calls_keep_their_own_types() {
    let text = parse_and_render(
        "define void @test() {\nentry:\n  %a = call i32 @foo()\n  %b = call i64 @foo()\n  ret void\n}\ndeclare i32 @foo()\n",
    );
    assert!(text.contains("%a = call i32 @foo()"), "{text}");
    assert!(text.contains("%b = call i64 @foo()"), "{text}");
    assert!(text.contains("declare i32 @foo()"), "{text}");
}

/// Mirrors `test/Assembler/2002-05-02-InvalidForwardRef.ll`: resolving a
/// provisional callee through a later definition preserves parsed parameter
/// names on the function arguments.
#[test]
fn forward_function_definition_applies_parameter_names() {
    let text = parse_and_render(
        "define i32 @caller() {\n\
         entry:\n  \
           %r = call i32 @callee(i32 7)\n  \
           ret i32 %r\n\
         }\n\
         define i32 @callee(i32 %x) {\n\
         entry:\n  \
           ret i32 %x\n\
         }\n",
    );
    assert!(text.contains("define i32 @callee(i32 %x)"), "{text}");
    assert!(text.contains("ret i32 %x"), "{text}");
}

/// Mirrors `LLParser::PerFunctionState::finishFunction`: placeholder blocks
/// created by forward branches must be defined by a later label.
///
/// Upstream keeps blocks in the same `ForwardRefVals` map as values, so the
/// leftover is reported as an undefined **value**, not an undefined label —
/// `finishFunction` has only the one message.
#[test]
fn undefined_block_label_is_rejected() {
    let err = parse_err("define void @f() {\nentry:\n  br label %missing\n}\n");
    assert_eq!(err.to_string(), "use of undefined value '%missing'");
}

/// Mirrors `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
/// SlotMappingTest)`: numbered declarations populate global slots.
#[test]
fn numbered_declare_records_slot_mapping() {
    let module = module_new!("numbered_declare").expect("fresh module");
    let parsed = Parser::new(b"declare void @0()\n", &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    assert!(parsed.slot_mapping.global_values.get(0).is_some());
}
