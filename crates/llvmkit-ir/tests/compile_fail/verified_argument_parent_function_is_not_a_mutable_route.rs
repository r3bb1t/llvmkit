//! llvmkit typestate compile-fail (D1, D8): an argument hands back its parent
//! function at the argument's own capability, so a `ReadOnly` walk cannot hop
//! from an operand that is an argument to that function's blocks and reach a
//! mutator.
//!
//! The walk below is a verified module's: its instructions are `ReadOnly`, and
//! so is the operand one erases to and the `Argument` that operand narrows to.
//! `Argument::parent_function` keeps that capability, so the function's entry
//! block and its instructions are `ReadOnly` too, and the tokenless
//! `set_fast_math_flags` is refused by `CanMutate`'s own message. While
//! functions carried no capability, `parent_function` handed out a `Mutable`
//! function and this program compiled.
//!
//! The program is never run: the function holds no phi, and only the route
//! has to type-check for the law to be tested.
//!
//! No upstream counterpart: `Argument::getParent` returns a plain
//! `Function *` and LLVM has no verified-module typestate.
use llvmkit_ir::{
    Argument, Dyn, FastMathFlags, InstructionKind, IrBuilder, Linkage, Module, PhiKind, User,
};

fn main() {
    let m = Module::dynamic("m");
    let f32_ty = m.f32_type();
    let f = m
        .add_function_dyn(
            "f",
            m.function_type(f32_ty, [f32_ty.as_type()]),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let x = m.view(f).param(0).expect("x");
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .ret(x.as_erased())
        .expect("ret");
    let m = m.verify().expect("verifies");
    for instruction in m
        .view(f)
        .entry_block()
        .expect("a definition")
        .instructions()
    {
        let Some(operand) = instruction.operand(0) else {
            continue;
        };
        let Ok(argument) = Argument::try_from(operand) else {
            continue;
        };
        let parent = argument.parent_function();
        for inner in parent.entry_block().expect("a definition").instructions() {
            if let Some(InstructionKind::Phi(PhiKind::Fp(phi))) = inner.kind() {
                phi.set_fast_math_flags(FastMathFlags::fast()).expect("set");
            }
        }
    }
}
