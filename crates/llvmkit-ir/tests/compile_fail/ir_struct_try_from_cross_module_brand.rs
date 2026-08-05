//! llvmkit derive compile-fail (Doctrine D7).
//!
//! Closest upstream behaviour: LLVM rejects cross-context value mixing at
//! runtime. TryFrom<Argument> preserves llvmkit's module brand statically.
//!
//! The two modules are separated by *named brand types*, so the rejection is a
//! plain type mismatch (`Left` vs `Right`) rather than a region error.

use llvmkit_ir::{IrBuilder, IrError, IrStruct, Linkage, Module, ModuleBrand};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Left;
impl ModuleBrand for Left {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Right;
impl ModuleBrand for Right {}

fn main() -> Result<(), IrError> {
    let left = Module::branded::<Left, _>("left")?;
    let point_ty = <Point as llvmkit_ir::StructSchema>::ir_type(left.as_view())?;
    let fn_ty = left.fn_type(left.void_type(), [point_ty.as_type()], false);
    let left_fn = left.add_function_dyn("left", fn_ty, Linkage::External)?;
    let left_point = PointValue::try_from(left.view(left_fn).param(0)?)?;

    let right = Module::branded::<Right, _>("right")?;
    let right_fn = right
        .add_typed_function::<(), (), _>("right", Linkage::External)?
        .as_function();
    let entry = right.view(right_fn).append_basic_block(&right, "entry");
    let builder = IrBuilder::new_for::<()>(&right).position_at_end(entry);
    let _ = builder.build_insert_field::<Point, i32, _, _, _>(left_point, 1_i32, 0, "wrong_module")?;
    builder.build_ret_void();

    Ok(())
}
