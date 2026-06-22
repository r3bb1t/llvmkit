//! Intrinsic parser tests.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError};
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    Module::with_new("parser_intrinsics", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
}

fn parse_err(src: &str) -> ParseError {
    Module::with_new("parser_intrinsics_err", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects intrinsic misuse")
    })
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::int_lifetime_start` and
/// `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseCall`: known direct
/// intrinsic callees may be declared from the callsite.
#[test]
fn known_intrinsic_auto_declares_direct_callee() {
    let text = parse_and_render(
        "define void @f(ptr %p) {\nentry:\n  call void @llvm.lifetime.start.p0(i64 4, ptr %p)\n  ret void\n}\n",
    );
    assert!(
        text.contains("declare void @llvm.lifetime.start.p0(i64 %0, ptr %1)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("declare void @llvm.lifetime.start.p0(i64 %0, ptr %1)"));
}

/// Mirrors `llvm/lib/IR/Verifier.cpp` intrinsic validation: unknown `llvm.*`
/// names are rejected rather than modeled as ordinary functions.
#[test]
fn unknown_intrinsic_is_rejected() {
    let err = parse_err(
        "define void @f() {\nentry:\n  call void @llvm.not.a.real.intrinsic()\n  ret void\n}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "unknown intrinsic"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors LLVM intrinsic verifier rules: intrinsic globals are callable
/// symbols, not ordinary pointer constants.
#[test]
fn intrinsic_non_callee_use_is_rejected() {
    let err = parse_err("@p = global ptr @llvm.lifetime.start.p0\n");
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "intrinsic can only be used as callee")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::int_lifetime_start`: direct
/// intrinsic calls must match the canonical signature.
#[test]
fn intrinsic_signature_mismatch_is_rejected() {
    let err = parse_err(
        "define void @f(ptr %p) {\nentry:\n  call void @llvm.lifetime.start.p0(i32 4, ptr %p)\n  ret void\n}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "intrinsic signature mismatch")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td` definitions for `int_assume`,
/// bit manipulation, saturation arithmetic, min/max, `int_vector_reduce_add`,
/// `int_ptrmask`, and `int_vscale`: represented direct callees auto-declare
/// with the canonical overloaded signature.
#[test]
fn represented_intrinsics_auto_declare_canonical_signatures() {
    let text = parse_and_render(
        "define void @f(i32 %x, i32 %y, i1 %b, <4 x i32> %v, ptr %p) {\nentry:\n  call void @llvm.assume(i1 %b)\n  call i32 @llvm.abs.i32(i32 %x, i1 %b)\n  call i32 @llvm.bswap.i32(i32 %x)\n  call i32 @llvm.bitreverse.i32(i32 %x)\n  call i32 @llvm.ctlz.i32(i32 %x, i1 %b)\n  call i32 @llvm.cttz.i32(i32 %x, i1 %b)\n  call i32 @llvm.ctpop.i32(i32 %x)\n  call i32 @llvm.fshl.i32(i32 %x, i32 %y, i32 %x)\n  call i32 @llvm.fshr.i32(i32 %x, i32 %y, i32 %x)\n  call i32 @llvm.umax.i32(i32 %x, i32 %y)\n  call i32 @llvm.umin.i32(i32 %x, i32 %y)\n  call i32 @llvm.smax.i32(i32 %x, i32 %y)\n  call i32 @llvm.smin.i32(i32 %x, i32 %y)\n  call i32 @llvm.uadd.sat.i32(i32 %x, i32 %y)\n  call i32 @llvm.usub.sat.i32(i32 %x, i32 %y)\n  call i32 @llvm.sadd.sat.i32(i32 %x, i32 %y)\n  call i32 @llvm.ssub.sat.i32(i32 %x, i32 %y)\n  call <4 x i32> @llvm.ctpop.v4i32(<4 x i32> %v)\n  call <4 x i32> @llvm.uadd.sat.v4i32(<4 x i32> %v, <4 x i32> %v)\n  call i32 @llvm.vector.reduce.add.v4i32(<4 x i32> %v)\n  call ptr @llvm.ptrmask.p0.i64(ptr %p, i64 7)\n  call i32 @llvm.vscale.i32()\n  ret void\n}\n",
    );

    for declaration in [
        "declare void @llvm.assume(i1 %0)",
        "declare i32 @llvm.abs.i32(i32 %0, i1 %1)",
        "declare i32 @llvm.bswap.i32(i32 %0)",
        "declare i32 @llvm.bitreverse.i32(i32 %0)",
        "declare i32 @llvm.ctlz.i32(i32 %0, i1 %1)",
        "declare i32 @llvm.cttz.i32(i32 %0, i1 %1)",
        "declare i32 @llvm.ctpop.i32(i32 %0)",
        "declare i32 @llvm.fshl.i32(i32 %0, i32 %1, i32 %2)",
        "declare i32 @llvm.fshr.i32(i32 %0, i32 %1, i32 %2)",
        "declare i32 @llvm.umax.i32(i32 %0, i32 %1)",
        "declare i32 @llvm.umin.i32(i32 %0, i32 %1)",
        "declare i32 @llvm.smax.i32(i32 %0, i32 %1)",
        "declare i32 @llvm.smin.i32(i32 %0, i32 %1)",
        "declare i32 @llvm.uadd.sat.i32(i32 %0, i32 %1)",
        "declare i32 @llvm.usub.sat.i32(i32 %0, i32 %1)",
        "declare i32 @llvm.sadd.sat.i32(i32 %0, i32 %1)",
        "declare i32 @llvm.ssub.sat.i32(i32 %0, i32 %1)",
        "declare <4 x i32> @llvm.ctpop.v4i32(<4 x i32> %0)",
        "declare <4 x i32> @llvm.uadd.sat.v4i32(<4 x i32> %0, <4 x i32> %1)",
        "declare i32 @llvm.vector.reduce.add.v4i32(<4 x i32> %0)",
        "declare ptr @llvm.ptrmask.p0.i64(ptr %0, i64 %1)",
        "declare i32 @llvm.vscale.i32()",
    ] {
        assert!(
            text.contains(declaration),
            "missing `{declaration}` in:\n{text}"
        );
    }
}

/// Port of `llvm/test/Bitcode/compatibility.ll`: `llvm.readcyclecounter`
/// declares as `i64 ()` and round-trips a direct call.
#[test]
fn readcyclecounter_intrinsic_round_trips() {
    let text = parse_and_render(
        "declare i64 @llvm.readcyclecounter()\n\ndefine i64 @test_readcyclecounter() {\nentry:\n  %0 = call i64 @llvm.readcyclecounter()\n  ret i64 %0\n}\n",
    );

    assert!(
        text.contains("declare i64 @llvm.readcyclecounter()"),
        "{text}"
    );
    assert!(text.contains("call i64 @llvm.readcyclecounter()"), "{text}");
}

/// Mirrors `llvm/lib/IR/Verifier.cpp` intrinsic signature validation: every
/// represented intrinsic family rejects call-site signatures that disagree
/// with the overloaded name's canonical type.
#[test]
fn represented_intrinsic_signature_mismatches_are_rejected() {
    for src in [
        "define void @f(i32 %x) {\nentry:\n  call void @llvm.assume(i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.abs.i32(i32 %x, i1 false)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.bswap.i32(i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.bitreverse.i32(i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i32 @llvm.ctlz.i32(i32 %x, i32 0)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i32 @llvm.cttz.i32(i32 %x, i32 0)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.ctpop.i32(i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i32 @llvm.fshl.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i32 @llvm.fshr.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.umax.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.umin.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.smax.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.smin.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.uadd.sat.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.usub.sat.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.sadd.sat.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(i32 %x) {\nentry:\n  call i64 @llvm.ssub.sat.i32(i32 %x, i32 %x)\n  ret void\n}\n",
        "define void @f(<4 x i32> %v) {\nentry:\n  call i64 @llvm.vector.reduce.add.v4i32(<4 x i32> %v)\n  ret void\n}\n",
        "define void @f(ptr %p) {\nentry:\n  call ptr @llvm.ptrmask.p0.i64(ptr %p, i32 7)\n  ret void\n}\n",
        "define void @f() {\nentry:\n  call i64 @llvm.vscale.i32()\n  ret void\n}\n",
    ] {
        let err = parse_err(src);
        match err {
            ParseError::Expected { expected, .. } => {
                assert_eq!(expected, "intrinsic signature mismatch")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }
}
