//! llvmkit-specific Pass API v2 lock (Doctrine D1/D8), not a 1:1 LLVM port.
//!
//! The capability rung a `FunctionPass` declares via `type Access` fixes how
//! much of the IR it may touch. `Inspect` is the read-only rung: it deliberately
//! has NO `MutatingFn` impl, so `FnCx::mutate` — defined only where
//! `A: MutatingFn` (`pass_context.rs`) — is structurally absent from an
//! `Inspect` context. A pass cannot "accidentally" mutate while claiming to only
//! inspect; the mutation door simply is not on the type. Upstream LLVM has no
//! such compile-time capability grading (any `FunctionPass` can mutate anything).
//!
//! This is the guarantee the user specifically asked to lock: calling
//! `cx.mutate()` from an `Inspect` pass is `E0599` — the method exists but its
//! `Inspect: MutatingFn` bound is not satisfied.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, Inspect, IrResult, ModuleBrand};

struct InspectMutates;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for InspectMutates {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "inspect-mutates";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport> {
        // `Inspect` does not implement `MutatingFn`, so `mutate()` is not on the
        // context: over-claiming (mutating from a read-only rung) is unspellable.
        let patch = cx.mutate();
        Ok(patch.done())
    }
}

fn main() {}
