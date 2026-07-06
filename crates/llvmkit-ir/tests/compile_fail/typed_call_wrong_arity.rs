//! llvmkit-specific compile-fail (Doctrine D7/D4), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `CallInst::init`'s "Calling a function with a
//! bad signature!" assertion (`lib/IR/Instructions.cpp`) and
//! `Verifier::visitCallBase`'s authoritative arity check reject a wrong
//! argument count for a call site *at runtime*. llvmkit's typed `build_call`
//! pushes that same invariant into the Rust type system for statically-known
//! callee schemas: passing a 1-element tuple against a 2-parameter typed
//! callee has no `CallArgs<'ctx, (i32, i32), _>` impl, so it is a compile
//! error, not a build-time `IrError`.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let callee = m
            .add_typed_function::<i32, (i32, i32), _>("callee", Linkage::External)
            .unwrap();
        let caller = m
            .add_typed_function::<i32, (i32,), _>("caller", Linkage::External)
            .unwrap();
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (x,) = caller.params();
        // `(x,)` is a 1-tuple; `callee`'s schema is `(i32, i32)` (arity 2).
        // There is no `CallArgs<'_, (i32, i32), _>` impl for `(IntValue<i32>,)`.
        let _ = b.build_call(callee, (x,), "bad");
    });
}
