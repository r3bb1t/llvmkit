use llvmkit_ir::{IRBuilder, Linkage, Module, Type};

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let left_value = left.i64_type().const_int(1_i64);
        Module::with_new::<_, _, _>("right", |right| {
            let i64_ty = right.i64_type();
            let params = Vec::<Type<'_, _>>::new();
            let fn_ty = right.fn_type(i64_ty.as_type(), params, false);
            let function = right.add_function::<i64, _>("f", fn_ty, Linkage::External).unwrap();
            let entry = function.append_basic_block(&right, "entry");
            let builder = IRBuilder::new_for::<i64>(&right).position_at_end(entry);
            let _ = builder.build_int_add(left_value, left_value, "bad");
        });
    });
}
