//! llvmkit-specific capability-graded pass API lock (Doctrine D1 — make invalid states
//! unrepresentable).
//!
//! `FnPatch::erase` accepts only a `NonTerminator`, obtained from
//! `InstructionView::as_non_terminator` (which returns `None` for a
//! terminator). Handing `erase` a raw `InstructionView` — the only thing you
//! have before narrowing, and the shape a terminator is stuck in — is a type
//! error. So erasing a terminator, which would break a `PatchBody` pass's
//! "CFG preserved" floor, is unrepresentable rather than a runtime rejection.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, PatchBody};

struct EraseTerminator;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for EraseTerminator {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "erase-terminator";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        let patch = cx.mutate();
        let terminator = patch
            .function()
            .entry_block()
            .expect("definition")
            .instructions()
            .last()
            .expect("terminator");
        // `terminator` is an `InstructionView`, not a `NonTerminator`.
        patch.erase(&terminator);
        Ok(patch.done())
    }
}

fn main() {}
