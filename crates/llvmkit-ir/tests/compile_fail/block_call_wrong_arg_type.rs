//! llvmkit-specific compile-fail (Doctrine D7/D4) for the typed block-argument
//! edge (`BlockCall`), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: LLVM's verifier rejects a branch whose block-
//! argument type disagrees with the successor block's matching parameter phi
//! *at runtime*. llvmkit's typed `BlockCall` pushes that invariant into the Rust
//! type system: `BasicBlockLabel::call` carries a `CallArgs<'ctx, Params, B>`
//! bound, so an argument that cannot fill its typed block-parameter slot is a
//! compile error, not a build-time `IrError`.
//!
//! Locks the realistic "wrong lifted type" mistake against a typed block-
//! parameter slot: an `f64` *does* have an `IntoCallArg` candidate impl in scope
//! (the int blanket, via `V: IntoIntValue<'ctx, i32, B>`), so rustc's root-bound
//! reporting surfaces the unsatisfied `IntoIntValue<'_, i32, _>` bound one level
//! *below* `IntoCallArg` — an llvmkit-authored trait bound, stable across rustc
//! versions. (`CallArgs`'s own `on_unimplemented` message only fires for a slot
//! with zero candidate impls; see `block_call_wrong_arity.rs` for that shape.)

use llvmkit_ir::{IRBuilder, Linkage, Module, Type};

fn main() {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();

        let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m);
        // `head`'s single parameter schema is `i32`.
        let (head, _params) = b.append_block_typed::<(i32,), _>(f, "head").unwrap();

        // `1.0_f64` does not implement `IntoIntValue<'_, i32, _>`, so it cannot
        // fill the `i32` block-parameter slot: `.call` does not type-check.
        let _ = head.label().call((1.0_f64,));
    });
}
