//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! A copyable `Value` must not convert into a fresh linear
//! `Instruction<Attached>` lifecycle handle.

use llvmkit_ir::{Dyn, IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("value-remint");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type<_>>::new(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let add = b
        .build_int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "sum")
        .unwrap();

    let _inst: llvmkit_ir::Instruction<_, _> = b.view(add).into_erased().try_into().unwrap();
}
