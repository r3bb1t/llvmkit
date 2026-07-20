//! Target-independent constant-folding tests.
//!
//! Source-derived subsets from `llvm/lib/IR/ConstantFold.cpp` plus exact
//! assembler excerpts where cited explicitly.

use llvmkit_ir::instr_types::CastOpcode;
use llvmkit_ir::{
    Align, ApFloat, ApFloatSemantics, ApFloatSign, ApInt, BinaryOpcode, CmpPredicate, Constant,
    ConstantExprFlags, ConstantExprInRange, ConstantExprOpcode, ConstantExprOptions,
    ConstantFloatValue, ConstantIntValue, FloatDyn, FloatPredicate, GepNoWrapFlags, IRBuilder,
    InstructionView, IntDyn, IntPredicate, IrError, Linkage, MaybeAlign, Module, NoFolder,
    RoundingMode, UDivFlags, UnaryOpcode, UnnamedAddr, constant_fold_binary_instruction,
    constant_fold_cast_instruction, constant_fold_compare_instruction,
    constant_fold_extract_element_instruction, constant_fold_extract_value_instruction,
    constant_fold_get_element_ptr, constant_fold_insert_element_instruction,
    constant_fold_insert_value_instruction, constant_fold_instruction,
    constant_fold_select_instruction, constant_fold_shuffle_vector_instruction,
    constant_fold_unary_instruction,
};

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction` APInt `shl` path.
#[test]
fn shl_i257_by_width_minus_one_folds_to_apint_result() -> Result<(), IrError> {
    Module::with_new("fold", |m| {
        let ty = m.int_type_n::<257>();
        let one = ty.const_int(1_u8).as_constant();
        let shift = ty.const_int(256_u16).as_constant();
        let folded = constant_fold_binary_instruction(BinaryOpcode::Shl, one, shift)?
            .expect("all-constant shl folds");
        let int = ConstantIntValue::<IntDyn>::try_from(folded)?;
        assert_eq!(int.ap_int(), ApInt::one_bit_set(257, 256));
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction` shift poison rule.
#[test]
fn shl_i257_by_width_returns_poison() -> Result<(), IrError> {
    Module::with_new("fold", |m| {
        let ty = m.int_type_n::<257>();
        let one = ty.const_int(1_u8).as_constant();
        let shift = ty.const_int(257_u16).as_constant();
        let folded = constant_fold_binary_instruction(BinaryOpcode::Shl, one, shift)?
            .expect("invalid constant shift folds to poison");
        assert_eq!(folded, ty.as_type().get_poison().as_constant());

        let i32_ty = m.i32_type();
        let zero = i32_ty.const_zero().as_constant();
        let shift = i32_ty.const_int(32_i32).as_constant();
        let folded = constant_fold_binary_instruction(BinaryOpcode::Shl, zero, shift)?
            .expect("zero shifted by bitwidth folds to poison");
        assert_eq!(folded, i32_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp` integer division-by-zero poison rule.
#[test]
fn udiv_by_zero_returns_poison() -> Result<(), IrError> {
    Module::with_new("fold", |m| {
        let ty = m.i32_type();
        let lhs = ty.const_int(42_i32).as_constant();
        let zero = ty.const_zero().as_constant();
        let folded = constant_fold_binary_instruction(BinaryOpcode::UDiv, lhs, zero)?
            .expect("invalid constant division folds to poison");
        assert_eq!(folded, ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp` signed min / -1 poison rule.
#[test]
fn sdiv_signed_min_by_minus_one_returns_poison() -> Result<(), IrError> {
    Module::with_new("fold", |m| {
        let ty = m.i32_type();
        let lhs = ty.const_int(i32::MIN).as_constant();
        let minus_one = ty.const_int(-1_i32).as_constant();
        let folded = constant_fold_binary_instruction(BinaryOpcode::SDiv, lhs, minus_one)?
            .expect("overflowing constant sdiv folds to poison");
        assert_eq!(folded, ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCastInstruction`:
/// negative finite FP-to-uint inputs fold to poison, not wrapped integers.
#[test]
fn fptoui_negative_constant_returns_poison() -> Result<(), IrError> {
    Module::with_new("fold-cast", |m| {
        let f64_ty = m.f64_type();
        let i32_ty = m.i32_type();
        let folded = constant_fold_cast_instruction(
            CastOpcode::FpToUI,
            f64_ty.const_double(-1.0).as_constant(),
            i32_ty.as_type(),
        )?
        .expect("invalid fptoui folds");
        assert_eq!(folded, i32_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCastInstruction`:
/// fp128 integer-valued constants must not narrow through host `double` before
/// producing the destination APInt.
#[test]
fn fptosi_fp128_integer_keeps_low_bits() -> Result<(), IrError> {
    Module::with_new("fold-cast-wide", |m| {
        let fp128_ty = m.fp128_type();
        let i128_ty = m.i128_type();
        let fp = ApFloat::from_bits(
            ApFloatSemantics::IeeeQuad,
            &ApInt::from_words(128, &[0x1000, 0x4063_0000_0000_0000]),
        )?;
        let folded = constant_fold_cast_instruction(
            CastOpcode::FpToSI,
            fp128_ty.const_ap_float(&fp)?.as_constant(),
            i128_ty.as_type(),
        )?
        .expect("valid fp128 fptosi folds");
        let int = ConstantIntValue::<IntDyn>::try_from(folded)?;
        let expected = ApInt::one_bit_set(128, 100).wrapping_add(&ApInt::from_words(128, &[1]));
        assert_eq!(int.ap_int(), expected);
        Ok(())
    })
}

/// Port of `Constants.cpp::ConstantFP::isNullValue` plus
/// `ConstantFold.cpp::ConstantFoldCastInstruction`: `-0.0` is not a null
/// floating constant, so bitcast preserves its sign bit.
#[test]
fn bitcast_negative_zero_float_preserves_sign_bit() -> Result<(), IrError> {
    Module::with_new("fold-neg-zero-bitcast", |m| {
        let f32_ty = m.f32_type();
        let i32_ty = m.i32_type();
        let neg_zero = ApFloat::zero(ApFloatSemantics::IeeeSingle, ApFloatSign::Negative);
        let folded = constant_fold_cast_instruction(
            CastOpcode::BitCast,
            f32_ty.const_ap_float(&neg_zero)?.as_constant(),
            i32_ty.as_type(),
        )?
        .expect("negative zero bitcast folds");
        let int = ConstantIntValue::<IntDyn>::try_from(folded)?;
        assert_eq!(int.ap_int(), ApInt::one_bit_set(32, 31));
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::ConstantFoldCastInstruction` lines 153-183:
/// fixed-vector casts with matching lane counts fold element-wise.
#[test]
fn vector_trunc_cast_folds_elementwise() -> Result<(), IrError> {
    Module::with_new("fold-vector-cast", |m| {
        let i32_ty = m.i32_type();
        let i16_ty = m.i16_type();
        let src_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let dst_ty = m.vector_type(i16_ty.as_type(), 2, false);
        let source = src_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let folded = constant_fold_cast_instruction(
            CastOpcode::Trunc,
            source.as_constant(),
            dst_ty.as_type(),
        )?
        .expect("same-lane vector trunc folds");
        let expected = dst_ty.const_vector::<ConstantIntValue<'_, i16>, _>([
            i16_ty.const_int(1_i16),
            i16_ty.const_int(2_i16),
        ])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::ConstantFoldBinaryInstruction`: fixed-length
/// vector integer constants fold element-wise.
#[test]
fn vector_integer_binary_folds_elementwise() -> Result<(), IrError> {
    Module::with_new("fold-vector-binop", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let lhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let rhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
        ])?;
        let folded = constant_fold_binary_instruction(
            BinaryOpcode::Add,
            lhs.as_constant(),
            rhs.as_constant(),
        )?
        .expect("vector add folds");
        let expected = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(4_i32),
            i32_ty.const_int(6_i32),
        ])?;
        assert_eq!(folded, expected.as_constant());

        let zero_vec = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_zero(),
            i32_ty.const_zero(),
        ])?;
        let poison_lane = i32_ty.as_type().get_poison().as_constant();
        let poison_vec = vec_ty.const_vector::<Constant<'_>, _>([
            poison_lane,
            i32_ty.const_int(7_i32).as_constant(),
        ])?;
        let folded = constant_fold_binary_instruction(
            BinaryOpcode::Mul,
            zero_vec.as_constant(),
            poison_vec.as_constant(),
        )?
        .expect("vector mul folds per lane before absorber");
        let expected = vec_ty
            .const_vector::<Constant<'_>, _>([poison_lane, i32_ty.const_zero().as_constant()])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::ConstantFoldBinaryInstruction` lines 927-947:
/// vector division/remainder by a zero RHS folds to vector poison.
#[test]
fn vector_div_by_zero_splat_folds_to_vector_poison() -> Result<(), IrError> {
    Module::with_new("fold-vector-div-zero", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let lhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let rhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_zero(),
            i32_ty.const_zero(),
        ])?;
        let folded = constant_fold_binary_instruction(
            BinaryOpcode::UDiv,
            lhs.as_constant(),
            rhs.as_constant(),
        )?
        .expect("vector udiv by zero folds");
        assert_eq!(folded, vec_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::ConstantFoldBinaryInstruction` lines 620-621:
/// scalable vector undef operands follow the scalar undef fold rules before
/// vector element extraction is considered.
#[test]
fn scalable_vector_undef_binary_folds_before_bailout() -> Result<(), IrError> {
    Module::with_new("fold-scalable-vector-undef-binop", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let undef = vec_ty.as_type().get_undef().as_constant();
        let folded = constant_fold_binary_instruction(BinaryOpcode::Xor, undef, undef)?
            .expect("scalable vector undef xor folds");
        let expected = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_zero(),
            i32_ty.const_zero(),
        ])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::ConstantFoldBinaryInstruction` lines 620-621:
/// fixed-length vector undef operands fold per lane instead of taking the
/// scalar/scalable undef shortcut.
#[test]
fn fixed_vector_undef_binary_folds_per_lane() -> Result<(), IrError> {
    Module::with_new("fold-fixed-vector-undef-binop", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let undef = vec_ty.as_type().get_undef().as_constant();
        let rhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let folded = constant_fold_binary_instruction(BinaryOpcode::Mul, undef, rhs.as_constant())?
            .expect("fixed vector undef mul folds per lane");
        let lane_zero = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(0_i32).as_constant(),
        )?
        .expect("lane zero extracts");
        let lane_one = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(1_i32).as_constant(),
        )?
        .expect("lane one extracts");
        assert_eq!(lane_zero, i32_ty.as_type().get_undef().as_constant());
        assert_eq!(lane_one, i32_ty.const_zero().as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::ConstantFoldCastInstruction` lines 153-170:
/// scalable vector splat casts fold before the scalable-vector bailout.
#[test]
fn scalable_vector_trunc_splat_folds() -> Result<(), IrError> {
    Module::with_new("fold-scalable-vector-cast", |m| {
        let i32_ty = m.i32_type();
        let i16_ty = m.i16_type();
        let src_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let dst_ty = m.vector_type(i16_ty.as_type(), 2, true);
        let one = i32_ty.const_int(1_i32);
        let source = src_ty.const_vector::<ConstantIntValue<'_, i32>, _>([one, one])?;
        let folded = constant_fold_cast_instruction(
            CastOpcode::Trunc,
            source.as_constant(),
            dst_ty.as_type(),
        )?
        .expect("scalable vector splat trunc folds");
        let expected = dst_ty.const_vector::<ConstantIntValue<'_, i16>, _>([
            i16_ty.const_int(1_i16),
            i16_ty.const_int(1_i16),
        ])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::FoldBitCast` all-ones handling: all-ones
/// fixed-vector integer bitcasts can produce all-ones floating vector splats.
#[test]
fn vector_bitcast_all_ones_to_float_splat_folds() -> Result<(), IrError> {
    Module::with_new("fold-vector-bitcast-float", |m| {
        let i16_ty = m.i16_type();
        let f32_ty = m.f32_type();
        let src_ty = m.vector_type(i16_ty.as_type(), 4, false);
        let dst_ty = m.vector_type(f32_ty.as_type(), 2, false);
        let minus_one = i16_ty.const_int(-1_i16);
        let source = src_ty.const_vector::<ConstantIntValue<'_, i16>, _>([
            minus_one, minus_one, minus_one, minus_one,
        ])?;
        let folded = constant_fold_cast_instruction(
            CastOpcode::BitCast,
            source.as_constant(),
            dst_ty.as_type(),
        )?
        .expect("all-ones vector bitcast to float folds");
        let all_ones_float =
            ApFloat::from_bits(ApFloatSemantics::IeeeSingle, &ApInt::all_ones(32))?;
        let scalar = f32_ty.const_ap_float(&all_ones_float)?.as_constant();
        let expected = dst_ty.const_vector::<Constant<'_>, _>([scalar, scalar])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::FoldBitCast` lines 67-76:
/// scalar all-ones bitcasts fold to all-ones destination constants before
/// scalar-to-vector bitcasts are canonicalized as vector bitcasts.
#[test]
fn scalar_all_ones_bitcast_to_vector_splat_folds() -> Result<(), IrError> {
    Module::with_new("fold-scalar-all-ones-bitcast", |m| {
        let i64_ty = m.i64_type();
        let i32_ty = m.i32_type();
        let dst_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let folded = constant_fold_cast_instruction(
            CastOpcode::BitCast,
            i64_ty.const_all_ones().as_constant(),
            dst_ty.as_type(),
        )?
        .expect("scalar all-ones bitcast folds");
        let all_ones = i32_ty.const_all_ones().as_constant();
        let expected = dst_ty.const_vector::<Constant<'_>, _>([all_ones, all_ones])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::FoldBitCast` lines 67-76:
/// scalar floating constants whose bit pattern is all ones bitcast to all-ones
/// destination constants before scalar-to-vector bitcasts are canonicalized.
#[test]
fn fp_all_ones_bitcast_to_vector_splat_folds() -> Result<(), IrError> {
    Module::with_new("fold-fp-all-ones-bitcast", |m| {
        let f64_ty = m.f64_type();
        let bits = ApInt::all_ones(64);
        let fp = ApFloat::from_bits(ApFloatSemantics::IeeeDouble, &bits)?;
        let operand = f64_ty.const_ap_float(&fp)?.as_constant();
        let i32_ty = m.i32_type();
        let dst_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let folded =
            constant_fold_cast_instruction(CastOpcode::BitCast, operand, dst_ty.as_type())?
                .expect("FP all-ones bitcast folds");
        let all_ones = i32_ty.const_all_ones().as_constant();
        let expected = dst_ty.const_vector::<Constant<'_>, _>([all_ones, all_ones])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::FoldBitCast` lines 70-76:
/// non-all-ones scalar integer bitcasts to vector destinations canonicalize
/// through a one-lane vector bitcast constant expression instead of declining.
#[test]
fn scalar_int_bitcast_to_vector_canonicalizes_as_vector_bitcast() -> Result<(), IrError> {
    Module::with_new("fold-scalar-vector-bitcast", |m| {
        let i32_ty = m.i32_type();
        let i16_ty = m.i16_type();
        let dst_ty = m.vector_type(i16_ty.as_type(), 2, false);
        let folded = constant_fold_cast_instruction(
            CastOpcode::BitCast,
            i32_ty.const_int(42_i32).as_constant(),
            dst_ty.as_type(),
        )?
        .expect("scalar-to-vector bitcast canonicalizes");
        m.add_global("bits", folded)?;
        let text = format!("{m}");
        assert!(
            text.contains(
                "@bits = global <2 x i16> bitcast (<1 x i32> splat (i32 42) to <2 x i16>)"
            ),
            "{text}"
        );
        Ok(())
    })
}

/// llvmkit-specific regression for `ConstantFold.cpp::ConstantFoldBinaryInstruction`:
/// scalar i1 shortcuts do not apply to non-splat scalable i1 vector constants
/// after the scalable vector elementwise fold declines.
#[test]
fn scalable_i1_non_splat_divrem_does_not_use_scalar_i1_shortcuts() -> Result<(), IrError> {
    Module::with_new("fold-scalable-i1-non-splat", |m| {
        let i1_ty = m.bool_type();
        let vec_ty = m.vector_type(i1_ty.as_type(), 2, true);
        let one = i1_ty.const_int(true).as_constant();
        let zero = i1_ty.const_int(false).as_constant();
        let lhs = vec_ty
            .const_vector::<Constant<'_>, _>([one, zero])?
            .as_constant();
        let rhs = vec_ty
            .const_vector::<Constant<'_>, _>([one, zero])?
            .as_constant();

        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::UDiv, lhs, rhs)?,
            None
        );
        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::URem, lhs, rhs)?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCastInstruction`
/// lines 153-182: same-lane vector casts use `foldMaybeUndesirableCast`, so
/// desirable scalar casts materialize per-lane constant expressions.
#[test]
fn same_lane_vector_ptrtoint_cast_builds_lane_constant_exprs() -> Result<(), IrError> {
    Module::with_new("fold-vector-ptrtoint-cast", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr = g.as_global_constant_ptr();
        let src_ty = m.vector_type(ptr.ty(), 2, false);
        let dst_ty = m.vector_type(i64_ty.as_type(), 2, false);
        let vector = src_ty.const_vector::<Constant<'_>, _>([ptr, ptr])?;
        let folded = constant_fold_cast_instruction(
            CastOpcode::PtrToInt,
            vector.as_constant(),
            dst_ty.as_type(),
        )?
        .expect("vector ptrtoint folds through lane constant expressions");
        let scalar = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [ptr.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let expected = dst_ty.const_vector::<Constant<'_>, _>([scalar, scalar])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `Constants.cpp::ConstantExpr::isSupportedCastOp`:
/// `ptrtoaddr` is a supported target-independent constant-expression cast opcode,
/// distinct from `ptrtoint`.
#[test]
fn ptrtoaddr_cast_opcode_is_supported_constexpr_cast() {
    assert_eq!(CastOpcode::PtrToAddr.keyword(), "ptrtoaddr");
    assert!(CastOpcode::PtrToAddr.is_desirable_constant_expr());
    assert!(CastOpcode::PtrToAddr.is_supported_constant_expr());
}

/// llvmkit-specific subset of `ConstantFold.cpp::FoldBitCast`: target-independent
/// folding refuses PPC double-double bit reinterpretation because memory layout
/// needs target endianness.
#[test]
fn ppc_fp128_bitcasts_return_none_without_data_layout() -> Result<(), IrError> {
    Module::with_new("fold-ppc-bitcast", |m| {
        let ppc_ty = m.ppc_fp128_type();
        let i128_ty = m.i128_type();
        let bits = ApInt::from_words(128, &[0, 0x3ff0_0000_0000_0000]);
        let fp = ApFloat::from_bits(ApFloatSemantics::PpcDoubleDouble, &bits)?;
        let ppc_const = ppc_ty.const_ap_float(&fp)?.as_constant();
        assert_eq!(
            constant_fold_cast_instruction(CastOpcode::BitCast, ppc_const, i128_ty.as_type())?,
            None
        );

        let int_const = i128_ty.const_ap_int(&bits)?.as_constant();
        assert_eq!(
            constant_fold_cast_instruction(CastOpcode::BitCast, int_const, ppc_ty.as_type())?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractElementInstruction`:
/// undef indices and out-of-range fixed-vector indices fold to poison.
#[test]
fn extractelement_undef_and_out_of_range_indices_fold_to_poison() -> Result<(), IrError> {
    Module::with_new("fold-extractelement", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let one = i32_ty.const_int(1_i32);
        let two = i32_ty.const_int(2_i32);
        let vector = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([one, two])?;
        let poison = i32_ty.as_type().get_poison().as_constant();

        let undef_index = i32_ty.as_type().get_undef().as_constant();
        let folded = constant_fold_extract_element_instruction(vector.as_constant(), undef_index)?
            .expect("undef extractelement index folds");
        assert_eq!(folded, poison);

        let out_of_range = i32_ty.const_int(2_i32).as_constant();
        let folded = constant_fold_extract_element_instruction(vector.as_constant(), out_of_range)?
            .expect("out-of-range extractelement index folds");
        assert_eq!(folded, poison);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`:
/// undef integer rules for bitwise and shift opcodes.
#[test]
fn undef_integer_binary_rules_fold_to_llvm_constants() -> Result<(), IrError> {
    Module::with_new("fold-undef", |m| {
        let ty = m.i32_type();
        let undef = ty.as_type().get_undef().as_constant();
        let five = ty.const_int(5_i32).as_constant();
        let zero = ty.const_zero().as_constant();
        let all_ones = ty.const_all_ones().as_constant();

        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::And, undef, all_ones)?
                .expect("undef & all-ones folds to identity operand"),
            undef
        );
        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::Or, undef, zero)?
                .expect("undef | zero folds to identity operand"),
            undef
        );
        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::Shl, undef, zero)?
                .expect("undef << zero folds to identity operand"),
            undef
        );
        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::LShr, undef, zero)?
                .expect("undef lshr zero folds to identity operand"),
            undef
        );
        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::AShr, undef, zero)?
                .expect("undef ashr zero folds to identity operand"),
            undef
        );

        let and = constant_fold_binary_instruction(BinaryOpcode::And, undef, five)?
            .expect("undef & X folds");
        let and_int = ConstantIntValue::<IntDyn>::try_from(and)?;
        assert!(and_int.ap_int().is_zero());

        let or = constant_fold_binary_instruction(BinaryOpcode::Or, undef, five)?
            .expect("undef | X folds");
        let or_int = ConstantIntValue::<IntDyn>::try_from(or)?;
        assert_eq!(or_int.ap_int(), ty.const_all_ones().ap_int());

        let xor = constant_fold_binary_instruction(BinaryOpcode::Xor, undef, undef)?
            .expect("undef ^ undef folds");
        let xor_int = ConstantIntValue::<IntDyn>::try_from(xor)?;
        assert!(xor_int.ap_int().is_zero());

        let shl = constant_fold_binary_instruction(BinaryOpcode::Shl, five, undef)?
            .expect("X << undef folds");
        assert_eq!(shl, ty.as_type().get_poison().as_constant());

        let udiv_zero = constant_fold_binary_instruction(BinaryOpcode::UDiv, undef, zero)?
            .expect("undef / zero folds");
        assert_eq!(udiv_zero, ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `Constants.cpp::ConstantExpr::get`: supported
/// binary constant expressions consult `ConstantFoldBinaryInstruction` before interning.
#[test]
fn constant_expr_add_folds_before_interning() -> Result<(), IrError> {
    Module::with_new("fold", |m| {
        let ty = m.i32_type();
        let lhs = ty.const_int(1_i32);
        let rhs = ty.const_int(2_i32);
        let expr = m.constant_expr(
            ty.as_type(),
            ConstantExprOpcode::Add,
            [lhs.as_value(), rhs.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let int = ConstantIntValue::<IntDyn>::try_from(expr)?;
        assert_eq!(int.ap_int(), ApInt::from_words(32, &[3]));
        m.add_global("sum", expr)?;
        let text = format!("{m}");
        assert!(text.contains("@sum = global i32 3"), "{text}");
        Ok(())
    })
}

/// llvmkit-specific refinement invariant backed by
/// `llvm/test/Assembler/ConstantExprNoFold.ll`: an unfolded integer-typed
/// `ConstantExpr` is not a `ConstantInt`.
#[test]
fn constant_int_refinement_rejects_unfolded_integer_constant_expr() -> Result<(), IrError> {
    Module::with_new("fold-refine", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr_as_int = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let expr = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::Add,
            [ptr_as_int.as_value(), i64_ty.const_int(1_i64).as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert!(ConstantIntValue::<IntDyn>::try_from(expr).is_err());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractElementInstruction`
/// lines 374-381: extracting the same constant index from an unreduced
/// `insertelement` constant expression returns the inserted element.
#[test]
fn extractelement_from_insertelement_constant_expr_folds_inserted_lane() -> Result<(), IrError> {
    Module::with_new("fold-extract-insertelement-expr", |m| {
        let i64_ty = m.i64_type();
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let base = m.constant_expr(
            vec_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [i64_ty.const_int(42_i64).as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let inserted = i32_ty.const_int(7_i32).as_constant();
        let index = i32_ty.const_int(1_i32).as_constant();
        let insert_expr = m.constant_expr(
            vec_ty.as_type(),
            ConstantExprOpcode::InsertElement,
            [base.as_value(), inserted.as_value(), index.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;

        let folded = constant_fold_extract_element_instruction(insert_expr, index)?
            .expect("extractelement from insertelement constexpr folds");
        assert_eq!(folded, inserted);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractElementInstruction`
/// lines 374-381: `ExtractElement(InsertElement)` compares index constants by
/// numeric value, not by APInt bit width.
#[test]
fn extractelement_from_insertelement_matches_indices_across_widths() -> Result<(), IrError> {
    Module::with_new("fold-extract-insertelement-index-width", |m| {
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let base = vec_ty.as_type().get_undef().as_constant();
        let inserted = i32_ty.const_int(7_i32).as_constant();
        let insert_index = i8_ty.const_int(1_i8).as_constant();
        let insert_expr = m.constant_expr(
            vec_ty.as_type(),
            ConstantExprOpcode::InsertElement,
            [
                base.as_value(),
                inserted.as_value(),
                insert_index.as_value(),
            ],
            [],
            [],
            ConstantExprFlags::none(),
        )?;

        let extract_index = i32_ty.const_int(1_i32).as_constant();
        let folded = constant_fold_extract_element_instruction(insert_expr, extract_index)?
            .expect("same numeric index extracts inserted lane");
        assert_eq!(folded, inserted);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractElementInstruction`
/// lines 374-381: `APSInt::isSameValue` compares arbitrary-width index
/// constants without truncating through host integer widths.
#[test]
fn extractelement_from_insertelement_matches_wide_indices() -> Result<(), IrError> {
    Module::with_new("fold-extract-insertelement-wide-index", |m| {
        let i32_ty = m.i32_type();
        let i129_ty = m.int_type_n::<129>();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let base = vec_ty.as_type().get_undef().as_constant();
        let inserted = i32_ty.const_int(7_i32).as_constant();
        let wide_index = i129_ty
            .const_ap_int(&ApInt::one_bit_set(129, 128))?
            .as_constant();
        let insert_expr = m.constant_expr(
            vec_ty.as_type(),
            ConstantExprOpcode::InsertElement,
            [base.as_value(), inserted.as_value(), wide_index.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;

        let folded = constant_fold_extract_element_instruction(insert_expr, wide_index)?
            .expect("same arbitrary-width index extracts inserted lane");
        assert_eq!(folded, inserted);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp` lines 338-341 and 398-407:
/// poison indices are undef-like for extract/insert, and inserting null into an
/// all-zero vector returns the original zero vector before range checks.
#[test]
fn extract_insert_poison_indices_and_zero_insert_fold_like_llvm() -> Result<(), IrError> {
    Module::with_new("fold-extract-insert-poison-zero", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let one = i32_ty.const_int(1_i32);
        let two = i32_ty.const_int(2_i32);
        let vector = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([one, two])?;
        let poison_index = i32_ty.as_type().get_poison().as_constant();
        let poison_element = i32_ty.as_type().get_poison().as_constant();
        let poison_vector = vec_ty.as_type().get_poison().as_constant();

        let folded = constant_fold_extract_element_instruction(vector.as_constant(), poison_index)?
            .expect("poison extractelement index folds");
        assert_eq!(folded, poison_element);

        let folded = constant_fold_insert_element_instruction(
            vector.as_constant(),
            i32_ty.const_zero().as_constant(),
            poison_index,
        )?
        .expect("poison insertelement index folds");
        assert_eq!(folded, poison_vector);

        let zero_vec = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_zero(),
            i32_ty.const_zero(),
        ])?;
        let out_of_range = i32_ty.const_int(99_i32).as_constant();
        let folded = constant_fold_insert_element_instruction(
            zero_vec.as_constant(),
            i32_ty.const_zero().as_constant(),
            out_of_range,
        )?
        .expect("zero inserted into zero vector folds before range checks");
        assert_eq!(folded, zero_vec.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantExpr::getExtractElement`: the constructor
/// returns a folded poison constant instead of interning an invalid fixed-lane extraction.
#[test]
fn constant_expr_extractelement_out_of_range_folds_to_poison() -> Result<(), IrError> {
    Module::with_new("fold-extractelement-expr", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let one = i32_ty.const_int(1_i32);
        let two = i32_ty.const_int(2_i32);
        let vector = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([one, two])?;
        let out_of_range = i32_ty.const_int(2_i32);
        let expr = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::ExtractElement,
            [vector.as_value(), out_of_range.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(expr, i32_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `Constants.cpp::ConstantExpr::getGetElementPtr`:
/// an empty index list is folded by the constructor before a `ConstantExpr` key is interned.
#[test]
fn constant_expr_empty_gep_folds_before_interning() -> Result<(), IrError> {
    Module::with_new("fold-gep-expr", |m| {
        let ty = m.i32_type();
        let g = m.add_global("g", ty.const_zero())?;
        let base = g.as_global_constant_ptr();
        let expr = m.constant_expr_with_options(
            base.ty(),
            ConstantExprOpcode::GetElementPtr,
            [base.as_value()],
            [],
            [],
            ConstantExprOptions::new().source_ty(ty.as_type()),
        )?;
        assert_eq!(expr, base);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldGetElementPtr`:
/// an empty index list is a no-op GEP and folds to the base pointer.
#[test]
fn gep_empty_indices_fold_to_base_pointer() -> Result<(), IrError> {
    Module::with_new("gep-fold", |m| {
        let ty = m.i32_type();
        let g = m.add_global("g", ty.const_zero())?;
        let base = g.as_global_constant_ptr();
        let folded = constant_fold_get_element_ptr(ty.as_type(), base, &[], None)?
            .expect("empty-index GEP folds");
        assert_eq!(folded, base);
        Ok(())
    })
}

/// llvmkit-specific subset of `include/llvm/Analysis/ConstantFolding.h::ConstantFoldInstruction`:
/// instruction-level analysis folding dispatches to the same APInt binary folder
/// as direct constant folding.
#[test]
fn analysis_instruction_fold_uses_apint_binary_folder() -> Result<(), IrError> {
    Module::with_new("analysis-fold", |m| {
        let ty = m.int_type_n::<257>();
        let fn_ty = m.fn_type_no_params(ty, false);
        let f = m.add_function_dyn("wide", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let high = ty.const_ap_int(&ApInt::one_bit_set(257, 256))?;
        let value = b.build_int_add(high, ty.const_zero(), "sum")?;
        let instruction = InstructionView::try_from(value.as_value())?;
        let folded = constant_fold_instruction(&instruction)?.expect("constant add folds");
        let int = ConstantIntValue::<IntDyn>::try_from(folded)?;
        assert_eq!(int.ap_int(), ApInt::one_bit_set(257, 256));
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// plus `PossiblyExactOperator`: instruction-level exact `udiv` with a non-zero
/// remainder folds to poison.
#[test]
fn analysis_instruction_fold_exact_udiv_inexact_returns_poison() -> Result<(), IrError> {
    Module::with_new("analysis-exact-fold", |m| {
        let ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(ty, false);
        let f = m.add_function_dyn("exact", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let value = b.build_int_udiv_with_flags::<i32, _, _, _>(
            ty.const_int(7_i32),
            ty.const_int(3_i32),
            UDivFlags::new().exact(),
            "q",
        )?;
        let instruction = InstructionView::try_from(value.as_value())?;

        let folded = constant_fold_instruction(&instruction)?.expect("exact udiv folds");

        assert_eq!(folded, ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::FoldBitCast`: PPC double-double
/// to i128 target-independent bitcast folding declines without `DataLayout`.
#[test]
fn bitcast_ppc_fp128_to_i128_declines_target_independent_fold() -> Result<(), IrError> {
    Module::with_new("fold-ppc-to-int", |m| {
        let ppc_ty = m.ppc_fp128_type();
        let i128_ty = m.i128_type();
        let bits = ApInt::from_words(128, &[0, 0x3ff0_0000_0000_0000]);
        let fp = ApFloat::from_bits(ApFloatSemantics::PpcDoubleDouble, &bits)?;
        let ppc_const = ppc_ty.const_ap_float(&fp)?.as_constant();

        assert_eq!(
            constant_fold_cast_instruction(CastOpcode::BitCast, ppc_const, i128_ty.as_type())?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::FoldBitCast`: i128 to PPC
/// double-double target-independent bitcast folding declines without `DataLayout`.
#[test]
fn bitcast_i128_to_ppc_fp128_declines_target_independent_fold() -> Result<(), IrError> {
    Module::with_new("fold-int-to-ppc", |m| {
        let ppc_ty = m.ppc_fp128_type();
        let i128_ty = m.i128_type();
        let bits = ApInt::from_words(128, &[0, 0x3ff0_0000_0000_0000]);
        let int_const = i128_ty.const_ap_int(&bits)?.as_constant();

        assert_eq!(
            constant_fold_cast_instruction(CastOpcode::BitCast, int_const, ppc_ty.as_type())?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractElementInstruction`:
/// an invalid fixed-vector lane index folds to element poison.
#[test]
fn extractelement_fixed_vector_out_of_range_returns_poison() -> Result<(), IrError> {
    Module::with_new("fold-extract-oob", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let vector = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let out_of_range = i32_ty.const_int(2_i32).as_constant();

        let folded = constant_fold_extract_element_instruction(vector.as_constant(), out_of_range)?
            .expect("out-of-range fixed extractelement folds");

        assert_eq!(folded, i32_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractElementInstruction`:
/// undef fixed-vector lane index folds to element poison.
#[test]
fn extractelement_fixed_vector_undef_index_returns_poison() -> Result<(), IrError> {
    Module::with_new("fold-extract-undef", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let vector = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let undef_index = i32_ty.as_type().get_undef().as_constant();

        let folded = constant_fold_extract_element_instruction(vector.as_constant(), undef_index)?
            .expect("undef fixed extractelement index folds");

        assert_eq!(folded, i32_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractElementInstruction`:
/// non-integer indices are not `ConstantInt`, so target-independent folding declines.
#[test]
fn extractelement_fixed_vector_non_integer_index_declines() -> Result<(), IrError> {
    Module::with_new("extract-non-int-index", |m| {
        let elem_ty = m.i32_type();
        let vector_ty = m.vector_type(elem_ty.as_type(), 2, false);
        let vector = vector_ty
            .const_vector::<ConstantIntValue<'_, i32>, _>([
                elem_ty.const_int(11_i32),
                elem_ty.const_int(22_i32),
            ])?
            .as_constant();
        let index = m.f32_type().const_float(0.0).as_constant();

        let folded = constant_fold_extract_element_instruction(vector, index)?;

        assert_eq!(folded, None);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldInsertElementInstruction`:
/// fixed-vector insert with a constant in-range index rebuilds the vector.
#[test]
fn insertelement_fixed_vector_replaces_lane() -> Result<(), IrError> {
    Module::with_new("fold-insertelement", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let vector = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let folded = constant_fold_insert_element_instruction(
            vector.as_constant(),
            i32_ty.const_int(99_i32).as_constant(),
            i32_ty.const_int(1_i32).as_constant(),
        )?
        .expect("in-range insertelement folds");
        let lane_one = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(1_i32).as_constant(),
        )?
        .expect("inserted lane extracts");

        assert_eq!(lane_one, i32_ty.const_int(99_i32).as_constant());

        let poison_index = i32_ty.as_type().get_poison().as_constant();
        let folded = constant_fold_insert_element_instruction(
            vector.as_constant(),
            i32_ty.const_int(7_i32).as_constant(),
            poison_index,
        )?
        .expect("poison index folds to poison vector");
        assert_eq!(folded, vec_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldShuffleVectorInstruction`
/// lines 448-479: a fixed-vector mask selects lanes from both operands,
/// individual `-1` mask elements become undef, and an all-`-1` mask becomes
/// a poison vector.
#[test]
fn shufflevector_fixed_mask_selects_lanes() -> Result<(), IrError> {
    Module::with_new("fold-shuffle", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let lhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let rhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
        ])?;
        let folded = constant_fold_shuffle_vector_instruction(
            lhs.as_constant(),
            rhs.as_constant(),
            &[1, 2, -1],
        )?
        .expect("fixed shufflevector folds");
        let lane_zero = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(0_i32).as_constant(),
        )?
        .expect("shuffle lane zero extracts");
        let lane_one = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(1_i32).as_constant(),
        )?
        .expect("shuffle lane one extracts");
        let lane_two = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(2_i32).as_constant(),
        )?
        .expect("shuffle undef lane extracts");

        assert_eq!(lane_zero, i32_ty.const_int(2_i32).as_constant());
        assert_eq!(lane_one, i32_ty.const_int(3_i32).as_constant());
        assert_eq!(lane_two, i32_ty.as_type().get_undef().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldShuffleVectorInstruction`
/// lines 448-469: all-poison masks fold before scalable-vector iteration is
/// declined.
#[test]
fn shufflevector_scalable_all_poison_mask_folds() -> Result<(), IrError> {
    Module::with_new("fold-scalable-shuffle-poison", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let splat = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(1_i32),
        ])?;
        let folded = constant_fold_shuffle_vector_instruction(
            splat.as_constant(),
            splat.as_constant(),
            &[-1, -1],
        )?
        .expect("all-poison scalable shufflevector mask folds");
        assert_eq!(folded, vec_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldInsertValueInstruction`:
/// nested array insertvalue rebuilds the aggregate with the inserted constant.
#[test]
fn insertvalue_array_replaces_element() -> Result<(), IrError> {
    Module::with_new("fold-insertvalue", |m| {
        let i32_ty = m.i32_type();
        let array_ty = m.array_type(i32_ty.as_type(), 2);
        let aggregate = array_ty.const_array::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let folded = constant_fold_insert_value_instruction(
            aggregate.as_constant(),
            i32_ty.const_int(77_i32).as_constant(),
            &[0],
        )?
        .expect("insertvalue folds");
        let first = constant_fold_extract_value_instruction(folded, &[0])?
            .expect("inserted element extracts");
        let second = constant_fold_extract_value_instruction(folded, &[1])?
            .expect("preserved element extracts");

        assert_eq!(first, i32_ty.const_int(77_i32).as_constant());
        assert_eq!(second, i32_ty.const_int(2_i32).as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldExtractValueInstruction`
/// lines 499-508: `getAggregateElement` on undef/poison aggregates yields a
/// typed undef/poison element rather than declining the fold.
#[test]
fn extractvalue_undef_and_poison_aggregates_fold_to_typed_elements() -> Result<(), IrError> {
    Module::with_new("fold-extractvalue-undef-poison", |m| {
        let i32_ty = m.i32_type();
        let array_ty = m.array_type(i32_ty.as_type(), 2);
        let undef = array_ty.as_type().get_undef().as_constant();
        let poison = array_ty.as_type().get_poison().as_constant();

        let undef_lane = constant_fold_extract_value_instruction(undef, &[1])?
            .expect("extractvalue undef aggregate folds");
        assert_eq!(undef_lane, i32_ty.as_type().get_undef().as_constant());

        let poison_lane = constant_fold_extract_value_instruction(poison, &[0])?
            .expect("extractvalue poison aggregate folds");
        assert_eq!(poison_lane, i32_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `Constants.cpp::ConstantExpr::get`: supported
/// trunc constant expressions consult `ConstantFoldCastInstruction` before interning.
#[test]
fn constant_expr_trunc_folds_before_interning() -> Result<(), IrError> {
    Module::with_new("fold-trunc-expr", |m| {
        let i64_ty = m.i64_type();
        let i8_ty = m.i8_type();
        let wide = i64_ty.const_int(257_i64);
        let expr = m.constant_expr(
            i8_ty.as_type(),
            ConstantExprOpcode::Trunc,
            [wide.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;

        let int = ConstantIntValue::<IntDyn>::try_from(expr)?;
        assert_eq!(int.ap_int(), ApInt::from_words(8, &[1]));
        Ok(())
    })
}

/// Exact excerpt of `test/Assembler/ptrtoaddr.ll`: ptrtoaddr constant
/// expressions keep their distinct opcode and address-space spelling.
#[test]
fn constant_expr_ptrtoaddr_uses_distinct_opcode() -> Result<(), IrError> {
    Module::with_new("fold-ptrtoaddr-expr", |m| {
        m.set_data_layout("p1:64:64:64:32")?;
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let i_as0 = m.add_global("i_as0", i32_ty.const_zero())?;
        let cast_as0 = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::PtrToAddr,
            [i_as0.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        m.add_global("global_cast_as0", cast_as0)?;
        let i_as1 = m
            .global_builder("i_as1", i32_ty.as_type())
            .address_space(1)
            .initializer(i32_ty.const_zero())
            .build()?;
        let cast_as1 = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToAddr,
            [i_as1.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        m.add_global("global_cast_as1", cast_as1)?;

        assert_eq!(
            format!("{m}"),
            concat!(
                "; ModuleID = 'fold-ptrtoaddr-expr'\n",
                "target datalayout = \"p1:64:64:64:32\"\n",
                "\n",
                "@i_as0 = global i32 0\n",
                "@global_cast_as0 = global i64 ptrtoaddr (ptr @i_as0 to i64)\n",
                "@i_as1 = addrspace(1) global i32 0\n",
                "@global_cast_as1 = global i32 ptrtoaddr (ptr addrspace(1) @i_as1 to i32)\n",
            )
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::foldConstantCastPair`:
/// `trunc (ptrtoint @g to i64) to i32` folds to `ptrtoint @g to i32`.
#[test]
fn cast_of_cast_ptrtoint_trunc_folds_to_narrow_ptrtoint() -> Result<(), IrError> {
    Module::with_new("fold-cast-pair", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr = g.as_global_constant_ptr();
        let wide = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [ptr.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let folded = constant_fold_cast_instruction(CastOpcode::Trunc, wide, i32_ty.as_type())?
            .expect("cast-of-cast ptrtoint trunc folds");
        let expected = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [ptr.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(folded, expected);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`:
/// LLVM IR `frem` follows C `fmod`, not IEEE remainder.
#[test]
fn frem_uses_modulo_not_ieee_remainder() -> Result<(), IrError> {
    Module::with_new("fold-frem-modulo", |m| {
        let f64_ty = m.f64_type();

        let lhs = f64_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    "7.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();
        let rhs = f64_ty
            .const_ap_float(
                &ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    "4.0",
                    RoundingMode::NearestTiesToEven,
                )?
                .0,
            )?
            .as_constant();

        let folded = constant_fold_binary_instruction(BinaryOpcode::FRem, lhs, rhs)?
            .expect("all-constant frem folds");
        let folded = ConstantFloatValue::<FloatDyn>::try_from(folded)?;

        assert_eq!(
            folded.ap_float().to_bits(),
            ApInt::from_words(64, &[0x4008_0000_0000_0000])
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// lines 907-918: associative constant expressions reassociate when the nested
/// RHS and new RHS fold to a non-same-op constant.
#[test]
fn associative_constant_expr_binary_reassociates_folded_rhs() -> Result<(), IrError> {
    Module::with_new("fold-assoc-constexpr", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr_as_int = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let one = i64_ty.const_int(1_i64).as_constant();
        let two = i64_ty.const_int(2_i64).as_constant();
        let three = i64_ty.const_int(3_i64).as_constant();
        let inner = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::Add,
            [ptr_as_int.as_value(), one.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;

        let folded = constant_fold_binary_instruction(BinaryOpcode::Add, inner, two)?
            .expect("associative constexpr add folds nested constants");
        let expected = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::Add,
            [ptr_as_int.as_value(), three.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(folded, expected);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// lines 783-789: a constant integer LHS and commutative desirable
/// constant-expression RHS are swapped before building the folded expression.
#[test]
fn commuted_desirable_binop_with_constant_expr_rhs_builds_swapped_expr() -> Result<(), IrError> {
    Module::with_new("fold-commuted-desirable-constexpr", |m| {
        let i32_ty = m.i32_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr_as_i32 = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let one = i32_ty.const_int(1_i32).as_constant();

        let folded = constant_fold_binary_instruction(BinaryOpcode::Xor, one, ptr_as_i32)?
            .expect("commuted desirable constant expression folds");
        let expected = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::Xor,
            [ptr_as_i32.as_value(), one.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(folded, expected);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCastInstruction`
/// lines 129-136: undef casts that introduce a bounded result fold to zero,
/// while the remaining target-independent casts preserve undef in the
/// destination type.
#[test]
fn undef_cast_rules_fold_to_zero_or_undef() -> Result<(), IrError> {
    Module::with_new("fold-undef-cast", |m| {
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let f64_ty = m.f64_type();
        let undef_i8 = i8_ty.as_type().get_undef().as_constant();
        let undef_i32 = i32_ty.as_type().get_undef().as_constant();

        let zext = constant_fold_cast_instruction(CastOpcode::ZExt, undef_i8, i32_ty.as_type())?
            .expect("zext undef folds to zero");
        assert_eq!(zext, i32_ty.const_zero().as_constant());

        let sext = constant_fold_cast_instruction(CastOpcode::SExt, undef_i8, i32_ty.as_type())?
            .expect("sext undef folds to zero");
        assert_eq!(sext, i32_ty.const_zero().as_constant());

        let uitofp =
            constant_fold_cast_instruction(CastOpcode::UIToFp, undef_i32, f64_ty.as_type())?
                .expect("uitofp undef folds to zero");
        let uitofp = ConstantFloatValue::<FloatDyn>::try_from(uitofp)?;
        assert!(uitofp.ap_float().is_zero());

        let sitofp =
            constant_fold_cast_instruction(CastOpcode::SIToFp, undef_i32, f64_ty.as_type())?
                .expect("sitofp undef folds to zero");
        let sitofp = ConstantFloatValue::<FloatDyn>::try_from(sitofp)?;
        assert!(sitofp.ap_float().is_zero());

        let trunc = constant_fold_cast_instruction(CastOpcode::Trunc, undef_i32, i8_ty.as_type())?
            .expect("trunc undef folds to destination undef");
        assert_eq!(trunc, i8_ty.as_type().get_undef().as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// lines 693-712: FP undef operands fold to undef for undef/undef and
/// negative-zero subtraction, otherwise to a quiet NaN because undef may be
/// chosen as NaN.
#[test]
fn fp_undef_binary_rules_fold_to_undef_or_nan() -> Result<(), IrError> {
    Module::with_new("fold-fp-undef", |m| {
        let f64_ty = m.f64_type();
        let undef = f64_ty.as_type().get_undef().as_constant();

        let fadd = constant_fold_binary_instruction(BinaryOpcode::FAdd, undef, undef)?
            .expect("undef fadd undef folds");
        assert_eq!(fadd, undef);

        let fsub = constant_fold_binary_instruction(
            BinaryOpcode::FSub,
            f64_ty.const_double(-0.0).as_constant(),
            undef,
        )?
        .expect("-0.0 - undef folds");
        assert_eq!(fsub, undef);

        let fmul = constant_fold_binary_instruction(
            BinaryOpcode::FMul,
            f64_ty.const_double(3.0).as_constant(),
            undef,
        )?
        .expect("finite fp op with undef folds to NaN");
        let fmul = ConstantFloatValue::<FloatDyn>::try_from(fmul)?;
        assert!(fmul.ap_float().is_nan());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// lines 693-696 plus `PatternMatch.h::m_NegZeroFP`: scalable vector
/// `-0.0 - undef` follows the scalar/scalable undef rule only when the
/// left operand matches the negative-zero FP pattern. Poison lanes are
/// ignored, but undef lanes and poison-only vectors do not match.
#[test]
fn scalable_vector_fsub_negative_zero_pattern_controls_undef_fold() -> Result<(), IrError> {
    Module::with_new("fold-scalable-fp-negzero-undef", |m| {
        let f32_ty = m.f32_type();
        let vec_ty = m.vector_type(f32_ty.as_type(), 2, true);
        let neg_zero = f32_ty.const_float(-0.0).as_constant();
        let undef_lane = f32_ty.as_type().get_undef().as_constant();
        let poison_lane = f32_ty.as_type().get_poison().as_constant();
        let rhs = vec_ty.as_type().get_undef().as_constant();

        for lhs in [
            vec_ty
                .const_vector::<Constant<'_>, _>([neg_zero, neg_zero])?
                .as_constant(),
            vec_ty
                .const_vector::<Constant<'_>, _>([neg_zero, poison_lane])?
                .as_constant(),
        ] {
            let folded = constant_fold_binary_instruction(BinaryOpcode::FSub, lhs, rhs)?
                .expect("scalable -0.0 - undef folds");
            assert_eq!(folded, rhs);
        }

        for lhs in [
            vec_ty
                .const_vector::<Constant<'_>, _>([neg_zero, undef_lane])?
                .as_constant(),
            vec_ty
                .const_vector::<Constant<'_>, _>([poison_lane, poison_lane])?
                .as_constant(),
        ] {
            let folded = constant_fold_binary_instruction(BinaryOpcode::FSub, lhs, rhs)?
                .expect("non-matching scalable fsub undef folds to NaN");
            let lane_zero = constant_fold_extract_element_instruction(
                folded,
                m.i32_type().const_zero().as_constant(),
            )?
            .expect("folded NaN splat extracts lane zero");
            let lane_zero = ConstantFloatValue::<FloatDyn>::try_from(lane_zero)?;
            assert!(lane_zero.ap_float().is_nan());
        }
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCompareInstruction`
/// lines 1096-1131 and 1167-1199: scalar undef compares choose undef,
/// equality-at-the-non-undef value, or NaN for FP predicates; fixed vectors
/// rebuild the per-lane compare result.
#[test]
fn compare_undef_rules_fold_scalar_and_vector_results() -> Result<(), IrError> {
    Module::with_new("fold-compare-undef", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let f64_ty = m.f64_type();
        let undef_i32 = i32_ty.as_type().get_undef().as_constant();
        let five = i32_ty.const_int(5_i32).as_constant();

        let eq = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Eq),
            undef_i32,
            five,
        )?
        .expect("icmp eq undef folds");
        assert_eq!(eq, bool_ty.as_type().get_undef().as_constant());

        let slt = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Slt),
            undef_i32,
            five,
        )?
        .expect("ordered int predicate with undef folds");
        assert_eq!(slt, bool_ty.const_int(false).as_constant());

        let uno = constant_fold_compare_instruction(
            CmpPredicate::Float(FloatPredicate::Uno),
            f64_ty.as_type().get_undef().as_constant(),
            f64_ty.const_double(1.0).as_constant(),
        )?
        .expect("unordered fp predicate with undef folds");
        assert_eq!(uno, bool_ty.const_int(true).as_constant());

        let i32_vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let bool_vec_ty = m.vector_type(bool_ty.as_type(), 2, false);
        let lhs = i32_vec_ty
            .const_vector::<Constant<'_>, _>([undef_i32, i32_ty.const_int(7_i32).as_constant()])?;
        let rhs = i32_vec_ty.const_vector::<Constant<'_>, _>([
            i32_ty.const_int(5_i32).as_constant(),
            i32_ty.const_int(7_i32).as_constant(),
        ])?;
        let folded = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Eq),
            lhs.as_constant(),
            rhs.as_constant(),
        )?
        .expect("fixed-vector icmp undef folds per lane");
        let expected = bool_vec_ty.const_vector::<Constant<'_>, _>([
            bool_ty.as_type().get_undef().as_constant(),
            bool_ty.const_int(true).as_constant(),
        ])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCompareInstruction`
/// lines 1169-1179: splatted vector compares fold before the scalable-vector
/// bailout, while non-splat scalable vectors still decline.
#[test]
fn compare_scalable_vector_splats_fold_before_scalable_bailout() -> Result<(), IrError> {
    Module::with_new("fold-compare-scalable-splat", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let i32_vec_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let bool_vec_ty = m.vector_type(bool_ty.as_type(), 2, true);
        let int_lane = i32_ty.const_int(9_i32).as_constant();
        let lhs = i32_vec_ty.const_vector::<Constant<'_>, _>([int_lane, int_lane])?;
        let rhs = i32_vec_ty.const_vector::<Constant<'_>, _>([int_lane, int_lane])?;

        let folded = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Eq),
            lhs.as_constant(),
            rhs.as_constant(),
        )?
        .expect("scalable vector splat compare folds");
        let bool_lane = bool_ty.const_int(true).as_constant();
        let expected = bool_vec_ty.const_vector::<Constant<'_>, _>([bool_lane, bool_lane])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCompareInstruction`
/// lines 1134-1156 and 1202-1209: non-concrete constants still use the
/// unsigned-null shortcut, i1 EQ/NE xor rewrites, and identical-FP folds.
#[test]
fn compare_constant_expr_edge_cases_fold() -> Result<(), IrError> {
    Module::with_new("fold-compare-constexpr-edges", |m| {
        let bool_ty = m.bool_type();
        let i1_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr_as_i32 = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let zero_i32 = i32_ty.const_zero().as_constant();

        let uge_zero = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Uge),
            ptr_as_i32,
            zero_i32,
        )?
        .expect("C >=u 0 folds for constant expressions");
        assert_eq!(uge_zero, bool_ty.const_int(true).as_constant());

        let ult_zero = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Ult),
            ptr_as_i32,
            zero_i32,
        )?
        .expect("C <u 0 folds for constant expressions");
        assert_eq!(ult_zero, bool_ty.const_int(false).as_constant());

        let ptr_as_i1 = m.constant_expr(
            i1_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let true_i1 = i1_ty.const_int(true).as_constant();
        let eq_true = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Eq),
            ptr_as_i1,
            true_i1,
        )?
        .expect("i1 constexpr eq folds");
        assert_eq!(eq_true, ptr_as_i1);

        let ne_true = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Ne),
            ptr_as_i1,
            true_i1,
        )?
        .expect("i1 constexpr ne folds");
        let expected_ne = m.constant_expr(
            i1_ty.as_type(),
            ConstantExprOpcode::Xor,
            [ptr_as_i1.as_value(), true_i1.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(ne_true, expected_ne);

        let fp_bits = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let fp_expr = m.constant_expr(
            f32_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [fp_bits.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let one = constant_fold_compare_instruction(
            CmpPredicate::Float(FloatPredicate::One),
            fp_expr,
            fp_expr,
        )?
        .expect("same FP constexpr one folds");
        assert_eq!(one, bool_ty.const_int(false).as_constant());
        let ueq = constant_fold_compare_instruction(
            CmpPredicate::Float(FloatPredicate::Ueq),
            fp_expr,
            fp_expr,
        )?
        .expect("same FP constexpr ueq folds");
        assert_eq!(ueq, bool_ty.const_int(true).as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCompareInstruction`
/// lines 1298-1305: a null LHS and non-null constant-expression RHS are
/// retried with a swapped predicate, enabling the RHS-null shortcuts.
#[test]
fn compare_null_lhs_constant_expr_rhs_commutes_to_rhs_null_shortcut() -> Result<(), IrError> {
    Module::with_new("fold-compare-null-lhs-constexpr", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr_as_i64 = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;

        let folded = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Ule),
            i64_ty.const_zero().as_constant(),
            ptr_as_i64,
        )?
        .expect("null-left compare folds after swapping");
        assert_eq!(folded, bool_ty.const_int(true).as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldCompareInstruction`
/// lines 1181-1199: fixed-vector compare folding extracts lanes with
/// `ConstantExpr::getExtractElement`, so vector constant expressions fold too.
#[test]
fn compare_vector_constant_expr_operands_fold_by_extracting_lanes() -> Result<(), IrError> {
    Module::with_new("fold-compare-vector-constexpr", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let bool_vec_ty = m.vector_type(bool_ty.as_type(), 2, false);
        let vector = m.constant_expr(
            vec_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [i64_ty.const_int(42_i64).as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;

        let folded =
            constant_fold_compare_instruction(CmpPredicate::Int(IntPredicate::Eq), vector, vector)?
                .expect("vector constexpr compare folds per lane");
        let true_lane = bool_ty.const_int(true).as_constant();
        let expected = bool_vec_ty.const_vector::<Constant<'_>, _>([true_lane, true_lane])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::evaluateICmpRelation`:
/// globals compare greater than null, distinct safe globals compare not-equal,
/// blockaddresses compare not-equal to null/globals/different functions, and
/// inbounds global GEPs compare greater than null.
#[test]
fn compare_global_pointer_relations_fold() -> Result<(), IrError> {
    Module::with_new("fold-compare-global", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let g = m.add_global("g", i32_ty.const_zero())?;
        let h = m.add_global("h", i32_ty.const_zero())?;
        let g_ptr = g.as_global_constant_ptr();
        let h_ptr = h.as_global_constant_ptr();
        let null = ptr_ty.const_null().as_constant();

        let ugt =
            constant_fold_compare_instruction(CmpPredicate::Int(IntPredicate::Ugt), g_ptr, null)?
                .expect("global > null relation folds");
        assert_eq!(ugt, bool_ty.const_int(true).as_constant());

        let eq =
            constant_fold_compare_instruction(CmpPredicate::Int(IntPredicate::Eq), null, g_ptr)?
                .expect("null == global swapped relation folds");
        assert_eq!(eq, bool_ty.const_int(false).as_constant());

        let ne =
            constant_fold_compare_instruction(CmpPredicate::Int(IntPredicate::Ne), g_ptr, h_ptr)?
                .expect("distinct globals relation folds");
        assert_eq!(ne, bool_ty.const_int(true).as_constant());

        let same_ne = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Ne),
            g_ptr,
            g.as_global_constant_ptr(),
        )?
        .expect("fresh refs to the same global fold equal");
        assert_eq!(same_ne, bool_ty.const_int(false).as_constant());

        let same_eq = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Eq),
            g_ptr,
            g.as_global_constant_ptr(),
        )?
        .expect("fresh refs to the same global fold equal");
        assert_eq!(same_eq, bool_ty.const_int(true).as_constant());

        let void_ty = m.void_type();
        let fn_ty = m.fn_type_no_params(void_ty.as_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::Internal)?;
        let f_entry = f.append_basic_block(&m, "entry");
        let f_addr = m.block_address(f, &f_entry)?;
        let other = m.add_function_dyn("other", fn_ty, Linkage::Internal)?;
        let other_entry = other.append_basic_block(&m, "entry");
        let other_addr = m.block_address(other, &other_entry)?;

        let block_ne_null =
            constant_fold_compare_instruction(CmpPredicate::Int(IntPredicate::Ne), f_addr, null)?
                .expect("blockaddress != null relation folds");
        assert_eq!(block_ne_null, bool_ty.const_int(true).as_constant());

        let block_ne_other_function = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Ne),
            f_addr,
            other_addr,
        )?
        .expect("different-function blockaddresses fold");
        assert_eq!(
            block_ne_other_function,
            bool_ty.const_int(true).as_constant()
        );

        let global_ne_block =
            constant_fold_compare_instruction(CmpPredicate::Int(IntPredicate::Ne), g_ptr, f_addr)?
                .expect("global != blockaddress relation folds");
        assert_eq!(global_ne_block, bool_ty.const_int(true).as_constant());

        let gep = m.constant_expr_with_options(
            ptr_ty.as_type(),
            ConstantExprOpcode::GetElementPtr,
            [g_ptr.as_value(), m.i64_type().const_int(1_i64).as_value()],
            [],
            [],
            ConstantExprOptions::new()
                .source_ty(i32_ty.as_type())
                .flags(ConstantExprFlags::gep(GepNoWrapFlags::inbounds())),
        )?;
        let gep_ne_null =
            constant_fold_compare_instruction(CmpPredicate::Int(IntPredicate::Ne), gep, null)?
                .expect("inbounds global GEP != null relation folds");
        assert_eq!(gep_ne_null, bool_ty.const_int(true).as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::areGlobalsPotentiallyEqual` lines 957-979 and
/// `evaluateICmpRelation` lines 1027-1044: ifuncs are `GlobalValue`s, so
/// non-interposable ifuncs can compare not-equal while interposable/external
/// weak ifuncs must not be folded.
#[test]
fn compare_ifunc_linkage_relations_match_globalvalue_rules() -> Result<(), IrError> {
    Module::with_new("fold-compare-ifunc", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let resolver = m.add_global("resolver", i32_ty.const_zero())?;

        let internal_a = m
            .ifunc_builder("internal_a", i32_ty.as_type(), resolver)
            .linkage(Linkage::Internal)
            .build()?;
        let internal_b = m
            .ifunc_builder("internal_b", i32_ty.as_type(), resolver)
            .linkage(Linkage::Internal)
            .build()?;
        let safe_ne = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Ne),
            internal_a.as_global_constant_ptr(),
            internal_b.as_global_constant_ptr(),
        )?
        .expect("distinct non-interposable ifuncs fold");
        assert_eq!(safe_ne, bool_ty.const_int(true).as_constant());

        let weak_a = m
            .ifunc_builder("weak_a", i32_ty.as_type(), resolver)
            .linkage(Linkage::WeakAny)
            .build()?;
        let weak_b = m
            .ifunc_builder("weak_b", i32_ty.as_type(), resolver)
            .linkage(Linkage::WeakAny)
            .build()?;
        assert!(
            constant_fold_compare_instruction(
                CmpPredicate::Int(IntPredicate::Ne),
                weak_a.as_global_constant_ptr(),
                weak_b.as_global_constant_ptr(),
            )?
            .is_none()
        );

        let external_weak = m
            .ifunc_builder("external_weak", i32_ty.as_type(), resolver)
            .linkage(Linkage::ExternalWeak)
            .build()?;
        let null = m.ptr_type(0).const_null().as_constant();
        assert!(
            constant_fold_compare_instruction(
                CmpPredicate::Int(IntPredicate::Ne),
                external_weak.as_global_constant_ptr(),
                null,
            )?
            .is_none()
        );
        Ok(())
    })
}

/// Port of `Type.cpp::Type::isEmptyTy` lines 180-194 as consumed by
/// `ConstantFold.cpp::areGlobalsPotentiallyEqual`: arrays with empty element
/// types and structs whose fields are all empty remain empty for global
/// equality folding.
#[test]
fn compare_globals_with_recursive_empty_value_type_declines() -> Result<(), IrError> {
    Module::with_new("fold-compare-empty-global", |m| {
        let i8_ty = m.i8_type();
        let empty_array_ty = m.array_type(i8_ty.as_type(), 0);
        let nested_array_ty = m.array_type(empty_array_ty.as_type(), 1);
        let nested_g = m.add_global("nested_g", nested_array_ty.as_type().get_undef())?;
        let nested_h = m.add_global("nested_h", nested_array_ty.as_type().get_undef())?;
        assert!(
            constant_fold_compare_instruction(
                CmpPredicate::Int(IntPredicate::Ne),
                nested_g.as_global_constant_ptr(),
                nested_h.as_global_constant_ptr(),
            )?
            .is_none()
        );

        let wrapper_ty = m.struct_type([empty_array_ty.as_type()], false);
        let wrapper_g = m.add_global("wrapper_g", wrapper_ty.as_type().get_undef())?;
        let wrapper_h = m.add_global("wrapper_h", wrapper_ty.as_type().get_undef())?;
        assert!(
            constant_fold_compare_instruction(
                CmpPredicate::Int(IntPredicate::Ne),
                wrapper_g.as_global_constant_ptr(),
                wrapper_h.as_global_constant_ptr(),
            )?
            .is_none()
        );
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::areGlobalsPotentiallyEqual` lines 959-961:
/// `hasGlobalUnnamedAddr` rejects only `unnamed_addr`, not
/// `local_unnamed_addr`.
#[test]
fn compare_local_unnamed_addr_globals_still_fold_not_equal() -> Result<(), IrError> {
    Module::with_new("fold-compare-local-unnamed-addr", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let local_g = m
            .global_builder("local_g", i32_ty.as_type())
            .unnamed_addr(UnnamedAddr::Local)
            .initializer(i32_ty.const_zero())
            .build()?;
        let local_h = m
            .global_builder("local_h", i32_ty.as_type())
            .unnamed_addr(UnnamedAddr::Local)
            .initializer(i32_ty.const_zero())
            .build()?;
        let local_ne = constant_fold_compare_instruction(
            CmpPredicate::Int(IntPredicate::Ne),
            local_g.as_global_constant_ptr(),
            local_h.as_global_constant_ptr(),
        )?
        .expect("local_unnamed_addr globals still fold not-equal");
        assert_eq!(local_ne, bool_ty.const_int(true).as_constant());

        let global_g = m
            .global_builder("global_g", i32_ty.as_type())
            .unnamed_addr(UnnamedAddr::Global)
            .initializer(i32_ty.const_zero())
            .build()?;
        let global_h = m
            .global_builder("global_h", i32_ty.as_type())
            .unnamed_addr(UnnamedAddr::Global)
            .initializer(i32_ty.const_zero())
            .build()?;
        assert!(
            constant_fold_compare_instruction(
                CmpPredicate::Int(IntPredicate::Ne),
                global_g.as_global_constant_ptr(),
                global_h.as_global_constant_ptr(),
            )?
            .is_none()
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldSelectInstruction`
/// lines 256-331: undef and poison conditions fold before the scalar equal-arm
/// simplification, while equal non-poison arms fold to that shared arm.
#[test]
fn select_undef_poison_and_equal_arm_rules_fold() -> Result<(), IrError> {
    Module::with_new("fold-select-undef", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let undef_cond = bool_ty.as_type().get_undef().as_constant();
        let poison_cond = bool_ty.as_type().get_poison().as_constant();
        let undef_arm = i32_ty.as_type().get_undef().as_constant();
        let seven = i32_ty.const_int(7_i32).as_constant();

        let undef_select = constant_fold_select_instruction(undef_cond, undef_arm, seven)?
            .expect("undef condition with undef true arm folds");
        assert_eq!(undef_select, undef_arm);

        let undef_select = constant_fold_select_instruction(undef_cond, seven, undef_arm)?
            .expect("undef condition with defined true arm folds to false arm");
        assert_eq!(undef_select, undef_arm);

        let poison_select = constant_fold_select_instruction(poison_cond, seven, seven)?
            .expect("poison condition folds before equal arms");
        assert_eq!(poison_select, i32_ty.as_type().get_poison().as_constant());

        let equal_arms =
            constant_fold_select_instruction(bool_ty.const_int(true).as_constant(), seven, seven)?
                .expect("equal select arms fold");
        assert_eq!(equal_arms, seven);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldSelectInstruction`
/// lines 307-329: direct global variables are not-poison constants, so an
/// undef arm can fold to the direct global arm.
#[test]
fn select_undef_arm_with_direct_global_arm_folds_to_global() -> Result<(), IrError> {
    Module::with_new("fold-select-direct-global", |m| {
        let i1_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let cond = m.constant_expr(
            i1_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let undef_arm = g.as_constant().ty().get_undef().as_constant();

        let folded = constant_fold_select_instruction(cond, undef_arm, g.as_constant())?
            .expect("undef arm folds to not-poison direct global");
        assert_eq!(folded, g.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldSelectInstruction`
/// lines 262-289: fixed-vector select folds each lane, including undef
/// condition lanes, poison condition lanes, and equal true/false arm lanes.
#[test]
fn select_vector_undef_poison_and_equal_lanes_rebuild_result() -> Result<(), IrError> {
    Module::with_new("fold-select-vector", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let cond_ty = m.vector_type(bool_ty.as_type(), 3, false);
        let vec_ty = m.vector_type(i32_ty.as_type(), 3, false);
        let condition = cond_ty.const_vector::<Constant<'_>, _>([
            bool_ty.as_type().get_undef().as_constant(),
            bool_ty.as_type().get_poison().as_constant(),
            bool_ty.const_int(true).as_constant(),
        ])?;
        let true_value = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(11_i32),
            i32_ty.const_int(22_i32),
            i32_ty.const_int(33_i32),
        ])?;
        let false_value = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(44_i32),
            i32_ty.const_int(55_i32),
            i32_ty.const_int(33_i32),
        ])?;

        let folded = constant_fold_select_instruction(
            condition.as_constant(),
            true_value.as_constant(),
            false_value.as_constant(),
        )?
        .expect("fixed-vector select folds per lane");
        let lane_zero = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(0_i32).as_constant(),
        )?
        .expect("undef-condition lane extracts");
        let lane_one = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(1_i32).as_constant(),
        )?
        .expect("poison-condition lane extracts");
        let lane_two = constant_fold_extract_element_instruction(
            folded,
            i32_ty.const_int(2_i32).as_constant(),
        )?
        .expect("equal-arm lane extracts");

        assert_eq!(lane_zero, i32_ty.const_int(44_i32).as_constant());
        assert_eq!(lane_one, i32_ty.as_type().get_poison().as_constant());
        assert_eq!(lane_two, i32_ty.const_int(33_i32).as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldSelectInstruction`
/// lines 258-289: vector all-true/all-false conditions fold before scalable
/// iteration is declined, and fixed-vector arms are lane-extracted through
/// `ConstantExpr::getExtractElement`.
#[test]
fn select_vector_shortcuts_and_constant_expr_arms_fold() -> Result<(), IrError> {
    Module::with_new("fold-select-vector-constexpr", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();

        let scalable_cond_ty = m.vector_type(bool_ty.as_type(), 2, true);
        let scalable_value_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let true_lane = bool_ty.const_int(true).as_constant();
        let all_true = scalable_cond_ty.const_vector::<Constant<'_>, _>([true_lane, true_lane])?;
        let true_value = scalable_value_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(11_i32),
            i32_ty.const_int(11_i32),
        ])?;
        let false_value = scalable_value_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(22_i32),
            i32_ty.const_int(22_i32),
        ])?;
        let folded = constant_fold_select_instruction(
            all_true.as_constant(),
            true_value.as_constant(),
            false_value.as_constant(),
        )?
        .expect("scalable all-true condition selects true arm");
        assert_eq!(folded, true_value.as_constant());

        let cond_ty = m.vector_type(bool_ty.as_type(), 2, false);
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let condition = cond_ty.const_vector::<Constant<'_>, _>([
            bool_ty.const_int(true).as_constant(),
            bool_ty.const_int(false).as_constant(),
        ])?;
        let true_expr = m.constant_expr(
            vec_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [i64_ty.const_int(42_i64).as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let false_vec = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(5_i32),
            i32_ty.const_int(6_i32),
        ])?;
        let folded = constant_fold_select_instruction(
            condition.as_constant(),
            true_expr,
            false_vec.as_constant(),
        )?
        .expect("fixed-vector select with constexpr arm folds");
        let zero = i32_ty.const_zero().as_constant();
        let lane_zero = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::ExtractElement,
            [true_expr.as_value(), zero.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let expected = vec_ty
            .const_vector::<Constant<'_>, _>([lane_zero, i32_ty.const_int(6_i32).as_constant()])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp` lines 398-438, 440-497, and
/// 511-538: insertelement, shufflevector, and nested insertvalue rebuild
/// constants from their extracted elements rather than declining the fold.
#[test]
fn vector_and_aggregate_rebuilders_materialize_constants() -> Result<(), IrError> {
    Module::with_new("fold-rebuilders", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let undef_vec = vec_ty.as_type().get_undef().as_constant();
        let inserted = i32_ty.const_int(42_i32).as_constant();
        let rebuilt_vec = constant_fold_insert_element_instruction(
            undef_vec,
            inserted,
            i32_ty.const_int(0_i32).as_constant(),
        )?
        .expect("insertelement rebuilds undef vector");
        let lane_zero = constant_fold_extract_element_instruction(
            rebuilt_vec,
            i32_ty.const_int(0_i32).as_constant(),
        )?
        .expect("inserted lane extracts");
        let lane_one = constant_fold_extract_element_instruction(
            rebuilt_vec,
            i32_ty.const_int(1_i32).as_constant(),
        )?
        .expect("preserved undef lane extracts");
        assert_eq!(lane_zero, inserted);
        assert_eq!(lane_one, i32_ty.as_type().get_undef().as_constant());

        let lhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let rhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
        ])?;
        let shuffled = constant_fold_shuffle_vector_instruction(
            lhs.as_constant(),
            rhs.as_constant(),
            &[0, 4],
        )?
        .expect("shufflevector rebuilds selected lanes");
        let shuffle_lane_one = constant_fold_extract_element_instruction(
            shuffled,
            i32_ty.const_int(1_i32).as_constant(),
        )?
        .expect("out-of-range shuffle lane extracts");
        assert_eq!(shuffle_lane_one, i32_ty.as_type().get_undef().as_constant());

        let inner_ty = m.array_type(i32_ty.as_type(), 2);
        let outer_ty = m.array_type(inner_ty.as_type(), 2);
        let row_zero = inner_ty.const_array::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let row_one = inner_ty.const_array::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
        ])?;
        let aggregate = outer_ty
            .const_array::<Constant<'_>, _>([row_zero.as_constant(), row_one.as_constant()])?;
        let rebuilt_aggregate = constant_fold_insert_value_instruction(
            aggregate.as_constant(),
            i32_ty.const_int(77_i32).as_constant(),
            &[1, 0],
        )?
        .expect("nested insertvalue rebuilds aggregate");
        let nested_inserted = constant_fold_extract_value_instruction(rebuilt_aggregate, &[1, 0])?
            .expect("nested inserted element extracts");
        let nested_preserved = constant_fold_extract_value_instruction(rebuilt_aggregate, &[1, 1])?
            .expect("nested preserved element extracts");
        assert_eq!(nested_inserted, i32_ty.const_int(77_i32).as_constant());
        assert_eq!(nested_preserved, i32_ty.const_int(4_i32).as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp` lines 423-437 and 481-492:
/// fixed-vector insertelement and shufflevector rebuild non-aggregate vector
/// constants through per-lane `extractelement` constant expressions.
#[test]
fn vector_rebuilders_extract_lanes_from_non_aggregate_constants() -> Result<(), IrError> {
    Module::with_new("fold-rebuilders-constexpr-vectors", |m| {
        let i64_ty = m.i64_type();
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let base = m.constant_expr(
            vec_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [i64_ty.const_int(42_i64).as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let inserted = i32_ty.const_int(7_i32).as_constant();
        let rebuilt = constant_fold_insert_element_instruction(
            base,
            inserted,
            i32_ty.const_zero().as_constant(),
        )?
        .expect("insertelement rebuilds constexpr vector");
        let lane_one_index = i32_ty.const_int(1_i32).as_constant();
        let expected_lane_one = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::ExtractElement,
            [base.as_value(), lane_one_index.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let lane_one = constant_fold_extract_element_instruction(rebuilt, lane_one_index)?
            .expect("rebuilt lane extracts");
        assert_eq!(lane_one, expected_lane_one);

        let shuffled = constant_fold_shuffle_vector_instruction(base, base, &[1, 0])?
            .expect("shufflevector rebuilds constexpr vector");
        let first_lane =
            constant_fold_extract_element_instruction(shuffled, i32_ty.const_zero().as_constant())?
                .expect("shuffled lane extracts");
        assert_eq!(first_lane, expected_lane_one);
        Ok(())
    })
}

/// Port of `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, Integer_i1)`
/// lines 62-132: i1 binary constant folding matches upstream's special cases.
#[test]
fn constants_test_integer_i1_binary_folds() -> Result<(), IrError> {
    Module::with_new("fold-i1", |m| {
        let i1_ty = m.bool_type();
        let one = i1_ty.const_int(true).as_constant();
        let zero = i1_ty.const_int(false).as_constant();
        let neg_one = i1_ty.const_all_ones().as_constant();
        let poison = i1_ty.as_type().get_poison().as_constant();

        for (opcode, lhs, rhs) in [
            (ConstantExprOpcode::Add, one, one),
            (ConstantExprOpcode::Add, neg_one, one),
            (ConstantExprOpcode::Add, neg_one, neg_one),
            (ConstantExprOpcode::Sub, neg_one, one),
            (ConstantExprOpcode::Sub, one, neg_one),
            (ConstantExprOpcode::Sub, one, one),
        ] {
            let expr = m.constant_expr(
                i1_ty.as_type(),
                opcode,
                [lhs.as_value(), rhs.as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )?;
            assert_eq!(expr, zero, "{opcode:?}");
        }

        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::Shl, one, one)?
                .expect("i1 shl by one folds"),
            poison
        );
        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::Shl, one, zero)?
                .expect("i1 shl by zero folds"),
            one
        );
        assert_eq!(
            constant_fold_binary_instruction(BinaryOpcode::Mul, neg_one, one)?
                .expect("i1 mul folds"),
            one
        );
        for (opcode, lhs, rhs) in [
            (BinaryOpcode::SDiv, neg_one, one),
            (BinaryOpcode::SDiv, one, neg_one),
            (BinaryOpcode::UDiv, neg_one, one),
            (BinaryOpcode::UDiv, one, neg_one),
        ] {
            assert_eq!(
                constant_fold_binary_instruction(opcode, lhs, rhs)?.expect("i1 div folds"),
                one,
                "{opcode:?}"
            );
        }
        for (lhs, rhs) in [(neg_one, one), (one, neg_one)] {
            assert_eq!(
                constant_fold_binary_instruction(BinaryOpcode::SRem, lhs, rhs)?
                    .expect("i1 srem folds"),
                zero
            );
        }
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// lines 926-947: i1 special cases apply to all i1 constants, including
/// non-`ConstantInt` constant expressions.
#[test]
fn i1_constant_expr_binary_special_cases_fold() -> Result<(), IrError> {
    Module::with_new("fold-i1-constexpr", |m| {
        let i1_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr_as_i1 = m.constant_expr(
            i1_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let one = i1_ty.const_int(true).as_constant();

        let add = constant_fold_binary_instruction(BinaryOpcode::Add, ptr_as_i1, one)?
            .expect("i1 constexpr add folds to xor");
        let expected_add = m.constant_expr(
            i1_ty.as_type(),
            ConstantExprOpcode::Xor,
            [ptr_as_i1.as_value(), one.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(add, expected_add);

        let sdiv = constant_fold_binary_instruction(BinaryOpcode::SDiv, ptr_as_i1, one)?
            .expect("i1 constexpr sdiv by one folds to lhs");
        assert_eq!(sdiv, ptr_as_i1);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// lines 871-883: vector splats use `ConstantExpr::get` for desirable scalar
/// binops, so non-foldable scalar constant expressions still produce a splat.
#[test]
fn vector_splat_desirable_binop_builds_splat_constant_expr() -> Result<(), IrError> {
    Module::with_new("fold-vector-splat-desirable-binop", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let g = m.add_global("g", i32_ty.const_zero())?;
        let ptr_as_i32 = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let one = i32_ty.const_int(1_i32).as_constant();
        let lhs = vec_ty.const_vector::<Constant<'_>, _>([ptr_as_i32, ptr_as_i32])?;
        let rhs = vec_ty.const_vector::<Constant<'_>, _>([one, one])?;

        let folded = constant_fold_binary_instruction(
            BinaryOpcode::Add,
            lhs.as_constant(),
            rhs.as_constant(),
        )?
        .expect("vector splat desirable binop folds to splat constexpr");
        let scalar = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::Add,
            [ptr_as_i32.as_value(), one.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let expected = vec_ty.const_vector::<Constant<'_>, _>([scalar, scalar])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldBinaryInstruction`
/// lines 620-716: scalable-vector undef operands use the scalar/scalable undef
/// rules before fixed-vector element iteration is declined.
#[test]
fn scalable_vector_fp_undef_binary_folds_to_nan_splat() -> Result<(), IrError> {
    Module::with_new("fold-scalable-fp-undef-binop", |m| {
        let f32_ty = m.f32_type();
        let vec_ty = m.vector_type(f32_ty.as_type(), 2, true);
        let undef = vec_ty.as_type().get_undef().as_constant();
        let zero = f32_ty.const_float(0.0).as_constant();
        let rhs = vec_ty.const_vector::<Constant<'_>, _>([zero, zero])?;

        let folded =
            constant_fold_binary_instruction(BinaryOpcode::FAdd, undef, rhs.as_constant())?
                .expect("scalable vector fp undef binop folds");
        let lane_zero = constant_fold_extract_element_instruction(
            folded,
            m.i32_type().const_zero().as_constant(),
        )?
        .expect("folded NaN splat extracts lane zero");
        let lane_zero = ConstantFloatValue::<FloatDyn>::try_from(lane_zero)?;
        assert!(lane_zero.ap_float().is_nan());
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/lib/IR/ConstantFold.cpp` lines 540-596:
/// `fneg` preserves scalar/scalable undef and folds fixed vectors lane-wise,
/// including splat vectors.
#[test]
fn constant_fold_unary_fneg_undef_and_vector_elements() -> Result<(), IrError> {
    Module::with_new("fold-fneg-vector", |m| {
        let f32_ty = m.f32_type();
        let vec_ty = m.vector_type(f32_ty.as_type(), 2, false);
        let one = f32_ty.const_float(1.0).as_constant();
        let neg_one = f32_ty.const_float(-1.0).as_constant();
        let undef = f32_ty.as_type().get_undef().as_constant();

        let scalar = constant_fold_unary_instruction(UnaryOpcode::FNeg, undef)?
            .expect("scalar fneg undef folds");
        assert_eq!(scalar, undef);

        let vector_undef = vec_ty.as_type().get_undef().as_constant();
        let folded = constant_fold_unary_instruction(UnaryOpcode::FNeg, vector_undef)?
            .expect("fixed-vector fneg undef folds lane-wise");
        let expected = vec_ty.const_vector::<Constant<'_>, _>([undef, undef])?;
        assert_eq!(folded, expected.as_constant());

        let vector = vec_ty.const_vector::<Constant<'_>, _>([one, undef])?;
        let folded = constant_fold_unary_instruction(UnaryOpcode::FNeg, vector.as_constant())?
            .expect("fixed-vector fneg folds");
        let lane_zero = constant_fold_extract_element_instruction(
            folded,
            m.i32_type().const_int(0_i32).as_constant(),
        )?
        .expect("lane zero extracts");
        let lane_one = constant_fold_extract_element_instruction(
            folded,
            m.i32_type().const_int(1_i32).as_constant(),
        )?
        .expect("lane one extracts");
        assert_eq!(lane_zero, neg_one);
        assert_eq!(lane_one, undef);

        let splat = vec_ty.const_vector::<Constant<'_>, _>([one, one])?;
        let folded = constant_fold_unary_instruction(UnaryOpcode::FNeg, splat.as_constant())?
            .expect("splat fneg folds");
        let expected = vec_ty.const_vector::<Constant<'_>, _>([neg_one, neg_one])?;
        assert_eq!(folded, expected.as_constant());

        let scalable_ty = m.vector_type(f32_ty.as_type(), 2, true);
        let scalable_splat = scalable_ty.const_vector::<Constant<'_>, _>([one, one])?;
        let folded =
            constant_fold_unary_instruction(UnaryOpcode::FNeg, scalable_splat.as_constant())?
                .expect("scalable splat fneg folds");
        let expected = scalable_ty.const_vector::<Constant<'_>, _>([neg_one, neg_one])?;
        assert_eq!(folded, expected.as_constant());
        let scalable_undef = scalable_ty.as_type().get_undef().as_constant();
        let folded = constant_fold_unary_instruction(UnaryOpcode::FNeg, scalable_undef)?
            .expect("scalable undef fneg folds");
        assert_eq!(folded, scalable_undef);
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/lib/IR/ConstantFold.cpp` lines 1310-1340:
/// poison/undef bases use the computed GEP result type, no-op scalar GEPs
/// fold to the base, and scalar base plus vector zero index splats the base.
#[test]
fn constant_fold_gep_poison_undef_and_noop_indices() -> Result<(), IrError> {
    Module::with_new("fold-gep-noop", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let ptr_ty = m.ptr_type(0);
        let g = m.add_global("g", i32_ty.const_zero())?;
        let base = g.as_global_constant_ptr();
        let zero = i64_ty.const_zero().as_constant();
        let undef_index = i64_ty.as_type().get_undef().as_constant();

        let poison = constant_fold_get_element_ptr(
            i32_ty.as_type(),
            ptr_ty.as_type().get_poison().as_constant(),
            &[zero],
            None,
        )?
        .expect("poison-base GEP folds");
        assert_eq!(poison, ptr_ty.as_type().get_poison().as_constant());

        let undef = constant_fold_get_element_ptr(
            i32_ty.as_type(),
            ptr_ty.as_type().get_undef().as_constant(),
            &[zero],
            None,
        )?
        .expect("undef-base GEP folds");
        assert_eq!(undef, ptr_ty.as_type().get_undef().as_constant());

        let folded = constant_fold_get_element_ptr(i32_ty.as_type(), base, &[zero], None)?
            .expect("zero GEP folds");
        assert_eq!(folded, base);
        let folded = constant_fold_get_element_ptr(i32_ty.as_type(), base, &[undef_index], None)?
            .expect("undef-index no-op GEP folds");
        assert_eq!(folded, base);

        let vec_i64_ty = m.vector_type(i64_ty.as_type(), 2, false);
        let vec_zero = vec_i64_ty.const_vector::<ConstantIntValue<'_, i64>, _>([
            i64_ty.const_zero(),
            i64_ty.const_zero(),
        ])?;
        let folded =
            constant_fold_get_element_ptr(i32_ty.as_type(), base, &[vec_zero.as_constant()], None)?
                .expect("scalar base plus vector zero index folds");
        let vec_ptr_ty = m.vector_type(ptr_ty.as_type(), 2, false);
        let vector_poison = constant_fold_get_element_ptr(
            i32_ty.as_type(),
            ptr_ty.as_type().get_poison().as_constant(),
            &[vec_zero.as_constant()],
            None,
        )?
        .expect("poison-base vector GEP folds");
        assert_eq!(
            vector_poison,
            vec_ptr_ty.as_type().get_poison().as_constant()
        );
        let vector_undef = constant_fold_get_element_ptr(
            i32_ty.as_type(),
            ptr_ty.as_type().get_undef().as_constant(),
            &[vec_zero.as_constant()],
            None,
        )?
        .expect("undef-base vector GEP folds");
        assert_eq!(vector_undef, vec_ptr_ty.as_type().get_undef().as_constant());
        let expected = vec_ptr_ty.const_vector::<Constant<'_>, _>([base, base])?;
        assert_eq!(folded, expected.as_constant());
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/lib/IR/ConstantFold.cpp` lines 1324-1334:
/// all-zero GEPs with `inrange` are not folded because upstream avoids losing
/// the `inrange` information.
#[test]
fn constant_fold_gep_inrange_noop_does_not_fold() -> Result<(), IrError> {
    Module::with_new("fold-gep-inrange", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let g = m.add_global("g", i32_ty.const_zero())?;
        let base = g.as_global_constant_ptr();
        let in_range = ConstantExprInRange::new([0_u64], [1_u64], 64);
        let expr = m.constant_expr_with_options(
            base.ty(),
            ConstantExprOpcode::GetElementPtr,
            [base.as_value(), i64_ty.const_zero().as_value()],
            [],
            [],
            ConstantExprOptions::new()
                .source_ty(i32_ty.as_type())
                .flags(ConstantExprFlags::gep_with_in_range(
                    GepNoWrapFlags::empty(),
                    in_range,
                )),
        )?;

        assert_ne!(expr, base);
        m.add_global("p", expr)?;
        let text = format!("{m}");
        assert!(
            text.contains("@p = global ptr getelementptr inrange(0, 1) (i32, ptr @g, i64 0)"),
            "{text}"
        );
        Ok(())
    })
}

/// Port of `unittests/IR/ConstantsTest.cpp` function-pointer alignment folding
/// cases lines 497-550: `ptrtoint(function) & mask` folds to zero exactly when
/// upstream can prove the low bits are clear from pointer/function alignment.
#[test]
fn function_pointer_and_mask_folds_from_alignment() -> Result<(), IrError> {
    assert!(function_ptr_and_mask_folds_to_zero(
        None,
        MaybeAlign::NONE,
        1_i32
    )?);
    assert!(function_ptr_and_mask_folds_to_zero(
        None,
        MaybeAlign::NONE,
        2_i32
    )?);
    assert!(!function_ptr_and_mask_folds_to_zero(
        None,
        MaybeAlign::NONE,
        4_i32
    )?);

    for layout in ["Fi32", "Fn32"] {
        assert!(function_ptr_and_mask_folds_to_zero(
            Some(layout),
            MaybeAlign::NONE,
            1_i32,
        )?);
        assert!(function_ptr_and_mask_folds_to_zero(
            Some(layout),
            MaybeAlign::NONE,
            2_i32,
        )?);
    }
    for layout in ["Fi8", "Fn8"] {
        assert!(!function_ptr_and_mask_folds_to_zero(
            Some(layout),
            MaybeAlign::NONE,
            2_i32,
        )?);
    }
    assert!(function_ptr_and_mask_folds_to_zero(
        Some("Fn8"),
        MaybeAlign::from(Align::new(4)?),
        2_i32,
    )?);
    assert!(!function_ptr_and_mask_folds_to_zero(
        Some("Fi8"),
        MaybeAlign::from(Align::new(4)?),
        2_i32,
    )?);
    Ok(())
}

/// Port of `ConstantFold.cpp::ConstantFoldBinaryInstruction` lines 740-779:
/// commutative integer ops commute constant masks before global pointer
/// alignment folding.
#[test]
fn commuted_global_pointer_mask_folds_to_null() -> Result<(), IrError> {
    Module::with_new("fold-commuted-global-ptr-mask", |m| {
        let i32_ty = m.i32_type();
        let g = m
            .global_builder("g", i32_ty.as_type())
            .align(MaybeAlign::from(Align::new(4)?))
            .initializer(i32_ty.const_zero())
            .build()?;
        let ptr_as_int = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let folded = constant_fold_binary_instruction(
            BinaryOpcode::And,
            i32_ty.const_int(2_i32).as_constant(),
            ptr_as_int,
        )?
        .expect("commuted global pointer mask folds");
        assert_eq!(folded, i32_ty.const_zero().as_constant());
        Ok(())
    })
}

/// Port of `ConstantFold.cpp::ConstantFoldBinaryInstruction` lines 724-742:
/// `and` with a zero mask folds through the integer absorber before global
/// pointer alignment is consulted.
#[test]
fn global_pointer_zero_mask_folds_without_alignment() -> Result<(), IrError> {
    Module::with_new("fold-global-ptr-zero-mask", |m| {
        let i32_ty = m.i32_type();
        let g = m
            .global_builder("g", i32_ty.as_type())
            .initializer(i32_ty.const_zero())
            .build()?;
        let ptr_as_int = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let zero = i32_ty.const_zero().as_constant();
        for (lhs, rhs) in [(ptr_as_int, zero), (zero, ptr_as_int)] {
            let folded = constant_fold_binary_instruction(BinaryOpcode::And, lhs, rhs)?
                .expect("zero mask folds");
            assert_eq!(folded, zero);
        }
        Ok(())
    })
}

/// Port of `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, FoldGlobalVariablePtr)`
/// lines 559-579: aligned global-variable `ptrtoint` and `ptrtoaddr` low-bit
/// masks fold to integer zero.
#[test]
fn global_variable_ptrtoint_and_ptrtoaddr_and_mask_fold_to_null() -> Result<(), IrError> {
    Module::with_new("fold-global-ptr-mask", |m| {
        let i32_ty = m.i32_type();
        let g = m
            .global_builder("g", i32_ty.as_type())
            .align(MaybeAlign::from(Align::new(4)?))
            .initializer(i32_ty.const_zero())
            .build()?;
        let mask = i32_ty.const_int(2_i32).as_constant();
        for opcode in [ConstantExprOpcode::PtrToInt, ConstantExprOpcode::PtrToAddr] {
            let ptr_as_int = m.constant_expr(
                i32_ty.as_type(),
                opcode,
                [g.as_global_constant_ptr().as_value()],
                [],
                [],
                ConstantExprFlags::none(),
            )?;
            let folded = constant_fold_binary_instruction(BinaryOpcode::And, ptr_as_int, mask)?
                .expect("global pointer low-bit mask folds");
            assert_eq!(folded, i32_ty.const_zero().as_constant(), "{opcode:?}");
        }
        Ok(())
    })
}

/// Port of `Value.cpp::Value::getPointerAlignment` lines 974-988 as reached
/// from `ConstantFold.cpp::ConstantFoldBinaryInstruction`: an unannotated
/// defined global variable gets DataLayout-derived pointer alignment for low
/// bit-mask folding.
#[test]
fn global_variable_ptrtoint_mask_uses_implicit_datalayout_alignment() -> Result<(), IrError> {
    Module::with_new("fold-global-ptr-mask-implicit-align", |m| {
        let i32_ty = m.i32_type();
        let g = m
            .global_builder("g", i32_ty.as_type())
            .initializer(i32_ty.const_zero())
            .build()?;
        let ptr_as_int = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let folded = constant_fold_binary_instruction(
            BinaryOpcode::And,
            ptr_as_int,
            i32_ty.const_int(2_i32).as_constant(),
        )?
        .expect("DataLayout-derived i32 global alignment clears bit 1");
        assert_eq!(folded, i32_ty.const_zero().as_constant());
        Ok(())
    })
}

fn function_ptr_and_mask_folds_to_zero(
    layout: Option<&str>,
    function_align: MaybeAlign,
    mask: i32,
) -> Result<bool, IrError> {
    Module::with_new("fold-function-ptr-mask", |m| {
        if let Some(layout) = layout {
            m.set_data_layout(layout)?;
        }
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m
            .function_builder::<(), _>("f", fn_ty)
            .align(function_align)
            .build()?;
        let ptr_as_int = m.constant_expr(
            i32_ty.as_type(),
            ConstantExprOpcode::PtrToInt,
            [f.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let folded = constant_fold_binary_instruction(
            BinaryOpcode::And,
            ptr_as_int,
            i32_ty.const_int(mask).as_constant(),
        )?;
        Ok(folded == Some(i32_ty.const_zero().as_constant()))
    })
}
