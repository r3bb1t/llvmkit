//! Positive verifier coverage. Every opcode the IRBuilder ships
//! should produce IR that passes `Module::verify_borrowed`. Lock that
//! in here so any future regression in the verifier or in the
//! builder's emitted shapes shows up as a test failure rather than
//! silent acceptance of malformed IR.
//!
//! Source: each test mirrors a construction pattern already in use
//! elsewhere in `tests/`. The corpus is sized to cover every opcode
//! at least once; specific edge cases live in their dedicated
//! per-opcode test files.
//!
//! ## Upstream provenance
//!
//! Per-test citations below. Most cases reference
//! `unittests/IR/VerifierTest.cpp` (positive coverage of the rule under
//! test) plus a `test/Verifier/*.ll` fixture when one targets the same
//! shape. llvmkit-specific tests (typestate brand, Rust enum API) are
//! marked accordingly.

use llvmkit_ir::{
    AShrFlags, AddFlags, Align, FloatPredicate, FloatValue, IRBuilder, IntPredicate, IntValue,
    IrError, LShrFlags, Linkage, Module, MulFlags, PointerValue, SDivFlags, ShlFlags, SubFlags,
    Type, UDivFlags, VerifiedModule, VerifierRule,
};

/// Empty module is trivially well-formed.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` -- the
/// `verifyModule` calls in TESTs like `Branch_i1` exercise the empty/positive
/// path. llvmkit-specific: empty-module is the trivial base case.
#[test]
fn verify_empty_module() -> Result<(), IrError> {
    let m = Module::new("empty");
    m.verify_borrowed()?;
    Ok(())
}

/// `define i32 @id(i32 %x) { ret i32 %x }` -- minimum valid function.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (every TEST
/// constructs `define`d functions and runs `verifyModule`). Mirrors the
/// minimum-valid shape of `test/Verifier/2002-04-13-RetTypes.ll`.
#[test]
fn verify_identity_function() -> Result<(), IrError> {
    let m = Module::new("id");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("id", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let x: IntValue<i32> = f.param(0)?.try_into()?;
    b.build_ret(x)?;
    m.verify_borrowed()?;
    Ok(())
}

/// Every integer arithmetic + logical opcode plus per-opcode flags.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (general
/// `verifyModule` happy-path) plus `test/Assembler/flags.ll` for the
/// nuw/nsw/exact flag rendering on add/sub/mul/div/shift opcodes.
#[test]
fn verify_int_arithmetic_full() -> Result<(), IrError> {
    let m = Module::new("ia");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<i32>("k", fn_ty, Linkage::External)?;
    let bb = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(bb);
    let x: IntValue<i32> = f.param(0)?.try_into()?;
    let y: IntValue<i32> = f.param(1)?.try_into()?;
    let a = b.build_int_add_with_flags(x, y, AddFlags::new().nuw().nsw(), "a")?;
    let s = b.build_int_sub_with_flags(a, y, SubFlags::new().nsw(), "s")?;
    let mu = b.build_int_mul_with_flags(s, x, MulFlags::new().nuw(), "mu")?;
    let ud = b.build_int_udiv_with_flags(mu, 1_i32, UDivFlags::new().exact(), "ud")?;
    let sd = b.build_int_sdiv_with_flags(ud, 1_i32, SDivFlags::new(), "sd")?;
    let ur = b.build_int_urem(sd, 1_i32, "ur")?;
    let sr = b.build_int_srem(ur, 1_i32, "sr")?;
    let sl = b.build_int_shl_with_flags(sr, 1_i32, ShlFlags::new().nuw(), "sl")?;
    let lr = b.build_int_lshr_with_flags(sl, 1_i32, LShrFlags::new().exact(), "lr")?;
    let ar = b.build_int_ashr_with_flags(lr, 1_i32, AShrFlags::new(), "ar")?;
    let aa = b.build_int_and(ar, x, "aa")?;
    let oo = b.build_int_or(aa, x, "oo")?;
    let xx = b.build_int_xor(oo, x, "xx")?;
    b.build_ret(xx)?;
    m.verify_borrowed()?;
    Ok(())
}

/// Every floating-point arithmetic opcode + `fcmp`.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (positive
/// `verifyModule` path) plus `test/Assembler/fast-math-flags.ll` for the FP
/// opcode shapes.
#[test]
fn verify_float_arithmetic_full() -> Result<(), IrError> {
    let m = Module::new("fa");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type(), f32_ty.as_type()], false);
    let f = m.add_function::<f32>("k", fn_ty, Linkage::External)?;
    let bb = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(bb);
    let x: FloatValue<f32> = f.param(0)?.try_into()?;
    let y: FloatValue<f32> = f.param(1)?.try_into()?;
    let a = b.build_fp_add(x, y, "a")?;
    let s = b.build_fp_sub(a, y, "s")?;
    let mu = b.build_fp_mul(s, x, "mu")?;
    let d = b.build_fp_div(mu, x, "d")?;
    let r = b.build_fp_rem(d, x, "r")?;
    let _cmp = b.build_fp_cmp(FloatPredicate::Oeq, r, x, "cmp")?;
    b.build_ret(r)?;
    m.verify_borrowed()?;
    Ok(())
}

/// `trunc`/`zext`/`sext`/`fpext`/`fptrunc`/`fptosi`/`sitofp`/`ptrtoint`/
/// `inttoptr`/`addrspacecast`. (`bitcast` lives behind a future
/// builder method; `fptoui`/`uitofp` are exercised below.)
/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// for the cast-shape coverage; verifier acceptance tracks
/// `unittests/IR/VerifierTest.cpp` (positive path).
#[test]
fn verify_casts_full() -> Result<(), IrError> {
    let m = Module::new("c");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let f32_ty = m.f32_type();
    let f64_ty = m.f64_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(
        i64_ty,
        [
            i64_ty.as_type(),
            f32_ty.as_type(),
            ptr_ty.as_type(),
            m.i8_type().as_type(),
        ],
        false,
    );
    let f = m.add_function::<i64>("c", fn_ty, Linkage::External)?;
    let bb = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(bb);
    let x: IntValue<i64> = f.param(0)?.try_into()?;
    let y: FloatValue<f32> = f.param(1)?.try_into()?;
    let p: PointerValue = f.param(2)?.try_into()?;
    let s: IntValue<i8> = f.param(3)?.try_into()?;
    let t: IntValue<i32> = b.build_trunc(x, i32_ty, "t")?;
    let e: IntValue<i64> = b.build_sext(t, i64_ty, "e")?;
    let z: IntValue<i64> = b.build_zext(s, i64_ty, "z")?;
    let xf: FloatValue<f64> = b.build_fp_ext(y, f64_ty, "xf")?;
    let _xt: FloatValue<f32> = b.build_fp_trunc(xf, f32_ty, "xt")?;
    let fi: IntValue<i64> = b.build_fp_to_si(y, i64_ty, "fi")?;
    let _fu: IntValue<i64> = b.build_fp_to_ui(y, i64_ty, "fu")?;
    let _is: FloatValue<f32> = b.build_si_to_fp(x, f32_ty, "is")?;
    let _iu: FloatValue<f32> = b.build_ui_to_fp(x, f32_ty, "iu")?;
    let pi: IntValue<i64> = b.build_ptr_to_int(p, i64_ty, "pi")?;
    let _ip: PointerValue = b.build_int_to_ptr(pi, ptr_ty, "ip")?;
    // `addrspacecast` (identity here -- both ptrs in addr space 0 --
    // is a no-op, but exercises the builder + verifier path).
    let _ac: PointerValue = b.build_addrspace_cast(p, ptr_ty, "ac")?;
    let sum = b.build_int_add(e, z, "sum")?;
    let total = b.build_int_add(sum, fi, "total")?;
    b.build_ret(total)?;
    m.verify_borrowed()?;
    Ok(())
}

/// Memory ops + GEP + integer compare + select + phi + control flow.
/// Mirrors `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, GetElementPtrInst)`
/// (GEP verifier rule) plus `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// CreateCondBr)` for the cond-br + phi scaffolding.
#[test]
fn verify_memory_gep_select_control() -> Result<(), IrError> {
    let m = Module::new("mem");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<i32>("k", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let then_bb = f.append_basic_block("then");
    let else_bb = f.append_basic_block("else");
    let join = f.append_basic_block("join");

    let p: PointerValue = f.param(0)?.try_into()?;
    let v: IntValue<i32> = f.param(1)?.try_into()?;

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let slot = b.build_alloca(i32_ty, "slot")?;
    b.build_store_with_align(v, slot, Align::new(4)?)?;
    let loaded: IntValue<i32> = b.build_int_load::<i32, _>(p, "ld")?;
    let cmp = b.build_int_cmp(IntPredicate::Slt, loaded, 0_i32, "cmp")?;
    let arr_ty = m.array_type(i32_ty, 4);
    let v_dyn: IntValue<llvmkit_ir::IntDyn> = v.into();
    let _gep = b.build_inbounds_gep(arr_ty, p, [v_dyn], "ix")?;
    b.build_cond_br(cmp, then_bb, else_bb)?;

    let bt = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
    let one_const = i32_ty.const_int(1_i32);
    let two_const = i32_ty.const_int(2_i32);
    // Use `loaded` for both arms; the verifier just needs same-typed
    // arms, not different values. ConstantIntValue is not yet a
    // `SelectArm` (constants narrow through value not int-value path).
    let _ = (one_const, two_const);
    let sel = bt.build_select(cmp, loaded, loaded, "sel")?;
    bt.build_br(join)?;

    let be = IRBuilder::new_for::<i32>(&m).position_at_end(else_bb);
    be.build_br(join)?;

    let bj = IRBuilder::new_for::<i32>(&m).position_at_end(join);
    let phi = bj
        .build_int_phi::<i32>("p")?
        .add_incoming(sel, then_bb)?
        .add_incoming(loaded, else_bb)?;
    bj.build_ret(phi.as_int_value())?;

    m.verify_borrowed()?;
    Ok(())
}

/// Direct call: caller invokes callee, narrows the return value via
/// `CallInst::return_value`. Mirrors `tests/builder_call.rs`.
/// Mirrors `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, CrossFunctionRef)`
/// (a function calling another function in the same module passes verification).
#[test]
fn verify_call() -> Result<(), IrError> {
    let m = Module::new("c");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let callee = m.add_function::<i32>("inc", fn_ty, Linkage::External)?;
    let cb = callee.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(cb);
    let x: IntValue<i32> = callee.param(0)?.try_into()?;
    let r = b.build_int_add(x, 1_i32, "r")?;
    b.build_ret(r)?;

    let caller = m.add_function::<i32>("dbl", fn_ty, Linkage::External)?;
    let bb = caller.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(bb);
    let arg: IntValue<i32> = caller.param(0)?.try_into()?;
    let inst = b.build_call(callee, [arg.as_value()], "c1")?;
    let one: IntValue<i32> = inst
        .return_value()
        .expect("non-void call returns a value")
        .try_into()?;
    let two = b.build_int_add(one, 1_i32, "two")?;
    b.build_ret(two)?;

    m.verify_borrowed()?;
    Ok(())
}

/// `ret void` from a void function, with `unreachable` as terminator
/// of an else-branch.
/// Mirrors `test/Verifier/2008-11-15-RetVoid.ll` (void return shape) plus
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` for the
/// branch-to-unreachable construction.
#[test]
fn verify_void_return_and_unreachable() -> Result<(), IrError> {
    let m = Module::new("v");
    let void = m.void_type();
    let i1 = m.bool_type();
    let fn_ty = m.fn_type(void, [i1.as_type()], false);
    let f = m.add_function::<()>("trap", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let then_bb = f.append_basic_block("then");
    let else_bb = f.append_basic_block("else");

    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let cond: IntValue<bool> = f.param(0)?.try_into()?;
    b.build_cond_br(cond, then_bb, else_bb)?;

    let bt = IRBuilder::new_for::<()>(&m).position_at_end(then_bb);
    bt.build_ret_void();

    let be = IRBuilder::new_for::<()>(&m).position_at_end(else_bb);
    be.build_unreachable();

    m.verify_borrowed()?;
    Ok(())
}

/// `Module::verify` consumes and returns a `VerifiedModule<'ctx>`.
/// The brand wrapper forwards `Display` to the underlying module.
/// llvmkit-specific: `VerifiedModule<'ctx>` is a typestate brand on the result
/// of `Module::verify`; LLVM C++ has no equivalent (verification is a free
/// function with side effects). Closest upstream coverage:
/// `unittests/IR/VerifierTest.cpp` (the `verifyModule` API surface).
#[test]
fn verify_consuming_returns_branded_module() -> Result<(), IrError> {
    let m = Module::new("brand");
    let i32_ty = m.i32_type();
    let no_params: [Type<'_>; 0] = [];
    let fn_ty = m.fn_type(i32_ty, no_params, false);
    let f = m.add_function::<i32>("k", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    b.build_ret(0_i32)?;

    let verified: VerifiedModule<'_> = m.verify()?;
    let printed = format!("{verified}");
    assert!(printed.contains("define i32 @k()"), "got:\n{printed}");
    let recovered = verified.unverify();
    let printed2 = format!("{recovered}");
    assert_eq!(printed, printed2);
    Ok(())
}

/// `VerifierRule` is `Copy + Eq + Hash` -- pattern match ergonomics
/// work as advertised in test code.
/// llvmkit-specific: `VerifierRule` is a Rust enum surfacing the verifier's
/// failure mode taxonomy; LLVM C++ returns free-form strings. Closest upstream
/// coverage: `unittests/IR/VerifierTest.cpp` (whose TESTs assert specific
/// failure messages).
#[test]
fn verifier_rule_matchable() {
    let rule = VerifierRule::ReturnTypeMismatch;
    assert!(matches!(rule, VerifierRule::ReturnTypeMismatch));
    let _ = std::collections::HashSet::from([rule]);
}

/// A function that is never given a body should fail verification with
/// MissingTerminator. This is the only "negative" case we can easily
/// construct via the public API today (the IRBuilder typestate
/// prevents emitting most other invalid shapes); broader negative
/// coverage lives in the verifier crate's internal `#[cfg(test)]`
/// suite where bypass constructors can fabricate pathological IR.
/// Mirrors the `MissingTerminator` rule exercised throughout
/// `unittests/IR/VerifierTest.cpp` (e.g. `TEST(VerifierTest, Branch_i1)` builds
/// a function whose entry block must be terminated). No dedicated
/// `test/Verifier/*.ll` fixture exists for the bare "empty block" shape
/// because the parser rejects it before the verifier runs.
#[test]
fn verify_function_with_empty_block_fails_missing_terminator() -> Result<(), IrError> {
    let m = Module::new("nt");
    let void = m.void_type();
    let no_params: [Type<'_>; 0] = [];
    let fn_ty = m.fn_type(void, no_params, false);
    let f = m.add_function::<()>("empty", fn_ty, Linkage::External)?;
    let _entry = f.append_basic_block("entry");
    // Deliberately no IRBuilder calls -- block stays empty.
    let err = m
        .verify_borrowed()
        .expect_err("empty block must fail verification");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::MissingTerminator,
                ..
            }
        ),
        "expected MissingTerminator, got {err:?}"
    );
    Ok(())
}
