//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Upstream `StructType::setBody` (`lib/IR/Type.cpp`) asserts at
//! runtime that the body has not already been set. llvmkit pulls the
//! check forward: `set_struct_body_typed` consumes the
//! `StructType<Opaque>` handle, so a second call requires another
//! `StructType<Opaque>` -- which the type system does not provide.

use llvmkit_ir::Module;

fn main() {
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let opaque = m.opaque_struct("S").unwrap();
    let body_set = m
        .set_struct_body_typed(opaque, [i32_ty.as_type()], false)
        .unwrap();
    // `body_set` is `StructType<'_, BodySet>`, not `Opaque`; passing it
    // to `set_struct_body_typed` is a compile error.
    let _ = m.set_struct_body_typed(body_set, [i32_ty.as_type()], false);
}
