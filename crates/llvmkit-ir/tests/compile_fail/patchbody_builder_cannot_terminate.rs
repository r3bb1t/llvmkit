//! llvmkit-specific capability-graded pass API lock (D1, D8): a `PatchBody`
//! pass's lent builder cannot emit a terminator.
//!
//! `FnPatch::builder_before` lends its builder in the `InPass<PatchBody, _>`
//! state, which is not `Terminating`. Every terminator build lives in an impl
//! bounded `S: Terminating`, so `unreachable` is absent (`E0599`, naming the
//! unmet `InPass<PatchBody, PositionedBefore>: Terminating`). A terminator in the middle of a block adds or removes CFG edges,
//! and the `PatchBody` rung reports every CFG analysis preserved, so allowing
//! it would let `done()` over-claim. No upstream counterpart: an upstream
//! `IRBuilder` inside a `FunctionPass` can build any instruction, and
//! `PreservedAnalyses` is whatever the pass declares.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, PatchBody};

struct TerminateMidBlock;

impl<B: ModuleBrand> FunctionPass<B> for TerminateMidBlock {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "terminate-mid-block";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut patch = cx.mutate();
        let terminator = patch
            .function()
            .entry_block()
            .expect("definition")
            .placed_instructions()
            .last()
            .expect("terminator");
        patch.builder_before(terminator)?.unreachable();
        Ok(patch.done())
    }
}

fn main() {}
