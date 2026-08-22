//! Central ValID parsing/conversion regression tests.

use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_asmparser::{ll_parser::Parser, parser};
use llvmkit_ir::Module;
use llvmkit_ir::module_new;

pub mod support;

use support::line_and_column;

fn parse_err(src: &str) -> ParseError {
    let module = Module::dynamic("parser_val_id");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("parser rejects unsupported value form")
}

/// Mirrors `llvm/include/llvm/AsmParser/Parser.h::parseConstantValue` and
/// `llvm/lib/AsmParser/LLParser.cpp::parseStandaloneConstantValue`: standalone
/// constant parsing consumes exactly one constant and then requires EOF.
#[test]
fn standalone_constant_rejects_trailing_token() {
    let module = module_new!("parser_val_id_constant").expect("fresh module");
    let err = parser::parse_constant_value(b"42 trailing", &module, module.i32_type().as_type())
        .expect_err("parser rejects trailing token after standalone constant");
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "end of string"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID`: floating
/// constexpr opcodes like `fadd` are explicitly rejected in LLVM 22.1.4.
#[test]
fn fadd_constant_expr_rejected_as_unsupported() {
    let err = parse_err("@x = global double fadd (double 1.0, double 2.0)\n");
    // Upstream prints this sentence bare, so it must render verbatim — the
    // variant assertion locks the routing that guarantees that, and the text
    // assertion is the mirror of the upstream FileCheck line.
    assert!(matches!(err, ParseError::Message { .. }));
    assert_eq!(err.to_string(), "fadd constexprs are no longer supported");
}

/// Assert both halves of a diagnostic: the message text *and* the caret
/// position.
///
/// A message-only assertion is what let a wrong anchor ship — the text was
/// upstream's while the caret pointed at an unrelated line. Upstream's own
/// `FileCheck` lines assert both where it has a fixture to assert them in
/// (`test/Assembler/call-nonzero-program-addrspace.ll` writes
/// `[[@LINE-1]]:25:`), so these routine-anchored tests do the same.
fn assert_diagnostic(src: &str, expected: &str, expected_loc: (u32, u32)) {
    let err = parse_err(src);
    assert_eq!(err.to_string(), expected, "message text");
    let start = err
        .loc()
        .unwrap_or_else(|| panic!("`{expected}` should carry a location"))
        .span
        .start;
    let offset = usize::try_from(start).unwrap_or(usize::MAX);
    assert_eq!(
        line_and_column(src.as_bytes(), offset),
        expected_loc,
        "caret position for `{expected}`"
    );
}

fn parse_ok_and_render(src: &str) -> String {
    let module = Module::dynamic("parser_val_id");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser accepts this module");
    format!("{module}")
}

/// `LLParser::getGlobalVal`'s
/// `if (Val) return cast_or_null<GlobalValue>(checkValidVariableType(Loc,
/// "@" + Name, Ty, Val));` — a `@name` that resolves has its
/// `GlobalValue::getType()` compared against the demanded type with `ValTy ==
/// Ty`, and a mismatch is
/// `'@g' defined with type 'ptr' but expected 'ptr addrspace(3)'`
/// (`LLParser::checkValidVariableType`, message quoted verbatim).
///
/// **Anchored on the routine, not on a fixture.** `grep -rn "defined with
/// type" llvm/test/{Assembler,Verifier}` returns seven lines and every one of
/// them spells a **local** (`'%0'`, `'%1'`, `'%w'`, `'%x'`, `'%fnptr42'`,
/// `'%fnptr200'`); the `"@" + Name` / `"@" + Twine(ID)` spellings that
/// `getGlobalVal` passes have no fixture in the vendored tree. The local
/// spelling is covered by the three `*-nonzero-program-addrspace` corpus rows,
/// which pin upstream's column as well as its text.
///
/// Each arm below is one position that reaches `convertValIDToValue`'s
/// `t_GlobalName` / `t_GlobalID` arm. **Both** halves are asserted — the text
/// and the caret — because upstream reports at `ID.Loc`, the `@` token, and a
/// message-only assertion passed while the caret sat on an unrelated line.
#[test]
fn a_global_reference_at_the_wrong_address_space_is_rejected() {
    const WRONG: &str = "'@g' defined with type 'ptr' but expected 'ptr addrspace(3)'";

    // Global initializer.
    assert_diagnostic(
        "@g = global i32 0\n@p = global ptr addrspace(3) @g\n",
        WRONG,
        (2, 30),
    );
    // Constant expression inside an initializer.
    assert_diagnostic(
        "@g = global i32 0\n@p = global i64 ptrtoint (ptr addrspace(3) @g to i64)\n",
        WRONG,
        (2, 44),
    );
    // Instruction operand.
    assert_diagnostic(
        "@g = global i32 0\n\
define void @f(ptr %p) {\n\
\x20\x20store ptr addrspace(3) @g, ptr %p\n\
\x20\x20ret void\n\
}\n",
        WRONG,
        (3, 26),
    );
    // Operand-bundle input.
    assert_diagnostic(
        "@g = global i32 0\n\
declare void @c()\n\
define void @f() {\n\
\x20\x20call void @c() [ \"tag\"(ptr addrspace(3) @g) ]\n\
\x20\x20ret void\n\
}\n",
        WRONG,
        (4, 43),
    );
    // Alias target, resolved immediately.
    assert_diagnostic(
        "@g = global i32 0\n@a = alias i32, ptr addrspace(3) @g\n",
        WRONG,
        (2, 34),
    );
    // `personality` clause.
    assert_diagnostic(
        "@g = global i32 0\n\
define void @f() personality ptr addrspace(3) @g {\n\
\x20\x20ret void\n\
}\n",
        WRONG,
        (2, 47),
    );
    // Metadata operand.
    assert_diagnostic(
        "@g = global i32 0\n!n = !{!0}\n!0 = !{ptr addrspace(3) @g}\n",
        WRONG,
        (3, 25),
    );
    // `getGlobalVal(unsigned ID, …)` quotes `"@" + Twine(ID)` instead.
    assert_diagnostic(
        "@0 = global i32 0\n@1 = global ptr addrspace(3) @0\n",
        "'@0' defined with type 'ptr' but expected 'ptr addrspace(3)'",
        (2, 30),
    );
}

/// The same routine's `ForwardRefVals` arm: upstream's `if (Val)` covers a
/// forward-reference-table hit as well as a symbol-table one, and the
/// placeholder `createGlobalFwdRef(M, PTy)` minted carries the *first*
/// reference's address space. A second reference at a different one therefore
/// fails `checkValidVariableType` against the placeholder, and is reported at
/// the second reference's own `@` token.
///
/// **Anchored on the routine, not on a fixture**, for the reason given on
/// [`a_global_reference_at_the_wrong_address_space_is_rejected`].
#[test]
fn a_second_forward_reference_at_a_different_address_space_is_rejected() {
    assert_diagnostic(
        "@p = global ptr @g\n@q = global ptr addrspace(3) @g\n@g = global i32 0\n",
        "'@g' defined with type 'ptr' but expected 'ptr addrspace(3)'",
        (2, 30),
    );
    assert_diagnostic(
        "@p = global ptr addrspace(3) @g\n@q = global ptr @g\n@g = global i32 0\n",
        "'@g' defined with type 'ptr addrspace(3)' but expected 'ptr'",
        (2, 17),
    );
}

/// `LLParser::getGlobalVal`'s opening
/// `PointerType *PTy = dyn_cast<PointerType>(Ty); if (!PTy) { error(Loc,
/// "global variable reference must have pointer type"); return nullptr; }`,
/// which runs **before** the symbol-table lookup and so fires for a name the
/// module already defines. llvmkit reached that message only through the
/// forward-reference tail, so `[ "tag"(i32 @g) ]` on a defined `@g` was
/// accepted and silently printed as `ptr @g`.
///
/// **Anchored on the routine, not on a fixture**: the message appears in no
/// `llvm/test/{Assembler,Verifier}` `CHECK` line.
#[test]
fn a_global_reference_at_a_non_pointer_type_is_rejected() {
    const NOT_POINTER: &str = "global variable reference must have pointer type";

    assert_diagnostic(
        "@g = global i32 0\n@p = global i32 @g\n",
        NOT_POINTER,
        (2, 17),
    );
    assert_diagnostic(
        "@g = global i32 0\n\
declare void @c()\n\
define void @f() {\n\
\x20\x20call void @c() [ \"tag\"(i32 @g) ]\n\
\x20\x20ret void\n\
}\n",
        NOT_POINTER,
        (4, 30),
    );
    assert_diagnostic(
        "@0 = global i32 0\n@1 = global i32 @0\n",
        NOT_POINTER,
        (2, 17),
    );
}

/// The accepting half of `checkValidVariableType`'s `if (ValTy == Ty) return
/// Val;`: every position rejected by
/// [`a_global_reference_at_the_wrong_address_space_is_rejected`] still parses
/// when the spelled address space is the symbol's own, including the two
/// llvmkit resolves at end of module (`alias` and `personality`) whose
/// deferred records now carry the spelled pointer type rather than a
/// fabricated `ptr`.
///
/// **Anchored on the routine, not on a fixture**, for the reason given there.
#[test]
fn a_global_reference_at_the_matching_address_space_round_trips() {
    let text = parse_ok_and_render(
        "@g = addrspace(3) global i32 0\n\
@p = global ptr addrspace(3) @g\n\
@q = global i64 ptrtoint (ptr addrspace(3) @g to i64)\n\
@a = alias i32, ptr addrspace(3) @g\n\
declare void @c()\n\
define void @f(ptr %p) personality ptr addrspace(3) @g {\n\
\x20\x20store ptr addrspace(3) @g, ptr %p\n\
\x20\x20call void @c() [ \"tag\"(ptr addrspace(3) @g) ]\n\
\x20\x20ret void\n\
}\n",
    );
    assert!(text.contains("@p = global ptr addrspace(3) @g"), "{text}");
    assert!(
        text.contains("@q = global i64 ptrtoint (ptr addrspace(3) @g to i64)"),
        "{text}"
    );
    assert!(
        text.contains("@a = alias i32, ptr addrspace(3) @g"),
        "{text}"
    );
    assert!(text.contains("personality ptr addrspace(3) @g"), "{text}");
    assert!(text.contains("store ptr addrspace(3) @g, ptr %p"), "{text}");
    assert!(
        text.contains("call void @c() [ \"tag\"(ptr addrspace(3) @g) ]"),
        "{text}"
    );
    // The forward-referenced spelling of the same two deferred positions.
    let deferred = parse_ok_and_render(
        "@a = alias i32, ptr addrspace(3) @g\n\
define void @f() personality ptr addrspace(3) @g {\n\
\x20\x20ret void\n\
}\n\
@g = addrspace(3) global i32 0\n",
    );
    assert!(
        deferred.contains("@a = alias i32, ptr addrspace(3) @g"),
        "{deferred}"
    );
    assert!(
        deferred.contains("personality ptr addrspace(3) @g"),
        "{deferred}"
    );
}
