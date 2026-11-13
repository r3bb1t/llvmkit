//! llvmkit typestate compile-fail (D10) — no upstream counterpart: a C++
//! `PointerType *` carries its address space only at run time, and
//! `Type::setSubclassData` asserts it fits.
//!
//! A pointer type no parser reads back must not compile. This also reaches the
//! typed signatures and struct fields, which mint through `ptr_type_in`.
use llvmkit_ir::Module;

fn main() {
    let m = Module::dynamic("m");
    let _ = m.ptr_type_in::<16777216>();
}
