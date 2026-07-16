//! llvmkit-specific compile-fail (Doctrine D7/D4) for the typed block-argument
//! edge (`BlockCall`), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: LLVM's verifier rejects a branch whose block-
//! argument count disagrees with the successor block's leading parameter phis
//! *at runtime*. llvmkit's typed `BlockCall` pushes that invariant into the Rust
//! type system: `BasicBlockLabel::call` carries a `CallArgs<'ctx, Params, B>`
//! bound, and a 1-element tuple has no `CallArgs<'_, (i32, i32), _>` impl, so
//! seeding a two-parameter typed block with a single argument is a compile
//! error, not a build-time `IrError`.
//!
//! Locks `CallArgs`'s own `#[diagnostic::on_unimplemented]` message ("argument
//! tuple ... does not match the callee's parameter schema") — an llvmkit-authored
//! diagnostic that does not drift across rustc versions (unlike a native
//! `E0308`/`E0599`).

use llvmkit_ir::{IRBuilder, Linkage, Module, Type};

fn main() {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m
            .add_function::<i32, _>("f", fn_ty, Linkage::External)
            .unwrap();

        let b = IRBuilder::new_for::<i32>(&m);
        // `head` carries a TWO-i32 typed parameter schema (arity 2).
        let (head, _params) = b.append_block_typed::<(i32, i32), _>(f, "head").unwrap();

        // `(x,)` is a 1-tuple; there is no `CallArgs<'_, (i32, i32), _>` impl for
        // a 1-element tuple, so `.call` does not type-check — the arity mismatch
        // is caught here, not at a distant `verify()`.
        let x = i32_ty.const_int(0_i32);
        let _ = head.label().call((x,));
    });
}
