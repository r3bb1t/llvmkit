//! llvmkit typestate compile-fail (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! An `invoke` edge is never removable (an `invoke` always has exactly its
//! normal and unwind edges), so [`InvokeEdit`] carries only `redirect_*` and no
//! `remove_*` on any trait. Calling `remove_normal` is therefore an
//! `E0599 no method` — a stable diagnostic on OUR method name, not a
//! toolchain-drifting one.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct InvokeNoRemove;

impl<B: ModuleBrand> FunctionPass<B> for InvokeNoRemove {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "invoke-no-remove";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let bb = reshape.function().entry_block().expect("entry");
        // `InvokeEdit` has no `remove_*`: an invoke edge is not removable.
        reshape.edit_invoke(bb.id())?.remove_normal();
        Ok(reshape.done())
    }
}

fn main() {}
