//! llvmkit-specific compile-fail (Doctrine D7/D4) for the typed `indirectbr`
//! address operand, not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: LLVM's verifier
//! (`Verifier::visitIndirectBrInst`) rejects an `indirectbr` whose address
//! operand is not a pointer *at runtime*. llvmkit's `build_indirectbr` binds
//! the address by `IntoPointerValue<'ctx, B>`, so a typed non-pointer value
//! handle cannot type-check — the pointer-ness check moves from `verify()` to
//! compile time.
//!
//! Locks the "address is a typed integer" mistake: an `IntValue<'ctx, i32, B>`
//! has no `IntoPointerValue<'ctx, B>` impl (that trait is implemented only for
//! the pointer family plus the runtime-checked erased handles), so rustc
//! surfaces the unsatisfied `IntoPointerValue` bound — an llvmkit-authored
//! trait bound, stable across rustc versions. A *value handle* (not a bare
//! literal) is used so the fixture unambiguously proves `IntoPointerValue` is
//! the gate: a bare literal is `!IsValue` and would fail under either bound.

use llvmkit_ir::{IRBuilder, IntValue, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");

        // A typed non-pointer value handle: the `i32` function parameter
        // narrowed to `IntValue<i32>`.
        let addr: IntValue<i32> = f.param(0).unwrap().try_into().unwrap();
        let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);

        // `IntValue<'_, i32, _>` does not implement `IntoPointerValue`, so it
        // cannot be an `indirectbr` address: `build_indirectbr` does not
        // type-check.
        let _ = b.build_indirectbr(addr, "");
    });
}
