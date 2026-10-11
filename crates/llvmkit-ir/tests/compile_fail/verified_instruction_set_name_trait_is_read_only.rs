//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` instruction cannot be
//! renamed through the `SetName` trait either.
//!
//! The walk below is the one an `Inspect` pass receives, a `ModuleView`'s
//! blocks, over a `Module<B, Verified>`, so every instruction it reaches is
//! `ReadOnly`. `SetName` is implemented for `InstructionView` only where
//! `C: CanMutate`, so each trait-qualified call is refused by `CanMutate`'s
//! own message. The token is a second unverified module of the same brand,
//! which type-checks, so nothing but the view's capability refuses the calls.
//!
//! The impl's bodies call the inherent `InstructionView::set_name` /
//! `clear_name`, which carry the same bound, so widening the impl alone breaks
//! the crate's build; the inherent methods have their own fixture,
//! `verified_instruction_name_is_read_only.rs`. This fixture is what fails if
//! the impl is widened and its bodies are routed around the inherent methods,
//! which keep their bound.
//!
//! The program is never run: the module is empty, and only the calls have to
//! type-check for the law to be tested.
//!
//! No upstream counterpart: `Value::setName` is a plain non-const method, and
//! LLVM has no verified-module typestate.
use llvmkit_ir::{Module, SetName};

fn main() {
    // A second unverified module of the same brand: its token type-checks.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m").verify().expect("verifies");
    for function in m.as_view().functions() {
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                SetName::set_name(instruction, &other, "renamed");
                SetName::clear_name(instruction, &other);
            }
        }
    }
}
