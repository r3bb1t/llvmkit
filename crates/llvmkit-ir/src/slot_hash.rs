//! A hasher for the crate's own slot keys.
//!
//! The struct-layout maps, the instruction and block lists and the use lists
//! all key a map by slots the crate assigned in sequence ([`ValueSlot`],
//! [`TypeSlot`], [`ModuleId`], and the edges built of them), never by values
//! an input chooses. The collision resistance of the default SipHash buys
//! nothing for such keys, and these lookups sit on hot paths: every struct a
//! size walk reaches, every instruction inserted or erased, every use added
//! or dropped.
//!
//! [`ValueSlot`]: crate::value::ValueSlot
//! [`TypeSlot`]: crate::r#type::TypeSlot
//! [`ModuleId`]: crate::module::ModuleId

use std::collections::HashMap;

/// One multiply per word: Fibonacci hashing, whose odd multiplier mixes the
/// high bits well.
#[derive(Default)]
pub(crate) struct SlotHasher(u64);

impl core::hash::Hasher for SlotHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(u64::from(*byte));
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// A map keyed by crate-assigned slots, hashed with [`SlotHasher`].
pub(crate) type SlotMap<K, V> = HashMap<K, V, core::hash::BuildHasherDefault<SlotHasher>>;
