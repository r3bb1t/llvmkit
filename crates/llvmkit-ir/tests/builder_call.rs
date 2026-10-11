//! `call` instruction print form and call-builder ergonomics.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` carries a citation naming the upstream
//! `unittests/IR/InstructionsTest.cpp` `TEST` (or `test/Assembler/*.ll`
//! fixture) it ports.

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, AttributeStorage, BasicBlock, CallAttributeData, CallBase,
    CallInst, CallSiteConfig, CallingConv, DetachedCallSite, Dyn, DynBrand, FastMathFlags,
    FloatValue, InlineAsmOptions, IntValue, IntrinsicDescriptor, IntrinsicId, InvokeInst,
    IrBuilder, IrError, IrResult, Linkage, MetadataAttachmentKind, MetadataId, Module, ModuleBrand,
    OperandBundleDef, OperandBundleTag, OperandBundleUse, Ptr, Unverified,
    instr_types::TailCallKind, module_new,
};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// — exercises construction of a non-void `CallInst` against a declared
/// callee.
#[test]
fn call_int_returning_function() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    // declare i32 @callee(i32, i32)
    let callee = m
        .add_typed_function::<i32, (i32, i32), _>("callee", Linkage::External)?
        .as_function();
    // define i32 @caller(i32 %x, i32 %y) { %r = call i32 @callee(i32 %x, i32 %y); ret i32 %r }
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: llvmkit_ir::IntValue<'_, i32, _> = m.view(caller).param(0)?.try_into()?;
    let y: llvmkit_ir::IntValue<'_, i32, _> = m.view(caller).param(1)?.try_into()?;
    let inst = b.call_dyn(callee, [x.as_erased(), y.as_erased()], "r")?;
    // Typed return accessor (Doctrine D4): `R` flows from the callee
    // through `call_dyn` into `CallInst<'ctx, i32>`, which directly
    // exposes `return_int_value(): IntValue<i32>` -- no runtime
    // `try_into` is needed.
    let ret_val = b.view(inst).return_int_value();
    b.ret(ret_val)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = call i32 @callee(i32 %0, i32 %1)"),
        "got:\n{text}"
    );
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// applied to a `void`-returning callee; the C++ test creates calls
/// against a `FunctionType::get(...)` declaration just like this Rust
/// counterpart.
#[test]
fn call_void_returning_function() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let void_ty = m.void_type();
    // declare void @sink()
    let callee_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let callee = m.add_function_dyn("sink", callee_ty, Linkage::External)?;
    // define void @caller() { call void @sink(); ret void }
    let caller_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let inst = b.call_dyn(callee, Vec::<llvmkit_ir::Value<'_, _>>::new(), "")?;
    assert!(b.view(inst).return_value().is_none());
    b.ret_void()?;
    let text = format!("{m}");
    assert!(text.contains("call void @sink()"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: covers the `call_builder` fluent API mixing an
/// `IntValue<i32>` and a `PointerValue` argument. Closest upstream
/// functional coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CloneCall)`
/// (constructs a `CallInst` with mixed-type args).
#[test]
fn call_builder_mixed_arg_types() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let callee_ty = m.function_type(void_ty.as_type(), [i32_ty.as_type(), ptr_ty.as_type()]);
    let callee = m.add_function_dyn("with_ptr", callee_ty, Linkage::External)?;
    let caller_ty = m.function_type(void_ty.as_type(), [ptr_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(caller).param(0)?.try_into()?;
    // Mixed-type args: an `IntValue<i32>` and a `PointerValue` go into
    // the same call. The builder pattern accepts each via a
    // monomorphised `arg<V: IsValue>` call.
    let answer = m.i32_type().const_int(42_i32);
    b.call_builder(m.view(callee)).arg(answer).arg(p).build()?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("call void @with_ptr(i32 42, ptr %0)"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors the `tail call` textual form locked by
/// `test/Assembler/call-arg-is-callee.ll` and friends in
/// `test/Assembler/`. Closest upstream functional coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`.
#[test]
fn call_tail() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let callee = m
        .add_typed_function::<i32, (), _>("g", Linkage::External)?
        .as_function();
    let caller_ty = m.function_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let caller = m.add_function_dyn("f", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let inst = b.call_builder(m.view(callee)).tail().name("r").build()?;
    let r = b.view(inst).return_int_value();
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = tail call i32 @g()"), "got:\n{text}");
    Ok(())
}

/// Mirrors `Intrinsic::getOrInsertDeclaration` plus `IRBuilder::CreateCall`:
/// an intrinsic call helper inserts the canonical declaration and emits a
/// direct call to it.
#[test]
fn intrinsic_call_inserts_declaration_and_emits_direct_call() -> Result<(), IrError> {
    let m = module_new!("intrinsic-call")?;
    let f32_ty = m.f32_type();
    let caller_ty = m.function_type(f32_ty, [f32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: FloatValue<'_, f32, _> = m.view(caller).param(0)?.try_into()?;
    let descriptor = IntrinsicDescriptor::new(
        IntrinsicId::lookup("llvm.acos.f32").expect("acos intrinsic"),
        [f32_ty.as_type()],
    )?;
    let call = b.intrinsic_call(&descriptor, &[x.as_erased()], "r")?;
    let r: FloatValue<'_, f32, _> = b
        .view(call)
        .return_value()
        .ok_or(IrError::InvalidOperation {
            message: "non-void intrinsic result",
        })?
        .try_into()?;
    b.ret(r)?;
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
}
/// Mirrors `Intrinsic::getOrInsertDeclaration` plus `IRBuilder::CreateCall`:
/// descriptor-backed intrinsic builders reject operands that do not match the
/// generated IIT signature.
#[test]
fn intrinsic_call_rejects_wrong_argument_type() -> Result<(), IrError> {
    let m = module_new!("intrinsic-call-mismatch")?;
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let caller_ty = m.function_type(m.void_type().as_type(), [i32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(caller).param(0)?.try_into()?;
    let descriptor = IntrinsicDescriptor::new(
        IntrinsicId::lookup("llvm.acos.f32").expect("acos intrinsic"),
        [f32_ty.as_type()],
    )?;
    let err = b
        .intrinsic_call(&descriptor, &[x.as_erased()], "bad")
        .expect_err("i32 argument should not match llvm.acos.f32");
    assert!(matches!(
        err,
        IrError::IntrinsicSignatureMismatch { name } if name == "llvm.acos.f32"
    ));
    let _ = b.ret_void();
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// specialized for a pointer-returning callee.
#[test]
fn call_to_pointer_returning_function() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let ptr_ty = m.ptr_type(0);
    let callee = m
        .add_typed_function::<Ptr, (), _>("alloc_ptr", Linkage::External)?
        .as_function();
    let caller_ty = m.function_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let caller = m.add_function_dyn("g", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let inst = b.call_dyn(callee, Vec::<llvmkit_ir::Value<'_, _>>::new(), "p")?;
    let p = b.view(inst).return_pointer_value();
    b.ret(p)?;
    let text = format!("{m}");
    assert!(text.contains("%p = call ptr @alloc_ptr()"), "got:\n{text}");
    Ok(())
}

// --------------------------------------------------------------------------
// Typed call / call_with_config / typed_call_builder
// --------------------------------------------------------------------------

/// The typed `call` prints identically to the dyn form for the
/// same signature, and its result narrows to `IntValue<i32>` without a
/// runtime `try_into`. Closest upstream coverage: same
/// `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest,
/// CallInst)` shape as `call_int_returning_function`, exercised through
/// the typed callee facade instead of a raw `FunctionValue`.
#[test]
fn typed_build_call_prints_like_dyn_form() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let callee = m.add_typed_function::<i32, (i32, i32), _>("callee", Linkage::External)?;
    let caller = m.add_typed_function::<i32, (i32, i32), _>("caller", Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let (x, y) = m.view(caller).params();
    let call = b.call(callee, (x, y), "r")?;
    let ret_val = b.view(call).result();
    b.ret(ret_val)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = call i32 @callee(i32 %0, i32 %1)"),
        "got:\n{text}"
    );
    Ok(())
}

/// `call_with_config` threads a non-default calling convention
/// into the emitted typed call, mirroring `call_tail`'s dyn-path
/// coverage of `CallSiteConfig`.
#[test]
fn typed_build_call_with_config_threads_calling_convention() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let callee = m.add_typed_function::<i32, (), _>("g", Linkage::External)?;
    m.view(callee)
        .as_function()
        .set_calling_conv(&m, CallingConv::FAST);
    let caller = m.add_typed_function::<i32, (), _>("f", Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let call = b.call_with_config(
        callee,
        (),
        llvmkit_ir::CallSiteConfig::new("r").calling_conv(CallingConv::FAST),
    )?;
    let ret_val = b.view(call).result();
    b.ret(ret_val)?;
    let text = format!("{m}");
    assert!(text.contains("%r = call fastcc i32 @g()"), "got:\n{text}");
    Ok(())
}

/// `typed_call_builder` chains `.tail()` the same way the dyn
/// `call_builder` does, mirroring `call_tail`.
#[test]
fn typed_call_builder_chains_tail() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let callee = m.add_typed_function::<i32, (), _>("g", Linkage::External)?;
    let caller = m.add_typed_function::<i32, (), _>("f", Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let call = b
        .typed_call_builder(m.view(callee), ())
        .tail()
        .name("r")
        .build()?;
    let ret_val = b.view(call).result();
    b.ret(ret_val)?;
    let text = format!("{m}");
    assert!(text.contains("%r = tail call i32 @g()"), "got:\n{text}");
    Ok(())
}

/// Typed indirect call: the callee's function type is derived from the
/// `Sig` schema rather than spelled by hand, and the result narrows to
/// `IntValue<i32>` without a runtime `try_into`. Closest upstream
/// coverage: `unittests/IR/IRBuilderTest.cpp` opaque-pointer indirect
/// call construction (`IRBuilder::CreateCall(FunctionType*, Value*,
/// ...)`).
#[test]
fn typed_build_indirect_call_derives_function_type_from_schema() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let ptr_ty = m.ptr_type(0);
    let host_ty = m.function_type(ptr_ty.as_type(), [ptr_ty.as_type()]);
    let host = m.add_function_dyn("host", host_ty, Linkage::External)?;
    let entry = m.view(host).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let callee_ptr =
        llvmkit_ir::PointerValue::try_from(m.view(host).param(0).expect("callee ptr"))?;
    let x = m.i32_type().const_int(7_i32);
    let call = b.indirect_call::<fn(i32) -> i32, _, _, _>(callee_ptr, (x,), "r")?;
    let r = b.view(call).result();
    let text_ty = format!("{}", r.as_erased().ty());
    assert_eq!(text_ty, "i32", "typed indirect call result must be i32");
    let text = format!("{m}");
    assert!(text.contains("%r = call i32 %0(i32 7)"), "got:\n{text}");
    Ok(())
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
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let callee_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(caller).param(0)?.try_into()?;
    let err = b
        .call_builder(m.view(callee))
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
    let _ = b.ret(i32_ty.const_int(0_i32));
    Ok(())
}

/// A non-vararg callee called through `call_builder` with an argument
/// whose type does not match the parameter at that position must fail
/// at build time with `CallArgumentTypeMismatch`. Mirrors the
/// `FTy->getParamType(i) == Args[i]->getType()` half of `CallInst::init`
/// and `Verifier::visitCallBase`'s `Call.getArgOperand(i)->getType() ==
/// FTy->getParamType(i)` check.
#[test]
fn call_builder_rejects_wrong_argument_type() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let callee_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.function_type(i32_ty, [f32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: FloatValue<'_, f32, _> = m.view(caller).param(0)?.try_into()?;
    let err = b
        .call_builder(m.view(callee))
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
    let _ = b.ret(i32_ty.const_int(0_i32));
    Ok(())
}

/// A vararg callee accepts more arguments than its fixed parameter
/// count without error -- `got > expected` is legal for vararg,
/// mirroring `Verifier::visitCallBase`'s `FTy->isVarArg()` branch
/// (`Call.arg_size() >= FTy->getNumParams()`).
#[test]
fn call_builder_accepts_extra_arguments_for_vararg_callee() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let callee = m
        .add_typed_varargs_function::<i32, (i32,), _>("callee", Linkage::External)?
        .as_function();
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(caller).param(0)?.try_into()?;
    let y: IntValue<'_, i32, _> = m.view(caller).param(1)?.try_into()?;
    let inst = b
        .call_builder(m.view(callee))
        .arg(x)
        .arg(y)
        .name("r")
        .build()?;
    let ret_val = b.view(inst).return_int_value();
    b.ret(ret_val)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = call i32 (i32, ...) @callee(i32 %0, i32 %1)"),
        "got:\n{text}"
    );
    Ok(())
}

/// An indirect call through `indirect_call_dyn` with too many
/// arguments for a non-vararg function type must fail at build time
/// with `CallArgumentCountMismatch`, exercising the same
/// `validate_call_site_args` gate as the direct-callee path.
#[test]
fn indirect_call_rejects_too_many_arguments() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let host_ty = m.function_type(void_ty.as_type(), [ptr_ty.as_type()]);
    let host = m.add_function_dyn("host", host_ty, Linkage::External)?;
    let entry = m.view(host).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let callee_ptr =
        llvmkit_ir::PointerValue::try_from(m.view(host).param(0).expect("callee ptr"))?;
    let callee_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let extra_arg = i32_ty.const_int(1_i32);
    let err = b
        .indirect_call_dyn::<(), _, _, _, _>(callee_ty, callee_ptr, [extra_arg], "bad")
        .expect_err("zero-parameter function type rejects a supplied argument");
    assert_eq!(
        err,
        IrError::CallArgumentCountMismatch {
            expected: 0,
            got: 1,
        }
    );
    b.ret_void()?;
    Ok(())
}

// --------------------------------------------------------------------------
// TypedVarArgsFunctionValue + varargs_call
// --------------------------------------------------------------------------

/// `TypedFunctionValue::try_from_function` rejects a variadic raw
/// function up front -- the fixed-arity facade cannot represent a
/// `...` tail. Mirrors `FunctionType::isVarArg` gating the two facades
/// as mutually exclusive.
#[test]
fn fixed_arity_facade_rejects_variadic_function() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let raw = m
        .add_typed_varargs_function::<i32, (i32,), _>("printf_like", Linkage::External)?
        .as_function();
    let err = llvmkit_ir::TypedFunctionValue::<i32, (i32,), _>::try_from_function(m.view(raw))
        .expect_err("variadic signature must be rejected by the fixed-arity facade");
    assert_eq!(err, IrError::UnexpectedVarArgsSignature);
    Ok(())
}

/// `TypedVarArgsFunctionValue::try_from_function` rejects a non-variadic
/// raw function -- the varargs facade requires an actual `...` tail.
#[test]
fn varargs_facade_rejects_non_variadic_function() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let raw = m
        .add_typed_function::<i32, (i32,), _>("plain", Linkage::External)?
        .as_function();
    let err =
        llvmkit_ir::TypedVarArgsFunctionValue::<i32, (i32,), _>::try_from_function(m.view(raw))
            .expect_err("non-variadic signature must be rejected by the varargs facade");
    assert_eq!(err, IrError::MissingVarArgsSignature);
    Ok(())
}

/// `varargs_call` lowers the fixed prefix through `CallArgs`
/// exactly like `call`, then appends the erased varargs tail
/// unchecked -- matching LLVM's own variadic-argument contract (no
/// static or verifier type checking on the `...` operands). Mirrors
/// `IRBuilder::CreateCall` against a variadic `FunctionCallee`, closest
/// upstream fixture `test/Assembler/varargs.ll`-style `(...)` printing.
#[test]
fn build_varargs_call_lowers_fixed_prefix_and_appends_erased_tail() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let callee =
        m.add_typed_varargs_function::<i32, (i32,), _>("sum_varargs", Linkage::External)?;
    let caller = m.add_typed_function::<i32, (i32,), _>("caller", Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let (count,) = m.view(caller).params();
    let extra_a = i32_ty.const_int(10_i32);
    let extra_b = m.f32_type().const_float(2.5_f32);
    let call = b.varargs_call(
        m.view(callee),
        (count,),
        [extra_a.as_erased(), extra_b.as_erased()],
        "r",
    )?;
    let ret_val = b.view(call).result();
    b.ret(ret_val)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = call i32 (i32, ...) @sum_varargs(i32 %0, i32 10, float 2.500000e+00)"),
        "got:\n{text}"
    );
    Ok(())
}

/// `IrBuilder::call_erased` is the port of
/// `IRBuilder::CreateCall(FunctionType *FTy, Value *Callee, ArrayRef<Value *> Args,
/// ArrayRef<OperandBundleDef> OpBundles, const Twine &Name)`: the callee is a
/// bare `Value` and the call site carries its own `FunctionType`. **No upstream
/// unit-test counterpart** -- `unittests/IR/IRBuilderTest.cpp` exercises the
/// typed `FunctionCallee` overloads only; the anchor is that overload plus
/// `CallInst::Create(Ty, Callee, Args, BundleList)` and the
/// `setTailCallKind` / `setCallingConv` / `setAttributes` triple
/// `LLParser::parseCall` runs after it.
///
/// The law under test is that one construction accepts a named function, an
/// inline-asm value and a function pointer, and that the call-site
/// configuration survives on all three -- which is what makes
/// `LLParser::parseCall`'s single tail expressible.
#[test]
fn call_erased_carries_the_call_site_configuration_for_every_callee_shape() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let callee_ty = m.function_type(void_ty.as_type(), [i32_ty.as_type()]);

    // declare void @g(i32)
    let g = m.add_function_dyn("g", callee_ty, Linkage::External)?;
    // The same signature, spelled as inline asm.
    let asm = m.inline_asm(callee_ty, "nop", "r", InlineAsmOptions::new());

    // define void @caller(ptr %fp, i32 %v)
    let caller_ty = m.function_type(void_ty.as_type(), [ptr_ty.as_type(), i32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let fp = llvmkit_ir::PointerValue::try_from(m.view(caller).param(0)?)?;
    let v = m.view(caller).param(1)?;

    // One configuration, reused verbatim on all three callee shapes: `fastcc`
    // (`setCallingConv`) plus a `noundef` on the single argument
    // (`setAttributes`), with `tail` (`setTailCallKind`) passed alongside.
    let mut arg_attr = AttributeStorage::new();
    arg_attr.add(
        AttrIndex::Param(0),
        Attribute::<DynBrand>::enum_attr(AttrKind::NoUndef).expect("noundef is enum"),
    );
    let config = || {
        CallSiteConfig::new("")
            .calling_conv(CallingConv::FAST)
            .attrs(CallAttributeData::new(
                AttributeStorage::new(),
                Box::new([arg_attr.clone()]),
                AttributeStorage::new(),
            ))
    };

    b.call_erased::<Dyn, _, _>(
        callee_ty,
        m.view(g).as_erased(),
        [v],
        TailCallKind::Tail,
        FastMathFlags::empty(),
        config(),
    )?;
    b.call_erased::<Dyn, _, _>(
        callee_ty,
        asm.as_erased(),
        [v],
        TailCallKind::Tail,
        FastMathFlags::empty(),
        config(),
    )?;
    b.call_erased::<Dyn, _, _>(
        callee_ty,
        fp.as_erased(),
        [v],
        TailCallKind::Tail,
        FastMathFlags::empty(),
        config(),
    )?;
    b.ret_void()?;

    let text = format!("{m}");
    for expected in [
        "tail call fastcc void @g(i32 noundef %1)",
        r#"tail call fastcc void asm "nop", "r"(i32 noundef %1)"#,
        "tail call fastcc void %0(i32 noundef %1)",
    ] {
        assert!(
            text.contains(expected),
            "missing `{expected}`; got:\n{text}"
        );
    }
    Ok(())
}

/// `IrBuilder::call_erased` honours a [`CallSiteConfig::call_site_type`]
/// override in preference to its positional `fn_ty`, mirroring `CallBase`'s own
/// `FunctionType` — which `LLParser::parseCall` relies on when it spells a call
/// through a type that differs from the callee's declaration. **No upstream
/// unit-test counterpart**: upstream has no `CallSiteConfig`, and the rule it
/// stands for is `CallInst::Create(FunctionType *Ty, Value *Func, ...)` taking
/// the type as an argument rather than reading it off `Func`.
///
/// The override is chosen so that dropping it cannot pass silently: the
/// positional type takes no parameters and returns `void`, the override takes
/// one `i32` and returns `i32`, and one argument is supplied. Without the
/// override branch this fails in `validate_call_site_args` with
/// `CallArgumentCountMismatch` rather than printing a differently-typed call.
#[test]
fn call_erased_prefers_the_call_site_type_override() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();

    // declare i32 @g(i32) -- the shape the call site is spelled through.
    let declared = m.function_type(i32_ty.as_type(), [i32_ty.as_type()]);
    let g = m.add_function_dyn("g", declared, Linkage::External)?;
    // The positional `fn_ty` disagrees with it in both arity and return type.
    let positional = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());

    let caller_ty = m.function_type(void_ty.as_type(), [i32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v = m.view(caller).param(0)?;

    b.call_erased::<Dyn, _, _>(
        positional,
        m.view(g).as_erased(),
        [v],
        TailCallKind::None,
        FastMathFlags::empty(),
        CallSiteConfig::new("r").call_site_type(declared),
    )?;
    b.ret_void()?;

    let text = format!("{m}");
    assert!(
        text.contains("%r = call i32 @g(i32 %0)"),
        "the override should decide both the call-site arity and the result type; got:\n{text}"
    );
    Ok(())
}

/// `IrBuilder::call_erased` takes a call's fast-math flags — the
/// `CallInst::setFastMathFlags` that `LLParser::parseCall` runs on the call it
/// just built — and refuses them on a call whose return type is not
/// floating-point, where upstream's `Instruction::setFastMathFlags` asserts
/// `isa<FPMathOperator>(this)` ("setting fast-math flag on invalid op"). The
/// refusal carries the sentence `LLParser::parseCall` reports for the same
/// fault, verbatim, so the builder and the parser cannot disagree.
///
/// **llvmkit-specific**: upstream's builder answer is an assertion, so no
/// upstream test reaches it. The positive control is the same flags on a
/// `float` call, printed where `writeOptimizationInfo` puts them, straight
/// after `call`; the refusal leaves the module as it was.
#[test]
fn call_erased_refuses_fast_math_flags_on_a_call_that_is_not_floating_point() -> Result<(), IrError>
{
    let m = module_new!("c")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let int_fn_ty = m.function_type(i32_ty.as_type(), [i32_ty.as_type()]);
    let float_fn_ty = m.function_type(f32_ty.as_type(), [f32_ty.as_type()]);
    let g = m.add_function_dyn("g", int_fn_ty, Linkage::External)?;
    let h = m.add_function_dyn("h", float_fn_ty, Linkage::External)?;

    let caller_ty = m.function_type(void_ty.as_type(), [i32_ty.as_type(), f32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let int_arg = m.view(caller).param(0)?;
    let float_arg = m.view(caller).param(1)?;

    let printed_before = format!("{m}");
    let refused = b.call_erased::<Dyn, _, _>(
        int_fn_ty,
        m.view(g).as_erased(),
        [int_arg],
        TailCallKind::None,
        FastMathFlags::fast(),
        CallSiteConfig::new("r"),
    );
    assert!(
        matches!(
            refused,
            Err(IrError::InvalidOperation {
                message: "fast-math-flags specified for call without floating-point scalar or vector return type"
            })
        ),
        "fast-math flags on an `i32` call must be refused; got {refused:?}"
    );
    assert_eq!(
        format!("{m}"),
        printed_before,
        "a refused call must leave the module unchanged"
    );

    b.call_erased::<Dyn, _, _>(
        float_fn_ty,
        m.view(h).as_erased(),
        [float_arg],
        TailCallKind::None,
        FastMathFlags::fast(),
        CallSiteConfig::new("y"),
    )?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("%y = call fast float @h(float %1)"),
        "the flags belong on a `float` call; got:\n{text}"
    );
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
/// AlterCallBundles)`, whole: a call created in no block is copied with its
/// operand bundles replaced, and the copy keeps everything else.
///
/// The spellings, one per upstream statement: `LLVMContext C` is a module,
/// since llvmkit's values live in one; `CallInst::Create(FnTy, Callee, Args,
/// OldBundle, "result")` is [`CallInst::create_detached`], whose
/// `CallSiteConfig` carries the bundle and the name; `AttributeList::get(C,
/// AttributeList::FunctionIndex, AB)` is a [`CallAttributeData`] whose
/// function attributes hold `cold`; `setDebugLoc(DebugLoc(MDNode::get(C,
/// {})))` sets the `!dbg` attachment, which is an instruction's debug location
/// in llvmkit; `CallInst::Create(Call.get(), NewBundle)` is
/// [`CallInst::with_operand_bundles`]; the tags `"before"` and `"after"` name
/// no bundle LLVM knows, so they are [`OperandBundleTag::Custom`]; and the two
/// `unique_ptr`s' destruction is [`Instruction::drop_detached`](llvmkit_ir::Instruction::drop_detached),
/// newest first.
#[test]
fn alter_call_bundles() -> Result<(), IrError> {
    let m = module_new!("C")?;
    let int32_ty = m.i32_type();
    let fn_ty = m.function_type(int32_ty.as_type(), [int32_ty.as_type()]);
    let callee = m.ptr_type(0).const_null();
    let args = [int32_ty.const_int(42_i32)];
    let old_bundle = OperandBundleDef::new(
        OperandBundleTag::Custom("before".to_owned()),
        [int32_ty.as_type().undef()],
    );
    let (call_instruction, call) = CallInst::<Dyn, _>::create_detached(
        &m,
        fn_ty,
        callee.as_erased(),
        args,
        CallSiteConfig::new("result").operand_bundles([old_bundle]),
    )?;
    call.set_tail_call_kind(&m, TailCallKind::NoTail);
    let mut ab = AttributeStorage::new();
    ab.add(
        AttrIndex::Function,
        Attribute::<DynBrand>::enum_attr(AttrKind::Cold).expect("cold is an enum attribute"),
    );
    call.set_attributes(
        &m,
        CallAttributeData::new(AttributeStorage::new(), Box::new([]), ab),
    );
    call.as_view().set_metadata(
        &m,
        MetadataAttachmentKind::Dbg,
        m.metadata_tuple(Vec::<MetadataId<_>>::new())?,
    )?;

    let new_bundle = OperandBundleDef::new(
        OperandBundleTag::Custom("after".to_owned()),
        [int32_ty.const_int(7_i32)],
    );
    let (clone_instruction, clone) = call.with_operand_bundles(&m, [new_bundle])?;
    assert_eq!(call.args().len(), clone.args().len());
    assert_eq!(call.args().next(), clone.args().next());
    assert_eq!(call.calling_conv(), clone.calling_conv());
    assert_eq!(call.tail_call_kind(), clone.tail_call_kind());
    assert!(clone.has_fn_attr(AttrKind::Cold));
    assert_eq!(
        call.as_view().metadata().get(&MetadataAttachmentKind::Dbg),
        clone.as_view().metadata().get(&MetadataAttachmentKind::Dbg)
    );
    assert_eq!(clone.operand_bundles().len(), 1);
    assert!(
        clone
            .operand_bundle(&OperandBundleTag::Custom("after".to_owned()))?
            .is_some()
    );

    clone_instruction.drop_detached(&m);
    call_instruction.drop_detached(&m);
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
/// AlterInvokeBundles)`, whole: the `invoke` twin of [`alter_call_bundles`],
/// whose destinations are two blocks in no function.
///
/// Spelled as that port spells its statements, plus: `BasicBlock::Create(C)`
/// is [`BasicBlock::create_orphan`] with an empty name; `InvokeInst::Create(FnTy,
/// Callee, NormalDest.get(), UnwindDest.get(), Args, OldBundle, "result")` is
/// [`InvokeInst::create_detached`]; `InvokeInst::Create(Invoke.get(),
/// NewBundle)` is [`InvokeInst::with_operand_bundles`]; and `getNormalDest` /
/// `getUnwindDest` are the destinations' [`BlockId`](llvmkit_ir::BlockId)s.
/// The two blocks' `unique_ptr`s have no counterpart to run: an orphan block
/// stays in its module's arena, as a dropped detached instruction does.
#[test]
fn alter_invoke_bundles() -> Result<(), IrError> {
    let m = module_new!("C")?;
    let int32_ty = m.i32_type();
    let fn_ty = m.function_type(int32_ty.as_type(), [int32_ty.as_type()]);
    let callee = m.ptr_type(0).const_null();
    let args = [int32_ty.const_int(42_i32)];
    let normal_dest = BasicBlock::create_orphan(&m, "");
    let unwind_dest = BasicBlock::create_orphan(&m, "");
    let old_bundle = OperandBundleDef::new(
        OperandBundleTag::Custom("before".to_owned()),
        [int32_ty.as_type().undef()],
    );
    let (invoke_instruction, invoke) = InvokeInst::<Dyn, _>::create_detached(
        &m,
        fn_ty,
        callee.as_erased(),
        &normal_dest,
        &unwind_dest,
        args,
        CallSiteConfig::new("result").operand_bundles([old_bundle]),
    )?;
    let mut ab = AttributeStorage::new();
    ab.add(
        AttrIndex::Function,
        Attribute::<DynBrand>::enum_attr(AttrKind::Cold).expect("cold is an enum attribute"),
    );
    invoke.set_attributes(
        &m,
        CallAttributeData::new(AttributeStorage::new(), Box::new([]), ab),
    );
    invoke.as_view().set_metadata(
        &m,
        MetadataAttachmentKind::Dbg,
        m.metadata_tuple(Vec::<MetadataId<_>>::new())?,
    )?;

    let new_bundle = OperandBundleDef::new(
        OperandBundleTag::Custom("after".to_owned()),
        [int32_ty.const_int(7_i32)],
    );
    let (clone_instruction, clone) = invoke.with_operand_bundles(&m, [new_bundle])?;
    assert_eq!(invoke.normal_destination(), clone.normal_destination());
    assert_eq!(invoke.unwind_destination(), clone.unwind_destination());
    assert_eq!(invoke.args().len(), clone.args().len());
    assert_eq!(invoke.args().next(), clone.args().next());
    assert_eq!(invoke.calling_conv(), clone.calling_conv());
    assert!(clone.has_fn_attr(AttrKind::Cold));
    assert_eq!(
        invoke
            .as_view()
            .metadata()
            .get(&MetadataAttachmentKind::Dbg),
        clone.as_view().metadata().get(&MetadataAttachmentKind::Dbg)
    );
    assert_eq!(clone.operand_bundles().len(), 1);
    assert!(
        clone
            .operand_bundle(&OperandBundleTag::Custom("after".to_owned()))?
            .is_some()
    );

    clone_instruction.drop_detached(&m);
    invoke_instruction.drop_detached(&m);
    Ok(())
}

/// `CallInst::set_tail_call_kind` and the port of `CallBase::setAttributes`
/// replace what they name and nothing else. `setAttributes` swaps the call's
/// `AttributeList`; a call's fast-math flags (`SubclassOptionalData`) and
/// operand bundles (operands) are not part of that list, so a call carrying
/// both keeps both. [`alter_call_bundles`] reads the tail-call kind only as
/// `Call == Clone`, which a setter that did nothing would pass, so the setter
/// is pinned here directly.
///
/// **llvmkit-specific**: no upstream unit test calls either setter on its
/// own. The positive control is the call as built — flags, bundle, no `cold`,
/// no marker — read before either setter runs.
#[test]
fn call_setters_replace_what_they_name_and_nothing_else() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let float_fn_ty = m.function_type(f32_ty.as_type(), [f32_ty.as_type()]);
    let h = m.add_function_dyn("h", float_fn_ty, Linkage::External)?;
    let caller_ty = m.function_type(void_ty.as_type(), [f32_ty.as_type()]);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x = m.view(caller).param(0)?;
    let id = b.call_erased::<Dyn, _, _>(
        float_fn_ty,
        m.view(h).as_erased(),
        [x],
        TailCallKind::None,
        FastMathFlags::fast(),
        CallSiteConfig::new("y").operand_bundles([OperandBundleDef::new(
            OperandBundleTag::Deopt,
            [i32_ty.const_int(7_i32)],
        )]),
    )?;
    b.ret_void()?;
    let call = m.view(id);
    let built = format!("{m}");
    assert!(
        built.contains(r#"%y = call fast float @h(float %0) [ "deopt"(i32 7) ]"#),
        "{built}"
    );
    assert_eq!(call.tail_call_kind(), TailCallKind::None);
    assert!(!call.has_fn_attr(AttrKind::Cold));

    call.set_tail_call_kind(&m, TailCallKind::NoTail);
    assert_eq!(call.tail_call_kind(), TailCallKind::NoTail);
    let mut cold = AttributeStorage::new();
    cold.add(
        AttrIndex::Function,
        Attribute::<DynBrand>::enum_attr(AttrKind::Cold).expect("cold is an enum attribute"),
    );
    call.set_attributes(
        &m,
        CallAttributeData::new(AttributeStorage::new(), Box::new([]), cold),
    );
    assert!(call.has_fn_attr(AttrKind::Cold));

    let text = format!("{m}");
    assert!(
        text.contains("%y = notail call fast float @h(float %0)"),
        "the marker is replaced and the flags kept:\n{text}"
    );
    assert!(
        text.contains(r#"[ "deopt"(i32 7) ]"#),
        "the bundle is kept:\n{text}"
    );
    Ok(())
}

/// A copy of a call site, through the one interface upstream's
/// `CallBase::Create(CallBase *CB, ArrayRef<OperandBundleDef> Bundles,
/// InsertPosition)` is: a switch on the opcode to the three per-class copies,
/// which [`CallBase`] is, so one generic function copies a `call`, an
/// `invoke` and a `callbr`. The same interface's `setAttributes` gives each
/// original `cold` first (absent before, as the control reads). Each copy
/// keeps its original's operands, destinations, calling convention,
/// attributes and debug location and carries only the new bundle. The
/// `callbr` copy keeps the default and indirect destinations (upstream's
/// `NumIndirectDests`), which no upstream unit test reads.
///
/// The `call` copy is then inserted after its original. It joins the
/// function's name table, which uniques its name the way
/// `ValueSymbolTable::makeUniqueName` does, and prints with the new bundle
/// while the original keeps its own. The module is not verified: its `callbr`
/// names a function rather than inline assembly, which the verifier refuses,
/// and the copy's fields are what is under test.
///
/// **llvmkit-specific**: upstream's tests reach the `call` and `invoke` copies
/// only through their per-class `Create`, and never the `callbr` one.
#[test]
fn call_base_copies_each_call_site_with_other_bundles() -> Result<(), IrError> {
    fn with_bundle<'ctx, C, B>(
        module: &'ctx Module<B, Unverified>,
        site: C,
        bundle: OperandBundleDef<'ctx, B>,
    ) -> IrResult<DetachedCallSite<'ctx, C, B>>
    where
        C: CallBase<'ctx, B>,
        B: ModuleBrand + 'ctx,
    {
        site.with_operand_bundles(module, [bundle])
    }
    fn make_cold<'ctx, C, B>(module: &'ctx Module<B, Unverified>, site: C)
    where
        C: CallBase<'ctx, B>,
        B: ModuleBrand + 'ctx,
    {
        let mut cold = AttributeStorage::new();
        cold.add(
            AttrIndex::Function,
            Attribute::<DynBrand>::enum_attr(AttrKind::Cold).expect("cold is an enum attribute"),
        );
        site.set_attributes(
            module,
            CallAttributeData::new(AttributeStorage::new(), Box::new([]), cold),
        );
    }

    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty.as_type(), [i32_ty.as_type()]);
    let h = m.add_function_dyn("h", fn_ty, Linkage::External)?;
    let deopt = |k: i32| OperandBundleDef::new(OperandBundleTag::Deopt, [i32_ty.const_int(k)]);
    let dbg = m.metadata_tuple(Vec::<MetadataId<_>>::new())?;

    // define i32 @caller(i32 %x): a call, then an invoke and a callbr in
    // blocks of their own.
    let caller = m.add_function_dyn("caller", fn_ty, Linkage::External)?;
    let x = m.view(caller).param(0)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let invoking = m.view(caller).append_basic_block(&m, "invoking");
    let jumping = m.view(caller).append_basic_block(&m, "jumping");
    let normal = m.view(caller).append_basic_block(&m, "normal");
    let unwind = m.view(caller).append_basic_block(&m, "unwind");
    let indirect = m.view(caller).append_basic_block(&m, "indirect");

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let call = m.view(
        b.call_builder(m.view(h))
            .arg(x)
            .operand_bundles([deopt(1)])
            .name("c")
            .build()?,
    );
    call.as_view()
        .set_metadata(&m, MetadataAttachmentKind::Dbg, dbg)?;
    b.br(&invoking)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(invoking);
    let (_, invoke) = b.invoke_dyn_with_config::<Dyn, _, _, _, _, _>(
        m.view(h),
        [x],
        &normal,
        &unwind,
        CallSiteConfig::new("i")
            .calling_conv(CallingConv::FAST)
            .operand_bundles([deopt(1)]),
    )?;
    invoke
        .as_view()
        .set_metadata(&m, MetadataAttachmentKind::Dbg, dbg)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(jumping);
    let (_, call_br) =
        b.callbr::<Dyn, _, _, _, _, _, _, _>(m.view(h), [x], &normal, [&indirect], "cb")?;
    call_br
        .as_view()
        .set_metadata(&m, MetadataAttachmentKind::Dbg, dbg)?;
    assert!(!call.has_fn_attr(AttrKind::Cold));
    assert!(!invoke.has_fn_attr(AttrKind::Cold));
    assert!(!call_br.has_fn_attr(AttrKind::Cold));
    make_cold(&m, call);
    make_cold(&m, invoke);
    make_cold(&m, call_br);

    fn only_bundle<'ctx, B: ModuleBrand + 'ctx>(
        bundles: Vec<OperandBundleUse<'ctx, B>>,
    ) -> Vec<llvmkit_ir::Value<'ctx, B>> {
        assert_eq!(bundles.len(), 1, "one bundle, the new one");
        bundles[0].inputs().collect()
    }

    let (call_copy_instruction, call_copy) = with_bundle(&m, call, deopt(2))?;
    assert_eq!(call_copy.callee(), call.callee());
    assert_eq!(call_copy.args().collect::<Vec<_>>(), vec![x.as_erased()]);
    assert_eq!(
        only_bundle(call_copy.operand_bundles().collect()),
        vec![i32_ty.const_int(2_i32).as_erased()]
    );
    assert_eq!(
        call_copy
            .as_view()
            .metadata()
            .get(&MetadataAttachmentKind::Dbg),
        Some(dbg)
    );
    assert!(call_copy.has_fn_attr(AttrKind::Cold));

    let (invoke_copy_instruction, invoke_copy) = with_bundle(&m, invoke, deopt(3))?;
    assert_eq!(
        invoke_copy.normal_destination(),
        invoke.normal_destination()
    );
    assert_eq!(
        invoke_copy.unwind_destination(),
        invoke.unwind_destination()
    );
    assert_eq!(invoke_copy.calling_conv(), CallingConv::FAST);
    assert_eq!(
        only_bundle(invoke_copy.operand_bundles().collect()),
        vec![i32_ty.const_int(3_i32).as_erased()]
    );
    assert_eq!(
        invoke_copy
            .as_view()
            .metadata()
            .get(&MetadataAttachmentKind::Dbg),
        Some(dbg)
    );
    assert!(invoke_copy.has_fn_attr(AttrKind::Cold));

    let (call_br_copy_instruction, call_br_copy) = with_bundle(&m, call_br, deopt(4))?;
    assert_eq!(
        call_br_copy.default_destination(),
        call_br.default_destination()
    );
    assert_eq!(
        call_br_copy.indirect_destinations().collect::<Vec<_>>(),
        call_br.indirect_destinations().collect::<Vec<_>>()
    );
    assert_eq!(call_br_copy.args().collect::<Vec<_>>(), vec![x.as_erased()]);
    assert_eq!(
        only_bundle(call_br_copy.operand_bundles().collect()),
        vec![i32_ty.const_int(4_i32).as_erased()]
    );
    assert_eq!(
        call_br_copy
            .as_view()
            .metadata()
            .get(&MetadataAttachmentKind::Dbg),
        Some(dbg)
    );
    assert!(call_br_copy.has_fn_attr(AttrKind::Cold));
    invoke_copy_instruction.drop_detached(&m);
    call_br_copy_instruction.drop_detached(&m);

    // The call copy joins the function after its original.
    let anchor = call
        .as_view()
        .placed()
        .expect("the original call is in a block");
    call_copy_instruction.insert_after(&m, anchor)?;
    // Read line by line rather than as whole call lines: between the argument
    // list and the bundle sits the call's `cold`, which llvmkit prints inline
    // where upstream prints a `#N` group (`docs/divergences.md`), and that
    // spelling is not what is under test.
    let text = format!("{m}");
    let line = |name: &str| {
        text.lines()
            .find(|line| {
                line.trim_start()
                    .starts_with(&format!("%{name} = call i32 @h(i32 %0)"))
            })
            .unwrap_or_else(|| panic!("no `%{name}` call:\n{text}"))
    };
    assert!(
        line("c1").contains(r#"[ "deopt"(i32 2) ]"#),
        "the copy is renamed by the function's table and carries the new bundle:\n{text}"
    );
    assert!(
        line("c").contains(r#"[ "deopt"(i32 1) ]"#),
        "the original is untouched:\n{text}"
    );
    Ok(())
}

/// `InvokeInst::create_detached` refuses a parameterised destination — a
/// block made by `IrBuilder::append_block_with_params` — on either edge with
/// `IrError::PhiArgArityMismatch`, the guard every plain `invoke` edge takes:
/// neither edge of an `invoke` carries block arguments, so the destination's
/// parameters would be left one incoming short. A refusal creates nothing,
/// which the callee's use count witnesses.
///
/// **llvmkit-specific**: block parameters are llvmkit's own. The positive
/// control is the same invoke to plain blocks.
#[test]
fn a_detached_invoke_refuses_a_parameterised_destination() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty.as_type(), [i32_ty.as_type()]);
    let h = m.add_function_dyn("h", fn_ty, Linkage::External)?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let callee = m.view(h).as_erased();
    let (with_params, _) = IrBuilder::new_for::<Dyn>(&m).append_block_with_params(
        m.view(f),
        &[i32_ty.as_type()],
        "merge",
    )?;
    let plain = m.view(f).append_basic_block(&m, "plain");
    let uses = callee.num_uses();

    for (edge, refused) in [
        (
            "normal",
            InvokeInst::<Dyn, _>::create_detached(
                &m,
                fn_ty,
                callee,
                &with_params,
                &plain,
                [i32_ty.const_int(1_i32)],
                CallSiteConfig::new("r"),
            )
            .err(),
        ),
        (
            "unwind",
            InvokeInst::<Dyn, _>::create_detached(
                &m,
                fn_ty,
                callee,
                &plain,
                &with_params,
                [i32_ty.const_int(1_i32)],
                CallSiteConfig::new("r"),
            )
            .err(),
        ),
    ] {
        assert!(
            matches!(
                refused,
                Some(IrError::PhiArgArityMismatch {
                    expected: 1,
                    got: 0
                })
            ),
            "{edge}: {refused:?}"
        );
    }
    assert_eq!(callee.num_uses(), uses, "a refusal creates nothing");

    let (created, _) = InvokeInst::<Dyn, _>::create_detached(
        &m,
        fn_ty,
        callee,
        &plain,
        &plain,
        [i32_ty.const_int(1_i32)],
        CallSiteConfig::new("r"),
    )?;
    assert_eq!(callee.num_uses(), uses + 1);
    created.drop_detached(&m);
    Ok(())
}

/// `BasicBlock::Create(Context, Name)` with no parent makes a block whose
/// `getParent()` is null; [`BasicBlock::create_orphan`] is that constructor.
///
/// **llvmkit-specific**: upstream's tests create such blocks
/// (`InstructionsTest.AlterInvokeBundles`) without reading their parent. The
/// positive control is a block appended to a function, which names it.
#[test]
fn an_orphan_block_belongs_to_no_function() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let orphan = BasicBlock::create_orphan(&m, "orphan");
    assert!(orphan.parent_function().is_none());
    assert_eq!(orphan.name().as_deref(), Some("orphan"));
    let unnamed = BasicBlock::create_orphan(&m, "");
    assert_eq!(unnamed.name(), None);

    let fn_ty = m.function_type(
        m.void_type().as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
    );
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    assert_eq!(entry.parent_function(), Some(m.view(f).as_dyn()));
    Ok(())
}
