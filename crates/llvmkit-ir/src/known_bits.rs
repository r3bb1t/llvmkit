//! Known zero/one bit facts for integer-like values.
//!
//! Mirrors the shape of `llvm::KnownBits` from `llvm/include/llvm/Support/KnownBits.h`.

use crate::ap_int::ApInt;
use crate::constants::ConstantIntValue;
use crate::int_width::IntWidth;
use crate::module::ModuleBrand;
use crate::{IrError, IrResult};
use core::fmt;
use core::ops::Not;

/// Struct for tracking known zeros and ones of a value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KnownBits {
    zero: ApInt,
    one: ApInt,
}

impl KnownBits {
    /// Constructs a known-bits fact from zero and one masks.
    pub fn from_zero_one(zero: ApInt, one: ApInt) -> IrResult<Self> {
        if zero.bit_width() != one.bit_width() {
            return Err(IrError::OperandWidthMismatch {
                lhs: zero.bit_width(),
                rhs: one.bit_width(),
            });
        }
        Ok(Self { zero, one })
    }

    /// Unknown facts over `bit_width` bits.
    #[inline]
    pub fn unknown(bit_width: u32) -> Self {
        Self {
            zero: ApInt::zero(bit_width),
            one: ApInt::zero(bit_width),
        }
    }

    /// Exact known-bits facts for a constant integer.
    pub fn from_ap_int(value: ApInt) -> Self {
        Self {
            zero: value.clone().not(),
            one: value,
        }
    }

    /// Exact known-bits facts for a constant integer handle.
    #[inline]
    pub fn from_constant_int<'ctx, W: IntWidth, B: ModuleBrand + 'ctx>(
        value: ConstantIntValue<'ctx, W, B>,
    ) -> Self {
        Self::from_ap_int(value.ap_int())
    }

    #[inline]
    pub fn make_constant(value: ApInt) -> Self {
        Self::from_ap_int(value)
    }

    /// Known bits for a truncation.
    pub fn trunc(&self, bit_width: u32) -> KnownBits {
        KnownBits {
            zero: self
                .zero
                .trunc(bit_width)
                .unwrap_or_else(|| ApInt::zero(bit_width)),
            one: self
                .one
                .trunc(bit_width)
                .unwrap_or_else(|| ApInt::zero(bit_width)),
        }
    }

    /// Known bits for an extension that leaves the high bits unknown.
    pub fn anyext(&self, bit_width: u32) -> KnownBits {
        KnownBits {
            zero: self
                .zero
                .zext(bit_width)
                .unwrap_or_else(|| ApInt::zero(bit_width)),
            one: self
                .one
                .zext(bit_width)
                .unwrap_or_else(|| ApInt::zero(bit_width)),
        }
    }

    /// Known bits for a zero extension.
    pub fn zext(&self, bit_width: u32) -> KnownBits {
        let old_width = self.bit_width();
        let high_zero = if bit_width > old_width {
            ApInt::bits_set(bit_width, old_width, bit_width)
        } else {
            ApInt::zero(bit_width)
        };
        KnownBits {
            zero: self
                .zero
                .zext(bit_width)
                .unwrap_or_else(|| ApInt::zero(bit_width))
                .bitor(&high_zero),
            one: self
                .one
                .zext(bit_width)
                .unwrap_or_else(|| ApInt::zero(bit_width)),
        }
    }

    /// Known bits for a sign extension.
    pub fn sext(&self, bit_width: u32) -> KnownBits {
        let old_width = self.bit_width();
        if bit_width <= old_width {
            return self.trunc(bit_width);
        }
        let mut zero = self
            .zero
            .zext(bit_width)
            .unwrap_or_else(|| ApInt::zero(bit_width));
        let mut one = self
            .one
            .zext(bit_width)
            .unwrap_or_else(|| ApInt::zero(bit_width));
        if old_width != 0 {
            let high = ApInt::bits_set(bit_width, old_width, bit_width);
            if self.is_known_zero(old_width - 1) {
                zero = zero.bitor(&high);
            }
            if self.is_known_one(old_width - 1) {
                one = one.bitor(&high);
            }
        }
        KnownBits { zero, one }
    }

    /// Known bits for any-extension or truncation.
    pub fn anyext_or_trunc(&self, bit_width: u32) -> KnownBits {
        if bit_width > self.bit_width() {
            self.anyext(bit_width)
        } else if bit_width < self.bit_width() {
            self.trunc(bit_width)
        } else {
            self.clone()
        }
    }

    /// Known bits for zero-extension or truncation.
    pub fn zext_or_trunc(&self, bit_width: u32) -> KnownBits {
        if bit_width > self.bit_width() {
            self.zext(bit_width)
        } else if bit_width < self.bit_width() {
            self.trunc(bit_width)
        } else {
            self.clone()
        }
    }

    /// Known bits for sign-extension or truncation.
    pub fn sext_or_trunc(&self, bit_width: u32) -> KnownBits {
        if bit_width > self.bit_width() {
            self.sext(bit_width)
        } else if bit_width < self.bit_width() {
            self.trunc(bit_width)
        } else {
            self.clone()
        }
    }

    pub fn insert_bits(&mut self, sub_bits: &KnownBits, bit_position: u32) {
        self.zero.insert_bits(&sub_bits.zero, bit_position);
        self.one.insert_bits(&sub_bits.one, bit_position);
    }

    pub fn extract_bits(&self, num_bits: u32, bit_position: u32) -> KnownBits {
        KnownBits {
            zero: self.zero.extract_bits(num_bits, bit_position),
            one: self.one.extract_bits(num_bits, bit_position),
        }
    }

    pub fn concat(&self, lo: &KnownBits) -> KnownBits {
        KnownBits {
            zero: self.zero.concat(&lo.zero),
            one: self.one.concat(&lo.one),
        }
    }

    pub fn sext_in_reg(&self, src_bit_width: u32) -> KnownBits {
        let bit_width = self.bit_width();
        if src_bit_width == 0 || src_bit_width > bit_width {
            return KnownBits::unknown(bit_width);
        }
        if src_bit_width == bit_width {
            return self.clone();
        }
        let ext_bits = bit_width - src_bit_width;
        KnownBits {
            zero: self.zero.shl(ext_bits).ashr(ext_bits),
            one: self.one.shl(ext_bits).ashr(ext_bits),
        }
    }

    pub fn make_ge(&self, value: &ApInt) -> KnownBits {
        let bit_width = self.bit_width();
        if value.bit_width() != bit_width {
            return KnownBits::unknown(bit_width);
        }
        let leading_le = self.zero.bitor(value).count_leading_ones();
        let high_mask = ApInt::bits_set(bit_width, bit_width.saturating_sub(leading_le), bit_width);
        KnownBits {
            zero: self.zero.clone(),
            one: self.one.bitor(&value.bitand(&high_mask)),
        }
    }

    /// Bit width tracked by this fact.
    #[inline]
    pub fn bit_width(&self) -> u32 {
        self.zero.bit_width()
    }

    /// Bits known to be zero.
    #[inline]
    pub fn zero_mask(&self) -> &ApInt {
        &self.zero
    }

    /// Bits known to be one.
    #[inline]
    pub fn one_mask(&self) -> &ApInt {
        &self.one
    }

    /// Returns true if a bit is known both zero and one.
    #[inline]
    pub fn has_conflict(&self) -> bool {
        self.zero.intersects(&self.one)
    }

    /// Returns true if no bits are known.
    #[inline]
    pub fn is_unknown(&self) -> bool {
        self.zero.is_zero() && self.one.is_zero()
    }

    /// Returns true if every bit has a known value.
    #[inline]
    pub fn is_constant(&self) -> bool {
        self.zero.popcount().saturating_add(self.one.popcount()) == self.bit_width()
    }

    /// Returns the constant value when every bit is known.
    #[inline]
    pub fn constant(&self) -> Option<ApInt> {
        self.is_constant().then(|| self.one.clone())
    }

    /// Returns true if `bit` is known zero.
    #[inline]
    pub fn is_known_zero(&self, bit: u32) -> bool {
        bit_is_set(&self.zero, bit)
    }

    /// Returns true if `bit` is known one.
    #[inline]
    pub fn is_known_one(&self, bit: u32) -> bool {
        bit_is_set(&self.one, bit)
    }

    /// Returns true if all bits are known zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.zero.is_all_ones()
    }

    /// Returns true if all bits are known one.
    #[inline]
    pub fn is_all_ones(&self) -> bool {
        self.one.is_all_ones()
    }

    /// Returns true if the sign bit is known one.
    #[inline]
    pub fn is_negative(&self) -> bool {
        self.bit_width() != 0 && self.is_known_one(self.bit_width() - 1)
    }

    /// Returns true if the sign bit is known zero.
    #[inline]
    pub fn is_non_negative(&self) -> bool {
        self.bit_width() != 0 && self.is_known_zero(self.bit_width() - 1)
    }

    /// Returns true if at least one bit is known one.
    #[inline]
    pub fn is_non_zero(&self) -> bool {
        !self.one.is_zero()
    }

    /// Returns true if the value is known positive in signed interpretation.
    #[inline]
    pub fn is_strictly_positive(&self) -> bool {
        self.is_non_negative() && !self.one.is_zero()
    }

    /// Returns true if the value is known non-positive in signed interpretation.
    #[inline]
    pub fn is_non_positive(&self) -> bool {
        self.signed_max_value().is_non_positive()
    }

    /// Minimal unsigned value possible.
    #[inline]
    pub fn min_value(&self) -> ApInt {
        self.one.clone()
    }

    /// Maximal unsigned value possible.
    #[inline]
    pub fn max_value(&self) -> ApInt {
        self.zero.clone().not()
    }

    /// Minimal signed value possible.
    pub fn signed_min_value(&self) -> ApInt {
        let width = self.bit_width();
        if width == 0 {
            return ApInt::zero(0);
        }
        let sign = ApInt::one_bit_set(width, width - 1);
        if self.zero.intersects(&sign) {
            self.one.clone()
        } else {
            self.one.bitor(&sign)
        }
    }

    /// Maximal signed value possible.
    pub fn signed_max_value(&self) -> ApInt {
        let width = self.bit_width();
        if width == 0 {
            return ApInt::zero(0);
        }
        let max = self.zero.clone().not();
        let sign = ApInt::one_bit_set(width, width - 1);
        if self.one.intersects(&sign) {
            max
        } else {
            max.bitand(&sign.not())
        }
    }

    /// Minimum trailing known-zero bits.
    #[inline]
    pub fn count_min_trailing_zeros(&self) -> u32 {
        self.zero.count_trailing_ones()
    }

    /// Minimum trailing known-one bits.
    #[inline]
    pub fn count_min_trailing_ones(&self) -> u32 {
        self.one.count_trailing_ones()
    }

    /// Minimum leading known-zero bits.
    #[inline]
    pub fn count_min_leading_zeros(&self) -> u32 {
        self.zero.count_leading_ones()
    }

    /// Minimum leading known-one bits.
    #[inline]
    pub fn count_min_leading_ones(&self) -> u32 {
        self.one.count_leading_ones()
    }

    /// Minimum sign bits known common to every possible value.
    pub fn count_min_sign_bits(&self) -> u32 {
        if self.is_non_negative() {
            self.count_min_leading_zeros()
        } else if self.is_negative() {
            self.count_min_leading_ones()
        } else {
            1
        }
    }

    /// Maximum signed significant bits needed by every possible value.
    #[inline]
    pub fn count_max_significant_bits(&self) -> u32 {
        self.bit_width()
            .saturating_sub(self.count_min_sign_bits())
            .saturating_add(1)
    }

    /// Maximum trailing zeros possible.
    #[inline]
    pub fn count_max_trailing_zeros(&self) -> u32 {
        self.one.count_trailing_zeros()
    }

    /// Maximum trailing ones possible.
    #[inline]
    pub fn count_max_trailing_ones(&self) -> u32 {
        self.zero.count_trailing_zeros()
    }

    /// Maximum leading zeros possible.
    #[inline]
    pub fn count_max_leading_zeros(&self) -> u32 {
        self.one.count_leading_zeros()
    }

    /// Maximum leading ones possible.
    #[inline]
    pub fn count_max_leading_ones(&self) -> u32 {
        self.zero.count_leading_zeros()
    }

    /// Minimum population count possible.
    #[inline]
    pub fn count_min_population(&self) -> u32 {
        self.one.popcount()
    }

    /// Maximum population count possible.
    #[inline]
    pub fn count_max_population(&self) -> u32 {
        self.bit_width().saturating_sub(self.zero.popcount())
    }

    /// Maximum active bits needed by every possible unsigned value.
    #[inline]
    pub fn count_max_active_bits(&self) -> u32 {
        self.bit_width()
            .saturating_sub(self.count_min_leading_zeros())
    }

    /// Known bits that are true for both inputs.
    #[inline]
    pub fn intersect_with(&self, rhs: &KnownBits) -> KnownBits {
        if self.bit_width() != rhs.bit_width() {
            return Self::unknown(self.bit_width());
        }
        Self {
            zero: self.zero.bitand(&rhs.zero),
            one: self.one.bitand(&rhs.one),
        }
    }

    /// Known bits that combine independent facts about the same value.
    #[inline]
    pub fn union_with(&self, rhs: &KnownBits) -> KnownBits {
        if self.bit_width() != rhs.bit_width() {
            return Self::unknown(self.bit_width());
        }
        Self {
            zero: self.zero.bitor(&rhs.zero),
            one: self.one.bitor(&rhs.one),
        }
    }

    pub fn set_all_zero(&mut self) {
        self.zero = ApInt::all_ones(self.bit_width());
        self.one = ApInt::zero(self.bit_width());
    }

    pub fn set_all_conflict(&mut self) {
        self.zero = ApInt::all_ones(self.bit_width());
        self.one = ApInt::all_ones(self.bit_width());
    }

    pub(crate) fn set_known_zero_bit(&mut self, bit: u32) {
        self.zero.set_bit(bit);
    }

    pub(crate) fn set_known_one_bit(&mut self, bit: u32) {
        self.one.set_bit(bit);
    }

    pub(crate) fn set_known_zero_bits_from(&mut self, bit: u32) {
        self.zero.set_bits_from(bit);
    }

    pub fn make_negative(&mut self) {
        self.one.set_sign_bit();
    }

    pub fn make_non_negative(&mut self) {
        self.zero.set_sign_bit();
    }

    pub fn have_no_common_bits_set(lhs: &KnownBits, rhs: &KnownBits) -> bool {
        if lhs.bit_width() != rhs.bit_width() {
            return false;
        }
        lhs.zero.bitor(&rhs.zero).is_all_ones()
    }

    /// Bitwise NOT transfer.
    #[inline]
    pub fn not(&self) -> KnownBits {
        Self {
            zero: self.one.clone(),
            one: self.zero.clone(),
        }
    }

    /// Bitwise AND transfer.
    #[inline]
    pub fn bitand(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        if lhs.bit_width() != rhs.bit_width() {
            return Self::unknown(lhs.bit_width());
        }
        Self {
            zero: lhs.zero.bitor(&rhs.zero),
            one: lhs.one.bitand(&rhs.one),
        }
    }

    /// Bitwise OR transfer.
    #[inline]
    pub fn bitor(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        if lhs.bit_width() != rhs.bit_width() {
            return Self::unknown(lhs.bit_width());
        }
        Self {
            zero: lhs.zero.bitand(&rhs.zero),
            one: lhs.one.bitor(&rhs.one),
        }
    }

    /// Bitwise XOR transfer.
    pub fn bitxor(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        if lhs.bit_width() != rhs.bit_width() {
            return Self::unknown(lhs.bit_width());
        }
        let zero = lhs.zero.bitand(&rhs.zero).bitor(&lhs.one.bitand(&rhs.one));
        let one = lhs.zero.bitand(&rhs.one).bitor(&lhs.one.bitand(&rhs.zero));
        Self { zero, one }
    }

    /// Add transfer.
    #[inline]
    pub fn add(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        Self::add_with_flags(lhs, rhs, false, false)
    }

    pub fn compute_for_add_carry(lhs: &KnownBits, rhs: &KnownBits, carry: &KnownBits) -> KnownBits {
        if lhs.bit_width() != rhs.bit_width() || carry.bit_width() != 1 {
            return Self::unknown(lhs.bit_width());
        }
        compute_for_add_carry_raw(lhs, rhs, carry.zero.bool_value(), carry.one.bool_value())
    }

    pub fn compute_for_add_sub(
        add: bool,
        nsw: bool,
        nuw: bool,
        lhs: &KnownBits,
        rhs: &KnownBits,
    ) -> KnownBits {
        compute_for_add_sub_impl(add, nsw, nuw, lhs, rhs)
    }

    pub fn compute_for_sub_borrow(
        lhs: &KnownBits,
        mut rhs: KnownBits,
        borrow: &KnownBits,
    ) -> KnownBits {
        if lhs.bit_width() != rhs.bit_width() || borrow.bit_width() != 1 {
            return Self::unknown(lhs.bit_width());
        }
        core::mem::swap(&mut rhs.zero, &mut rhs.one);
        compute_for_add_carry_raw(lhs, &rhs, borrow.one.bool_value(), borrow.zero.bool_value())
    }

    pub fn add_with_flags(lhs: &KnownBits, rhs: &KnownBits, nsw: bool, nuw: bool) -> KnownBits {
        Self::compute_for_add_sub(true, nsw, nuw, lhs, rhs)
    }

    /// Sub transfer.
    #[inline]
    pub fn sub(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        Self::sub_with_flags(lhs, rhs, false, false)
    }

    pub fn sub_with_flags(lhs: &KnownBits, rhs: &KnownBits, nsw: bool, nuw: bool) -> KnownBits {
        Self::compute_for_add_sub(false, nsw, nuw, lhs, rhs)
    }

    /// Multiply transfer.
    #[inline]
    pub fn mul(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        compute_for_mul(lhs, rhs, false)
    }

    pub fn mulhs(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        let bit_width = lhs.bit_width();
        if bit_width != rhs.bit_width() {
            return KnownBits::unknown(bit_width);
        }
        compute_for_mul(
            &lhs.sext(bit_width.saturating_mul(2)),
            &rhs.sext(bit_width.saturating_mul(2)),
            false,
        )
        .extract_bits(bit_width, bit_width)
    }

    pub fn mulhu(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        let bit_width = lhs.bit_width();
        if bit_width != rhs.bit_width() {
            return KnownBits::unknown(bit_width);
        }
        compute_for_mul(
            &lhs.zext(bit_width.saturating_mul(2)),
            &rhs.zext(bit_width.saturating_mul(2)),
            false,
        )
        .extract_bits(bit_width, bit_width)
    }

    /// Shift-left transfer.
    #[inline]
    pub fn shl(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        Self::shl_with_flags(lhs, rhs, false, false, false)
    }

    pub fn shl_with_flags(
        lhs: &KnownBits,
        rhs: &KnownBits,
        nuw: bool,
        nsw: bool,
        shift_amount_non_zero: bool,
    ) -> KnownBits {
        compute_for_shl(lhs, rhs, nuw, nsw, shift_amount_non_zero)
    }

    /// Logical-shift-right transfer.
    #[inline]
    pub fn lshr(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        Self::lshr_with_flags(lhs, rhs, false, false)
    }

    pub fn lshr_with_flags(
        lhs: &KnownBits,
        rhs: &KnownBits,
        shift_amount_non_zero: bool,
        exact: bool,
    ) -> KnownBits {
        compute_for_lshr(lhs, rhs, shift_amount_non_zero, exact)
    }

    /// Arithmetic-shift-right transfer.
    #[inline]
    pub fn ashr(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        Self::ashr_with_flags(lhs, rhs, false, false)
    }

    pub fn ashr_with_flags(
        lhs: &KnownBits,
        rhs: &KnownBits,
        shift_amount_non_zero: bool,
        exact: bool,
    ) -> KnownBits {
        compute_for_ashr(lhs, rhs, shift_amount_non_zero, exact)
    }

    /// Unsigned division transfer.
    #[inline]
    pub fn udiv(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        Self::udiv_with_exact(lhs, rhs, false)
    }

    pub fn udiv_with_exact(lhs: &KnownBits, rhs: &KnownBits, exact: bool) -> KnownBits {
        compute_for_udiv(lhs, rhs, exact)
    }

    pub fn sdiv_with_exact(lhs: &KnownBits, rhs: &KnownBits, exact: bool) -> KnownBits {
        compute_for_sdiv(lhs, rhs, exact)
    }

    /// Unsigned remainder transfer.
    #[inline]
    pub fn urem(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        compute_for_urem(lhs, rhs)
    }

    pub fn srem(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        compute_for_srem(lhs, rhs)
    }

    pub fn sadd_sat(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        compute_for_sat_add_sub(true, true, lhs, rhs)
    }

    pub fn uadd_sat(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        compute_for_sat_add_sub(true, false, lhs, rhs)
    }

    pub fn ssub_sat(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        compute_for_sat_add_sub(false, true, lhs, rhs)
    }

    pub fn usub_sat(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        compute_for_sat_add_sub(false, false, lhs, rhs)
    }

    pub fn avg_floor_s(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        flip_sign_bit(&Self::avg_floor_u(&flip_sign_bit(lhs), &flip_sign_bit(rhs)))
    }

    pub fn avg_floor_u(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        avg_compute_u(lhs, rhs, false)
    }

    pub fn avg_ceil_s(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        flip_sign_bit(&Self::avg_ceil_u(&flip_sign_bit(lhs), &flip_sign_bit(rhs)))
    }

    pub fn avg_ceil_u(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        avg_compute_u(lhs, rhs, true)
    }

    /// Unsigned minimum transfer.
    #[inline]
    pub fn umin(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        let flip = |value: &KnownBits| KnownBits {
            zero: value.one.clone(),
            one: value.zero.clone(),
        };
        flip(&Self::umax(&flip(lhs), &flip(rhs)))
    }

    /// Unsigned maximum transfer.
    #[inline]
    pub fn umax(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        if lhs.min_value().uge(&rhs.max_value()) {
            return lhs.clone();
        }
        if rhs.min_value().uge(&lhs.max_value()) {
            return rhs.clone();
        }
        lhs.make_ge(&rhs.min_value())
            .intersect_with(&rhs.make_ge(&lhs.min_value()))
    }

    /// Signed minimum transfer.
    #[inline]
    pub fn smin(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        let flip = |value: &KnownBits| {
            let bit_width = value.bit_width();
            let sign = bit_width.saturating_sub(1);
            let mut zero = value.one.clone();
            let mut one = value.zero.clone();
            if value.is_known_zero(sign) {
                zero.set_bit(sign);
            } else {
                zero.clear_bit(sign);
            }
            if value.is_known_one(sign) {
                one.set_bit(sign);
            } else {
                one.clear_bit(sign);
            }
            KnownBits { zero, one }
        };
        flip(&Self::umax(&flip(lhs), &flip(rhs)))
    }

    /// Signed maximum transfer.
    #[inline]
    pub fn smax(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        flip_sign_bit(&Self::umax(&flip_sign_bit(lhs), &flip_sign_bit(rhs)))
    }

    /// Unsigned absolute difference transfer.
    #[inline]
    pub fn abdu(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        if lhs.min_value().uge(&rhs.max_value()) {
            return Self::sub(lhs, rhs);
        }
        if rhs.min_value().uge(&lhs.max_value()) {
            return Self::sub(rhs, lhs);
        }
        Self::sub_with_flags(lhs, rhs, false, true)
            .intersect_with(&Self::sub_with_flags(rhs, lhs, false, true))
    }

    /// Signed absolute difference transfer.
    #[inline]
    pub fn abds(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
        if lhs.signed_min_value().sge(&rhs.signed_max_value()) {
            return Self::sub(lhs, rhs);
        }
        if rhs.signed_min_value().sge(&lhs.signed_max_value()) {
            return Self::sub(rhs, lhs);
        }
        let lhs_flipped = flip_sign_bit(lhs);
        let rhs_flipped = flip_sign_bit(rhs);
        Self::sub_with_flags(&lhs_flipped, &rhs_flipped, false, true).intersect_with(
            &Self::sub_with_flags(&rhs_flipped, &lhs_flipped, false, true),
        )
    }

    pub fn eq(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        if lhs.is_constant() && rhs.is_constant() {
            return Some(lhs.one.eq_ap_int(&rhs.one));
        }
        if lhs.one.intersects(&rhs.zero) || rhs.one.intersects(&lhs.zero) {
            return Some(false);
        }
        None
    }

    pub fn ne(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        Self::eq(lhs, rhs).map(core::ops::Not::not)
    }

    pub fn ugt(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        if lhs.max_value().ule(&rhs.min_value()) {
            return Some(false);
        }
        if lhs.min_value().ugt(&rhs.max_value()) {
            return Some(true);
        }
        None
    }

    pub fn uge(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        Self::ugt(rhs, lhs).map(core::ops::Not::not)
    }

    pub fn ult(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        Self::ugt(rhs, lhs)
    }

    pub fn ule(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        Self::uge(rhs, lhs)
    }

    pub fn sgt(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        if lhs.signed_max_value().sle(&rhs.signed_min_value()) {
            return Some(false);
        }
        if lhs.signed_min_value().sgt(&rhs.signed_max_value()) {
            return Some(true);
        }
        None
    }

    pub fn sge(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        Self::sgt(rhs, lhs).map(core::ops::Not::not)
    }

    pub fn slt(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        Self::sgt(rhs, lhs)
    }

    pub fn sle(lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
        Self::sge(rhs, lhs)
    }

    /// Absolute-value transfer.
    #[inline]
    pub fn abs(&self) -> KnownBits {
        self.abs_with_int_min_poison(false)
    }

    pub fn abs_with_int_min_poison(&self, int_min_is_poison: bool) -> KnownBits {
        compute_for_abs(self, int_min_is_poison)
    }

    pub fn reduce_add(&self, num_elts: u32) -> KnownBits {
        compute_for_reduce_add(self, num_elts)
    }

    pub fn byte_swap(&self) -> KnownBits {
        KnownBits {
            zero: self.zero.byte_swap(),
            one: self.one.byte_swap(),
        }
    }

    pub fn reverse_bits(&self) -> KnownBits {
        KnownBits {
            zero: self.zero.reverse_bits(),
            one: self.one.reverse_bits(),
        }
    }

    pub fn blsi(&self) -> KnownBits {
        let bit_width = self.bit_width();
        let mut known = KnownBits {
            zero: self.zero.clone(),
            one: ApInt::zero(bit_width),
        };
        let max = self.count_max_trailing_zeros();
        known
            .zero
            .set_bits_from(max.saturating_add(1).min(bit_width));
        let min = self.count_min_trailing_zeros();
        if max == min && max < bit_width {
            known.one.set_bit(max);
        }
        known
    }

    pub fn blsmsk(&self) -> KnownBits {
        let bit_width = self.bit_width();
        let mut known = KnownBits::unknown(bit_width);
        let max = self.count_max_trailing_zeros();
        known
            .zero
            .set_bits_from(max.saturating_add(1).min(bit_width));
        let min = self.count_min_trailing_zeros();
        known.one.set_low_bits(min.saturating_add(1).min(bit_width));
        known
    }
}

impl fmt::Display for KnownBits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let width = self.bit_width();
        let mut bit = width;
        while bit > 0 {
            bit -= 1;
            match (self.is_known_zero(bit), self.is_known_one(bit)) {
                (true, true) => f.write_str("!")?,
                (true, false) => f.write_str("0")?,
                (false, true) => f.write_str("1")?,
                (false, false) => f.write_str("?")?,
            }
        }
        Ok(())
    }
}

fn ap_one(bit_width: u32) -> ApInt {
    ApInt::from_words(bit_width, &[1])
}

fn set_known_range(mask: &mut ApInt, bit_width: u32, lo: u32, hi: u32) {
    let set = ApInt::bits_set(bit_width, lo, hi);
    *mask = mask.bitor(&set);
}

fn flip_sign_bit(value: &KnownBits) -> KnownBits {
    let bit_width = value.bit_width();
    if bit_width == 0 {
        return value.clone();
    }
    let sign = bit_width - 1;
    let mut zero = value.zero.clone();
    let mut one = value.one.clone();
    if value.is_known_one(sign) {
        zero.set_bit(sign);
    } else {
        zero.clear_bit(sign);
    }
    if value.is_known_zero(sign) {
        one.set_bit(sign);
    } else {
        one.clear_bit(sign);
    }
    KnownBits { zero, one }
}

fn compute_for_add_carry_raw(
    lhs: &KnownBits,
    rhs: &KnownBits,
    carry_zero: bool,
    carry_one: bool,
) -> KnownBits {
    let bit_width = lhs.bit_width();
    if bit_width != rhs.bit_width() {
        return KnownBits::unknown(bit_width);
    }
    let carry_zero_addend = if carry_zero {
        ApInt::zero(bit_width)
    } else {
        ap_one(bit_width)
    };
    let carry_one_addend = if carry_one {
        ap_one(bit_width)
    } else {
        ApInt::zero(bit_width)
    };
    let possible_sum_zero = lhs
        .max_value()
        .wrapping_add(&rhs.max_value())
        .wrapping_add(&carry_zero_addend);
    let possible_sum_one = lhs
        .min_value()
        .wrapping_add(&rhs.min_value())
        .wrapping_add(&carry_one_addend);
    let carry_known_zero = possible_sum_zero.bitxor(&lhs.zero).bitxor(&rhs.zero).not();
    let carry_known_one = possible_sum_one.bitxor(&lhs.one).bitxor(&rhs.one);
    let known = lhs
        .zero
        .bitor(&lhs.one)
        .bitand(&rhs.zero.bitor(&rhs.one))
        .bitand(&carry_known_zero.bitor(&carry_known_one));
    KnownBits {
        zero: possible_sum_zero.not().bitand(&known),
        one: possible_sum_one.bitand(&known),
    }
}

fn compute_for_add_sub_impl(
    add: bool,
    nsw: bool,
    nuw: bool,
    lhs: &KnownBits,
    rhs: &KnownBits,
) -> KnownBits {
    let bit_width = lhs.bit_width();
    if bit_width != rhs.bit_width() {
        return KnownBits::unknown(bit_width);
    }
    let mut known_out = KnownBits::unknown(bit_width);
    if !lhs.is_unknown() && !rhs.is_unknown() {
        known_out = if add {
            compute_for_add_carry_raw(lhs, rhs, true, false)
        } else {
            let mut not_rhs = rhs.clone();
            core::mem::swap(&mut not_rhs.zero, &mut not_rhs.one);
            compute_for_add_carry_raw(lhs, &not_rhs, false, true)
        };
    }

    if bit_width == 0 {
        return known_out;
    }

    if nuw {
        if add {
            let min_val = lhs.min_value().uadd_sat(&rhs.min_value());
            if nsw && bit_width > 1 {
                let num_bits = min_val
                    .trunc(bit_width - 1)
                    .unwrap_or_else(|| ApInt::zero(bit_width - 1))
                    .count_leading_ones();
                set_known_range(
                    &mut known_out.one,
                    bit_width,
                    bit_width - 1 - num_bits,
                    bit_width - 1,
                );
            }
            known_out.one.set_high_bits(min_val.count_leading_ones());
        } else {
            let max_val = lhs.max_value().usub_sat(&rhs.min_value());
            if nsw && bit_width > 1 {
                let num_bits = max_val
                    .trunc(bit_width - 1)
                    .unwrap_or_else(|| ApInt::zero(bit_width - 1))
                    .count_leading_zeros();
                set_known_range(
                    &mut known_out.zero,
                    bit_width,
                    bit_width - 1 - num_bits,
                    bit_width - 1,
                );
            }
            known_out.zero.set_high_bits(max_val.count_leading_zeros());
        }
    }

    if nsw {
        let (min_val, max_val) = if add {
            (
                lhs.signed_min_value().sadd_sat(&rhs.signed_min_value()),
                lhs.signed_max_value().sadd_sat(&rhs.signed_max_value()),
            )
        } else {
            (
                lhs.signed_min_value().ssub_sat(&rhs.signed_max_value()),
                lhs.signed_max_value().ssub_sat(&rhs.signed_min_value()),
            )
        };
        if min_val.is_non_negative() {
            if bit_width > 1 {
                let num_bits = min_val
                    .trunc(bit_width - 1)
                    .unwrap_or_else(|| ApInt::zero(bit_width - 1))
                    .count_leading_ones();
                set_known_range(
                    &mut known_out.one,
                    bit_width,
                    bit_width - 1 - num_bits,
                    bit_width - 1,
                );
            }
            known_out.zero.set_sign_bit();
        }
        if max_val.is_negative() {
            if bit_width > 1 {
                let num_bits = max_val
                    .trunc(bit_width - 1)
                    .unwrap_or_else(|| ApInt::zero(bit_width - 1))
                    .count_leading_zeros();
                set_known_range(
                    &mut known_out.zero,
                    bit_width,
                    bit_width - 1 - num_bits,
                    bit_width - 1,
                );
            }
            known_out.one.set_sign_bit();
        }
    }

    if known_out.has_conflict() {
        known_out.set_all_zero();
    }
    known_out
}

fn clear_low_bits(value: &mut ApInt, count: u32) {
    let keep = ApInt::bits_set_from(value.bit_width(), count.min(value.bit_width()));
    let updated = value.bitand(&keep);
    *value = updated;
}

fn compute_for_sat_add_sub(add: bool, signed: bool, lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
    let bit_width = lhs.bit_width();
    if bit_width != rhs.bit_width() {
        return KnownBits::unknown(bit_width);
    }

    let mut overflow: Option<bool> = None;
    let mut may_neg_clamp = true;
    let mut may_pos_clamp = true;
    if signed {
        let easy_no_overflow = (add
            && ((lhs.is_negative() && rhs.is_non_negative())
                || (lhs.is_non_negative() && rhs.is_negative())))
            || (!add
                && ((lhs.is_negative() && rhs.is_negative())
                    || (lhs.is_non_negative() && rhs.is_non_negative())));
        if easy_no_overflow {
            overflow = Some(false);
        } else {
            let mut unsigned_lhs = lhs.clone();
            let mut unsigned_rhs = rhs.clone();
            unsigned_lhs.one.clear_sign_bit();
            unsigned_lhs.zero.set_sign_bit();
            unsigned_rhs.one.clear_sign_bit();
            unsigned_rhs.zero.set_sign_bit();
            let res = compute_for_add_sub_impl(add, false, false, &unsigned_lhs, &unsigned_rhs);
            if add {
                if res.is_negative() {
                    may_neg_clamp = false;
                    if lhs.is_non_negative() && rhs.is_non_negative() {
                        overflow = Some(true);
                    }
                } else if res.is_non_negative() {
                    may_pos_clamp = false;
                    if lhs.is_negative() && rhs.is_negative() {
                        overflow = Some(true);
                    }
                }
                if lhs.is_negative() || rhs.is_negative() {
                    may_pos_clamp = false;
                }
                if lhs.is_non_negative() || rhs.is_non_negative() {
                    may_neg_clamp = false;
                }
            } else {
                if res.is_negative() {
                    may_pos_clamp = false;
                    if lhs.is_negative() && rhs.is_non_negative() {
                        overflow = Some(true);
                    }
                } else if res.is_non_negative() {
                    may_neg_clamp = false;
                    if lhs.is_non_negative() && rhs.is_negative() {
                        overflow = Some(true);
                    }
                }
                if lhs.is_negative() || rhs.is_non_negative() {
                    may_pos_clamp = false;
                }
                if lhs.is_non_negative() || rhs.is_negative() {
                    may_neg_clamp = false;
                }
            }
        }
        if !may_neg_clamp && !may_pos_clamp {
            overflow = Some(false);
        }
    } else if add {
        let (_, max_overflow) = lhs.max_value().uadd_ov(&rhs.max_value());
        if !max_overflow {
            overflow = Some(false);
        } else {
            let (_, min_overflow) = lhs.min_value().uadd_ov(&rhs.min_value());
            if min_overflow {
                overflow = Some(true);
            }
        }
    } else {
        let (_, max_underflow) = lhs.min_value().usub_ov(&rhs.max_value());
        if !max_underflow {
            overflow = Some(false);
        } else {
            let (_, min_underflow) = lhs.max_value().usub_ov(&rhs.min_value());
            if min_underflow {
                overflow = Some(true);
            }
        }
    }

    let mut res = compute_for_add_sub_impl(add, signed, !signed, lhs, rhs);
    if let Some(did_overflow) = overflow {
        if !did_overflow {
            return res;
        }
        let clamp = if signed {
            if lhs.is_negative() {
                ApInt::signed_min_value(bit_width)
            } else if lhs.is_non_negative() {
                ApInt::signed_max_value(bit_width)
            } else {
                unreachable!("known signed saturation overflow requires known input sign")
            }
        } else if add {
            ApInt::max_value(bit_width)
        } else {
            ApInt::zero(bit_width)
        };
        return KnownBits {
            zero: clamp.clone().not(),
            one: clamp,
        };
    }

    if signed {
        if may_pos_clamp {
            clear_low_bits(&mut res.zero, bit_width.saturating_sub(1));
        }
        if may_neg_clamp {
            clear_low_bits(&mut res.one, bit_width.saturating_sub(1));
        }
    } else if add {
        res.zero = ApInt::zero(bit_width);
    } else {
        res.one = ApInt::zero(bit_width);
    }
    res
}

fn is_power_of_two_u32(value: u32) -> bool {
    value != 0 && (value & (value - 1)) == 0
}

fn log2_floor_u32(value: u32) -> u32 {
    if value == 0 {
        return 0;
    }
    u32::BITS - 1 - value.leading_zeros()
}

fn log2_ceil_u32(value: u32) -> u32 {
    if value <= 1 {
        return 0;
    }
    let floor = log2_floor_u32(value - 1);
    floor + 1
}

fn low_u32(value: &ApInt) -> u32 {
    u32::try_from(value.zext_or_trunc(32).try_zext_u64().unwrap_or(0)).unwrap_or(0)
}

fn get_max_shift_amount(max_value: &ApInt, bit_width: u32) -> u32 {
    if bit_width == 0 {
        return 0;
    }
    if is_power_of_two_u32(bit_width) {
        let bits = log2_floor_u32(bit_width);
        return u32::try_from(max_value.extract_bits(bits, 0).try_zext_u64().unwrap_or(0))
            .unwrap_or(bit_width - 1);
    }
    u32::try_from(max_value.limited_value(u64::from(bit_width - 1))).unwrap_or(bit_width - 1)
}

fn shift_amount_possible(rhs: &KnownBits, amount: u32) -> bool {
    let zero_mask = low_u32(&rhs.zero);
    let one_mask = low_u32(&rhs.one);
    (zero_mask & amount) == 0 && (one_mask | amount) == amount
}

fn compute_for_shl(
    lhs: &KnownBits,
    rhs: &KnownBits,
    nuw: bool,
    nsw: bool,
    shift_amount_non_zero: bool,
) -> KnownBits {
    let bit_width = lhs.bit_width();
    let mut known = KnownBits::unknown(bit_width);
    if bit_width == 0 {
        return known;
    }
    let mut min_shift =
        u32::try_from(rhs.min_value().limited_value(u64::from(bit_width))).unwrap_or(bit_width);
    if min_shift == 0 && shift_amount_non_zero {
        min_shift = 1;
    }
    if lhs.is_unknown() {
        known.zero.set_low_bits(min_shift);
        if nuw && nsw && min_shift != 0 {
            known.make_non_negative();
        }
        return known;
    }
    let mut max_shift = get_max_shift_amount(&rhs.max_value(), bit_width);
    if nuw && nsw {
        max_shift = max_shift.min(lhs.count_max_leading_zeros().saturating_sub(1));
    }
    if nuw {
        max_shift = max_shift.min(lhs.count_max_leading_zeros());
    }
    if nsw {
        max_shift = max_shift.min(
            lhs.count_max_leading_zeros()
                .max(lhs.count_max_leading_ones())
                .saturating_sub(1),
        );
    }
    if min_shift == 0 && max_shift == bit_width - 1 && is_power_of_two_u32(bit_width) {
        known.zero.set_low_bits(lhs.count_min_trailing_zeros());
        if lhs.is_all_ones() {
            known.one.set_sign_bit();
        }
        if nsw {
            if lhs.is_non_negative() {
                known.make_non_negative();
            }
            if lhs.is_negative() {
                known.make_negative();
            }
        }
        return known;
    }
    known.set_all_conflict();
    let mut shift = min_shift;
    while shift <= max_shift {
        if shift_amount_possible(rhs, shift) {
            let (zero, shifted_out_zero) = lhs.zero.ushl_ov(shift);
            let (one, shifted_out_one) = lhs.one.ushl_ov(shift);
            let mut shifted = KnownBits { zero, one };
            shifted.zero.set_low_bits(shift);
            if nsw {
                if (nuw && shift != 0) || shifted_out_zero {
                    shifted.make_non_negative();
                } else if shifted_out_one {
                    shifted.make_negative();
                }
            }
            known = known.intersect_with(&shifted);
            if known.is_unknown() {
                break;
            }
        }
        if shift == u32::MAX {
            break;
        }
        shift += 1;
    }
    if known.has_conflict() {
        known.set_all_zero();
    }
    known
}

fn compute_for_lshr(
    lhs: &KnownBits,
    rhs: &KnownBits,
    shift_amount_non_zero: bool,
    exact: bool,
) -> KnownBits {
    let bit_width = lhs.bit_width();
    let mut known = KnownBits::unknown(bit_width);
    if bit_width == 0 {
        return known;
    }
    let mut min_shift =
        u32::try_from(rhs.min_value().limited_value(u64::from(bit_width))).unwrap_or(bit_width);
    if min_shift == 0 && shift_amount_non_zero {
        min_shift = 1;
    }
    if lhs.is_unknown() {
        known.zero.set_high_bits(min_shift);
        return known;
    }
    let mut max_shift = get_max_shift_amount(&rhs.max_value(), bit_width);
    if exact {
        let first_one = lhs.count_max_trailing_zeros();
        if first_one < min_shift {
            known.set_all_zero();
            return known;
        }
        max_shift = max_shift.min(first_one);
    }
    known.set_all_conflict();
    let mut shift = min_shift;
    while shift <= max_shift {
        if shift_amount_possible(rhs, shift) {
            let mut shifted = KnownBits {
                zero: lhs.zero.lshr(shift),
                one: lhs.one.lshr(shift),
            };
            shifted.zero.set_high_bits(shift);
            known = known.intersect_with(&shifted);
            if known.is_unknown() {
                break;
            }
        }
        if shift == u32::MAX {
            break;
        }
        shift += 1;
    }
    if known.has_conflict() {
        known.set_all_zero();
    }
    known
}

fn compute_for_ashr(
    lhs: &KnownBits,
    rhs: &KnownBits,
    shift_amount_non_zero: bool,
    exact: bool,
) -> KnownBits {
    let bit_width = lhs.bit_width();
    let mut known = KnownBits::unknown(bit_width);
    if bit_width == 0 {
        return known;
    }
    let mut min_shift =
        u32::try_from(rhs.min_value().limited_value(u64::from(bit_width))).unwrap_or(bit_width);
    if min_shift == 0 && shift_amount_non_zero {
        min_shift = 1;
    }
    if lhs.is_unknown() {
        if min_shift == bit_width {
            known.set_all_zero();
        }
        return known;
    }
    let mut max_shift = get_max_shift_amount(&rhs.max_value(), bit_width);
    if exact {
        let first_one = lhs.count_max_trailing_zeros();
        if first_one < min_shift {
            known.set_all_zero();
            return known;
        }
        max_shift = max_shift.min(first_one);
    }
    known.set_all_conflict();
    let mut shift = min_shift;
    while shift <= max_shift {
        if shift_amount_possible(rhs, shift) {
            let shifted = KnownBits {
                zero: lhs.zero.ashr(shift),
                one: lhs.one.ashr(shift),
            };
            known = known.intersect_with(&shifted);
            if known.is_unknown() {
                break;
            }
        }
        if shift == u32::MAX {
            break;
        }
        shift += 1;
    }
    if known.has_conflict() {
        known.set_all_zero();
    }
    known
}

fn compute_for_mul(lhs: &KnownBits, rhs: &KnownBits, no_undef_self_multiply: bool) -> KnownBits {
    let bit_width = lhs.bit_width();
    if bit_width != rhs.bit_width() {
        return KnownBits::unknown(bit_width);
    }
    let (max_product, overflow) = lhs.max_value().umul_ov(&rhs.max_value());
    let lead_zero = if overflow {
        0
    } else {
        max_product.count_leading_zeros()
    };
    let trail_bits_known_lhs = lhs.zero.bitor(&lhs.one).count_trailing_ones();
    let trail_bits_known_rhs = rhs.zero.bitor(&rhs.one).count_trailing_ones();
    let trail_zero_lhs = lhs.count_min_trailing_zeros();
    let trail_zero_rhs = rhs.count_min_trailing_zeros();
    let trail_zero = trail_zero_lhs.saturating_add(trail_zero_rhs);
    let smallest_operand = trail_bits_known_lhs
        .saturating_sub(trail_zero_lhs)
        .min(trail_bits_known_rhs.saturating_sub(trail_zero_rhs));
    let result_bits_known = smallest_operand.saturating_add(trail_zero).min(bit_width);
    let bottom_lhs = lhs
        .one
        .bitand(&ApInt::low_bits_set(bit_width, trail_bits_known_lhs));
    let bottom_rhs = rhs
        .one
        .bitand(&ApInt::low_bits_set(bit_width, trail_bits_known_rhs));
    let bottom_known = bottom_lhs.wrapping_mul(&bottom_rhs);
    let low_mask = ApInt::low_bits_set(bit_width, result_bits_known);
    let mut result = KnownBits::unknown(bit_width);
    result.zero.set_high_bits(lead_zero);
    result.zero = result
        .zero
        .bitor(&bottom_known.clone().not().bitand(&low_mask));
    result.one = bottom_known.bitand(&low_mask);
    if no_undef_self_multiply {
        let two_tz_plus_one = trail_zero_lhs.saturating_mul(2).saturating_add(1);
        if two_tz_plus_one < bit_width {
            result.zero.set_bit(two_tz_plus_one);
        }
        if trail_zero_lhs < bit_width && lhs.one.is_one_bit_set(trail_zero_lhs) {
            let two_tz_plus_two = two_tz_plus_one.saturating_add(1);
            if two_tz_plus_two < bit_width {
                result.zero.set_bit(two_tz_plus_two);
            }
        }
    }
    result
}

fn avg_compute_u(lhs: &KnownBits, rhs: &KnownBits, is_ceil: bool) -> KnownBits {
    let bit_width = lhs.bit_width();
    if bit_width != rhs.bit_width() {
        return KnownBits::unknown(bit_width);
    }
    let wide_lhs = lhs.zext(bit_width.saturating_add(1));
    let wide_rhs = rhs.zext(bit_width.saturating_add(1));
    compute_for_add_carry_raw(&wide_lhs, &wide_rhs, !is_ceil, is_ceil).extract_bits(bit_width, 1)
}

fn div_compute_low_bit(
    mut known: KnownBits,
    lhs: &KnownBits,
    rhs: &KnownBits,
    exact: bool,
) -> KnownBits {
    if !exact {
        return known;
    }
    if lhs.one.is_one_bit_set(0) {
        known.one.set_bit(0);
    }
    let min_tz =
        i64::from(lhs.count_min_trailing_zeros()) - i64::from(rhs.count_max_trailing_zeros());
    let max_tz =
        i64::from(lhs.count_max_trailing_zeros()) - i64::from(rhs.count_min_trailing_zeros());
    if min_tz >= 0 {
        let min = u32::try_from(min_tz).unwrap_or(lhs.bit_width());
        known.zero.set_low_bits(min);
        if min_tz == max_tz && min < lhs.bit_width() {
            known.one.set_bit(min);
        }
    } else if max_tz < 0 {
        known.set_all_zero();
    }
    if known.has_conflict() {
        known.set_all_zero();
    }
    known
}

fn compute_for_udiv(lhs: &KnownBits, rhs: &KnownBits, exact: bool) -> KnownBits {
    let bit_width = lhs.bit_width();
    let mut known = KnownBits::unknown(bit_width);
    if bit_width != rhs.bit_width() {
        return known;
    }
    if lhs.is_zero() || rhs.is_zero() {
        known.set_all_zero();
        return known;
    }
    let min_denom = rhs.min_value();
    let max_num = lhs.max_value();
    let max_res = if min_denom.is_zero() {
        max_num
    } else {
        max_num
            .checked_udiv(&min_denom)
            .unwrap_or_else(|| ApInt::zero(bit_width))
    };
    known.zero.set_high_bits(max_res.count_leading_zeros());
    div_compute_low_bit(known, lhs, rhs, exact)
}

fn compute_for_sdiv(lhs: &KnownBits, rhs: &KnownBits, exact: bool) -> KnownBits {
    if lhs.is_non_negative() && rhs.is_non_negative() {
        return compute_for_udiv(lhs, rhs, exact);
    }
    let bit_width = lhs.bit_width();
    let mut known = KnownBits::unknown(bit_width);
    if bit_width != rhs.bit_width() {
        return known;
    }
    if lhs.is_zero() || rhs.is_zero() {
        known.set_all_zero();
        return known;
    }
    let result = if lhs.is_negative() && rhs.is_negative() {
        let denom = rhs.signed_max_value();
        let num = lhs.signed_min_value();
        if num.is_min_signed_value() && denom.is_all_ones() {
            Some(ApInt::signed_max_value(bit_width))
        } else {
            num.checked_sdiv(&denom)
        }
    } else if lhs.is_negative() && rhs.is_non_negative() {
        if exact || lhs.signed_max_value().negate().uge(&rhs.signed_max_value()) {
            let denom = rhs.signed_min_value();
            let num = lhs.signed_min_value();
            if denom.is_zero() {
                Some(num)
            } else {
                num.checked_sdiv(&denom)
            }
        } else {
            None
        }
    } else if lhs.is_strictly_positive() && rhs.is_negative() {
        if exact || lhs.signed_min_value().uge(&rhs.signed_min_value().negate()) {
            lhs.signed_max_value().checked_sdiv(&rhs.signed_max_value())
        } else {
            None
        }
    } else {
        None
    };
    if let Some(result) = result {
        if result.is_non_negative() {
            known.zero.set_high_bits(result.count_leading_zeros());
        } else {
            known.one.set_high_bits(result.count_leading_ones());
        }
    }
    div_compute_low_bit(known, lhs, rhs, exact)
}

fn rem_get_low_bits(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
    let bit_width = lhs.bit_width();
    if !rhs.is_zero() && rhs.is_known_zero(0) {
        let rhs_zeros = rhs.count_min_trailing_zeros();
        let mask = ApInt::low_bits_set(bit_width, rhs_zeros);
        return KnownBits {
            zero: lhs.zero.bitand(&mask),
            one: lhs.one.bitand(&mask),
        };
    }
    KnownBits::unknown(bit_width)
}

fn compute_for_urem(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
    let bit_width = lhs.bit_width();
    if bit_width != rhs.bit_width() {
        return KnownBits::unknown(bit_width);
    }
    let mut known = rem_get_low_bits(lhs, rhs);
    if let Some(rhs_const) = rhs.constant()
        && rhs_const.is_power_of_2()
    {
        let high_bits = rhs_const.wrapping_sub(&ap_one(bit_width)).not();
        known.zero = known.zero.bitor(&high_bits);
        return known;
    }
    known.zero.set_high_bits(
        lhs.count_min_leading_zeros()
            .max(rhs.count_min_leading_zeros()),
    );
    known
}

fn compute_for_srem(lhs: &KnownBits, rhs: &KnownBits) -> KnownBits {
    let bit_width = lhs.bit_width();
    if bit_width != rhs.bit_width() {
        return KnownBits::unknown(bit_width);
    }
    let mut known = rem_get_low_bits(lhs, rhs);
    if let Some(rhs_const) = rhs.constant()
        && rhs_const.is_power_of_2()
    {
        let low_bits = rhs_const.wrapping_sub(&ap_one(bit_width));
        let high_bits = low_bits.clone().not();
        if lhs.is_non_negative() || low_bits.is_subset_of(&lhs.zero) {
            known.zero = known.zero.bitor(&high_bits);
        }
        if lhs.is_negative() && low_bits.intersects(&lhs.one) {
            known.one = known.one.bitor(&high_bits);
        }
        return known;
    }
    if lhs.is_negative() && known.is_non_zero() {
        known
            .one
            .set_high_bits(lhs.count_min_leading_ones().max(rhs.count_min_sign_bits()));
    } else if lhs.is_non_negative() {
        known
            .zero
            .set_high_bits(lhs.count_min_leading_zeros().max(rhs.count_min_sign_bits()));
    }
    known
}

fn compute_for_abs(value: &KnownBits, int_min_is_poison: bool) -> KnownBits {
    let bit_width = value.bit_width();
    if value.is_non_negative() {
        return value.clone();
    }
    let mut known_abs = KnownBits::unknown(bit_width);
    if value.is_negative() {
        let mut tmp = value.clone();
        if int_min_is_poison && tmp.zero.popcount().saturating_add(2) == bit_width {
            tmp.one.set_bit(tmp.count_min_trailing_zeros());
        }
        known_abs = compute_for_add_sub_impl(
            false,
            int_min_is_poison,
            false,
            &KnownBits::make_constant(ApInt::zero(bit_width)),
            &tmp,
        );
        if int_min_is_poison
            && tmp.count_min_population() == 1
            && tmp.count_max_population() != 1
            && bit_width != 0
        {
            tmp.one.clear_sign_bit();
            tmp.zero.set_sign_bit();
            set_known_range(
                &mut known_abs.one,
                bit_width,
                bit_width.saturating_sub(tmp.count_min_leading_zeros()),
                bit_width - 1,
            );
        }
    } else {
        let max_tz = value.count_max_trailing_zeros();
        let min_tz = value.count_min_trailing_zeros();
        known_abs.zero.set_low_bits(min_tz);
        if max_tz == min_tz && max_tz < bit_width {
            known_abs.one.set_bit(max_tz);
        }
        if int_min_is_poison || (!value.one.is_zero() && !value.one.is_min_signed_value()) {
            known_abs.one.clear_sign_bit();
            known_abs.zero.set_sign_bit();
        }
    }
    known_abs
}

fn compute_for_reduce_add(value: &KnownBits, num_elts: u32) -> KnownBits {
    let bit_width = value.bit_width();
    let mut result = KnownBits::unknown(bit_width);
    if num_elts == 0 {
        return result;
    }
    if let Some(constant) = value.constant() {
        return KnownBits::make_constant(
            constant.wrapping_mul(&ApInt::from_words(bit_width, &[u64::from(num_elts)])),
        );
    }
    let lost_bits = log2_ceil_u32(num_elts);
    if value.is_non_negative() {
        result
            .zero
            .set_high_bits(value.count_min_leading_zeros().saturating_sub(lost_bits));
    } else if value.is_negative() {
        result
            .one
            .set_high_bits(value.count_min_leading_ones().saturating_sub(lost_bits));
    }
    result
}

fn bit_is_set(value: &ApInt, bit: u32) -> bool {
    value.intersects(&ApInt::one_bit_set(value.bit_width(), bit))
}
