//! llvmkit typestate compile-fail (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! An unconditional `br` has exactly one edge; removing it would leave the
//! block with no successor, so [`BrEdit`] carries only `redirect` and no
//! `remove`. Calling `remove` is therefore an `E0599 no method` — a stable
//! diagnostic on OUR method name.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct BrNoRemove;

impl<B: ModuleBrand> FunctionPass<B> for BrNoRemove {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "br-no-remove";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let bb = reshape.function().entry_block().expect("entry");
        // `BrEdit` has no `remove`: an unconditional br's sole edge is not
        // removable.
        reshape.edit_br(bb.id())?.remove();
        Ok(reshape.done())
    }
}

fn main() {}
