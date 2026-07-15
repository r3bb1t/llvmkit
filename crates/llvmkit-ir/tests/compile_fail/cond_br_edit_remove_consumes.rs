//! llvmkit typestate compile-fail (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! Removing an arm of a `cond_br` collapses it to an unconditional `br` to the
//! survivor — the `cond_br` no longer exists afterward. So
//! [`CondBrEdit::remove_then`]/`remove_else` take `self` by value: a second
//! collapse is a use-after-move (`E0382`), proving a `cond_br` has exactly one
//! collapse. A stable diagnostic driven by OUR by-value receiver.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct CondBrDoubleRemove;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for CondBrDoubleRemove {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "cond-br-double-remove";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let bb = reshape.function().entry_block().expect("entry");
        let e = reshape.edit_cond_br(&bb)?;
        // `remove_then` consumes `e`; the second removal is a use-after-move.
        e.remove_then()?;
        e.remove_else()?;
        Ok(reshape.done())
    }
}

fn main() {}
