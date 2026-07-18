use llvmkit_ir::{
    Align, ApInt, AttrIndex, AttrKind, Attribute, AttributeStorage, Brand, CFGAnalyses,
    CallAttributeData, ConstantExprOpcode, ConstantExprOptions, DominatorTreeAnalysis,
    FunctionAnalysisManager, IRBuilder, InstructionView, IntValue, IrError, KnownBits,
    KnownBitsAnalysis, LShrFlags, Linkage, MetadataAttachmentKind, MetadataRef, Module, MulFlags,
    NoFolder, PointerValue, PreservedAnalyses, Ptr, Value, ValueTrackingQuery, Width,
    compute_known_bits, is_known_non_zero, is_known_one, is_known_zero,
};

fn zeros(width: usize) -> String {
    "0".repeat(width)
}

fn known<'a>(value: Value<'a>, query: &ValueTrackingQuery<'_, 'a>) -> Result<KnownBits, IrError> {
    compute_known_bits(value, query)
}

/// Port of `llvm/unittests/Analysis/ValueTrackingTest.cpp::TEST_F(ComputeKnownBitsTest, ComputeKnownBits)`
/// and `llvm/test/Analysis/ValueTracking/known-bits.ll` integer-operator cases.
#[test]
fn constants_and_integer_operators_compute_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-int", |m| {
        let i8_ty = m.i8_type();
        let fn_ty = m.fn_type_no_params(i8_ty, false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let c_aa = i8_ty.const_int(0xaa_u8);
        let c_0f = i8_ty.const_int(0x0f_u8);
        let c_03 = i8_ty.const_int(0x03_u8);
        let c_04 = i8_ty.const_int(0x04_u8);
        let c_08 = i8_ty.const_int(0x08_u8);
        let c_01 = i8_ty.const_int(0x01_u8);

        let and_v = b.build_int_and::<i8, _, _, _>(c_aa, c_0f, "and")?;
        let or_v = b.build_int_or::<i8, _, _, _>(c_aa, c_0f, "or")?;
        let xor_v = b.build_int_xor::<i8, _, _, _>(c_aa, c_0f, "xor")?;
        let add_v = b.build_int_add::<i8, _, _, _>(c_aa, c_01, "add")?;
        let mul_v = b.build_int_mul::<i8, _, _, _>(c_03, c_04, "mul")?;
        let shl_v = b.build_int_shl::<i8, _, _, _>(c_03, c_01, "shl")?;
        let lshr_v = b.build_int_lshr::<i8, _, _, _>(c_08, c_01, "lshr")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);

        assert_eq!(known(c_aa.as_value(), &query)?.to_string(), "10101010");
        assert_eq!(known(and_v.as_value(), &query)?.to_string(), "00001010");
        assert_eq!(known(or_v.as_value(), &query)?.to_string(), "10101111");
        assert_eq!(known(xor_v.as_value(), &query)?.to_string(), "10100101");
        assert_eq!(known(add_v.as_value(), &query)?.to_string(), "10101011");
        assert_eq!(known(mul_v.as_value(), &query)?.to_string(), "00001100");
        assert_eq!(known(shl_v.as_value(), &query)?.to_string(), "00000110");
        assert_eq!(known(lshr_v.as_value(), &query)?.to_string(), "00000100");
        assert!(is_known_non_zero(c_aa.as_value(), &query)?);
        assert!(!is_known_zero(c_aa.as_value(), &query)?);
        assert!(is_known_one(c_aa.as_value(), 7, &query)?);
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
/// signed `sdiv` / `srem` arms.
#[test]
fn signed_division_and_remainder_compute_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-signed-div-rem", |m| {
        let i8_ty = m.i8_type();
        let fn_ty = m.fn_type_no_params(i8_ty, false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let sdiv =
            b.build_int_sdiv::<i8, _, _, _>(i8_ty.const_int(-64_i8), i8_ty.const_int(2_i8), "sd")?;
        let srem =
            b.build_int_srem::<i8, _, _, _>(i8_ty.const_int(7_i8), i8_ty.const_int(4_i8), "sr")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert_eq!(known(sdiv.as_value(), &query)?.to_string(), "111?????");
        assert_eq!(known(srem.as_value(), &query)?.to_string(), "00000011");
        Ok(())
    })
}

/// Port of `llvm/unittests/Analysis/ValueTrackingTest.cpp::TEST_F(ComputeKnownBitsTest, ComputeKnownBits)`
/// cast/select/phi/freeze/icmp propagation shape.
#[test]
fn casts_select_phi_freeze_and_icmp_compute_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-flow", |m| {
        let i1_ty = m.bool_type();
        let i8_ty = m.i8_type();
        let i16_ty = m.i16_type();
        let fn_ty = m.fn_type(i8_ty, [i1_ty.as_type()], false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        // join(%p: i8): the phi is the block's head-phi parameter, seeded from
        // each predecessor by a block-argument `br`.
        let bwp = IRBuilder::new_for::<i8>(&m);
        let (join, params) = bwp.append_block_with_params(f, &[i8_ty.as_type()], "join")?;
        let join_label = join.label();

        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let cond: IntValue<bool> = f.param(0)?.try_into()?;
        let c_aa = i8_ty.const_int(0xaa_u8);
        let c_ae = i8_ty.const_int(0xae_u8);
        let c_aa_val: IntValue<i8> = c_aa.as_constant().try_into()?;
        let c_ae_val: IntValue<i8> = c_ae.as_constant().try_into()?;
        let select = b.build_select(cond, c_aa_val, c_ae_val, "sel")?;
        b.build_br_with_args(join_label, &[i8_ty.const_int(0x03_u8).as_value()])?;

        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(other);
        b.build_br_with_args(join_label, &[i8_ty.const_int(0x07_u8).as_value()])?;

        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(join);
        let phi = params[0];
        let trunc_src: IntValue<i16> = i16_ty.const_int(0x00f0_u16).as_constant().try_into()?;
        let zext_src: IntValue<i8> = c_aa.as_constant().try_into()?;
        let sext_src: IntValue<i8> = c_aa.as_constant().try_into()?;
        let bitcast_src: IntValue<i8> = i8_ty.const_int(0x5a_u8).as_constant().try_into()?;
        let trunc = b.build_trunc::<i16, i8, _>(trunc_src, i8_ty, "tr")?;
        let zext = b.build_zext::<i8, i16, _>(zext_src, i16_ty, "zext")?;
        let sext = b.build_sext::<i8, i16, _>(sext_src, i16_ty, "sext")?;
        let bitcast = b.build_bitcast_int_to_int::<i8, Width<8>, _, _>(
            bitcast_src,
            m.int_type_n::<8>(),
            "bc",
        )?;
        let freeze = b.build_freeze(c_aa, "fr")?;
        let cmp = b.build_icmp_eq::<i8, _, _, _>(c_aa, c_aa, "cmp")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);

        assert_eq!(known(select.as_value(), &query)?.to_string(), "10101?10");
        assert_eq!(known(phi, &query)?.to_string(), "00000?11");
        assert_eq!(known(trunc.as_value(), &query)?.to_string(), "11110000");
        assert_eq!(
            known(zext.as_value(), &query)?.to_string(),
            "0000000010101010"
        );
        assert_eq!(
            known(sext.as_value(), &query)?.to_string(),
            "1111111110101010"
        );
        assert_eq!(known(bitcast.as_value(), &query)?.to_string(), "01011010");
        assert_eq!(known(freeze.as_value(), &query)?.to_string(), "10101010");
        assert_eq!(known(cmp.as_value(), &query)?.to_string(), "1");
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::getKnownBitsFromAndXorOr`
/// low-bit refinements for `and`/`or`/`xor` with `x +/- odd`.
#[test]
fn bitwise_with_self_plus_odd_refines_low_bit() -> Result<(), IrError> {
    Module::with_new("vt-bitwise-odd", |m| {
        let i8_ty = m.i8_type();
        let fn_ty = m.fn_type(i8_ty, [i8_ty.as_type()], false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let x: IntValue<i8> = f.param(0)?.try_into()?;
        let x_plus_one = b.build_int_add::<i8, _, _, _>(x, i8_ty.const_int(1_u8), "x1")?;
        let and_v = b.build_int_and::<i8, _, _, _>(x, x_plus_one, "and")?;
        let or_v = b.build_int_or::<i8, _, _, _>(x, x_plus_one, "or")?;
        let xor_v = b.build_int_xor::<i8, _, _, _>(x, x_plus_one, "xor")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert!(known(and_v.as_value(), &query)?.is_known_zero(0));
        assert!(known(or_v.as_value(), &query)?.is_known_one(0));
        assert!(known(xor_v.as_value(), &query)?.is_known_one(0));
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsMul`: `mul nsw`
/// with the same operand on both sides has a non-negative result.
#[test]
fn mul_nsw_self_product_is_non_negative() -> Result<(), IrError> {
    Module::with_new("vt-mul-nsw", |m| {
        let i8_ty = m.i8_type();
        let fn_ty = m.fn_type(i8_ty, [i8_ty.as_type()], false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let x: IntValue<i8> = f.param(0)?.try_into()?;
        let square = b.build_int_mul_with_flags(x, x, MulFlags::new().nsw(), "square")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert!(known(square.as_value(), &query)?.is_known_zero(7));
        Ok(())
    })
}

/// Port of `llvm/test/Analysis/ValueTracking/dereferenceable-and-aligned.ll`
/// alignment low-zero-bit reasoning, plus null-pointer all-zero facts.
#[test]
fn pointer_null_and_alloca_alignment_compute_low_zero_bits() -> Result<(), IrError> {
    Module::with_new("vt-ptr", |m| {
        let ptr_ty = m.ptr_type(0);
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(ptr_ty.as_type(), false);
        let f = m.add_function::<Ptr, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let alloca = b.build_alloca_with_align(i32_ty, Align::new(16)?, "slot")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        let null_bits = known(ptr_ty.const_null().as_value(), &query)?;
        let ptr_width = usize::try_from(dl.pointer_size_in_bits(0)).expect("u32 fits usize");
        assert_eq!(null_bits.to_string(), zeros(ptr_width));

        let alloca_bits = known(alloca.as_value(), &query)?;
        for bit in 0..4 {
            assert!(
                alloca_bits.is_known_zero(bit),
                "bit {bit} should be zero: {alloca_bits}"
            );
        }
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromRangeMetadata`
/// and the three `llvm/test/Analysis/ValueTracking/known-bits-from-range-md.ll`
/// checks: `test0` and `test1` fold to `ret i1 true`; `test2` must not.
#[test]
fn load_range_metadata_matches_known_bits_fixture() -> Result<(), IrError> {
    Module::with_new("vt-range-md", |m| {
        let i1_ty = m.bool_type();
        let i8_ty = m.i8_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i1_ty, [ptr_ty.as_type()], false);

        let f0 = m.add_function::<bool, _>("test0", fn_ty, Linkage::External)?;
        let entry0 = f0.append_basic_block(&m, "entry");
        let b0 = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry0);
        let p0: PointerValue = f0.param(0)?.try_into()?;
        let val0 = b0.build_int_load::<i8, _, _>(p0, "val")?;
        let lo0 = m.metadata_constant(i8_ty.const_int(-50_i8));
        let hi0 = m.metadata_constant(i8_ty.const_int(0_i8));
        let range0 = m.metadata_tuple([MetadataRef(lo0), MetadataRef(hi0)]);
        let val0_inst = InstructionView::try_from(val0.as_value())?;
        val0_inst.set_metadata(MetadataAttachmentKind::Range, range0);
        let mask128 = i8_ty.const_ap_int(&ApInt::from_words(8, &[128]))?;
        let and0 = b0.build_int_and::<i8, _, _, _>(val0, mask128, "and")?;
        let cmp0 = b0.build_icmp_eq::<i8, _, _, _>(and0, mask128, "is.eq")?;
        b0.build_ret(cmp0)?;

        let f1 = m.add_function::<bool, _>("test1", fn_ty, Linkage::External)?;
        let entry1 = f1.append_basic_block(&m, "entry");
        let b1 = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry1);
        let p1: PointerValue = f1.param(0)?.try_into()?;
        let val1 = b1.build_int_load::<i8, _, _>(p1, "val")?;
        let lo1 = m.metadata_constant(i8_ty.const_int(64_i8));
        let hi1 = m.metadata_constant(i8_ty.const_ap_int(&ApInt::from_words(8, &[128]))?);
        let range1 = m.metadata_tuple([MetadataRef(lo1), MetadataRef(hi1)]);
        let val1_inst = InstructionView::try_from(val1.as_value())?;
        val1_inst.set_metadata(MetadataAttachmentKind::Range, range1);
        let mask64 = i8_ty.const_int(64_i8);
        let and1 = b1.build_int_and::<i8, _, _, _>(val1, mask64, "and")?;
        let cmp1 = b1.build_icmp_eq::<i8, _, _, _>(and1, mask64, "is.eq")?;
        b1.build_ret(cmp1)?;

        let f2 = m.add_function::<bool, _>("test2", fn_ty, Linkage::External)?;
        let entry2 = f2.append_basic_block(&m, "entry");
        let b2 = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry2);
        let p2: PointerValue = f2.param(0)?.try_into()?;
        let val2 = b2.build_int_load::<i8, _, _>(p2, "val")?;
        let lo2 = m.metadata_constant(i8_ty.const_int(64_i8));
        let hi2 = m.metadata_constant(i8_ty.const_ap_int(&ApInt::from_words(8, &[129]))?);
        let range2 = m.metadata_tuple([MetadataRef(lo2), MetadataRef(hi2)]);
        let val2_inst = InstructionView::try_from(val2.as_value())?;
        val2_inst.set_metadata(MetadataAttachmentKind::Range, range2);
        let and2 = b2.build_int_and::<i8, _, _, _>(val2, mask64, "and")?;
        let cmp2 = b2.build_icmp_eq::<i8, _, _, _>(and2, mask64, "is.eq")?;
        b2.build_ret(cmp2)?;

        m.verify_borrowed()?;
        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert_eq!(known(cmp0.as_value(), &query)?.to_string(), "1");
        assert_eq!(known(cmp1.as_value(), &query)?.to_string(), "1");
        assert_eq!(known(cmp2.as_value(), &query)?.to_string(), "?");
        let query_without_instr_info = ValueTrackingQuery::new(&dl).without_instruction_info();
        assert!(known(cmp0.as_value(), &query_without_instr_info)?.is_unknown());
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
/// call/invoke range-attribute arm, with print form from
/// `llvm/test/Assembler/amdgcn-intrinsic-attributes.ll`.
#[test]
fn call_return_range_attribute_contributes_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-call-range-attr", |m| {
        let i1_ty = m.bool_type();
        let i8_ty = m.i8_type();
        let callee_ty = m.fn_type_no_params(i8_ty, false);
        let callee = m.add_function::<i8, _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type_no_params(i1_ty, false);
        let caller = m.add_function::<bool, _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let mut return_attrs = AttributeStorage::new();
        return_attrs.add(
            AttrIndex::Return,
            Attribute::range(i8_ty.as_type(), ApInt::zero(8), ApInt::from_words(8, &[64]))
                .expect("valid range"),
        );
        let attrs = CallAttributeData::new(return_attrs, Box::new([]), AttributeStorage::new());
        let call = b
            .call_builder(callee)
            .call_attributes(attrs)
            .name("val")
            .build()?;
        let call_value = call.return_int_value();
        let masked =
            b.build_int_and::<i8, _, _, _>(call_value, i8_ty.const_int(0x80_u8), "masked")?;
        let cmp = b.build_icmp_eq::<i8, _, _, _>(masked, i8_ty.const_int(0_u8), "is.zero")?;
        b.build_ret(cmp)?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert_eq!(known(cmp.as_value(), &query)?.to_string(), "1");
        let text = format!("{m}");
        assert!(text.contains("call range(i8 0, 64) i8 @callee()"), "{text}");
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
/// `CallBase::getReturnedArgOperand` call/invoke arm.
#[test]
fn returned_argument_call_and_invoke_contribute_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-returned-arg", |m| {
        let i8_ty = m.i8_type();
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(i8_ty, [i8_ty.as_type()], false);
        let callee = m.add_function::<i8, _>("identity", callee_ty, Linkage::External)?;

        let mut arg_attr = AttributeStorage::new();
        arg_attr.add(
            AttrIndex::Param(0),
            Attribute::enum_attr(AttrKind::Returned).expect("returned is enum"),
        );
        let attrs = CallAttributeData::new(
            AttributeStorage::new(),
            Box::new([arg_attr]),
            AttributeStorage::new(),
        );

        let caller_ty = m.fn_type_no_params(void_ty, false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let call_entry = caller.append_basic_block(&m, "call.entry");
        let invoke_entry = caller.append_basic_block(&m, "invoke.entry");
        let invoke_normal = caller.append_basic_block(&m, "invoke.normal");
        let invoke_unwind = caller.append_basic_block(&m, "invoke.unwind");
        let invoke_entry_label = invoke_entry.label();
        let invoke_normal_label = invoke_normal.label();
        let invoke_unwind_label = invoke_unwind.label();

        let call_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(call_entry);
        let call = call_b
            .call_builder(callee)
            .arg(i8_ty.const_int(0xa5_u8))
            .call_attributes(attrs.clone())
            .name("call")
            .build()?
            .return_int_value();
        let (_, _) = call_b.build_br(invoke_entry_label)?;

        let (_, invoke) = IRBuilder::with_folder(&m, NoFolder)
            .position_at_end(invoke_entry)
            .build_invoke_dyn_with_config(
                callee,
                [i8_ty.const_int(0x3c_u8)],
                invoke_normal_label,
                invoke_unwind_label,
                llvmkit_ir::CallSiteConfig::new("invoke").attrs(attrs),
            )?;
        let invoke_value: IntValue<i8> = invoke.as_value().try_into()?;

        IRBuilder::new_for::<()>(&m)
            .position_at_end(invoke_unwind)
            .build_unreachable();
        IRBuilder::new_for::<()>(&m)
            .position_at_end(invoke_normal)
            .build_ret_void();

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert_eq!(known(call.as_value(), &query)?.to_string(), "10100101");
        assert_eq!(
            known(invoke_value.as_value(), &query)?.to_string(),
            "00111100"
        );
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
/// call intrinsic arms for represented integer intrinsics.
#[test]
fn intrinsic_calls_compute_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-intrinsic-calls", |m| {
        let i1_ty = m.bool_type();
        let i8_ty = m.i8_type();
        let i16_ty = m.i16_type();
        let void_ty = m.void_type();

        let caller_ty = m.fn_type_no_params(void_ty, false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let abs_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.abs.i8")?;
        let abs: IntValue<i8> = b
            .call_builder(abs_fn)
            .arg(i8_ty.const_int(-5_i8))
            .arg(i1_ty.const_int(true))
            .name("abs")
            .build()?
            .return_value()
            .expect("abs returns value")
            .try_into()?;

        let bitreverse_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.bitreverse.i8")?;
        let bitreverse: IntValue<i8> = b
            .call_builder(bitreverse_fn)
            .arg(i8_ty.const_int(0x10_u8))
            .name("rev")
            .build()?
            .return_value()
            .expect("bitreverse returns value")
            .try_into()?;

        let ctlz_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.ctlz.i8")?;
        let ctlz: IntValue<i8> = b
            .call_builder(ctlz_fn)
            .arg(i8_ty.const_int(0x10_u8))
            .arg(i1_ty.const_int(true))
            .name("ctlz")
            .build()?
            .return_value()
            .expect("ctlz returns value")
            .try_into()?;

        let ctpop_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.ctpop.i8")?;
        let ctpop: IntValue<i8> = b
            .call_builder(ctpop_fn)
            .arg(i8_ty.const_int(0x0f_u8))
            .name("pop")
            .build()?
            .return_value()
            .expect("ctpop returns value")
            .try_into()?;

        let uadd_sat_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.uadd.sat.i8")?;
        let uadd_sat: IntValue<i8> = b
            .call_builder(uadd_sat_fn)
            .arg(i8_ty.const_int(250_u8))
            .arg(i8_ty.const_int(10_u8))
            .name("usat")
            .build()?
            .return_value()
            .expect("uadd.sat returns value")
            .try_into()?;

        let smax_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.smax.i8")?;
        let smax: IntValue<i8> = b
            .call_builder(smax_fn)
            .arg(i8_ty.const_int(-5_i8))
            .arg(i8_ty.const_int(7_i8))
            .name("smax")
            .build()?
            .return_value()
            .expect("smax returns value")
            .try_into()?;

        let bswap_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.bswap.i16")?;
        let bswap: IntValue<i16> = b
            .call_builder(bswap_fn)
            .arg(i16_ty.const_int(0x1234_u16))
            .name("swap")
            .build()?
            .return_value()
            .expect("bswap returns value")
            .try_into()?;

        let fshl_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.fshl.i8")?;
        let fshl: IntValue<i8> = b
            .call_builder(fshl_fn)
            .arg(i8_ty.const_int(0x12_u8))
            .arg(i8_ty.const_int(0x34_u8))
            .arg(i8_ty.const_int(4_u8))
            .name("fshl")
            .build()?
            .return_value()
            .expect("fshl returns value")
            .try_into()?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert_eq!(known(abs.as_value(), &query)?.to_string(), "00000101");
        assert_eq!(
            known(bitreverse.as_value(), &query)?.to_string(),
            "00001000"
        );
        assert_eq!(known(ctlz.as_value(), &query)?.to_string(), "000000??");
        assert_eq!(known(ctpop.as_value(), &query)?.to_string(), "00000???");
        assert_eq!(known(uadd_sat.as_value(), &query)?.to_string(), "11111111");
        assert_eq!(known(smax.as_value(), &query)?.to_string(), "00000111");
        assert_eq!(
            known(bswap.as_value(), &query)?.to_string(),
            "0011010000010010"
        );
        assert_eq!(known(fshl.as_value(), &query)?.to_string(), "00100011");
        Ok(())
    })
}

/// Defensive regression for `llvm/lib/IR/Intrinsics.cpp::getIntrinsicInfoTableEntries`
/// intrinsic semantics require a generated intrinsic callee; ordinary lookalike
/// functions stay conservative.
#[test]
fn intrinsic_known_bits_ignore_mismatched_declarations() -> Result<(), IrError> {
    Module::with_new("vt-intrinsic-mismatch", |m| {
        let i1_ty = m.bool_type();
        let i16_ty = m.i16_type();
        let void_ty = m.void_type();
        let caller_ty = m.fn_type_no_params(void_ty, false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let malformed_ty = m.fn_type(i16_ty, [i16_ty.as_type(), i1_ty.as_type()], false);
        let malformed =
            m.add_function::<i16, _>("not.llvm.abs.i8", malformed_ty, Linkage::External)?;
        let call: IntValue<i16> = b
            .call_builder(malformed)
            .arg(i16_ty.const_int(-5_i16))
            .arg(i1_ty.const_int(true))
            .name("abs")
            .build()?
            .return_value()
            .expect("lookalike returns value")
            .try_into()?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert_eq!(
            known(call.as_value(), &query)?.to_string(),
            "????????????????"
        );
        Ok(())
    })
}

/// Port of `llvm/include/llvm/Analysis/ValueTracking.h::computeKnownBits`
/// overloads carrying `DemandedElts`, `CxtI`, and `UseInstrInfo`, plus
/// `llvm/include/llvm/Analysis/SimplifyQuery.h::InstrInfoQuery`.
#[test]
fn query_carries_context_demanded_elements_and_instr_info_policy() -> Result<(), IrError> {
    Module::with_new("vt-query-shape", |m| {
        let i8_ty = m.i8_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let p: PointerValue = f.param(0)?.try_into()?;
        let load = b.build_int_load::<i8, _, _>(p, "load")?;
        let load_inst = InstructionView::try_from(load.as_value())?;
        let demanded = ApInt::from_words(1, &[1]);
        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl)
            .with_context_instruction(&load_inst)
            .with_demanded_elements(&demanded)
            .without_instruction_info();

        assert_eq!(query.context_instruction(), Some(load_inst.as_value()));
        assert_eq!(query.demanded_elements(), Some(&demanded));
        assert!(!query.uses_instruction_info());
        assert!(query.with_instruction_info().uses_instruction_info());
        Ok(())
    })
}

/// `llvmkit-specific subset` of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBits`:
/// the new-PM function analysis caches a reusable query result for one function.
#[test]
fn function_analysis_caches_known_bits_queries() -> Result<(), IrError> {
    Module::with_new("vt-analysis", |m| {
        let i8_ty = m.i8_type();
        let fn_ty = m.fn_type_no_params(i8_ty, false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let value = b.build_int_and::<i8, _, _, _>(
            i8_ty.const_int(0b1111_0000_u8),
            i8_ty.const_int(0b1010_1010_u8),
            "known",
        )?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(KnownBitsAnalysis);

        let result = fam.get_result::<KnownBitsAnalysis, _>(f)?;
        assert_eq!(
            result.compute_known_bits(value.as_value())?.to_string(),
            "10100000"
        );
        assert!(result.is_known_non_zero(value.as_value())?);
        assert!(fam.get_cached_result::<KnownBitsAnalysis, _>(f).is_some());
        Ok(())
    })
}

/// `llvmkit-specific subset` of LLVM new-PM invalidation dependency handling:
/// a cached KnownBits result that captures a DominatorTree is invalidated when
/// the dominator tree is not preserved, even if KnownBits itself is preserved.
#[test]
fn known_bits_analysis_invalidates_with_dominator_tree_dependency() -> Result<(), IrError> {
    Module::with_new("vt-analysis-invalidate", |m| {
        let i8_ty = m.i8_type();
        let fn_ty = m.fn_type_no_params(i8_ty, false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let value = b.build_int_and::<i8, _, _, _>(
            i8_ty.const_int(0b1111_0000_u8),
            i8_ty.const_int(0b1010_1010_u8),
            "known",
        )?;
        b.build_ret(value)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DominatorTreeAnalysis);
        fam.register_pass(KnownBitsAnalysis);

        let _ = fam.get_result::<DominatorTreeAnalysis, _>(f)?;
        {
            let result = fam.get_result::<KnownBitsAnalysis, _>(f)?;
            let query: ValueTrackingQuery<'_, '_, Brand<'_>> = result.query();
            assert!(query.dominator_tree().is_some());
        }

        let mut preserves_known_bits_and_cfg = PreservedAnalyses::none();
        preserves_known_bits_and_cfg.preserve::<KnownBitsAnalysis>();
        preserves_known_bits_and_cfg.preserve_set::<CFGAnalyses>();
        fam.invalidate(f, &preserves_known_bits_and_cfg)?;
        assert!(fam.get_cached_result::<KnownBitsAnalysis, _>(f).is_some());

        let mut preserves_known_bits_only = PreservedAnalyses::none();
        preserves_known_bits_only.preserve::<KnownBitsAnalysis>();
        fam.invalidate(f, &preserves_known_bits_only)?;
        assert!(fam.get_cached_result::<KnownBitsAnalysis, _>(f).is_none());
        Ok(())
    })
}

/// Regression for `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBits` poison
/// handling through shifts: an invalid shift amount makes a frozen result unknown.
#[test]
fn shift_with_possible_invalid_amount_is_unknown_after_freeze() -> Result<(), IrError> {
    Module::with_new("vt-shift-poison", |m| {
        let i4_ty = m.int_type_n::<4>();
        let fn_ty = m.fn_type(i4_ty, [i4_ty.as_type()], false);
        let f = m.add_function::<Width<4>, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let x: IntValue<Width<4>> = f.param(0)?.try_into()?;
        let one = i4_ty.const_ap_int(&ApInt::from_words(4, &[1]))?;
        let eight = i4_ty.const_ap_int(&ApInt::from_words(4, &[8]))?;
        let shift = b.build_int_and::<Width<4>, _, _, _>(x, eight, "shift")?;
        let shl = b.build_int_shl::<Width<4>, _, _, _>(one, shift, "shl")?;
        let frozen = b.build_freeze(shl, "fr")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert!(known(frozen.as_value(), &query)?.is_unknown());
        Ok(())
    })
}

/// Regression for target-specific pointer representation casts: address-space
/// casts must not preserve source pointer known bits without representation proof.
#[test]
fn addrspacecast_drops_source_pointer_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-addrspacecast", |m| {
        let i32_ty = m.i32_type();
        let ptr1_ty = m.ptr_type(1);
        let fn_ty = m.fn_type_no_params(ptr1_ty, false);
        let f = m.add_function::<Ptr, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let slot = b.build_alloca_with_align(i32_ty, Align::new(16)?, "slot")?;
        let cast = b.build_addrspace_cast(slot, ptr1_ty, "cast")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert!(known(cast.as_value(), &query)?.is_unknown());
        Ok(())
    })
}

/// Regression for `llvm/lib/Analysis/ValueTracking.cpp::isGuaranteedNotToBePoison`:
/// `exact` shifts can be poison even when the shift amount is in range.
#[test]
fn freeze_of_exact_shift_that_can_poison_is_unknown() -> Result<(), IrError> {
    Module::with_new("vt-freeze-exact-shift", |m| {
        let i4_ty = m.int_type_n::<4>();
        let fn_ty = m.fn_type_no_params(i4_ty, false);
        let f = m.add_function::<Width<4>, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let one = i4_ty.const_ap_int(&ApInt::from_words(4, &[1]))?;
        let lshr = b.build_int_lshr_with_flags::<Width<4>, _, _, _>(
            one,
            one,
            LShrFlags::new().exact(),
            "lshr",
        )?;
        let frozen = b.build_freeze(lshr, "fr")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        assert!(known(frozen.as_value(), &query)?.is_unknown());
        let query_without_instr_info = ValueTrackingQuery::new(&dl).without_instruction_info();
        assert_eq!(
            known(frozen.as_value(), &query_without_instr_info)?.to_string(),
            "0000"
        );
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
/// `getelementptr` offset accumulation and vector
/// `extractelement` / `insertelement` / `shufflevector` demanded-element arms.
#[test]
fn gep_and_vector_lane_operations_compute_known_bits() -> Result<(), IrError> {
    Module::with_new("vt-gep-vector", |m| {
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i8_ty.as_type(), 2, false);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type_no_params(void_ty.as_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let slot = b.build_alloca_with_align(i32_ty, Align::new(16)?, "slot")?;
        let ptr = b.build_ptr_add(slot, i8_ty.const_int(8_u8), "ptr")?;

        let poison_vec = vec_ty.as_type().get_poison();
        let lane0 = b.build_insert_element(
            poison_vec,
            i8_ty.const_int(0xf0_u8),
            i8_ty.const_int(0_u8),
            "lane0",
        )?;
        let lane01 = b.build_insert_element(
            lane0,
            i8_ty.const_int(0x0f_u8),
            i8_ty.const_int(1_u8),
            "lane1",
        )?;
        let extract = b.build_extract_element(lane01, i8_ty.const_int(1_u8), "extract")?;

        let rhs = vec_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i8>, _>([
            i8_ty.const_int(0x55_u8),
            i8_ty.const_int(0xaa_u8),
        ])?;
        let shuffle = b.build_shuffle_vector(lane01, rhs, &[1], "shuffle")?;
        let shuffle_extract =
            b.build_extract_element(shuffle, i8_ty.const_int(0_u8), "shuf.ext")?;

        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        let ptr_bits = known(ptr.as_value(), &query)?;
        assert!(ptr_bits.is_known_zero(0), "{ptr_bits}");
        assert!(ptr_bits.is_known_zero(1), "{ptr_bits}");
        assert!(ptr_bits.is_known_zero(2), "{ptr_bits}");
        assert!(ptr_bits.is_known_one(3), "{ptr_bits}");
        assert_eq!(known(extract, &query)?.to_string(), "00001111");
        assert_eq!(known(shuffle_extract, &query)?.to_string(), "00001111");
        Ok(())
    })
}

/// Mirrors `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
/// `getelementptr` handling and `DataLayout::getIndexType(Type*)`: a pointer
/// vector result uses its element pointer address space when selecting the GEP
/// index width.
#[test]
fn vector_gep_known_bits_use_element_pointer_address_space_index_width() -> Result<(), IrError> {
    Module::with_new("vt-vector-gep-as", |m| {
        m.set_data_layout("p1:64:64:64:32")?;
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let ptr1_ty = m.ptr_type(1);
        let ptr_vec_ty = m.vector_type(ptr1_ty.as_type(), 2, false);
        let i32_vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let base = ptr_vec_ty.const_vector([ptr1_ty.const_null(); 2])?;
        let minus_one = i32_ty.const_int(-1_i32);
        let index = i32_vec_ty
            .const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([minus_one, minus_one])?;
        let gep = m.constant_expr_with_options(
            ptr_vec_ty.as_type(),
            ConstantExprOpcode::GetElementPtr,
            [base.as_value(), index.as_value()],
            [],
            [],
            ConstantExprOptions::new().source_ty(i8_ty.as_type()),
        )?;
        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);

        assert_eq!(
            known(gep.as_value(), &query)?.to_string(),
            "0000000000000000000000000000000011111111111111111111111111111111"
        );
        Ok(())
    })
}
