//! Target-independent constant-folding tests.
//!
//! Source-derived subsets from `llvm/lib/IR/ConstantFold.cpp` plus exact
//! assembler excerpts where cited explicitly.

use llvmkit_ir::instr_types::CastOpcode;
use llvmkit_ir::{
    ApFloat, ApFloatSemantics, ApInt, BinaryOpcode, CmpPredicate, Constant, ConstantExprFlags,
    ConstantExprOpcode, ConstantExprOptions, ConstantFloatValue, ConstantIntValue, FloatDyn,
    FloatPredicate, IRBuilder, InstructionView, IntDyn, IntPredicate, IrError, Linkage, Module,
    NoFolder, RoundingMode, Type, UDivFlags, Width, constant_fold_binary_instruction,
    constant_fold_cast_instruction, constant_fold_compare_instruction,
    constant_fold_extract_element_instruction, constant_fold_extract_value_instruction,
    constant_fold_get_element_ptr, constant_fold_insert_element_instruction,
    constant_fold_insert_value_instruction, constant_fold_instruction,
    constant_fold_select_instruction, constant_fold_shuffle_vector_instruction,
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

        let zero = ty.const_zero().as_constant();
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
        m.add_global("sum", ty.as_type(), expr)?;
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
        let g = m.add_global("g", i32_ty.as_type(), i32_ty.const_zero())?;
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
        let g = m.add_global("g", ty.as_type(), ty.const_zero())?;
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
        let g = m.add_global("g", ty.as_type(), ty.const_zero())?;
        let base = g.as_global_constant_ptr();
        let folded =
            constant_fold_get_element_ptr(ty.as_type(), base, &[])?.expect("empty-index GEP folds");
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
        let fn_ty = m.fn_type(ty, Vec::<Type>::new(), false);
        let f = m.add_function::<Width<257>, _>("wide", fn_ty, Linkage::External)?;
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
        let fn_ty = m.fn_type(ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("exact", fn_ty, Linkage::External)?;
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
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFold.cpp::ConstantFoldShuffleVectorInstruction`:
/// a fixed-vector mask selects lanes from both operands and `-1` becomes poison.
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
        .expect("shuffle poison lane extracts");

        assert_eq!(lane_zero, i32_ty.const_int(2_i32).as_constant());
        assert_eq!(lane_one, i32_ty.const_int(3_i32).as_constant());
        assert_eq!(lane_two, i32_ty.as_type().get_poison().as_constant());
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
        let i_as0 = m.add_global("i_as0", i32_ty.as_type(), i32_ty.const_zero())?;
        let cast_as0 = m.constant_expr(
            i64_ty.as_type(),
            ConstantExprOpcode::PtrToAddr,
            [i_as0.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        m.add_global("global_cast_as0", i64_ty.as_type(), cast_as0)?;
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
        m.add_global("global_cast_as1", i32_ty.as_type(), cast_as1)?;

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
