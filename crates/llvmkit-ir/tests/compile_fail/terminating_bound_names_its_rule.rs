//! llvmkit-specific capability-graded pass API lock (D1): where code names
//! the `Terminating` bound itself, a pass's lent builder fails it with
//! `Terminating`'s own `on_unimplemented` message (`E0277`).
//!
//! `pass_builder_cannot_terminate.rs` calls a terminator build directly, and
//! rustc reports that as `E0599` naming the unmet bound, never with a trait's
//! message. A helper generic over `S: Terminating` is the other route, and the
//! one the message exists for: it says which state was handed in and where a
//! `ReshapeCfg` pass changes the CFG instead. Both states that fail the bound
//! are pinned: a lent `InPass` builder, and an `Unpositioned` one. This
//! fixture fails if the attribute is dropped or its message changes. No
//! upstream counterpart: an upstream `IRBuilder` has no positioning
//! typestate.

use llvmkit_ir::{
    FnCx, FnReport, FunctionPass, IrBuilder, IrBuilderFolder, IrResult, Module, ModuleBrand,
    ReshapeCfg, ReturnMarker, Terminating,
};

/// Close the builder's block with `unreachable`.
fn close<'a, 'ctx, B, F, S, R>(builder: IrBuilder<'a, 'ctx, B, F, S, R>)
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    S: Terminating,
    R: ReturnMarker,
{
    let _ = builder.unreachable();
}

struct CloseThroughHelper;

impl<B: ModuleBrand> FunctionPass<B> for CloseThroughHelper {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "close-through-helper";

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
        close(reshape.builder_before(terminator)?);
        Ok(reshape.done())
    }
}

/// The other state that fails the bound: an unpositioned builder, whose
/// error names its state and says to position it first.
fn unpositioned() {
    let mut module = Module::dynamic("unpositioned");
    let mut key = module.layout_token();
    close(IrBuilder::new_for::<()>(&mut key));
}

fn main() {
    unpositioned();
}
