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

fn assert_expected_error(src: &str, expected: &str) {
    match parse_err(src) {
        ParseError::Expected {
            expected: actual, ..
        } => assert_eq!(actual.as_str(), expected),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::int_lifetime_start` and
/// `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseCall`: known direct
/// intrinsic callees may be declared from the callsite.
#[test]
fn known_intrinsic_auto_declares_direct_callee() {
    let text = parse_and_render(
        "define void @f(ptr %p) {\nentry:\n  call void @llvm.lifetime.start.p0(ptr %p)\n  ret void\n}\n",
    );
    assert!(
        text.contains("declare void @llvm.lifetime.start.p0(ptr nocapture %0)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("declare void @llvm.lifetime.start.p0(ptr nocapture %0)"));
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

/// Mirrors `LLParser::parseDeclare` and intrinsic validation: unknown direct
/// `llvm.*` declarations are rejected rather than modeled as ordinary
/// functions.
#[test]
fn unknown_intrinsic_declaration_is_rejected() {
    let err = parse_err("declare void @llvm.not.a.real.intrinsic()\n");
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "unknown intrinsic"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitFunction` and
/// `llvm/lib/IR/Verifier.cpp::visitInstruction`: intrinsic globals are
/// callable symbols, not ordinary pointer constants.
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

/// Mirrors `Intrinsics.td::int_bswap` and verifier intrinsic declaration
/// checks: generated direct declarations must match the canonical signature.
#[test]
fn intrinsic_declaration_signature_mismatch_is_rejected() {
    let err = parse_err("declare i64 @llvm.bswap.i32(i32 %x)\n");
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
        "declare void @llvm.assume(i1 noundef %0)",
        "declare i32 @llvm.abs.i32(i32 %0, i1 immarg %1)",
        "declare i32 @llvm.bswap.i32(i32 %0)",
        "declare i32 @llvm.bitreverse.i32(i32 %0)",
        "declare i32 @llvm.ctlz.i32(i32 %0, i1 immarg %1)",
        "declare i32 @llvm.cttz.i32(i32 %0, i1 immarg %1)",
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

/// Mirrors `Intrinsics.td::int_acos` and `LLParser::parseCall`: generated
/// overloaded intrinsic callees auto-declare with the type encoded in the name.
#[test]
fn generated_float_intrinsic_auto_declares_canonical_signature() {
    let text = parse_and_render(
        "define float @f(float %x) {\nentry:\n  %r = call float @llvm.acos.f32(float %x)\n  ret float %r\n}\n",
    );

    assert!(
        text.contains("declare float @llvm.acos.f32(float %0)"),
        "{text}"
    );
}

/// Mirrors `IntrinsicsAMDGPU.td::int_amdgcn_kill`: target-prefixed generated
/// intrinsic names use the target subtable and canonical declaration path.
#[test]
fn target_intrinsic_auto_declares_canonical_signature() {
    let text = parse_and_render(
        "define void @f(i1 %c) {\nentry:\n  call void @llvm.amdgcn.kill(i1 %c)\n  ret void\n}\n",
    );

    assert!(
        text.contains("declare void @llvm.amdgcn.kill(i1 %0)"),
        "{text}"
    );
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td` definitions for memory
/// intrinsics and `int_expect`: canonical overloaded names auto-declare and
/// round-trip through the generated intrinsic declaration path.
#[test]
fn canonical_memory_and_expect_intrinsics_auto_declare() {
    let called = parse_and_render(
        "define i32 @f(ptr %dst, ptr %src, i64 %n, i32 %x) {\nentry:\n  call void @llvm.memcpy.p0.p0.i64(ptr %dst, ptr %src, i64 %n, i1 false)\n  call void @llvm.memmove.p0.p0.i64(ptr %dst, ptr %src, i64 %n, i1 false)\n  call void @llvm.memset.p0.i64(ptr %dst, i8 0, i64 %n, i1 false)\n  %expected = call i32 @llvm.expect.i32(i32 %x, i32 7)\n  ret i32 %expected\n}\n",
    );
    let declared = parse_and_render(
        "declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)\n\
         declare void @llvm.memmove.p0.p0.i64(ptr, ptr, i64, i1)\n\
         declare void @llvm.memset.p0.i64(ptr, i8, i64, i1)\n\
         declare i32 @llvm.expect.i32(i32, i32)\n",
    );

    for rendered in [&called, &declared] {
        for declaration in [
            "declare void @llvm.memcpy.p0.p0.i64(",
            "declare void @llvm.memmove.p0.p0.i64(",
            "declare void @llvm.memset.p0.i64(",
            "declare i32 @llvm.expect.i32(",
        ] {
            assert!(
                rendered.contains(declaration),
                "missing `{declaration}` in:\n{rendered}"
            );
        }
    }
}

/// Mirrors `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicSignature`: old
/// noncanonical overloaded spellings, bare overloaded names, and wrong
/// signatures are rejected instead of becoming ordinary declarations.
#[test]
fn noncanonical_memory_expect_and_target_intrinsics_are_rejected() {
    for src in [
        "declare void @llvm.memcpy.p0.p0(ptr, ptr, i64, i1)\n",
        "declare void @llvm.memmove.p0.p0(ptr, ptr, i64, i1)\n",
        "declare void @llvm.memset.p0(ptr, i8, i64, i1)\n",
        "declare void @llvm.memcpy(ptr, ptr, i64, i1)\n",
        "declare void @llvm.memmove(ptr, ptr, i64, i1)\n",
        "declare void @llvm.memset(ptr, i8, i64, i1)\n",
        "declare i32 @llvm.expect(i32, i32)\n",
        "declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i32, i1)\n",
        "declare void @llvm.memmove.p0.p0.i64(ptr, i32, i64, i1)\n",
        "declare void @llvm.memset.p0.i64(ptr, i32, i64, i1)\n",
        "declare i64 @llvm.expect.i32(i32, i32)\n",
        "declare void @llvm.amdgcn.kill(i32)\n",
    ] {
        assert_expected_error(src, "intrinsic signature mismatch");
    }
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseDeclare` and
/// `llvm/lib/IR/Verifier.cpp::visitFunction`: canonical function attributes
/// may be supplied through an attribute group while preserving the generated
/// intrinsic declaration.
#[test]
fn intrinsic_declaration_accepts_matching_function_attr_group() {
    let text = parse_and_render(
        "attributes #0 = { nounwind nocallback nosync nofree willreturn speculatable nocreateundeforpoison memory(none) }\ndeclare i32 @llvm.bswap.i32(i32 %x) #0\n",
    );

    assert!(
        text.contains("declare i32 @llvm.bswap.i32(i32 %x) nounwind nocallback nosync nofree willreturn speculatable nocreateundeforpoison memory(none)"),
        "{text}"
    );
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseDeclare` and
/// `llvm/lib/IR/Verifier.cpp::visitFunction`: inline generated attributes may
/// be supplied alone or duplicate a matching resolved group.
#[test]
fn intrinsic_declaration_accepts_inline_generated_attr_with_matching_group() {
    let expected = "declare i32 @llvm.bswap.i32(i32 %x) nounwind nocallback nosync nofree willreturn speculatable nocreateundeforpoison memory(none)";
    let inline = parse_and_render("declare i32 @llvm.bswap.i32(i32 %x) nounwind\n");
    let grouped = parse_and_render(
        "attributes #0 = { nounwind nocallback nosync nofree willreturn speculatable nocreateundeforpoison memory(none) }\ndeclare i32 @llvm.bswap.i32(i32 %x) nounwind #0\n",
    );

    assert!(inline.contains(expected), "{inline}");
    assert!(grouped.contains(expected), "{grouped}");
}

/// Mirrors `LLParser::parseDeclare`: a declaration may spell only a subset of
/// generated function attributes; accepted intrinsic declarations still
/// canonicalize to the generated attribute set.
#[test]
fn intrinsic_declaration_accepts_partial_function_attr_group() {
    let text =
        parse_and_render("attributes #0 = { nounwind }\ndeclare i32 @llvm.bswap.i32(i32 %x) #0\n");

    assert!(
        text.contains("declare i32 @llvm.bswap.i32(i32 %x) nounwind nocallback nosync nofree willreturn speculatable nocreateundeforpoison memory(none)"),
        "{text}"
    );
}

/// Mirrors `LLParser::parseDeclare`: declaration attributes outside the
/// generated intrinsic attribute set are rejected instead of being dropped
/// while the declaration is canonicalized.
#[test]
fn intrinsic_declaration_rejects_extra_function_attr_group() {
    for src in [
        "attributes #0 = { nounwind noinline }\ndeclare i32 @llvm.bswap.i32(i32 %x) #0\n",
        "declare i32 @llvm.bswap.i32(i32 %x) noinline\n",
    ] {
        assert_expected_error(src, "intrinsic declaration attribute mismatch");
    }
}

/// Mirrors `LLParser::parseDeclare`: declaration modifiers outside the
/// canonical generated intrinsic declaration are rejected immediately instead
/// of being dropped while the declaration is canonicalized.
#[test]
fn intrinsic_declaration_rejects_noncanonical_suffix_modifier() {
    for src in [
        "declare void @llvm.assume(i1 noundef %x) section \"intr\"\n",
        "declare void @llvm.assume(i1 noundef %x) partition \"intr\"\n",
        "$intr = comdat any\ndeclare void @llvm.assume(i1 noundef %x) comdat($intr)\n",
        "declare void @llvm.assume(i1 noundef %x) align 4\n",
        "declare void @llvm.assume(i1 noundef %x) gc \"statepoint-example\"\n",
        "@g = global i32 0\ndeclare void @llvm.assume(i1 noundef %x) prefix ptr @g\n",
        "@g = global i32 0\ndeclare void @llvm.assume(i1 noundef %x) prologue ptr @g\n",
        "@g = global i32 0\ndeclare void @llvm.assume(i1 noundef %x) personality ptr @g\n",
        "!0 = !{}\ndeclare void @llvm.assume(i1 noundef %x) !dbg !0\n",
        "declare void @llvm.assume(i1 noundef %x) unnamed_addr\n",
        "declare void @llvm.assume(i1 noundef %x) addrspace(1)\n",
        "declare fastcc void @llvm.assume(i1 noundef %x)\n",
    ] {
        assert_expected_error(src, "intrinsic declaration modifier");
    }
}

/// Mirrors `IntrinsicsDirectX.td::int_dx_resource_handlefrombinding`: target
/// extension overload suffixes parse back into canonical generated
/// declarations.
#[test]
fn target_extension_intrinsic_auto_declares_canonical_signature() {
    let text = parse_and_render(
        "define target(\"dx.Resource\") @f(ptr %non_uniform) {\nentry:\n  %r = call target(\"dx.Resource\") @llvm.dx.resource.handlefrombinding.tdx.Resourcet(i32 0, i32 0, i32 0, i32 0, ptr %non_uniform)\n  ret target(\"dx.Resource\") %r\n}\n",
    );

    assert!(
        text.contains("declare target(\"dx.Resource\") @llvm.dx.resource.handlefrombinding.tdx.Resourcet(i32 %0, i32 %1, i32 %2, i32 %3, ptr %4)"),
        "{text}"
    );
}

/// Mirrors `Verifier::visitIntrinsicCall`: generated overloaded intrinsic
/// names reject signatures whose type suffix does not satisfy the IIT kind.
#[test]
fn generated_float_intrinsic_signature_mismatch_is_rejected() {
    let err = parse_err(
        "define i32 @f(i32 %x) {\nentry:\n  %r = call i32 @llvm.acos.i32(i32 %x)\n  ret i32 %r\n}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "intrinsic signature mismatch")
        }
        other => panic!("unexpected error variant: {other:?}"),
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

/// Mirrors `LLParser::parseType`: target-extension type parameters must
/// precede integer parameters, even when the legacy typed-pointer lookahead is
/// considering a trailing `*`.
#[test]
fn target_extension_type_rejects_type_parameter_after_integer_parameter_before_pointer_suffix() {
    let err = parse_err("declare void @f(target(\"dx.Resource\", 1, i32)* %p)\n");
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "target extension type")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}
