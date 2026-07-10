//! llvmkit-specific capability-graded pass API lock (Doctrine D1/D8), not a 1:1 LLVM port.
//!
//! The capability rungs are split across two levels: `FnAccess` names the rungs
//! usable over a single function body (`Inspect`/`PatchBody`/`ReshapeCfg`), while
//! `RewriteModule` is a module-only rung implementing `ModAccess`, NOT `FnAccess`
//! (`pass_access.rs`). A `FunctionPass` declaring `type Access = RewriteModule`
//! therefore fails the `FunctionPass::Access: FnAccess` bound: you cannot smuggle
//! a whole-module capability onto a function pass. Upstream LLVM has no such
//! level/capability distinction on its pass types.
//!
//! Primary error: `RewriteModule: FnAccess` is not satisfied (surfaced through
//! the `#[function_pass]`-generated `type Access = RewriteModule` and the
//! `FnCx<.., RewriteModule, ..>` context).

use llvmkit_ir::function_pass;

struct WrongLevel;

// `RewriteModule` is a `ModAccess` rung; used as a `FunctionPass` `Access` it
// fails the `FnAccess` bound. The `cx: FnCx<Self>` is the macro's readability
// sentinel (full-path so the fixture needs no extra imports).
#[function_pass(name = "wrong-level", access = RewriteModule)]
impl WrongLevel {
    fn run(
        &mut self,
        cx: llvmkit_ir::FnCx<Self>,
    ) -> llvmkit_ir::IrResult<llvmkit_ir::FnReport> {
        Ok(cx.done())
    }
}

fn main() {}
