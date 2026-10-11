//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` call site is not a
//! `CallBase`, so neither of the trait's mutators reaches it.
//!
//! The walk below is the one an `Inspect` pass receives, a `ModuleView`'s
//! blocks, over a `Module<B, Verified>`, so every call site it reaches is
//! `ReadOnly`. `CallBase` is implemented for the call, invoke and `callbr`
//! handles only where `C: CanMutate`, so each trait-qualified call to
//! `with_operand_bundles` and `set_attributes` is refused by `CanMutate`'s own
//! message. The token is a second unverified module of the same brand, which
//! type-checks, so nothing but the call site's capability refuses the calls.
//!
//! Each impl's bodies call the handle's inherent `with_operand_bundles` /
//! `set_attributes`, which carry the same bound, so widening an impl alone
//! breaks the crate's build; the inherent methods have fixtures of their own,
//! `verified_call_operand_bundles_are_read_only.rs` and
//! `verified_instruction_mutators_are_read_only.rs`. This fixture is what fails
//! if an impl is widened and its bodies are routed around the inherent
//! methods, which keep their bound.
//!
//! The program is never run: the module is empty, and only the calls have to
//! type-check for the law to be tested.
//!
//! No upstream counterpart: `CallBase::setAttributes` and
//! `CallBase::Create(CallBase *, ArrayRef<OperandBundleDef>, InsertPosition)`
//! take a plain `CallBase *`, and LLVM has no verified-module typestate.
use llvmkit_ir::{
    CallAttributeData, CallBase, DynBrand, InstructionKind, Module, OperandBundleDef,
    TerminatorKind,
};

fn main() {
    // A second unverified module of the same brand: its token type-checks.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m").verify().expect("verifies");
    for function in m.as_view().functions() {
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                if let Some(InstructionKind::Call(call)) = instruction.kind() {
                    let _ = CallBase::with_operand_bundles(
                        call,
                        &other,
                        Vec::<OperandBundleDef<'_, DynBrand>>::new(),
                    );
                    CallBase::set_attributes(call, &other, CallAttributeData::default());
                }
                match instruction.terminator_kind() {
                    Some(TerminatorKind::Invoke(invoke)) => {
                        let _ = CallBase::with_operand_bundles(
                            invoke,
                            &other,
                            Vec::<OperandBundleDef<'_, DynBrand>>::new(),
                        );
                        CallBase::set_attributes(invoke, &other, CallAttributeData::default());
                    }
                    Some(TerminatorKind::CallBr(callbr)) => {
                        let _ = CallBase::with_operand_bundles(
                            callbr,
                            &other,
                            Vec::<OperandBundleDef<'_, DynBrand>>::new(),
                        );
                        CallBase::set_attributes(callbr, &other, CallAttributeData::default());
                    }
                    _ => {}
                }
            }
        }
    }
}
