//! Parser safety regression tests.
//!
//! These lock parser diagnostics that should never silently fall back to a
//! different IR construct.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_err(src: &str) -> ParseError {
    let module = Module::dynamic("parser_errors");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("parser rejects malformed input")
}

/// Mirrors `LLParser.cpp::parseType`: integer widths outside LLVM's modeled
/// range are rejected instead of falling back to another integer type.
#[test]
fn malformed_integer_type_rejects_width_overflow() {
    let err = parse_err("@x = global i16777216 0\n");
    match err {
        ParseError::IntegerWidthOutOfRange { width, .. } => assert_eq!(width, 16_777_216),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `LLParser::parseValID`'s `default:` arm: a shufflevector mask
/// element must be a valid value token, never a silently substituted value.
///
/// The message used to be llvmkit's own `valid shufflevector mask element`,
/// re-worded from a lexer failure inside `parse_shuffle_mask`.
/// `LLParser::parseShuffleVector` re-words nothing — it propagates
/// `parseTypeAndValue` — and now so does llvmkit, so upstream's own text
/// arrives.
#[test]
fn malformed_shuffle_mask_rejects_bad_element() {
    let err = parse_err(
        "define <4 x i32> @shuffle(<4 x i32> %a, <4 x i32> %b) {\n\
entry:\n\
  %r = shufflevector <4 x i32> %a, <4 x i32> %b, <4 x i32> <i32 0, i32 bad>\n\
  ret <4 x i32> %r\n\
}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "value token"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `LLParser::parseShuffleVector` and
/// `ShuffleVectorInst::isValidOperands`'s `const Value *Mask` overload: the
/// mask operand must be a vector of **i32**, not any integer vector later
/// coerced to i32.
///
/// The message used to be llvmkit's own `expected valid shufflevector mask`,
/// raised inside a second routine (`parse_shuffle_mask`) and anchored at the
/// mask. `LLParser::parseShuffleVector` has exactly one error of its own —
/// `error(Loc, "invalid shufflevector operands")`, anchored at the **first
/// operand** — and the mask's type is part of that one check, so that is what
/// llvmkit now reports.
#[test]
fn shufflevector_rejects_non_i32_mask_type() {
    let err = parse_err(
        "define <2 x i8> @shuffle(<2 x i8> %a, <2 x i8> %b) {\n\
entry:\n\
  %r = shufflevector <2 x i8> %a, <2 x i8> %b, <2 x i64> <i64 0, i64 1>\n\
  ret <2 x i8> %r\n\
}\n",
    );
    match err {
        ParseError::Message { message, .. } => {
            assert_eq!(message, "invalid shufflevector operands")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Parse `src` and return `Ok(())` on success, propagating any parse error.
fn parse_ok(src: &str) -> Result<(), ParseError> {
    let module = Module::dynamic("parser_ok");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .map(|_| ())
}

/// A `phi` appearing after a non-phi instruction is a parse error.
///
/// With the auto-hoisting phi builders, feeding a misplaced `phi` to a builder
/// would silently reorder it into valid position, laundering ill-formed `.ll`
/// into valid IR. The parser rejects it up front instead.
///
/// Uses a zero-input `phi` (as in `zero-input-phi/phi_int_round_trips.ll`) so
/// the test isolates *placement*: the guard fires before `parse_phi` runs, and
/// no incoming-edge resolution is involved.
#[test]
fn phi_after_non_phi_is_a_parse_error() {
    let src = r#"
define void @f() {
entry:
  ret void

return:
  %x = add i32 0, 1
  %r = phi i32
  ret void
}
"#;
    let err = parse_err(src);
    let msg = err.to_string();
    assert!(
        msg.contains("phi must be grouped at the top"),
        "expected phi-placement parse error, got: {msg}"
    );
}

/// A `phi` that appears before the first non-phi instruction still parses,
/// even when a non-phi instruction follows it in the same block.
#[test]
fn leading_phis_still_parse() {
    let src = r#"
define void @f() {
entry:
  ret void

return:
  %r = phi i32
  %x = add i32 %r, 1
  ret void
}
"#;
    parse_ok(src).expect("well-placed phi must keep parsing");
}

/// A `phi` whose incoming value type does not match the phi result type is a
/// PARSE error, not something deferred to `verify()`. Here the incoming value
/// `%v` is a `ptr` (from `alloca`) fed to an `i32` phi.
///
/// The rejection comes from `checkValidVariableType`, one layer earlier than
/// the phi itself: `parsePHI` reads each incoming with
/// `parseValue(Ty, Op0, PFS)`, so the name is looked up *at the phi's result
/// type* and disagreeing there is the same error any other operand would
/// give. The phi's own result-type check stays as the backstop for values
/// that do not arrive through a name.
#[test]
fn phi_incoming_type_mismatch_is_a_parse_error() {
    let src = r#"
define i32 @f() {
entry:
  %v = alloca i8
  br label %next

next:
  %p = phi i32 [ %v, %fwd ]
  br label %fwd

fwd:
  ret i32 %p
}
"#;
    let err = parse_err(src);
    assert_eq!(
        err.to_string(),
        "'%v' defined with type 'ptr' but expected 'i32'"
    );
}

/// A phi that omits one of its block's predecessors used to parse fine and
/// only fail `Module::verify()` later, far from the source. After a function
/// is fully parsed all predecessors are known (Cranelift's seal insight), so
/// the parser checks completeness itself, at the phi's location.
///
/// The predecessor the phi *does* list (`%other`) is written as a
/// forward-referenced block on purpose: a phi incoming can only name a block
/// that is still unterminated at edge-add time — a predecessor defined
/// *earlier* is already terminated and rejected by
/// `basic_block_for_construction`. Using the later-defined `%other` keeps the
/// edge resolvable while leaving the earlier `%entry` predecessor unlisted,
/// which is exactly the completeness failure under test: `merge` has two
/// predecessors but the phi supplies one incoming.
#[test]
fn incomplete_phi_is_a_parse_error() {
    let src = r#"
define i32 @f(i32 %a, i1 %c) {
entry:
  br i1 %c, label %merge, label %other
merge:
  %p = phi i32 [ %a, %other ]
  ret i32 %p
other:
  br label %merge
}
"#;
    let err = parse_err(src);
    let msg = err.to_string();
    assert!(msg.contains("phi"), "got: {msg}");
    assert!(msg.contains("predecessor"), "got: {msg}");
}

/// Valid loop-shaped phis (back-edge incoming) must keep parsing — the check
/// runs after the WHOLE function is parsed, so predecessor blocks defined
/// later in the text are fine. Here `%latch` is defined *after* the `loop`
/// header yet is a predecessor of it; a per-block eager check would not yet
/// see that edge, but the end-of-function check does, and the phi is complete.
#[test]
fn loop_phi_still_parses() {
    let src = r#"
define i32 @f(i32 %n) {
entry:
  br label %preheader
loop:
  %i = phi i32 [ 0, %preheader ], [ 1, %latch ]
  %done = icmp eq i32 %i, %n
  br i1 %done, label %exit, label %latch
preheader:
  br label %loop
latch:
  br label %loop
exit:
  ret i32 %i
}
"#;
    parse_ok(src).expect("loop phi must parse");
}
