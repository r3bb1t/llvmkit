//! `ComputeNumSignBits` / `ComputeMaxSignificantBits` — tranche 1 of the
//! `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Two kinds of test here, and they answer different questions.
//!
//! **Ports** reproduce an upstream fixture exactly.
//!
//! **Soundness sweeps** check the property the function is *defined* by: for a
//! value whose concrete result is known, the computed count may never exceed
//! the true count of that result. Under-approximating is the whole contract —
//! `ComputeNumSignBits` returns a lower bound — so a sweep that only checked
//! equality would reject correct conservatism, and one that asserted
//! hand-derived numbers would be testing the derivation rather than the port.
//! The oracle is `ApInt::num_sign_bits` of the folded value, which is the
//! definition, not a second implementation of the analysis.

use llvmkit_ir::{
    ApInt, ApIntSignedness, ApIntTruncation, Dyn, IntValue, IrBuilder, IrError, Linkage, Module,
    NoFolder, Value, ValueTrackingQuery, compute_max_significant_bits, compute_num_sign_bits,
};

/// An `i32` constant, built the way the fixtures spell one.
fn i32_const(value: i64) -> ApInt {
    ApInt::new(
        32,
        value as u64,
        ApIntSignedness::Signed,
        ApIntTruncation::Truncate,
    )
    .expect("32-bit constant")
}

/// Build a single-block `@test(i32 %a)` and hand the body builder back.
fn in_function<F, R>(name: &str, body: F) -> Result<R, IrError>
where
    F: for<'m> FnOnce(
        &'m Module<llvmkit_ir::DynBrand>,
        &IrBuilder<'m, 'm, llvmkit_ir::DynBrand, NoFolder, llvmkit_ir::Positioned, Dyn>,
        IntValue<'m, i32, llvmkit_ir::DynBrand>,
    ) -> Result<R, IrError>,
{
    let m = Module::dynamic(name);
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty.as_type(), [i32_ty.as_type()], false);
    let f = m.add_function_dyn("test", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    body(&m, &b, a)
}

/// Port of
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp::TEST_F(ValueTrackingTest, ComputeNumSignBits_PR32045)`.
///
/// ```llvm
/// define i32 @test(i32 %a) {
///   %A = ashr i32 %a, -1
///   ret i32 %A
/// }
/// ```
///
/// The shift amount `-1` is `0xFFFFFFFF`, which is `uge` the 32-bit width, so
/// upstream's `ashr` arm takes its "bad shift" `break` and the answer comes
/// from the `computeKnownBits` tail. Upstream expects 32.
#[test]
fn compute_num_sign_bits_pr32045() -> Result<(), IrError> {
    let bits = in_function("nsb-pr32045", |m, b, a| {
        let minus_one = m.i32_type().const_int(-1_i32);
        let shifted = b.build_int_ashr::<i32, _, _, _>(a, minus_one, "A")?;
        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        compute_num_sign_bits(b.view(shifted).into_erased(), &query)
    })?;
    assert_eq!(bits, 32);
    Ok(())
}

/// A value with no structure known gives the floor answer of 1, and
/// `ComputeMaxSignificantBits` is then the full width — mirroring
/// `llvm::ComputeMaxSignificantBits`, which is
/// `getScalarSizeInBits() - SignBits + 1`.
#[test]
fn unknown_value_has_one_sign_bit_and_full_significant_width() -> Result<(), IrError> {
    let (sign_bits, significant) = in_function("nsb-unknown", |m, _b, a| {
        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        let value: Value<'_, _> = a.into_erased();
        Ok((
            compute_num_sign_bits(value, &query)?,
            compute_max_significant_bits(value, &query)?,
        ))
    })?;
    assert_eq!(sign_bits, 1, "an opaque argument gives the floor");
    assert_eq!(significant, 32, "32 - 1 + 1");
    Ok(())
}

/// Soundness sweep over the arms tranche 1 ports, driven by constant operands
/// so the concrete result — and therefore the true sign-bit count — is known.
///
/// Each case names the upstream arm it exercises. The assertion is the
/// contract: the computed count is a lower bound on the truth, never above it,
/// and never below the floor of 1.
#[test]
fn ported_arms_never_over_report_sign_bits() -> Result<(), IrError> {
    // (label, lhs, rhs, concrete result of the operation)
    struct Case {
        label: &'static str,
        lhs: i64,
        rhs: i64,
        result: i64,
    }
    let cases = [
        // `Instruction::SDiv` — sdiv X, C adds floor(log2 C) sign bits.
        Case {
            label: "sdiv",
            lhs: -1024,
            rhs: 16,
            result: -64,
        },
        Case {
            label: "sdiv",
            lhs: 1024,
            rhs: 4,
            result: 256,
        },
        // `Instruction::SRem` — result lands in (-C, C).
        Case {
            label: "srem",
            lhs: -1000,
            rhs: 16,
            result: -8,
        },
        Case {
            label: "srem",
            lhs: 1000,
            rhs: 16,
            result: 8,
        },
        // `Instruction::AShr` — adds C sign bits.
        Case {
            label: "ashr",
            lhs: -4096,
            rhs: 4,
            result: -256,
        },
        Case {
            label: "ashr",
            lhs: 4096,
            rhs: 4,
            result: 256,
        },
        // `Instruction::Shl` — destroys sign bits.
        Case {
            label: "shl",
            lhs: 3,
            rhs: 4,
            result: 48,
        },
        // `Instruction::Add` / `Sub` — at most one carry bit.
        Case {
            label: "add",
            lhs: 1000,
            rhs: 24,
            result: 1024,
        },
        Case {
            label: "add",
            lhs: -1000,
            rhs: -24,
            result: -1024,
        },
        Case {
            label: "sub",
            lhs: 1000,
            rhs: 24,
            result: 976,
        },
        // `Instruction::Mul` — at most the sum of the inputs' valid bits.
        Case {
            label: "mul",
            lhs: 16,
            rhs: 16,
            result: 256,
        },
        Case {
            label: "mul",
            lhs: -16,
            rhs: 16,
            result: -256,
        },
        // `Instruction::And` / `Or` / `Xor` — preserve sign bits at worst.
        Case {
            label: "and",
            lhs: -256,
            rhs: -16,
            result: -256,
        },
        Case {
            label: "or",
            lhs: -256,
            rhs: 15,
            result: -241,
        },
        Case {
            label: "xor",
            lhs: -256,
            rhs: 15,
            result: -241,
        },
    ];

    for case in cases {
        let computed = in_function("nsb-sweep", |m, b, _a| {
            let i32_ty = m.i32_type();
            let lhs = i32_ty.const_int(i32::try_from(case.lhs).expect("fits i32"));
            let rhs = i32_ty.const_int(i32::try_from(case.rhs).expect("fits i32"));
            let result = match case.label {
                "sdiv" => b.build_int_sdiv::<i32, _, _, _>(lhs, rhs, "r")?,
                "srem" => b.build_int_srem::<i32, _, _, _>(lhs, rhs, "r")?,
                "ashr" => b.build_int_ashr::<i32, _, _, _>(lhs, rhs, "r")?,
                "shl" => b.build_int_shl::<i32, _, _, _>(lhs, rhs, "r")?,
                "add" => b.build_int_add::<i32, _, _, _>(lhs, rhs, "r")?,
                "sub" => b.build_int_sub::<i32, _, _, _>(lhs, rhs, "r")?,
                "mul" => b.build_int_mul::<i32, _, _, _>(lhs, rhs, "r")?,
                "and" => b.build_int_and::<i32, _, _, _>(lhs, rhs, "r")?,
                "or" => b.build_int_or::<i32, _, _, _>(lhs, rhs, "r")?,
                "xor" => b.build_int_xor::<i32, _, _, _>(lhs, rhs, "r")?,
                other => panic!("unhandled case label {other}"),
            };
            let dl = m.data_layout();
            let query = ValueTrackingQuery::new(&dl);
            compute_num_sign_bits(b.view(result).into_erased(), &query)
        })?;

        let truth = i32_const(case.result).num_sign_bits();
        assert!(
            computed >= 1,
            "{}({}, {}): sign-bit count must never be zero",
            case.label,
            case.lhs,
            case.rhs
        );
        assert!(
            computed <= truth,
            "{}({}, {}) = {}: computed {computed} sign bits but the value only has {truth}",
            case.label,
            case.lhs,
            case.rhs,
            case.result
        );
    }
    Ok(())
}

/// `sext` adds exactly the widened bits, and `trunc` keeps whatever survives —
/// the two cast arms, checked against the concrete widened/narrowed value.
#[test]
fn sext_and_trunc_arms_never_over_report() -> Result<(), IrError> {
    let m = Module::dynamic("nsb-casts");
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty.as_type(), false);
    let f = m.add_function_dyn("test", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);

    // sext i8 -8 to i32 == -8; an i8 -8 has 5 sign bits, the i32 has 29.
    let narrow: IntValue<'_, i8, _> = i8_ty.const_int(-8_i8).as_constant().try_into()?;
    let widened = b.build_sext::<i8, i32, _, _>(narrow, i32_ty, "s")?;

    // trunc i32 -8 to i8 == -8.
    let wide: IntValue<'_, i32, _> = i32_ty.const_int(-8_i32).as_constant().try_into()?;
    let narrowed = b.build_trunc::<i32, i8, _, _>(wide, i8_ty, "t")?;

    let dl = m.data_layout();
    let query = ValueTrackingQuery::new(&dl);

    let sext_bits = compute_num_sign_bits(b.view(widened).into_erased(), &query)?;
    let sext_truth = i32_const(-8).num_sign_bits();
    assert!(
        sext_bits >= 1 && sext_bits <= sext_truth,
        "sext: {sext_bits} vs {sext_truth}"
    );

    let trunc_bits = compute_num_sign_bits(b.view(narrowed).into_erased(), &query)?;
    let trunc_truth = ApInt::new(
        8,
        (-8_i64) as u64,
        ApIntSignedness::Signed,
        ApIntTruncation::Truncate,
    )
    .expect("8-bit constant")
    .num_sign_bits();
    assert!(
        trunc_bits >= 1 && trunc_bits <= trunc_truth,
        "trunc: {trunc_bits} vs {trunc_truth}"
    );
    Ok(())
}
