//! llvmkit-specific capability-graded pass API lock (D1): a builder a pass's
//! context lent cannot be repositioned.
//!
//! `FnReshape::builder_before` checks that the anchor is in the pass's own
//! function and lends the builder in the `InPass<_>` state, which
//! is not `Repositionable`. So `position_before` — which would move it to an
//! instruction of any function — is a type error carrying `Repositionable`'s
//! own `on_unimplemented` message, and the context's one check cannot be
//! bypassed after the fact. The rung is `ReshapeCfg`, the widest function
//! rung, so the refusal holds at every rung. No upstream
//! counterpart: an upstream `IRBuilder` inside a `FunctionPass` can be
//! pointed at any instruction (`IRBuilderBase::SetInsertPoint`).

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct Wander;

impl<B: ModuleBrand> FunctionPass<B> for Wander {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "wander";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut reshape = cx.mutate();
        let entry = reshape.function().entry_block().expect("definition");
        let first = entry.placed_instructions().next().expect("instruction");
        let last = entry.placed_instructions().last().expect("terminator");
        let _ = reshape.builder_before(first)?.position_before(last);
        Ok(reshape.done())
    }
}

fn main() {}
