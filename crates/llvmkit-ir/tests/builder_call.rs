//! `call` instruction print form and call-builder ergonomics.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` carries a citation naming the upstream
//! `unittests/IR/InstructionsTest.cpp` `TEST` (or `test/Assembler/*.ll`
//! fixture) it ports.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module, Ptr};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// — exercises construction of a non-void `CallInst` against a declared
/// callee.
#[test]
fn call_int_returning_function() -> Result<(), IrError> {
    let m = Module::new("c");
    let i32_ty = m.i32_type();
    // declare i32 @callee(i32, i32)
    let callee_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let callee = m.add_function::<i32>("callee", callee_ty, Linkage::External)?;
    // define i32 @caller(i32 %x, i32 %y) { %r = call i32 @callee(i32 %x, i32 %y); ret i32 %r }
    let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let caller = m.add_function::<i32>("caller", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let x: llvmkit_ir::IntValue<i32> = caller.param(0)?.try_into()?;
    let y: llvmkit_ir::IntValue<i32> = caller.param(1)?.try_into()?;
    let inst = b.build_call(callee, [x.as_value(), y.as_value()], "r")?;
    let ret_val: llvmkit_ir::IntValue<i32> = inst
        .return_value()
        .expect("non-void call returns a value")
        .try_into()?;
    b.build_ret(ret_val)?;
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
    let m = Module::new("c");
    let void_ty = m.void_type();
    // declare void @sink()
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<()>("sink", callee_ty, Linkage::External)?;
    // define void @caller() { call void @sink(); ret void }
    let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let caller = m.add_function::<()>("caller", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let inst = b.build_call(callee, Vec::<llvmkit_ir::Value>::new(), "")?;
    assert!(inst.return_value().is_none());
    b.build_ret_void();
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
    let m = Module::new("c");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(
        void_ty.as_type(),
        [i32_ty.as_type(), ptr_ty.as_type()],
        false,
    );
    let callee = m.add_function::<()>("with_ptr", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let caller = m.add_function::<()>("caller", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
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
}

/// Mirrors the `tail call` textual form locked by
/// `test/Assembler/call-arg-is-callee.ll` and friends in
/// `test/Assembler/`. Closest upstream functional coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`.
#[test]
fn call_tail() -> Result<(), IrError> {
    let m = Module::new("c");
    let i32_ty = m.i32_type();
    let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<i32>("g", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let caller = m.add_function::<i32>("f", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let inst = b.call_builder(callee).tail().name("r").build()?;
    let r: llvmkit_ir::IntValue<i32> = inst.return_value().unwrap().try_into()?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = tail call i32 @g()"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)`
/// specialized for a pointer-returning callee.
#[test]
fn call_to_pointer_returning_function() -> Result<(), IrError> {
    let m = Module::new("c");
    let ptr_ty = m.ptr_type(0);
    let callee_ty = m.fn_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<Ptr>("alloc_ptr", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let caller = m.add_function::<Ptr>("g", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let b = IRBuilder::new_for::<Ptr>(&m).position_at_end(entry);
    let inst = b.build_call(callee, Vec::<llvmkit_ir::Value>::new(), "p")?;
    let p: llvmkit_ir::PointerValue = inst.return_value().unwrap().try_into()?;
    b.build_ret(p)?;
    let text = format!("{m}");
    assert!(text.contains("%p = call ptr @alloc_ptr()"), "got:\n{text}");
    Ok(())
}
