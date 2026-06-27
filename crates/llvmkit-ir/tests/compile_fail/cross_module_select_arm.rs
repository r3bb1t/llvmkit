//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitSelectInst` in
//! `lib/IR/Verifier.cpp` checks select operand type consistency at runtime, and
//! `Verifier::visitGlobalValue` rejects values referenced from a different module.
//! llvmkit pushes the module-provenance part into the Rust type system: both
//! select arms must carry the builder module's brand.

use llvmkit_ir::{IRBuilder, Linkage, Module, Type};

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let i32_ty = left.i32_type();
        let params = Vec::<Type<'_, _>>::new();
        let fn_ty = left.fn_type(i32_ty.as_type(), params, false);
        let f = left.add_function::<i32, _>("left_f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&left, "entry");
        let left_builder = IRBuilder::new_for::<i32>(&left).position_at_end(entry);
        let left_arm = left_builder
            .build_int_add(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "left")
            .unwrap();
        Module::with_new::<_, _, _>("right", |right| {
            let i1_ty = right.bool_type();
            let i32_ty = right.i32_type();
            let cond = i1_ty.const_int(true);
            let params = Vec::<Type<'_, _>>::new();
            let fn_ty = right.fn_type(i32_ty.as_type(), params, false);
            let f = right.add_function::<i32, _>("f", fn_ty, Linkage::External).unwrap();
            let entry = f.append_basic_block(&right, "entry");
            let builder = IRBuilder::new_for::<i32>(&right).position_at_end(entry);
            let right_arm = builder
                .build_int_add(i32_ty.const_int(3_i32), i32_ty.const_int(4_i32), "right")
                .unwrap();
            let _ = builder.build_select(cond, left_arm, right_arm, "bad");
        });
    });
}
