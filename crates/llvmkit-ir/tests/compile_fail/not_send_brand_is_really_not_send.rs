//! llvmkit-specific compile-fail (cycle C4). No LLVM analogue: C++ has no
//! auto traits.
//!
//! Companion to the positive `Send` compile-assert in
//! `tests/module_ownership.rs`, which claims that
//! `Module<NotSendBrand, S>` is `Send` *even though the brand is not*. That
//! claim is only interesting if the premise holds, so this fixture pins the
//! premise: the brand type itself really is `!Send`.
//!
//! Together the two say exactly what the brand phantom buys: the module stores
//! `Invariant<B>` = `PhantomData<fn(B) -> B>`, and a `fn` pointer type is
//! `Send + Sync` whatever `B` is, so `B`'s auto traits never reach the module.
//! Replace that phantom with a plain `PhantomData<B>` and the assert in
//! `module_ownership.rs` starts failing — while this fixture keeps passing.

use llvmkit_ir::ModuleBrand;

/// `!Send` because of the raw pointer, which still satisfies every
/// [`ModuleBrand`] supertrait (`Copy + Debug + Eq + Hash + 'static`). A raw
/// field rather than a `PhantomData` wrapper on purpose: the `PhantomData`
/// spelling makes rustc quote its own `core` source into the diagnostic, and
/// that line drifts between toolchains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NotSendBrand(*const ());
impl ModuleBrand for NotSendBrand {}

fn assert_send<T: Send>() {}

fn main() {
    assert_send::<NotSendBrand>();
}
