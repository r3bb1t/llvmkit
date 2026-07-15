//! llvmkit typestate compile-fail (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! A `switch` must keep its default edge, so [`SwitchEdit`] offers
//! `remove_successor` (a case edge) but NO `remove_default`. Calling
//! `remove_default` is therefore an `E0599 no method` — a stable diagnostic on
//! OUR method name.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg};

struct SwitchNoRemoveDefault;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for SwitchNoRemoveDefault {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "switch-no-remove-default";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let bb = reshape.function().entry_block().expect("entry");
        // `SwitchEdit` has no `remove_default`: a switch must keep its default.
        reshape.edit_switch(&bb)?.remove_default();
        Ok(reshape.done())
    }
}

fn main() {}
