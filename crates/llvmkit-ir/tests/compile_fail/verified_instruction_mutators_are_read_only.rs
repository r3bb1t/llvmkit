//! llvmkit typestate compile-fail (D1, D8): an instruction mutator is refused
//! on a `ReadOnly` handle by the handle's own capability.
//!
//! The walk below is the one an `Inspect` pass receives — a `ModuleView`'s
//! blocks — over a `Module<B, Verified>`, so every instruction it reaches is
//! `ReadOnly`, and so is every handle `kind()` and `terminator_kind()` narrow
//! it to. Each call below is to a mutator whose only guard is its
//! `where C: CanMutate` bound:
//!
//! - nothing else refuses the call: `set_fast_math_flags` takes no token, and
//!   the others accept a second unverified module of the same brand as their
//!   token, which type-checks;
//! - their bodies need no `Mutable` reference, so the crate still builds when
//!   one of these bounds is dropped.
//!
//! So this fixture is what fails if a bound goes: one error per call, each
//! `CanMutate`'s own message.
//!
//! `set_metadata` and the float phi's `set_fast_math_flags` hold the same law
//! in fixtures of their own (`verified_instruction_metadata_is_read_only.rs`,
//! `verified_phi_fast_math_flags_are_immutable.rs`).
//!
//! The program is never run: the module is empty, and only the calls have to
//! type-check for the law to be tested.
//!
//! No upstream counterpart: upstream's matching setters,
//! `CallInst::setTailCallKind`, `CallBase::setAttributes` and
//! `Instruction::setFastMathFlags` among them, are plain non-const methods,
//! and LLVM has no verified-module typestate.
use llvmkit_ir::{
    CallAttributeData, FastMathFlags, InstructionKind, Module, PhiKind, TailCallKind,
    TerminatorKind,
};

fn main() {
    // A second unverified module of the same brand: its token type-checks.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m").verify().expect("verifies");
    for function in m.as_view().functions() {
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                if let Some(record) = instruction.debug_records().next() {
                    instruction.push_debug_record(&other, record).expect("push");
                }
                match instruction.kind() {
                    Some(InstructionKind::Call(call)) => {
                        call.set_tail_call_kind(&other, TailCallKind::Tail);
                        call.set_attributes(&other, CallAttributeData::default());
                    }
                    Some(InstructionKind::AtomicRmw(rmw)) => {
                        rmw.set_value_operand(&other, rmw.value_operand())
                            .expect("set");
                    }
                    Some(InstructionKind::Phi(PhiKind::Int(phi))) => {
                        phi.set_fast_math_flags(FastMathFlags::fast())
                            .expect("set");
                    }
                    Some(InstructionKind::Phi(PhiKind::Other(phi))) => {
                        phi.set_fast_math_flags(FastMathFlags::fast())
                            .expect("set");
                    }
                    _ => {}
                }
                match instruction.terminator_kind() {
                    Some(TerminatorKind::Invoke(invoke)) => {
                        invoke.set_attributes(&other, CallAttributeData::default());
                    }
                    Some(TerminatorKind::CallBr(callbr)) => {
                        callbr.set_attributes(&other, CallAttributeData::default());
                    }
                    _ => {}
                }
            }
        }
    }
}
