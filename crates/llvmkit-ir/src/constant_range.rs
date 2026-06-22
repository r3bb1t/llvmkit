//! Half-open integer ranges. Mirrors `llvm/include/llvm/IR/ConstantRange.h`.

use crate::ApInt;
use crate::constant::ConstantData;
use crate::error::{IrError, IrResult};
use crate::metadata::{MetadataId, MetadataKind, MetadataStore};
use crate::module::ModuleCore;
use crate::r#type::{TypeData, TypeId};
use crate::value::{ValueId, ValueKindData};

/// Half-open range `[lower, upper)` over a fixed-width integer domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstantRange {
    lower: ApInt,
    upper: ApInt,
}

impl ConstantRange {
    pub fn new(lower: ApInt, upper: ApInt) -> IrResult<Self> {
        if lower.bit_width() != upper.bit_width() {
            return Err(IrError::OperandWidthMismatch {
                lhs: lower.bit_width(),
                rhs: upper.bit_width(),
            });
        }
        Ok(Self { lower, upper })
    }

    #[inline]
    pub fn full(bit_width: u32) -> Self {
        let max = ApInt::max_value(bit_width);
        Self {
            lower: max.clone(),
            upper: max,
        }
    }

    #[inline]
    pub fn empty(bit_width: u32) -> Self {
        Self {
            lower: ApInt::zero(bit_width),
            upper: ApInt::zero(bit_width),
        }
    }

    #[inline]
    pub fn bit_width(&self) -> u32 {
        self.lower.bit_width()
    }

    #[inline]
    pub fn lower(&self) -> &ApInt {
        &self.lower
    }

    #[inline]
    pub fn upper(&self) -> &ApInt {
        &self.upper
    }

    #[inline]
    pub fn is_full_set(&self) -> bool {
        self.lower.eq_ap_int(&self.upper) && self.lower.is_max_value()
    }

    #[inline]
    pub fn is_empty_set(&self) -> bool {
        self.lower.eq_ap_int(&self.upper) && self.lower.is_min_value()
    }

    #[inline]
    pub fn is_wrapped_set(&self) -> bool {
        !self.is_full_set()
            && !self.is_empty_set()
            && self.lower.ugt(&self.upper)
            && !self.upper.is_min_value()
    }

    #[inline]
    pub fn is_upper_wrapped(&self) -> bool {
        !self.is_full_set() && !self.is_empty_set() && self.lower.ugt(&self.upper)
    }

    pub fn contains(&self, value: &ApInt) -> bool {
        if value.bit_width() != self.bit_width() || self.is_empty_set() {
            return false;
        }
        if self.is_full_set() {
            return true;
        }
        if self.is_upper_wrapped() {
            value.uge(&self.lower) || value.ult(&self.upper)
        } else {
            value.uge(&self.lower) && value.ult(&self.upper)
        }
    }

    /// Smallest unsigned value contained by this range.
    pub fn unsigned_min(&self) -> ApInt {
        if self.is_empty_set() || self.is_full_set() || self.is_wrapped_set() {
            ApInt::zero(self.bit_width())
        } else {
            self.lower.clone()
        }
    }

    /// Largest unsigned value contained by this range.
    pub fn unsigned_max(&self) -> ApInt {
        if self.is_empty_set() || self.is_full_set() || self.is_upper_wrapped() {
            ApInt::max_value(self.bit_width())
        } else {
            self.upper.wrapping_sub(&one(self.bit_width()))
        }
    }

    pub fn intersects_with(&self, rhs: &Self) -> bool {
        if self.bit_width() != rhs.bit_width() {
            return false;
        }
        for (lhs_lo, lhs_hi) in self.segments() {
            for (rhs_lo, rhs_hi) in rhs.segments() {
                if lhs_lo.ule(&rhs_hi) && rhs_lo.ule(&lhs_hi) {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_contiguous_with(&self, rhs: &Self) -> bool {
        if self.bit_width() != rhs.bit_width() {
            return false;
        }
        for (lhs_lo, lhs_hi) in self.segments() {
            for (rhs_lo, rhs_hi) in rhs.segments() {
                if next_value(&lhs_hi).eq_ap_int(&rhs_lo) || next_value(&rhs_hi).eq_ap_int(&lhs_lo)
                {
                    return true;
                }
            }
        }
        false
    }

    fn segments(&self) -> Vec<(ApInt, ApInt)> {
        let bit_width = self.bit_width();
        if self.is_empty_set() {
            return Vec::new();
        }
        if self.is_full_set() {
            return vec![(ApInt::zero(bit_width), ApInt::max_value(bit_width))];
        }
        if self.is_upper_wrapped() {
            let mut segments = vec![(self.lower.clone(), ApInt::max_value(bit_width))];
            if !self.upper.is_min_value() {
                segments.push((
                    ApInt::zero(bit_width),
                    self.upper.wrapping_sub(&one(bit_width)),
                ));
            }
            return segments;
        }
        vec![(self.lower.clone(), self.upper.wrapping_sub(&one(bit_width)))]
    }
}

pub(crate) fn constant_ranges_from_metadata(
    module: &ModuleCore,
    store: &MetadataStore,
    id: MetadataId,
    expected_scalar_ty: TypeId,
) -> Option<Vec<ConstantRange>> {
    let MetadataKind::Tuple { operands, .. } = store.get(id)? else {
        return None;
    };
    if operands.is_empty() || operands.len() % 2 != 0 {
        return None;
    }
    let mut ranges = Vec::with_capacity(operands.len() / 2);
    for pair in operands.chunks_exact(2) {
        let (low_ty, low) = metadata_constant_int(module, store, pair[0].0)?;
        let (high_ty, high) = metadata_constant_int(module, store, pair[1].0)?;
        if low_ty != high_ty || high_ty != expected_scalar_ty {
            return None;
        }
        let range = ConstantRange::new(low, high).ok()?;
        if range.is_empty_set() || range.is_full_set() {
            return None;
        }
        ranges.push(range);
    }
    Some(ranges)
}

pub(crate) fn metadata_constant_int(
    module: &ModuleCore,
    store: &MetadataStore,
    id: MetadataId,
) -> Option<(TypeId, ApInt)> {
    let MetadataKind::Constant(value_id) = store.get(id)? else {
        return None;
    };
    constant_int_from_value(module, *value_id)
}

fn constant_int_from_value(module: &ModuleCore, id: ValueId) -> Option<(TypeId, ApInt)> {
    let data = module.context().value_data(id);
    let ValueKindData::Constant(ConstantData::Int(words)) = &data.kind else {
        return None;
    };
    let TypeData::Integer { bits } = module.context().type_data(data.ty) else {
        return None;
    };
    Some((data.ty, ApInt::from_words(*bits, words)))
}

fn one(bit_width: u32) -> ApInt {
    ApInt::from_words(bit_width, &[1])
}

fn next_value(value: &ApInt) -> ApInt {
    value.wrapping_add(&one(value.bit_width()))
}
