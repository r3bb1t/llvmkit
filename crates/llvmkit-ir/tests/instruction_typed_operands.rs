//! P1 typed-operand coverage.
//!
//! Pointer operands rediscovered through `InstructionKind` come back as
//! [`PointerValue`] rather than the erased `Value`, and a direct call's
//! callee classifies as [`Callee::Direct`] carrying a [`FunctionValue`].

use llvmkit_ir::{
    Callee, IRBuilder, InstructionKind, InstructionView, IntValue, IrError, Linkage, Module,
    PointerValue, Value,
};

/// A rediscovered `load`'s pointer operand is statically a `PointerValue`.
#[test]
fn load_pointer_operand_is_typed() -> Result<(), IrError> {
    Module::with_new("typed_load_ptr", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let p: PointerValue = f.param(0)?.try_into()?;
        let loaded = b.build_load(i32_ty, p, "v")?;

        let view = InstructionView::try_from(loaded)?;
        let Some(InstructionKind::Load(load)) = view.kind() else {
            panic!("expected a load instruction");
        };
        // `pointer()` returns `PointerValue`, not an erased `Value`.
        let ptr: PointerValue = load.pointer();
        assert_eq!(ptr.as_value(), p.as_value());
        Ok(())
    })
}

/// A direct call classifies its callee as `Direct` carrying the function.
#[test]
fn direct_call_callee_is_direct() -> Result<(), IrError> {
    Module::with_new("direct_call", |m| {
        let i32_ty = m.i32_type();
        let callee_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let callee = m.add_function::<i32, _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let caller = m.add_function::<i32, _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let call = b.build_call_dyn(callee, [x.as_value()], "r")?;

        match call.classify_callee() {
            Callee::Direct(function) => assert_eq!(function.as_value(), callee.as_value()),
            Callee::Indirect(_) => panic!("expected a direct call to classify as Direct"),
        }
        Ok(())
    })
}

/// An indirect call (callee is a function-pointer argument) classifies as
/// `Indirect` carrying a `PointerValue`.
#[test]
fn indirect_call_callee_is_indirect() -> Result<(), IrError> {
    Module::with_new("indirect_call", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        // define i32 @caller(ptr %fp) { %r = call i32 %fp(); ret ... }
        let caller_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
        let caller = m.add_function::<i32, _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let fp: PointerValue = caller.param(0)?.try_into()?;
        let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let call = b.build_indirect_call_dyn::<i32, _, Value, _>(
            callee_ty,
            fp,
            Vec::<Value>::new(),
            "r",
        )?;

        match call.classify_callee() {
            Callee::Indirect(pointer) => assert_eq!(pointer.as_value(), fp.as_value()),
            Callee::Direct(_) => panic!("expected an indirect call to classify as Indirect"),
        }
        Ok(())
    })
}
