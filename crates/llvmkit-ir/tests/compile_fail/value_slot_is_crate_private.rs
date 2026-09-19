//! Compile-fail lock (error-surface cleanup Task 24 fix round 1, D7): no value
//! slot is nameable or readable outside llvmkit. A slot carries no module tag
//! and means something only in the module that minted it; a caller holding
//! one could carry it into another module, or hand it back with another
//! module's tag. `ValueSlot` is crate-private like `TypeSlot`, so it cannot be
//! named, and the argument list the hidden `CallArgs::lower` produces — the
//! last public route that handed slots out — keeps them in a private field.

use llvmkit_ir::{CallArgs, Module};

fn main() {
    let _: Option<llvmkit_ir::ValueSlot> = None;
    let m = Module::dynamic("lock");
    let lowered = <(i32,) as CallArgs<'_, (i32,), _>>::lower((1_i32,), (&m).into())
        .expect("one literal argument lowers");
    let _ = lowered.0;
}
