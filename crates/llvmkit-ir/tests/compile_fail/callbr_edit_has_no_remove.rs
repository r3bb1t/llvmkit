//! llvmkit typestate compile-fail (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! A `callbr` edge is never removable, so [`CallBrEdit`] carries only
//! `redirect_*` and no `remove_*` on any trait. Calling `remove_default` is
//! therefore an `E0599 no method` — a stable diagnostic on OUR method name.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct CallBrNoRemove;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for CallBrNoRemove {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "callbr-no-remove";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let bb = reshape.function().entry_block().expect("entry");
        // `CallBrEdit` has no `remove_*`: a callbr edge is not removable.
        reshape.edit_callbr(&bb)?.remove_default();
        Ok(reshape.done())
    }
}

fn main() {}
