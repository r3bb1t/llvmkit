//! DataLayout-aware analysis constant-folding tests.
//!
//! llvmkit-specific harnesses backed by `llvm/lib/Analysis/ConstantFolding.cpp`
//! and libcall availability behavior.

use llvmkit_ir::constant_folding::{
    can_constant_fold_call_to, constant_fold_call, constant_fold_cast_operand,
    constant_fold_instruction, constant_fold_load_from_const_ptr, flush_fp_constant,
};
use llvmkit_ir::instr_types::CastOpcode;
use llvmkit_ir::{
    ApFloat, ApFloatSemantics, ApInt, AttrIndex, Attribute, BinaryIntrinsic, BinaryOpcode,
    CmpPredicate, ConstantExprOpcode, ConstantExprOptions, ConstantFloatValue, ConstantIntValue,
    DataLayout, DenormalMode, DenormalModeKind, DenormalModeSide, FastMathFlags,
    FoldNonDeterminism, IRBuilder, InstructionView, IntDyn, IntPredicate, IrError, LibFunc,
    Linkage, Module, NoFolder, PreservedCastFlags, RoundingMode, TargetLibraryInfo, UnaryOpcode,
    attributes::AttributeStorage, constant_fold_binary_intrinsic, constant_fold_binary_op_operands,
    constant_fold_compare_inst_operands, constant_fold_constant,
    constant_fold_extract_element_instruction, constant_fold_fp_inst_operands,
    constant_fold_inst_operands, constant_fold_integer_cast, constant_fold_load_from_uniform_value,
    constant_fold_load_through_bitcast, constant_fold_unary_op_operand,
    constant_offset_from_global, is_constant_offset_from_global, lossless_inv_cast,
    lossless_signed_trunc, lossless_unsigned_trunc,
};

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldLoadFromConstPtr`:
/// byte extraction from constant globals uses the module `DataLayout` endianness.
#[test]
fn load_from_const_ptr_uses_little_endian_layout() -> Result<(), IrError> {
    Module::with_new("analysis-load-le", |m| {
        let dl = DataLayout::parse("e-p:64:64:64")?;
        let i8_ty = m.i8_type();
        let i16_ty = m.i16_type();
        let arr_ty = m.array_type(i8_ty.as_type(), 2);
        let init = arr_ty.const_array::<ConstantIntValue<'_, i8>, _>([
            i8_ty.const_int(0x34_i8),
            i8_ty.const_int(0x12_i8),
        ])?;
        let g = m.add_global_constant("bytes", init)?;

        let folded = constant_fold_load_from_const_ptr(
            g.as_global_constant_ptr(),
            i16_ty.as_type(),
            ApInt::zero(64),
            &dl,
        )?
        .expect("constant load folds");
        let int = ConstantIntValue::<IntDyn>::try_from(folded)?;

        assert_eq!(int.ap_int(), ApInt::from_words(16, &[0x1234]));
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldLoadFromConst`:
/// bytes past the end of a constant aggregate load as poison.
#[test]
fn load_from_const_ptr_oob_returns_poison() -> Result<(), IrError> {
    Module::with_new("analysis-load-oob", |m| {
        let dl = DataLayout::parse("e-p:64:64:64")?;
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let arr_ty = m.array_type(i8_ty.as_type(), 1);
        let init = arr_ty.const_array::<ConstantIntValue<'_, i8>, _>([i8_ty.const_int(7_i8)])?;
        let g = m.add_global_constant("one", init)?;

        let folded = constant_fold_load_from_const_ptr(
            g.as_global_constant_ptr(),
            i32_ty.as_type(),
            ApInt::zero(64),
            &dl,
        )?
        .expect("oob constant load folds to poison");

        assert_eq!(folded, i32_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// Port of `CastInst::isEliminableCastPair` case 11 (`llvm/lib/IR/Instructions.cpp`):
/// the `ptrtoint(inttoptr x)` collapse is sized by the *pointer* size, not the
/// index size — even though this layout's index size (32) is narrower than
/// its pointer size (64), the round trip preserves the bit set above the
/// index-size boundary (bit 32), because case 11 sizes the collapse using
/// `getPointerTypeSizeInBits`, reached before the (unimplemented in llvmkit)
/// index-size-based switch-fallback in `ConstantFoldCastOperand`.
#[test]
fn ptrtoint_of_inttoptr_uses_pointer_size() -> Result<(), IrError> {
    Module::with_new("analysis-ptr-index", |m| {
        let dl = DataLayout::parse("e-p:64:64:64:32")?;
        let i64_ty = m.i64_type();
        let ptr_ty = m.ptr_type(0);
        let wide = i64_ty.const_ap_int(&ApInt::from_words(64, &[0x1_0000_0001]))?;
        let ptr = constant_fold_cast_operand(
            CastOpcode::IntToPtr,
            wide.as_constant(),
            ptr_ty.as_type(),
            &dl,
        )?
        .expect("inttoptr folds through analysis layer");
        let folded = constant_fold_cast_operand(CastOpcode::PtrToInt, ptr, i64_ty.as_type(), &dl)?
            .expect("ptrtoint folds through analysis layer");

        // Pointer size (64) equals both the i64 source and i64 destination
        // widths, so case 11 collapses the pair to a straight `BitCast` of
        // the original value: the round trip is a lossless identity, even
        // though the index size (32) would have masked off bit 32.
        assert_eq!(folded, wide.as_constant());
        Ok(())
    })
}

/// llvmkit-specific pin for `CastInst::isEliminableCastPair` case 11 on a
/// fat-pointer/CHERI-like layout where pointer size (128) and index size (64)
/// genuinely differ: `ptrtoint(inttoptr x)` on an `i128` must collapse to `x`
/// unmasked. Sizing the collapse by index width instead of pointer width
/// would truncate the high 64 bits a real pointer round trip preserves.
#[test]
fn ptrtoint_of_inttoptr_i128_collapses_to_original_value() -> Result<(), IrError> {
    Module::with_new("analysis-ptr-i128-pointer-size", |m| {
        let dl = DataLayout::parse("e-p:128:128:128:64")?;
        let i128_ty = m.i128_type();
        let ptr_ty = m.ptr_type(0);
        // Bit 64 (above the 64-bit index boundary, within the 128-bit
        // pointer boundary) must survive the round trip.
        let x = i128_ty.const_ap_int(&ApInt::from_words(128, &[1, 1]))?;
        let ptr = constant_fold_cast_operand(
            CastOpcode::IntToPtr,
            x.as_constant(),
            ptr_ty.as_type(),
            &dl,
        )?
        .expect("inttoptr folds through analysis layer");
        let folded = constant_fold_cast_operand(CastOpcode::PtrToInt, ptr, i128_ty.as_type(), &dl)?
            .expect("ptrtoint folds through analysis layer");

        assert_eq!(folded, x.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldLoadThroughBitcast`:
/// PPC fp128 bit reinterpretation is DataLayout-aware analysis folding, not a
/// target-independent `ConstantFold.cpp` fold.
#[test]
fn ppc_fp128_bitcast_requires_datalayout_path() -> Result<(), IrError> {
    Module::with_new("analysis-ppc-bitcast", |m| {
        let dl = DataLayout::parse("e-p:64:64:64")?;
        let ppc_ty = m.ppc_fp128_type();
        let i128_ty = m.i128_type();
        let bits = ApInt::from_words(128, &[0, 0x3ff0_0000_0000_0000]);
        let fp = ApFloat::from_bits(ApFloatSemantics::PpcDoubleDouble, &bits)?;
        let ppc = ppc_ty.const_ap_float(&fp)?.as_constant();

        let folded = constant_fold_cast_operand(CastOpcode::BitCast, ppc, i128_ty.as_type(), &dl)?
            .expect("DataLayout-aware PPC bitcast folds");
        let int = ConstantIntValue::<IntDyn>::try_from(folded)?;

        assert_eq!(int.ap_int(), bits);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolding.cpp::FlushFPConstant`: dynamic
/// denormal mode declines to choose a folded value for denormal inputs.
#[test]
fn dynamic_denormal_mode_declines_flush() -> Result<(), IrError> {
    Module::with_new("analysis-denormal", |m| {
        let f32_ty = m.f32_type();
        let denormal =
            ApFloat::from_bits(ApFloatSemantics::IeeeSingle, &ApInt::from_words(32, &[1]))?;
        let operand = f32_ty.const_ap_float(&denormal)?.as_constant();
        let mode = DenormalMode::new(DenormalModeKind::Dynamic, DenormalModeKind::Dynamic);

        assert_eq!(
            flush_fp_constant(operand, mode, DenormalModeSide::Input)?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolding.cpp::FlushFPConstant`: FP
/// *vector* operands must flush and fold element-wise through the
/// DataLayout-aware analysis path (`constant_fold_fp_inst_operands`), not
/// just scalars — `flush_fp_constant`'s `ConstantVector`/`ConstantDataVector`
/// arms. The IR-builder `ConstantFolder` shell already folds FP vectors
/// element-wise through the target-independent core; this test is specific
/// to the analysis-layer entry point, which used to decline every FP vector.
#[test]
fn fp_vector_fadd_folds_elementwise_through_analysis_path() -> Result<(), IrError> {
    Module::with_new("analysis-fp-vector", |m| {
        let dl = DataLayout::default();
        let f32_ty = m.f32_type();
        let i64_ty = m.i64_type();
        let vec_ty = m.vector_type(f32_ty.as_type(), 2, false);

        let lhs = vec_ty
            .const_vector::<ConstantFloatValue<'_, f32>, _>([
                f32_ty.const_float(1.0),
                f32_ty.const_float(2.0),
            ])?
            .as_constant();
        let rhs = vec_ty
            .const_vector::<ConstantFloatValue<'_, f32>, _>([
                f32_ty.const_float(3.0),
                f32_ty.const_float(4.0),
            ])?
            .as_constant();

        let folded = constant_fold_fp_inst_operands(
            BinaryOpcode::FAdd,
            lhs,
            rhs,
            &dl,
            DenormalMode::ieee(),
            FastMathFlags::empty(),
            FoldNonDeterminism::Allow,
        )?
        .expect("fp vector fadd folds elementwise through the analysis path");

        for (index, expected) in [(0_i64, 4.0_f64), (1, 6.0)] {
            let element = constant_fold_extract_element_instruction(
                folded,
                i64_ty.const_int(index).as_constant(),
            )?
            .expect("lane extracts from the folded vector");
            let fp = ConstantFloatValue::<f32>::try_from(element)?;
            assert!(fp.ap_float().is_exactly_value_f64(expected));
        }
        Ok(())
    })
}

/// Port of `ConstantFolding.cpp::ConstantFoldLoadFromConstPtr` +
/// `GlobalValue::hasDefinitiveInitializer`: an interposable (weak) constant
/// global's initializer is not authoritative — the linker may select another
/// definition — so the load declines to fold, while a strong definition folds.
#[test]
fn interposable_constant_global_load_declines_to_fold() -> Result<(), IrError> {
    Module::with_new("analysis-load-interposable", |m| {
        let dl = DataLayout::parse("e-p:64:64:64")?;
        let i32_ty = m.i32_type();
        let weak = m.add_global_constant("weak_g", i32_ty.const_int(42_i32))?;
        weak.set_linkage(&m, Linkage::WeakAny);
        let strong = m.add_global_constant("strong_g", i32_ty.const_int(7_i32))?;

        assert_eq!(
            constant_fold_load_from_const_ptr(
                weak.as_global_constant_ptr(),
                i32_ty.as_type(),
                ApInt::zero(64),
                &dl,
            )?,
            None,
            "interposable initializer must not fold"
        );
        let folded = constant_fold_load_from_const_ptr(
            strong.as_global_constant_ptr(),
            i32_ty.as_type(),
            ApInt::zero(64),
            &dl,
        )?
        .expect("definitive initializer folds");
        assert_eq!(folded, i32_ty.const_int(7_i32).as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldScalarCall`:
/// modelled math libcalls can be folded when the target library reports support.
#[test]
fn foldable_libcall_sqrt_folds_constant() -> Result<(), IrError> {
    Module::with_new("analysis-libcall", |m| {
        let tli = TargetLibraryInfo::default();
        let f64_ty = m.f64_type();
        let input = f64_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    "4.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();

        assert!(can_constant_fold_call_to(LibFunc::Sqrt, &tli));
        let folded = constant_fold_call(
            LibFunc::Sqrt,
            &[input],
            f64_ty.as_type(),
            &tli,
            FoldNonDeterminism::Allow,
        )?
        .expect("sqrt(4.0) folds");
        let fp = ConstantFloatValue::<f64>::try_from(folded)?;

        assert!(fp.ap_float().is_exactly_value_f64(2.0));
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldScalarCall`:
/// the `LibFunc_sqrt` arm requires non-negative APFloat input before folding.
#[test]
fn negative_sqrt_libcall_declines_without_nan() -> Result<(), IrError> {
    Module::with_new("analysis-libcall-negative-sqrt", |m| {
        let tli = TargetLibraryInfo::default();
        let f64_ty = m.f64_type();
        let input = f64_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    "-4.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();

        assert_eq!(
            constant_fold_call(
                LibFunc::Sqrt,
                &[input],
                f64_ty.as_type(),
                &tli,
                FoldNonDeterminism::Allow,
            )?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldScalarCall`:
/// an unavailable target-library entry declines the fold.
#[test]
fn llvm_null_libcall_case_declines_fold() -> Result<(), IrError> {
    Module::with_new("analysis-libcall-null", |m| {
        let tli = TargetLibraryInfo::without(LibFunc::Sqrt);
        let f64_ty = m.f64_type();
        let input = f64_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    "4.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();

        assert!(!can_constant_fold_call_to(LibFunc::Sqrt, &tli));
        assert_eq!(
            constant_fold_call(
                LibFunc::Sqrt,
                &[input],
                f64_ty.as_type(),
                &tli,
                FoldNonDeterminism::Allow,
            )?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `TargetLibraryInfo.td` and
/// `ConstantFolding.cpp::ConstantFoldScalarCall`: every C libcall spelling
/// recognized by the analysis constant-folder switch maps to its `LibFunc`.
#[test]
fn libfunc_from_name_recognizes_constant_folding_switch_names() {
    let cases = [
        ("acos", LibFunc::Acos),
        ("acosf", LibFunc::Acosf),
        ("__acos_finite", LibFunc::AcosFinite),
        ("__acosf_finite", LibFunc::AcosfFinite),
        ("asin", LibFunc::Asin),
        ("asinf", LibFunc::Asinf),
        ("__asin_finite", LibFunc::AsinFinite),
        ("__asinf_finite", LibFunc::AsinfFinite),
        ("atan", LibFunc::Atan),
        ("atanf", LibFunc::Atanf),
        ("atan2", LibFunc::Atan2),
        ("atan2f", LibFunc::Atan2f),
        ("__atan2_finite", LibFunc::Atan2Finite),
        ("__atan2f_finite", LibFunc::Atan2fFinite),
        ("ceil", LibFunc::Ceil),
        ("ceilf", LibFunc::Ceilf),
        ("cos", LibFunc::Cos),
        ("cosf", LibFunc::Cosf),
        ("cosh", LibFunc::Cosh),
        ("coshf", LibFunc::Coshf),
        ("__cosh_finite", LibFunc::CoshFinite),
        ("__coshf_finite", LibFunc::CoshfFinite),
        ("exp", LibFunc::Exp),
        ("expf", LibFunc::Expf),
        ("__exp_finite", LibFunc::ExpFinite),
        ("__expf_finite", LibFunc::ExpfFinite),
        ("exp2", LibFunc::Exp2),
        ("exp2f", LibFunc::Exp2f),
        ("__exp2_finite", LibFunc::Exp2Finite),
        ("__exp2f_finite", LibFunc::Exp2fFinite),
        ("erf", LibFunc::Erf),
        ("erff", LibFunc::Erff),
        ("fabs", LibFunc::Fabs),
        ("fabsf", LibFunc::Fabsf),
        ("floor", LibFunc::Floor),
        ("floorf", LibFunc::Floorf),
        ("fmod", LibFunc::Fmod),
        ("fmodf", LibFunc::Fmodf),
        ("ilogb", LibFunc::Ilogb),
        ("ilogbf", LibFunc::Ilogbf),
        ("log", LibFunc::Log),
        ("logf", LibFunc::Logf),
        ("logl", LibFunc::Logl),
        ("log2", LibFunc::Log2),
        ("log2f", LibFunc::Log2f),
        ("__log2_finite", LibFunc::Log2Finite),
        ("__log2f_finite", LibFunc::Log2fFinite),
        ("log10", LibFunc::Log10),
        ("log10f", LibFunc::Log10f),
        ("__log_finite", LibFunc::LogFinite),
        ("__logf_finite", LibFunc::LogfFinite),
        ("__log10_finite", LibFunc::Log10Finite),
        ("__log10f_finite", LibFunc::Log10fFinite),
        ("logb", LibFunc::Logb),
        ("logbf", LibFunc::Logbf),
        ("log1p", LibFunc::Log1p),
        ("log1pf", LibFunc::Log1pf),
        ("nearbyint", LibFunc::Nearbyint),
        ("nearbyintf", LibFunc::Nearbyintf),
        ("pow", LibFunc::Pow),
        ("powf", LibFunc::Powf),
        ("__pow_finite", LibFunc::PowFinite),
        ("__powf_finite", LibFunc::PowfFinite),
        ("remainder", LibFunc::Remainder),
        ("remainderf", LibFunc::Remainderf),
        ("rint", LibFunc::Rint),
        ("rintf", LibFunc::Rintf),
        ("round", LibFunc::Round),
        ("roundf", LibFunc::Roundf),
        ("roundeven", LibFunc::Roundeven),
        ("roundevenf", LibFunc::Roundevenf),
        ("sin", LibFunc::Sin),
        ("sinf", LibFunc::Sinf),
        ("sinh", LibFunc::Sinh),
        ("sinhf", LibFunc::Sinhf),
        ("__sinh_finite", LibFunc::SinhFinite),
        ("__sinhf_finite", LibFunc::SinhfFinite),
        ("sqrt", LibFunc::Sqrt),
        ("sqrtf", LibFunc::Sqrtf),
        ("tan", LibFunc::Tan),
        ("tanf", LibFunc::Tanf),
        ("tanh", LibFunc::Tanh),
        ("tanhf", LibFunc::Tanhf),
        ("trunc", LibFunc::Trunc),
        ("truncf", LibFunc::Truncf),
    ];

    for (name, lib_func) in cases {
        assert_eq!(LibFunc::from_name(name), Some(lib_func), "{name}");
        assert_eq!(lib_func.name(), name);
    }
}

/// llvmkit-specific subset of `llvm/Analysis/ConstantFolding.h`: the
/// DataLayout-aware public analysis APIs are exported and usable from callers.
#[test]
fn public_analysis_constant_folding_api_surface_is_usable() -> Result<(), IrError> {
    Module::with_new("analysis-api-surface", |m| {
        let dl = DataLayout::parse("e-p:64:64:64")?;
        let tli = TargetLibraryInfo::default();
        let bool_ty = m.bool_type();
        let i8_ty = m.i8_type();
        let i16_ty = m.i16_type();
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let arr_ty = m.array_type(i8_ty.as_type(), 1);
        let init = arr_ty.const_array::<ConstantIntValue<'_, i8>, _>([i8_ty.const_int(0_i8)])?;
        let g = m.add_global_constant("api_bytes", init)?;
        let c2_i = i32_ty.const_int(2_i32);
        let c5_i = i32_ty.const_int(5_i32);
        let c7_i = i32_ty.const_int(7_i32);
        let c2 = c2_i.as_constant();
        let c5 = c5_i.as_constant();
        let c7 = c7_i.as_constant();
        let one = f32_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeSingle,
                    "1.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();
        let two = f32_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeSingle,
                    "2.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();

        let offset = is_constant_offset_from_global(g.as_global_constant_ptr(), &dl)
            .expect("global pointer has a constant offset");
        assert_eq!(offset.offset(), &ApInt::zero(64));
        assert_eq!(constant_fold_constant(c7, &dl, Some(&tli))?, c7);
        assert_eq!(
            constant_fold_binary_op_operands(BinaryOpcode::Add, c2, c5, &dl)?,
            Some(c7)
        );
        assert_eq!(
            constant_fold_compare_inst_operands(
                CmpPredicate::Int(IntPredicate::Eq),
                c7,
                c7,
                &dl,
                None,
            )?,
            Some(bool_ty.const_int(true).as_constant())
        );
        assert!(constant_fold_unary_op_operand(UnaryOpcode::FNeg, one, &dl)?.is_some());
        assert!(
            constant_fold_fp_inst_operands(
                BinaryOpcode::FAdd,
                one,
                two,
                &dl,
                DenormalMode::ieee(),
                FastMathFlags::empty(),
                FoldNonDeterminism::Allow,
            )?
            .is_some()
        );
        assert_eq!(
            constant_fold_integer_cast(c7, i16_ty.as_type(), false, &dl)?,
            Some(i16_ty.const_int(7_i16).as_constant())
        );
        assert_eq!(
            constant_fold_load_from_uniform_value(
                i8_ty.const_all_ones().as_constant(),
                i16_ty.as_type(),
                &dl,
            )?,
            Some(i16_ty.const_all_ones().as_constant())
        );
        assert_eq!(
            constant_fold_binary_intrinsic(BinaryIntrinsic::UMax, c7, c7, i32_ty.as_type(), &dl)?,
            None
        );
        let one_bits = i32_ty
            .const_ap_int(&ApInt::from_words(32, &[0x3f80_0000]))?
            .as_constant();
        let bitcast = constant_fold_load_through_bitcast(one_bits, f32_ty.as_type(), &dl)?
            .expect("equal-width integer to float load-through-bitcast folds");
        assert!(
            ConstantFloatValue::<f32>::try_from(bitcast)?
                .ap_float()
                .is_exactly_value_f64(1.0)
        );
        let (lossless_bitcast, bitcast_flags) =
            lossless_inv_cast(one_bits, f32_ty.as_type(), CastOpcode::BitCast, &dl)?
                .expect("bitcast is lossless");
        assert_eq!(lossless_bitcast, bitcast);
        assert_eq!(bitcast_flags, PreservedCastFlags::none());
        let (unsigned_trunc, unsigned_flags) =
            lossless_unsigned_trunc(c7, i8_ty.as_type(), &dl)?.expect("small unsigned trunc fits");
        assert_eq!(unsigned_trunc, i8_ty.const_int(7_i8).as_constant());
        assert!(unsigned_flags.has_non_negative());
        let signed_source = i16_ty
            .const_ap_int(&ApInt::from_words(16, &[0x007f]))?
            .as_constant();
        let (signed_trunc, signed_flags) =
            lossless_signed_trunc(signed_source, i8_ty.as_type(), &dl)?
                .expect("positive signed trunc preserves sign");
        assert_eq!(signed_trunc, i8_ty.const_int(127_i8).as_constant());
        assert_eq!(signed_flags, PreservedCastFlags::none());

        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("api_fold_inst", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let add = b.build_int_add::<i32, _, _, _>(c2_i, c5_i, "sum")?;
        let instruction = InstructionView::try_from(add.into_erased())?;
        assert_eq!(
            constant_fold_inst_operands(
                &instruction,
                &[c2, c5],
                &dl,
                Some(&tli),
                FoldNonDeterminism::Allow,
            )?,
            Some(c7)
        );
        Ok(())
    })
}

/// llvmkit-specific validation of `llvm/include/llvm/Analysis/ConstantFolding.h`:
/// the crate root exports `constant_offset_from_global` for Rust callers.
#[test]
fn crate_root_constant_offset_from_global_resolves_global_pointer() -> Result<(), IrError> {
    Module::with_new("analysis-offset-root-export", |m| {
        let dl = DataLayout::parse("e-p:64:64:64")?;
        let i8_ty = m.i8_type();
        let g = m.add_global_constant("root_export", i8_ty.const_int(0_i8))?;

        let resolved = constant_offset_from_global(g.ptr_offset(3), &dl)
            .expect("global pointer plus constant offset resolves");

        assert_eq!(resolved.global(), g);
        assert_eq!(resolved.offset(), &ApInt::from_words(64, &[3]));
        Ok(())
    })
}

/// Mirrors `llvm/lib/Analysis/ConstantFolding.cpp::ConstantFoldInstOperands`
/// for `Instruction::Freeze`: only values guaranteed not to be undef or poison
/// fold through `isGuaranteedNotToBeUndefOrPoison`.
#[test]
fn freeze_folds_only_non_undef_non_poison_constants() -> Result<(), IrError> {
    Module::with_new("analysis-freeze", |m| {
        let dl = DataLayout::default();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("freeze_fold", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let concrete = b.build_freeze(i32_ty.const_int(42_i32), "concrete")?;
        let undef = b.build_freeze(i32_ty.as_type().get_undef(), "undef")?;
        let poison = b.build_freeze(i32_ty.as_type().get_poison(), "poison")?;

        let concrete_inst = concrete.as_view();
        let undef_inst = undef.as_view();
        let poison_inst = poison.as_view();

        assert_eq!(
            constant_fold_instruction(&concrete_inst, &dl, None)?,
            Some(i32_ty.const_int(42_i32).as_constant())
        );
        assert_eq!(constant_fold_instruction(&undef_inst, &dl, None)?, None);
        assert_eq!(constant_fold_instruction(&poison_inst, &dl, None)?, None);
        Ok(())
    })
}

/// llvmkit-specific subset of
/// `llvm/lib/Analysis/ConstantFolding.cpp::IsConstantOffsetFromGlobal`,
/// `ConstantFoldLoadFromConstPtr`, and `ConstantFoldLoadThroughBitcast`:
/// recursive GEP offsets into globals feed load-through-bitcast folding.
#[test]
fn recursive_gep_load_through_bitcast_from_global_folds() -> Result<(), IrError> {
    Module::with_new("analysis-recursive-gep-load", |m| {
        let dl = DataLayout::parse("e-p:64:64:64")?;
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let f32_ty = m.f32_type();
        let arr_ty = m.array_type(i32_ty.as_type(), 2);
        let init = arr_ty.const_array::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(0x3f80_0000_i32),
            i32_ty.const_int(0x4000_0000_i32),
        ])?;
        let g = m.add_global_constant("fp_bits", init)?;
        let zero = i64_ty.const_zero();
        let one = i64_ty.const_int(1_i64);
        let gep = m.constant_expr_with_options(
            m.ptr_type(0).as_type(),
            ConstantExprOpcode::GetElementPtr,
            [
                g.as_global_constant_ptr().into_erased(),
                zero.into_erased(),
                one.into_erased(),
            ],
            [],
            [],
            ConstantExprOptions::new().source_ty(arr_ty.as_type()),
        )?;

        let resolved = constant_offset_from_global(gep, &dl)
            .expect("recursive GEP offset resolves to the base global");
        assert_eq!(resolved.global(), g);
        assert_eq!(resolved.offset(), &ApInt::from_words(64, &[4]));

        let folded =
            constant_fold_load_from_const_ptr(gep, f32_ty.as_type(), ApInt::zero(64), &dl)?
                .expect("GEP to i32 bits folds as a load-through-bitcast to f32");
        let fp = ConstantFloatValue::<f32>::try_from(folded)?;

        assert!(fp.ap_float().is_exactly_value_f64(2.0));
        Ok(())
    })
}

/// Mirrors `llvm/lib/Analysis/ConstantFolding.cpp::ConstantFoldLoadThroughBitcast`:
/// non-integral pointer address spaces decline non-null pointer/int bitcasts.
#[test]
fn non_integral_pointer_load_through_bitcast_declines() -> Result<(), IrError> {
    Module::with_new("analysis-non-integral-bitcast", |m| {
        let dl = DataLayout::parse("e-p:64:64:64-p1:64:64:64-ni:1")?;
        let i8_ty = m.i8_type();
        let i64_ty = m.i64_type();
        let ptr1_ty = m.ptr_type(1);
        let g = m
            .global_builder("ni_global", i8_ty.as_type())
            .constant(true)
            .address_space(1)
            .initializer(i8_ty.const_int(0_i8))
            .build()?;

        assert!(dl.is_non_integral_address_space(1));
        assert_eq!(
            constant_fold_load_through_bitcast(g.as_global_constant_ptr(), i64_ty.as_type(), &dl)?,
            None
        );
        assert_eq!(
            constant_fold_load_through_bitcast(
                i64_ty.const_int(1_i64).as_constant(),
                ptr1_ty.as_type(),
                &dl,
            )?,
            None
        );
        Ok(())
    })
}

/// Mirrors `llvm/lib/Analysis/ConstantFolding.cpp::getInstrDenormalMode` and
/// `llvm/lib/IR/Function.cpp::Function::getDenormalMode`: function string
/// attributes supply denormal mode, and `denormal-fp-math-f32` overrides the
/// generic `denormal-fp-math` mode for f32 operations.
#[test]
fn function_denormal_f32_attribute_overrides_generic_mode() -> Result<(), IrError> {
    Module::with_new("analysis-denormal-attrs", |m| {
        let dl = DataLayout::default();
        let f32_ty = m.f32_type();
        let fn_ty = m.fn_type_no_params(f32_ty, false);
        let f = m.add_function_dyn("denormal_attr", fn_ty, Linkage::External)?;
        f.set_string_attribute(&m, AttrIndex::Function, "denormal-fp-math", "ieee,ieee");
        f.set_string_attribute(
            &m,
            AttrIndex::Function,
            "denormal-fp-math-f32",
            "positive-zero,positive-zero",
        );
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let denormal =
            ApFloat::from_bits(ApFloatSemantics::IeeeSingle, &ApInt::from_words(32, &[1]))?;
        assert!(denormal.is_denormal());
        let lhs = f32_ty.const_ap_float(&denormal)?;
        let rhs = f32_ty.const_ap_float(&denormal)?;
        let add = b.build_fp_add::<f32, _, _, _>(lhs, rhs, "sum")?;
        let instruction = InstructionView::try_from(add.into_erased())?;

        let folded = constant_fold_instruction(&instruction, &dl, None)?
            .expect("f32 denormal inputs fold after f32 attribute flush");
        let fp = ConstantFloatValue::<f32>::try_from(folded)?;

        assert!(fp.ap_float().is_pos_zero());
        Ok(())
    })
}

/// Mirrors `llvm/lib/Analysis/ConstantFolding.cpp::getInstrDenormalMode` and
/// `llvm/lib/IR/Function.cpp::Function::getDenormalMode`: numbered function
/// attribute groups participate in the same denormal lookup as inline attrs.
#[test]
fn function_denormal_attribute_group_overrides_generic_mode() -> Result<(), IrError> {
    Module::with_new("analysis-denormal-attr-group", |m| {
        let dl = DataLayout::default();
        let f32_ty = m.f32_type();
        let fn_ty = m.fn_type_no_params(f32_ty, false);
        let mut group = AttributeStorage::new();
        group.add(
            AttrIndex::Function,
            Attribute::string("denormal-fp-math", "ieee,ieee"),
        );
        group.add(
            AttrIndex::Function,
            Attribute::string("denormal-fp-math-f32", "positive-zero,positive-zero"),
        );
        m.set_attribute_group(0, group);
        let f = m
            .function_builder::<f32, _>("denormal_attr", fn_ty)
            .linkage(Linkage::External)
            .function_attr_group(0)
            .build()?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let denormal =
            ApFloat::from_bits(ApFloatSemantics::IeeeSingle, &ApInt::from_words(32, &[1]))?;
        let lhs = f32_ty.const_ap_float(&denormal)?;
        let rhs = f32_ty.const_ap_float(&denormal)?;
        let add = b.build_fp_add::<f32, _, _, _>(lhs, rhs, "sum")?;
        let instruction = InstructionView::try_from(add.into_erased())?;

        let folded = constant_fold_instruction(&instruction, &dl, None)?
            .expect("f32 denormal inputs fold after attribute-group f32 flush");
        let fp = ConstantFloatValue::<f32>::try_from(folded)?;

        assert!(fp.ap_float().is_pos_zero());
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/lib/Analysis/ConstantFolding.cpp`:
/// host-libm-dependent libcalls decline under deterministic folding, while
/// APFloat-native libcalls such as sqrt remain foldable.
#[test]
fn determinism_deny_declines_host_libm_but_keeps_apfloat_sqrt() -> Result<(), IrError> {
    Module::with_new("analysis-libm-determinism", |m| {
        let tli = TargetLibraryInfo::default();
        let f64_ty = m.f64_type();
        let zero = f64_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    "0.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();
        let four = f64_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    "4.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();

        assert!(can_constant_fold_call_to(LibFunc::Cos, &tli));
        assert_eq!(
            constant_fold_call(
                LibFunc::Cos,
                &[zero],
                f64_ty.as_type(),
                &tli,
                FoldNonDeterminism::Deny,
            )?,
            None
        );

        let folded = constant_fold_call(
            LibFunc::Sqrt,
            &[four],
            f64_ty.as_type(),
            &tli,
            FoldNonDeterminism::Deny,
        )?
        .expect("APFloat-native sqrt folds even when host libm folds are denied");
        let fp = ConstantFloatValue::<f64>::try_from(folded)?;

        assert!(fp.ap_float().is_exactly_value_f64(2.0));
        Ok(())
    })
}

/// Mirrors `ConstantFoldFPInstOperands`'s non-determinism guard in
/// `llvm/lib/Analysis/ConstantFolding.cpp`: under `FoldNonDeterminism::Deny`,
/// an FP instruction carrying `nsz` must decline to fold even though its
/// operands are perfectly foldable constants — a later optimization pass
/// could change the answer once `nsz` is exploited. `Allow` still folds the
/// very same instruction, proving the decline is FMF-driven, not a general
/// failure to fold. Exercises `constant_fold_inst_operands`, the only
/// production path that threads a real instruction's fast-math flags to
/// `constant_fold_fp_inst_operands`.
#[test]
fn deny_declines_fp_binop_with_nsz_flag() -> Result<(), IrError> {
    Module::with_new("analysis-fp-determinism-nsz", |m| {
        let dl = DataLayout::default();
        let f32_ty = m.f32_type();
        let fn_ty = m.fn_type_no_params(f32_ty, false);
        let f = m.add_function_dyn("nsz_fadd", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let one = f32_ty.const_float(1.0);
        let two = f32_ty.const_float(2.0);
        let add =
            b.build_fp_add_fmf::<f32, _, _, _>(one, two, FastMathFlags::NO_SIGNED_ZEROS, "sum")?;
        let instruction = InstructionView::try_from(add.into_erased())?;
        let operands = [one.as_constant(), two.as_constant()];

        assert_eq!(
            constant_fold_inst_operands(
                &instruction,
                &operands,
                &dl,
                None,
                FoldNonDeterminism::Deny,
            )?,
            None,
            "an nsz FP op must decline to fold under deterministic folding"
        );
        let folded = constant_fold_inst_operands(
            &instruction,
            &operands,
            &dl,
            None,
            FoldNonDeterminism::Allow,
        )?
        .expect("the same nsz FP op still folds when non-determinism is allowed");
        let fp = ConstantFloatValue::<f32>::try_from(folded)?;

        assert!(fp.ap_float().is_exactly_value_f64(3.0));
        Ok(())
    })
}
