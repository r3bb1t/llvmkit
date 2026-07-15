//! llvmkit typestate compile-fail (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! An unconditional `br` has exactly one edge; removing it would leave the
//! block with no successor, so [`BrEdit`] carries only `redirect` and no
//! `remove`. Calling `remove` is therefore an `E0599 no method` — a stable
//! diagnostic on OUR method name.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct BrNoRemove;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for BrNoRemove {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "br-no-remove";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let bb = reshape.function().entry_block().expect("entry");
        // `BrEdit` has no `remove`: an unconditional br's sole edge is not
        // removable.
        reshape.edit_br(&bb)?.remove();
        Ok(reshape.done())
    }
}

fn main() {}
