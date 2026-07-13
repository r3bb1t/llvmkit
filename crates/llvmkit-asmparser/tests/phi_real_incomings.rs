//! Phis with real incoming edges from already-terminated predecessors must
//! parse, round-trip, and verify. This is the most common phi form in LLVM
//! IR; the parser previously accepted only zero-input (dead) phis because phi
//! incoming-block resolution went through the `Unterminated`-only
//! construction path, rejecting any predecessor that was already terminated.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    Module::with_new("phi_real_incomings", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
}

/// Parse, then verify the produced IR, then render — proving the parsed phi
/// is not merely printable but structurally coherent.
fn parse_verify_render(src: &str) -> String {
    Module::with_new("phi_real_incomings", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        let verified = module.verify().expect("parsed IR verifies");
        format!("{verified}")
    })
}

fn parse_err(src: &str) -> ParseError {
    Module::with_new("phi_real_incomings", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects malformed input")
    })
}

/// One predecessor, already terminated before the phi's block — the
/// merge-block shape. `%entry` is fully parsed (and terminated) by the time
/// `merge`'s phi names it.
#[test]
fn phi_incoming_from_terminated_predecessor_parses() {
    let src = "\
define i32 @f(i32 %a) {
entry:
  %x = add i32 %a, 1
  br label %merge
merge:
  %p = phi i32 [ %x, %entry ]
  ret i32 %p
}
";
    let rendered = parse_verify_render(src);
    assert!(
        rendered.contains("phi i32 [ %x, %entry ]"),
        "phi with a real incoming from a terminated predecessor must round-trip, got:\n{rendered}"
    );
}

/// Diamond: `merge` has two predecessors, both terminated before it.
#[test]
fn diamond_phi_two_terminated_predecessors_verifies() {
    let src = "\
define i32 @f(i32 %a, i1 %c) {
entry:
  br i1 %c, label %l, label %r
l:
  %x = add i32 %a, 1
  br label %merge
r:
  %y = add i32 %a, 2
  br label %merge
merge:
  %p = phi i32 [ %x, %l ], [ %y, %r ]
  ret i32 %p
}
";
    let rendered = parse_verify_render(src);
    assert!(
        rendered.contains("[ %x, %l ]") && rendered.contains("[ %y, %r ]"),
        "diamond phi must round-trip both incomings, got:\n{rendered}"
    );
}

/// Loop header phi: an entry incoming (terminated predecessor, resolved value)
/// plus a back-edge incoming whose value is defined later in the same block
/// (a forward reference, exercising the deferred-resolution path) and whose
/// predecessor is the loop block itself (terminated at its own back-branch).
#[test]
fn loop_header_phi_verifies() {
    let src = "\
define i32 @f(i32 %n) {
entry:
  br label %loop
loop:
  %i = phi i32 [ 0, %entry ], [ %next, %loop ]
  %next = add i32 %i, 1
  %done = icmp eq i32 %next, %n
  br i1 %done, label %exit, label %loop
exit:
  ret i32 %i
}
";
    let rendered = parse_verify_render(src);
    assert!(
        rendered.contains("[ 0, %entry ]") && rendered.contains("[ %next, %loop ]"),
        "loop header phi must round-trip, got:\n{rendered}"
    );
}

/// A phi whose predecessor *block* is defined strictly later in the source
/// than the phi itself (a forward-referenced block, not merely a forward
/// value). When the phi is parsed, `%late` does not yet exist; the widened
/// resolution creates the empty forward-ref block, and it is filled in below.
#[test]
fn phi_with_forward_referenced_predecessor_block_verifies() {
    let src = "\
define i32 @f(i32 %a, i1 %c) {
entry:
  %x = add i32 %a, 1
  br i1 %c, label %merge, label %late
merge:
  %p = phi i32 [ %x, %entry ], [ %x, %late ]
  ret i32 %p
late:
  br label %merge
}
";
    let rendered = parse_verify_render(src);
    assert!(
        rendered.contains("[ %x, %entry ]") && rendered.contains("[ %x, %late ]"),
        "phi naming a forward-referenced predecessor block must round-trip, got:\n{rendered}"
    );
}

/// Numbered predecessor blocks exercise the numbered-block resolution path
/// (`get_or_create_numbered_block_label`) rather than the named one.
#[test]
fn phi_with_numbered_predecessor_blocks_verifies() {
    // The unlabeled entry block is slot %0, so the numbered blocks run %1..%3.
    let src = "\
define i32 @f(i32 %a, i1 %c) {
  br i1 %c, label %1, label %2
1:
  br label %3
2:
  br label %3
3:
  %p = phi i32 [ %a, %1 ], [ %a, %2 ]
  ret i32 %p
}
";
    // Same-value incomings from two distinct predecessors are legal.
    let rendered = parse_verify_render(src);
    assert!(
        rendered.contains("phi i32"),
        "phi over numbered predecessors must round-trip, got:\n{rendered}"
    );
}

/// The incoming-value type check now reaches a phi whose predecessor is
/// already terminated: `%pp` is a pointer fed to an `i32` phi, so the edge-add
/// rejects it at parse time (previously the terminated predecessor was
/// rejected first, so this path was unreachable).
#[test]
fn phi_incoming_type_mismatch_from_terminated_predecessor_is_a_parse_error() {
    let src = "\
define i32 @f(i32 %a) {
entry:
  %pp = alloca i8
  br label %merge
merge:
  %p = phi i32 [ %pp, %entry ]
  ret i32 %p
}
";
    let err = parse_err(src);
    let msg = err.to_string();
    assert!(
        msg.contains("phi.add_incoming") && msg.contains("type mismatch"),
        "expected a phi incoming type-mismatch parse error, got: {msg}"
    );
}

/// An incomplete phi with a *numbered* (anonymous) result still reports its
/// parse error at the phi's own source location. The completeness check keys
/// the phi's recorded span by the phi's arena id, so a numbered phi — whose
/// textual result name does not match the diagnostic name — no longer falls
/// back to a default (offset-zero) location.
#[test]
fn incomplete_numbered_phi_error_is_located_at_the_phi() {
    // Block %3 has two predecessors (%1 and %2), but the numbered-result phi
    // `%4` lists only one incoming — a completeness failure.
    let src = "\
define i32 @f(i32 %a, i1 %c) {
  br i1 %c, label %1, label %2
1:
  br label %3
2:
  br label %3
3:
  %4 = phi i32 [ %a, %1 ]
  ret i32 %4
}
";
    let err = parse_err(src);
    let msg = err.to_string();
    assert!(
        msg.contains("phi") && msg.contains("predecessor"),
        "expected an incomplete-phi parse error, got: {msg}"
    );
    let span = err.loc().expect("error carries a location").span;
    assert!(
        span.start > 0,
        "a numbered incomplete phi must be located at the phi, not the default \
         offset-zero span (got {span:?})"
    );
}

/// A vector-typed phi (`<4 x i32>`) is a valid first-class phi result type.
/// The incoming value is a function parameter of the matching vector type,
/// flowing from a terminated predecessor (the merge-block shape). Previously
/// the parser rejected any non-int/float/pointer phi result type at
/// `ll_parser.rs:7328` ("phi result type must be int, float, or pointer").
#[test]
fn vector_phi_result_type_parses_and_verifies() {
    let src = "\
define <4 x i32> @f(<4 x i32> %a) {
entry:
  br label %merge
merge:
  %p = phi <4 x i32> [ %a, %entry ]
  ret <4 x i32> %p
}
";
    let rendered = parse_verify_render(src);
    assert!(
        rendered.contains("phi <4 x i32> [ %a, %entry ]"),
        "vector phi must round-trip, got:\n{rendered}"
    );
}

/// An aggregate (literal struct) phi result type `{i32, i8}` is likewise a
/// valid first-class phi type. Same merge-block shape with a struct-typed
/// function parameter as the incoming value.
#[test]
fn aggregate_struct_phi_result_type_parses_and_verifies() {
    let src = "\
define { i32, i8 } @g({ i32, i8 } %agg) {
entry:
  br label %merge
merge:
  %q = phi { i32, i8 } [ %agg, %entry ]
  ret { i32, i8 } %q
}
";
    let rendered = parse_verify_render(src);
    assert!(
        rendered.contains("phi { i32, i8 } [ %agg, %entry ]"),
        "aggregate struct phi must round-trip, got:\n{rendered}"
    );
}

/// A phi whose result type is not first-class (here a function type
/// `i32 (i32)`) is still rejected — widening the result-type gate accepts
/// every first-class type but must not admit function/void/opaque-struct
/// types. (`void` alone is caught earlier by `parse_type`; a function type
/// reaches the widened result-type gate.)
#[test]
fn non_first_class_phi_result_type_is_a_parse_error() {
    let src = "\
define void @f() {
entry:
  br label %merge
merge:
  %p = phi i32 (i32)
  ret void
}
";
    let err = parse_err(src);
    let msg = err.to_string();
    assert!(
        msg.contains("phi") && msg.contains("first-class"),
        "expected a non-first-class phi result-type parse error, got: {msg}"
    );
}

/// A zero-input (dead) phi still parses — the pre-existing behavior is not
/// regressed by widening predecessor resolution.
#[test]
fn zero_input_phi_still_parses() {
    let src = "\
define void @f() {
entry:
  ret void
dead:
  %p = phi i32
  ret void
}
";
    let rendered = parse_and_render(src);
    assert!(rendered.contains("phi i32"), "got:\n{rendered}");
}
