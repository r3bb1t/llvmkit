//! Word order of the `ppc_fp128` component pair.
//!
//! **llvmkit stores the pair mirrored from upstream, and this file pins that.**
//!
//! Upstream bitcasts the pair with
//! `Data[] = {Floats[0]…, Floats[1]…}; APInt(128, 2, Data)`
//! (`DoubleAPFloat::bitcastToAPInt`), and APInt word 0 is the least
//! significant — so upstream puts the *leading* double in the **low** 64 bits.
//! llvmkit's `ppc_words` reads the **high** word as the leading double.
//!
//! The mirroring is invisible to finite arithmetic, because llvmkit sums both
//! components and addition does not care which word it read first. It is
//! visible in exactly three places: the zero/NaN/infinity category, which is
//! decided by the leading component alone; the placement of a special value by
//! the `qnan`/`inf`/`zero` constructors; and `to_bits`, which therefore does
//! **not** agree with upstream's `bitcastToAPInt` for this one semantics.
//!
//! The textual form is unaffected: the `.ll` reader and `AsmWriter` both
//! compensate, so `0xM3FF0000000000000` + sixteen zeros is 1.0 either way,
//! matching `LLLexer::HexToIntPair`. See `parser_hex_float_word_order.rs`.

use llvmkit_ir::{ApFloat, ApFloatSemantics, ApInt, RoundingMode};

/// llvmkit's order: `words[1]` is the leading double, `words[0]` the residual.
fn ppc(leading: u64, residual: u64) -> ApFloat {
    ApFloat::from_bits(
        ApFloatSemantics::PpcDoubleDouble,
        &ApInt::from_words(128, &[residual, leading]),
    )
    .expect("128-bit pattern is valid for PpcDoubleDouble")
}

fn as_double_bits(value: &ApFloat) -> u64 {
    let (converted, _status, _loses) = value.convert(
        ApFloatSemantics::IeeeDouble,
        RoundingMode::NearestTiesToEven,
    );
    converted
        .to_bits()
        .try_zext_u64()
        .expect("IEEEdouble bits fit in u64")
}

#[test]
fn leading_double_dominates_the_value() {
    let one = ppc(0x3FF0_0000_0000_0000, 0);
    assert!(!one.is_zero(), "1.0 + 0 must not be zero");
    assert_eq!(
        as_double_bits(&one),
        0x3FF0_0000_0000_0000,
        "the leading component decides the value"
    );
}

/// The residual is the *small* half: `1.0 + 2^-60` still reads as 1.0 once
/// narrowed to a single `double`.
#[test]
fn residual_double_is_the_small_half() {
    let value = ppc(0x3FF0_0000_0000_0000, 0x3C30_0000_0000_0000);
    assert_eq!(
        as_double_bits(&value),
        0x3FF0_0000_0000_0000,
        "the residual is the small half"
    );
}

/// A zero leading double means the whole value is zero, whatever the residual
/// holds — upstream's `DoubleAPFloat::getCategory` reads `Floats[0]` alone.
#[test]
fn zero_leading_double_is_zero() {
    assert!(
        ppc(0, 0x3FF0_0000_0000_0000).is_zero(),
        "a zero leading double is a zero value regardless of the residual"
    );
}
