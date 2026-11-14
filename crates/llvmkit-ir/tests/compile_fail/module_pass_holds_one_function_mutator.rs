//! llvmkit-specific compile-fail, no upstream counterpart: an upstream module
//! pass can hold any number of `IRBuilder`s over any functions at once.
//!
//! `ModRewrite::patch_functions` is a lending cursor: `step` lends one
//! `FnPatch` borrowing the cursor, so a module pass cannot hold two function
//! mutators at once. Two at once could each hand out a builder (`builder_at`
//! / `builder_before`), and one mutator's edit could then invalidate the other
//! mutator's builder, which the module's `LayoutToken` design rules out
//! (`docs/design/layout-token-design.md`).

use llvmkit_ir::{IrResult, ModCx, ModReport, ModuleBrand, ModulePass, RewriteModule};

struct TwoAtOnce;

impl<B: ModuleBrand> ModulePass<B> for TwoAtOnce {
    type Access = RewriteModule;
    type Requires = ();
    const NAME: &'static str = "two-at-once";

    fn run<'m, 'ctx>(
        &mut self,
        cx: ModCx<'m, '_, '_, 'ctx, B, RewriteModule, ()>,
    ) -> IrResult<ModReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut rewrite = cx.mutate();
        let mut functions = rewrite.patch_functions();
        let first = functions.step();
        let second = functions.step();
        let _ = (first, second);
        Ok(rewrite.done())
    }
}

fn main() {}
