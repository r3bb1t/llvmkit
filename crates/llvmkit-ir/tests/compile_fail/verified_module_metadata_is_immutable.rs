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
//! The law held here is the token alone. Since blocks and instructions carry
//! their capability (D8), a view a verified module or a `ModuleView` walk
//! mints is `ReadOnly`, and `set_metadata`'s `C: CanMutate` bound refuses it
//! before the token is looked at — `verified_instruction_metadata_is_read_only.rs`
//! holds that law. So the view here comes from a second, unverified module of
//! the same brand: it is `Mutable`, its capability does not refuse the call,
//! and the one refusal left is the `Verified` module offered as the
//! `Unverified` token (E0308).
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
    b.ret_void();
    let node = m.metadata_string("attached");

    // Consumes the `Unverified` token: `m` is moved into `verify`.
    let verified = m.verify().unwrap();

    // A `Mutable` view, from an unverified module of the same brand, so only
    // the token can refuse the call below.
    let n = Module::dynamic("n");
    let g = n
        .add_typed_function::<(), (), _>("g", Linkage::External)
        .unwrap()
        .as_function();
    let n_entry = n.view(g).append_basic_block(&n, "entry");
    llvmkit_ir::IrBuilder::at_end(n_entry).ret_void();
    let inst = n
        .view(g)
        .entry_block()
        .unwrap()
        .instructions()
        .next()
        .unwrap();

    // The `Verified` module cannot stand in for the `Unverified` token — so
    // this call cannot be written with it.
    inst.set_metadata(&verified, MetadataAttachmentKind::Dbg, node);
}
