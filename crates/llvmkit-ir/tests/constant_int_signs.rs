//! Port of `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, IntSigns)`.
//!
//! Exercises the same bit-pattern interpreted as both signed (sext)
//! and unsigned (zext). LLVM's `getSExtValue` / `getZExtValue` map to
//! llvmkit's [`crate::ConstantIntValue::value_sext_i128`] /
//! [`value_zext_u128`].

use llvmkit_ir::{ConstantIntValue, IrError, Module};

/// Port of `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, IntSigns)`.
/// Upstream uses `int8_t`; we run the equivalent through llvmkit's typed
/// `IntType<i8>::const_int(rust_literal)` lifts and read the value back.
#[test]
fn int_signs_i8_round_trips() -> Result<(), IrError> {
    Module::with_new("s", |m| {
        let i8_ty = m.i8_type();

        // Upstream: `EXPECT_EQ(100, ConstantInt::get(Int8Ty, 100, false)->getSExtValue())`.
        // Positive literal: signed and unsigned interpretations coincide.
        let pos: ConstantIntValue<i8> = i8_ty.const_int(100_i8);
        assert_eq!(pos.value_sext_i128(), Some(100));
        assert_eq!(pos.value_zext_u128(), Some(100));

        // Upstream: `EXPECT_EQ(-50, ConstantInt::get(Int8Ty, 206)->getSExtValue())`.
        // The bit pattern 0xCE (= 206 unsigned, = -50 signed) demonstrates the
        // two accessors diverging.
        //
        // We get the same bit pattern by lifting `-50_i8`; its two's-complement
        // representation in 8 bits is 0xCE.
        let neg: ConstantIntValue<i8> = i8_ty.const_int(-50_i8);
        assert_eq!(neg.value_sext_i128(), Some(-50));
        // Upstream: `EXPECT_EQ(206U, ConstantInt::getSigned(Int8Ty, -50)->getZExtValue())`.
        assert_eq!(neg.value_zext_u128(), Some(206));
        Ok(())
    })
}

/// llvmkit-specific (Doctrine D11): `value_sext_i128` for `i32` matches
/// `value_zext_u128` on positive values and propagates the sign bit on
/// negative ones. Closest upstream functional reference:
/// `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, IntSigns)` (the
/// `Int8Ty` arm scaled to wider widths).
#[test]
fn int_signs_i32_propagates_sign() -> Result<(), IrError> {
    Module::with_new("s", |m| {
        let i32_ty = m.i32_type();

        let max: ConstantIntValue<i32> = i32_ty.const_int(i32::MAX);
        assert_eq!(max.value_sext_i128(), Some(i128::from(i32::MAX)));
        assert_eq!(
            max.value_zext_u128(),
            Some(u128::from(u32::try_from(i32::MAX).unwrap()))
        );

        let min: ConstantIntValue<i32> = i32_ty.const_int(i32::MIN);
        assert_eq!(min.value_sext_i128(), Some(i128::from(i32::MIN)));
        // `i32::MIN` as unsigned 32-bit is 0x8000_0000.
        assert_eq!(min.value_zext_u128(), Some(0x8000_0000));

        let neg_one: ConstantIntValue<i32> = i32_ty.const_int(-1_i32);
        assert_eq!(neg_one.value_sext_i128(), Some(-1));
        // `-1_i32` as unsigned 32-bit is 0xFFFF_FFFF.
        assert_eq!(neg_one.value_zext_u128(), Some(0xFFFF_FFFF));
        Ok(())
    })
}

/// llvmkit-specific (Doctrine D11): a negative `i64` lifted into a WIDER
/// target must sign-extend, per the signed-lift contract documented on
/// [`llvmkit_ir::IntType::const_int`] ("signed Rust ints sign-extend") and
/// the `i64 -> Width<N>` impl's own "preserve the signed bit pattern"
/// comment. Closest upstream reference:
/// `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, IntSigns)` --
/// upstream's `ConstantInt::getSigned` is exactly `get(ty, v, true)`
/// (sign-extend), the semantics these lifts must match for signed sources.
#[test]
fn int_signs_i64_sign_extends_into_wider_targets() -> Result<(), IrError> {
    Module::with_new("s", |m| {
        // Static wide target: Width<128>.
        let w128 = m.int_type_n::<128>();
        let neg_one = w128.const_int(-1_i64);
        // Sign-extension: all 128 bits set, NOT the zero-extended 2^64 - 1.
        assert_eq!(neg_one.value_sext_i128(), Some(-1));
        assert_eq!(neg_one.value_zext_u128(), Some(u128::MAX));

        let min = w128.const_int(i64::MIN);
        assert_eq!(min.value_sext_i128(), Some(i128::from(i64::MIN)));

        // Dyn wide target: 128-bit IntDyn.
        let dyn128 = m.custom_width_int_type(128)?;
        let dyn_neg = dyn128.const_int_checked(-1_i64)?;
        assert_eq!(dyn_neg.value_sext_i128(), Some(-1));
        assert_eq!(dyn_neg.value_zext_u128(), Some(u128::MAX));

        // Dyn NARROW target: -1 fits in i32 under signed interpretation,
        // exactly as the signed i8/i16/i32 dyn lifts already accept it.
        let dyn32 = m.custom_width_int_type(32)?;
        let narrow_neg = dyn32.const_int_checked(-1_i64)?;
        assert_eq!(narrow_neg.value_sext_i128(), Some(-1));
        assert_eq!(narrow_neg.value_zext_u128(), Some(0xFFFF_FFFF));

        // A value that genuinely does not fit still rejects.
        assert!(matches!(
            dyn32.const_int_checked(i64::MAX),
            Err(IrError::ImmediateOverflow { .. })
        ));
        Ok(())
    })
}

/// llvmkit-specific (Doctrine D11): a negative `i128` lifted into a WIDER
/// `Width<N>` must sign-extend above bit 128, same signed-lift contract as
/// the `i64` locks above (`ConstantInt::getSigned` = `get(ty, v, true)`;
/// `APInt(bits, val, /*isSigned=*/true)` sign-fills the high words). The
/// unsigned `u128` lift zero-extends as the control.
#[test]
fn int_signs_i128_sign_extends_into_wider_targets() -> Result<(), IrError> {
    Module::with_new("s", |m| {
        let w256 = m.int_type_n::<256>();

        // Sign-extension: all 256 bits set, NOT the zero-extended 2^128 - 1
        // (which does not even fit a signed i128 readback).
        let neg_one = w256.const_int(-1_i128);
        assert_eq!(neg_one.value_sext_i128(), Some(-1));

        let min = w256.const_int(i128::MIN);
        assert_eq!(min.value_sext_i128(), Some(i128::MIN));

        // N == 128 stays a bit-identity lift.
        let w128 = m.int_type_n::<128>();
        let exact = w128.const_int(-1_i128);
        assert_eq!(exact.value_sext_i128(), Some(-1));
        assert_eq!(exact.value_zext_u128(), Some(u128::MAX));

        // Unsigned control: u128::MAX zero-extends into the wider target.
        let unsigned = w256.const_int(u128::MAX);
        assert_eq!(unsigned.value_zext_u128(), Some(u128::MAX));
        Ok(())
    })
}

/// llvmkit-specific (Doctrine D11): exercises the `bool` (i1) edge case.
/// Closest upstream reference: `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest,
/// IntSigns)` (the `int1_t` corner of `getSExtValue` -- a one-bit `1`
/// sign-extends to `-1`).
#[test]
fn int_signs_i1_sign_extends_to_minus_one() -> Result<(), IrError> {
    Module::with_new("s", |m| {
        let i1_ty = m.bool_type();
        let one_bit_true: ConstantIntValue<bool> = i1_ty.const_int(true);
        // Upstream rule: a 1-bit constant set to `1` sext-extends to `-1` (all
        // bits set). zext gives `1`.
        assert_eq!(one_bit_true.value_sext_i128(), Some(-1));
        assert_eq!(one_bit_true.value_zext_u128(), Some(1));

        let one_bit_false: ConstantIntValue<bool> = i1_ty.const_int(false);
        assert_eq!(one_bit_false.value_sext_i128(), Some(0));
        assert_eq!(one_bit_false.value_zext_u128(), Some(0));
        Ok(())
    })
}
