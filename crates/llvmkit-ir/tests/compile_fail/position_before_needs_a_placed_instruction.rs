//! Compile-fail lock (error-surface cleanup Task 24, D1): the entries that
//! insert or move relative to an instruction take a `PlacedInstruction`, so an
//! instruction that was never in a block cannot reach them at all. A plain
//! `InstructionView` does not compile (`E0308`). Mint the witness with
//! `InstructionView::placed` (checked, `Option`) or `Instruction::placed`
//! (total: the `Attached` typestate is only minted in a block).

use llvmkit_ir::{Dyn, IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("lock");
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m
        .add_function_dyn("f", fn_ty, Linkage::External)
        .expect("function");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let builder = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let (_block, ret) = builder.ret(i32_ty.const_zero()).expect("ret");
    let view = ret.as_view();
    let _ = IrBuilder::new_for::<Dyn>(&m).position_before(&view);
}
