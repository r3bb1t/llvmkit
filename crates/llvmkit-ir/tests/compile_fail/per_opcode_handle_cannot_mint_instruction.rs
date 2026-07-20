//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Copyable per-opcode handles are read-only views. They must not mint a fresh
//! linear `Instruction<Attached>` lifecycle handle.

use llvmkit_ir::{Dyn, IRBuilder, InstructionKind, InstructionView, Linkage, Module};

fn main() {
    Module::with_new("per-opcode-remint", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let add_value = b
            .build_int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "sum")
            .unwrap();
        let view = InstructionView::try_from(add_value.as_value()).unwrap();

        if let Some(InstructionKind::Add(add)) = view.kind() {
            let _inst = add.as_instruction();
        }
    });
}
