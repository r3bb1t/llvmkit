//! llvmkit derive compile-fail (Doctrine D7).
//!
//! Closest upstream behaviour: LLVM rejects cross-context value mixing at
//! runtime. The generated wrapper preserves llvmkit's module brand statically.

use llvmkit_ir::{IRBuilder, IrError, IrStruct, Linkage, Module};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

fn main() -> Result<(), IrError> {
    Module::with_new("left", |left| {
        let left_fn = left.add_typed_function::<(), (Point,), _>("left", Linkage::External)?;
        let (left_point,) = left_fn.params();

        Module::with_new("right", |right| {
            let void_ty = right.void_type();
            let fn_ty = right.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
            let right_fn = right.add_function::<(), _>("right", fn_ty, Linkage::External)?;
            let entry = right_fn.append_basic_block(&right, "entry");
            let builder = IRBuilder::new_for::<()>(&right).position_at_end(entry);
            let _ = builder.build_insert_field::<Point, i32, _, _, _>(
                left_point,
                1_i32,
                0,
                "wrong_module",
            )?;
            builder.build_ret_void();
            Ok(())
        })?;

        Ok(())
    })
}
