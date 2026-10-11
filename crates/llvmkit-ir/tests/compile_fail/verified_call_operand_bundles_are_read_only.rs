//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` call site cannot be
//! rebuilt with new operand bundles.
//!
//! The walk below is the one an `Inspect` pass receives, a `ModuleView`'s
//! blocks, over a `Module<B, Verified>`, so every call site it reaches is
//! `ReadOnly`. `with_operand_bundles` on the call, invoke and `callbr` handles
//! carries `where C: CanMutate`, and each call is refused by `CanMutate`'s own
//! message. The token is a second unverified module of the same brand, which
//! type-checks, so nothing but the call site's capability refuses the calls.
//!
//! Each body creates the new call site through `proven_mutable()`, which the
//! bound licenses, so dropping the bound alone breaks the crate's build. This
//! fixture is what fails if the bound goes and the body is routed around it.
//!
//! The program is never run: the module is empty, and only the calls have to
//! type-check for the law to be tested.
//!
//! No upstream counterpart: `CallBase::Create(CallBase *, ArrayRef<OperandBundleDef>,
//! InsertPosition)` takes a plain `CallBase *`, and LLVM has no
//! verified-module typestate.
use llvmkit_ir::{DynBrand, InstructionKind, Module, OperandBundleDef, TerminatorKind};

fn main() {
    // A second unverified module of the same brand: its token type-checks.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m").verify().expect("verifies");
    for function in m.as_view().functions() {
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                if let Some(InstructionKind::Call(call)) = instruction.kind() {
                    let _ = call.with_operand_bundles(
                        &other,
                        Vec::<OperandBundleDef<'_, DynBrand>>::new(),
                    );
                }
                match instruction.terminator_kind() {
                    Some(TerminatorKind::Invoke(invoke)) => {
                        let _ = invoke.with_operand_bundles(
                            &other,
                            Vec::<OperandBundleDef<'_, DynBrand>>::new(),
                        );
                    }
                    Some(TerminatorKind::CallBr(callbr)) => {
                        let _ = callbr.with_operand_bundles(
                            &other,
                            Vec::<OperandBundleDef<'_, DynBrand>>::new(),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}
