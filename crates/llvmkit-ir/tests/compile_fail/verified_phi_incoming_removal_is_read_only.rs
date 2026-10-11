//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` phi cannot drop an
//! incoming edge.
//!
//! The walk below is the one an `Inspect` pass receives, a `ModuleView`'s
//! blocks, over a `Module<B, Verified>`, so every phi it reaches is
//! `ReadOnly`. `remove_incoming` on `PhiKind` and on each of the four phi
//! handles carries `where C: CanMutate`, and each call is refused by
//! `CanMutate`'s own message. The token is a second unverified module of the
//! same brand, which type-checks, so nothing but the phi's capability refuses
//! the calls.
//!
//! The per-phi bodies call a crate-private helper that carries the same bound,
//! and `PhiKind`'s dispatches to them, so dropping one bound alone breaks the
//! crate's build. This fixture is what fails if the bounds go and the bodies
//! are routed around them.
//!
//! The program is never run: the module is empty, and only the calls have to
//! type-check for the law to be tested.
//!
//! No upstream counterpart: `PHINode::removeIncomingValue` is a plain
//! non-const method, and LLVM has no verified-module typestate.
use llvmkit_ir::{InstructionKind, Module, PhiKind};

fn main() {
    // A second unverified module of the same brand: its token type-checks.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m").verify().expect("verifies");
    for function in m.as_view().functions() {
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                let Some(InstructionKind::Phi(phi)) = instruction.kind() else {
                    continue;
                };
                let _ = phi.remove_incoming(&other, 0);
                match phi {
                    PhiKind::Int(phi) => {
                        let _ = phi.remove_incoming(&other, 0);
                    }
                    PhiKind::Fp(phi) => {
                        let _ = phi.remove_incoming(&other, 0);
                    }
                    PhiKind::Ptr(phi) => {
                        let _ = phi.remove_incoming(&other, 0);
                    }
                    PhiKind::Other(phi) => {
                        let _ = phi.remove_incoming(&other, 0);
                    }
                }
            }
        }
    }
}
