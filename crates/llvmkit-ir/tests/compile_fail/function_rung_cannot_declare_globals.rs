//! llvmkit-specific Pass-API-v2 honesty lock (no upstream analog): a function
//! rung cannot reach the module's **declaration** surface, so "declared
//! body-patching, then mutated the module" has no spelling.
//!
//! The thesis is a boundary, not a ban on module access. Type and constant
//! construction is preservation-*neutral* — interning a type touches the
//! context's type table, never a function or a global — so it stays reachable at
//! every rung through the read-only `FnPatch::module` view (exercised by
//! `pass_context::tests::patchbody_reaches_types_through_module_view`).
//! *Declarations* (`add_global`, `add_function_dyn`, `set_struct_body`) are
//! module-structural: a `PatchBody` pass's `done()` floor claims that everything
//! outside this function's body is preserved, and a fresh global falsifies that
//! claim while the report keeps making it.
//!
//! Mechanism: `FnPatch::module_mut` — the `&Module<Unverified>` token that
//! carries those declarations — is `pub(crate)` (`pass_context.rs`). Only
//! `ModRewrite::module_mut` stays public, and that rung's floor is already
//! `none()` (nothing preserved), so there is nothing left for a declaration to
//! falsify. The narrowing costs a pass author nothing: types come from
//! `patch.module()`, a positioned builder from `patch.builder_at(ip)`, and
//! instruction construction from `IRBuilder::at_end(bb)`.
//!
//! Primary error: `error[E0624]: method `module_mut` is private`.

use llvmkit_ir::{FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, PatchBody};

struct DeclareGlobalFromPatchBody;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for DeclareGlobalFromPatchBody {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "declare-global-from-patch-body";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        let patch = cx.mutate();
        // The neutral half compiles: a type comes off the read-only view.
        let i32_ty = patch.module().i32_type();
        // The structural half does not: `module_mut` is `pub(crate)`, so this
        // rung cannot declare a global and then report the body-level floor.
        patch.module_mut().add_global("g", i32_ty.const_zero())?;
        Ok(patch.done())
    }
}

fn main() {}
