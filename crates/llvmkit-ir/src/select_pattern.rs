//! Select-pattern classification — the min/max/abs idioms a `select` can spell.
//!
//! Mirrors the `SelectPatternResult` slice of
//! `llvm/include/llvm/Analysis/ValueTracking.h` and its implementation in
//! `llvm/lib/Analysis/ValueTracking.cpp`.
//!
//! This module is the *vocabulary*: the flavours, the result record, and the
//! total functions that map between a flavour and its predicate, intrinsic,
//! inverse and saturating limit. Matching an actual `select` against these
//! flavours (`matchSelectPattern` and friends) is a separate piece of work and
//! is recorded in the parity ledger until it lands.

use crate::ap_int::ApInt;
use crate::cmp_predicate::{CmpPredicate, FloatPredicate, IntPredicate};

/// Which min/max/abs idiom a `select` implements.
///
/// Ports `llvm::SelectPatternFlavor`. `SPF_UNKNOWN` is spelled [`Self::Unknown`]
/// rather than dropped, because [`SelectPatternResult`] is the return type of a
/// classification that legitimately fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectPatternFlavor {
    /// Not a recognised pattern.
    Unknown,
    /// Signed minimum.
    SMin,
    /// Unsigned minimum.
    UMin,
    /// Signed maximum.
    SMax,
    /// Unsigned maximum.
    UMax,
    /// Floating-point `minnum`.
    FMinNum,
    /// Floating-point `maxnum`.
    FMaxNum,
    /// Absolute value.
    Abs,
    /// Negated absolute value.
    NAbs,
}

impl SelectPatternFlavor {
    /// Whether this flavour is a minimum or a maximum.
    ///
    /// Ports `SelectPatternResult::isMinOrMax`, which lives on the result
    /// upstream but reads only the flavour.
    #[inline]
    pub const fn is_min_or_max(self) -> bool {
        !matches!(self, Self::Unknown | Self::Abs | Self::NAbs)
    }

    /// The canonical comparison predicate for this minimum/maximum.
    ///
    /// Ports `llvm::getMinMaxPred`. `ordered` selects between the ordered and
    /// unordered float predicate and is ignored by the integer flavours.
    ///
    /// Upstream ends in `llvm_unreachable` for the three non-min/max flavours;
    /// here that precondition is the `None`, so a caller cannot read a
    /// predicate that was never defined.
    #[inline]
    pub const fn min_max_predicate(self, ordered: bool) -> Option<CmpPredicate> {
        Some(match self {
            Self::SMin => CmpPredicate::Int(IntPredicate::Slt),
            Self::UMin => CmpPredicate::Int(IntPredicate::Ult),
            Self::SMax => CmpPredicate::Int(IntPredicate::Sgt),
            Self::UMax => CmpPredicate::Int(IntPredicate::Ugt),
            Self::FMinNum => CmpPredicate::Float(if ordered {
                FloatPredicate::Olt
            } else {
                FloatPredicate::Ult
            }),
            Self::FMaxNum => CmpPredicate::Float(if ordered {
                FloatPredicate::Ogt
            } else {
                FloatPredicate::Ugt
            }),
            Self::Unknown | Self::Abs | Self::NAbs => return None,
        })
    }

    /// The integer min/max intrinsic equivalent to this flavour.
    ///
    /// Ports `llvm::getMinMaxIntrinsic`, whose doc says "Caller must ensure
    /// `SPF` is an integer min or max pattern" and whose `default` arm is
    /// `llvm_unreachable`. That precondition is the `None` here.
    #[inline]
    pub const fn min_max_intrinsic(self) -> Option<MinMaxIntrinsic> {
        Some(match self {
            Self::SMin => MinMaxIntrinsic::SMin,
            Self::SMax => MinMaxIntrinsic::SMax,
            Self::UMin => MinMaxIntrinsic::UMin,
            Self::UMax => MinMaxIntrinsic::UMax,
            _ => return None,
        })
    }

    /// The opposite minimum/maximum: signed minimum inverts to signed maximum,
    /// and so on.
    ///
    /// Ports `llvm::getInverseMinMaxFlavor`, which handles the four integer
    /// flavours and is `llvm_unreachable` for the rest — including the two
    /// float ones, which it does *not* cover.
    #[inline]
    pub const fn inverse_min_max(self) -> Option<Self> {
        Some(match self {
            Self::SMin => Self::SMax,
            Self::SMax => Self::SMin,
            Self::UMin => Self::UMax,
            Self::UMax => Self::UMin,
            _ => return None,
        })
    }

    /// The extreme value this minimum/maximum can produce at `bit_width`.
    ///
    /// Ports `llvm::getMinMaxLimit`, "the minimum or maximum constant value
    /// for the specified integer min/max flavor and type": a signed maximum
    /// tops out at `signed_max_value`, an unsigned minimum bottoms out at
    /// zero. Note this is the *limit*, not the identity element — the identity
    /// of `smax` is the signed **minimum**, which is the opposite end.
    #[inline]
    pub fn min_max_limit(self, bit_width: u32) -> Option<ApInt> {
        Some(match self {
            Self::SMax => ApInt::signed_max_value(bit_width),
            Self::SMin => ApInt::signed_min_value(bit_width),
            Self::UMax => ApInt::all_ones(bit_width),
            Self::UMin => ApInt::zero(bit_width),
            _ => return None,
        })
    }
}

/// The four integer min/max intrinsics.
///
/// A dedicated enum rather than llvmkit's crate-internal intrinsic semantic,
/// so the mapping can be part of the public API. It is also exactly the range
/// of `llvm::getMinMaxIntrinsic`, which makes [`Self::inverse`] total where
/// upstream's `getInverseMinMaxIntrinsic` needs an `llvm_unreachable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MinMaxIntrinsic {
    /// `llvm.smin`.
    SMin,
    /// `llvm.smax`.
    SMax,
    /// `llvm.umin`.
    UMin,
    /// `llvm.umax`.
    UMax,
}

impl MinMaxIntrinsic {
    /// The intrinsic computing the opposite extremum.
    ///
    /// Ports the integer arms of `llvm::getInverseMinMaxIntrinsic`. Upstream
    /// also inverts `maximum`/`minimum`, `maxnum`/`minnum` and
    /// `maximumnum`/`minimumnum`; llvmkit models no floating-point min/max
    /// intrinsic, so those six have nothing to map to and are recorded as a
    /// gap rather than invented here.
    #[inline]
    pub const fn inverse(self) -> Self {
        match self {
            Self::SMin => Self::SMax,
            Self::SMax => Self::SMin,
            Self::UMin => Self::UMax,
            Self::UMax => Self::UMin,
        }
    }

    /// The flavour this intrinsic implements.
    #[inline]
    pub const fn flavor(self) -> SelectPatternFlavor {
        match self {
            Self::SMin => SelectPatternFlavor::SMin,
            Self::SMax => SelectPatternFlavor::SMax,
            Self::UMin => SelectPatternFlavor::UMin,
            Self::UMax => SelectPatternFlavor::UMax,
        }
    }

    /// The intrinsic's base name, as it appears in `.ll` text.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SMin => "llvm.smin",
            Self::SMax => "llvm.smax",
            Self::UMin => "llvm.umin",
            Self::UMax => "llvm.umax",
        }
    }
}

/// What a floating-point min/max does when given one NaN and one non-NaN.
///
/// Ports `llvm::SelectPatternNaNBehavior`. Only meaningful when the flavour is
/// [`SelectPatternFlavor::FMinNum`] or [`SelectPatternFlavor::FMaxNum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectPatternNaNBehavior {
    /// NaN behaviour does not apply — upstream's `SPNB_NA`.
    NotApplicable,
    /// Given one NaN input, returns the NaN.
    ReturnsNaN,
    /// Given one NaN input, returns the non-NaN.
    ReturnsOther,
    /// May return either, or no operand can be NaN.
    ReturnsAny,
}

/// The classification of a `select`.
///
/// Ports `llvm::SelectPatternResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectPatternResult {
    /// Which idiom was recognised.
    pub flavor: SelectPatternFlavor,
    /// NaN behaviour; only applicable to the two float flavours.
    pub nan_behavior: SelectPatternNaNBehavior,
    /// Whether implementing this min/max as `fcmp; select` needs the `fcmp`
    /// to be ordered.
    pub ordered: bool,
}

impl SelectPatternResult {
    /// The "no pattern" answer, upstream's `{SPF_UNKNOWN, SPNB_NA, false}`.
    #[inline]
    pub const fn unknown() -> Self {
        Self {
            flavor: SelectPatternFlavor::Unknown,
            nan_behavior: SelectPatternNaNBehavior::NotApplicable,
            ordered: false,
        }
    }

    /// Whether the recognised flavour is a minimum or a maximum.
    #[inline]
    pub const fn is_min_or_max(self) -> bool {
        self.flavor.is_min_or_max()
    }
}

/// The pattern `X <predicate> Y ? X : Y` implements.
///
/// Ports `llvm::getSelectPattern`. `nan_behavior` and `ordered` are carried
/// through to the result for the float predicates and ignored for the integer
/// ones, exactly as upstream does.
///
/// Equality predicates select one operand regardless of order, so they are not
/// a min/max and fall into [`SelectPatternFlavor::Unknown`] — upstream's
/// `default` arm, commented "Equality".
pub fn get_select_pattern(
    predicate: CmpPredicate,
    nan_behavior: SelectPatternNaNBehavior,
    ordered: bool,
) -> SelectPatternResult {
    let integer = |flavor| SelectPatternResult {
        flavor,
        nan_behavior: SelectPatternNaNBehavior::NotApplicable,
        ordered: false,
    };
    match predicate {
        CmpPredicate::Int(IntPredicate::Ugt | IntPredicate::Uge) => {
            integer(SelectPatternFlavor::UMax)
        }
        CmpPredicate::Int(IntPredicate::Sgt | IntPredicate::Sge) => {
            integer(SelectPatternFlavor::SMax)
        }
        CmpPredicate::Int(IntPredicate::Ult | IntPredicate::Ule) => {
            integer(SelectPatternFlavor::UMin)
        }
        CmpPredicate::Int(IntPredicate::Slt | IntPredicate::Sle) => {
            integer(SelectPatternFlavor::SMin)
        }
        CmpPredicate::Float(
            FloatPredicate::Ugt | FloatPredicate::Uge | FloatPredicate::Ogt | FloatPredicate::Oge,
        ) => SelectPatternResult {
            flavor: SelectPatternFlavor::FMaxNum,
            nan_behavior,
            ordered,
        },
        CmpPredicate::Float(
            FloatPredicate::Ult | FloatPredicate::Ule | FloatPredicate::Olt | FloatPredicate::Ole,
        ) => SelectPatternResult {
            flavor: SelectPatternFlavor::FMinNum,
            nan_behavior,
            ordered,
        },
        _ => SelectPatternResult::unknown(),
    }
}
