//! Vector integer arithmetic and comparison through the parser.
//!
//! llvmkit's typed integer handles carry a *scalar* width, so the parser routes
//! vector operands to the erased builder family instead. These lock that path:
//! the IR parses, verifies, and prints back byte-identically.
//!
//! Sources are upstream `.ll` fixtures that use vector binops directly; the
//! `Assembler` corpus is the closest counterpart to what the parser must read.

use llvmkit_asmparser::parser;

/// The indented lines of a function body — the instructions, in order.
///
/// Compared instead of the whole text because the printer materialises the
/// entry block's implicit label (`0:`), which the source omits.
fn body_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| line.starts_with("  "))
        .collect()
}

fn round_trip(source: &str) {
    let module = match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("parses: {error:?}\n--- source ---\n{source}"),
    };
    let verified = module
        .verify()
        .unwrap_or_else(|e| panic!("verifies: {e:?}\n--- source ---\n{source}"));
    let printed = format!("{verified}");
    assert_eq!(
        body_lines(&printed),
        body_lines(source),
        "round trip changed the instructions\n--- printed ---\n{printed}\n--- source ---\n{source}"
    );
}

/// Every integer binary opcode over `<2 x i32>`, with each flag the opcode
/// accepts.
///
/// llvmkit-specific: no single upstream fixture spans the opcode table this
/// way. The closest functional reference is
/// `llvm/lib/IR/Verifier.cpp::visitBinaryOperator`, which accepts integer
/// *vector* operands for exactly this set, and
/// `crates/llvmkit-ir/tests/builder_vector_binop_dyn.rs`, which locks the
/// builder half of the same path.
#[test]
fn every_vector_integer_binop_round_trips() {
    round_trip(
        "define void @f(<2 x i32> %a, <2 x i32> %b) {\n\
         \x20 %add = add <2 x i32> %a, %b\n\
         \x20 %addnw = add nuw nsw <2 x i32> %a, %b\n\
         \x20 %sub = sub <2 x i32> %a, %b\n\
         \x20 %mul = mul nsw <2 x i32> %a, %b\n\
         \x20 %udiv = udiv <2 x i32> %a, %b\n\
         \x20 %udive = udiv exact <2 x i32> %a, %b\n\
         \x20 %sdiv = sdiv exact <2 x i32> %a, %b\n\
         \x20 %urem = urem <2 x i32> %a, %b\n\
         \x20 %srem = srem <2 x i32> %a, %b\n\
         \x20 %shl = shl nuw <2 x i32> %a, %b\n\
         \x20 %lshr = lshr exact <2 x i32> %a, %b\n\
         \x20 %ashr = ashr <2 x i32> %a, %b\n\
         \x20 %and = and <2 x i32> %a, %b\n\
         \x20 %or = or disjoint <2 x i32> %a, %b\n\
         \x20 %xor = xor <2 x i32> %a, %b\n\
         \x20 ret void\n\
         }",
    );
}

/// `icmp` over a vector yields `<N x i1>`, not `i1`.
///
/// llvmkit-specific: locks the result-type computation the erased compare
/// builder performs. Closest upstream reference:
/// `llvm/lib/IR/Verifier.cpp::visitICmpInst`, whose result-type rule is
/// "i1 (or vector of i1 for vector compares)".
#[test]
fn vector_icmp_yields_a_vector_of_i1() {
    round_trip(
        "define void @f(<4 x i16> %a, <4 x i16> %b) {\n\
         \x20 %eq = icmp eq <4 x i16> %a, %b\n\
         \x20 %slt = icmp slt <4 x i16> %a, %b\n\
         \x20 ret void\n\
         }",
    );
}

/// A scalable vector takes the same path as a fixed one.
///
/// llvmkit-specific: the erased builder computes the result type from the
/// operand's vector shape, so `<vscale x N x iM>` has to carry through.
/// Closest upstream reference: `Verifier::visitBinaryOperator`'s
/// `isIntOrIntVectorTy`, which does not distinguish the two vector kinds.
#[test]
fn scalable_vector_binops_round_trip() {
    round_trip(
        "define void @f(<vscale x 4 x i32> %a, <vscale x 4 x i32> %b) {\n\
         \x20 %and = and <vscale x 4 x i32> %a, %b\n\
         \x20 %cmp = icmp ne <vscale x 4 x i32> %a, %b\n\
         \x20 ret void\n\
         }",
    );
}

/// The third block of `TEST_F(ValueTrackingTest, HaveNoCommonBitsSet)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// This is the fixture whose failure to parse surfaced the gap. It is asserted
/// here as a parse/verify/print round trip; the analysis half lives in
/// `value_tracking_predicates.rs`.
#[test]
fn upstream_have_no_common_bits_set_vector_block_parses() {
    round_trip(
        "define void @test(<2 x i32> noundef %A, <2 x i32> noundef %B) {\n\
         \x20 %LHS = and <2 x i32> %A, %B\n\
         \x20 %or = or <2 x i32> %A, %B\n\
         \x20 %RHS = xor <2 x i32> %or, splat (i32 -1)\n\
         \x20 ret void\n\
         }",
    );
}
