//! llvmkit-specific capability-graded pass API lock (D1, D8): a builder a
//! pass's context lent cannot emit a terminator, at any rung.
//!
//! `FnReshape::builder_before` lends its builder in the `InPass<_>` state,
//! which is not `Terminating`. Every terminator build lives in an impl
//! bounded `S: Terminating`, so `unreachable` is absent (`E0599`, naming the
//! unmet `InPass<PositionedBefore>: Terminating`). The rung is `ReshapeCfg`,
//! the widest function rung, so the law is not `PatchBody`'s alone: a lent
//! builder sits in front of an existing instruction, where a terminator lands
//! mid-block, and its edge change would bypass the `CfgUpdate` log the
//! reshape context's own edits (`split_block`, `edit_terminator`,
//! `redirect_*`) keep for the analyses the driver repairs. At `PatchBody` it
//! would also change the CFG the rung reports preserved. No upstream
//! counterpart: an upstream `IRBuilder` inside a `FunctionPass` can build any
//! instruction, and `PreservedAnalyses` is whatever the pass declares.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct TerminateMidBlock;

impl<B: ModuleBrand> FunctionPass<B> for TerminateMidBlock {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "terminate-mid-block";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut reshape = cx.mutate();
        let terminator = reshape
            .function()
            .entry_block()
            .expect("definition")
            .placed_instructions()
            .last()
            .expect("terminator");
        reshape.builder_before(terminator)?.unreachable();
        Ok(reshape.done())
    }
}

fn main() {}
