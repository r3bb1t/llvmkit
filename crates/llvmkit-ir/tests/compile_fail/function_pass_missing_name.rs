//! llvmkit-specific capability-graded pass API lock (Doctrine D1), not a 1:1 LLVM port.
//!
//! The `#[function_pass]` attribute macro maps `name = "..."` onto the trait's
//! `const NAME`. A pass with no name has no instrumentation identity, so the
//! macro front-end (`pass_macro_shared.rs`) rejects an attribute that omits
//! `name` with our own `syn::Error` BEFORE it ever emits the `FunctionPass`
//! impl. The message is one we own, so it does not drift across rustc versions.
//!
//! Primary error: our `compile_error!` "missing `name = \"...\"`; a pass must
//! declare its `NAME`".

use llvmkit_ir::function_pass;

struct MissingName;

// No `name = "..."`: the macro rejects this before emitting any impl. The
// `cx: FnCx<Self>` / `-> IrResult<..>` here are the macro's readability
// sentinels (full-path so the fixture needs no extra imports), never resolved.
#[function_pass(access = Inspect)]
impl MissingName {
    fn run(
        &mut self,
        cx: llvmkit_ir::FnCx<Self>,
    ) -> llvmkit_ir::IrResult<llvmkit_ir::FnReport> {
        Ok(cx.done())
    }
}

fn main() {}
