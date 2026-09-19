//! Compile-fail lock (error-surface cleanup Task 24 fix rounds 1 and 2, D7):
//! no value slot is nameable or readable outside llvmkit. A slot carries no
//! module tag and means something only in the module that minted it; a
//! caller holding one could carry it into another module, or hand it back
//! with another module's tag. `ValueSlot` is crate-private like `TypeSlot`, so
//! it cannot be named. The hidden `CallArgs::lower` — the last public route
//! that handed slots out, and one that interns constants into any module it
//! is handed, a `Verified` one included — takes the crate-only token, so the
//! call itself does not compile outside the crate (`E0061`).

use llvmkit_ir::{CallArgs, Module};

fn main() {
    let _: Option<llvmkit_ir::ValueSlot> = None;
    let m = Module::dynamic("lock");
    let _lowered = <(i32,) as CallArgs<'_, (i32,), _>>::lower((1_i32,), (&m).into());
}
