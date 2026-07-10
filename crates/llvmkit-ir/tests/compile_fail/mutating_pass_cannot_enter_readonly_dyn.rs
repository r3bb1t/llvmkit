//! llvmkit-specific capability-graded pass API lock (Doctrine D1/D8), not a 1:1 LLVM port.
//!
//! A `DynReadOnlyFunctionPipeline` guarantees a `Module<Verified>` output by
//! REFUSING entry to anything that could downgrade it: its `push` is bounded to
//! `P::Access: ReadOnlyFn` (`pass_manager.rs`), and `ReadOnlyFn` is implemented
//! for the `Inspect` rung only. A mutating pass — here `DcePass`, whose
//! `type Access = PatchBody` — does not satisfy that bound, so it cannot be
//! pushed into a read-only container: the verified/unverified split is enforced
//! at the type level, never at runtime. Upstream LLVM has no read-only pipeline
//! container that structurally excludes mutating passes.
//!
//! Primary error: our own `#[diagnostic::on_unimplemented]` for
//! `PatchBody: ReadOnlyFn` ("`PatchBody` is a mutating function rung…").

use llvmkit_ir::{DcePass, DynReadOnlyFunctionPipeline, ModuleBrand};

fn build<'ctx, B: ModuleBrand + 'ctx>(pipeline: &mut DynReadOnlyFunctionPipeline<'ctx, B>) {
    // `DcePass::Access` is `PatchBody`, a mutating rung; the read-only
    // container's `push` bound `P::Access: ReadOnlyFn` admits only `Inspect`.
    pipeline.push(DcePass);
}

fn main() {}
