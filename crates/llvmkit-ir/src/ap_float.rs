//! Safe Rust facade for LLVM's `APFloat` semantics.
//!
//! The representation is target-independent bit storage plus semantic helpers
//! for the IR floating kinds currently modeled by llvmkit. Finite binary
//! arithmetic uses `ApInt` significand/exponent math so wide semantics do not
//! narrow through host `f64`; narrower helper-only operations are still ported
//! incrementally.

use crate::ap_int::{ApInt, ApIntSignedness};
use crate::{IrError, IrResult};
use core::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApFloatSemantics {
    IeeeHalf,
    BFloat,
    IeeeSingle,
    IeeeDouble,
    IeeeQuad,
    X87DoubleExtended,
    PpcDoubleDouble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApFloatSign {
    Positive,
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundingMode {
    NearestTiesToEven,
    TowardPositive,
    TowardNegative,
    TowardZero,
    NearestTiesToAway,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApFloatNextDirection {
    TowardPositive,
    TowardNegative,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ApFloatStatus: u8 {
        const OK = 0;
        const INVALID_OP = 0x01;
        const DIV_BY_ZERO = 0x02;
        const OVERFLOW = 0x04;
        const UNDERFLOW = 0x08;
        const INEXACT = 0x10;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApFloatCategory {
    Infinity,
    Nan,
    Normal,
    Zero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApFloatCmpResult {
    LessThan,
    Equal,
    GreaterThan,
    Unordered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LosesInfo {
    No,
    Yes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exactness {
    Inexact,
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NanPayload<'a> {
    Absent,
    Bits(&'a ApInt),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ApFloatRepr {
    Bits(ApInt),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApFloat {
    semantics: ApFloatSemantics,
    repr: ApFloatRepr,
}

impl ApFloatSemantics {
    pub fn bit_width(self) -> u32 {
        match self {
            Self::IeeeHalf | Self::BFloat => 16,
            Self::IeeeSingle => 32,
            Self::IeeeDouble => 64,
            Self::IeeeQuad | Self::PpcDoubleDouble => 128,
            Self::X87DoubleExtended => 80,
        }
    }

    pub fn precision(self) -> u32 {
        match self {
            Self::IeeeHalf => 11,
            Self::BFloat => 8,
            Self::IeeeSingle => 24,
            Self::IeeeDouble => 53,
            Self::IeeeQuad => 113,
            Self::X87DoubleExtended => 64,
            Self::PpcDoubleDouble => 106,
        }
    }
}

impl ApFloat {
    pub fn from_bits(semantics: ApFloatSemantics, bits: &ApInt) -> IrResult<ApFloat> {
        let expected = semantics.bit_width();
        if bits.bit_width() != expected {
            return Err(IrError::OperandWidthMismatch {
                lhs: expected,
                rhs: bits.bit_width(),
            });
        }
        Ok(Self {
            semantics,
            repr: ApFloatRepr::Bits(bits.clone()),
        })
    }

    pub fn to_bits(&self) -> ApInt {
        match &self.repr {
            ApFloatRepr::Bits(bits) => bits.clone(),
        }
    }

    pub fn from_string(
        semantics: ApFloatSemantics,
        text: &str,
        rounding: RoundingMode,
    ) -> IrResult<(ApFloat, ApFloatStatus)> {
        let lower = text.trim().to_ascii_lowercase();
        if lower == "nan" || lower == "+nan" {
            return Ok((
                Self::qnan(semantics, ApFloatSign::Positive, NanPayload::Absent),
                ApFloatStatus::OK,
            ));
        }
        if lower == "-nan" {
            return Ok((
                Self::qnan(semantics, ApFloatSign::Negative, NanPayload::Absent),
                ApFloatStatus::OK,
            ));
        }
        if lower == "inf" || lower == "+inf" || lower == "infinity" || lower == "+infinity" {
            return Ok((
                Self::inf(semantics, ApFloatSign::Positive),
                ApFloatStatus::OK,
            ));
        }
        if lower == "-inf" || lower == "-infinity" {
            return Ok((
                Self::inf(semantics, ApFloatSign::Negative),
                ApFloatStatus::OK,
            ));
        }
        if let Some((value, status)) = decimal_to_semantic_float(semantics, &lower, rounding) {
            return Ok((value, status));
        }
        Err(IrError::InvalidOperation {
            message: "APFloat decimal literal could not be parsed",
        })
    }

    #[inline]
    pub fn semantics(&self) -> ApFloatSemantics {
        self.semantics
    }

    pub fn zero(semantics: ApFloatSemantics, sign: ApFloatSign) -> ApFloat {
        let bits = match semantics {
            ApFloatSemantics::IeeeHalf | ApFloatSemantics::BFloat => {
                if matches!(sign, ApFloatSign::Negative) {
                    0x8000
                } else {
                    0
                }
            }
            ApFloatSemantics::IeeeSingle => u64::from(if matches!(sign, ApFloatSign::Negative) {
                0x8000_0000u32
            } else {
                0
            }),
            ApFloatSemantics::IeeeDouble => {
                if matches!(sign, ApFloatSign::Negative) {
                    0x8000_0000_0000_0000
                } else {
                    0
                }
            }
            ApFloatSemantics::IeeeQuad | ApFloatSemantics::PpcDoubleDouble => 0,
            ApFloatSemantics::X87DoubleExtended => 0,
        };
        let mut words = vec![bits];
        if matches!(sign, ApFloatSign::Negative) && semantics.bit_width() > 64 {
            words = vec![0, 1u64 << ((semantics.bit_width() - 1) - 64)];
        }
        Self::from_bits_unchecked(semantics, &words)
    }

    pub fn one(semantics: ApFloatSemantics, sign: ApFloatSign) -> ApFloat {
        let mut value = match semantics {
            ApFloatSemantics::IeeeHalf => Self::from_bits_unchecked(semantics, &[0x3c00]),
            ApFloatSemantics::BFloat => Self::from_bits_unchecked(semantics, &[0x3f80]),
            ApFloatSemantics::IeeeSingle => Self::from_bits_unchecked(semantics, &[0x3f80_0000]),
            ApFloatSemantics::IeeeDouble => {
                Self::from_bits_unchecked(semantics, &[0x3ff0_0000_0000_0000])
            }
            ApFloatSemantics::IeeeQuad => {
                Self::from_bits_unchecked(semantics, &[0, 0x3fff_0000_0000_0000])
            }
            ApFloatSemantics::X87DoubleExtended => {
                Self::from_bits_unchecked(semantics, &[0x8000_0000_0000_0000, 0x3fff])
            }
            ApFloatSemantics::PpcDoubleDouble => {
                Self::from_bits_unchecked(semantics, &[0, 0x3ff0_0000_0000_0000])
            }
        };
        if matches!(sign, ApFloatSign::Negative) {
            value = value.change_sign();
        }
        value
    }

    pub fn inf(semantics: ApFloatSemantics, sign: ApFloatSign) -> ApFloat {
        let mut value = match semantics {
            ApFloatSemantics::IeeeHalf => Self::from_bits_unchecked(semantics, &[0x7c00]),
            ApFloatSemantics::BFloat => Self::from_bits_unchecked(semantics, &[0x7f80]),
            ApFloatSemantics::IeeeSingle => Self::from_bits_unchecked(semantics, &[0x7f80_0000]),
            ApFloatSemantics::IeeeDouble => {
                Self::from_bits_unchecked(semantics, &[0x7ff0_0000_0000_0000])
            }
            ApFloatSemantics::IeeeQuad => {
                Self::from_bits_unchecked(semantics, &[0, 0x7fff_0000_0000_0000])
            }
            ApFloatSemantics::X87DoubleExtended => {
                Self::from_bits_unchecked(semantics, &[0x8000_0000_0000_0000, 0x7fff])
            }
            ApFloatSemantics::PpcDoubleDouble => {
                Self::from_bits_unchecked(semantics, &[0, 0x7ff0_0000_0000_0000])
            }
        };
        if matches!(sign, ApFloatSign::Negative) {
            value = value.change_sign();
        }
        value
    }

    pub fn qnan(
        semantics: ApFloatSemantics,
        sign: ApFloatSign,
        payload: NanPayload<'_>,
    ) -> ApFloat {
        Self::nan(semantics, sign, payload, false)
    }

    pub fn snan(
        semantics: ApFloatSemantics,
        sign: ApFloatSign,
        payload: NanPayload<'_>,
    ) -> ApFloat {
        Self::nan(semantics, sign, payload, true)
    }

    pub fn largest(semantics: ApFloatSemantics, sign: ApFloatSign) -> ApFloat {
        let mut value = match semantics {
            ApFloatSemantics::IeeeHalf => Self::from_bits_unchecked(semantics, &[0x7bff]),
            ApFloatSemantics::BFloat => Self::from_bits_unchecked(semantics, &[0x7f7f]),
            ApFloatSemantics::IeeeSingle => Self::from_bits_unchecked(semantics, &[0x7f7f_ffff]),
            ApFloatSemantics::IeeeDouble => {
                Self::from_bits_unchecked(semantics, &[0x7fef_ffff_ffff_ffff])
            }
            ApFloatSemantics::IeeeQuad => {
                Self::from_bits_unchecked(semantics, &[u64::MAX, 0x7ffe_ffff_ffff_ffff])
            }
            ApFloatSemantics::X87DoubleExtended => {
                Self::from_bits_unchecked(semantics, &[u64::MAX, 0x7ffe])
            }
            ApFloatSemantics::PpcDoubleDouble => Self::from_bits_unchecked(
                semantics,
                &[0x7c8f_ffff_ffff_fffe, 0x7fef_ffff_ffff_ffff],
            ),
        };
        if matches!(sign, ApFloatSign::Negative) {
            value = value.change_sign();
        }
        value
    }

    pub fn smallest(semantics: ApFloatSemantics, sign: ApFloatSign) -> ApFloat {
        let mut value = match semantics {
            ApFloatSemantics::PpcDoubleDouble => Self::from_bits_unchecked(semantics, &[0, 1]),
            _ => Self::from_bits_unchecked(semantics, &[1]),
        };
        if matches!(sign, ApFloatSign::Negative) {
            value = value.change_sign();
        }
        value
    }

    pub fn smallest_normalized(semantics: ApFloatSemantics, sign: ApFloatSign) -> ApFloat {
        let mut value = match semantics {
            ApFloatSemantics::IeeeHalf => Self::from_bits_unchecked(semantics, &[0x0400]),
            ApFloatSemantics::BFloat => Self::from_bits_unchecked(semantics, &[0x0080]),
            ApFloatSemantics::IeeeSingle => Self::from_bits_unchecked(semantics, &[0x0080_0000]),
            ApFloatSemantics::IeeeDouble => {
                Self::from_bits_unchecked(semantics, &[0x0010_0000_0000_0000])
            }
            ApFloatSemantics::IeeeQuad => {
                Self::from_bits_unchecked(semantics, &[0, 0x0001_0000_0000_0000])
            }
            ApFloatSemantics::X87DoubleExtended => {
                Self::from_bits_unchecked(semantics, &[0x8000_0000_0000_0000, 1])
            }
            ApFloatSemantics::PpcDoubleDouble => {
                Self::from_bits_unchecked(semantics, &[0, 0x0360_0000_0000_0000])
            }
        };
        if matches!(sign, ApFloatSign::Negative) {
            value = value.change_sign();
        }
        value
    }

    pub fn add(&self, rhs: &ApFloat, rounding: RoundingMode) -> (ApFloat, ApFloatStatus) {
        self.binary_arithmetic(rhs, rounding, FiniteBinaryOp::Add)
    }

    pub fn subtract(&self, rhs: &ApFloat, rounding: RoundingMode) -> (ApFloat, ApFloatStatus) {
        self.binary_arithmetic(rhs, rounding, FiniteBinaryOp::Subtract)
    }

    pub fn multiply(&self, rhs: &ApFloat, rounding: RoundingMode) -> (ApFloat, ApFloatStatus) {
        self.binary_arithmetic(rhs, rounding, FiniteBinaryOp::Multiply)
    }

    pub fn divide(&self, rhs: &ApFloat, rounding: RoundingMode) -> (ApFloat, ApFloatStatus) {
        self.binary_arithmetic(rhs, rounding, FiniteBinaryOp::Divide)
    }

    pub fn remainder(&self, rhs: &ApFloat) -> (ApFloat, ApFloatStatus) {
        self.binary_arithmetic(
            rhs,
            RoundingMode::NearestTiesToEven,
            FiniteBinaryOp::Remainder,
        )
    }

    pub fn modulo(&self, rhs: &ApFloat) -> (ApFloat, ApFloatStatus) {
        self.binary_arithmetic(rhs, RoundingMode::NearestTiesToEven, FiniteBinaryOp::Modulo)
    }

    pub fn fused_multiply_add(
        &self,
        multiplicand: &ApFloat,
        addend: &ApFloat,
        rounding: RoundingMode,
    ) -> (ApFloat, ApFloatStatus) {
        if self.semantics != multiplicand.semantics || self.semantics != addend.semantics {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        }
        if let Some(result) = special_fma_result(self, multiplicand, addend, rounding) {
            return result;
        }
        let Some(lhs) = self.binary_components() else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        let Some(rhs) = multiplicand.binary_components() else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        let Some(add) = addend.binary_components() else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        finite_fused_multiply_add(self.semantics, &lhs, &rhs, &add, rounding)
            .unwrap_or_else(|| (self.clone(), ApFloatStatus::INVALID_OP))
    }

    pub fn round_to_integral(&self, rounding: RoundingMode) -> (ApFloat, ApFloatStatus) {
        if self.is_nan() {
            let status = if self.is_signaling() {
                ApFloatStatus::INVALID_OP
            } else {
                ApFloatStatus::OK
            };
            return (self.make_quiet(), status);
        }
        if self.is_infinity() || self.is_zero() {
            return (self.clone(), ApFloatStatus::OK);
        }
        let Some(components) = self.binary_components() else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        let Some(scale) = component_scale(&components) else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        if scale >= 0 {
            if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
                return encode_binary_scaled(
                    self.semantics,
                    components.negative,
                    components.magnitude.clone(),
                    scale,
                    rounding,
                )
                .unwrap_or_else(|| (self.clone(), ApFloatStatus::INVALID_OP));
            }
            return (self.clone(), ApFloatStatus::OK);
        }
        let (magnitude, exact) = round_power_of_two_div(
            &components.magnitude,
            scale.unsigned_abs(),
            rounding,
            components.negative,
        );
        let Some((rounded, mut status)) =
            encode_binary_scaled(self.semantics, components.negative, magnitude, 0, rounding)
        else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        if matches!(exact, Exactness::Inexact) {
            status.insert(ApFloatStatus::INEXACT);
        }
        (rounded, status)
    }

    pub fn next(&self, direction: ApFloatNextDirection) -> (ApFloat, ApFloatStatus) {
        if self.is_nan() {
            let status = if self.is_signaling() {
                ApFloatStatus::INVALID_OP
            } else {
                ApFloatStatus::OK
            };
            return (self.make_quiet(), status);
        }
        if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            return ppc_next(self, direction);
        }
        if self.is_zero() {
            let sign = match direction {
                ApFloatNextDirection::TowardPositive => ApFloatSign::Positive,
                ApFloatNextDirection::TowardNegative => ApFloatSign::Negative,
            };
            return (Self::smallest(self.semantics, sign), ApFloatStatus::OK);
        }
        if self.is_pos_infinity() {
            return match direction {
                ApFloatNextDirection::TowardPositive => (self.clone(), ApFloatStatus::OK),
                ApFloatNextDirection::TowardNegative => (
                    Self::largest(self.semantics, ApFloatSign::Positive),
                    ApFloatStatus::OK,
                ),
            };
        }
        if self.is_neg_infinity() {
            return match direction {
                ApFloatNextDirection::TowardPositive => (
                    Self::largest(self.semantics, ApFloatSign::Negative),
                    ApFloatStatus::OK,
                ),
                ApFloatNextDirection::TowardNegative => (self.clone(), ApFloatStatus::OK),
            };
        }

        let bits = self.to_bits();
        let step = ApInt::from_words(bits.bit_width(), &[1]);
        let increment = match direction {
            ApFloatNextDirection::TowardPositive => !self.is_negative(),
            ApFloatNextDirection::TowardNegative => self.is_negative(),
        };
        let next_bits = if increment {
            bits.wrapping_add(&step)
        } else {
            bits.wrapping_sub(&step)
        };
        match Self::from_bits(self.semantics, &next_bits) {
            Ok(v) => (v, ApFloatStatus::OK),
            Err(_) => (self.clone(), ApFloatStatus::INVALID_OP),
        }
    }

    pub fn change_sign(&self) -> ApFloat {
        let sign = if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            ApInt::from_words(128, &[0x8000_0000_0000_0000, 0x8000_0000_0000_0000])
        } else {
            ApInt::one_bit_set(self.semantics.bit_width(), self.semantics.bit_width() - 1)
        };
        Self {
            semantics: self.semantics,
            repr: ApFloatRepr::Bits(ApInt::bitxor(&self.to_bits(), &sign)),
        }
    }

    pub fn clear_sign(&self) -> ApFloat {
        if self.is_negative() {
            self.change_sign()
        } else {
            self.clone()
        }
    }

    pub fn copy_sign(&self, sign_source: &ApFloat) -> ApFloat {
        let cleared = self.clear_sign();
        if sign_source.is_negative() {
            cleared.change_sign()
        } else {
            cleared
        }
    }

    pub fn make_quiet(&self) -> ApFloat {
        if !self.is_nan() || !self.is_signaling() {
            return self.clone();
        }
        Self::qnan(
            self.semantics,
            if self.is_negative() {
                ApFloatSign::Negative
            } else {
                ApFloatSign::Positive
            },
            NanPayload::Absent,
        )
    }

    fn convert_quiet_nan_to(&self, semantics: ApFloatSemantics) -> ApFloat {
        if self.is_signaling() {
            Self::snan(
                semantics,
                sign_from_negative(self.is_negative()),
                NanPayload::Absent,
            )
        } else {
            Self::qnan(
                semantics,
                sign_from_negative(self.is_negative()),
                NanPayload::Absent,
            )
        }
    }

    pub fn convert(
        &self,
        to_semantics: ApFloatSemantics,
        rounding: RoundingMode,
    ) -> (ApFloat, ApFloatStatus, LosesInfo) {
        let (value, status) = if self.semantics == to_semantics {
            (self.clone(), ApFloatStatus::OK)
        } else if self.is_zero() {
            (
                Self::zero(to_semantics, sign_from_negative(self.is_negative())),
                ApFloatStatus::OK,
            )
        } else if self.is_infinity() {
            (
                Self::inf(to_semantics, sign_from_negative(self.is_negative())),
                ApFloatStatus::OK,
            )
        } else if self.is_nan() {
            let mut status = ApFloatStatus::OK;
            if self.is_signaling() {
                status.insert(ApFloatStatus::INVALID_OP);
            }
            (self.make_quiet().convert_quiet_nan_to(to_semantics), status)
        } else if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            convert_ppc_double_double(self, to_semantics, rounding)
                .unwrap_or_else(|| (self.clone(), ApFloatStatus::INVALID_OP))
        } else {
            let Some(components) = self.binary_components() else {
                return (self.clone(), ApFloatStatus::INVALID_OP, LosesInfo::Yes);
            };
            let Some(scale) = component_scale(&components) else {
                return (self.clone(), ApFloatStatus::INVALID_OP, LosesInfo::Yes);
            };
            encode_binary_scaled(
                to_semantics,
                components.negative,
                components.magnitude,
                scale,
                rounding,
            )
            .unwrap_or_else(|| (self.clone(), ApFloatStatus::INVALID_OP))
        };
        let loses = if status
            .intersects(ApFloatStatus::INEXACT | ApFloatStatus::OVERFLOW | ApFloatStatus::UNDERFLOW)
        {
            LosesInfo::Yes
        } else {
            LosesInfo::No
        };
        (value, status, loses)
    }

    pub fn convert_to_integer(
        &self,
        width: u32,
        signedness: ApIntSignedness,
        rounding: RoundingMode,
    ) -> (ApInt, ApFloatStatus, Exactness) {
        if self.is_nan() || self.is_infinity() {
            return (
                ApInt::zero(width),
                ApFloatStatus::INVALID_OP,
                Exactness::Inexact,
            );
        }
        if let Some(result) = self.convert_ppc_double_double_to_integer(width, signedness, rounding)
        {
            return result;
        }
        if let Some(result) = self.convert_binary_float_to_integer(width, signedness, rounding) {
            return result;
        }
        (
            ApInt::zero(width),
            ApFloatStatus::INVALID_OP,
            Exactness::Inexact,
        )
    }

    fn convert_binary_float_to_integer(
        &self,
        width: u32,
        signedness: ApIntSignedness,
        rounding: RoundingMode,
    ) -> Option<(ApInt, ApFloatStatus, Exactness)> {
        let components = self.binary_components()?;
        if components.magnitude.is_zero() {
            return Some((ApInt::zero(width), ApFloatStatus::OK, Exactness::Exact));
        }
        let shift = components
            .exponent
            .checked_sub(i32::try_from(components.precision.checked_sub(1)?).ok()?)?;
        let work_width = components
            .magnitude
            .active_bits()
            .max(width)
            .checked_add(shift.unsigned_abs())?
            .checked_add(2)?;
        let magnitude = ApInt::from_words(work_width, components.magnitude.words());
        let (mut rounded_magnitude, exact) = if shift >= 0 {
            (
                apint_shl(&magnitude, u32::try_from(shift).ok()?),
                Exactness::Exact,
            )
        } else {
            round_power_of_two_div(
                &magnitude,
                shift.unsigned_abs(),
                rounding,
                components.negative,
            )
        };
        if rounded_magnitude.is_zero() {
            return Some((
                ApInt::zero(width),
                if matches!(exact, Exactness::Exact) {
                    ApFloatStatus::OK
                } else {
                    ApFloatStatus::INEXACT
                },
                exact,
            ));
        }
        if integer_magnitude_out_of_range(
            &rounded_magnitude,
            width,
            signedness,
            components.negative,
        ) {
            return Some((
                ApInt::zero(width),
                ApFloatStatus::INVALID_OP,
                Exactness::Inexact,
            ));
        }
        rounded_magnitude = ApInt::from_words(width, rounded_magnitude.words());
        let int = if matches!(signedness, ApIntSignedness::Signed) && components.negative {
            rounded_magnitude.negate()
        } else {
            rounded_magnitude
        };
        Some((
            int,
            if matches!(exact, Exactness::Exact) {
                ApFloatStatus::OK
            } else {
                ApFloatStatus::INEXACT
            },
            exact,
        ))
    }

    fn convert_ppc_double_double_to_integer(
        &self,
        width: u32,
        signedness: ApIntSignedness,
        rounding: RoundingMode,
    ) -> Option<(ApInt, ApFloatStatus, Exactness)> {
        if !matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            return None;
        }
        let bits = self.to_bits();
        let words = bits.words();
        let low = ppc_double_component(words.first().copied().unwrap_or(0))?;
        let high = ppc_double_component(words.get(1).copied().unwrap_or(0))?;
        let mut min_scale: Option<i32> = None;
        for component in [high.as_ref(), low.as_ref()].into_iter().flatten() {
            let scale = component_scale(component)?;
            min_scale = Some(min_scale.map_or(scale, |current| current.min(scale)));
        }
        let Some(min_scale) = min_scale else {
            return Some((ApInt::zero(width), ApFloatStatus::OK, Exactness::Exact));
        };

        let mut work_width = width.max(1).checked_add(2)?;
        for component in [high.as_ref(), low.as_ref()].into_iter().flatten() {
            let scale = component_scale(component)?;
            let shift = u32::try_from(scale.checked_sub(min_scale)?).ok()?;
            let term_width = component
                .magnitude
                .active_bits()
                .checked_add(shift)?
                .checked_add(2)?;
            work_width = work_width.max(term_width);
        }

        let mut positive = ApInt::zero(work_width);
        let mut negative = ApInt::zero(work_width);
        for component in [high.as_ref(), low.as_ref()].into_iter().flatten() {
            let scale = component_scale(component)?;
            let shift = u32::try_from(scale.checked_sub(min_scale)?).ok()?;
            let magnitude = component.magnitude.zext_or_trunc(work_width);
            let term = if shift == 0 {
                magnitude
            } else {
                magnitude.checked_shl(shift)?
            };
            if component.negative {
                negative = negative.wrapping_add(&term);
            } else {
                positive = positive.wrapping_add(&term);
            }
        }

        let (result_negative, magnitude) = if positive.uge(&negative) {
            (false, positive.wrapping_sub(&negative))
        } else {
            (true, negative.wrapping_sub(&positive))
        };
        if magnitude.is_zero() {
            return Some((ApInt::zero(width), ApFloatStatus::OK, Exactness::Exact));
        }
        let (mut rounded_magnitude, exact) = if min_scale >= 0 {
            let shift = u32::try_from(min_scale).ok()?;
            let shifted_width = magnitude
                .active_bits()
                .max(width)
                .checked_add(shift)?
                .checked_add(2)?;
            let magnitude = magnitude.zext_or_trunc(shifted_width);
            let shifted = if shift == 0 {
                magnitude
            } else {
                magnitude.checked_shl(shift)?
            };
            (shifted, Exactness::Exact)
        } else {
            round_power_of_two_div(
                &magnitude,
                min_scale.unsigned_abs(),
                rounding,
                result_negative,
            )
        };
        if rounded_magnitude.is_zero() {
            return Some((
                ApInt::zero(width),
                if matches!(exact, Exactness::Exact) {
                    ApFloatStatus::OK
                } else {
                    ApFloatStatus::INEXACT
                },
                exact,
            ));
        }
        if integer_magnitude_out_of_range(&rounded_magnitude, width, signedness, result_negative) {
            return Some((
                ApInt::zero(width),
                ApFloatStatus::INVALID_OP,
                Exactness::Inexact,
            ));
        }
        rounded_magnitude = ApInt::from_words(width, rounded_magnitude.words());
        let int = if matches!(signedness, ApIntSignedness::Signed) && result_negative {
            rounded_magnitude.negate()
        } else {
            rounded_magnitude
        };
        Some((
            int,
            if matches!(exact, Exactness::Exact) {
                ApFloatStatus::OK
            } else {
                ApFloatStatus::INEXACT
            },
            exact,
        ))
    }

    fn binary_components(&self) -> Option<BinaryFloatComponents> {
        if self.is_zero() || self.is_nan() || self.is_infinity() {
            return None;
        }
        let bits = self.to_bits();
        let negative = self.is_negative();
        match self.semantics {
            ApFloatSemantics::IeeeHalf
            | ApFloatSemantics::BFloat
            | ApFloatSemantics::IeeeSingle
            | ApFloatSemantics::IeeeDouble
            | ApFloatSemantics::IeeeQuad => {
                let (exp_lo, exp_bits) = exponent_layout(self.semantics);
                let raw_exp = extract_bits_u64(&bits, exp_lo, exp_bits);
                let precision = self.semantics.precision();
                let fraction_bits = precision.checked_sub(1)?;
                let fraction = low_u128_from_apint(&bits) & low_bits_mask(fraction_bits);
                if raw_exp == 0 && fraction == 0 {
                    return None;
                }
                let significand = if raw_exp == 0 {
                    fraction
                } else {
                    1u128.checked_shl(fraction_bits)? | fraction
                };
                let exponent = if raw_exp == 0 {
                    1i32.checked_sub(exponent_bias(self.semantics)?)?
                } else {
                    i32::try_from(raw_exp).ok()? - exponent_bias(self.semantics)?
                };
                Some(BinaryFloatComponents {
                    negative,
                    exponent,
                    precision,
                    magnitude: apint_from_u128_with_width(precision, significand),
                })
            }
            ApFloatSemantics::X87DoubleExtended => {
                let words = bits.words();
                let raw_exp = words.get(1).copied().unwrap_or(0) & 0x7fff;
                let magnitude = ApInt::from_words(64, &[words.first().copied().unwrap_or(0)]);
                if raw_exp == 0 && magnitude.is_zero() {
                    return None;
                }
                let exponent = if raw_exp == 0 {
                    1 - 16383
                } else {
                    i32::try_from(raw_exp).ok()? - 16383
                };
                Some(BinaryFloatComponents {
                    negative,
                    exponent,
                    precision: 64,
                    magnitude,
                })
            }
            ApFloatSemantics::PpcDoubleDouble => ppc_double_double_components(self),
        }
    }

    pub fn convert_from_ap_int(
        semantics: ApFloatSemantics,
        input: &ApInt,
        signedness: ApIntSignedness,
        rounding: RoundingMode,
    ) -> (ApFloat, ApFloatStatus) {
        let negative = matches!(signedness, ApIntSignedness::Signed) && input.is_negative();
        let magnitude = if negative {
            input.negate()
        } else {
            input.clone()
        };
        if magnitude.is_zero() {
            return (
                ApFloat::zero(semantics, sign_from_negative(negative)),
                ApFloatStatus::OK,
            );
        }
        encode_binary_scaled(semantics, negative, magnitude, 0, rounding).unwrap_or_else(|| {
            (
                ApFloat::zero(semantics, ApFloatSign::Positive),
                ApFloatStatus::INVALID_OP,
            )
        })
    }

    pub fn compare(&self, rhs: &ApFloat) -> ApFloatCmpResult {
        if self.is_nan() || rhs.is_nan() || self.semantics != rhs.semantics {
            return ApFloatCmpResult::Unordered;
        }
        if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            return match compare_ppc_double_double(self, rhs) {
                Some(Ordering::Less) => ApFloatCmpResult::LessThan,
                Some(Ordering::Equal) => ApFloatCmpResult::Equal,
                Some(Ordering::Greater) => ApFloatCmpResult::GreaterThan,
                None => ApFloatCmpResult::Unordered,
            };
        }
        if self.is_zero() && rhs.is_zero() {
            return ApFloatCmpResult::Equal;
        }
        let lhs_negative = self.is_negative();
        let rhs_negative = rhs.is_negative();
        if lhs_negative != rhs_negative {
            return if lhs_negative {
                ApFloatCmpResult::LessThan
            } else {
                ApFloatCmpResult::GreaterThan
            };
        }
        let Some(ordering) = float_abs_cmp(self, rhs) else {
            return ApFloatCmpResult::Unordered;
        };
        let ordering = if lhs_negative {
            ordering.reverse()
        } else {
            ordering
        };
        match ordering {
            Ordering::Less => ApFloatCmpResult::LessThan,
            Ordering::Equal => ApFloatCmpResult::Equal,
            Ordering::Greater => ApFloatCmpResult::GreaterThan,
        }
    }

    #[inline]
    pub fn bitwise_is_equal(&self, rhs: &ApFloat) -> bool {
        self.semantics == rhs.semantics && self.to_bits().eq_ap_int(&rhs.to_bits())
    }

    #[inline]
    pub fn is_exactly_value_f64(&self, value: f64) -> bool {
        self.to_f64_value() == value
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.category() == ApFloatCategory::Zero
    }

    #[inline]
    pub fn is_infinity(&self) -> bool {
        self.category() == ApFloatCategory::Infinity
    }

    #[inline]
    pub fn is_nan(&self) -> bool {
        self.category() == ApFloatCategory::Nan
    }

    #[inline]
    pub fn is_negative(&self) -> bool {
        self.to_bits()
            .is_one_bit_set(self.semantics.bit_width() - 1)
    }

    pub fn is_denormal(&self) -> bool {
        if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            return ppc_is_denormal(self);
        }
        let bits = self.to_bits();
        let (exp_lo, exp_bits) = exponent_layout(self.semantics);
        if exp_bits == 0 {
            return false;
        }
        let exp = extract_bits_u64(&bits, exp_lo, exp_bits);
        exp == 0 && !self.is_zero()
    }

    pub fn is_signaling(&self) -> bool {
        if !self.is_nan() {
            return false;
        }
        let quiet_bit = quiet_nan_bit(self.semantics);
        quiet_bit != 0 && !self.to_bits().is_one_bit_set(quiet_bit)
    }

    #[inline]
    pub fn is_normal(&self) -> bool {
        self.category() == ApFloatCategory::Normal && !self.is_denormal()
    }

    #[inline]
    pub fn is_finite(&self) -> bool {
        matches!(
            self.category(),
            ApFloatCategory::Normal | ApFloatCategory::Zero
        )
    }

    pub fn category(&self) -> ApFloatCategory {
        if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            return ppc_high_float(self).category();
        }
        let bits = self.to_bits();
        let (exp_lo, exp_bits) = exponent_layout(self.semantics);
        let frac_bits = fraction_bits(self.semantics);
        if exp_bits == 0 {
            return if bits.is_zero() {
                ApFloatCategory::Zero
            } else {
                ApFloatCategory::Normal
            };
        }
        let exp = extract_bits_u64(&bits, exp_lo, exp_bits);
        let exp_all_ones = if exp_bits >= 64 {
            u64::MAX
        } else {
            (1u64 << exp_bits) - 1
        };
        let frac_nonzero = has_fraction_bits(&bits, frac_bits);
        if exp == 0 {
            if frac_nonzero {
                ApFloatCategory::Normal
            } else {
                ApFloatCategory::Zero
            }
        } else if exp == exp_all_ones {
            if frac_nonzero {
                ApFloatCategory::Nan
            } else {
                ApFloatCategory::Infinity
            }
        } else {
            ApFloatCategory::Normal
        }
    }

    #[inline]
    pub fn is_non_zero(&self) -> bool {
        !self.is_zero()
    }

    #[inline]
    pub fn is_finite_non_zero(&self) -> bool {
        self.is_finite() && !self.is_zero()
    }

    #[inline]
    pub fn is_pos_zero(&self) -> bool {
        self.is_zero() && !self.is_negative()
    }

    #[inline]
    pub fn is_neg_zero(&self) -> bool {
        self.is_zero() && self.is_negative()
    }

    #[inline]
    pub fn is_pos_infinity(&self) -> bool {
        self.is_infinity() && !self.is_negative()
    }

    #[inline]
    pub fn is_neg_infinity(&self) -> bool {
        self.is_infinity() && self.is_negative()
    }

    #[inline]
    pub fn is_smallest(&self) -> bool {
        self.bitwise_is_equal(&Self::smallest(
            self.semantics,
            if self.is_negative() {
                ApFloatSign::Negative
            } else {
                ApFloatSign::Positive
            },
        ))
    }

    #[inline]
    pub fn is_largest(&self) -> bool {
        self.bitwise_is_equal(&Self::largest(
            self.semantics,
            if self.is_negative() {
                ApFloatSign::Negative
            } else {
                ApFloatSign::Positive
            },
        ))
    }

    #[inline]
    pub fn is_integer(&self) -> bool {
        if self.is_zero() {
            return true;
        }
        if !self.is_finite() {
            return false;
        }
        if matches!(self.semantics, ApFloatSemantics::PpcDoubleDouble) {
            let Some((_, status, exact)) = self.convert_ppc_double_double_to_integer(
                2048,
                ApIntSignedness::Signed,
                RoundingMode::TowardZero,
            ) else {
                return false;
            };
            return !status.contains(ApFloatStatus::INVALID_OP)
                && matches!(exact, Exactness::Exact);
        }
        self.binary_components()
            .and_then(|components| binary_component_is_integer(&components))
            .unwrap_or(false)
    }

    pub fn exact_inverse(&self) -> Option<ApFloat> {
        if self.is_zero() || self.is_nan() {
            return None;
        }
        if self.is_infinity() {
            return Some(Self::zero(
                self.semantics,
                sign_from_negative(self.is_negative()),
            ));
        }
        let exponent = self.exact_log2_abs()?;
        let scale = exponent.checked_neg()?;
        let (inverse, status) = encode_binary_scaled(
            self.semantics,
            self.is_negative(),
            ApInt::from_words(1, &[1]),
            scale,
            RoundingMode::NearestTiesToEven,
        )?;
        if status == ApFloatStatus::OK {
            Some(inverse)
        } else {
            None
        }
    }

    pub fn exact_log2_abs(&self) -> Option<i32> {
        if !self.is_finite_non_zero() {
            return None;
        }
        let components = self.binary_components()?;
        if !components.magnitude.is_power_of_2() {
            return None;
        }
        let scale = component_scale(&components)?;
        let bit = i32::try_from(components.magnitude.count_trailing_zeros()).ok()?;
        scale.checked_add(bit)
    }

    pub fn exact_log2(&self) -> Option<i32> {
        if self.is_negative() {
            None
        } else {
            self.exact_log2_abs()
        }
    }

    pub fn ilogb(&self) -> i32 {
        self.exact_log2_abs().unwrap_or(0)
    }

    pub fn scalbn(&self, exponent: i32, rounding: RoundingMode) -> (ApFloat, ApFloatStatus) {
        if self.is_zero() || self.is_nan() || self.is_infinity() {
            return (self.clone(), ApFloatStatus::OK);
        }
        let Some(components) = self.binary_components() else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        let Some(scale) =
            component_scale(&components).and_then(|scale| scale.checked_add(exponent))
        else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        encode_binary_scaled(
            self.semantics,
            components.negative,
            components.magnitude,
            scale,
            rounding,
        )
        .unwrap_or_else(|| (self.clone(), ApFloatStatus::INVALID_OP))
    }

    pub fn frexp(&self, rounding: RoundingMode) -> (ApFloat, i32, ApFloatStatus) {
        if self.is_zero() {
            return (self.clone(), 0, ApFloatStatus::OK);
        }
        if self.is_nan() || self.is_infinity() {
            return (self.clone(), 0, ApFloatStatus::OK);
        }
        let Some(components) = self.binary_components() else {
            return (self.clone(), 0, ApFloatStatus::INVALID_OP);
        };
        let Some(scale) = component_scale(&components) else {
            return (self.clone(), 0, ApFloatStatus::INVALID_OP);
        };
        let Some(active_bits) = i32::try_from(components.magnitude.active_bits()).ok() else {
            return (self.clone(), 0, ApFloatStatus::INVALID_OP);
        };
        let Some(exp_i32) = scale.checked_add(active_bits) else {
            return (self.clone(), 0, ApFloatStatus::INVALID_OP);
        };
        let Some(mantissa_scale) = scale.checked_sub(exp_i32) else {
            return (self.clone(), 0, ApFloatStatus::INVALID_OP);
        };
        let (mantissa, status) = encode_binary_scaled(
            self.semantics,
            components.negative,
            components.magnitude,
            mantissa_scale,
            rounding,
        )
        .unwrap_or_else(|| (self.clone(), ApFloatStatus::INVALID_OP));
        (mantissa, exp_i32, status)
    }

    #[inline]
    pub fn abs(&self) -> ApFloat {
        self.clear_sign()
    }

    #[inline]
    pub fn neg(&self) -> ApFloat {
        self.change_sign()
    }

    fn nan(
        semantics: ApFloatSemantics,
        sign: ApFloatSign,
        payload: NanPayload<'_>,
        signaling: bool,
    ) -> ApFloat {
        let payload_bits = match payload {
            NanPayload::Absent => ApInt::zero(semantics.bit_width()),
            NanPayload::Bits(bits) => bits.zext_or_trunc(semantics.bit_width()),
        };
        let mut value = match semantics {
            ApFloatSemantics::IeeeHalf => {
                Self::from_bits_unchecked(semantics, &[if signaling { 0x7d00 } else { 0x7e00 }])
            }
            ApFloatSemantics::BFloat => {
                Self::from_bits_unchecked(semantics, &[if signaling { 0x7fa0 } else { 0x7fc0 }])
            }
            ApFloatSemantics::IeeeSingle => Self::from_bits_unchecked(
                semantics,
                &[if signaling { 0x7fa0_0000 } else { 0x7fc0_0000 }],
            ),
            ApFloatSemantics::IeeeDouble => Self::from_bits_unchecked(
                semantics,
                &[if signaling {
                    0x7ff4_0000_0000_0000
                } else {
                    0x7ff8_0000_0000_0000
                }],
            ),
            ApFloatSemantics::IeeeQuad => Self::from_bits_unchecked(
                semantics,
                &[
                    0,
                    if signaling {
                        0x7fff_4000_0000_0000
                    } else {
                        0x7fff_8000_0000_0000
                    },
                ],
            ),
            ApFloatSemantics::X87DoubleExtended => Self::from_bits_unchecked(
                semantics,
                &[
                    if signaling {
                        0xa000_0000_0000_0000
                    } else {
                        0xc000_0000_0000_0000
                    },
                    0x7fff,
                ],
            ),
            ApFloatSemantics::PpcDoubleDouble => Self::from_bits_unchecked(
                semantics,
                &[
                    0,
                    if signaling {
                        0x7ff4_0000_0000_0000
                    } else {
                        0x7ff8_0000_0000_0000
                    },
                ],
            ),
        };
        value = Self {
            semantics,
            repr: ApFloatRepr::Bits(ApInt::bitor(&value.to_bits(), &payload_bits)),
        };
        if matches!(sign, ApFloatSign::Negative) {
            value = value.change_sign();
        }
        value
    }

    fn from_bits_unchecked(semantics: ApFloatSemantics, words: &[u64]) -> ApFloat {
        Self {
            semantics,
            repr: ApFloatRepr::Bits(ApInt::from_words(semantics.bit_width(), words)),
        }
    }

    fn binary_arithmetic(
        &self,
        rhs: &ApFloat,
        rounding: RoundingMode,
        op: FiniteBinaryOp,
    ) -> (ApFloat, ApFloatStatus) {
        if self.semantics != rhs.semantics {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        }
        if let Some(result) = special_binary_result(self, rhs, rounding, op) {
            return result;
        }
        let Some(lhs_components) = self.binary_components() else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        let Some(rhs_components) = rhs.binary_components() else {
            return (self.clone(), ApFloatStatus::INVALID_OP);
        };
        let folded = match op {
            FiniteBinaryOp::Add => finite_add_or_subtract(
                self.semantics,
                &lhs_components,
                &rhs_components,
                rounding,
                MagnitudeOp::Add,
            ),
            FiniteBinaryOp::Subtract => finite_add_or_subtract(
                self.semantics,
                &lhs_components,
                &rhs_components,
                rounding,
                MagnitudeOp::Subtract,
            ),
            FiniteBinaryOp::Multiply => {
                finite_multiply(self.semantics, &lhs_components, &rhs_components, rounding)
            }
            FiniteBinaryOp::Divide => {
                finite_divide(self.semantics, &lhs_components, &rhs_components, rounding)
            }
            FiniteBinaryOp::Remainder => {
                finite_remainder(self.semantics, &lhs_components, &rhs_components)
            }
            FiniteBinaryOp::Modulo => {
                finite_modulo(self.semantics, &lhs_components, &rhs_components)
            }
        };
        match folded {
            Some(result) => result,
            None => (self.clone(), ApFloatStatus::INVALID_OP),
        }
    }

    fn to_f64_value(&self) -> f64 {
        match self.semantics {
            ApFloatSemantics::IeeeSingle => {
                let bits = self.to_bits().try_zext_u64().unwrap_or(0);
                let raw = u32::try_from(bits).unwrap_or(0);
                f64::from(f32::from_bits(raw))
            }
            ApFloatSemantics::IeeeDouble => {
                f64::from_bits(self.to_bits().try_zext_u64().unwrap_or(0))
            }
            ApFloatSemantics::IeeeHalf => {
                half_to_f64(u16::try_from(self.to_bits().try_zext_u64().unwrap_or(0)).unwrap_or(0))
            }
            ApFloatSemantics::BFloat => {
                let raw = u16::try_from(self.to_bits().try_zext_u64().unwrap_or(0)).unwrap_or(0);
                f64::from(f32::from_bits(u32::from(raw) << 16))
            }
            ApFloatSemantics::IeeeQuad => quad_to_f64(&self.to_bits()),
            ApFloatSemantics::X87DoubleExtended => x87_to_f64(&self.to_bits()),
            ApFloatSemantics::PpcDoubleDouble => {
                let words = self.to_bits();
                f64::from_bits(words.words().get(1).copied().unwrap_or(0))
                    + f64::from_bits(words.words().first().copied().unwrap_or(0))
            }
        }
    }
}

struct BinaryFloatComponents {
    negative: bool,
    exponent: i32,
    precision: u32,
    magnitude: ApInt,
}

// PPC double-double is stored as two IEEE doubles in llvmkit word order
// (low component word first, high component word second), mirroring
// `llvm/lib/Support/APFloat.cpp::DoubleAPFloat::bitcastToAPInt`.
fn ppc_words(value: &ApFloat) -> (u64, u64) {
    let bits = value.to_bits();
    let words = bits.words();
    (
        words.get(1).copied().unwrap_or(0),
        words.first().copied().unwrap_or(0),
    )
}

fn ppc_high_float(value: &ApFloat) -> ApFloat {
    let (high, _) = ppc_words(value);
    ApFloat::from_bits_unchecked(ApFloatSemantics::IeeeDouble, &[high])
}

fn ppc_low_float(value: &ApFloat) -> ApFloat {
    let (_, low) = ppc_words(value);
    ApFloat::from_bits_unchecked(ApFloatSemantics::IeeeDouble, &[low])
}

fn ppc_from_high_low(high: &ApFloat, low: &ApFloat) -> Option<ApFloat> {
    Some(ApFloat::from_bits_unchecked(
        ApFloatSemantics::PpcDoubleDouble,
        &[
            low.to_bits().try_zext_u64()?,
            high.to_bits().try_zext_u64()?,
        ],
    ))
}

fn ppc_double_component(bits: u64) -> Option<Option<BinaryFloatComponents>> {
    let value = ApFloat::from_bits_unchecked(ApFloatSemantics::IeeeDouble, &[bits]);
    if value.is_zero() {
        return Some(None);
    }
    value.binary_components().map(Some)
}

fn ppc_double_double_components(value: &ApFloat) -> Option<BinaryFloatComponents> {
    let (high_bits, low_bits) = ppc_words(value);
    let high = ppc_double_component(high_bits)?;
    let low = ppc_double_component(low_bits)?;
    scaled_sum_components(&[high.as_ref(), low.as_ref()])?
}

fn scaled_sum_components(
    components: &[Option<&BinaryFloatComponents>],
) -> Option<Option<BinaryFloatComponents>> {
    let mut min_scale: Option<i32> = None;
    for component in components.iter().filter_map(|component| *component) {
        let scale = component_scale(component)?;
        min_scale = Some(min_scale.map_or(scale, |current| current.min(scale)));
    }
    let Some(min_scale) = min_scale else {
        return Some(None);
    };

    let mut work_width = 2;
    for component in components.iter().filter_map(|component| *component) {
        let scale = component_scale(component)?;
        let shift = u32::try_from(scale.checked_sub(min_scale)?).ok()?;
        work_width = work_width.max(
            component
                .magnitude
                .active_bits()
                .checked_add(shift)?
                .checked_add(2)?,
        );
    }

    let mut positive = ApInt::zero(work_width);
    let mut negative = ApInt::zero(work_width);
    for component in components.iter().filter_map(|component| *component) {
        let scale = component_scale(component)?;
        let shift = u32::try_from(scale.checked_sub(min_scale)?).ok()?;
        let term = shift_magnitude_to_width(&component.magnitude, shift, work_width)?;
        if component.negative {
            negative = negative.wrapping_add(&term);
        } else {
            positive = positive.wrapping_add(&term);
        }
    }

    let (is_negative, magnitude) = if positive.uge(&negative) {
        (false, positive.wrapping_sub(&negative))
    } else {
        (true, negative.wrapping_sub(&positive))
    };
    if magnitude.is_zero() {
        return Some(None);
    }
    let precision = magnitude.active_bits().max(1);
    let exponent = min_scale.checked_add(i32::try_from(precision.checked_sub(1)?).ok()?)?;
    Some(Some(BinaryFloatComponents {
        negative: is_negative,
        exponent,
        precision,
        magnitude,
    }))
}

fn ppc_is_denormal(value: &ApFloat) -> bool {
    if !matches!(value.category(), ApFloatCategory::Normal) {
        return false;
    }
    let high = ppc_high_float(value);
    let low = ppc_low_float(value);
    if high.is_denormal() || low.is_denormal() {
        return true;
    }
    let (sum, _) = high.add(&low, RoundingMode::NearestTiesToEven);
    !sum.bitwise_is_equal(&high)
}

fn ppc_in_lattice(high: &ApFloat, low: &ApFloat) -> bool {
    let (sum, _) = high.add(low, RoundingMode::NearestTiesToEven);
    sum.bitwise_is_equal(high)
}

fn ppc_harrison_ulp(value: &ApFloat) -> Option<ApFloat> {
    if value.is_nan() {
        return Some(ApFloat::qnan(
            ApFloatSemantics::IeeeDouble,
            ApFloatSign::Positive,
            NanPayload::Absent,
        ));
    }
    if value.is_infinity() {
        return Some(ApFloat::inf(
            ApFloatSemantics::IeeeDouble,
            sign_from_negative(value.is_negative()),
        ));
    }
    if value.is_zero()
        || value.is_denormal()
        || value.bitwise_is_equal(&ApFloat::smallest_normalized(
            ApFloatSemantics::IeeeDouble,
            sign_from_negative(value.is_negative()),
        ))
    {
        return Some(ApFloat::smallest(
            ApFloatSemantics::IeeeDouble,
            ApFloatSign::Positive,
        ));
    }
    let components = value.binary_components()?;
    let mut exponent = components.exponent;
    if components.magnitude.is_power_of_2() {
        exponent = exponent.checked_sub(1)?;
    }
    encode_binary_scaled(
        ApFloatSemantics::IeeeDouble,
        false,
        ApInt::from_words(1, &[1]),
        exponent.checked_sub(i32::try_from(ApFloatSemantics::IeeeDouble.precision() - 1).ok()?)?,
        RoundingMode::NearestTiesToEven,
    )
    .map(|(ulp, _)| ulp)
}

fn ppc_next(value: &ApFloat, direction: ApFloatNextDirection) -> (ApFloat, ApFloatStatus) {
    if matches!(direction, ApFloatNextDirection::TowardNegative) {
        let negated = value.change_sign();
        let (next, status) = ppc_next(&negated, ApFloatNextDirection::TowardPositive);
        return (next.change_sign(), status);
    }
    if value.is_pos_infinity() {
        return (value.clone(), ApFloatStatus::OK);
    }
    if value.is_neg_infinity() {
        return (
            ApFloat::largest(ApFloatSemantics::PpcDoubleDouble, ApFloatSign::Negative),
            ApFloatStatus::OK,
        );
    }
    if value.is_zero() {
        return (
            ApFloat::smallest(ApFloatSemantics::PpcDoubleDouble, ApFloatSign::Positive),
            ApFloatStatus::OK,
        );
    }

    let high = ppc_high_float(value);
    let low = ppc_low_float(value);
    let (next_low, low_status) = low.next(ApFloatNextDirection::TowardPositive);
    if low_status.contains(ApFloatStatus::INVALID_OP) {
        return (value.clone(), low_status);
    }
    if ppc_in_lattice(&high, &next_low) {
        let Some(candidate) = ppc_from_high_low(&high, &next_low) else {
            return (value.clone(), ApFloatStatus::INVALID_OP);
        };
        if matches!(
            candidate.compare(&ApFloat::largest(
                ApFloatSemantics::PpcDoubleDouble,
                ApFloatSign::Positive,
            )),
            ApFloatCmpResult::GreaterThan
        ) {
            return (
                ApFloat::inf(ApFloatSemantics::PpcDoubleDouble, ApFloatSign::Positive),
                ApFloatStatus::OK,
            );
        }
        return (candidate, ApFloatStatus::OK);
    }

    let (next_high, _) = high.next(ApFloatNextDirection::TowardPositive);
    if next_high.is_infinity() {
        return (
            ApFloat::inf(ApFloatSemantics::PpcDoubleDouble, ApFloatSign::Positive),
            ApFloatStatus::OK,
        );
    }
    if next_high.is_zero() {
        return (
            ApFloat::zero(ApFloatSemantics::PpcDoubleDouble, ApFloatSign::Negative),
            ApFloatStatus::OK,
        );
    }
    let Some(ulp) = ppc_harrison_ulp(&next_high) else {
        return (value.clone(), ApFloatStatus::INVALID_OP);
    };
    let (half_ulp, _) = ulp.scalbn(-1, RoundingMode::TowardZero);
    let mut next_low = half_ulp.change_sign();
    if !ppc_in_lattice(&next_high, &next_low) {
        next_low = next_low.next(ApFloatNextDirection::TowardPositive).0;
    }
    let Some(candidate) = ppc_from_high_low(&next_high, &next_low) else {
        return (value.clone(), ApFloatStatus::INVALID_OP);
    };
    (candidate, ApFloatStatus::OK)
}

fn compare_ppc_double_double(lhs: &ApFloat, rhs: &ApFloat) -> Option<Ordering> {
    let lhs_bits = lhs.to_bits();
    let rhs_bits = rhs.to_bits();
    let lhs_words = lhs_bits.words();
    let rhs_words = rhs_bits.words();
    let lhs_high = ApFloat::from_bits_unchecked(
        ApFloatSemantics::IeeeDouble,
        &[lhs_words.get(1).copied().unwrap_or(0)],
    );
    let rhs_high = ApFloat::from_bits_unchecked(
        ApFloatSemantics::IeeeDouble,
        &[rhs_words.get(1).copied().unwrap_or(0)],
    );
    let high_ordering = cmp_result_to_ordering(lhs_high.compare(&rhs_high))?;
    if !matches!(high_ordering, Ordering::Equal) {
        return Some(high_ordering);
    }
    let lhs_low = ApFloat::from_bits_unchecked(
        ApFloatSemantics::IeeeDouble,
        &[lhs_words.first().copied().unwrap_or(0)],
    );
    let rhs_low = ApFloat::from_bits_unchecked(
        ApFloatSemantics::IeeeDouble,
        &[rhs_words.first().copied().unwrap_or(0)],
    );
    cmp_result_to_ordering(lhs_low.compare(&rhs_low))
}

fn cmp_result_to_ordering(result: ApFloatCmpResult) -> Option<Ordering> {
    match result {
        ApFloatCmpResult::LessThan => Some(Ordering::Less),
        ApFloatCmpResult::Equal => Some(Ordering::Equal),
        ApFloatCmpResult::GreaterThan => Some(Ordering::Greater),
        ApFloatCmpResult::Unordered => None,
    }
}

fn float_abs_cmp(lhs: &ApFloat, rhs: &ApFloat) -> Option<Ordering> {
    if lhs.is_zero() || rhs.is_zero() {
        return Some(match (lhs.is_zero(), rhs.is_zero()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => Ordering::Equal,
        });
    }
    if lhs.is_infinity() || rhs.is_infinity() {
        return Some(match (lhs.is_infinity(), rhs.is_infinity()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => Ordering::Equal,
        });
    }
    let lhs_components = lhs.binary_components()?;
    let rhs_components = rhs.binary_components()?;
    compare_binary_magnitude(&lhs_components, &rhs_components)
}

fn compare_binary_magnitude(
    lhs: &BinaryFloatComponents,
    rhs: &BinaryFloatComponents,
) -> Option<Ordering> {
    let lhs_scale = component_scale(lhs)?;
    let rhs_scale = component_scale(rhs)?;
    let common_scale = lhs_scale.min(rhs_scale);
    let lhs_shift = u32::try_from(lhs_scale.checked_sub(common_scale)?).ok()?;
    let rhs_shift = u32::try_from(rhs_scale.checked_sub(common_scale)?).ok()?;
    let lhs_width = lhs
        .magnitude
        .active_bits()
        .checked_add(lhs_shift)?
        .checked_add(1)?;
    let rhs_width = rhs
        .magnitude
        .active_bits()
        .checked_add(rhs_shift)?
        .checked_add(1)?;
    let work_width = lhs_width.max(rhs_width).max(1);
    let lhs_magnitude = lhs.magnitude.zext_or_trunc(work_width);
    let rhs_magnitude = rhs.magnitude.zext_or_trunc(work_width);
    let lhs_magnitude = if lhs_shift == 0 {
        lhs_magnitude
    } else {
        lhs_magnitude.checked_shl(lhs_shift)?
    };
    let rhs_magnitude = if rhs_shift == 0 {
        rhs_magnitude
    } else {
        rhs_magnitude.checked_shl(rhs_shift)?
    };
    if lhs_magnitude.ult(&rhs_magnitude) {
        Some(Ordering::Less)
    } else if lhs_magnitude.ugt(&rhs_magnitude) {
        Some(Ordering::Greater)
    } else {
        Some(Ordering::Equal)
    }
}

fn binary_component_is_integer(components: &BinaryFloatComponents) -> Option<bool> {
    let scale = component_scale(components)?;
    if scale >= 0 {
        Some(true)
    } else {
        Some(!any_low_bit_set(
            &components.magnitude,
            scale.unsigned_abs(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FiniteBinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Modulo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MagnitudeOp {
    Add,
    Subtract,
}

fn special_binary_result(
    lhs: &ApFloat,
    rhs: &ApFloat,
    rounding: RoundingMode,
    op: FiniteBinaryOp,
) -> Option<(ApFloat, ApFloatStatus)> {
    match op {
        FiniteBinaryOp::Add => special_add_or_subtract(lhs, rhs, rounding, MagnitudeOp::Add),
        FiniteBinaryOp::Subtract => {
            special_add_or_subtract(lhs, rhs, rounding, MagnitudeOp::Subtract)
        }
        FiniteBinaryOp::Multiply => special_multiply(lhs, rhs),
        FiniteBinaryOp::Divide => special_divide(lhs, rhs),
        FiniteBinaryOp::Remainder | FiniteBinaryOp::Modulo => special_modulo(lhs, rhs),
    }
}

fn special_add_or_subtract(
    lhs: &ApFloat,
    rhs: &ApFloat,
    rounding: RoundingMode,
    op: MagnitudeOp,
) -> Option<(ApFloat, ApFloatStatus)> {
    if lhs.is_nan() || rhs.is_nan() {
        return Some(nan_binary_result(lhs, rhs));
    }
    let subtract = matches!(op, MagnitudeOp::Subtract);
    let rhs_negative = rhs.is_negative() != subtract;
    if lhs.is_infinity() || rhs.is_infinity() {
        if lhs.is_infinity() && rhs.is_infinity() {
            if lhs.is_negative() != rhs_negative {
                return Some(invalid_nan(lhs.semantics()));
            }
            return Some((lhs.clone(), ApFloatStatus::OK));
        }
        if lhs.is_infinity() {
            return Some((lhs.clone(), ApFloatStatus::OK));
        }
        return Some((
            ApFloat::inf(lhs.semantics(), sign_from_negative(rhs_negative)),
            ApFloatStatus::OK,
        ));
    }
    if lhs.is_zero() || rhs.is_zero() {
        if lhs.is_zero() && rhs.is_zero() {
            let negative = if lhs.is_negative() == rhs_negative {
                lhs.is_negative()
            } else {
                matches!(rounding, RoundingMode::TowardNegative)
            };
            return Some((
                ApFloat::zero(lhs.semantics(), sign_from_negative(negative)),
                ApFloatStatus::OK,
            ));
        }
        if lhs.is_zero() {
            return Some((with_sign(rhs, rhs_negative), ApFloatStatus::OK));
        }
        return Some((lhs.clone(), ApFloatStatus::OK));
    }
    None
}

fn special_multiply(lhs: &ApFloat, rhs: &ApFloat) -> Option<(ApFloat, ApFloatStatus)> {
    if lhs.is_nan() || rhs.is_nan() {
        return Some(nan_binary_result(lhs, rhs));
    }
    let negative = lhs.is_negative() != rhs.is_negative();
    if (lhs.is_zero() && rhs.is_infinity()) || (lhs.is_infinity() && rhs.is_zero()) {
        return Some(invalid_nan(lhs.semantics()));
    }
    if lhs.is_infinity() || rhs.is_infinity() {
        return Some((
            ApFloat::inf(lhs.semantics(), sign_from_negative(negative)),
            ApFloatStatus::OK,
        ));
    }
    if lhs.is_zero() || rhs.is_zero() {
        return Some((
            ApFloat::zero(lhs.semantics(), sign_from_negative(negative)),
            ApFloatStatus::OK,
        ));
    }
    None
}

fn special_fma_result(
    lhs: &ApFloat,
    multiplicand: &ApFloat,
    addend: &ApFloat,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let product_negative = lhs.is_negative() != multiplicand.is_negative();
    if (lhs.is_zero() && multiplicand.is_infinity())
        || (lhs.is_infinity() && multiplicand.is_zero())
    {
        return Some(invalid_nan(lhs.semantics()));
    }
    if lhs.is_nan() || multiplicand.is_nan() || addend.is_nan() {
        let mut status = ApFloatStatus::OK;
        if lhs.is_signaling() || multiplicand.is_signaling() || addend.is_signaling() {
            status.insert(ApFloatStatus::INVALID_OP);
        }
        if lhs.is_nan() {
            return Some((lhs.make_quiet(), status));
        }
        if multiplicand.is_nan() {
            return Some((multiplicand.make_quiet(), status));
        }
        return Some((addend.make_quiet(), status));
    }
    if lhs.is_infinity() || multiplicand.is_infinity() {
        if addend.is_infinity() && addend.is_negative() != product_negative {
            return Some(invalid_nan(lhs.semantics()));
        }
        return Some((
            ApFloat::inf(lhs.semantics(), sign_from_negative(product_negative)),
            ApFloatStatus::OK,
        ));
    }
    if addend.is_infinity() {
        return Some((addend.clone(), ApFloatStatus::OK));
    }
    if lhs.is_zero() || multiplicand.is_zero() {
        if addend.is_zero() {
            let negative = if product_negative == addend.is_negative() {
                product_negative
            } else {
                matches!(rounding, RoundingMode::TowardNegative)
            };
            return Some((
                ApFloat::zero(lhs.semantics(), sign_from_negative(negative)),
                ApFloatStatus::OK,
            ));
        }
        return Some((addend.clone(), ApFloatStatus::OK));
    }
    if addend.is_zero() {
        let lhs_components = lhs.binary_components()?;
        let rhs_components = multiplicand.binary_components()?;
        return finite_multiply(lhs.semantics(), &lhs_components, &rhs_components, rounding);
    }
    None
}

fn special_divide(lhs: &ApFloat, rhs: &ApFloat) -> Option<(ApFloat, ApFloatStatus)> {
    if lhs.is_nan() || rhs.is_nan() {
        return Some(nan_binary_result(lhs, rhs));
    }
    let negative = lhs.is_negative() != rhs.is_negative();
    if (lhs.is_zero() && rhs.is_zero()) || (lhs.is_infinity() && rhs.is_infinity()) {
        return Some(invalid_nan(lhs.semantics()));
    }
    if lhs.is_infinity() {
        return Some((
            ApFloat::inf(lhs.semantics(), sign_from_negative(negative)),
            ApFloatStatus::OK,
        ));
    }
    if rhs.is_zero() {
        let status = ApFloatStatus::DIV_BY_ZERO;
        return Some((
            ApFloat::inf(lhs.semantics(), sign_from_negative(negative)),
            status,
        ));
    }
    if rhs.is_infinity() {
        return Some((
            ApFloat::zero(lhs.semantics(), sign_from_negative(negative)),
            ApFloatStatus::OK,
        ));
    }
    if lhs.is_zero() {
        return Some((
            ApFloat::zero(lhs.semantics(), sign_from_negative(negative)),
            ApFloatStatus::OK,
        ));
    }
    None
}

fn special_modulo(lhs: &ApFloat, rhs: &ApFloat) -> Option<(ApFloat, ApFloatStatus)> {
    if lhs.is_nan() || rhs.is_nan() {
        return Some(nan_binary_result(lhs, rhs));
    }
    if lhs.is_zero() && !rhs.is_zero() {
        return Some((lhs.clone(), ApFloatStatus::OK));
    }
    if rhs.is_infinity() && lhs.is_finite() {
        return Some((lhs.clone(), ApFloatStatus::OK));
    }
    if rhs.is_zero() || lhs.is_infinity() {
        return Some(invalid_nan(lhs.semantics()));
    }
    None
}

fn nan_binary_result(lhs: &ApFloat, rhs: &ApFloat) -> (ApFloat, ApFloatStatus) {
    let mut status = ApFloatStatus::OK;
    if lhs.is_signaling() || rhs.is_signaling() {
        status.insert(ApFloatStatus::INVALID_OP);
    }
    let value = if lhs.is_nan() {
        lhs.make_quiet()
    } else {
        rhs.make_quiet()
    };
    (value, status)
}

fn invalid_nan(semantics: ApFloatSemantics) -> (ApFloat, ApFloatStatus) {
    (
        ApFloat::qnan(semantics, ApFloatSign::Positive, NanPayload::Absent),
        ApFloatStatus::INVALID_OP,
    )
}

fn with_sign(value: &ApFloat, negative: bool) -> ApFloat {
    if value.is_negative() == negative {
        value.clone()
    } else {
        value.change_sign()
    }
}

fn sign_from_negative(negative: bool) -> ApFloatSign {
    if negative {
        ApFloatSign::Negative
    } else {
        ApFloatSign::Positive
    }
}

fn convert_ppc_double_double(
    value: &ApFloat,
    to_semantics: ApFloatSemantics,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    if to_semantics == ApFloatSemantics::PpcDoubleDouble {
        return Some((value.clone(), ApFloatStatus::OK));
    }
    let components = ppc_double_double_components(value)?;
    let scale = component_scale(&components)?;
    encode_binary_scaled(
        to_semantics,
        components.negative,
        components.magnitude,
        scale,
        rounding,
    )
}

fn finite_add_or_subtract(
    semantics: ApFloatSemantics,
    lhs: &BinaryFloatComponents,
    rhs: &BinaryFloatComponents,
    rounding: RoundingMode,
    op: MagnitudeOp,
) -> Option<(ApFloat, ApFloatStatus)> {
    let lhs_scale = component_scale(lhs)?;
    let rhs_scale = component_scale(rhs)?;
    let common_scale = lhs_scale.min(rhs_scale);
    let lhs_shift = u32::try_from(lhs_scale.checked_sub(common_scale)?).ok()?;
    let rhs_shift = u32::try_from(rhs_scale.checked_sub(common_scale)?).ok()?;
    let lhs_width = lhs
        .magnitude
        .active_bits()
        .checked_add(lhs_shift)?
        .checked_add(2)?;
    let rhs_width = rhs
        .magnitude
        .active_bits()
        .checked_add(rhs_shift)?
        .checked_add(2)?;
    let work_width = lhs_width.max(rhs_width).max(2);
    let lhs_value = shift_magnitude_to_width(&lhs.magnitude, lhs_shift, work_width)?;
    let rhs_value = shift_magnitude_to_width(&rhs.magnitude, rhs_shift, work_width)?;
    let rhs_negative = rhs.negative != matches!(op, MagnitudeOp::Subtract);
    if lhs.negative == rhs_negative {
        return encode_binary_scaled(
            semantics,
            lhs.negative,
            lhs_value.wrapping_add(&rhs_value),
            common_scale,
            rounding,
        );
    }
    let (negative, magnitude) = if lhs_value.uge(&rhs_value) {
        (lhs.negative, lhs_value.wrapping_sub(&rhs_value))
    } else {
        (rhs_negative, rhs_value.wrapping_sub(&lhs_value))
    };
    if magnitude.is_zero() {
        return Some((
            ApFloat::zero(
                semantics,
                sign_from_negative(matches!(rounding, RoundingMode::TowardNegative)),
            ),
            ApFloatStatus::OK,
        ));
    }
    encode_binary_scaled(semantics, negative, magnitude, common_scale, rounding)
}
fn finite_fused_multiply_add(
    semantics: ApFloatSemantics,
    lhs: &BinaryFloatComponents,
    rhs: &BinaryFloatComponents,
    addend: &BinaryFloatComponents,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let lhs_scale = component_scale(lhs)?;
    let rhs_scale = component_scale(rhs)?;
    let product_scale = lhs_scale.checked_add(rhs_scale)?;
    let add_scale = component_scale(addend)?;
    let common_scale = product_scale.min(add_scale);
    let product_shift = u32::try_from(product_scale.checked_sub(common_scale)?).ok()?;
    let add_shift = u32::try_from(add_scale.checked_sub(common_scale)?).ok()?;
    let product_width = lhs
        .magnitude
        .active_bits()
        .checked_add(rhs.magnitude.active_bits())?
        .checked_add(product_shift)?
        .checked_add(2)?;
    let add_width = addend
        .magnitude
        .active_bits()
        .checked_add(add_shift)?
        .checked_add(2)?;
    let work_width = product_width.max(add_width).max(2);
    let lhs_value = lhs.magnitude.zext_or_trunc(work_width);
    let rhs_value = rhs.magnitude.zext_or_trunc(work_width);
    let product = lhs_value.wrapping_mul(&rhs_value);
    let product = shift_magnitude_to_width(&product, product_shift, work_width)?;
    let add_value = shift_magnitude_to_width(&addend.magnitude, add_shift, work_width)?;
    let product_negative = lhs.negative != rhs.negative;
    if product_negative == addend.negative {
        return encode_binary_scaled(
            semantics,
            product_negative,
            product.wrapping_add(&add_value),
            common_scale,
            rounding,
        );
    }
    let (negative, magnitude) = if product.uge(&add_value) {
        (product_negative, product.wrapping_sub(&add_value))
    } else {
        (addend.negative, add_value.wrapping_sub(&product))
    };
    if magnitude.is_zero() {
        return Some((
            ApFloat::zero(
                semantics,
                sign_from_negative(matches!(rounding, RoundingMode::TowardNegative)),
            ),
            ApFloatStatus::OK,
        ));
    }
    encode_binary_scaled(semantics, negative, magnitude, common_scale, rounding)
}

fn finite_multiply(
    semantics: ApFloatSemantics,
    lhs: &BinaryFloatComponents,
    rhs: &BinaryFloatComponents,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let lhs_scale = component_scale(lhs)?;
    let rhs_scale = component_scale(rhs)?;
    let scale = lhs_scale.checked_add(rhs_scale)?;
    let work_width = lhs
        .magnitude
        .active_bits()
        .checked_add(rhs.magnitude.active_bits())?
        .checked_add(2)?
        .max(2);
    let lhs_value = lhs.magnitude.zext_or_trunc(work_width);
    let rhs_value = rhs.magnitude.zext_or_trunc(work_width);
    encode_binary_scaled(
        semantics,
        lhs.negative != rhs.negative,
        lhs_value.wrapping_mul(&rhs_value),
        scale,
        rounding,
    )
}

fn finite_divide(
    semantics: ApFloatSemantics,
    lhs: &BinaryFloatComponents,
    rhs: &BinaryFloatComponents,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let scale = component_scale(lhs)?.checked_sub(component_scale(rhs)?)?;
    encode_binary_rational_scaled(
        semantics,
        lhs.negative != rhs.negative,
        lhs.magnitude.clone(),
        rhs.magnitude.clone(),
        scale,
        rounding,
    )
}

fn finite_modulo(
    semantics: ApFloatSemantics,
    lhs: &BinaryFloatComponents,
    rhs: &BinaryFloatComponents,
) -> Option<(ApFloat, ApFloatStatus)> {
    let lhs_scale = component_scale(lhs)?;
    let rhs_scale = component_scale(rhs)?;
    let common_scale = lhs_scale.min(rhs_scale);
    let lhs_shift = u32::try_from(lhs_scale.checked_sub(common_scale)?).ok()?;
    let rhs_shift = u32::try_from(rhs_scale.checked_sub(common_scale)?).ok()?;
    let lhs_width = lhs
        .magnitude
        .active_bits()
        .checked_add(lhs_shift)?
        .checked_add(1)?;
    let rhs_width = rhs
        .magnitude
        .active_bits()
        .checked_add(rhs_shift)?
        .checked_add(1)?;
    let work_width = lhs_width.max(rhs_width).max(2);
    let lhs_value = shift_magnitude_to_width(&lhs.magnitude, lhs_shift, work_width)?;
    let rhs_value = shift_magnitude_to_width(&rhs.magnitude, rhs_shift, work_width)?;
    let remainder = lhs_value.checked_urem(&rhs_value)?;
    if remainder.is_zero() {
        return Some((
            ApFloat::zero(semantics, sign_from_negative(lhs.negative)),
            ApFloatStatus::OK,
        ));
    }
    encode_binary_scaled(
        semantics,
        lhs.negative,
        remainder,
        common_scale,
        RoundingMode::NearestTiesToEven,
    )
}

fn finite_remainder(
    semantics: ApFloatSemantics,
    lhs: &BinaryFloatComponents,
    rhs: &BinaryFloatComponents,
) -> Option<(ApFloat, ApFloatStatus)> {
    let lhs_scale = component_scale(lhs)?;
    let rhs_scale = component_scale(rhs)?;
    let common_scale = lhs_scale.min(rhs_scale);
    let lhs_shift = u32::try_from(lhs_scale.checked_sub(common_scale)?).ok()?;
    let rhs_shift = u32::try_from(rhs_scale.checked_sub(common_scale)?).ok()?;
    let lhs_width = lhs
        .magnitude
        .active_bits()
        .checked_add(lhs_shift)?
        .checked_add(1)?;
    let rhs_width = rhs
        .magnitude
        .active_bits()
        .checked_add(rhs_shift)?
        .checked_add(1)?;
    let work_width = lhs_width.max(rhs_width).max(2);
    let lhs_value = shift_magnitude_to_width(&lhs.magnitude, lhs_shift, work_width)?;
    let rhs_value = shift_magnitude_to_width(&rhs.magnitude, rhs_shift, work_width)?;
    let divrem = lhs_value.udivrem(&rhs_value)?;
    let remainder = divrem.remainder().clone();
    if remainder.is_zero() {
        return Some((
            ApFloat::zero(semantics, sign_from_negative(lhs.negative)),
            ApFloatStatus::OK,
        ));
    }
    let cmp_width = work_width.checked_add(1)?;
    let twice_remainder = remainder.zext_or_trunc(cmp_width).checked_shl(1)?;
    let rhs_value = rhs_value.zext_or_trunc(cmp_width);
    let round_away_from_remainder = twice_remainder.ugt(&rhs_value)
        || (twice_remainder.eq_ap_int(&rhs_value) && divrem.quotient().is_one_bit_set(0));
    if round_away_from_remainder {
        return encode_binary_scaled(
            semantics,
            !lhs.negative,
            rhs_value.wrapping_sub(&remainder.zext_or_trunc(cmp_width)),
            common_scale,
            RoundingMode::NearestTiesToEven,
        );
    }
    encode_binary_scaled(
        semantics,
        lhs.negative,
        remainder,
        common_scale,
        RoundingMode::NearestTiesToEven,
    )
}

fn component_scale(components: &BinaryFloatComponents) -> Option<i32> {
    components
        .exponent
        .checked_sub(i32::try_from(components.precision.checked_sub(1)?).ok()?)
}

fn shift_magnitude_to_width(magnitude: &ApInt, shift: u32, width: u32) -> Option<ApInt> {
    let extended = magnitude.zext_or_trunc(width);
    if shift == 0 {
        Some(extended)
    } else {
        extended.checked_shl(shift)
    }
}

fn encode_binary_scaled(
    semantics: ApFloatSemantics,
    negative: bool,
    magnitude: ApInt,
    scale: i32,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    encode_binary_rational_scaled(
        semantics,
        negative,
        magnitude,
        ApInt::from_words(1, &[1]),
        scale,
        rounding,
    )
}

// Mirrors `DoubleAPFloat::convertFromUnsignedParts` and
// `IEEEFloat::convertPPCDoubleDoubleLegacyAPFloatToAPInt`: form the nearest
// high double first, then encode the exact residual as the low double.
fn encode_ppc_double_double_rational(
    negative: bool,
    numerator: ApInt,
    denominator: ApInt,
    scale: i32,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    if denominator.is_zero() {
        return None;
    }
    let (high, _) = encode_binary_rational_scaled(
        ApFloatSemantics::IeeeDouble,
        negative,
        numerator.clone(),
        denominator.clone(),
        scale,
        RoundingMode::NearestTiesToEven,
    )?;
    if high.is_infinity() {
        return Some(overflow_result(
            ApFloatSemantics::PpcDoubleDouble,
            negative,
            rounding,
        ));
    }
    if high.is_zero() {
        let (low, status) = encode_binary_rational_scaled(
            ApFloatSemantics::IeeeDouble,
            negative,
            numerator,
            denominator,
            scale,
            rounding,
        )?;
        if low.is_zero() {
            return Some((
                ApFloat::zero(
                    ApFloatSemantics::PpcDoubleDouble,
                    sign_from_negative(negative),
                ),
                status,
            ));
        }
        let zero = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive);
        return Some((ppc_from_high_low(&low, &zero)?, status));
    }

    let high_component = high.binary_components()?;
    let (residual_negative, residual, denominator, residual_scale) =
        residual_rational_after_component(
            negative,
            numerator,
            denominator,
            scale,
            &high_component,
        )?;
    let (low, status) = if residual.is_zero() {
        (
            ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive),
            ApFloatStatus::OK,
        )
    } else {
        encode_binary_rational_scaled(
            ApFloatSemantics::IeeeDouble,
            residual_negative,
            residual,
            denominator,
            residual_scale,
            rounding,
        )?
    };
    let value = ppc_from_high_low(&high, &low)?;
    let limit = ApFloat::largest(
        ApFloatSemantics::PpcDoubleDouble,
        sign_from_negative(negative),
    );
    let overflow = if negative {
        matches!(value.compare(&limit), ApFloatCmpResult::LessThan)
    } else {
        matches!(value.compare(&limit), ApFloatCmpResult::GreaterThan)
    };
    if overflow {
        return Some(overflow_result(
            ApFloatSemantics::PpcDoubleDouble,
            negative,
            rounding,
        ));
    }
    Some((value, status))
}

fn residual_rational_after_component(
    value_negative: bool,
    numerator: ApInt,
    denominator: ApInt,
    scale: i32,
    component: &BinaryFloatComponents,
) -> Option<(bool, ApInt, ApInt, i32)> {
    if denominator.is_zero() {
        return None;
    }
    let component_scale = component_scale(component)?;
    let common_scale = scale.min(component_scale);
    let value_shift = u32::try_from(scale.checked_sub(common_scale)?).ok()?;
    let component_shift = u32::try_from(component_scale.checked_sub(common_scale)?).ok()?;
    let value_width = numerator
        .active_bits()
        .checked_add(value_shift)?
        .checked_add(2)?;
    let component_width = component
        .magnitude
        .active_bits()
        .checked_add(denominator.active_bits())?
        .checked_add(component_shift)?
        .checked_add(2)?;
    let work_width = value_width.max(component_width).max(2);
    let value_term = shift_magnitude_to_width(&numerator, value_shift, work_width)?;
    let component_product = component
        .magnitude
        .zext_or_trunc(work_width)
        .wrapping_mul(&denominator.zext_or_trunc(work_width));
    let component_term = if component_shift == 0 {
        component_product
    } else {
        component_product.checked_shl(component_shift)?
    };

    let mut positive = ApInt::zero(work_width);
    let mut negative = ApInt::zero(work_width);
    if value_negative {
        negative = negative.wrapping_add(&value_term);
    } else {
        positive = positive.wrapping_add(&value_term);
    }
    if component.negative {
        positive = positive.wrapping_add(&component_term);
    } else {
        negative = negative.wrapping_add(&component_term);
    }

    let (residual_negative, residual) = if positive.uge(&negative) {
        (false, positive.wrapping_sub(&negative))
    } else {
        (true, negative.wrapping_sub(&positive))
    };
    Some((residual_negative, residual, denominator, common_scale))
}

fn encode_binary_rational_scaled(
    semantics: ApFloatSemantics,
    negative: bool,
    numerator: ApInt,
    denominator: ApInt,
    scale: i32,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    if numerator.is_zero() {
        return Some((
            ApFloat::zero(semantics, sign_from_negative(negative)),
            ApFloatStatus::OK,
        ));
    }
    if matches!(semantics, ApFloatSemantics::PpcDoubleDouble) {
        return encode_ppc_double_double_rational(
            negative,
            numerator,
            denominator,
            scale,
            rounding,
        );
    }
    let bias = exponent_bias(semantics)?;
    let precision = semantics.precision();
    let mut exponent =
        rational_binary_exponent_wide(&numerator, &denominator)?.checked_add(scale)?;
    let max_exponent = max_exponent_field(semantics)?;
    let exponent_field = exponent.checked_add(bias)?;
    if exponent_field <= 0 {
        return encode_binary_rational_subnormal(
            semantics,
            negative,
            numerator,
            denominator,
            scale,
            rounding,
        );
    }
    if exponent_field >= max_exponent {
        return Some(overflow_result(semantics, negative, rounding));
    }
    let shift = scale
        .checked_add(i32::try_from(precision.checked_sub(1)?).ok()?)?
        .checked_sub(exponent)?;
    let (scaled_numerator, scaled_denominator) =
        scale_rational_by_power_of_two(numerator, denominator, shift)?;
    let (mut significand, inexact) =
        round_apint_div_wide(scaled_numerator, scaled_denominator, rounding, negative)?;
    if apint_ge_power_of_two(&significand, precision)? {
        significand = significand.checked_lshr(1)?;
        exponent = exponent.checked_add(1)?;
    }
    let exponent_field = exponent.checked_add(bias)?;
    if exponent_field >= max_exponent {
        return Some(overflow_result(semantics, negative, rounding));
    }
    let fraction_width = significand.bit_width().max(precision);
    let significand = significand.zext_or_trunc(fraction_width);
    let hidden = ApInt::one_bit_set(fraction_width, precision.checked_sub(1)?);
    let fraction = significand.wrapping_sub(&hidden).try_zext_u128()?;
    let bits = pack_decimal_bits(
        semantics,
        negative,
        u64::try_from(exponent_field).ok()?,
        fraction,
        significand.try_zext_u128()?,
    )?;
    Some((
        ApFloat::from_bits_unchecked(semantics, &bits),
        status_from_inexact(inexact),
    ))
}

fn encode_binary_rational_subnormal(
    semantics: ApFloatSemantics,
    negative: bool,
    numerator: ApInt,
    denominator: ApInt,
    scale: i32,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let bias = exponent_bias(semantics)?;
    let fraction_bits = semantics.precision().checked_sub(1)?;
    let unit_exponent = 1i32
        .checked_sub(bias)?
        .checked_sub(i32::try_from(fraction_bits).ok()?)?;
    let shift = scale.checked_sub(unit_exponent)?;
    let (scaled_numerator, scaled_denominator) =
        scale_rational_by_power_of_two(numerator, denominator, shift)?;
    let (significand, inexact) =
        round_apint_div_wide(scaled_numerator, scaled_denominator, rounding, negative)?;
    let mut status = status_from_inexact(inexact);
    if inexact {
        status.insert(ApFloatStatus::UNDERFLOW);
    }
    if significand.is_zero() {
        return Some((
            ApFloat::zero(semantics, sign_from_negative(negative)),
            status,
        ));
    }
    let (exponent, fraction, significand_bits) =
        if apint_ge_power_of_two(&significand, fraction_bits)? {
            let width = significand.bit_width().max(fraction_bits.checked_add(1)?);
            let hidden = ApInt::one_bit_set(width, fraction_bits);
            (1_u64, 0_u128, hidden.try_zext_u128()?)
        } else {
            let fraction = significand.try_zext_u128()?;
            (0_u64, fraction, fraction)
        };
    let bits = pack_decimal_bits(semantics, negative, exponent, fraction, significand_bits)?;
    Some((ApFloat::from_bits_unchecked(semantics, &bits), status))
}

fn scale_rational_by_power_of_two(
    numerator: ApInt,
    denominator: ApInt,
    shift: i32,
) -> Option<(ApInt, ApInt)> {
    if shift >= 0 {
        Some((
            apint_shl_ext(&numerator, u32::try_from(shift).ok()?)?,
            denominator,
        ))
    } else {
        Some((
            numerator,
            apint_shl_ext(&denominator, shift.unsigned_abs())?,
        ))
    }
}

fn apint_shl_ext(value: &ApInt, amount: u32) -> Option<ApInt> {
    let width = value
        .active_bits()
        .checked_add(amount)?
        .checked_add(1)?
        .max(1);
    let extended = value.zext_or_trunc(width);
    if amount == 0 {
        Some(extended)
    } else {
        extended.checked_shl(amount)
    }
}

fn rational_binary_exponent_wide(numerator: &ApInt, denominator: &ApInt) -> Option<i32> {
    let num_bits = i32::try_from(numerator.active_bits()).ok()?;
    let den_bits = i32::try_from(denominator.active_bits()).ok()?;
    let mut exponent = num_bits.checked_sub(den_bits)?;
    if !rational_ge_power_of_two_wide(numerator, denominator, exponent)? {
        exponent = exponent.checked_sub(1)?;
    }
    while rational_ge_power_of_two_wide(numerator, denominator, exponent.checked_add(1)?)? {
        exponent = exponent.checked_add(1)?;
    }
    Some(exponent)
}

fn rational_ge_power_of_two_wide(
    numerator: &ApInt,
    denominator: &ApInt,
    exponent: i32,
) -> Option<bool> {
    if exponent >= 0 {
        let shifted_denominator = apint_shl_ext(denominator, u32::try_from(exponent).ok()?)?;
        apint_uge_wide(numerator, &shifted_denominator)
    } else {
        let shifted_numerator = apint_shl_ext(numerator, exponent.unsigned_abs())?;
        apint_uge_wide(&shifted_numerator, denominator)
    }
}

fn apint_uge_wide(lhs: &ApInt, rhs: &ApInt) -> Option<bool> {
    let width = lhs
        .active_bits()
        .max(rhs.active_bits())
        .checked_add(1)?
        .max(1);
    Some(lhs.zext_or_trunc(width).uge(&rhs.zext_or_trunc(width)))
}

fn round_apint_div_wide(
    numerator: ApInt,
    denominator: ApInt,
    rounding: RoundingMode,
    negative: bool,
) -> Option<(ApInt, bool)> {
    if denominator.is_zero() {
        return None;
    }
    let width = numerator
        .active_bits()
        .max(denominator.active_bits())
        .checked_add(2)?
        .max(2);
    let numerator = numerator.zext_or_trunc(width);
    let denominator = denominator.zext_or_trunc(width);
    let divrem = numerator.udivrem(&denominator)?;
    let cmp_width = width.checked_add(1)?;
    let mut quotient = divrem.quotient().zext_or_trunc(cmp_width);
    let remainder = divrem.remainder();
    if remainder.is_zero() {
        return Some((quotient, false));
    }
    let twice_remainder = remainder.zext_or_trunc(cmp_width).checked_shl(1)?;
    let denominator = denominator.zext_or_trunc(cmp_width);
    let increment = match rounding {
        RoundingMode::NearestTiesToEven => {
            twice_remainder.ugt(&denominator)
                || (twice_remainder.eq_ap_int(&denominator) && quotient.is_one_bit_set(0))
        }
        RoundingMode::NearestTiesToAway => twice_remainder.uge(&denominator),
        RoundingMode::TowardZero => false,
        RoundingMode::TowardPositive => !negative,
        RoundingMode::TowardNegative => negative,
    };
    if increment {
        quotient = quotient.wrapping_add(&ApInt::from_words(cmp_width, &[1]));
    }
    Some((quotient, true))
}

fn apint_ge_power_of_two(value: &ApInt, bit: u32) -> Option<bool> {
    let width = value.bit_width().max(bit.checked_add(1)?);
    Some(
        value
            .zext_or_trunc(width)
            .uge(&ApInt::one_bit_set(width, bit)),
    )
}

fn max_exponent_field(semantics: ApFloatSemantics) -> Option<i32> {
    1i32.checked_shl(exponent_bits(semantics)?)?.checked_sub(1)
}

fn overflow_result(
    semantics: ApFloatSemantics,
    negative: bool,
    rounding: RoundingMode,
) -> (ApFloat, ApFloatStatus) {
    let rounds_to_infinity = match rounding {
        RoundingMode::NearestTiesToEven | RoundingMode::NearestTiesToAway => true,
        RoundingMode::TowardZero => false,
        RoundingMode::TowardPositive => !negative,
        RoundingMode::TowardNegative => negative,
    };
    let value = if rounds_to_infinity {
        ApFloat::inf(semantics, sign_from_negative(negative))
    } else {
        ApFloat::largest(semantics, sign_from_negative(negative))
    };
    let mut status = ApFloatStatus::OVERFLOW;
    status.insert(ApFloatStatus::INEXACT);
    (value, status)
}

fn status_from_inexact(inexact: bool) -> ApFloatStatus {
    let mut status = ApFloatStatus::OK;
    if inexact {
        status.insert(ApFloatStatus::INEXACT);
    }
    status
}

fn round_power_of_two_div(
    value: &ApInt,
    shift: u32,
    rounding: RoundingMode,
    negative: bool,
) -> (ApInt, Exactness) {
    let mut quotient = if shift == 0 {
        value.clone()
    } else {
        value
            .checked_lshr(shift)
            .unwrap_or_else(|| ApInt::zero(value.bit_width()))
    };
    if !any_low_bit_set(value, shift) {
        return (quotient, Exactness::Exact);
    }
    let half_cmp = low_bits_cmp_half(value, shift);
    let increment = match rounding {
        RoundingMode::NearestTiesToEven => {
            matches!(half_cmp, Ordering::Greater)
                || (matches!(half_cmp, Ordering::Equal) && quotient.is_one_bit_set(0))
        }
        RoundingMode::NearestTiesToAway => !matches!(half_cmp, Ordering::Less),
        RoundingMode::TowardZero => false,
        RoundingMode::TowardPositive => !negative,
        RoundingMode::TowardNegative => negative,
    };
    if increment {
        quotient = quotient.wrapping_add(&ApInt::from_words(quotient.bit_width(), &[1]));
    }
    (quotient, Exactness::Inexact)
}

fn integer_magnitude_out_of_range(
    magnitude: &ApInt,
    width: u32,
    signedness: ApIntSignedness,
    negative: bool,
) -> bool {
    if width == 0 {
        return true;
    }
    if magnitude.is_zero() {
        return false;
    }
    match signedness {
        ApIntSignedness::Unsigned => negative || magnitude.active_bits() > width,
        ApIntSignedness::Signed => {
            let Some(limit_bit) = width.checked_sub(1) else {
                return true;
            };
            let Some(compare_width) = magnitude.active_bits().max(width).checked_add(1) else {
                return true;
            };
            let magnitude = magnitude.zext_or_trunc(compare_width);
            let limit = ApInt::one_bit_set(compare_width, limit_bit);
            if negative {
                magnitude.ugt(&limit)
            } else {
                magnitude.uge(&limit)
            }
        }
    }
}

fn any_low_bit_set(value: &ApInt, count: u32) -> bool {
    let mut bit = 0u32;
    let limit = count.min(value.bit_width());
    while bit < limit {
        if value.is_one_bit_set(bit) {
            return true;
        }
        bit += 1;
    }
    false
}

fn low_bits_cmp_half(value: &ApInt, count: u32) -> Ordering {
    let Some(half_bit) = count.checked_sub(1) else {
        return Ordering::Equal;
    };
    if !value.is_one_bit_set(half_bit) {
        return Ordering::Less;
    }
    if any_low_bit_set(value, half_bit) {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

fn low_u128_from_apint(bits: &ApInt) -> u128 {
    let words = bits.words();
    let low = u128::from(words.first().copied().unwrap_or(0));
    let high = u128::from(words.get(1).copied().unwrap_or(0));
    low | (high << 64)
}

fn low_bits_mask(bits: u32) -> u128 {
    match bits {
        0 => 0,
        128.. => u128::MAX,
        bits => (1u128 << bits) - 1,
    }
}

fn apint_from_u128_with_width(width: u32, value: u128) -> ApInt {
    let low = u64::try_from(value & u128::from(u64::MAX)).unwrap_or(0);
    let high = u64::try_from(value >> 64).unwrap_or(0);
    ApInt::from_words(width, &[low, high])
}
struct DecimalParts {
    negative: bool,
    digits: String,
    exp10: i32,
}

fn decimal_to_semantic_float(
    semantics: ApFloatSemantics,
    text: &str,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let parts = parse_decimal_parts(text)?;
    if decimal_digits_are_zero(&parts.digits) {
        return Some((
            ApFloat::zero(
                semantics,
                if parts.negative {
                    ApFloatSign::Negative
                } else {
                    ApFloatSign::Positive
                },
            ),
            ApFloatStatus::OK,
        ));
    }
    if matches!(semantics, ApFloatSemantics::PpcDoubleDouble) {
        return decimal_to_ppc_double_double(&parts, rounding);
    }
    let (numerator, denominator) = decimal_rational(&parts, semantics.precision())?;
    encode_decimal_rational(semantics, parts.negative, numerator, denominator, rounding)
}

fn decimal_to_ppc_double_double(
    parts: &DecimalParts,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let (numerator, denominator) =
        decimal_rational(parts, ApFloatSemantics::PpcDoubleDouble.precision())?;
    encode_binary_rational_scaled(
        ApFloatSemantics::PpcDoubleDouble,
        parts.negative,
        numerator,
        denominator,
        0,
        rounding,
    )
}

fn parse_decimal_parts(text: &str) -> Option<DecimalParts> {
    if text.contains("0x") || text.contains("0xk") || text.contains("0xl") || text.contains("0xr") {
        return None;
    }
    let (mantissa, exp_text) = text.split_once('e').unwrap_or((text, "0"));
    let exp_from_suffix = exp_text.parse::<i32>().ok()?;
    let (negative, mantissa) = if let Some(rest) = mantissa.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = mantissa.strip_prefix('+') {
        (false, rest)
    } else {
        (false, mantissa)
    };
    let mut digits = String::new();
    let mut saw_digit = false;
    let mut fractional_digits = 0i32;
    let mut past_point = false;
    for ch in mantissa.chars() {
        if ch == '.' {
            if past_point {
                return None;
            }
            past_point = true;
            continue;
        }
        ch.to_digit(10)?;
        saw_digit = true;
        digits.push(ch);
        if past_point {
            fractional_digits = fractional_digits.checked_add(1)?;
        }
    }
    if !saw_digit {
        return None;
    }
    Some(DecimalParts {
        negative,
        digits,
        exp10: exp_from_suffix.checked_sub(fractional_digits)?,
    })
}

fn decimal_digits_are_zero(digits: &str) -> bool {
    digits.bytes().all(|b| b == b'0')
}

fn decimal_rational(parts: &DecimalParts, precision: u32) -> Option<(ApInt, ApInt)> {
    let significant = parts.digits.trim_start_matches('0');
    let significant = if significant.is_empty() {
        "0"
    } else {
        significant
    };
    let mut numerator_text = significant.to_owned();
    let mut denominator_text = String::from("1");
    if parts.exp10 >= 0 {
        append_decimal_zeros(&mut numerator_text, u32::try_from(parts.exp10).ok()?)?;
    } else {
        append_decimal_zeros(&mut denominator_text, parts.exp10.unsigned_abs())?;
    }
    let digits = numerator_text.len().max(denominator_text.len());
    let width = decimal_work_width(digits, precision)?;
    let numerator = ApInt::from_string(width, &numerator_text, 10).ok()?;
    let denominator = ApInt::from_string(width, &denominator_text, 10).ok()?;
    Some((numerator, denominator))
}

fn append_decimal_zeros(text: &mut String, count: u32) -> Option<()> {
    let count = usize::try_from(count).ok()?;
    text.try_reserve(count).ok()?;
    for _ in 0..count {
        text.push('0');
    }
    Some(())
}

fn decimal_work_width(decimal_digits: usize, precision: u32) -> Option<u32> {
    let digits = u32::try_from(decimal_digits).ok()?;
    digits
        .checked_mul(4)?
        .checked_add(precision)?
        .checked_add(16)
}

fn encode_decimal_rational(
    semantics: ApFloatSemantics,
    negative: bool,
    numerator: ApInt,
    denominator: ApInt,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let bias = exponent_bias(semantics)?;
    let precision = semantics.precision();
    let mut exponent = rational_binary_exponent(&numerator, &denominator)?;
    let max_exponent = (1i32.checked_shl(exponent_bits(semantics)?)?).checked_sub(1)?;
    let exponent_field = exponent.checked_add(bias)?;
    if exponent_field <= 0 {
        return encode_decimal_subnormal(semantics, negative, numerator, denominator, rounding);
    }
    if exponent_field >= max_exponent {
        return Some(overflow_result(semantics, negative, rounding));
    }
    let shift = i32::try_from(precision.checked_sub(1)?)
        .ok()?
        .checked_sub(exponent)?;
    let mut scaled_numerator = numerator.clone();
    let mut scaled_denominator = denominator.clone();
    if shift >= 0 {
        scaled_numerator = apint_shl(&scaled_numerator, u32::try_from(shift).ok()?);
    } else {
        scaled_denominator = apint_shl(&scaled_denominator, shift.unsigned_abs());
    }
    let (mut significand, inexact) =
        round_apint_div(scaled_numerator, scaled_denominator, rounding, negative)?;
    if significand.uge(&ApInt::one_bit_set(significand.bit_width(), precision)) {
        significand = significand.checked_lshr(1)?;
        exponent = exponent.checked_add(1)?;
    }
    let exponent_field = exponent.checked_add(bias)?;
    if exponent_field >= max_exponent {
        return Some(overflow_result(semantics, negative, rounding));
    }
    let hidden = ApInt::one_bit_set(significand.bit_width(), precision.checked_sub(1)?);
    let fraction = significand.wrapping_sub(&hidden).try_zext_u128()?;
    let bits = pack_decimal_bits(
        semantics,
        negative,
        u64::try_from(exponent_field).ok()?,
        fraction,
        significand.try_zext_u128()?,
    )?;
    Some((
        ApFloat::from_bits_unchecked(semantics, &bits),
        status_from_inexact(inexact),
    ))
}

fn encode_decimal_subnormal(
    semantics: ApFloatSemantics,
    negative: bool,
    numerator: ApInt,
    denominator: ApInt,
    rounding: RoundingMode,
) -> Option<(ApFloat, ApFloatStatus)> {
    let bias = exponent_bias(semantics)?;
    let precision = semantics.precision();
    let fraction_bits = precision.checked_sub(1)?;
    let unit_exponent = 1i32
        .checked_sub(bias)?
        .checked_sub(i32::try_from(fraction_bits).ok()?)?;
    let mut scaled_numerator = numerator;
    let mut scaled_denominator = denominator;
    if unit_exponent <= 0 {
        scaled_numerator = apint_shl(&scaled_numerator, unit_exponent.unsigned_abs());
    } else {
        scaled_denominator = apint_shl(&scaled_denominator, u32::try_from(unit_exponent).ok()?);
    }
    let (significand, inexact) =
        round_apint_div(scaled_numerator, scaled_denominator, rounding, negative)?;
    let hidden = ApInt::one_bit_set(significand.bit_width(), fraction_bits);
    let (exponent, fraction, significand_bits) = if significand.uge(&hidden) {
        (1_u64, 0_u128, hidden.try_zext_u128()?)
    } else {
        let fraction = significand.try_zext_u128()?;
        (0_u64, fraction, fraction)
    };
    let bits = pack_decimal_bits(semantics, negative, exponent, fraction, significand_bits)?;
    Some((
        ApFloat::from_bits_unchecked(semantics, &bits),
        status_from_inexact(inexact),
    ))
}

fn exponent_bias(semantics: ApFloatSemantics) -> Option<i32> {
    match semantics {
        ApFloatSemantics::IeeeHalf => Some(15),
        ApFloatSemantics::BFloat | ApFloatSemantics::IeeeSingle => Some(127),
        ApFloatSemantics::IeeeDouble => Some(1023),
        ApFloatSemantics::IeeeQuad | ApFloatSemantics::X87DoubleExtended => Some(16383),
        ApFloatSemantics::PpcDoubleDouble => None,
    }
}

fn exponent_bits(semantics: ApFloatSemantics) -> Option<u32> {
    match semantics {
        ApFloatSemantics::IeeeHalf => Some(5),
        ApFloatSemantics::BFloat | ApFloatSemantics::IeeeSingle => Some(8),
        ApFloatSemantics::IeeeDouble => Some(11),
        ApFloatSemantics::IeeeQuad | ApFloatSemantics::X87DoubleExtended => Some(15),
        ApFloatSemantics::PpcDoubleDouble => None,
    }
}

fn rational_binary_exponent(numerator: &ApInt, denominator: &ApInt) -> Option<i32> {
    let num_bits = i32::try_from(numerator.active_bits()).ok()?;
    let den_bits = i32::try_from(denominator.active_bits()).ok()?;
    let mut exponent = num_bits.checked_sub(den_bits)?;
    if !rational_ge_power_of_two(numerator, denominator, exponent)? {
        exponent = exponent.checked_sub(1)?;
    }
    while rational_ge_power_of_two(numerator, denominator, exponent.checked_add(1)?)? {
        exponent = exponent.checked_add(1)?;
    }
    Some(exponent)
}

fn rational_ge_power_of_two(numerator: &ApInt, denominator: &ApInt, exponent: i32) -> Option<bool> {
    if exponent >= 0 {
        Some(numerator.uge(&apint_shl(denominator, u32::try_from(exponent).ok()?)))
    } else {
        Some(apint_shl(numerator, exponent.unsigned_abs()).uge(denominator))
    }
}

fn round_apint_div(
    numerator: ApInt,
    denominator: ApInt,
    rounding: RoundingMode,
    negative: bool,
) -> Option<(ApInt, bool)> {
    let width = numerator.bit_width();
    let divrem = numerator.udivrem(&denominator)?;
    let mut quotient = divrem.quotient().clone();
    let remainder = divrem.remainder();
    if remainder.is_zero() {
        return Some((quotient, false));
    }
    let twice_remainder = remainder.wrapping_add(remainder);
    let increment = match rounding {
        RoundingMode::NearestTiesToEven => {
            twice_remainder.ugt(&denominator)
                || (twice_remainder.eq_ap_int(&denominator) && quotient.is_one_bit_set(0))
        }
        RoundingMode::NearestTiesToAway => twice_remainder.uge(&denominator),
        RoundingMode::TowardZero => false,
        RoundingMode::TowardPositive => !negative,
        RoundingMode::TowardNegative => negative,
    };
    if increment {
        quotient = quotient.wrapping_add(&ApInt::from_words(width, &[1]));
    }
    Some((quotient, true))
}

fn apint_shl(value: &ApInt, amount: u32) -> ApInt {
    if amount == 0 {
        return value.clone();
    }
    if amount >= value.bit_width() {
        return ApInt::zero(value.bit_width());
    }
    match value.checked_shl(amount) {
        Some(shifted) => shifted,
        None => ApInt::zero(value.bit_width()),
    }
}

fn pack_decimal_bits(
    semantics: ApFloatSemantics,
    negative: bool,
    exponent: u64,
    fraction: u128,
    significand: u128,
) -> Option<Vec<u64>> {
    match semantics {
        ApFloatSemantics::IeeeHalf => {
            let sign = if negative { 1u64 << 15 } else { 0 };
            Some(vec![
                sign | (exponent << 10) | u64::try_from(fraction).ok()?,
            ])
        }
        ApFloatSemantics::BFloat => {
            let sign = if negative { 1u64 << 15 } else { 0 };
            Some(vec![sign | (exponent << 7) | u64::try_from(fraction).ok()?])
        }
        ApFloatSemantics::IeeeSingle => {
            let sign = if negative { 1u64 << 31 } else { 0 };
            Some(vec![
                sign | (exponent << 23) | u64::try_from(fraction).ok()?,
            ])
        }
        ApFloatSemantics::IeeeDouble => {
            let sign = if negative { 1u64 << 63 } else { 0 };
            Some(vec![
                sign | (exponent << 52) | u64::try_from(fraction).ok()?,
            ])
        }
        ApFloatSemantics::IeeeQuad => {
            let sign = if negative { 0x8000_0000_0000_0000 } else { 0 };
            let low = u64::try_from(fraction & u128::from(u64::MAX)).ok()?;
            let high_fraction = u64::try_from(fraction >> 64).ok()?;
            Some(vec![low, sign | (exponent << 48) | high_fraction])
        }
        ApFloatSemantics::X87DoubleExtended => {
            let sign = if negative { 0x8000 } else { 0 };
            Some(vec![u64::try_from(significand).ok()?, sign | exponent])
        }
        ApFloatSemantics::PpcDoubleDouble => None,
    }
}

fn quad_to_f64(bits: &ApInt) -> f64 {
    let words = bits.words();
    let low = words.first().copied().unwrap_or(0);
    let high = words.get(1).copied().unwrap_or(0);
    let sign = high & 0x8000_0000_0000_0000;
    let exp = (high >> 48) & 0x7fff;
    let frac_high = high & 0x0000_ffff_ffff_ffff;
    if exp == 0 {
        return f64::from_bits(sign);
    }
    if exp == 0x7fff {
        return if frac_high == 0 && low == 0 {
            f64::from_bits(sign | 0x7ff0_0000_0000_0000)
        } else {
            f64::NAN
        };
    }
    let f64_exp = i32::try_from(exp).unwrap_or(0) - 16383 + 1023;
    if f64_exp <= 0 {
        return f64::from_bits(sign);
    }
    if f64_exp >= 0x7ff {
        return f64::from_bits(sign | 0x7ff0_0000_0000_0000);
    }
    let frac = (frac_high << 4) | (low >> 60);
    f64::from_bits(sign | (u64::try_from(f64_exp).unwrap_or(0) << 52) | frac)
}

fn x87_to_f64(bits: &ApInt) -> f64 {
    let words = bits.words();
    let low = words.first().copied().unwrap_or(0);
    let high = words.get(1).copied().unwrap_or(0);
    let sign = if (high & 0x8000) == 0 {
        0
    } else {
        0x8000_0000_0000_0000
    };
    let exp = high & 0x7fff;
    let frac = low & 0x7fff_ffff_ffff_ffff;
    if exp == 0 {
        return f64::from_bits(sign);
    }
    if exp == 0x7fff {
        return if frac == 0 {
            f64::from_bits(sign | 0x7ff0_0000_0000_0000)
        } else {
            f64::NAN
        };
    }
    let f64_exp = i32::try_from(exp).unwrap_or(0) - 16383 + 1023;
    if f64_exp <= 0 {
        return f64::from_bits(sign);
    }
    if f64_exp >= 0x7ff {
        return f64::from_bits(sign | 0x7ff0_0000_0000_0000);
    }
    f64::from_bits(sign | (u64::try_from(f64_exp).unwrap_or(0) << 52) | (frac >> 11))
}

fn exponent_layout(semantics: ApFloatSemantics) -> (u32, u32) {
    match semantics {
        ApFloatSemantics::IeeeHalf => (10, 5),
        ApFloatSemantics::BFloat => (7, 8),
        ApFloatSemantics::IeeeSingle => (23, 8),
        ApFloatSemantics::IeeeDouble => (52, 11),
        ApFloatSemantics::IeeeQuad => (112, 15),
        ApFloatSemantics::X87DoubleExtended => (64, 15),
        ApFloatSemantics::PpcDoubleDouble => (116, 11),
    }
}

fn fraction_bits(semantics: ApFloatSemantics) -> u32 {
    match semantics {
        ApFloatSemantics::IeeeHalf => 10,
        ApFloatSemantics::BFloat => 7,
        ApFloatSemantics::IeeeSingle => 23,
        ApFloatSemantics::IeeeDouble => 52,
        ApFloatSemantics::IeeeQuad => 112,
        ApFloatSemantics::X87DoubleExtended => 63,
        ApFloatSemantics::PpcDoubleDouble => 116,
    }
}

fn quiet_nan_bit(semantics: ApFloatSemantics) -> u32 {
    match semantics {
        ApFloatSemantics::IeeeHalf => 9,
        ApFloatSemantics::BFloat => 6,
        ApFloatSemantics::IeeeSingle => 22,
        ApFloatSemantics::IeeeDouble => 51,
        ApFloatSemantics::IeeeQuad => 111,
        ApFloatSemantics::X87DoubleExtended => 62,
        ApFloatSemantics::PpcDoubleDouble => 115,
    }
}

fn extract_bits_u64(bits: &ApInt, lo: u32, count: u32) -> u64 {
    let mut out = 0u64;
    let mut i = 0u32;
    while i < count && i < 64 {
        if bits.is_one_bit_set(lo + i) {
            out |= 1u64 << i;
        }
        i += 1;
    }
    out
}

fn has_fraction_bits(bits: &ApInt, count: u32) -> bool {
    let mut i = 0u32;
    while i < count {
        if bits.is_one_bit_set(i) {
            return true;
        }
        i += 1;
    }
    false
}

fn half_to_f64(raw: u16) -> f64 {
    let sign = if (raw & 0x8000) != 0 { -1.0 } else { 1.0 };
    let exp = (raw >> 10) & 0x1f;
    let frac = raw & 0x03ff;
    if exp == 0 {
        if frac == 0 {
            sign * 0.0
        } else {
            sign * f64::from(frac) * f64::from(2).powi(-24)
        }
    } else if exp == 0x1f {
        if frac == 0 {
            sign * f64::INFINITY
        } else {
            f64::NAN
        }
    } else {
        sign * (1.0 + f64::from(frac) / 1024.0) * f64::from(2).powi(i32::from(exp) - 15)
    }
}
