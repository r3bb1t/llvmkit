//! MD5 message digest.
//!
//! Ports `llvm/Support/MD5.h` and `llvm/lib/Support/MD5.cpp`. llvmkit needs it
//! for exactly one reason: `GlobalValue::getGUIDAssumingExternalLinkage` is
//! defined as `MD5Hash(GlobalIdentifier)`, the low 64 bits of the MD5 digest of
//! a global's identifier, and the module summary index is keyed by that value.
//!
//! The constant tables below are the RFC 1321 sine table and per-round shift
//! amounts. They are not cross-checked by a drift test because the ported
//! upstream test vectors (`llvm/unittests/Support/MD5Test.cpp`) pin the whole
//! algorithm end to end, which is strictly stronger: a single wrong constant
//! changes every digest.

use std::fmt;

/// Per-round addition constants, `floor(2^32 * abs(sin(i + 1)))`.
const SINE_TABLE: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// Per-round left-rotation amounts.
const SHIFT_TABLE: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const BLOCK_LENGTH: usize = 64;

/// A finished MD5 digest.
///
/// Mirrors `MD5::MD5Result`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Md5Result {
    bytes: [u8; 16],
}

impl Md5Result {
    /// The digest bytes, in the order MD5 produces them.
    #[inline]
    #[must_use]
    pub fn bytes(&self) -> [u8; 16] {
        self.bytes
    }

    /// The low 64 bits of the digest.
    ///
    /// Mirrors `MD5::MD5Result::low`. The digest is little-endian, so the low
    /// word comes first.
    #[must_use]
    pub fn low(&self) -> u64 {
        let mut word = [0u8; 8];
        word.copy_from_slice(&self.bytes[..8]);
        u64::from_le_bytes(word)
    }

    /// The high 64 bits of the digest.
    ///
    /// Mirrors `MD5::MD5Result::high`.
    #[must_use]
    pub fn high(&self) -> u64 {
        let mut word = [0u8; 8];
        word.copy_from_slice(&self.bytes[8..]);
        u64::from_le_bytes(word)
    }
}

/// Renders the digest as 32 lowercase hexadecimal digits.
///
/// Mirrors `MD5::stringifyResult`.
impl fmt::Display for Md5Result {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.bytes {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// An incremental MD5 hasher.
///
/// Mirrors the `MD5` class.
#[derive(Clone, Debug)]
pub struct Md5 {
    state: [u32; 4],
    /// Total number of message bytes consumed so far.
    length: u64,
    buffer: [u8; BLOCK_LENGTH],
    buffered: usize,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    /// A hasher over the empty message.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            length: 0,
            buffer: [0; BLOCK_LENGTH],
            buffered: 0,
        }
    }

    /// Feeds more message bytes into the hasher.
    ///
    /// Mirrors `MD5::update`.
    pub fn update(&mut self, data: &[u8]) {
        self.length = self
            .length
            .wrapping_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        let mut rest = data;
        while !rest.is_empty() {
            let free = BLOCK_LENGTH - self.buffered;
            let take = free.min(rest.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&rest[..take]);
            self.buffered += take;
            rest = &rest[take..];
            if self.buffered == BLOCK_LENGTH {
                let block = self.buffer;
                process_block(&mut self.state, &block);
                self.buffered = 0;
            }
        }
    }

    /// Pads the message and produces the digest, consuming the hasher.
    ///
    /// Mirrors `MD5::final`, whose name is a Rust keyword. Consuming `self`
    /// spells the once-only padding step upstream leaves to convention.
    #[must_use]
    pub fn finish(mut self) -> Md5Result {
        let bit_length = self.length.wrapping_mul(8);

        self.buffer[self.buffered] = 0x80;
        self.buffered += 1;
        if self.buffered > BLOCK_LENGTH - 8 {
            self.buffer[self.buffered..].fill(0);
            let block = self.buffer;
            process_block(&mut self.state, &block);
            self.buffered = 0;
        }
        self.buffer[self.buffered..BLOCK_LENGTH - 8].fill(0);
        self.buffer[BLOCK_LENGTH - 8..].copy_from_slice(&bit_length.to_le_bytes());
        let block = self.buffer;
        process_block(&mut self.state, &block);

        let mut bytes = [0u8; 16];
        for (word, chunk) in self.state.iter().zip(bytes.chunks_exact_mut(4)) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        Md5Result { bytes }
    }

    /// The digest of the message so far, leaving the hasher usable.
    ///
    /// Mirrors `MD5::result`, which copies the context before padding it.
    #[must_use]
    pub fn result(&self) -> Md5Result {
        self.clone().finish()
    }

    /// The digest of a complete message.
    ///
    /// Mirrors the static `MD5::hash`.
    #[must_use]
    pub fn hash(data: &[u8]) -> Md5Result {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finish()
    }
}

/// The low 64 bits of the MD5 digest of `data`.
///
/// Mirrors the free function `llvm::MD5Hash`.
#[must_use]
pub fn md5_hash(data: &[u8]) -> u64 {
    Md5::hash(data).low()
}

fn process_block(state: &mut [u32; 4], block: &[u8; BLOCK_LENGTH]) {
    let mut message = [0u32; 16];
    for (word, chunk) in message.iter_mut().zip(block.chunks_exact(4)) {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(chunk);
        *word = u32::from_le_bytes(bytes);
    }

    let [mut a, mut b, mut c, mut d] = *state;
    for (round, (constant, shift)) in SINE_TABLE.iter().zip(SHIFT_TABLE).enumerate() {
        let (mixed, index) = if round < 16 {
            ((b & c) | (!b & d), round)
        } else if round < 32 {
            ((d & b) | (!d & c), (5 * round + 1) % 16)
        } else if round < 48 {
            (b ^ c ^ d, (3 * round + 5) % 16)
        } else {
            (c ^ (b | !d), (7 * round) % 16)
        };
        let rotated = a
            .wrapping_add(mixed)
            .wrapping_add(*constant)
            .wrapping_add(message[index])
            .rotate_left(shift);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(rotated);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

#[cfg(test)]
mod tests {
    use super::{Md5, md5_hash};

    /// Mirrors `MD5Test.cpp::TestMD5Sum`.
    fn assert_md5_sum(input: &[u8], expected: &str) {
        let mut hasher = Md5::new();
        hasher.update(input);
        assert_eq!(hasher.finish().to_string(), expected);
    }

    /// Ported from `llvm/unittests/Support/MD5Test.cpp` `TEST(MD5Test, MD5)`.
    #[test]
    fn md5_matches_upstream_vectors() {
        assert_md5_sum(b"", "d41d8cd98f00b204e9800998ecf8427e");
        assert_md5_sum(b"a", "0cc175b9c0f1b6a831c399e269772661");
        assert_md5_sum(
            b"abcdefghijklmnopqrstuvwxyz",
            "c3fcd3d76192e4007dfb496cca67e13b",
        );
        assert_md5_sum(b"\0", "93b885adfe0da089cdf634904fd59f71");
        assert_md5_sum(b"a\0", "4144e195f46de78a3623da7364d04f11");
        assert_md5_sum(
            b"abcdefghijklmnopqrstuvwxyz\0",
            "81948d1f1554f58cd1a56ebb01f808cb",
        );
    }

    /// Ported from `llvm/unittests/Support/MD5Test.cpp` `TEST(MD5HashTest, MD5)`.
    #[test]
    fn md5_hash_exposes_high_and_low_words() {
        let result = Md5::hash(b"abcdefghijklmnopqrstuvwxyz");
        assert_eq!(result.to_string(), "c3fcd3d76192e4007dfb496cca67e13b");
        assert_eq!(result.high(), 0x3be1_67ca_6c49_fb7d);
        assert_eq!(result.low(), 0x00e4_9261_d7d3_fcc3);
        assert_eq!(
            md5_hash(b"abcdefghijklmnopqrstuvwxyz"),
            0x00e4_9261_d7d3_fcc3
        );
    }

    /// Ported from `llvm/unittests/Support/MD5Test.cpp`
    /// `TEST(MD5Test, FinalAndResultHelpers)`: `result()` leaves the hasher
    /// usable where `final()` consumes it.
    #[test]
    fn md5_result_does_not_consume_the_hasher() {
        let mut hasher = Md5::new();
        hasher.update(b"abcd");

        let mut reference = Md5::new();
        reference.update(b"abcd");
        assert_eq!(hasher.result(), reference.finish());

        hasher.update(b"xyz");
        let mut reference = Md5::new();
        reference.update(b"abcdxyz");
        assert_eq!(hasher.finish(), reference.finish());
    }

    /// Message lengths that straddle the 64-byte block and the 56-byte padding
    /// boundary exercise every branch of the padding step. No upstream
    /// counterpart: `MD5Test.cpp` has no multi-block vector, so the expected
    /// digests are RFC 1321's own published test suite values.
    #[test]
    fn md5_spans_block_boundaries() {
        assert_md5_sum(b"message digest", "f96b697d7cb7938d525a2f31aaf161d0");
        assert_md5_sum(
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            "d174ab98d277d9f5a5611c2c9f419d9f",
        );
        assert_md5_sum(
            b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            "57edf4a22be3c955ac49da2e2107b67a",
        );
    }
}
