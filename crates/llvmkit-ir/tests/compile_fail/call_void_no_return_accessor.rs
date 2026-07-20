//! llvmkit typestate compile-fail (Doctrine D4).
//!
//! `CallInst<'ctx, R>`'s typed-return accessors (`return_int_value`,
//! `return_float_value`, `return_pointer_value`) are gated to the
//! corresponding marker; a void call (`R = ()`) exposes none of them.
//! Closest upstream behaviour: LLVM's `CallInst::getType()` returns
//! `Type::getVoidTy()` and any caller that asks for an integer/float/
//! pointer must downcast at runtime.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let void_ty = m.void_type();
        let callee = m
            .add_typed_function::<(), (), _>("sink", Linkage::External)
            .unwrap()
            .as_function();
        let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function_dyn("c", caller_ty, Linkage::External).unwrap();
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
        let inst = b
            .build_call_dyn(callee, Vec::<llvmkit_ir::Value>::new(), "")
            .unwrap();
        // `return_int_value` is not in scope for `CallInst<'_, ()>`.
        let _ = inst.return_int_value();
    });
}
