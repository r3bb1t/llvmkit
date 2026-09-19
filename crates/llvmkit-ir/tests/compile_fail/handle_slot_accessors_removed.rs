//! Compile-fail lock (error-surface cleanup Task 24, D7): no public route
//! hands out a value or type handle's bare arena slot. A slot carries no
//! module tag and means something only in the module that minted it, so a
//! caller holding one could carry it into another module and have it read
//! there as whatever sits at that index. `Type::id` and `Value::slot` are
//! gone (`E0599`), and `TypeSlot` cannot be named outside the crate. A value
//! handle's storable form is its module-tagged `id()`; a `Type` handle is
//! itself what to compare and store, since its equality includes the module.

use llvmkit_ir::Module;

fn main() {
    let m = Module::dynamic("lock");
    let i32_ty = m.i32_type().as_type();
    let _ = i32_ty.id();
    let value = m.i32_type().const_int(1i32).as_erased();
    let _ = value.slot();
    let _: Option<llvmkit_ir::TypeSlot> = None;
}
