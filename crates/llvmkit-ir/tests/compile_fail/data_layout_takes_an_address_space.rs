//! llvmkit-specific compile-fail (D10): a `DataLayout` query takes an
//! `AddressSpace`, so a number that was never checked against
//! `Type::setSubclassData`'s 24 bits does not reach it (`E0308`). Upstream's
//! `DataLayout::getPointerSizeInBits(unsigned AS)` takes any `unsigned` and
//! answers for address space 0 when it has no spec for that number.

use llvmkit_ir::DataLayout;

fn main() {
    let _ = DataLayout::default().pointer_size_in_bits(1);
}
