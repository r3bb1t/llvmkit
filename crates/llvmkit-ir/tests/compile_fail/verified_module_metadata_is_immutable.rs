//! 0.0.4 cycle E compile-fail (Doctrine D8, D2).
//!
//! `verify(self)` consumes mutation capability. Every mutator in the crate
//! demands a `&Module<B, Unverified>` token, so once the module has been
//! consumed into `Module<B, Verified>` there is no token left to hand one —
//! the re-verify obligation is enforced by the type checker rather than by a
//! convention the caller may forget.
//!
//! Instruction *metadata* was the one mutator that had escaped this rule:
//! `set_metadata` / `push_debug_record` took no token, so a `Verified` module's
//! printed IR could be changed through a read-only `InstructionView` with the
//! typestate still claiming the module had been verified. The metadata setters
//! on `FunctionValue` and `GlobalVariable` already required the token; only the
//! instruction pair did not. This fixture locks the repaired rule.
//!
//! It also closes the pass-API leg of the same hole. An `Inspect`-rung pass is
//! handed only read-only views and never an `Unverified` token, so with the
//! token in the signature an inspect-only pass can no longer rewrite `!dbg`
//! attachments while the driver derives `Module<B, Verified>` and reports
//! everything preserved.
//!
//! Upstream has no analogue: `Instruction::setMetadata` is a plain non-const
//! method, `verifyModule` is a free function returning a bool a caller may
//! ignore, and nothing connects the two.

use llvmkit_ir::{Linkage, MetadataAttachmentKind, Module};

fn main() {
    let m = Module::dynamic("m");
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = llvmkit_ir::IrBuilder::at_end(entry);
    b.build_ret_void();
    let node = m.metadata_string("attached");

    // Consumes the `Unverified` token: `m` is moved into `verify`.
    let verified = m.verify().unwrap();

    let view = verified.as_view();
    let function = view.functions().next().unwrap();
    let block = function.basic_blocks().next().unwrap();
    let inst = block.instructions().next().unwrap();

    // There is no `&Module<B, Unverified>` left in scope to pass, and the
    // `Verified` module cannot supply one — so this call cannot be written.
    inst.set_metadata(&verified, MetadataAttachmentKind::Dbg, node);
}
