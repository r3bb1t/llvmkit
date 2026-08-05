//! Vector and shuffle-mask utilities.
//!
//! Ports `llvm/lib/Analysis/VectorUtils.cpp`.
//!
//! # What is not modeled, and why
//!
//! Upstream defines 37 functions here — 38 counting both
//! `widenShuffleMaskElts` overloads. **16 are absent**, each blocked on
//! something named:
//!
//! - **Eight take or return `Intrinsic::ID`** — `isTriviallyVectorizable`,
//!   `isTriviallyScalarizable`, `isVectorIntrinsicWithScalarOpAtArg`,
//!   `isVectorIntrinsicWithOverloadTypeAtArg`,
//!   `isVectorIntrinsicWithStructReturnOverloadAtField`,
//!   `getVectorIntrinsicIDForCall`, `getInterleaveIntrinsicFactor` and
//!   `getDeinterleaveIntrinsicFactor`. llvmkit has no public intrinsic-id
//!   type; the same blocker keeps `getIntrinsicForCallSite` out of the
//!   ValueTracking ledger, so closing it would unblock nine functions at once.
//! - **Four need metadata modeling** — `uniteAccessGroups`,
//!   `intersectAccessGroups`, `getMetadataToPropagate` and
//!   `propagateMetadata`.
//! - **Three need construction machinery** — `concatenateVectors` and
//!   `createBitMaskForGaps` need an `IRBuilder`, and
//!   `getDeinterleavedVectorType` needs `IntrinsicInst`.
//! - **`computeMinimumValueSizes` needs `TargetTransformInfo`.** llvmkit
//!   models no target — code generation and target backends are out of scope,
//!   not merely unfinished — so unlike the rest this one is blocked
//!   permanently rather than pending.
//!
//! Deriving that list by hand is error-prone: a `grep` anchoring the return
//! type and the `llvm::` name to one line silently misses every definition
//! whose return type wraps. Re-derive with
//! `grep -oE "\bllvm::[a-zA-Z_][a-zA-Z0-9_]*\(" … | sort -u`, discounting
//! `bit_ceil` and `bit_width`, which are calls into `ADT/bit.h` rather than
//! definitions here.

use crate::ap_int::ApInt;
use crate::instr_types::ShuffleMaskElem;

/// The lanes a `shufflevector` mask demands from each of its two sources.
///
/// Ports `llvm::getShuffleDemandedElts`, whose two out-parameters become the
/// returned pair. `None` is upstream's `false` — a poison mask element among
/// the demanded lanes when `allow_undefined_elements` is not set, or a mask
/// index past the end of both sources. Callers answer "nothing known" for it.
///
/// `source_width` is the lane count of *one* source, not their sum; both
/// operands of a `shufflevector` have the same type, and mask indices at or
/// above it select from the right-hand side.
///
/// `allow_undefined_elements` distinguishes the two questions a caller can
/// ask. Known-bits and known-fp-class analyses pass `false`: a demanded poison
/// lane means the result has no common state to describe, so the whole query
/// fails. A caller that only wants to know which source lanes are reachable
/// passes `true`, and poison lanes are simply skipped.
pub fn shuffle_demanded_elements(
    source_width: u32,
    mask: &[ShuffleMaskElem],
    demanded: &ApInt,
    allow_undefined_elements: bool,
) -> Option<(ApInt, ApInt)> {
    let mut left = ApInt::zero(source_width);
    let mut right = ApInt::zero(source_width);

    // Nothing demanded, nothing to trace back.
    if demanded.is_zero() {
        return Some((left, right));
    }

    // A shuffle with `zeroinitializer` reads lane 0 of the left source and
    // nothing else, whatever the demanded set says.
    if mask
        .iter()
        .all(|element| *element == ShuffleMaskElem::Lane(0))
    {
        left.set_bit(0);
        return Some((left, right));
    }

    for (lane, element) in mask.iter().enumerate() {
        let lane = u32::try_from(lane).ok()?;
        if !demanded.bit(lane) {
            continue;
        }
        let ShuffleMaskElem::Lane(index) = *element else {
            // For a poison element the result lane has no common state — so
            // unless the caller said it does not care, nothing is known.
            if allow_undefined_elements {
                continue;
            }
            return None;
        };
        if index < source_width {
            left.set_bit(index);
        } else {
            let right_lane = index.checked_sub(source_width)?;
            if right_lane >= source_width {
                return None;
            }
            right.set_bit(right_lane);
        }
    }
    Some((left, right))
}
