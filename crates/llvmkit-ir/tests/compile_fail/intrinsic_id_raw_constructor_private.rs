//! llvmkit-specific compile-fail (Doctrine D3): generated intrinsic IDs are
//! validated handles, not externally-mintable raw integers.
//!
//! Closest upstream behaviour: `llvm/include/llvm/IR/Intrinsics.h::ID` is a
//! closed generated enum. llvmkit exposes a checked newtype, so external code
//! must use `IntrinsicId::from_raw` or generated lookup rather than constructing
//! unchecked IDs.

use llvmkit_ir::IntrinsicId;

fn main() {
    let _id = IntrinsicId(core::num::NonZeroU32::new(1).unwrap());
}
