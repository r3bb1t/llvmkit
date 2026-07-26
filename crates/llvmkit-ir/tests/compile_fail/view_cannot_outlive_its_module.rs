//! llvmkit 2.0 cycle C/E compile-fail (Doctrine D2, D7).
//!
//! The counterpart to the id family's whole reason for existing. Since cycle C
//! a `Module<B, S>` is an ordinary owned value that can be dropped, so the
//! borrowing handles it hands out — `FunctionValue`, `BasicBlock`,
//! `BasicBlockLabel`, `Value`, every `*View` — carry a `'ctx` borrow *of that
//! module* and cannot outlive it. Here the module is dropped at the end of the
//! inner block while a `BasicBlock` minted from it escapes, which rustc rejects
//! with the stable `E0597` borrow-does-not-live-long-enough.
//!
//! This is the law that makes ids necessary rather than merely convenient: the
//! **id** form of exactly this program *does* compile, because `BlockId<R, B>`
//! is `'static` — it carries the brand and a `ModuleId` tag but no borrow. That
//! is why a stale id is a run-time `IrError::ForeignValueId` / `view` panic
//! (locked by `tests/module_ownership.rs`) rather than a compile error, while a
//! stale *view* like this one can never be built at all.
//!
//! Upstream has neither half: a `BasicBlock *` outliving its `Module` is a
//! dangling pointer with no diagnostic at any stage.

use llvmkit_ir::{Linkage, Module};

fn main() {
    let escaped = {
        let m = Module::dynamic("m");
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)
            .unwrap()
            .as_function();
        // Borrows `m`. Replacing this with `.id()` would compile — that is the
        // whole point of the storable id family.
        m.view(f).append_basic_block(&m, "entry")
    };
    let _ = escaped;
}
