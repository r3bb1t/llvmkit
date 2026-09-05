//! The shuffle-mask transforms of `llvm/lib/Analysis/VectorUtils.cpp`.
//!
//! Ports `TEST_F(BasicTest, narrowShuffleMaskElts)`,
//! `TEST_F(BasicTest, widenShuffleMaskElts)` and
//! `TEST_F(BasicTest, getShuffleMaskWithWidestElts)` from
//! `llvm/unittests/Analysis/VectorUtilsTest.cpp`, mask literals and expected
//! results unchanged, with upstream's own comments kept on the cases they
//! explain.
//!
//! # Three upstream assertions have no llvmkit spelling
//!
//! Upstream's fixtures include masks holding `-2` and `-3`:
//!
//! ```text
//! narrowShuffleMaskElts(1, {3,2,0,-2})     == {3,2,0,-2}
//! widenShuffleMaskElts(2, {-1,-2,-1,-1})   == false
//! widenShuffleMaskElts(2, {-2,-2,-3,-3})   == {-2,-3}   // and the same two
//!                                                       // through
//!                                                       // getShuffleMaskWithWidestElts
//! ```
//!
//! Those exercise a rule llvmkit cannot reach: upstream stores mask elements
//! as raw `int` and requires negatives to be *equal* across a widened group,
//! because SelectionDAG and the X86 backend extend the alphabet past `-1`
//! (`SM_SentinelZero` is `-2`, and `X86ISelLowering.cpp` compares against both).
//! llvmkit's `ShuffleMaskElem` models the IR mask alphabet, `{lane, poison}`,
//! and its `shufflevector` validation rejects any negative but `undef`/`poison`
//! — so `{-2,-2,-3,-3}` is not a mask llvmkit can hold.
//!
//! This is **not a deferred gap**. Code generation and target backends are out
//! of scope for llvmkit, permanently, so the extended alphabet has no llvmkit
//! meaning to preserve. On the alphabet that does exist, "all poison" and "all
//! equal" are the same predicate, and every remaining fixture below pins that.
//! The `-2` case of `narrowShuffleMaskElts` is covered by its `scale == 1`
//! sibling, which is the behaviour that case was demonstrating.

use llvmkit_ir::{
    ApInt, DemandedOperandElements, ShuffleMaskElem,
    horizontal_demanded_elements_for_first_operand, narrow_shuffle_mask_elements,
    shuffle_demanded_elements, shuffle_mask_with_widest_elements, widen_shuffle_mask_elements,
};

/// Upstream's mask literals are bare `int`s, negative meaning undefined.
fn mask(elements: &[i64]) -> Vec<ShuffleMaskElem> {
    elements
        .iter()
        .map(|element| ShuffleMaskElem::from_encoded(*element))
        .collect()
}

/// Upstream's `APInt(width, value)`.
fn bits(width: u32, value: u64) -> ApInt {
    ApInt::from_words(width, &[value])
}

/// Upstream's `LHS.getZExtValue()` / `RHS.getZExtValue()` pair.
fn demanded(
    source_width: u32,
    elements: &[i64],
    demanded: &ApInt,
    allow_undef: bool,
) -> (u64, u64) {
    let DemandedOperandElements {
        lhs: left,
        rhs: right,
    } = shuffle_demanded_elements(source_width, &mask(elements), demanded, allow_undef)
        .expect("the mask is valid for this demanded set");
    (left.limited_value(u64::MAX), right.limited_value(u64::MAX))
}

/// Ports `TEST_F(BasicTest, narrowShuffleMaskElts)`.
///
/// Upstream's first assertion uses `-2` to show that `scale == 1` copies the
/// mask verbatim, sentinel and all; `-1` shows the same thing on the alphabet
/// llvmkit has.
#[test]
fn narrow_shuffle_mask_elts() {
    assert_eq!(
        narrow_shuffle_mask_elements(1, &mask(&[3, 2, 0, -1])),
        Some(mask(&[3, 2, 0, -1]))
    );
    assert_eq!(
        narrow_shuffle_mask_elements(4, &mask(&[3, 2, 0, -1])),
        Some(mask(&[
            12, 13, 14, 15, 8, 9, 10, 11, 0, 1, 2, 3, -1, -1, -1, -1
        ]))
    );
}

/// Ports `TEST_F(BasicTest, widenShuffleMaskElts)`, including the round trips
/// back through `narrowShuffleMaskElts` that upstream interleaves.
#[test]
fn widen_shuffle_mask_elts() {
    // scale == 1 is a copy
    assert_eq!(
        widen_shuffle_mask_elements(1, &mask(&[3, 2, 0, -1])),
        Some(mask(&[3, 2, 0, -1]))
    );

    // back to original mask
    assert_eq!(
        narrow_shuffle_mask_elements(1, &mask(&[3, 2, 0, -1])),
        Some(mask(&[3, 2, 0, -1]))
    );

    // can't widen non-consecutive 3/2
    assert_eq!(widen_shuffle_mask_elements(2, &mask(&[3, 2, 0, -1])), None);

    // can't widen if not evenly divisible
    assert_eq!(widen_shuffle_mask_elements(2, &mask(&[0, 1, 2])), None);

    // can always widen identity to single element
    assert_eq!(
        widen_shuffle_mask_elements(3, &mask(&[0, 1, 2])),
        Some(mask(&[0]))
    );

    // back to original mask
    assert_eq!(
        narrow_shuffle_mask_elements(3, &mask(&[0])),
        Some(mask(&[0, 1, 2]))
    );

    // groups of 4 must be consecutive/undef
    assert_eq!(
        widen_shuffle_mask_elements(
            4,
            &mask(&[12, 13, 14, 15, 8, 9, 10, 11, 0, 1, 2, 3, -1, -1, -1, -1])
        ),
        Some(mask(&[3, 2, 0, -1]))
    );

    // back to original mask
    assert_eq!(
        narrow_shuffle_mask_elements(4, &mask(&[3, 2, 0, -1])),
        Some(mask(&[
            12, 13, 14, 15, 8, 9, 10, 11, 0, 1, 2, 3, -1, -1, -1, -1
        ]))
    );

    // groups of 2 must be consecutive/undef
    assert_eq!(
        widen_shuffle_mask_elements(
            2,
            &mask(&[12, 12, 14, 15, 8, 9, 10, 11, 0, 1, 2, 3, -1, -1, -1, -1])
        ),
        None
    );

    // groups of 3 must be consecutive/undef
    assert_eq!(
        widen_shuffle_mask_elements(3, &mask(&[6, 7, 8, 0, 1, 2, -1, -1, -1])),
        Some(mask(&[2, 0, -1]))
    );

    // back to original mask
    assert_eq!(
        narrow_shuffle_mask_elements(3, &mask(&[2, 0, -1])),
        Some(mask(&[6, 7, 8, 0, 1, 2, -1, -1, -1]))
    );

    // groups of 3 must be consecutive/undef (partial undefs are not ok)
    assert_eq!(
        widen_shuffle_mask_elements(3, &mask(&[-1, 7, 8, 0, -1, 2, -1, -1, -1])),
        None
    );
}

/// Ports `TEST_F(BasicTest, getShuffleMaskWithWidestElts)`.
#[test]
fn get_shuffle_mask_with_widest_elts() {
    // can not widen anything here.
    assert_eq!(
        shuffle_mask_with_widest_elements(&mask(&[3, 2, 0, -1])),
        mask(&[3, 2, 0, -1])
    );

    // can always widen identity to single element
    assert_eq!(
        shuffle_mask_with_widest_elements(&mask(&[0, 1, 2])),
        mask(&[0])
    );

    // groups of 4 must be consecutive/undef
    assert_eq!(
        shuffle_mask_with_widest_elements(&mask(&[
            12, 13, 14, 15, 8, 9, 10, 11, 0, 1, 2, 3, -1, -1, -1, -1
        ])),
        mask(&[3, 2, 0, -1])
    );

    // groups of 2 must be consecutive/undef
    assert_eq!(
        shuffle_mask_with_widest_elements(&mask(&[
            12, 12, 14, 15, 8, 9, 10, 11, 0, 1, 2, 3, -1, -1, -1, -1
        ])),
        mask(&[12, 12, 14, 15, 8, 9, 10, 11, 0, 1, 2, 3, -1, -1, -1, -1])
    );

    // groups of 3 must be consecutive/undef
    assert_eq!(
        shuffle_mask_with_widest_elements(&mask(&[6, 7, 8, 0, 1, 2, -1, -1, -1])),
        mask(&[2, 0, -1])
    );

    // groups of 3 must be consecutive/undef (partial undefs are not ok)
    assert_eq!(
        shuffle_mask_with_widest_elements(&mask(&[-1, 7, 8, 0, -1, 2, -1, -1, -1])),
        mask(&[-1, 7, 8, 0, -1, 2, -1, -1, -1])
    );
}

/// Ports `TEST_F(BasicTest, getShuffleDemandedElts)`, every assertion
/// unchanged.
///
/// This fixture went unported when `shuffle_demanded_elements` first shipped;
/// it is the upstream evidence that function had been missing. Two of its six
/// cases are the pair that motivates the `AllowUndefElts` flag at all — the
/// same broadcast, refused without the flag and accepted with it.
#[test]
fn get_shuffle_demanded_elts() {
    // broadcast zero
    assert_eq!(demanded(4, &[0, 0, 0, 0], &bits(4, 0xf), false), (0x1, 0x0));

    // broadcast zero (with non-permitted undefs)
    assert_eq!(
        shuffle_demanded_elements(2, &mask(&[0, -1]), &bits(2, 0x3), false),
        None
    );

    // broadcast zero (with permitted undefs)
    assert_eq!(demanded(3, &[0, 0, -1], &bits(3, 0x7), true), (0x1, 0x0));

    // broadcast one in demanded
    assert_eq!(
        demanded(4, &[1, 1, 1, -1], &bits(4, 0x7), false),
        (0x2, 0x0)
    );

    // broadcast 7 in demanded
    assert_eq!(demanded(4, &[7, 0, 7, 7], &bits(4, 0xd), false), (0x0, 0x8));

    // general test
    assert_eq!(demanded(4, &[4, 2, 7, 3], &bits(4, 0xf), false), (0xc, 0x9));
}

/// Upstream's `getHorizDemandedEltsForFirstOperand(…, LHS, RHS)` pair.
fn horizontal(vector_bit_width: u32, demanded: &ApInt) -> (u64, u64) {
    let DemandedOperandElements {
        lhs: left,
        rhs: right,
    } = horizontal_demanded_elements_for_first_operand(vector_bit_width, demanded)
        .expect("128 bits or wider");
    (left.limited_value(u64::MAX), right.limited_value(u64::MAX))
}

/// Ports `TEST_F(BasicTest, getHorizontalDemandedEltsForFirstOperand)`, all
/// five cases.
///
/// The last one is the reason the vector width is a parameter at all: at 256
/// bits the same four lanes split into two 128-bit groups, so lane 2 is the
/// *first* lane of the second group rather than the third of one group.
#[test]
fn get_horizontal_demanded_elts_for_first_operand() {
    assert_eq!(horizontal(128, &bits(4, 0b0000)), (0b0000, 0b0000));
    assert_eq!(horizontal(128, &bits(4, 0b0001)), (0b0001, 0b0000));
    assert_eq!(horizontal(128, &bits(4, 0b1000)), (0b0000, 0b0100));
    assert_eq!(horizontal(128, &bits(4, 0b0110)), (0b0100, 0b0001));
    assert_eq!(horizontal(256, &bits(4, 0b0100)), (0b0100, 0b0000));
}
