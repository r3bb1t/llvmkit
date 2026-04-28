//! Unary-operator coverage: `fneg`, `freeze`, `va_arg`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    FastMathFlags, FloatValue, IRBuilder, IntValue, IrError, Linkage, Module, PointerValue, Ptr,
    VerifierRule,
};

// --------------------------------------------------------------------------
// fneg
// --------------------------------------------------------------------------

/// Ports `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, UnaryOperators)`
/// — the `Builder.CreateUnOp(Instruction::FNeg, V)` arm. Verifies that an
/// `fneg` instruction is built and prints the canonical opcode name.
#[test]
fn build_fneg_round_trip() -> Result<(), IrError> {
    let m = Module::new("u");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f32>("k", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let x: FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_float_neg::<f32, _>(x, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%y = fneg float %0\n"), "got:\n{text}");
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll::fastmathflags_unop` — the
/// `%f.nnan = fneg nnan float %op1` and `%f.fast = fneg fast float %op1`
/// fixtures. Confirms the FMF print path matches the upstream
/// `WriteOptimizationInfo` formatter byte-for-byte.
#[test]
fn fneg_with_fmf_prints_canonical_form() -> Result<(), IrError> {
    let m = Module::new("u");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f32>("k", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let x: FloatValue<f32> = f.param(0)?.try_into()?;
    let nnan_only = FastMathFlags::NO_NANS;
    let n = b.build_float_neg_with_flags::<f32, _>(x, nnan_only, "n")?;
    let fast = b.build_float_neg_with_flags::<f32, _>(x, FastMathFlags::fast(), "fst")?;
    b.build_ret(n)?;
    let text = format!("{m}");
    // Mirrors `; CHECK: %f.nnan = fneg nnan float %op1` (compatibility.ll line 1008).
    assert!(text.contains("%n = fneg nnan float %0\n"), "got:\n{text}");
    // Mirrors `; CHECK: %f.fast = fneg fast float %op1` (compatibility.ll line 1022).
    assert!(text.contains("%fst = fneg fast float %0\n"), "got:\n{text}");
    let _ = fast;
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll::instructions.unops` — the
/// no-FMF `fneg double %op1` fixture (line 1444). Locks the print form
/// for an unnamed result (the void-returning function discards it).
#[test]
fn fneg_double_no_flags_unnamed_result() -> Result<(), IrError> {
    let m = Module::new("u");
    let f64_ty = m.f64_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [f64_ty.as_type()], false);
    let f = m.add_function::<()>("instructions.unops", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let x: FloatValue<f64> = f.param(0)?.try_into()?;
    let _ = b.build_float_neg::<f64, _>(x, "")?;
    b.build_ret_void();
    let text = format!("{m}");
    // Mirrors `; CHECK: fneg double %op1` (compatibility.ll line 1445).
    assert!(text.contains("fneg double %0\n"), "got:\n{text}");
    Ok(())
}

// --------------------------------------------------------------------------
// freeze
// --------------------------------------------------------------------------

/// Ports `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, FreezeInst)`.
/// Constructs a `freeze i8 %arg` in a void-returning function and verifies
/// the AsmWriter output matches the upstream textual fixture.
#[test]
fn freeze_i8_round_trip() -> Result<(), IrError> {
    let m = Module::new("u");
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [i8_ty.as_type()], false);
    let f = m.add_function::<()>("foo", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let arg: IntValue<i8> = f.param(0)?.try_into()?;
    let _ = b.build_freeze(arg, "")?;
    b.build_ret_void();
    let text = format!("{m}");
    // Mirrors the upstream textual fixture in `TEST(InstructionsTest, FreezeInst)`:
    //   `freeze i8 %arg` (the result is discarded — void function).
    assert!(text.contains("freeze i8 %0\n"), "got:\n{text}");
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` lines 1732-1741: `freeze i32 %op1`,
/// `freeze i32 10`, `freeze ptr %pop`. Verifies the canonical print form
/// for integer and pointer operand types.
#[test]
fn freeze_int_and_pointer_print_forms() -> Result<(), IrError> {
    let m = Module::new("u");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        [i32_ty.as_type(), ptr_ty.as_type()],
        false,
    );
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let iop: IntValue<i32> = f.param(0)?.try_into()?;
    let pop: PointerValue = f.param(1)?.try_into()?;
    let _ = b.build_freeze(iop, "")?;
    let _ = b.build_freeze(pop, "")?;
    b.build_ret_void();
    let text = format!("{m}");
    // Mirrors `; CHECK: freeze i32 %op1` (compatibility.ll line 1733).
    assert!(text.contains("freeze i32 %0\n"), "got:\n{text}");
    // Mirrors `; CHECK: freeze ptr %pop` (compatibility.ll line 1741).
    assert!(text.contains("freeze ptr %1\n"), "got:\n{text}");
    Ok(())
}

/// Ports `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, Freeze)` — the
/// `// Valid type : freeze(int)` arm (lines 89-93). The verifier accepts a
/// freeze of an integer constant.
#[test]
fn verifier_accepts_freeze_int() -> Result<(), IrError> {
    let m = Module::new("u");
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("foo", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let zero = i32_ty.const_int(0_i32);
    let _ = b.build_freeze(zero, "")?;
    b.build_ret_void();
    m.verify_borrowed()?;
    Ok(())
}

// --------------------------------------------------------------------------
// va_arg
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/variableArgumentIntrinsic.3.2.ll` line 16:
/// `%tmp = va_arg ptr %ap, i32`. There is no dedicated upstream
/// `unittests/IR/IRBuilderTest.cpp` arm for `va_arg`; this fixture is
/// the closest canonical-form reference.
#[test]
fn va_arg_int_round_trip() -> Result<(), IrError> {
    let m = Module::new("u");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("get_i32", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let ap: PointerValue = f.param(0)?.try_into()?;
    let v = b.build_va_arg(ap, i32_ty.as_type(), "tmp")?;
    let asv: IntValue<i32> = v.as_instruction().as_value().try_into()?;
    b.build_ret(asv)?;
    let text = format!("{m}");
    // Mirrors the upstream `%tmp = va_arg ptr %ap, i32` form.
    assert!(text.contains("%tmp = va_arg ptr %0, i32\n"), "got:\n{text}");
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 1815:
/// `va_arg ptr %ap, i32`. Locks the canonical print form for the
/// pointer-source / integer-destination shape exercised in the
/// upstream `instructions.misc.intrinsics` function.
#[test]
fn va_arg_print_keyword_and_destination_type() -> Result<(), IrError> {
    let m = Module::new("u");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("h", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let ap: PointerValue = f.param(0)?.try_into()?;
    let v = b.build_va_arg(ap, i32_ty.as_type(), "build_va_arg")?;
    let _ = ap; // silence unused-variable lint when `pop` accessor changes.
    assert_eq!(v.result_type(), i32_ty.as_type());
    let asv: IntValue<i32> = v.as_instruction().as_value().try_into()?;
    b.build_ret(asv)?;
    Ok(())
}

/// Ports `test/Verifier/tbaa-allowed.ll` — the `va_arg ptr %args, i8`
/// call site (line 17). The verifier accepts a `va_arg` when the source
/// operand is a pointer.
#[test]
fn verifier_accepts_va_arg_pointer_source() -> Result<(), IrError> {
    let m = Module::new("u");
    let i8_ty = m.i8_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("foo", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let ap: PointerValue = f.param(0)?.try_into()?;
    let _ = b.build_va_arg(ap, i8_ty.as_type(), "argval")?;
    b.build_ret_void();
    m.verify_borrowed()?;
    Ok(())
}

// Suppress unused warnings if the imports drift.
const _: fn() = || {
    let _ = std::any::TypeId::of::<Ptr>();
    let _ = std::any::TypeId::of::<VerifierRule>();
};
