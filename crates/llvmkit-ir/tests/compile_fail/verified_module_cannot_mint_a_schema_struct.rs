//! llvmkit typestate compile-fail (D8) — no upstream counterpart: LLVM has no
//! verified-module typestate.
//!
//! `ModuleView::get_or_insert_struct_of` could set a body through a read-only
//! view, so a verified module's `%Point = type opaque` printed differently
//! after `verify()`. Only an unverified module mints a schema's struct now.
use llvmkit_ir::{IrStruct, Module};

#[derive(IrStruct)]
struct Point {
    x: i32,
}

fn main() {
    let m = Module::dynamic("m");
    m.opaque_struct("Point").expect("a fresh name");
    let verified = m.verify().expect("verifies");
    let _ = verified.as_view().get_or_insert_struct_of::<Point>();
    let _ = verified.get_or_insert_struct_of::<Point>();
}
