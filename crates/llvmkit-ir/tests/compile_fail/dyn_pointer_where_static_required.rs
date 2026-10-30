//! llvmkit-specific compile-fail (D3): `ptr_type(n)` takes its address space at
//! run time, so its type is `SpaceDyn`; it does not silently become a
//! `Space<0>` type. The checked way across is `TryFrom`.

use llvmkit_ir::{AddressSpace, Module, PointerType, Space};

fn main() {
    let m = Module::dynamic("c");
    let _: PointerType<'_, Space<0>, _> = m.ptr_type(AddressSpace::DEFAULT);
}
