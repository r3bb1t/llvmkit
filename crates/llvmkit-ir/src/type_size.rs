//! A type's size, which may be a multiple of `vscale`. Mirrors `TypeSize`
//! (`llvm/Support/TypeSize.h`).

/// A size in bits or bytes: an exact quantity, or a known minimum that the
/// target multiplies by `vscale` at run time. Mirrors `llvm::TypeSize`.
///
/// Equality compares both halves, as upstream's `operator==` does, so a fixed
/// 64 and a scalable 64 differ. There is deliberately no ordering: upstream's
/// `TEST(VectorTypesTest, CrossComparisons)` says one across fixed and scalable
/// is not defined, and its `operator<` exists only through a conversion that
/// reports a fatal error on a scalable size.
///
/// ```
/// use llvmkit_ir::TypeSize;
///
/// let fixed = TypeSize::fixed(64);
/// let scalable = TypeSize::scalable(64);
/// assert_eq!(fixed.known_min_value(), scalable.known_min_value());
/// assert_ne!(fixed, scalable);
/// assert_eq!(fixed.fixed_value(), Some(64));
/// assert_eq!(scalable.fixed_value(), None);
/// assert!(scalable.is_scalable());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeSize {
    quantity: u64,
    scalable: bool,
}

impl TypeSize {
    /// Mirrors `TypeSize::getFixed`: exactly `exact`.
    #[inline]
    pub const fn fixed(exact: u64) -> Self {
        Self {
            quantity: exact,
            scalable: false,
        }
    }

    /// Mirrors `TypeSize::getScalable`: `minimum` times `vscale`.
    #[inline]
    pub const fn scalable(minimum: u64) -> Self {
        Self {
            quantity: minimum,
            scalable: true,
        }
    }

    /// Mirrors `TypeSize::get`: the two-part form the size routines build
    /// their answer with.
    #[inline]
    pub(crate) const fn new(quantity: u64, scalable: bool) -> Self {
        Self { quantity, scalable }
    }

    /// Mirrors `getKnownMinValue`: the quantity, whichever kind this is.
    #[inline]
    pub const fn known_min_value(self) -> u64 {
        self.quantity
    }

    /// Mirrors `isScalable`.
    #[inline]
    pub const fn is_scalable(self) -> bool {
        self.scalable
    }

    /// Mirrors `getFixedValue`, whose `assert` on a scalable size is `None`.
    #[inline]
    pub const fn fixed_value(self) -> Option<u64> {
        if self.scalable {
            None
        } else {
            Some(self.quantity)
        }
    }

    /// Mirrors `isKnownGE`: whether `lhs >= rhs` holds at every `vscale`. A
    /// scalable side's size is at least its known minimum, so a fixed `lhs` is
    /// never known to reach a scalable `rhs`.
    #[inline]
    pub(crate) const fn is_known_ge(lhs: Self, rhs: Self) -> bool {
        if lhs.scalable || !rhs.scalable {
            return lhs.quantity >= rhs.quantity;
        }
        false
    }

    /// `operator*` by a count, saturating as llvmkit's size products always
    /// have where upstream's wraps.
    #[inline]
    pub(crate) const fn saturating_scaled(self, factor: u64) -> Self {
        Self::new(self.quantity.saturating_mul(factor), self.scalable)
    }

    /// `operator+=`, saturating as [`saturating_scaled`](Self::saturating_scaled)
    /// does: the sum takes the scalability of `other` unless `other` is zero.
    #[inline]
    pub(crate) const fn saturating_add(self, other: Self) -> Self {
        Self::new(
            self.quantity.saturating_add(other.quantity),
            if other.quantity == 0 {
                self.scalable
            } else {
                other.scalable
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::TypeSize;

    /// Mirrors the `static_assert`s of `unittests/Support/TypeSizeTest.cpp` on
    /// `getKnownMinValue` and `operator==`, with `getZero` spelled `fixed(0)`.
    /// `fixed_value` answers `None` where `getFixedValue` asserts.
    #[test]
    fn equality_compares_both_halves() {
        assert_eq!(TypeSize::scalable(32).known_min_value(), 32);
        assert_ne!(TypeSize::fixed(0), TypeSize::scalable(0));
        assert_eq!(TypeSize::fixed(8).fixed_value(), Some(8));
        assert_eq!(TypeSize::scalable(8).fixed_value(), None);
    }

    /// Mirrors the `static_assert`s of `unittests/Support/TypeSizeTest.cpp` on
    /// `operator+`, which pin that a zero side of either kind takes the other
    /// side's scalability, and the one on `operator*`.
    #[test]
    fn a_sum_takes_the_scalability_of_a_non_zero_right_side() {
        assert_eq!(
            TypeSize::fixed(0).saturating_add(TypeSize::scalable(8)),
            TypeSize::scalable(8)
        );
        assert_eq!(
            TypeSize::scalable(8).saturating_add(TypeSize::fixed(0)),
            TypeSize::scalable(8)
        );
        assert_eq!(
            TypeSize::fixed(8).saturating_add(TypeSize::scalable(0)),
            TypeSize::fixed(8)
        );
        assert_eq!(
            TypeSize::scalable(0).saturating_add(TypeSize::fixed(8)),
            TypeSize::fixed(8)
        );
        assert_eq!(
            TypeSize::fixed(32).saturating_scaled(2),
            TypeSize::fixed(64)
        );
    }

    /// The fixed pair mirrors `unittests/Support/TypeSizeTest.cpp`'s
    /// `static_assert` on `isKnownGE`, which it asks of `ElementCount`, a sibling
    /// of the same `FixedOrScalableQuantity` routine. The scalable cases have **no
    /// upstream counterpart** and are that routine read directly: a fixed side
    /// is never known to reach a scalable one, because `vscale` can be large.
    #[test]
    fn a_fixed_size_is_never_known_to_reach_a_scalable_one() {
        assert!(TypeSize::is_known_ge(
            TypeSize::fixed(4),
            TypeSize::fixed(4)
        ));
        assert!(!TypeSize::is_known_ge(
            TypeSize::fixed(3),
            TypeSize::fixed(4)
        ));
        assert!(TypeSize::is_known_ge(
            TypeSize::scalable(4),
            TypeSize::scalable(4)
        ));
        assert!(!TypeSize::is_known_ge(
            TypeSize::scalable(3),
            TypeSize::scalable(4)
        ));
        assert!(TypeSize::is_known_ge(
            TypeSize::scalable(4),
            TypeSize::fixed(4)
        ));
        assert!(!TypeSize::is_known_ge(
            TypeSize::fixed(64),
            TypeSize::scalable(4)
        ));
    }
}
