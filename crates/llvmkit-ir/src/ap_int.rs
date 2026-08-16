//! Safe Rust port of LLVM's `APInt` value semantics.
//!
//! This mirrors the parts of `llvm/include/llvm/ADT/APInt.h` and
//! `llvm/lib/Support/APInt.cpp` that llvmkit's constants and folders need.

use core::cmp::Ordering;
use core::ops::{Add, BitAnd, BitOr, BitXor, Mul, Neg, Not, Sub};

use crate::{IrError, IrResult};

const WORD_BITS: u32 = 64;
const WORD_MASK_U128: u128 = 0xffff_ffff_ffff_ffff;
const TWO_64: u128 = 1u128 << 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signedness {
    Unsigned,
    Signed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApIntTruncation {
    RejectOverflow,
    Truncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApIntRounding {
    Down,
    TowardZero,
    Up,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApIntDivRem {
    quotient: ApInt,
    remainder: ApInt,
}

impl ApIntDivRem {
    #[inline]
    pub fn quotient(&self) -> &ApInt {
        &self.quotient
    }

    #[inline]
    pub fn remainder(&self) -> &ApInt {
        &self.remainder
    }

    #[inline]
    pub fn into_parts(self) -> (ApInt, ApInt) {
        (self.quotient, self.remainder)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ApInt {
    bit_width: u32,
    words: Box<[u64]>,
}

impl core::fmt::Debug for ApInt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ApInt")
            .field("bit_width", &self.bit_width)
            .field("words", &self.words)
            .finish()
    }
}

impl core::fmt::Display for ApInt {
    /// Print the value as **signed decimal**: the bit pattern is read as a
    /// two's-complement signed integer of this `ApInt`'s width, so a
    /// negative value prints with a leading `-`. The bit width itself is
    /// not printed. This is the representation the assembly writer uses
    /// for `i<N>` constant bodies, so it agrees byte-for-byte with the
    /// integer literal in module output.
    ///
    /// Use [`ApInt::to_string_radix`] for other radices or for an
    /// unsigned reading of the same bits.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.to_string_radix(10, Signedness::Signed))
    }
}

impl ApInt {
    pub fn new(
        bit_width: u32,
        value: u64,
        signedness: Signedness,
        truncation: ApIntTruncation,
    ) -> IrResult<Self> {
        if matches!(truncation, ApIntTruncation::RejectOverflow) {
            let fits = match signedness {
                Signedness::Unsigned => unsigned_u64_fits(bit_width, value),
                Signedness::Signed => signed_u64_pattern_fits(bit_width, value),
            };
            if !fits {
                return Err(IrError::ImmediateOverflow {
                    value: u128::from(value),
                    bits: bit_width,
                });
            }
        }

        if matches!(signedness, Signedness::Signed) && bit_width > WORD_BITS && (value >> 63) != 0 {
            let len = words_for_bits_usize(bit_width);
            let mut words = vec![u64::MAX; len];
            if let Some(first) = words.first_mut() {
                *first = value;
            }
            Ok(Self::from_words(bit_width, &words))
        } else {
            Ok(Self::from_words(bit_width, &[value]))
        }
    }

    pub fn from_words(bit_width: u32, words: &[u64]) -> Self {
        let mut v = Vec::with_capacity(words_for_bits_usize(bit_width));
        let limit = words_for_bits_usize(bit_width);
        for word in words.iter().take(limit) {
            v.push(*word);
        }
        canonicalize_words(bit_width, &mut v);
        Self {
            bit_width,
            words: v.into_boxed_slice(),
        }
    }

    pub fn from_string(bit_width: u32, text: &str, radix: u8) -> IrResult<Self> {
        if !matches!(radix, 2 | 8 | 10 | 16 | 36) {
            return Err(IrError::InvalidOperation {
                message: "APInt string radix must be 2, 8, 10, 16, or 36",
            });
        }
        let Some(rest) = text.strip_prefix('-') else {
            let digits = text.strip_prefix('+').unwrap_or(text);
            let words = parse_unsigned_words(digits, radix)?;
            let parsed_width = bits_used_in_words(&words);
            if parsed_width > bit_width {
                return Err(IrError::ImmediateOverflow {
                    value: u128::MAX,
                    bits: bit_width,
                });
            }
            return Ok(Self::from_words(bit_width, &words));
        };

        let words = parse_unsigned_words(rest, radix)?;
        let parsed_width = bits_used_in_words(&words);
        if parsed_width > bit_width {
            return Err(IrError::ImmediateOverflow {
                value: u128::MAX,
                bits: bit_width,
            });
        }
        Ok(Self::from_words(bit_width, &words).negate())
    }

    #[inline]
    pub fn zero(bit_width: u32) -> Self {
        Self::from_words(bit_width, &[])
    }

    #[inline]
    pub fn zero_width() -> Self {
        Self::zero(0)
    }

    #[inline]
    pub fn all_ones(bit_width: u32) -> Self {
        let len = words_for_bits_usize(bit_width);
        Self::from_words(bit_width, &vec![u64::MAX; len])
    }

    #[inline]
    pub fn max_value(bit_width: u32) -> Self {
        Self::all_ones(bit_width)
    }

    pub fn signed_max_value(bit_width: u32) -> Self {
        if bit_width == 0 {
            return Self::zero(0);
        }
        let mut v = Self::all_ones(bit_width);
        v.clear_bit(bit_width - 1);
        v
    }

    #[inline]
    pub fn min_value(bit_width: u32) -> Self {
        Self::zero(bit_width)
    }

    pub fn signed_min_value(bit_width: u32) -> Self {
        if bit_width == 0 {
            return Self::zero(0);
        }
        Self::one_bit_set(bit_width, bit_width - 1)
    }

    #[inline]
    pub fn sign_mask(bit_width: u32) -> Self {
        Self::signed_min_value(bit_width)
    }

    pub fn one_bit_set(bit_width: u32, bit: u32) -> Self {
        if bit >= bit_width {
            return Self::zero(bit_width);
        }
        let mut words = vec![0; words_for_bits_usize(bit_width)];
        let idx = u32_to_usize(bit / WORD_BITS);
        let shift = bit % WORD_BITS;
        if let Some(word) = words.get_mut(idx) {
            *word |= 1u64 << shift;
        }
        Self::from_words(bit_width, &words)
    }

    pub fn bits_set(bit_width: u32, lo: u32, hi: u32) -> Self {
        let mut out = Self::zero(bit_width);
        if lo >= hi || lo >= bit_width {
            return out;
        }
        let end = hi.min(bit_width);
        let mut bit = lo;
        while bit < end {
            out.set_bit(bit);
            bit += 1;
        }
        out
    }

    pub fn bits_set_with_wrap(bit_width: u32, lo: u32, hi: u32) -> Self {
        if lo < hi {
            Self::bits_set(bit_width, lo, hi)
        } else {
            ApInt::bitor(
                &Self::low_bits_set(bit_width, hi),
                &Self::high_bits_set(bit_width, bit_width.saturating_sub(lo)),
            )
        }
    }

    #[inline]
    pub fn bits_set_from(bit_width: u32, lo: u32) -> Self {
        Self::bits_set(bit_width, lo, bit_width)
    }

    #[inline]
    pub fn high_bits_set(bit_width: u32, count: u32) -> Self {
        Self::bits_set(bit_width, bit_width.saturating_sub(count), bit_width)
    }

    #[inline]
    pub fn low_bits_set(bit_width: u32, count: u32) -> Self {
        Self::bits_set(bit_width, 0, count.min(bit_width))
    }

    pub fn set_sign_bit(&mut self) {
        if self.bit_width == 0 {
            return;
        }
        self.set_bit(self.bit_width - 1);
    }

    pub fn clear_sign_bit(&mut self) {
        if self.bit_width == 0 {
            return;
        }
        self.clear_bit(self.bit_width - 1);
    }

    pub fn set_low_bits(&mut self, count: u32) {
        let mask = Self::low_bits_set(self.bit_width, count);
        let updated = ApInt::bitor(self, &mask);
        self.words = updated.words;
    }

    pub fn set_high_bits(&mut self, count: u32) {
        let mask = Self::high_bits_set(self.bit_width, count);
        let updated = ApInt::bitor(self, &mask);
        self.words = updated.words;
    }

    pub fn set_bits_from(&mut self, lo: u32) {
        let mask = Self::bits_set_from(self.bit_width, lo);
        let updated = ApInt::bitor(self, &mask);
        self.words = updated.words;
    }

    /// Set the bits in `[lo, hi)`. Mirrors `APInt::setBits`, which asserts
    /// `lo <= hi <= bit_width`; out-of-range requests are no-ops here rather
    /// than panics, matching the rest of this type's bit accessors.
    pub fn set_bits(&mut self, lo: u32, hi: u32) {
        if lo >= hi {
            return;
        }
        let mask = Self::bits_set(self.bit_width, lo, hi.min(self.bit_width));
        let updated = ApInt::bitor(self, &mask);
        self.words = updated.words;
    }

    /// Clear the bits in `[lo, hi)`. Mirrors `APInt::clearBits`.
    pub fn clear_bits(&mut self, lo: u32, hi: u32) {
        if lo >= hi {
            return;
        }
        let mask = Self::bits_set(self.bit_width, lo, hi.min(self.bit_width));
        let updated = ApInt::bitand(self, &mask.not());
        self.words = updated.words;
    }

    /// Clear the bottom `count` bits. Mirrors `APInt::clearLowBits`.
    pub fn clear_low_bits(&mut self, count: u32) {
        let keep = Self::high_bits_set(self.bit_width, self.bit_width.saturating_sub(count));
        let updated = ApInt::bitand(self, &keep);
        self.words = updated.words;
    }

    /// Clear the top `count` bits. Mirrors `APInt::clearHighBits`.
    pub fn clear_high_bits(&mut self, count: u32) {
        let keep = Self::low_bits_set(self.bit_width, self.bit_width.saturating_sub(count));
        let updated = ApInt::bitand(self, &keep);
        self.words = updated.words;
    }

    /// Set every bit to 1. Mirrors `APInt::setAllBits`.
    pub fn set_all_bits(&mut self) {
        self.words = Self::all_ones(self.bit_width).words;
    }

    /// Set every bit to 0. Mirrors `APInt::clearAllBits`.
    pub fn clear_all_bits(&mut self) {
        self.words = Self::zero(self.bit_width).words;
    }

    /// Toggle every bit. Mirrors `APInt::flipAllBits`.
    pub fn flip_all_bits(&mut self) {
        self.words = self.clone().not().words;
    }

    /// Toggle a single bit. Mirrors `APInt::flipBit`; out of range is a no-op.
    pub fn flip_bit(&mut self, bit: u32) {
        if self.bit(bit) {
            self.clear_bit(bit);
        } else {
            self.set_bit(bit);
        }
    }

    pub fn insert_bits(&mut self, src: &ApInt, bit_position: u32) {
        let mut bit = 0;
        while bit < src.bit_width {
            let Some(dst) = bit_position.checked_add(bit) else {
                break;
            };
            if dst >= self.bit_width {
                break;
            }
            self.clear_bit(dst);
            if src.bit(bit) {
                self.set_bit(dst);
            }
            bit += 1;
        }
    }

    pub fn extract_bits(&self, num_bits: u32, bit_position: u32) -> ApInt {
        let mut out = Self::zero(num_bits);
        let mut bit = 0;
        while bit < num_bits {
            let Some(src_bit) = bit_position.checked_add(bit) else {
                break;
            };
            if self.bit(src_bit) {
                out.set_bit(bit);
            }
            bit += 1;
        }
        out
    }

    pub fn concat(&self, lo: &ApInt) -> ApInt {
        let width = self.bit_width.saturating_add(lo.bit_width);
        let mut out = Self::zero(width);
        out.insert_bits(lo, 0);
        out.insert_bits(self, lo.bit_width);
        out
    }

    pub fn byte_swap(&self) -> ApInt {
        let num_bytes = self.bit_width.saturating_add(7) / 8;
        let mut out = Self::zero(self.bit_width);
        let mut byte = 0;
        while byte < num_bytes {
            let src_bit = byte.saturating_mul(8);
            let dst_byte = num_bytes - 1 - byte;
            let dst_bit = dst_byte.saturating_mul(8);
            out.insert_bits(&self.extract_bits(8, src_bit), dst_bit);
            byte += 1;
        }
        out
    }

    pub fn reverse_bits(&self) -> ApInt {
        let mut out = Self::zero(self.bit_width);
        let mut bit = 0;
        while bit < self.bit_width {
            if self.bit(bit) {
                out.set_bit(self.bit_width - 1 - bit);
            }
            bit += 1;
        }
        out
    }

    pub fn shl(&self, amount: u32) -> ApInt {
        if amount >= self.bit_width {
            Self::zero(self.bit_width)
        } else {
            self.shl_truncating(amount)
        }
    }

    pub fn lshr(&self, amount: u32) -> ApInt {
        self.checked_lshr(amount)
            .unwrap_or_else(|| Self::zero(self.bit_width))
    }

    /// Arithmetic right shift, saturating at the bit width.
    ///
    /// Shifting by at least the width leaves every bit equal to the original
    /// sign bit: zero for a non-negative value, all-ones for a negative one.
    /// Mirrors `APInt::ashrInPlace`, whose `ShiftAmt == BitWidth` arm is
    /// spelled `SExtVAL >> (APINT_BITS_PER_WORD - 1)` — "fill with sign bit".
    /// The `APInt` overload reaches that arm through
    /// `getLimitedValue(BitWidth)`, so a larger amount saturates the same way.
    pub fn ashr(&self, amount: u32) -> ApInt {
        self.checked_ashr(amount).unwrap_or_else(|| {
            if self.is_negative() {
                Self::all_ones(self.bit_width)
            } else {
                Self::zero(self.bit_width)
            }
        })
    }

    pub fn splat(new_len: u32, value: &ApInt) -> Self {
        if value.bit_width == 0 || new_len == 0 {
            return Self::zero(new_len);
        }
        let mut out = Self::zero(new_len);
        let mut offset = 0;
        while offset < new_len {
            let mut bit = 0;
            while bit < value.bit_width && offset + bit < new_len {
                if value.bit(bit) {
                    out.set_bit(offset + bit);
                }
                bit += 1;
            }
            offset = offset.saturating_add(value.bit_width);
        }
        out
    }

    #[inline]
    pub fn bit_width(&self) -> u32 {
        self.bit_width
    }

    #[inline]
    pub fn num_words(&self) -> usize {
        words_for_bits_usize(self.bit_width)
    }

    #[inline]
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    #[inline]
    pub fn raw_words(&self) -> &[u64] {
        self.words()
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.words.is_empty()
    }

    #[inline]
    pub fn is_one(&self) -> bool {
        self.words.len() == 1 && self.words[0] == 1
    }

    pub fn is_all_ones(&self) -> bool {
        self.bit_width == 0 || self.eq_ap_int(&Self::all_ones(self.bit_width))
    }

    #[inline]
    pub fn is_negative(&self) -> bool {
        self.bit_width != 0 && self.bit(self.bit_width - 1)
    }

    #[inline]
    pub fn is_non_negative(&self) -> bool {
        !self.is_negative()
    }

    #[inline]
    pub fn is_strictly_positive(&self) -> bool {
        self.is_non_negative() && !self.is_zero()
    }

    #[inline]
    pub fn is_non_positive(&self) -> bool {
        self.is_negative() || self.is_zero()
    }

    /// Whether `bit` is set **and is the only bit set**.
    ///
    /// Mirrors `APInt::isOneBitSet`, which is `(*this)[BitNo] && popcount() ==
    /// 1`. This used to answer the first half only — upstream's name over
    /// [`bit`](Self::bit)'s meaning — so a port of LLVM code calling
    /// `isOneBitSet` would have silently accepted values with other bits set.
    /// Every in-tree caller wanted `bit` and now says so.
    #[inline]
    pub fn is_one_bit_set(&self, bit: u32) -> bool {
        self.bit(bit) && self.popcount() == 1
    }

    #[inline]
    pub fn is_max_value(&self) -> bool {
        self.is_all_ones()
    }

    #[inline]
    pub fn is_max_signed_value(&self) -> bool {
        self.eq_ap_int(&Self::signed_max_value(self.bit_width))
    }

    #[inline]
    pub fn is_min_value(&self) -> bool {
        self.is_zero()
    }

    #[inline]
    pub fn is_min_signed_value(&self) -> bool {
        self.eq_ap_int(&Self::signed_min_value(self.bit_width))
    }

    #[inline]
    pub fn is_power_of_2(&self) -> bool {
        self.popcount() == 1
    }

    #[inline]
    pub fn is_negated_power_of_2(&self) -> bool {
        self.is_negative() && self.count_trailing_zeros() + 1 == self.bit_width
    }

    #[inline]
    pub fn is_sign_mask(&self) -> bool {
        self.eq_ap_int(&Self::sign_mask(self.bit_width))
    }

    #[inline]
    pub fn is_int_n(&self, bits: u32) -> bool {
        self.active_bits() <= bits
    }

    #[inline]
    pub fn is_signed_int_n(&self, bits: u32) -> bool {
        self.significant_bits() <= bits
    }

    /// Whether the value is a non-empty run of ones starting at bit zero.
    ///
    /// Ports `APInt::isMask`, whose single-word path is `isMask_64`
    /// (`Value && ((Value + 1) & Value) == 0`) and whose multi-word path
    /// requires `Ones > 0`. **Zero is not a mask** under either, which is the
    /// bit this used to get wrong: comparing the trailing-ones count against
    /// the active-bit count made `0 == 0` answer `true` for every zero.
    #[inline]
    pub fn is_mask(&self) -> bool {
        let trailing = self.count_trailing_ones();
        trailing > 0 && trailing == self.active_bits()
    }

    pub fn is_shifted_mask(&self) -> bool {
        if self.is_zero() {
            return false;
        }
        let tz = self.count_trailing_zeros();
        let shifted = self
            .checked_lshr(tz)
            .unwrap_or_else(|| Self::zero(self.bit_width));
        shifted.is_mask()
    }

    #[inline]
    pub fn bool_value(&self) -> bool {
        !self.is_zero()
    }

    pub fn limited_value(&self, limit: u64) -> u64 {
        match self.try_zext_u64() {
            Some(v) if v < limit => v,
            _ => limit,
        }
    }

    #[inline]
    pub fn active_bits(&self) -> u32 {
        self.bit_width.saturating_sub(self.count_leading_zeros())
    }

    #[inline]
    pub fn active_words(&self) -> usize {
        let bits = self.active_bits();
        if bits == 0 {
            1
        } else {
            u32_to_usize((bits - 1) / WORD_BITS) + 1
        }
    }

    #[inline]
    pub fn significant_bits(&self) -> u32 {
        self.bit_width
            .saturating_sub(self.num_sign_bits())
            .saturating_add(1)
    }

    pub fn try_zext_u64(&self) -> Option<u64> {
        if self.active_bits() > 64 {
            None
        } else {
            Some(word_at(&self.words, 0, self.bit_width))
        }
    }

    pub fn try_sext_i64(&self) -> Option<i64> {
        let v = self.try_sext_i128()?;
        i64::try_from(v).ok()
    }

    pub fn try_zext_u128(&self) -> Option<u128> {
        if self.active_bits() > 128 {
            return None;
        }
        let lo = u128::from(word_at(&self.words, 0, self.bit_width));
        let hi = u128::from(word_at(&self.words, 1, self.bit_width));
        Some(lo | (hi << 64))
    }

    pub fn try_sext_i128(&self) -> Option<i128> {
        if self.significant_bits() > 128 {
            return None;
        }
        if !self.is_negative() {
            return i128::try_from(self.try_zext_u128()?).ok();
        }
        let magnitude = self.negate().try_zext_u128()?;
        if magnitude == (1u128 << 127) {
            Some(i128::MIN)
        } else {
            i128::try_from(magnitude).ok().map(core::ops::Neg::neg)
        }
    }

    pub fn count_leading_zeros(&self) -> u32 {
        if self.bit_width == 0 {
            return 0;
        }
        let mut count = 0;
        let mut bit = self.bit_width;
        while bit > 0 {
            bit -= 1;
            if self.bit(bit) {
                break;
            }
            count += 1;
        }
        count
    }

    pub fn count_leading_ones(&self) -> u32 {
        if self.bit_width == 0 {
            return 0;
        }
        let mut count = 0;
        let mut bit = self.bit_width;
        while bit > 0 {
            bit -= 1;
            if !self.bit(bit) {
                break;
            }
            count += 1;
        }
        count
    }

    pub fn count_trailing_zeros(&self) -> u32 {
        let mut count = 0;
        while count < self.bit_width {
            if self.bit(count) {
                break;
            }
            count += 1;
        }
        count
    }

    pub fn count_trailing_ones(&self) -> u32 {
        let mut count = 0;
        while count < self.bit_width {
            if !self.bit(count) {
                break;
            }
            count += 1;
        }
        count
    }

    #[inline]
    pub fn num_sign_bits(&self) -> u32 {
        if self.is_negative() {
            self.count_leading_ones()
        } else {
            self.count_leading_zeros()
        }
    }

    pub fn popcount(&self) -> u32 {
        let mut total = 0u32;
        for word in self.words.iter() {
            total = total.saturating_add(word.count_ones());
        }
        total
    }

    pub fn wrapping_add(&self, rhs: &ApInt) -> ApInt {
        if self.bit_width != rhs.bit_width {
            return Self::zero(self.bit_width);
        }
        let len = self.num_words();
        let mut out = vec![0; len];
        let mut carry = 0u128;
        let mut i = 0usize;
        while i < len {
            let sum = u128::from(word_at(&self.words, i, self.bit_width))
                + u128::from(word_at(&rhs.words, i, rhs.bit_width))
                + carry;
            out[i] = low_u64(sum);
            carry = sum >> 64;
            i += 1;
        }
        Self::from_words(self.bit_width, &out)
    }

    pub fn wrapping_sub(&self, rhs: &ApInt) -> ApInt {
        if self.bit_width != rhs.bit_width {
            return Self::zero(self.bit_width);
        }
        let len = self.num_words();
        let mut out = vec![0; len];
        let mut borrow = 0u128;
        let mut i = 0usize;
        while i < len {
            let lhs_word = u128::from(word_at(&self.words, i, self.bit_width));
            let rhs_word = u128::from(word_at(&rhs.words, i, rhs.bit_width)) + borrow;
            if lhs_word >= rhs_word {
                out[i] = low_u64(lhs_word - rhs_word);
                borrow = 0;
            } else {
                out[i] = low_u64((lhs_word + TWO_64) - rhs_word);
                borrow = 1;
            }
            i += 1;
        }
        Self::from_words(self.bit_width, &out)
    }

    pub fn wrapping_mul(&self, rhs: &ApInt) -> ApInt {
        if self.bit_width != rhs.bit_width {
            return Self::zero(self.bit_width);
        }
        let len = self.num_words();
        let mut out = vec![0u64; len];
        let mut i = 0usize;
        while i < len {
            let mut carry = 0u128;
            let mut j = 0usize;
            while j < len && i + j < len {
                let idx = i + j;
                let accum = u128::from(out[idx])
                    + u128::from(word_at(&self.words, i, self.bit_width))
                        * u128::from(word_at(&rhs.words, j, rhs.bit_width))
                    + carry;
                out[idx] = low_u64(accum);
                carry = accum >> 64;
                j += 1;
            }
            i += 1;
        }
        Self::from_words(self.bit_width, &out)
    }

    #[inline]
    pub fn bitand(&self, rhs: &ApInt) -> ApInt {
        bitwise(self, rhs, |a, b| a & b)
    }

    #[inline]
    pub fn bitor(&self, rhs: &ApInt) -> ApInt {
        bitwise(self, rhs, |a, b| a | b)
    }

    #[inline]
    pub fn bitxor(&self, rhs: &ApInt) -> ApInt {
        bitwise(self, rhs, |a, b| a ^ b)
    }

    pub fn negate(&self) -> ApInt {
        self.not()
            .wrapping_add(&Self::from_words(self.bit_width, &[1]))
    }

    /// Rotate left by `amount` bits. Mirrors `APInt::rotl`.
    pub fn rotl(&self, amount: u32) -> ApInt {
        if self.bit_width == 0 {
            return self.clone();
        }
        let amount = amount % self.bit_width;
        if amount == 0 {
            return self.clone();
        }
        ApInt::bitor(&self.shl(amount), &self.lshr(self.bit_width - amount))
    }

    /// Rotate right by `amount` bits. Mirrors `APInt::rotr`.
    pub fn rotr(&self, amount: u32) -> ApInt {
        if self.bit_width == 0 {
            return self.clone();
        }
        let amount = amount % self.bit_width;
        if amount == 0 {
            return self.clone();
        }
        ApInt::bitor(&self.lshr(amount), &self.shl(self.bit_width - amount))
    }

    /// Rotate left by an amount held in an `ApInt` of any width. Mirrors
    /// `APInt::rotl(const APInt &)`, which reduces the amount through
    /// `rotateModulo` first.
    #[inline]
    pub fn rotl_by(&self, amount: &ApInt) -> ApInt {
        self.rotl(self.rotate_modulo(amount))
    }

    /// Rotate right by an amount held in an `ApInt` of any width. Mirrors
    /// `APInt::rotr(const APInt &)`.
    #[inline]
    pub fn rotr_by(&self, amount: &ApInt) -> ApInt {
        self.rotr(self.rotate_modulo(amount))
    }

    /// Reduce a rotate amount of arbitrary width modulo this value's width.
    /// Ports the file-local `rotateModulo` in `APInt.cpp`, which widens the
    /// amount first so the `urem` cannot divide by zero — an `ApInt` of width
    /// one can only hold 0 or 1, and `1 % 1` must be computable.
    fn rotate_modulo(&self, amount: &ApInt) -> u32 {
        if self.bit_width == 0 {
            return 0;
        }
        let widened = if amount.bit_width < self.bit_width {
            match amount.zext(self.bit_width) {
                Some(widened) => widened,
                None => return 0,
            }
        } else {
            amount.clone()
        };
        let modulus = Self::from_words(widened.bit_width, &[u64::from(self.bit_width)]);
        let Some(remainder) = widened.checked_urem(&modulus) else {
            return 0;
        };
        // A remainder modulo `bit_width` is below `bit_width`, so it fits the
        // same `u32`; `limited_value` caps it there in any case.
        let reduced = remainder.limited_value(u64::from(self.bit_width));
        u32::try_from(reduced).unwrap_or(self.bit_width)
    }

    /// Funnel shift left: the high `bit_width` bits of `(hi:lo) << shift`.
    /// Mirrors `APIntOps::fshl`, which reduces the shift with the same
    /// `rotateModulo` the rotates use. `None` when the widths differ.
    pub fn fshl(hi: &ApInt, lo: &ApInt, shift: &ApInt) -> Option<ApInt> {
        hi.same_width(lo)?;
        let amount = hi.rotate_modulo(shift);
        if amount == 0 {
            return Some(hi.clone());
        }
        Some(ApInt::bitor(
            &hi.shl(amount),
            &lo.lshr(hi.bit_width - amount),
        ))
    }

    /// Funnel shift right: the low `bit_width` bits of `(hi:lo) >> shift`.
    /// Mirrors `APIntOps::fshr`.
    pub fn fshr(hi: &ApInt, lo: &ApInt, shift: &ApInt) -> Option<ApInt> {
        hi.same_width(lo)?;
        let amount = hi.rotate_modulo(shift);
        if amount == 0 {
            return Some(lo.clone());
        }
        Some(ApInt::bitor(
            &hi.shl(hi.bit_width - amount),
            &lo.lshr(amount),
        ))
    }

    /// Carry-less multiply — the same schoolbook product with `xor` in place of
    /// addition. Mirrors `APIntOps::clmul`.
    pub fn carryless_mul(&self, rhs: &ApInt) -> Option<ApInt> {
        self.same_width(rhs)?;
        let mut result = Self::zero(self.bit_width);
        for bit in 0..self.bit_width {
            if rhs.bit(bit) {
                result = ApInt::bitxor(&result, &self.shl(bit));
            }
        }
        Some(result)
    }

    /// The bit-reversed carry-less product. Mirrors `APIntOps::clmulr`, which
    /// is `bitreverse(clmul(bitreverse(a), bitreverse(b)))`.
    pub fn carryless_mul_reversed(&self, rhs: &ApInt) -> Option<ApInt> {
        Some(
            self.reverse_bits()
                .carryless_mul(&rhs.reverse_bits())?
                .reverse_bits(),
        )
    }

    /// The high half of the carry-less product. Mirrors `APIntOps::clmulh`,
    /// which is `clmulr(a, b) >> 1`.
    pub fn carryless_mul_high(&self, rhs: &ApInt) -> Option<ApInt> {
        Some(self.carryless_mul_reversed(rhs)?.lshr(1))
    }

    /// The bottom `count` bits, zero-extended within the same width. Mirrors
    /// `APInt::getLoBits`.
    #[inline]
    pub fn lo_bits(&self, count: u32) -> ApInt {
        ApInt::bitand(self, &Self::low_bits_set(self.bit_width, count))
    }

    /// The top `count` bits, shifted down to the bottom of the same width.
    /// Mirrors `APInt::getHiBits`.
    #[inline]
    pub fn hi_bits(&self, count: u32) -> ApInt {
        self.lshr(self.bit_width.saturating_sub(count))
    }

    /// Whether the value repeats a `splat_size_in_bits`-wide pattern.
    ///
    /// Mirrors `APInt::isSplat`, which is the rotate-and-compare trick.
    /// Upstream asserts that the size divides the width; llvmkit runs the same
    /// comparison without asserting, so a size that does not divide the width
    /// answers whatever that comparison says rather than trapping.
    #[inline]
    pub fn is_splat(&self, splat_size_in_bits: u32) -> bool {
        self.eq_ap_int(&self.rotl(splat_size_in_bits))
    }

    /// `floor(log2(self))`, or `None` for zero.
    ///
    /// Mirrors `APInt::logBase2`, which is `getActiveBits() - 1` and therefore
    /// wraps to `UINT32_MAX` on zero. The wrap is upstream's out-of-band
    /// marker, not a value; it is spelled `None` here.
    #[inline]
    pub fn log_base2(&self) -> Option<u32> {
        self.active_bits().checked_sub(1)
    }

    /// `ceil(log2(self))`. Mirrors `APInt::ceilLogBase2`, including its answer
    /// of `bit_width` for zero — there `self - 1` wraps to all-ones.
    #[inline]
    pub fn ceil_log_base2(&self) -> u32 {
        self.wrapping_sub(&Self::from_words(self.bit_width, &[1]))
            .active_bits()
    }

    /// `log2` rounded to the nearest integer, or `None` for zero.
    ///
    /// Mirrors `APInt::nearestLogBase2`, whose zero answer is the `UINT32_MAX`
    /// marker. Upstream computes `logBase2(x) + x[logBase2(x) - 1]`; the
    /// one-bit-wide special case falls out of the same formula here because
    /// `bit(u32::MAX)` is `false`.
    pub fn nearest_log_base2(&self) -> Option<u32> {
        let lg = self.log_base2()?;
        Some(lg + u32::from(self.bit(lg.wrapping_sub(1))))
    }

    /// `log2(self)` when the value is exactly a power of two, else `None`.
    /// Mirrors `APInt::exactLogBase2`, whose `-1` is spelled `None` here.
    #[inline]
    pub fn exact_log_base2(&self) -> Option<u32> {
        if self.is_power_of_2() {
            self.log_base2()
        } else {
            None
        }
    }

    /// The high half of the signed full-width product. Mirrors
    /// `APIntOps::mulhs`; `None` when the widths differ or cannot be doubled.
    pub fn mul_high_signed(&self, rhs: &ApInt) -> Option<ApInt> {
        let full_width = self.doubled_width(rhs)?;
        let lhs = self.sext(full_width)?;
        let rhs = rhs.sext(full_width)?;
        Some(
            lhs.wrapping_mul(&rhs)
                .extract_bits(self.bit_width, self.bit_width),
        )
    }

    /// The high half of the unsigned full-width product. Mirrors
    /// `APIntOps::mulhu`.
    pub fn mul_high_unsigned(&self, rhs: &ApInt) -> Option<ApInt> {
        let full_width = self.doubled_width(rhs)?;
        let lhs = self.zext(full_width)?;
        let rhs = rhs.zext(full_width)?;
        Some(
            lhs.wrapping_mul(&rhs)
                .extract_bits(self.bit_width, self.bit_width),
        )
    }

    fn doubled_width(&self, rhs: &ApInt) -> Option<u32> {
        if self.bit_width != rhs.bit_width {
            return None;
        }
        self.bit_width.checked_mul(2)
    }

    /// Unsigned division under an explicit rounding mode. Mirrors
    /// `APIntOps::RoundingUDiv`; `udivrem` itself always rounds down.
    pub fn rounding_udiv(&self, rhs: &ApInt, rounding: ApIntRounding) -> Option<ApInt> {
        match rounding {
            ApIntRounding::Down | ApIntRounding::TowardZero => self.checked_udiv(rhs),
            ApIntRounding::Up => {
                let (quotient, remainder) = self.udivrem(rhs)?.into_parts();
                if remainder.is_zero() {
                    return Some(quotient);
                }
                Some(quotient.wrapping_add(&Self::from_words(self.bit_width, &[1])))
            }
        }
    }

    /// Signed division under an explicit rounding mode. Mirrors
    /// `APIntOps::RoundingSDiv`; `sdivrem` itself rounds toward zero, so the
    /// other two modes correct the quotient by inspecting the remainder's sign.
    pub fn rounding_sdiv(&self, rhs: &ApInt, rounding: ApIntRounding) -> Option<ApInt> {
        match rounding {
            ApIntRounding::TowardZero => self.checked_sdiv(rhs),
            ApIntRounding::Down | ApIntRounding::Up => {
                let (quotient, remainder) = self.sdivrem(rhs)?.into_parts();
                if remainder.is_zero() {
                    return Some(quotient);
                }
                let one = Self::from_words(self.bit_width, &[1]);
                // Whether the exact quotient's fractional part is negative.
                let rounded_up = remainder.is_negative() != rhs.is_negative();
                Some(match rounding {
                    ApIntRounding::Down if rounded_up => quotient.wrapping_sub(&one),
                    ApIntRounding::Down => quotient,
                    _ if rounded_up => quotient,
                    _ => quotient.wrapping_add(&one),
                })
            }
        }
    }

    /// `floor((self + rhs) / 2)` computed without overflow, treating both as
    /// signed. Mirrors `APIntOps::avgFloorS`.
    pub fn avg_floor_signed(&self, rhs: &ApInt) -> Option<ApInt> {
        self.same_width(rhs)?;
        Some(ApInt::bitand(self, rhs).wrapping_add(&ApInt::bitxor(self, rhs).ashr(1)))
    }

    /// `floor((self + rhs) / 2)` treating both as unsigned. Mirrors
    /// `APIntOps::avgFloorU`.
    pub fn avg_floor_unsigned(&self, rhs: &ApInt) -> Option<ApInt> {
        self.same_width(rhs)?;
        Some(ApInt::bitand(self, rhs).wrapping_add(&ApInt::bitxor(self, rhs).lshr(1)))
    }

    /// `ceil((self + rhs) / 2)` treating both as signed. Mirrors
    /// `APIntOps::avgCeilS`.
    pub fn avg_ceil_signed(&self, rhs: &ApInt) -> Option<ApInt> {
        self.same_width(rhs)?;
        Some(ApInt::bitor(self, rhs).wrapping_sub(&ApInt::bitxor(self, rhs).ashr(1)))
    }

    /// `ceil((self + rhs) / 2)` treating both as unsigned. Mirrors
    /// `APIntOps::avgCeilU`.
    pub fn avg_ceil_unsigned(&self, rhs: &ApInt) -> Option<ApInt> {
        self.same_width(rhs)?;
        Some(ApInt::bitor(self, rhs).wrapping_sub(&ApInt::bitxor(self, rhs).lshr(1)))
    }

    /// The signed absolute difference. Mirrors `APIntOps::abds`.
    pub fn abs_diff_signed(&self, rhs: &ApInt) -> Option<ApInt> {
        self.same_width(rhs)?;
        Some(if self.sge(rhs) {
            self.wrapping_sub(rhs)
        } else {
            rhs.wrapping_sub(self)
        })
    }

    /// The unsigned absolute difference. Mirrors `APIntOps::abdu`.
    pub fn abs_diff_unsigned(&self, rhs: &ApInt) -> Option<ApInt> {
        self.same_width(rhs)?;
        Some(if self.uge(rhs) {
            self.wrapping_sub(rhs)
        } else {
            rhs.wrapping_sub(self)
        })
    }

    /// The multiplicative inverse modulo `2^bit_width`, which exists only for
    /// odd values. Mirrors `APInt::multiplicativeInverse`, whose assertion on
    /// an even value is spelled `None` here.
    pub fn multiplicative_inverse(&self) -> Option<ApInt> {
        if !self.bit(0) {
            return None;
        }
        // Newton's method: `factor *= 2 - self * factor` until the product is 1.
        let two = Self::from_words(self.bit_width, &[2]);
        let mut factor = self.clone();
        loop {
            let product = self.wrapping_mul(&factor);
            if product.is_one() {
                return Some(factor);
            }
            factor = factor.wrapping_mul(&two.wrapping_sub(&product));
        }
    }

    /// The greatest common divisor. Mirrors
    /// `APIntOps::GreatestCommonDivisor`, a variant of Stein's algorithm.
    pub fn greatest_common_divisor(lhs: &ApInt, rhs: &ApInt) -> Option<ApInt> {
        lhs.same_width(rhs)?;
        // Fast-path a common case.
        if lhs.eq_ap_int(rhs) {
            return Some(lhs.clone());
        }
        // Corner cases: if either operand is zero, the other is the gcd.
        if lhs.is_zero() {
            return Some(rhs.clone());
        }
        if rhs.is_zero() {
            return Some(lhs.clone());
        }

        // Count common powers of 2 and remove all other powers of 2.
        let mut a = lhs.clone();
        let mut b = rhs.clone();
        let pow2_a = a.count_trailing_zeros();
        let pow2_b = b.count_trailing_zeros();
        let pow2 = pow2_a.min(pow2_b);
        a = a.lshr(pow2_a - pow2);
        b = b.lshr(pow2_b - pow2);

        // Both operands are now odd multiples of `2^pow2`:
        //   gcd(a, b) = gcd(|a - b| / 2^i, min(a, b))
        while !a.eq_ap_int(&b) {
            if a.ugt(&b) {
                a = a.wrapping_sub(&b);
                a = a.lshr(a.count_trailing_zeros() - pow2);
            } else {
                b = b.wrapping_sub(&a);
                b = b.lshr(b.count_trailing_zeros() - pow2);
            }
        }
        Some(a)
    }

    /// The index of the most significant bit on which the two values differ,
    /// or `None` when they are equal. Mirrors
    /// `APIntOps::GetMostSignificantDifferentBit`; a width mismatch, which
    /// upstream asserts against, also answers `None`.
    pub fn most_significant_different_bit(lhs: &ApInt, rhs: &ApInt) -> Option<u32> {
        lhs.same_width(rhs)?;
        if lhs.eq_ap_int(rhs) {
            return None;
        }
        lhs.bit_width
            .checked_sub(ApInt::bitxor(lhs, rhs).count_leading_zeros() + 1)
    }

    /// `self` raised to `exponent`, wrapping at the width. Mirrors
    /// `APIntOps::pow`; upstream's assertion against a negative exponent is
    /// spelled by taking an unsigned one.
    pub fn pow(&self, exponent: u64) -> ApInt {
        let mut accumulator = Self::from_words(self.bit_width, &[1]);
        if exponent == 0 {
            return accumulator;
        }
        let mut base = self.clone();
        let mut remaining = exponent;
        while remaining > 0 {
            while remaining.is_multiple_of(2) {
                base = base.wrapping_mul(&base);
                remaining /= 2;
            }
            remaining -= 1;
            accumulator = accumulator.wrapping_mul(&base);
        }
        accumulator
    }

    /// Rescale a bit mask to a different width, where one width is a multiple
    /// of the other. Mirrors `APIntOps::ScaleBitMask`: widening repeats each
    /// bit, narrowing folds each group down to one bit — by "all set" when
    /// `match_all_bits`, otherwise by "any set". `None` when neither width
    /// divides the other, which upstream asserts against.
    pub fn scale_bit_mask(&self, new_bit_width: u32, match_all_bits: bool) -> Option<ApInt> {
        let old_bit_width = self.bit_width;
        if old_bit_width == new_bit_width {
            return Some(self.clone());
        }
        let (wider, narrower) = if new_bit_width > old_bit_width {
            (new_bit_width, old_bit_width)
        } else {
            (old_bit_width, new_bit_width)
        };
        if narrower == 0 || wider % narrower != 0 {
            return None;
        }

        let mut scaled = Self::zero(new_bit_width);
        if self.is_zero() {
            return Some(scaled);
        }

        let scale = wider / narrower;
        if new_bit_width > old_bit_width {
            // Repeat bits.
            for bit in 0..old_bit_width {
                if self.bit(bit) {
                    scaled.set_bits(bit * scale, (bit + 1) * scale);
                }
            }
        } else {
            for bit in 0..new_bit_width {
                let group = self.extract_bits(scale, bit * scale);
                let carries = if match_all_bits {
                    group.is_all_ones()
                } else {
                    !group.is_zero()
                };
                if carries {
                    scaled.set_bit(bit);
                }
            }
        }
        Some(scaled)
    }

    #[inline]
    fn same_width(&self, rhs: &ApInt) -> Option<()> {
        (self.bit_width == rhs.bit_width).then_some(())
    }

    pub fn checked_shl(&self, amount: u32) -> Option<ApInt> {
        if amount >= self.bit_width {
            return None;
        }
        Some(self.shl_truncating(amount))
    }

    pub fn checked_lshr(&self, amount: u32) -> Option<ApInt> {
        if amount >= self.bit_width {
            return None;
        }
        let len = self.num_words();
        let word_shift = u32_to_usize(amount / WORD_BITS);
        let bit_shift = amount % WORD_BITS;
        let mut out = vec![0; len];
        let mut dst = 0usize;
        while dst < len {
            let src = dst + word_shift;
            if src < len {
                let mut value = word_at(&self.words, src, self.bit_width) >> bit_shift;
                if bit_shift != 0 && src + 1 < len {
                    value |=
                        word_at(&self.words, src + 1, self.bit_width) << (WORD_BITS - bit_shift);
                }
                out[dst] = value;
            }
            dst += 1;
        }
        Some(Self::from_words(self.bit_width, &out))
    }

    pub fn checked_ashr(&self, amount: u32) -> Option<ApInt> {
        if amount >= self.bit_width {
            return None;
        }
        let mut out = self.checked_lshr(amount)?;
        if self.is_negative() && amount != 0 {
            out = ApInt::bitor(
                &out,
                &Self::bits_set_from(self.bit_width, self.bit_width - amount),
            );
        }
        Some(out)
    }

    #[inline]
    pub fn checked_udiv(&self, rhs: &ApInt) -> Option<ApInt> {
        self.udivrem(rhs).map(|qr| qr.quotient)
    }

    pub fn checked_sdiv(&self, rhs: &ApInt) -> Option<ApInt> {
        self.sdivrem(rhs).map(|qr| qr.quotient)
    }

    #[inline]
    pub fn checked_urem(&self, rhs: &ApInt) -> Option<ApInt> {
        self.udivrem(rhs).map(|qr| qr.remainder)
    }

    #[inline]
    pub fn checked_srem(&self, rhs: &ApInt) -> Option<ApInt> {
        self.sdivrem(rhs).map(|qr| qr.remainder)
    }

    pub fn udivrem(&self, rhs: &ApInt) -> Option<ApIntDivRem> {
        if self.bit_width != rhs.bit_width || rhs.is_zero() {
            return None;
        }
        let mut quotient = Self::zero(self.bit_width);
        let mut remainder = Self::zero(self.bit_width);
        let mut i = self.bit_width;
        while i > 0 {
            i -= 1;
            remainder = remainder.shl_truncating(1);
            if self.bit(i) {
                remainder.set_bit(0);
            }
            if remainder.uge(rhs) {
                remainder = remainder.wrapping_sub(rhs);
                quotient.set_bit(i);
            }
        }
        Some(ApIntDivRem {
            quotient,
            remainder,
        })
    }

    pub fn sdivrem(&self, rhs: &ApInt) -> Option<ApIntDivRem> {
        if self.bit_width != rhs.bit_width || rhs.is_zero() {
            return None;
        }
        if self.is_min_signed_value() && rhs.is_all_ones() {
            return None;
        }
        let lhs_neg = self.is_negative();
        let rhs_neg = rhs.is_negative();
        let lhs_abs = if lhs_neg { self.negate() } else { self.clone() };
        let rhs_abs = if rhs_neg { rhs.negate() } else { rhs.clone() };
        let qr = lhs_abs.udivrem(&rhs_abs)?;
        let quotient = if lhs_neg != rhs_neg {
            qr.quotient.negate()
        } else {
            qr.quotient
        };
        let remainder = if lhs_neg {
            qr.remainder.negate()
        } else {
            qr.remainder
        };
        Some(ApIntDivRem {
            quotient,
            remainder,
        })
    }

    #[inline]
    pub fn eq_ap_int(&self, rhs: &ApInt) -> bool {
        self.bit_width == rhs.bit_width && self.words == rhs.words
    }

    #[inline]
    pub fn ult(&self, rhs: &ApInt) -> bool {
        self.unsigned_cmp(rhs) == Ordering::Less
    }

    #[inline]
    pub fn ule(&self, rhs: &ApInt) -> bool {
        !self.ugt(rhs)
    }

    #[inline]
    pub fn ugt(&self, rhs: &ApInt) -> bool {
        self.unsigned_cmp(rhs) == Ordering::Greater
    }

    #[inline]
    pub fn uge(&self, rhs: &ApInt) -> bool {
        !self.ult(rhs)
    }

    #[inline]
    pub fn slt(&self, rhs: &ApInt) -> bool {
        self.signed_cmp(rhs) == Ordering::Less
    }

    #[inline]
    pub fn sle(&self, rhs: &ApInt) -> bool {
        !self.sgt(rhs)
    }

    #[inline]
    pub fn sgt(&self, rhs: &ApInt) -> bool {
        self.signed_cmp(rhs) == Ordering::Greater
    }

    #[inline]
    pub fn sge(&self, rhs: &ApInt) -> bool {
        !self.slt(rhs)
    }

    pub fn intersects(&self, rhs: &ApInt) -> bool {
        if self.bit_width != rhs.bit_width {
            return false;
        }
        let len = self.num_words();
        let mut i = 0usize;
        while i < len {
            if (word_at(&self.words, i, self.bit_width) & word_at(&rhs.words, i, rhs.bit_width))
                != 0
            {
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn is_subset_of(&self, rhs: &ApInt) -> bool {
        if self.bit_width != rhs.bit_width {
            return false;
        }
        let len = self.num_words();
        let mut i = 0usize;
        while i < len {
            if (word_at(&self.words, i, self.bit_width) & !word_at(&rhs.words, i, rhs.bit_width))
                != 0
            {
                return false;
            }
            i += 1;
        }
        true
    }

    #[inline]
    pub fn same_value(lhs: &ApInt, rhs: &ApInt) -> bool {
        lhs.eq_ap_int(rhs)
    }

    pub fn trunc(&self, width: u32) -> Option<ApInt> {
        if width > self.bit_width {
            None
        } else {
            Some(Self::from_words(width, &self.words))
        }
    }

    pub fn trunc_usat(&self, width: u32) -> ApInt {
        let truncated = self.trunc(width).unwrap_or_else(|| self.clone());
        if truncated.zext_or_trunc(self.bit_width).eq_ap_int(self) {
            truncated
        } else {
            Self::max_value(width)
        }
    }

    pub fn trunc_ssat(&self, width: u32) -> ApInt {
        let truncated = self.trunc(width).unwrap_or_else(|| self.clone());
        if truncated.sext_or_trunc(self.bit_width).eq_ap_int(self) {
            truncated
        } else if self.is_negative() {
            Self::signed_min_value(width)
        } else {
            Self::signed_max_value(width)
        }
    }

    pub fn zext(&self, width: u32) -> Option<ApInt> {
        if width < self.bit_width {
            None
        } else {
            Some(Self::from_words(width, &self.words))
        }
    }

    pub fn sext(&self, width: u32) -> Option<ApInt> {
        if width < self.bit_width {
            return None;
        }
        let mut out = Self::from_words(width, &self.words);
        if self.is_negative() {
            out.set_bits_from(self.bit_width);
        }
        Some(out)
    }

    pub fn zext_or_trunc(&self, width: u32) -> ApInt {
        if width >= self.bit_width {
            self.zext(width).unwrap_or_else(|| Self::zero(width))
        } else {
            self.trunc(width).unwrap_or_else(|| Self::zero(width))
        }
    }

    pub fn sext_or_trunc(&self, width: u32) -> ApInt {
        if width >= self.bit_width {
            self.sext(width).unwrap_or_else(|| Self::zero(width))
        } else {
            self.trunc(width).unwrap_or_else(|| Self::zero(width))
        }
    }

    pub fn sadd_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        let result = self.wrapping_add(rhs);
        let overflow =
            self.is_negative() == rhs.is_negative() && result.is_negative() != self.is_negative();
        (result, overflow)
    }

    pub fn uadd_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        let result = self.wrapping_add(rhs);
        let overflow = result.ult(self) || result.ult(rhs);
        (result, overflow)
    }

    pub fn ssub_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        let result = self.wrapping_sub(rhs);
        let overflow =
            self.is_negative() != rhs.is_negative() && result.is_negative() != self.is_negative();
        (result, overflow)
    }

    pub fn usub_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        let result = self.wrapping_sub(rhs);
        (result, self.ult(rhs))
    }

    pub fn sdiv_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        match self.checked_sdiv(rhs) {
            Some(v) => (v, false),
            None => (Self::zero(self.bit_width), true),
        }
    }

    pub fn smul_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        let result = self.wrapping_mul(rhs);
        let wide = self.sext_or_trunc(self.bit_width.saturating_mul(2).max(1));
        let wide_rhs = rhs.sext_or_trunc(wide.bit_width);
        let wide_product = wide.wrapping_mul(&wide_rhs);
        let overflow = !result
            .sext_or_trunc(wide_product.bit_width)
            .eq_ap_int(&wide_product);
        (result, overflow)
    }

    pub fn umul_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        let result = self.wrapping_mul(rhs);
        let wide_width = self.bit_width.saturating_mul(2).max(1);
        let wide_product = self
            .zext_or_trunc(wide_width)
            .wrapping_mul(&rhs.zext_or_trunc(wide_width));
        let overflow = wide_product.active_bits() > self.bit_width;
        (result, overflow)
    }

    pub fn sshl_ov(&self, amount: u32) -> (ApInt, bool) {
        match self.checked_shl(amount) {
            Some(v) => {
                let overflow = v
                    .checked_ashr(amount)
                    .map(|r| !r.eq_ap_int(self))
                    .unwrap_or(true);
                (v, overflow)
            }
            None => (Self::zero(self.bit_width), true),
        }
    }

    pub fn ushl_ov(&self, amount: u32) -> (ApInt, bool) {
        match self.checked_shl(amount) {
            Some(v) => {
                let overflow = v
                    .checked_lshr(amount)
                    .map(|r| !r.eq_ap_int(self))
                    .unwrap_or(true);
                (v, overflow)
            }
            None => (Self::zero(self.bit_width), true),
        }
    }

    pub fn sfloordiv_ov(&self, rhs: &ApInt) -> (ApInt, bool) {
        let Some(qr) = self.sdivrem(rhs) else {
            return (Self::zero(self.bit_width), true);
        };
        if !qr.remainder.is_zero() && self.is_negative() != rhs.is_negative() {
            (
                qr.quotient
                    .wrapping_sub(&Self::from_words(self.bit_width, &[1])),
                false,
            )
        } else {
            (qr.quotient, false)
        }
    }

    pub fn sadd_sat(&self, rhs: &ApInt) -> ApInt {
        let (v, ov) = self.sadd_ov(rhs);
        if !ov {
            v
        } else if self.is_negative() {
            Self::signed_min_value(self.bit_width)
        } else {
            Self::signed_max_value(self.bit_width)
        }
    }

    pub fn uadd_sat(&self, rhs: &ApInt) -> ApInt {
        let (v, ov) = self.uadd_ov(rhs);
        if ov {
            Self::max_value(self.bit_width)
        } else {
            v
        }
    }

    pub fn ssub_sat(&self, rhs: &ApInt) -> ApInt {
        let (v, ov) = self.ssub_ov(rhs);
        if !ov {
            v
        } else if self.is_negative() {
            Self::signed_min_value(self.bit_width)
        } else {
            Self::signed_max_value(self.bit_width)
        }
    }

    pub fn usub_sat(&self, rhs: &ApInt) -> ApInt {
        let (v, ov) = self.usub_ov(rhs);
        if ov { Self::zero(self.bit_width) } else { v }
    }

    pub fn smul_sat(&self, rhs: &ApInt) -> ApInt {
        let (v, ov) = self.smul_ov(rhs);
        if !ov {
            v
        } else if self.is_negative() != rhs.is_negative() {
            Self::signed_min_value(self.bit_width)
        } else {
            Self::signed_max_value(self.bit_width)
        }
    }

    pub fn umul_sat(&self, rhs: &ApInt) -> ApInt {
        let (v, ov) = self.umul_ov(rhs);
        if ov {
            Self::max_value(self.bit_width)
        } else {
            v
        }
    }

    pub fn sshl_sat(&self, amount: u32) -> ApInt {
        let (v, ov) = self.sshl_ov(amount);
        if !ov {
            v
        } else if self.is_negative() {
            Self::signed_min_value(self.bit_width)
        } else {
            Self::signed_max_value(self.bit_width)
        }
    }

    pub fn ushl_sat(&self, amount: u32) -> ApInt {
        let (v, ov) = self.ushl_ov(amount);
        if ov {
            Self::max_value(self.bit_width)
        } else {
            v
        }
    }

    pub fn to_string_radix(&self, radix: u8, signedness: Signedness) -> String {
        if !matches!(radix, 2 | 8 | 10 | 16 | 36) {
            return String::new();
        }
        if matches!(signedness, Signedness::Signed) && self.is_negative() {
            let mut out = String::from("-");
            out.push_str(&self.negate().to_unsigned_string(radix));
            out
        } else {
            self.to_unsigned_string(radix)
        }
    }

    fn to_unsigned_string(&self, radix: u8) -> String {
        if self.is_zero() {
            return String::from("0");
        }
        let radix_bits = 64u32.saturating_sub(u64::from(radix).leading_zeros());
        let work_width = self.bit_width.max(radix_bits);
        let divisor = Self::from_words(work_width, &[u64::from(radix)]);
        let mut n = self.zext(work_width).unwrap_or_else(|| self.clone());
        let mut digits = Vec::new();
        while !n.is_zero() {
            let Some(qr) = n.udivrem(&divisor) else {
                return String::new();
            };
            let digit = qr.remainder.try_zext_u64().unwrap_or(0);
            digits.push(digit_char(u8::try_from(digit).unwrap_or(0)));
            n = qr.quotient;
        }
        digits.iter().rev().collect()
    }

    /// The value of a single bit. Mirrors `APInt::operator[]`, whose assert on
    /// an out-of-range index is spelled here as `false` — every bit above the
    /// width is zero by construction.
    pub fn bit(&self, bit: u32) -> bool {
        if bit >= self.bit_width {
            return false;
        }
        let idx = u32_to_usize(bit / WORD_BITS);
        let shift = bit % WORD_BITS;
        (word_at(&self.words, idx, self.bit_width) & (1u64 << shift)) != 0
    }

    pub fn set_bit(&mut self, bit: u32) {
        if bit >= self.bit_width {
            return;
        }
        let mut words = self.words.to_vec();
        let len = self.num_words();
        if words.len() < len {
            words.resize(len, 0);
        }
        let idx = u32_to_usize(bit / WORD_BITS);
        let shift = bit % WORD_BITS;
        if let Some(word) = words.get_mut(idx) {
            *word |= 1u64 << shift;
        }
        canonicalize_words(self.bit_width, &mut words);
        self.words = words.into_boxed_slice();
    }

    pub fn clear_bit(&mut self, bit: u32) {
        if bit >= self.bit_width {
            return;
        }
        let mut words = self.words.to_vec();
        let idx = u32_to_usize(bit / WORD_BITS);
        let shift = bit % WORD_BITS;
        if let Some(word) = words.get_mut(idx) {
            *word &= !(1u64 << shift);
        }
        canonicalize_words(self.bit_width, &mut words);
        self.words = words.into_boxed_slice();
    }

    fn shl_truncating(&self, amount: u32) -> ApInt {
        if self.bit_width == 0 || amount == 0 {
            return self.clone();
        }
        let len = self.num_words();
        let word_shift = u32_to_usize(amount / WORD_BITS);
        let bit_shift = amount % WORD_BITS;
        let mut out = vec![0; len];
        let mut src = 0usize;
        while src < len {
            let dst = src + word_shift;
            if dst < len {
                out[dst] |= word_at(&self.words, src, self.bit_width) << bit_shift;
                if bit_shift != 0 && dst + 1 < len {
                    out[dst + 1] |=
                        word_at(&self.words, src, self.bit_width) >> (WORD_BITS - bit_shift);
                }
            }
            src += 1;
        }
        Self::from_words(self.bit_width, &out)
    }

    /// Total unsigned ordering against a `u64`.
    ///
    /// Stands in for upstream's four scalar overloads `ult`/`ule`/`ugt`/`uge`
    /// taking a `uint64_t`, each of which is `getActiveBits() <= 64 &&
    /// getZExtValue() <op> RHS`. A value too wide for a `u64` is greater than
    /// every `u64`, so the one `Ordering` answers all four questions — which is
    /// why llvmkit does not carry the four booleans.
    pub fn unsigned_cmp_u64(&self, rhs: u64) -> Ordering {
        match self.try_zext_u64() {
            Some(value) => value.cmp(&rhs),
            None => Ordering::Greater,
        }
    }

    /// Total signed ordering against an `i64`.
    ///
    /// Stands in for upstream's `slt`/`sle`/`sgt`/`sge` scalar overloads, whose
    /// wide case is decided by the sign alone (`getSignificantBits() > 64 ?
    /// isNegative() : getSExtValue() <op> RHS`).
    pub fn signed_cmp_i64(&self, rhs: i64) -> Ordering {
        match self.try_sext_i64() {
            Some(value) => value.cmp(&rhs),
            None if self.is_negative() => Ordering::Less,
            None => Ordering::Greater,
        }
    }

    /// Total unsigned ordering. Mirrors `APInt::compare`, which returns an
    /// `int` triple; a width mismatch orders by width rather than trapping.
    #[inline]
    pub fn unsigned_cmp(&self, rhs: &ApInt) -> Ordering {
        if self.bit_width != rhs.bit_width {
            return self.bit_width.cmp(&rhs.bit_width);
        }
        let len = self.num_words();
        let mut i = len;
        while i > 0 {
            i -= 1;
            let lhs_word = word_at(&self.words, i, self.bit_width);
            let rhs_word = word_at(&rhs.words, i, rhs.bit_width);
            match lhs_word.cmp(&rhs_word) {
                Ordering::Equal => {}
                non_eq => return non_eq,
            }
        }
        Ordering::Equal
    }

    /// Total signed ordering. Mirrors `APInt::compareSigned`.
    #[inline]
    pub fn signed_cmp(&self, rhs: &ApInt) -> Ordering {
        if self.bit_width != rhs.bit_width {
            return self.bit_width.cmp(&rhs.bit_width);
        }
        match (self.is_negative(), rhs.is_negative()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => self.unsigned_cmp(rhs),
        }
    }
}

impl Add for ApInt {
    type Output = ApInt;

    fn add(self, rhs: Self) -> Self::Output {
        self.wrapping_add(&rhs)
    }
}

impl Add<&ApInt> for ApInt {
    type Output = ApInt;

    fn add(self, rhs: &ApInt) -> Self::Output {
        self.wrapping_add(rhs)
    }
}

impl Add<&ApInt> for &ApInt {
    type Output = ApInt;

    fn add(self, rhs: &ApInt) -> Self::Output {
        self.wrapping_add(rhs)
    }
}

impl Sub for ApInt {
    type Output = ApInt;

    fn sub(self, rhs: Self) -> Self::Output {
        self.wrapping_sub(&rhs)
    }
}

impl Sub<&ApInt> for ApInt {
    type Output = ApInt;

    fn sub(self, rhs: &ApInt) -> Self::Output {
        self.wrapping_sub(rhs)
    }
}

impl Sub<&ApInt> for &ApInt {
    type Output = ApInt;

    fn sub(self, rhs: &ApInt) -> Self::Output {
        self.wrapping_sub(rhs)
    }
}

impl Mul for ApInt {
    type Output = ApInt;

    fn mul(self, rhs: Self) -> Self::Output {
        self.wrapping_mul(&rhs)
    }
}

impl Mul<&ApInt> for ApInt {
    type Output = ApInt;

    fn mul(self, rhs: &ApInt) -> Self::Output {
        self.wrapping_mul(rhs)
    }
}

impl Mul<&ApInt> for &ApInt {
    type Output = ApInt;

    fn mul(self, rhs: &ApInt) -> Self::Output {
        self.wrapping_mul(rhs)
    }
}

impl BitAnd for ApInt {
    type Output = ApInt;

    fn bitand(self, rhs: Self) -> Self::Output {
        ApInt::bitand(&self, &rhs)
    }
}

impl BitOr for ApInt {
    type Output = ApInt;

    fn bitor(self, rhs: Self) -> Self::Output {
        ApInt::bitor(&self, &rhs)
    }
}

impl BitXor for ApInt {
    type Output = ApInt;

    fn bitxor(self, rhs: Self) -> Self::Output {
        ApInt::bitxor(&self, &rhs)
    }
}

impl Not for ApInt {
    type Output = ApInt;

    fn not(self) -> Self::Output {
        (&self).not()
    }
}

impl Not for &ApInt {
    type Output = ApInt;

    fn not(self) -> Self::Output {
        let mut words = Vec::with_capacity(self.num_words());
        let mut i = 0usize;
        while i < self.num_words() {
            words.push(!word_at(&self.words, i, self.bit_width));
            i += 1;
        }
        ApInt::from_words(self.bit_width, &words)
    }
}

impl Neg for ApInt {
    type Output = ApInt;

    fn neg(self) -> Self::Output {
        self.negate()
    }
}

impl Neg for &ApInt {
    type Output = ApInt;

    fn neg(self) -> Self::Output {
        self.negate()
    }
}

fn bitwise<F>(lhs: &ApInt, rhs: &ApInt, f: F) -> ApInt
where
    F: Fn(u64, u64) -> u64,
{
    if lhs.bit_width != rhs.bit_width {
        return ApInt::zero(lhs.bit_width);
    }
    let len = lhs.num_words();
    let mut out = Vec::with_capacity(len);
    let mut i = 0usize;
    while i < len {
        out.push(f(
            word_at(&lhs.words, i, lhs.bit_width),
            word_at(&rhs.words, i, rhs.bit_width),
        ));
        i += 1;
    }
    ApInt::from_words(lhs.bit_width, &out)
}

fn parse_unsigned_words(text: &str, radix: u8) -> IrResult<Vec<u64>> {
    let mut words = Vec::<u64>::new();
    let mut saw_digit = false;
    for ch in text.chars() {
        let Some(digit) = digit_value(ch) else {
            break;
        };
        if digit >= radix {
            break;
        }
        saw_digit = true;
        mul_words_small(&mut words, u64::from(radix));
        add_words_small(&mut words, u64::from(digit));
    }
    if !saw_digit {
        return Err(IrError::InvalidOperation {
            message: "APInt string contains no digits",
        });
    }
    trim_trailing_zeros(&mut words);
    Ok(words)
}

fn mul_words_small(words: &mut Vec<u64>, multiplier: u64) {
    if words.is_empty() || multiplier == 0 {
        words.clear();
        return;
    }
    let mut carry = 0u128;
    for word in words.iter_mut() {
        let product = u128::from(*word) * u128::from(multiplier) + carry;
        *word = low_u64(product);
        carry = product >> 64;
    }
    if carry != 0 {
        words.push(low_u64(carry));
    }
}

fn add_words_small(words: &mut Vec<u64>, addend: u64) {
    let mut carry = u128::from(addend);
    let mut idx = 0usize;
    while carry != 0 {
        if idx == words.len() {
            words.push(0);
        }
        let sum = u128::from(words[idx]) + carry;
        words[idx] = low_u64(sum);
        carry = sum >> 64;
        idx += 1;
    }
}

fn digit_value(ch: char) -> Option<u8> {
    match ch {
        '0'..='9' => u8::try_from(u32::from(ch) - u32::from('0')).ok(),
        'a'..='z' => u8::try_from(u32::from(ch) - u32::from('a') + 10).ok(),
        'A'..='Z' => u8::try_from(u32::from(ch) - u32::from('A') + 10).ok(),
        _ => None,
    }
}

/// Upper-case, matching `APInt::toString`'s `UpperCase = true` default.
fn digit_char(digit: u8) -> char {
    match digit {
        0..=9 => char::from(b'0' + digit),
        10..=35 => char::from(b'A' + (digit - 10)),
        _ => '?',
    }
}

fn unsigned_u64_fits(bit_width: u32, value: u64) -> bool {
    if bit_width == 0 {
        value == 0
    } else if bit_width >= 64 {
        true
    } else {
        value <= ((1u64 << bit_width) - 1)
    }
}

fn signed_u64_pattern_fits(bit_width: u32, value: u64) -> bool {
    if bit_width == 0 {
        return value == 0 || value == u64::MAX;
    }
    if bit_width >= 64 {
        return true;
    }
    let mask = (1u64 << bit_width) - 1;
    let low = value & mask;
    let sign_bit = 1u64 << (bit_width - 1);
    let canonical = if (low & sign_bit) != 0 {
        low | !mask
    } else {
        low
    };
    value == canonical
}

fn bits_used_in_words(words: &[u64]) -> u32 {
    let mut idx = words.len();
    while idx > 0 {
        idx -= 1;
        let word = words[idx];
        if word != 0 {
            let leading = word.leading_zeros();
            let base = match u32::try_from(idx) {
                Ok(v) => v.saturating_mul(WORD_BITS),
                Err(_) => return u32::MAX,
            };
            return base + (WORD_BITS - leading);
        }
    }
    0
}

fn canonicalize_words(bit_width: u32, words: &mut Vec<u64>) {
    let limit = words_for_bits_usize(bit_width);
    if words.len() > limit {
        words.truncate(limit);
    }
    if bit_width == 0 {
        words.clear();
        return;
    }
    if let Some(top) = words.get_mut(limit.saturating_sub(1)) {
        *top &= top_mask(bit_width);
    }
    trim_trailing_zeros(words);
}

fn trim_trailing_zeros(words: &mut Vec<u64>) {
    while matches!(words.last(), Some(0)) {
        words.pop();
    }
}

fn top_mask(bit_width: u32) -> u64 {
    if bit_width == 0 {
        0
    } else {
        let rem = bit_width % WORD_BITS;
        if rem == 0 {
            u64::MAX
        } else {
            u64::MAX >> (WORD_BITS - rem)
        }
    }
}

fn word_at(words: &[u64], idx: usize, bit_width: u32) -> u64 {
    let Some(word) = words.get(idx).copied() else {
        return 0;
    };
    if idx + 1 == words_for_bits_usize(bit_width) {
        word & top_mask(bit_width)
    } else {
        word
    }
}

fn words_for_bits_usize(bit_width: u32) -> usize {
    let words = (bit_width.saturating_add(WORD_BITS - 1)) / WORD_BITS;
    u32_to_usize(words)
}

fn u32_to_usize(value: u32) -> usize {
    match usize::try_from(value) {
        Ok(v) => v,
        Err(_) => unreachable!("u32 fits in usize on supported targets"),
    }
}

fn low_u64(value: u128) -> u64 {
    match u64::try_from(value & WORD_MASK_U128) {
        Ok(v) => v,
        Err(_) => unreachable!("masked low 64 bits fit in u64"),
    }
}
