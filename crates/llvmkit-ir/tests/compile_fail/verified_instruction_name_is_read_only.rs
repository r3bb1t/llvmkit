//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` instruction cannot be
//! renamed.
//!
//! The walk below is the one an `Inspect` pass receives, a `ModuleView`'s
//! blocks, over a `Module<B, Verified>`, so every instruction it reaches is
//! `ReadOnly`. `InstructionView::set_name` and `clear_name` carry
//! `where C: CanMutate`, and each call is refused by `CanMutate`'s own
//! message. The token is a second unverified module of the same brand, which
//! type-checks, so nothing but the view's capability refuses the calls.
//!
//! The bodies rename through `Value::set_name` / `clear_name`, which carry the
//! same bound, so dropping the bound alone breaks the crate's build. This
//! fixture is what fails if the bound goes and the body is routed around it.
//!
//! The program is never run: the module is empty, and only the calls have to
//! type-check for the law to be tested.
//!
//! No upstream counterpart: `Value::setName` is a plain non-const method, and
//! LLVM has no verified-module typestate.
use llvmkit_ir::Module;

fn main() {
    // A second unverified module of the same brand: its token type-checks.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m").verify().expect("verifies");
    for function in m.as_view().functions() {
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                instruction.set_name(&other, "renamed");
                instruction.clear_name(&other);
            }
        }
    }
}
