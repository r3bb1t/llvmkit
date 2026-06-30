//! `call` instruction print form and call-builder ergonomics.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` carries a citation naming the upstream
//! `unittests/IR/InstructionsTest.cpp` `TEST` (or `test/Assembler/*.ll`
//! fixture) it ports.

use llvmkit_ir::{
    FloatValue, IRBuilder, IntValue, IntrinsicDescriptor, IntrinsicId, IrError, Linkage, Module,
    Ptr,
};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// — exercises construction of a non-void `CallInst` against a declared
/// callee.
#[test]
fn call_int_returning_function() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        // declare i32 @callee(i32, i32)
        let callee_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let callee = m.add_function::<i32, _>("callee", callee_ty, Linkage::External)?;
        // define i32 @caller(i32 %x, i32 %y) { %r = call i32 @callee(i32 %x, i32 %y); ret i32 %r }
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let caller = m.add_function::<i32, _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: llvmkit_ir::IntValue<i32> = caller.param(0)?.try_into()?;
        let y: llvmkit_ir::IntValue<i32> = caller.param(1)?.try_into()?;
        let inst = b.build_call(callee, [x.as_value(), y.as_value()], "r")?;
        // Typed return accessor (Doctrine D4): `R` flows from the callee
        // through `build_call` into `CallInst<'ctx, i32>`, which directly
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
        let callee = m.add_function::<(), _>("sink", callee_ty, Linkage::External)?;
        // define void @caller() { call void @sink(); ret void }
        let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let inst = b.build_call(callee, Vec::<llvmkit_ir::Value>::new(), "")?;
        assert!(inst.return_value().is_none());
        b.build_ret_void();
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
        let callee = m.add_function::<(), _>("with_ptr", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let p: llvmkit_ir::PointerValue = caller.param(0)?.try_into()?;
        // Mixed-type args: an `IntValue<i32>` and a `PointerValue` go into
        // the same call. The builder pattern accepts each via a
        // monomorphised `arg<V: IsValue>` call.
        let answer = m.i32_type().const_int(42_i32);
        b.call_builder(callee).arg(answer).arg(p).build()?;
        b.build_ret_void();
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
        let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function::<i32, _>("g", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function::<i32, _>("f", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
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
        let caller = m.add_function::<f32, _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
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
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
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
        let callee_ty = m.fn_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function::<Ptr, _>("alloc_ptr", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function::<Ptr, _>("g", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Ptr>(&m).position_at_end(entry);
        let inst = b.build_call(callee, Vec::<llvmkit_ir::Value>::new(), "p")?;
        let p = inst.return_pointer_value();
        b.build_ret(p)?;
        let text = format!("{m}");
        assert!(text.contains("%p = call ptr @alloc_ptr()"), "got:\n{text}");
        Ok(())
    })
}
