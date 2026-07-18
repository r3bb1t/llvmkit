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
    AShrFlags, AddFlags, Align, AttrIndex, AttrKind, Attribute, AttributeStorage, Dyn,
    FloatPredicate, FloatValue, IRBuilder, IntPredicate, IntValue, IntrinsicId, IrError, LShrFlags,
    Linkage, MemoryEffects, Module, MulFlags, PointerValue, SDivFlags, ShlFlags, SubFlags,
    UDivFlags, VerifierRule,
};

fn abs_function_attrs_without_immarg() -> AttributeStorage {
    let mut attrs = AttributeStorage::new();
    for kind in [
        AttrKind::NoUnwind,
        AttrKind::NoCallback,
        AttrKind::NoSync,
        AttrKind::NoFree,
        AttrKind::WillReturn,
        AttrKind::Speculatable,
    ] {
        attrs.add(
            AttrIndex::Function,
            Attribute::enum_attr(kind).expect("generated enum attribute"),
        );
    }
    attrs.add(
        AttrIndex::Function,
        Attribute::memory(MemoryEffects::none()),
    );
    attrs
}

fn assert_intrinsic_modifier_error(err: IrError) {
    match err {
        IrError::InvalidOperation { message } => {
            assert_eq!(message, "intrinsic declaration modifier");
        }
        other => panic!("unexpected verifier error: {other:?}"),
    }
}

/// Empty module is trivially well-formed.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` -- the
/// `verifyModule` calls in TESTs like `Branch_i1` exercise the empty/positive
/// path. llvmkit-specific: empty-module is the trivial base case.
#[test]
fn verify_empty_module() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("empty", |m| {
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td` definitions for `int_assume`,
/// integer bit operations, min/max, saturation arithmetic, `int_vector_reduce_add`,
/// `int_ptrmask`, and `int_vscale`: canonical overloaded declarations verify.
#[test]
fn verify_represented_intrinsic_declarations() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("intrinsics", |m| {
        for name in [
            "llvm.acos.f32",
            "llvm.assume",
            "llvm.abs.i32",
            "llvm.bswap.i32",
            "llvm.bitreverse.i32",
            "llvm.ctlz.i32",
            "llvm.cttz.i32",
            "llvm.ctpop.i32",
            "llvm.fshl.i32",
            "llvm.fshr.i32",
            "llvm.umax.i32",
            "llvm.umin.i32",
            "llvm.smax.i32",
            "llvm.smin.i32",
            "llvm.uadd.sat.i32",
            "llvm.usub.sat.i32",
            "llvm.sadd.sat.i32",
            "llvm.ssub.sat.i32",
            "llvm.ctpop.v4i32",
            "llvm.uadd.sat.v4i32",
            "llvm.vector.reduce.add.v4i32",
            "llvm.ptrmask.p0.i64",
            "llvm.vscale.i32",
        ] {
            m.get_or_insert_intrinsic_declaration_by_name(name)?;
        }

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitIntrinsicCall`: every generated
/// fixed-signature intrinsic declaration materializes through the canonical
/// descriptor path and passes module verification.
#[test]
fn verify_all_fixed_signature_intrinsic_declarations() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("all-fixed-intrinsics", |m| {
        for id in IntrinsicId::all().filter(|id| !id.is_overloaded()) {
            m.get_or_insert_intrinsic_declaration_by_id(id, &[])
                .unwrap_or_else(|err| {
                    panic!(
                        "{}#{} declaration failed verifier setup: {err}",
                        id.enum_name(),
                        id.raw()
                    )
                });
        }

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitFunction`: generated intrinsic
/// declarations must retain TableGen-emitted function attributes such as
/// `nounwind`, `willreturn`, `speculatable`, and `memory(none)`.
#[test]
fn intrinsic_declaration_missing_generated_function_attrs_is_rejected() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("intrinsic-missing-function-attrs", |m| {
        let abs = m.get_or_insert_intrinsic_declaration_by_name("llvm.abs.i32")?;
        let mut attrs = AttributeStorage::new();
        attrs.add(
            AttrIndex::Param(1),
            Attribute::enum_attr(AttrKind::ImmArg).expect("generated immarg attribute"),
        );
        abs.set_attributes(&m, attrs);

        let err = m
            .verify_borrowed()
            .expect_err("missing generated function attrs rejected");
        assert_intrinsic_modifier_error(err);
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitFunction`: generated intrinsic
/// declarations must retain indexed argument attributes from Intrinsics.td;
/// `llvm.abs.*` marks its `is_int_min_poison` argument as `immarg`.
#[test]
fn intrinsic_declaration_missing_generated_argument_attr_is_rejected() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("intrinsic-missing-argument-attrs", |m| {
        let abs = m.get_or_insert_intrinsic_declaration_by_id(
            IntrinsicId::ABS,
            &[m.i32_type().as_type()],
        )?;
        abs.set_attributes(&m, abs_function_attrs_without_immarg());

        let err = m
            .verify_borrowed()
            .expect_err("missing generated argument attr rejected");
        assert_intrinsic_modifier_error(err);
        Ok(())
    })
}

/// Mirrors `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp` pretty-printer
/// argument metadata: generated declaration construction applies descriptor
/// argument names even when callers use the name-based convenience API.
#[test]
fn intrinsic_declaration_by_name_applies_generated_argument_names() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("intrinsic-arg-names", |m| {
        let intrinsic =
            m.get_or_insert_intrinsic_declaration_by_name("llvm.nvvm.tcgen05.mma.tensor")?;

        assert_eq!(intrinsic.param(5)?.name().as_deref(), Some("kind"));
        assert_eq!(intrinsic.param(6)?.name().as_deref(), Some("cta_group"));
        assert_eq!(intrinsic.param(7)?.name().as_deref(), Some("collector"));

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `Verifier::visitFunction` / `visitInstruction`: intrinsic
/// declarations may only be used as the direct callee operand, not as an
/// ordinary call argument.
#[test]
fn intrinsic_declaration_used_as_non_callee_operand_is_rejected() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("intrinsic-noncallee-use", |m| {
        let void_ty = m.void_type();
        let intrinsic = m.get_or_insert_intrinsic_declaration_by_name("llvm.bswap.i32")?;
        let sink_ty = m.fn_type(void_ty.as_type(), [intrinsic.signature().as_type()], false);
        let sink = m.add_function::<(), _>("sink", sink_ty, Linkage::External)?;
        let caller_ty = m.fn_type_no_params(void_ty.as_type(), false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        b.build_call_dyn(sink, [intrinsic.as_value()], "")?;
        b.build_ret_void();

        let err = m
            .verify_borrowed()
            .expect_err("non-callee intrinsic operand rejected");
        assert!(
            err.to_string()
                .contains("intrinsic can only be used as callee"),
            "{err}"
        );
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp` intrinsic validation: public construction
/// rejects direct `llvm.*` declarations that must instead use the canonical
/// intrinsic declaration API.
#[test]
fn direct_represented_intrinsic_declaration_is_rejected() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("intrinsic_mismatch", |m| {
        let i32_ty = m.i32_type().as_type();
        let i64_ty = m.i64_type().as_type();
        let fn_ty = m.fn_type(i64_ty, [i32_ty], false);
        let err = m
            .add_function::<Dyn, _>("llvm.bswap.i32", fn_ty, Linkage::External)
            .expect_err("direct intrinsic declaration is rejected");
        match err {
            IrError::ReservedIntrinsicName { name } => {
                assert_eq!(name, "llvm.bswap.i32");
            }
            other => panic!("unexpected verifier error: {other:?}"),
        }
        Ok(())
    })
}

/// `define i32 @id(i32 %x) { ret i32 %x }` -- minimum valid function.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (every TEST
/// constructs `define`d functions and runs `verifyModule`). Mirrors the
/// minimum-valid shape of `test/Verifier/2002-04-13-RetTypes.ll`.
#[test]
fn verify_identity_function() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("id", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("id", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        b.build_ret(x)?;
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Every integer arithmetic + logical opcode plus per-opcode flags.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (general
/// `verifyModule` happy-path) plus `test/Assembler/flags.ll` for the
/// nuw/nsw/exact flag rendering on add/sub/mul/div/shift opcodes.
#[test]
fn verify_int_arithmetic_full() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("ia", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("k", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "entry");
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
    })
}

/// Every floating-point arithmetic opcode + `fcmp`.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (positive
/// `verifyModule` path) plus `test/Assembler/fast-math-flags.ll` for the FP
/// opcode shapes.
#[test]
fn verify_float_arithmetic_full() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("fa", |m| {
        let f32_ty = m.f32_type();
        let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type(), f32_ty.as_type()], false);
        let f = m.add_function::<f32, _>("k", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "entry");
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
    })
}

/// `trunc`/`zext`/`sext`/`fpext`/`fptrunc`/`fptosi`/`sitofp`/`ptrtoint`/
/// `inttoptr`/`addrspacecast`. (`bitcast` lives behind a future
/// builder method; `fptoui`/`uitofp` are exercised below.)
/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// for the cast-shape coverage; verifier acceptance tracks
/// `unittests/IR/VerifierTest.cpp` (positive path).
#[test]
fn verify_casts_full() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("c", |m| {
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
        let f = m.add_function::<i64, _>("c", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "entry");
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
    })
}

/// Memory ops + GEP + integer compare + select + phi + control flow.
/// Mirrors `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, GetElementPtrInst)`
/// (GEP verifier rule) plus `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// CreateCondBr)` for the cond-br + phi scaffolding.
#[test]
fn verify_memory_gep_select_control() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("mem", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("k", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (join, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "join")?;
        let then_label = then_bb.label();
        let else_label = else_bb.label();
        let join_label = join.label();

        let p: PointerValue = f.param(0)?.try_into()?;
        let v: IntValue<i32> = f.param(1)?.try_into()?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let slot = b.build_alloca(i32_ty, "slot")?;
        b.build_store_with_align(v, slot, Align::new(4)?)?;
        let loaded: IntValue<i32> = b.build_int_load::<i32, _, _>(p, "ld")?;
        let cmp = b.build_int_cmp(IntPredicate::Slt, loaded, 0_i32, "cmp")?;
        let arr_ty = m.array_type(i32_ty, 4);
        let v_dyn: IntValue<llvmkit_ir::IntDyn> = v.into();
        let _gep = b.build_inbounds_gep(arr_ty, p, [v_dyn], "ix")?;
        b.build_cond_br(cmp, then_label, else_label)?;

        let bt = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        let one_const = i32_ty.const_int(1_i32);
        let two_const = i32_ty.const_int(2_i32);
        // Use `loaded` for both arms; the verifier just needs same-typed
        // arms, not different values. ConstantIntValue is not yet a
        // `SelectArm` (constants narrow through value not int-value path).
        let _ = (one_const, two_const);
        let sel = bt.build_select(cmp, loaded, loaded, "sel")?;
        bt.build_br_with_args(join_label, &[sel.as_value()])?;

        let be = IRBuilder::new_for::<i32>(&m).position_at_end(else_bb);
        be.build_br_with_args(join_label, &[loaded.as_value()])?;

        let bj = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let p: IntValue<i32> = params[0].try_into()?;
        bj.build_ret(p)?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Direct call: caller invokes callee, narrows the return value via
/// `CallInst::return_value`. Mirrors `tests/builder_call.rs`.
/// Mirrors `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, CrossFunctionRef)`
/// (a function calling another function in the same module passes verification).
#[test]
fn verify_call() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("c", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let callee = m.add_function::<i32, _>("inc", fn_ty, Linkage::External)?;
        let cb = callee.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(cb);
        let x: IntValue<i32> = callee.param(0)?.try_into()?;
        let r = b.build_int_add(x, 1_i32, "r")?;
        b.build_ret(r)?;

        let caller = m.add_function::<i32, _>("dbl", fn_ty, Linkage::External)?;
        let bb = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(bb);
        let arg: IntValue<i32> = caller.param(0)?.try_into()?;
        let inst = b.build_call_dyn(callee, [arg.as_value()], "c1")?;
        let one: IntValue<i32> = inst
            .return_value()
            .expect("non-void call returns a value")
            .try_into()?;
        let two = b.build_int_add(one, 1_i32, "two")?;
        b.build_ret(two)?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// `ret void` from a void function, with `unreachable` as terminator
/// of an else-branch.
/// Mirrors `test/Verifier/2008-11-15-RetVoid.ll` (void return shape) plus
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` for the
/// branch-to-unreachable construction.
#[test]
fn verify_void_return_and_unreachable() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("v", |m| {
        let void = m.void_type();
        let i1 = m.bool_type();
        let fn_ty = m.fn_type(void, [i1.as_type()], false);
        let f = m.add_function::<(), _>("trap", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        let then_label = then_bb.label();
        let else_label = else_bb.label();

        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let cond: IntValue<bool> = f.param(0)?.try_into()?;
        b.build_cond_br(cond, then_label, else_label)?;

        let bt = IRBuilder::new_for::<()>(&m).position_at_end(then_bb);
        bt.build_ret_void();

        let be = IRBuilder::new_for::<()>(&m).position_at_end(else_bb);
        be.build_unreachable();

        m.verify_borrowed()?;
        Ok(())
    })
}

/// `Module::verify` consumes and returns `Module<Verified>`.
/// The verified state forwards `Display` to the underlying module.
/// llvmkit-specific: `Module<Verified>` is a typestate brand on the result
/// of `Module::verify`; LLVM C++ has no equivalent (verification is a free
/// function with side effects). Closest upstream coverage:
/// `unittests/IR/VerifierTest.cpp` (the `verifyModule` API surface).
#[test]
fn verify_consuming_returns_branded_module() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("brand", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("k", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_ret(0_i32)?;

        let verified = m.verify()?;
        let printed = format!("{verified}");
        assert!(printed.contains("define i32 @k()"), "got:\n{printed}");
        let recovered = verified.unverify();
        let printed2 = format!("{recovered}");
        assert_eq!(printed, printed2);
        Ok(())
    })
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
    Module::with_new::<_, _, _>("nt", |m| {
        let void = m.void_type();
        let fn_ty = m.fn_type_no_params(void, false);
        let f = m.add_function::<(), _>("empty", fn_ty, Linkage::External)?;
        let _entry = f.append_basic_block(&m, "entry");
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
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse`: a value
/// defined in an entry block dominates ordinary uses in reachable successors.
#[test]
fn verify_cross_block_dominated_use_passes() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("dom_use_ok", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let next = f.append_basic_block(&m, "next");
        let next_label = next.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let y = b.build_int_add(x, 1_i32, "y")?;
        b.build_br(next_label)?;
        let bn = IRBuilder::new_for::<i32>(&m).position_at_end(next);
        let z = bn.build_int_add(y, 1_i32, "z")?;
        bn.build_ret(z)?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse`: a value
/// defined on only one branch does not dominate an ordinary use after a join.
#[test]
fn verify_cross_block_branch_value_used_after_join_fails() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("dom_use_bad", |m| {
        let i32_ty = m.i32_type();
        let bool_ty = m.bool_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        let join = f.append_basic_block(&m, "join");
        let then_label = then_bb.label();
        let else_label = else_bb.label();
        let join_label = join.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let cond: IntValue<bool> = f.param(1)?.try_into()?;

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_cond_br(cond, then_label, else_label)?;
        let bt = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        let y = bt.build_int_add(x, 1_i32, "y")?;
        bt.build_br(join_label)?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(else_bb)
            .build_br(join_label)?;
        let bj = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let z = bj.build_int_add(y, 1_i32, "z")?;
        bj.build_ret(z)?;

        let err = m
            .verify_borrowed()
            .expect_err("non-dominating branch value must fail");
        assert!(
            matches!(
                err,
                IrError::VerifierFailure {
                    rule: VerifierRule::UseBeforeDef,
                    ..
                }
            ),
            "got {err:?}"
        );
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse` and
/// `llvm/lib/IR/Dominators.cpp`: PHI incoming values are checked on their
/// incoming predecessor edge.
#[test]
fn verify_phi_incoming_edge_dominance_passes() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("dom_phi_ok", |m| {
        let i32_ty = m.i32_type();
        let bool_ty = m.bool_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (join, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "join")?;
        let then_label = then_bb.label();
        let else_label = else_bb.label();
        let join_label = join.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let cond: IntValue<bool> = f.param(1)?.try_into()?;

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_cond_br(cond, then_label, else_label)?;
        let bt = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        let y = bt.build_int_add(x, 1_i32, "y")?;
        bt.build_br_with_args(join_label, &[y.as_value()])?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(else_bb)
            .build_br_with_args(join_label, &[x.as_value()])?;
        let bj = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let p: IntValue<i32> = params[0].try_into()?;
        bj.build_ret(p)?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse` and
/// `llvm/lib/IR/Dominators.cpp`: invoke return values are defined on the
/// normal edge and do not dominate the unwind destination.
#[test]
fn verify_invoke_result_used_on_unwind_edge_fails() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("dom_invoke_bad", |m| {
        let i32_ty = m.i32_type();
        let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function::<i32, _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", caller_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let normal = f.append_basic_block(&m, "normal");
        let unwind = f.append_basic_block(&m, "unwind");
        let normal_label = normal.label();
        let unwind_label = unwind.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        let (_sealed, invoke) = IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_invoke_dyn(
                callee,
                Vec::<llvmkit_ir::Value>::new(),
                normal_label,
                unwind_label,
                "iv",
            )?;
        let invoke_value: IntValue<i32> = invoke.as_value().try_into()?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(normal)
            .build_ret(invoke_value)?;
        let bu = IRBuilder::new_for::<i32>(&m).position_at_end(unwind);
        let bad = bu.build_int_add(invoke_value, x, "bad")?;
        bu.build_ret(bad)?;

        let err = m
            .verify_borrowed()
            .expect_err("invoke result used on unwind must fail");
        assert!(
            matches!(
                err,
                IrError::VerifierFailure {
                    rule: VerifierRule::UseBeforeDef,
                    ..
                }
            ),
            "got {err:?}"
        );
        Ok(())
    })
}
