//! llvmkit typestate compile-fail (Doctrine D4), Task 19.
//!
//! `SsaBuilder::ret<V>` bounds its value parameter
//! `V: IntoReturnValue<'ctx, R, B>`, where `R` is the builder's own
//! return marker. `IntoReturnValue` is implemented per concrete
//! `(value-shape, R)` pair (`impl_into_return_value_int!`/
//! `impl_into_return_value_float!`/the pointer impl in
//! `ir_builder.rs`) -- there is no impl for `R = ()` at all, since a
//! void function's only valid terminator-with-no-operand is
//! `ret_void`. Passing ANY value to `ret` on an `R = ()` builder has
//! zero candidate `IntoReturnValue<'_, (), _>` impls and is a compile
//! error (`E0277`). Closest upstream behaviour:
//! `Verifier::visitReturnInst`'s "Found return instr that returns
//! non-void in Function of void return type!" check, enforced here at
//! the type level instead of at verify time.

use llvmkit_ir::{Linkage, Module};

fn main() {
    Module::with_new("ssa-ret-value-in-void-fn", |m| {
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)
            .unwrap()
            .as_function();
        let mut b = llvmkit_ir::SsaBuilder::for_function(&m, f).unwrap();
        let entry = b.create_block("entry");

        let b = b.switch_to_block(entry).unwrap();
        // `f`'s return marker is `()` (void) -- `ret_void()` is the only
        // valid terminator here, `ret(1_i32)` has no matching
        // `IntoReturnValue<'_, (), _>` impl for `i32`.
        let _oops = b.ret(1_i32).unwrap();
    });
}
