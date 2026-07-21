//! llvmkit-specific Pass-API-v2 honesty lock (no upstream analog): after
//! `cx.mutate()` consumes the context, the all-preserved `done()` report is
//! unspellable — a mutating pass cannot claim it preserved everything.
//!
//! This is the branch's whole thesis expressed as a compile-fail: "declared
//! mutating, then claimed preserved-everything after mutating" must not type
//! check. `FnCx::mutate` (`pass_context.rs`) takes `self` **by value** and moves
//! the token/function/results into the rung's mutator; `FnCx::done` also
//! takes `self` by value. So calling `cx.done()` after `cx.mutate()` is a
//! use of the already-moved `cx`. The only report a mutating pass can produce
//! after entering the mutator is the mutator's own `done()`, which carries the
//! rung's preservation floor — the over-claiming report has no spelling.
//!
//! Modeled on `retained_open_phi_cannot_add_after_finish.rs` (same "consumed by
//! value, then used again" pattern). Upstream LLVM has no such move-consuming
//! honesty seam: any `FunctionPass` can mutate and still return
//! `PreservedAnalyses::all()`.
//!
//! Primary error: `error[E0382]: use of moved value: cx`.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, PatchBody};

struct ClaimPreservedAfterMutate;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for ClaimPreservedAfterMutate {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "claim-preserved-after-mutate";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        // `mutate()` consumes `cx` by value and moves it into the mutator, so the
        // all-preserved `done()` on the moved `cx` is unspellable: a mutating
        // pass cannot claim it preserved everything.
        let _patch = cx.mutate();
        Ok(cx.done())
    }
}

fn main() {}
