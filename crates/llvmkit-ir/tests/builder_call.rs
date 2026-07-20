//! `call` instruction print form and call-builder ergonomics.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` carries a citation naming the upstream
//! `unittests/IR/InstructionsTest.cpp` `TEST` (or `test/Assembler/*.ll`
//! fixture) it ports.

use llvmkit_ir::{
    CallingConv, Dyn, FloatValue, IRBuilder, IntValue, IntrinsicDescriptor, IntrinsicId, IrError,
    Linkage, Module, Ptr,
};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// — exercises construction of a non-void `CallInst` against a declared
/// callee.
#[test]
fn call_int_returning_function() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        // declare i32 @callee(i32, i32)
        let callee = m
            .add_typed_function::<i32, (i32, i32), _>("callee", Linkage::External)?
            .as_function();
        // define i32 @caller(i32 %x, i32 %y) { %r = call i32 @callee(i32 %x, i32 %y); ret i32 %r }
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: llvmkit_ir::IntValue<i32> = caller.param(0)?.try_into()?;
        let y: llvmkit_ir::IntValue<i32> = caller.param(1)?.try_into()?;
        let inst = b.build_call_dyn(callee, [x.as_value(), y.as_value()], "r")?;
        // Typed return accessor (Doctrine D4): `R` flows from the callee
        // through `build_call_dyn` into `CallInst<'ctx, i32>`, which directly
        // exposes `return_int_value(): IntValue<i32>` -- no runtime
        // `try_into` is needed.
        let ret_val = inst.return_int_value();
        b.build_ret(ret_val)?;
        let text = format!("{m}");
        assert!(
            text.contains("%r = call i32 @callee(i32 %0, i32 %1)"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// applied to a `void`-returning callee; the C++ test creates calls
/// against a `FunctionType::get(...)` declaration just like this Rust
/// counterpart.
#[test]
fn call_void_returning_function() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let void_ty = m.void_type();
        // declare void @sink()
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function_dyn("sink", callee_ty, Linkage::External)?;
        // define void @caller() { call void @sink(); ret void }
        let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let inst = b.build_call_dyn(callee, Vec::<llvmkit_ir::Value>::new(), "")?;
        assert!(inst.return_value().is_none());
        b.build_ret_void()?;
        let text = format!("{m}");
        assert!(text.contains("call void @sink()"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific: covers the `call_builder` fluent API mixing an
/// `IntValue<i32>` and a `PointerValue` argument. Closest upstream
/// functional coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CloneCall)`
/// (constructs a `CallInst` with mixed-type args).
#[test]
fn call_builder_mixed_arg_types() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(
            void_ty.as_type(),
            [i32_ty.as_type(), ptr_ty.as_type()],
            false,
        );
        let callee = m.add_function_dyn("with_ptr", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let p: llvmkit_ir::PointerValue = caller.param(0)?.try_into()?;
        // Mixed-type args: an `IntValue<i32>` and a `PointerValue` go into
        // the same call. The builder pattern accepts each via a
        // monomorphised `arg<V: IsValue>` call.
        let answer = m.i32_type().const_int(42_i32);
        b.call_builder(callee).arg(answer).arg(p).build()?;
        b.build_ret_void()?;
        let text = format!("{m}");
        assert!(
            text.contains("call void @with_ptr(i32 42, ptr %0)"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Mirrors the `tail call` textual form locked by
/// `test/Assembler/call-arg-is-callee.ll` and friends in
/// `test/Assembler/`. Closest upstream functional coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`.
#[test]
fn call_tail() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let callee = m
            .add_typed_function::<i32, (), _>("g", Linkage::External)?
            .as_function();
        let caller_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function_dyn("f", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let inst = b.call_builder(callee).tail().name("r").build()?;
        let r = inst.return_int_value();
        b.build_ret(r)?;
        let text = format!("{m}");
        assert!(text.contains("%r = tail call i32 @g()"), "got:\n{text}");
        Ok(())
    })
}

/// Mirrors `Intrinsic::getOrInsertDeclaration` plus `IRBuilder::CreateCall`:
/// an intrinsic call helper inserts the canonical declaration and emits a
/// direct call to it.
#[test]
fn intrinsic_call_inserts_declaration_and_emits_direct_call() -> Result<(), IrError> {
    Module::with_new("intrinsic-call", |m| {
        let f32_ty = m.f32_type();
        let caller_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: FloatValue<f32> = caller.param(0)?.try_into()?;
        let descriptor = IntrinsicDescriptor::new(
            IntrinsicId::lookup("llvm.acos.f32").expect("acos intrinsic"),
            [f32_ty.as_type()],
        )?;
        let call = b.build_intrinsic_call(&descriptor, &[x.as_value()], "r")?;
        let r: FloatValue<f32> = call
            .return_value()
            .ok_or(IrError::InvalidOperation {
                message: "non-void intrinsic result",
            })?
            .try_into()?;
        b.build_ret(r)?;
        let text = format!("{m}");
        assert!(
            text.contains("declare float @llvm.acos.f32(float %0)"),
            "{text}"
        );
        assert!(
            text.contains("%r = call float @llvm.acos.f32(float %0)"),
            "{text}"
        );
        Ok(())
    })
}
/// Mirrors `Intrinsic::getOrInsertDeclaration` plus `IRBuilder::CreateCall`:
/// descriptor-backed intrinsic builders reject operands that do not match the
/// generated IIT signature.
#[test]
fn intrinsic_call_rejects_wrong_argument_type() -> Result<(), IrError> {
    Module::with_new("intrinsic-call-mismatch", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let caller_ty = m.fn_type(m.void_type().as_type(), [i32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let descriptor = IntrinsicDescriptor::new(
            IntrinsicId::lookup("llvm.acos.f32").expect("acos intrinsic"),
            [f32_ty.as_type()],
        )?;
        let err = b
            .build_intrinsic_call(&descriptor, &[x.as_value()], "bad")
            .expect_err("i32 argument should not match llvm.acos.f32");
        assert!(matches!(
            err,
            IrError::IntrinsicSignatureMismatch { name } if name == "llvm.acos.f32"
        ));
        let _ = b.build_ret_void();
        Ok(())
    })
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// specialized for a pointer-returning callee.
#[test]
fn call_to_pointer_returning_function() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let ptr_ty = m.ptr_type(0);
        let callee = m
            .add_typed_function::<Ptr, (), _>("alloc_ptr", Linkage::External)?
            .as_function();
        let caller_ty = m.fn_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function_dyn("g", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let inst = b.build_call_dyn(callee, Vec::<llvmkit_ir::Value>::new(), "p")?;
        let p = inst.return_pointer_value();
        b.build_ret(p)?;
        let text = format!("{m}");
        assert!(text.contains("%p = call ptr @alloc_ptr()"), "got:\n{text}");
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Typed build_call / build_call_with_config / typed_call_builder
// --------------------------------------------------------------------------

/// The typed `build_call` prints identically to the dyn form for the
/// same signature, and its result narrows to `IntValue<i32>` without a
/// runtime `try_into`. Closest upstream coverage: same
/// `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest,
/// CallInst)` shape as `call_int_returning_function`, exercised through
/// the typed callee facade instead of a raw `FunctionValue`.
#[test]
fn typed_build_call_prints_like_dyn_form() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let callee = m.add_typed_function::<i32, (i32, i32), _>("callee", Linkage::External)?;
        let caller = m.add_typed_function::<i32, (i32, i32), _>("caller", Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (x, y) = caller.params();
        let call = b.build_call(callee, (x, y), "r")?;
        let ret_val = call.result();
        b.build_ret(ret_val)?;
        let text = format!("{m}");
        assert!(
            text.contains("%r = call i32 @callee(i32 %0, i32 %1)"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// `build_call_with_config` threads a non-default calling convention
/// into the emitted typed call, mirroring `call_tail`'s dyn-path
/// coverage of `CallSiteConfig`.
#[test]
fn typed_build_call_with_config_threads_calling_convention() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let callee = m.add_typed_function::<i32, (), _>("g", Linkage::External)?;
        callee.as_function().set_calling_conv(&m, CallingConv::FAST);
        let caller = m.add_typed_function::<i32, (), _>("f", Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let call = b.build_call_with_config(
            callee,
            (),
            llvmkit_ir::CallSiteConfig::new("r").calling_conv(CallingConv::FAST),
        )?;
        b.build_ret(call.result())?;
        let text = format!("{m}");
        assert!(text.contains("%r = call fastcc i32 @g()"), "got:\n{text}");
        Ok(())
    })
}

/// `typed_call_builder` chains `.tail()` the same way the dyn
/// `call_builder` does, mirroring `call_tail`.
#[test]
fn typed_call_builder_chains_tail() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let callee = m.add_typed_function::<i32, (), _>("g", Linkage::External)?;
        let caller = m.add_typed_function::<i32, (), _>("f", Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let call = b.typed_call_builder(callee, ()).tail().name("r").build()?;
        b.build_ret(call.result())?;
        let text = format!("{m}");
        assert!(text.contains("%r = tail call i32 @g()"), "got:\n{text}");
        Ok(())
    })
}

/// Typed indirect call: the callee's function type is derived from the
/// `Sig` schema rather than spelled by hand, and the result narrows to
/// `IntValue<i32>` without a runtime `try_into`. Closest upstream
/// coverage: `unittests/IR/IRBuilderTest.cpp` opaque-pointer indirect
/// call construction (`IRBuilder::CreateCall(FunctionType*, Value*,
/// ...)`).
#[test]
fn typed_build_indirect_call_derives_function_type_from_schema() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let ptr_ty = m.ptr_type(0);
        let host_ty = m.fn_type(ptr_ty.as_type(), [ptr_ty.as_type()], false);
        let host = m.add_function_dyn("host", host_ty, Linkage::External)?;
        let entry = host.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let callee_ptr = llvmkit_ir::PointerValue::try_from(host.param(0).expect("callee ptr"))?;
        let x = m.i32_type().const_int(7_i32);
        let call = b.build_indirect_call::<fn(i32) -> i32, _, _>(callee_ptr, (x,), "r")?;
        let r = call.result();
        let text_ty = format!("{}", r.as_value().ty());
        assert_eq!(text_ty, "i32", "typed indirect call result must be i32");
        let text = format!("{m}");
        assert!(text.contains("%r = call i32 %0(i32 7)"), "got:\n{text}");
        Ok(())
    })
}

// --------------------------------------------------------------------------
// validate_call_site_args (D-numbers pending; ports `CallInst::init`'s
// "Calling a function with a bad signature!" assertion from
// `lib/IR/Instructions.cpp`, and `Verifier::visitCallBase`'s authoritative
// arity/type check, to build time for every dyn call/invoke/callbr path).
// --------------------------------------------------------------------------

/// A non-vararg callee called through `call_builder` with too few
/// arguments must fail at build time with `CallArgumentCountMismatch`,
/// not reach the verifier. Mirrors `CallInst::init`'s
/// `Args.size() == FTy->getNumParams()` assertion and
/// `Verifier::visitCallBase`'s `Call.arg_size() == FTy->getNumParams()`
/// check (`lib/IR/Instructions.cpp`, `lib/IR/Verifier.cpp`).
#[test]
fn call_builder_rejects_too_few_arguments() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let callee_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let err = b
            .call_builder(callee)
            .arg(x)
            .name("bad")
            .build()
            .expect_err("one argument against a two-parameter callee must be rejected");
        assert_eq!(
            err,
            IrError::CallArgumentCountMismatch {
                expected: 2,
                got: 1,
            }
        );
        let _ = b.build_ret(i32_ty.const_int(0_i32));
        Ok(())
    })
}

/// A non-vararg callee called through `call_builder` with an argument
/// whose type does not match the parameter at that position must fail
/// at build time with `CallArgumentTypeMismatch`. Mirrors the
/// `FTy->getParamType(i) == Args[i]->getType()` half of `CallInst::init`
/// and `Verifier::visitCallBase`'s `Call.getArgOperand(i)->getType() ==
/// FTy->getParamType(i)` check.
#[test]
fn call_builder_rejects_wrong_argument_type() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let callee_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [f32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: FloatValue<f32> = caller.param(0)?.try_into()?;
        let err = b
            .call_builder(callee)
            .arg(x)
            .name("bad")
            .build()
            .expect_err("an f32 argument against an i32 parameter must be rejected");
        assert_eq!(
            err,
            IrError::CallArgumentTypeMismatch {
                index: 0,
                expected: "i32".to_owned(),
                got: "float".to_owned(),
            }
        );
        let _ = b.build_ret(i32_ty.const_int(0_i32));
        Ok(())
    })
}

/// A vararg callee accepts more arguments than its fixed parameter
/// count without error -- `got > expected` is legal for vararg,
/// mirroring `Verifier::visitCallBase`'s `FTy->isVarArg()` branch
/// (`Call.arg_size() >= FTy->getNumParams()`).
#[test]
fn call_builder_accepts_extra_arguments_for_vararg_callee() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let callee = m
            .add_typed_varargs_function::<i32, (i32,), _>("callee", Linkage::External)?
            .as_function();
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let y: IntValue<i32> = caller.param(1)?.try_into()?;
        let inst = b.call_builder(callee).arg(x).arg(y).name("r").build()?;
        b.build_ret(inst.return_int_value())?;
        let text = format!("{m}");
        assert!(
            text.contains("%r = call i32 (i32, ...) @callee(i32 %0, i32 %1)"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// An indirect call through `build_indirect_call_dyn` with too many
/// arguments for a non-vararg function type must fail at build time
/// with `CallArgumentCountMismatch`, exercising the same
/// `validate_call_site_args` gate as the direct-callee path.
#[test]
fn indirect_call_rejects_too_many_arguments() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let host_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let host = m.add_function_dyn("host", host_ty, Linkage::External)?;
        let entry = host.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let callee_ptr = llvmkit_ir::PointerValue::try_from(host.param(0).expect("callee ptr"))?;
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let extra_arg = i32_ty.const_int(1_i32);
        let err = b
            .build_indirect_call_dyn::<(), _, _, _>(callee_ty, callee_ptr, [extra_arg], "bad")
            .expect_err("zero-parameter function type rejects a supplied argument");
        assert_eq!(
            err,
            IrError::CallArgumentCountMismatch {
                expected: 0,
                got: 1,
            }
        );
        b.build_ret_void()?;
        Ok(())
    })
}

// --------------------------------------------------------------------------
// TypedVarArgsFunctionValue + build_varargs_call
// --------------------------------------------------------------------------

/// `TypedFunctionValue::try_from_function` rejects a variadic raw
/// function up front -- the fixed-arity facade cannot represent a
/// `...` tail. Mirrors `FunctionType::isVarArg` gating the two facades
/// as mutually exclusive.
#[test]
fn fixed_arity_facade_rejects_variadic_function() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let raw = m
            .add_typed_varargs_function::<i32, (i32,), _>("printf_like", Linkage::External)?
            .as_function();
        let err = llvmkit_ir::TypedFunctionValue::<i32, (i32,), _>::try_from_function(raw)
            .expect_err("variadic signature must be rejected by the fixed-arity facade");
        assert_eq!(err, IrError::UnexpectedVarArgsSignature);
        Ok(())
    })
}

/// `TypedVarArgsFunctionValue::try_from_function` rejects a non-variadic
/// raw function -- the varargs facade requires an actual `...` tail.
#[test]
fn varargs_facade_rejects_non_variadic_function() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let raw = m
            .add_typed_function::<i32, (i32,), _>("plain", Linkage::External)?
            .as_function();
        let err = llvmkit_ir::TypedVarArgsFunctionValue::<i32, (i32,), _>::try_from_function(raw)
            .expect_err("non-variadic signature must be rejected by the varargs facade");
        assert_eq!(err, IrError::MissingVarArgsSignature);
        Ok(())
    })
}

/// `build_varargs_call` lowers the fixed prefix through `CallArgs`
/// exactly like `build_call`, then appends the erased varargs tail
/// unchecked -- matching LLVM's own variadic-argument contract (no
/// static or verifier type checking on the `...` operands). Mirrors
/// `IRBuilder::CreateCall` against a variadic `FunctionCallee`, closest
/// upstream fixture `test/Assembler/varargs.ll`-style `(...)` printing.
#[test]
fn build_varargs_call_lowers_fixed_prefix_and_appends_erased_tail() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let callee =
            m.add_typed_varargs_function::<i32, (i32,), _>("sum_varargs", Linkage::External)?;
        let caller = m.add_typed_function::<i32, (i32,), _>("caller", Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (count,) = caller.params();
        let extra_a = i32_ty.const_int(10_i32);
        let extra_b = m.f32_type().const_float(2.5_f32);
        let call = b.build_varargs_call(
            callee,
            (count,),
            [extra_a.as_value(), extra_b.as_value()],
            "r",
        )?;
        b.build_ret(call.result())?;
        let text = format!("{m}");
        assert!(
            text.contains(
                "%r = call i32 (i32, ...) @sum_varargs(i32 %0, i32 10, float 2.500000e+00)"
            ),
            "got:\n{text}"
        );
        Ok(())
    })
}
