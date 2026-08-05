//! llvmkit derive compile-fail (Doctrine D7).
//!
//! Closest upstream behaviour: LLVM rejects cross-context value mixing at
//! runtime. The generated wrapper preserves llvmkit's module brand statically.
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
    let left_fn = left.add_typed_function::<(), (Point,), _>("left", Linkage::External)?;
    let (left_point,) = left.view(left_fn).params();

    let right = Module::branded::<Right, _>("right")?;
    let right_fn = right
        .add_typed_function::<(), (), _>("right", Linkage::External)?
        .as_function();
    let entry = right.view(right_fn).append_basic_block(&right, "entry");
    let builder = IrBuilder::new_for::<()>(&right).position_at_end(entry);
    let _ = builder.insert_field::<Point, i32, _, _, _>(left_point, 1_i32, 0, "wrong_module")?;
    builder.ret_void();

    Ok(())
}
