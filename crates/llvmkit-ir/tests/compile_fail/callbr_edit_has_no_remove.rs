//! llvmkit typestate compile-fail (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! A `callbr` edge is never removable, so [`CallBrEdit`] carries only
//! `redirect_*` and no `remove_*` on any trait. Calling `remove_default` is
//! therefore an `E0599 no method` — a stable diagnostic on OUR method name.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct CallBrNoRemove;

impl<B: ModuleBrand> FunctionPass<B> for CallBrNoRemove {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "callbr-no-remove";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let bb = reshape.function().entry_block().expect("entry");
        // `CallBrEdit` has no `remove_*`: a callbr edge is not removable.
        reshape.edit_callbr(bb.id())?.remove_default();
        Ok(reshape.done())
    }
}

fn main() {}
