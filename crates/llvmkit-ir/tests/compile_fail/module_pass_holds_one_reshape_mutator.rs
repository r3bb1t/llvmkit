//! llvmkit-specific compile-fail, no upstream counterpart: an upstream module
//! pass can hold any number of `IRBuilder`s over any functions at once.
//!
//! `ModRewrite::reshape_functions` is a lending cursor, like
//! `patch_functions` (`module_pass_holds_one_function_mutator.rs`): `step`
//! lends one `FnReshape` borrowing the cursor, so a module pass cannot hold
//! two function reshapers at once. Two at once could each hand out a builder
//! (`builder_at` / `builder_before`), and one reshaper's edit could then
//! invalidate the other's builder, which the module's `LayoutToken` design
//! rules out (`docs/design/layout-token-design.md`).

use llvmkit_ir::{IrResult, ModCx, ModReport, ModuleBrand, ModulePass, RewriteModule};

struct TwoReshapersAtOnce;

impl<B: ModuleBrand> ModulePass<B> for TwoReshapersAtOnce {
    type Access = RewriteModule;
    type Requires = ();
    const NAME: &'static str = "two-reshapers-at-once";

    fn run<'m, 'ctx>(
        &mut self,
        cx: ModCx<'m, '_, '_, 'ctx, B, RewriteModule, ()>,
    ) -> IrResult<ModReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut rewrite = cx.mutate();
        let mut functions = rewrite.reshape_functions();
        let first = functions.step();
        let second = functions.step();
        let _ = (first, second);
        Ok(rewrite.done())
    }
}

fn main() {}
