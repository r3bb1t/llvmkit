//! llvmkit-specific compile-fail (Doctrine D7/D4), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `CallInst::init`'s "Calling a function with a
//! bad signature!" assertion (`lib/IR/Instructions.cpp`) and
//! `Verifier::visitCallBase`'s per-argument type check reject a
//! wrong-typed call argument *at runtime*. llvmkit's typed `build_call`
//! pushes that same invariant into the Rust type system via the
//! `IntoCallArg<'ctx, P, B>` bound on each call-argument position.
//!
//! This fixture locks `IntoCallArg`'s own
//! `#[diagnostic::on_unimplemented]` message ("cannot fill a call-
//! argument slot of schema"), which requires an argument type with
//! **zero** syntactically-matching `IntoCallArg` impls for the targeted
//! parameter slot -- not merely a value whose *inner* lifting bound
//! (`IntoIntValue`/`IntoFloatValue`/`IntoPointerValue`) fails.
//!
//! IMPORTANT: an `i32` (or any other integer-width, float-kind, or `Ptr`)
//! parameter slot cannot be used for this lock. Every one of those three
//! families' `IntoCallArg` blanket impls is unconstrained over its
//! generic `V` (e.g. `impl<V: IntoIntValue<'ctx, i32, B>> IntoCallArg<'ctx,
//! i32, B> for V`), so rustc's trait selection *always* finds one of
//! those blanket impls as a syntactic candidate for any `V` -- including
//! a `String` -- and reports the blanket's failed inner where-clause
//! (`IntoIntValue`/etc.), never falling through to "no `IntoCallArg` impl
//! found." This was verified empirically: a `String` argument against an
//! `i32` slot reports `the trait bound `String: IntoIntValue<'_, i32,
//! _>` is not satisfied`, exactly like the lifted-float-value fixture,
//! *not* the `on_unimplemented` message.
//!
//! The struct-schema family's blanket impls, by contrast, are
//! constrained on concrete source types (`Value`, `Argument`,
//! `Constant`, `Instruction` -- see `struct_schema.rs`'s
//! `impl_struct_into_call_arg!` invocations), not on unconstrained `V`.
//! So a struct-schema parameter slot (`Point`, an `#[derive(IrStruct)]`
//! type below) *does* have zero candidate `IntoCallArg` impls for a
//! `String` argument, and rustc correctly reports `IntoCallArg` itself
//! as unsatisfied, firing its `on_unimplemented` message.

use llvmkit_ir::{IRBuilder, IrStruct, Linkage, Module};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

fn main() {
    Module::with_new("c", |m| {
        let callee = m
            .add_typed_function::<i32, (Point, i32), _>("callee", Linkage::External)
            .unwrap();
        let caller = m
            .add_typed_function::<i32, (i32,), _>("caller", Linkage::External)
            .unwrap();
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (x,) = caller.params();
        // `String` implements neither `IntoIntValue`/`IntoFloatValue`/
        // `IntoPointerValue` nor is one of the struct-schema blanket's
        // concrete source types (`Value`/`Argument`/`Constant`/
        // `Instruction`), so it has zero candidate
        // `IntoCallArg<'_, Point, _>` impls at all: rustc reports
        // `IntoCallArg` itself as unsatisfied and its on_unimplemented
        // message fires.
        let bogus = String::from("not a value");
        let _ = b.build_call(callee, (bogus, x), "bad");
    });
}
