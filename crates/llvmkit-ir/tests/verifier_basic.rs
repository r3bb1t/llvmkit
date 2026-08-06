//! Positive verifier coverage. Every opcode the IrBuilder ships
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
    AddFlags, Align, AshrFlags, AttrIndex, AttrKind, Attribute, AttributeStorage, Dyn, DynBrand,
    FloatPredicate, FloatValue, FloatValueId, IntPredicate, IntValue, IntValueId, IntrinsicId,
    IrBuilder, IrError, Linkage, LshrFlags, MemoryEffects, MulFlags, PointerValue, PointerValueId,
    SdivFlags, ShlFlags, SubFlags, UdivFlags, VerifierRule, module_new,
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
            Attribute::<DynBrand>::enum_attr(kind).expect("generated enum attribute"),
        );
    }
    attrs.add(
        AttrIndex::Function,
        Attribute::<DynBrand>::memory(MemoryEffects::none()),
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
    let m = module_new!("empty")?;
    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td` definitions for `int_assume`,
/// integer bit operations, min/max, saturation arithmetic, `int_vector_reduce_add`,
/// `int_ptrmask`, and `int_vscale`: canonical overloaded declarations verify.
#[test]
fn verify_represented_intrinsic_declarations() -> Result<(), IrError> {
    let m = module_new!("intrinsics")?;
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
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitIntrinsicCall`: every generated
/// fixed-signature intrinsic declaration materializes through the canonical
/// descriptor path and passes module verification.
#[test]
fn verify_all_fixed_signature_intrinsic_declarations() -> Result<(), IrError> {
    let m = module_new!("all-fixed-intrinsics")?;
    for id in IntrinsicId::all().filter(|id| !id.is_overloaded()) {
        m.get_or_insert_intrinsic_declaration_by_id(id, [])
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
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitFunction`: generated intrinsic
/// declarations must retain TableGen-emitted function attributes such as
/// `nounwind`, `willreturn`, `speculatable`, and `memory(none)`.
#[test]
fn intrinsic_declaration_missing_generated_function_attrs_is_rejected() -> Result<(), IrError> {
    let m = module_new!("intrinsic-missing-function-attrs")?;
    let abs = m.get_or_insert_intrinsic_declaration_by_name("llvm.abs.i32")?;
    let mut attrs = AttributeStorage::new();
    attrs.add(
        AttrIndex::Param(1),
        Attribute::<DynBrand>::enum_attr(AttrKind::ImmArg).expect("generated immarg attribute"),
    );
    m.view(abs).set_attributes(&m, attrs);

    let err = m
        .verify_borrowed()
        .expect_err("missing generated function attrs rejected");
    assert_intrinsic_modifier_error(err);
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitFunction`: generated intrinsic
/// declarations must retain indexed argument attributes from Intrinsics.td;
/// `llvm.abs.*` marks its `is_int_min_poison` argument as `immarg`.
#[test]
fn intrinsic_declaration_missing_generated_argument_attr_is_rejected() -> Result<(), IrError> {
    let m = module_new!("intrinsic-missing-argument-attrs")?;
    let abs =
        m.get_or_insert_intrinsic_declaration_by_id(IntrinsicId::ABS, [m.i32_type().as_type()])?;
    m.view(abs)
        .set_attributes(&m, abs_function_attrs_without_immarg());

    let err = m
        .verify_borrowed()
        .expect_err("missing generated argument attr rejected");
    assert_intrinsic_modifier_error(err);
    Ok(())
}

/// Mirrors `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp` pretty-printer
/// argument metadata: generated declaration construction applies descriptor
/// argument names even when callers use the name-based convenience API.
#[test]
fn intrinsic_declaration_by_name_applies_generated_argument_names() -> Result<(), IrError> {
    let m = module_new!("intrinsic-arg-names")?;
    let intrinsic =
        m.get_or_insert_intrinsic_declaration_by_name("llvm.nvvm.tcgen05.mma.tensor")?;

    assert_eq!(m.view(intrinsic).param(5)?.name().as_deref(), Some("kind"));
    assert_eq!(
        m.view(intrinsic).param(6)?.name().as_deref(),
        Some("cta_group")
    );
    assert_eq!(
        m.view(intrinsic).param(7)?.name().as_deref(),
        Some("collector")
    );

    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `Verifier::visitFunction` / `visitInstruction`: intrinsic
/// declarations may only be used as the direct callee operand, not as an
/// ordinary call argument.
#[test]
fn intrinsic_declaration_used_as_non_callee_operand_is_rejected() -> Result<(), IrError> {
    let m = module_new!("intrinsic-noncallee-use")?;
    let void_ty = m.void_type();
    let intrinsic = m.get_or_insert_intrinsic_declaration_by_name("llvm.bswap.i32")?;
    let sink_ty = m.function_type(void_ty.as_type(), [m.view(intrinsic).signature().as_type()]);
    let sink = m.add_function_dyn("sink", sink_ty, Linkage::External)?;
    let caller_ty = m.function_type_no_parameters(void_ty.as_type());
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.call_dyn(sink, [m.view(intrinsic).as_erased()], "")?;
    b.ret_void()?;

    let err = m
        .verify_borrowed()
        .expect_err("non-callee intrinsic operand rejected");
    assert!(
        err.to_string()
            .contains("intrinsic can only be used as callee"),
        "{err}"
    );
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp` intrinsic validation: public construction
/// rejects direct `llvm.*` declarations that must instead use the canonical
/// intrinsic declaration API.
#[test]
fn direct_represented_intrinsic_declaration_is_rejected() -> Result<(), IrError> {
    let m = module_new!("intrinsic_mismatch")?;
    let i32_ty = m.i32_type().as_type();
    let i64_ty = m.i64_type().as_type();
    let fn_ty = m.function_type(i64_ty, [i32_ty]);
    let err = m
        .add_function_dyn("llvm.bswap.i32", fn_ty, Linkage::External)
        .expect_err("direct intrinsic declaration is rejected");
    match err {
        IrError::ReservedIntrinsicName { name } => {
            assert_eq!(name, "llvm.bswap.i32");
        }
        other => panic!("unexpected verifier error: {other:?}"),
    }
    Ok(())
}

/// `define i32 @id(i32 %x) { ret i32 %x }` -- minimum valid function.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (every TEST
/// constructs `define`d functions and runs `verifyModule`). Mirrors the
/// minimum-valid shape of `test/Verifier/2002-04-13-RetTypes.ll`.
#[test]
fn verify_identity_function() -> Result<(), IrError> {
    let m = module_new!("id")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("id", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    b.ret(x)?;
    m.verify_borrowed()?;
    Ok(())
}

/// Every integer arithmetic + logical opcode plus per-opcode flags.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (general
/// `verifyModule` happy-path) plus `test/Assembler/flags.ll` for the
/// nuw/nsw/exact flag rendering on add/sub/mul/div/shift opcodes.
#[test]
fn verify_int_arithmetic_full() -> Result<(), IrError> {
    let m = module_new!("ia")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("k", fn_ty, Linkage::External)?;
    let bb = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let y: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let a = b.int_add_with_flags(x, y, AddFlags::new().nuw().nsw(), "a")?;
    let s = b.int_sub_with_flags(a, y, SubFlags::new().nsw(), "s")?;
    let mu = b.int_mul_with_flags(s, x, MulFlags::new().nuw(), "mu")?;
    let ud = b.int_udiv_with_flags(mu, 1_i32, UdivFlags::new().exact(), "ud")?;
    let sd = b.int_sdiv_with_flags(ud, 1_i32, SdivFlags::new(), "sd")?;
    let ur = b.int_urem(sd, 1_i32, "ur")?;
    let sr = b.int_srem(ur, 1_i32, "sr")?;
    let sl = b.int_shl_with_flags(sr, 1_i32, ShlFlags::new().nuw(), "sl")?;
    let lr = b.int_lshr_with_flags(sl, 1_i32, LshrFlags::new().exact(), "lr")?;
    let ar = b.int_ashr_with_flags(lr, 1_i32, AshrFlags::new(), "ar")?;
    let aa = b.int_and(ar, x, "aa")?;
    let oo = b.int_or(aa, x, "oo")?;
    let xx = b.int_xor(oo, x, "xx")?;
    b.ret(xx)?;
    m.verify_borrowed()?;
    Ok(())
}

/// Every floating-point arithmetic opcode + `fcmp`.
/// Closest upstream coverage: `unittests/IR/VerifierTest.cpp` (positive
/// `verifyModule` path) plus `test/Assembler/fast-math-flags.ll` for the FP
/// opcode shapes.
#[test]
fn verify_float_arithmetic_full() -> Result<(), IrError> {
    let m = module_new!("fa")?;
    let f32_ty = m.f32_type();
    let fn_ty = m.function_type(f32_ty, [f32_ty.as_type(), f32_ty.as_type()]);
    let f = m.add_function_dyn("k", fn_ty, Linkage::External)?;
    let bb = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let x: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let y: FloatValue<'_, f32, _> = m.view(f).param(1)?.try_into()?;
    let a = b.fp_add(x, y, "a")?;
    let s = b.fp_sub(a, y, "s")?;
    let mu = b.fp_mul(s, x, "mu")?;
    let d = b.fp_div(mu, x, "d")?;
    let r = b.fp_rem(d, x, "r")?;
    let _cmp = b.fp_cmp(FloatPredicate::Oeq, r, x, "cmp")?;
    b.ret(r)?;
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
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let f32_ty = m.f32_type();
    let f64_ty = m.f64_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.function_type(
        i64_ty,
        [
            i64_ty.as_type(),
            f32_ty.as_type(),
            ptr_ty.as_type(),
            m.i8_type().as_type(),
        ],
    );
    let f = m.add_function_dyn("c", fn_ty, Linkage::External)?;
    let bb = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let x: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;
    let y: FloatValue<'_, f32, _> = m.view(f).param(1)?.try_into()?;
    let p: PointerValue<'_, _> = m.view(f).param(2)?.try_into()?;
    let s: IntValue<'_, i8, _> = m.view(f).param(3)?.try_into()?;
    let t: IntValueId<i32, _> = b.trunc(x, i32_ty, "t")?;
    let e: IntValueId<i64, _> = b.sext(t, i64_ty, "e")?;
    let z: IntValueId<i64, _> = b.zext(s, i64_ty, "z")?;
    let xf: FloatValueId<f64, _> = b.fp_ext(y, f64_ty, "xf")?;
    let _xt: FloatValueId<f32, _> = b.fp_trunc(xf, f32_ty, "xt")?;
    let fi: IntValueId<i64, _> = b.fp_to_si(y, i64_ty, "fi")?;
    let _fu: IntValueId<i64, _> = b.fp_to_ui(y, i64_ty, "fu")?;
    let _is: FloatValueId<f32, _> = b.si_to_fp(x, f32_ty, "is")?;
    let _iu: FloatValueId<f32, _> = b.ui_to_fp(x, f32_ty, "iu")?;
    let pi: IntValueId<i64, _> = b.ptr_to_int(p, i64_ty, "pi")?;
    let _ip: PointerValueId<_> = b.int_to_ptr(pi, ptr_ty, "ip")?;
    // `addrspacecast` (identity here -- both ptrs in addr space 0 --
    // is a no-op, but exercises the builder + verifier path).
    let _ac: PointerValueId<_> = b.addrspace_cast(p, ptr_ty, "ac")?;
    let sum = b.int_add(e, z, "sum")?;
    let total = b.int_add(sum, fi, "total")?;
    b.ret(total)?;
    m.verify_borrowed()?;
    Ok(())
}

/// Memory ops + GEP + integer compare + select + phi + control flow.
/// Mirrors `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, GetElementPtrInst)`
/// (GEP verifier rule) plus `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// CreateCondBr)` for the cond-br + phi scaffolding.
#[test]
fn verify_memory_gep_select_control() -> Result<(), IrError> {
    let m = module_new!("mem")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.function_type(i32_ty, [ptr_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("k", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (join, params) = bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "join")?;
    let then_label = then_bb.id();
    let else_label = else_bb.id();
    let join_label = join.id();

    let p: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let v: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let slot = b.alloca(i32_ty, "slot")?;
    b.store_with_align(v, slot, Align::new(4)?)?;
    let loaded: IntValue<'_, i32, _> = b.view(b.int_load::<i32, _, _>(p, "ld")?);
    let cmp = b.int_cmp(IntPredicate::Slt, loaded, 0_i32, "cmp")?;
    let arr_ty = m.array_type(i32_ty, 4);
    let v_dyn: IntValue<'_, llvmkit_ir::IntDyn, _> = v.into();
    let _gep = b.inbounds_gep(arr_ty, p, [v_dyn], "ix")?;
    b.cond_br(cmp, then_label, else_label)?;

    let bt = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_bb);
    let one_const = i32_ty.const_int(1_i32);
    let two_const = i32_ty.const_int(2_i32);
    // Use `loaded` for both arms; the verifier just needs same-typed
    // arms, not different values. ConstantIntValue is not yet a
    // `SelectArm` (constants narrow through value not int-value path).
    let _ = (one_const, two_const);
    let sel = bt.select(cmp, loaded, loaded, "sel")?;
    let sel_arg = bt.view(sel).as_erased();
    bt.br_with_args(join_label, &[sel_arg])?;

    let be = IrBuilder::new_for::<Dyn>(&m).position_at_end(else_bb);
    be.br_with_args(join_label, &[loaded.as_erased()])?;

    let bj = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let p: IntValue<'_, i32, _> = params[0].try_into()?;
    bj.ret(p)?;

    m.verify_borrowed()?;
    Ok(())
}

/// Direct call: caller invokes callee, narrows the return value via
/// `CallInst::return_value`. Mirrors `tests/builder_call.rs`.
/// Mirrors `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, CrossFunctionRef)`
/// (a function calling another function in the same module passes verification).
#[test]
fn verify_call() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let callee = m.add_function_dyn("inc", fn_ty, Linkage::External)?;
    let cb = m.view(callee).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(cb);
    let x: IntValue<'_, i32, _> = m.view(callee).param(0)?.try_into()?;
    let r = b.int_add(x, 1_i32, "r")?;
    b.ret(r)?;

    let caller = m.add_function_dyn("dbl", fn_ty, Linkage::External)?;
    let bb = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let arg: IntValue<'_, i32, _> = m.view(caller).param(0)?.try_into()?;
    let inst = b.call_dyn(callee, [arg.as_erased()], "c1")?;
    let one: IntValue<'_, i32, _> = b
        .view(inst)
        .return_value()
        .expect("non-void call returns a value")
        .try_into()?;
    let two = b.int_add(one, 1_i32, "two")?;
    b.ret(two)?;

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
    let m = module_new!("v")?;
    let void = m.void_type();
    let i1 = m.bool_type();
    let fn_ty = m.function_type(void, [i1.as_type()]);
    let f = m.add_function_dyn("trap", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let then_label = then_bb.id();
    let else_label = else_bb.id();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let cond: IntValue<'_, bool, _> = m.view(f).param(0)?.try_into()?;
    b.cond_br(cond, then_label, else_label)?;

    let bt = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_bb);
    bt.ret_void()?;

    let be = IrBuilder::new_for::<Dyn>(&m).position_at_end(else_bb);
    be.unreachable();

    m.verify_borrowed()?;
    Ok(())
}

/// `Module::verify` consumes and returns `Module<Verified>`.
/// The verified state forwards `Display` to the underlying module.
/// llvmkit-specific: `Module<Verified>` is a typestate brand on the result
/// of `Module::verify`; LLVM C++ has no equivalent (verification is a free
/// function with side effects). Closest upstream coverage:
/// `unittests/IR/VerifierTest.cpp` (the `verifyModule` API surface).
#[test]
fn verify_consuming_returns_branded_module() -> Result<(), IrError> {
    let m = module_new!("brand")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("k", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.ret(i32_ty.const_int(0_i32))?;

    let verified = m.verify()?;
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
/// construct via the public API today (the IrBuilder typestate
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
    let m = module_new!("nt")?;
    let void = m.void_type();
    let fn_ty = m.function_type_no_parameters(void);
    let f = m.add_function_dyn("empty", fn_ty, Linkage::External)?;
    let _entry = m.view(f).append_basic_block(&m, "entry");
    // Deliberately no IrBuilder calls -- block stays empty.
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

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse`: a value
/// defined in an entry block dominates ordinary uses in reachable successors.
#[test]
fn verify_cross_block_dominated_use_passes() -> Result<(), IrError> {
    let m = module_new!("dom_use_ok")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let next = m.view(f).append_basic_block(&m, "next");
    let next_label = next.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let y = b.int_add(x, 1_i32, "y")?;
    b.br(next_label)?;
    let bn = IrBuilder::new_for::<Dyn>(&m).position_at_end(next);
    let z = bn.int_add(y, 1_i32, "z")?;
    bn.ret(z)?;

    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse`: a value
/// defined on only one branch does not dominate an ordinary use after a join.
#[test]
fn verify_cross_block_branch_value_used_after_join_fails() -> Result<(), IrError> {
    let m = module_new!("dom_use_bad")?;
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let join = m.view(f).append_basic_block(&m, "join");
    let then_label = then_bb.id();
    let else_label = else_bb.id();
    let join_label = join.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let cond: IntValue<'_, bool, _> = m.view(f).param(1)?.try_into()?;

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .cond_br(cond, then_label, else_label)?;
    let bt = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_bb);
    let y = bt.int_add(x, 1_i32, "y")?;
    bt.br(join_label)?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(else_bb)
        .br(join_label)?;
    let bj = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let z = bj.int_add(y, 1_i32, "z")?;
    bj.ret(z)?;

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
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse` and
/// `llvm/lib/IR/Dominators.cpp`: PHI incoming values are checked on their
/// incoming predecessor edge.
#[test]
fn verify_phi_incoming_edge_dominance_passes() -> Result<(), IrError> {
    let m = module_new!("dom_phi_ok")?;
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (join, params) = bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "join")?;
    let then_label = then_bb.id();
    let else_label = else_bb.id();
    let join_label = join.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let cond: IntValue<'_, bool, _> = m.view(f).param(1)?.try_into()?;

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .cond_br(cond, then_label, else_label)?;
    let bt = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_bb);
    let y = bt.int_add(x, 1_i32, "y")?;
    bt.br_with_args(join_label, &[m.view(y).as_erased()])?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(else_bb)
        .br_with_args(join_label, &[x.as_erased()])?;
    let bj = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let p: IntValue<'_, i32, _> = params[0].try_into()?;
    bj.ret(p)?;

    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse` and
/// `llvm/lib/IR/Dominators.cpp`: invoke return values are defined on the
/// normal edge and do not dominate the unwind destination.
#[test]
fn verify_invoke_result_used_on_unwind_edge_fails() -> Result<(), IrError> {
    let m = module_new!("dom_invoke_bad")?;
    let i32_ty = m.i32_type();
    let callee_ty = m.function_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", caller_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let normal = m.view(f).append_basic_block(&m, "normal");
    let unwind = m.view(f).append_basic_block(&m, "unwind");
    let normal_label = normal.id();
    let unwind_label = unwind.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let (_sealed, invoke) = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .invoke_dyn(
            m.view(callee),
            Vec::<llvmkit_ir::Value<'_, _>>::new(),
            normal_label,
            unwind_label,
            "iv",
        )?;
    let invoke_value: IntValue<'_, i32, _> = invoke.to_erased().try_into()?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(normal)
        .ret(invoke_value)?;
    let bu = IrBuilder::new_for::<Dyn>(&m).position_at_end(unwind);
    let bad = bu.int_add(invoke_value, x, "bad")?;
    bu.ret(bad)?;

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
}
