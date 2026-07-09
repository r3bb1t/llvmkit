use llvmkit_ir::{
    Analyses, ApInt, DemandedBitsAnalysis, Dyn, FunctionAnalysisManager, IRBuilder, IntValue,
    IrError, KnownBits, Linkage, Module, NoFolder, SimplifyDemandedBitsPass, Type,
    ValueTrackingQuery, Width, ZExtFlags, run_function_pass, simplify_demanded_bits,
};

fn bits(value: ApInt) -> String {
    KnownBits::from_ap_int(value).to_string()
}

/// Port of `llvm/test/Analysis/DemandedBits/basic.ll::test_mul`.
#[test]
fn demanded_bits_basic_trunc_zext_chain() -> Result<(), IrError> {
    Module::with_new("demanded-basic", |m| {
        let i1_ty = m.bool_type();
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i8_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i8, _>("test_mul", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let b_arg: IntValue<i32> = f.param(1)?.try_into()?;

        let add = b.build_int_add::<i32, _, _, _>(a, i32_ty.const_int(5_u32), "add")?;
        let mul = b.build_int_mul::<i32, _, _, _>(add, b_arg, "mul")?;
        let trunc_i8 = b.build_trunc(mul, i8_ty, "lo8")?;
        let trunc_i1 = b.build_trunc(mul, i1_ty, "lo1")?;
        let zext = b.build_zext(trunc_i1, i8_ty, "wide")?;
        let sum = b.build_int_add::<i8, _, _, _>(trunc_i8, zext, "sum")?;
        b.build_ret(sum)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);
        let demanded = fam.get_result::<DemandedBitsAnalysis, _>(f)?;

        assert_eq!(
            bits(demanded.get_demanded_bits(add.as_value())),
            "00000000000000000000000011111111"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(mul.as_value())),
            "00000000000000000000000011111111"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(trunc_i8.as_value())),
            "11111111"
        );
        assert_eq!(bits(demanded.get_demanded_bits(trunc_i1.as_value())), "1");
        assert_eq!(
            bits(demanded.get_demanded_bits(zext.as_value())),
            "11111111"
        );
        assert_eq!(
            bits(demanded.get_operand_demanded_bits(trunc_i1.as_value(), 0)?),
            "00000000000000000000000000000001"
        );
        assert_eq!(
            bits(demanded.get_operand_demanded_bits(zext.as_value(), 0)?),
            "1"
        );
        Ok(())
    })
}

/// Port of `llvm/test/Analysis/DemandedBits/add.ll::test_add`.
#[test]
fn demanded_bits_add_and_or_carry_propagation() -> Result<(), IrError> {
    Module::with_new("demanded-add", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(
            i32_ty,
            [
                i32_ty.as_type(),
                i32_ty.as_type(),
                i32_ty.as_type(),
                i32_ty.as_type(),
            ],
            false,
        );
        let f = m.add_function::<i32, _>("test_add", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let b_arg: IntValue<i32> = f.param(1)?.try_into()?;
        let c: IntValue<i32> = f.param(2)?.try_into()?;
        let d: IntValue<i32> = f.param(3)?.try_into()?;

        let and_a = b.build_int_and::<i32, _, _, _>(a, i32_ty.const_int(9_u32), "and.a")?;
        let and_b = b.build_int_and::<i32, _, _, _>(b_arg, i32_ty.const_int(9_u32), "and.b")?;
        let and_c = b.build_int_and::<i32, _, _, _>(c, i32_ty.const_int(13_u32), "and.c")?;
        let and_d = b.build_int_and::<i32, _, _, _>(d, i32_ty.const_int(4_u32), "and.d")?;
        let or_bc = b.build_int_or::<i32, _, _, _>(and_b, and_c, "or.bc")?;
        let or_dbc = b.build_int_or::<i32, _, _, _>(and_d, or_bc, "or.dbc")?;
        let add = b.build_int_add::<i32, _, _, _>(and_a, or_dbc, "add")?;
        let mask = b.build_int_and::<i32, _, _, _>(add, i32_ty.const_int(16_u32), "mask")?;
        b.build_ret(mask)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);
        let demanded = fam.get_result::<DemandedBitsAnalysis, _>(f)?;

        assert_eq!(
            bits(demanded.get_demanded_bits(and_a.as_value())),
            "00000000000000000000000000011110"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(and_b.as_value())),
            "00000000000000000000000000011010"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(and_c.as_value())),
            "00000000000000000000000000011010"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(and_d.as_value())),
            "00000000000000000000000000011010"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(or_bc.as_value())),
            "00000000000000000000000000011010"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(or_dbc.as_value())),
            "00000000000000000000000000011010"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(add.as_value())),
            "00000000000000000000000000010000"
        );
        assert_eq!(
            bits(demanded.get_demanded_bits(mask.as_value())),
            "11111111111111111111111111111111"
        );
        assert_eq!(
            bits(demanded.get_operand_demanded_bits(and_d.as_value(), 0)?),
            "00000000000000000000000000000000"
        );
        assert!(demanded.is_use_dead(and_d.as_value(), 0)?);
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/DemandedBits.cpp::determineLiveOperandBits`
/// intrinsic arms for `bitreverse`, `bswap`, `fshl` / `fshr` value and amount
/// masks (including modulo amounts), and all represented min/max variants.
#[test]
fn demanded_bits_intrinsic_operand_masks_match_upstream() -> Result<(), IrError> {
    Module::with_new("demanded-intrinsics", |m| {
        let i8_ty = m.i8_type();
        let i16_ty = m.i16_type();
        let i128_ty = m.i128_type();

        let rev_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.bitreverse.i8")?;
        let rev_host_ty = m.fn_type(i8_ty, [i8_ty.as_type()], false);
        let rev_host = m.add_function::<i8, _>("rev_host", rev_host_ty, Linkage::External)?;
        let rev_entry = rev_host.append_basic_block(&m, "entry");
        let rev_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(rev_entry);
        let rev_x: IntValue<i8> = rev_host.param(0)?.try_into()?;
        let rev: IntValue<i8> = rev_b
            .call_builder(rev_fn)
            .arg(rev_x)
            .name("rev")
            .build()?
            .return_value()
            .expect("bitreverse returns value")
            .try_into()?;
        let rev_mask = rev_b.build_int_and::<i8, _, _, _>(rev, i8_ty.const_int(0x0f_u8), "mask")?;
        rev_b.build_ret(rev_mask)?;

        let swap_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.bswap.i16")?;
        let swap_host_ty = m.fn_type(i16_ty, [i16_ty.as_type()], false);
        let swap_host = m.add_function::<i16, _>("swap_host", swap_host_ty, Linkage::External)?;
        let swap_entry = swap_host.append_basic_block(&m, "entry");
        let swap_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(swap_entry);
        let swap_x: IntValue<i16> = swap_host.param(0)?.try_into()?;
        let swap: IntValue<i16> = swap_b
            .call_builder(swap_fn)
            .arg(swap_x)
            .name("swap")
            .build()?
            .return_value()
            .expect("bswap returns value")
            .try_into()?;
        let swap_mask =
            swap_b.build_int_and::<i16, _, _, _>(swap, i16_ty.const_int(0x00ff_u16), "mask")?;
        swap_b.build_ret(swap_mask)?;

        let fshl_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.fshl.i8")?;
        let fshl_host_ty = m.fn_type(i8_ty, [i8_ty.as_type(), i8_ty.as_type()], false);
        let fshl_host = m.add_function::<i8, _>("fshl_host", fshl_host_ty, Linkage::External)?;
        let fshl_entry = fshl_host.append_basic_block(&m, "entry");
        let fshl_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(fshl_entry);
        let fshl_x: IntValue<i8> = fshl_host.param(0)?.try_into()?;
        let fshl_y: IntValue<i8> = fshl_host.param(1)?.try_into()?;
        let fshl: IntValue<i8> = fshl_b
            .call_builder(fshl_fn)
            .arg(fshl_x)
            .arg(fshl_y)
            .arg(i8_ty.const_int(4_u8))
            .name("fshl")
            .build()?
            .return_value()
            .expect("fshl returns value")
            .try_into()?;
        let fshl_mask =
            fshl_b.build_int_and::<i8, _, _, _>(fshl, i8_ty.const_int(0x0f_u8), "mask")?;
        fshl_b.build_ret(fshl_mask)?;

        let fshr_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.fshr.i8")?;
        let fshr_host_ty = m.fn_type(i8_ty, [i8_ty.as_type(), i8_ty.as_type()], false);
        let fshr_host = m.add_function::<i8, _>("fshr_host", fshr_host_ty, Linkage::External)?;
        let fshr_entry = fshr_host.append_basic_block(&m, "entry");
        let fshr_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(fshr_entry);
        let fshr_x: IntValue<i8> = fshr_host.param(0)?.try_into()?;
        let fshr_y: IntValue<i8> = fshr_host.param(1)?.try_into()?;
        let fshr: IntValue<i8> = fshr_b
            .call_builder(fshr_fn)
            .arg(fshr_x)
            .arg(fshr_y)
            .arg(i8_ty.const_int(2_u8))
            .name("fshr")
            .build()?
            .return_value()
            .expect("fshr returns value")
            .try_into()?;
        let fshr_mask =
            fshr_b.build_int_and::<i8, _, _, _>(fshr, i8_ty.const_int(0x0f_u8), "mask")?;
        fshr_b.build_ret(fshr_mask)?;

        let fshr_zero_host_ty = m.fn_type(i8_ty, [i8_ty.as_type(), i8_ty.as_type()], false);
        let fshr_zero_host =
            m.add_function::<i8, _>("fshr_zero_host", fshr_zero_host_ty, Linkage::External)?;
        let fshr_zero_entry = fshr_zero_host.append_basic_block(&m, "entry");
        let fshr_zero_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(fshr_zero_entry);
        let fshr_zero_x: IntValue<i8> = fshr_zero_host.param(0)?.try_into()?;
        let fshr_zero_y: IntValue<i8> = fshr_zero_host.param(1)?.try_into()?;
        let fshr_zero: IntValue<i8> = fshr_zero_b
            .call_builder(fshr_fn)
            .arg(fshr_zero_x)
            .arg(fshr_zero_y)
            .arg(i8_ty.const_int(8_u8))
            .name("fshr.zero")
            .build()?
            .return_value()
            .expect("fshr returns value")
            .try_into()?;
        let fshr_zero_mask = fshr_zero_b.build_int_and::<i8, _, _, _>(
            fshr_zero,
            i8_ty.const_int(0x0f_u8),
            "mask",
        )?;
        fshr_zero_b.build_ret(fshr_zero_mask)?;

        let wide_fshl_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.fshl.i128")?;
        let wide_fshl_host_ty = m.fn_type(i128_ty, [i128_ty.as_type(), i128_ty.as_type()], false);
        let wide_fshl_host =
            m.add_function::<i128, _>("wide_fshl_host", wide_fshl_host_ty, Linkage::External)?;
        let wide_fshl_entry = wide_fshl_host.append_basic_block(&m, "entry");
        let wide_fshl_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(wide_fshl_entry);
        let wide_fshl_x: IntValue<i128> = wide_fshl_host.param(0)?.try_into()?;
        let wide_fshl_y: IntValue<i128> = wide_fshl_host.param(1)?.try_into()?;
        let wide_fshl_amount = i128_ty.const_ap_int(&ApInt::from_words(128, &[1, 1]))?;
        let wide_fshl: IntValue<i128> = wide_fshl_b
            .call_builder(wide_fshl_fn)
            .arg(wide_fshl_x)
            .arg(wide_fshl_y)
            .arg(wide_fshl_amount)
            .name("wide.fshl")
            .build()?
            .return_value()
            .expect("fshl returns value")
            .try_into()?;
        let wide_fshl_mask = i128_ty.const_ap_int(&ApInt::from_words(128, &[0xff]))?;
        let wide_fshl_masked =
            wide_fshl_b.build_int_and::<i128, _, _, _>(wide_fshl, wide_fshl_mask, "mask")?;
        wide_fshl_b.build_ret(wide_fshl_masked)?;

        let umax_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.umax.i8")?;
        let umax_host_ty = m.fn_type(i8_ty, [i8_ty.as_type(), i8_ty.as_type()], false);
        let umax_host = m.add_function::<i8, _>("umax_host", umax_host_ty, Linkage::External)?;
        let umax_entry = umax_host.append_basic_block(&m, "entry");
        let umax_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(umax_entry);
        let umax_x: IntValue<i8> = umax_host.param(0)?.try_into()?;
        let umax_y: IntValue<i8> = umax_host.param(1)?.try_into()?;
        let umax: IntValue<i8> = umax_b
            .call_builder(umax_fn)
            .arg(umax_x)
            .arg(umax_y)
            .name("umax")
            .build()?
            .return_value()
            .expect("umax returns value")
            .try_into()?;
        let umax_mask =
            umax_b.build_int_and::<i8, _, _, _>(umax, i8_ty.const_int(0xf0_u8), "mask")?;
        umax_b.build_ret(umax_mask)?;

        let umin_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.umin.i8")?;
        let umin_host_ty = m.fn_type(i8_ty, [i8_ty.as_type(), i8_ty.as_type()], false);
        let umin_host = m.add_function::<i8, _>("umin_host", umin_host_ty, Linkage::External)?;
        let umin_entry = umin_host.append_basic_block(&m, "entry");
        let umin_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(umin_entry);
        let umin_x: IntValue<i8> = umin_host.param(0)?.try_into()?;
        let umin_y: IntValue<i8> = umin_host.param(1)?.try_into()?;
        let umin: IntValue<i8> = umin_b
            .call_builder(umin_fn)
            .arg(umin_x)
            .arg(umin_y)
            .name("umin")
            .build()?
            .return_value()
            .expect("umin returns value")
            .try_into()?;
        let umin_mask =
            umin_b.build_int_and::<i8, _, _, _>(umin, i8_ty.const_int(0xf0_u8), "mask")?;
        umin_b.build_ret(umin_mask)?;

        let smax_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.smax.i8")?;
        let smax_host_ty = m.fn_type(i8_ty, [i8_ty.as_type(), i8_ty.as_type()], false);
        let smax_host = m.add_function::<i8, _>("smax_host", smax_host_ty, Linkage::External)?;
        let smax_entry = smax_host.append_basic_block(&m, "entry");
        let smax_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(smax_entry);
        let smax_x: IntValue<i8> = smax_host.param(0)?.try_into()?;
        let smax_y: IntValue<i8> = smax_host.param(1)?.try_into()?;
        let smax: IntValue<i8> = smax_b
            .call_builder(smax_fn)
            .arg(smax_x)
            .arg(smax_y)
            .name("smax")
            .build()?
            .return_value()
            .expect("smax returns value")
            .try_into()?;
        let smax_mask =
            smax_b.build_int_and::<i8, _, _, _>(smax, i8_ty.const_int(0xf0_u8), "mask")?;
        smax_b.build_ret(smax_mask)?;

        let smin_fn = m.get_or_insert_intrinsic_declaration_by_name("llvm.smin.i8")?;
        let smin_host_ty = m.fn_type(i8_ty, [i8_ty.as_type(), i8_ty.as_type()], false);
        let smin_host = m.add_function::<i8, _>("smin_host", smin_host_ty, Linkage::External)?;
        let smin_entry = smin_host.append_basic_block(&m, "entry");
        let smin_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(smin_entry);
        let smin_x: IntValue<i8> = smin_host.param(0)?.try_into()?;
        let smin_y: IntValue<i8> = smin_host.param(1)?.try_into()?;
        let smin: IntValue<i8> = smin_b
            .call_builder(smin_fn)
            .arg(smin_x)
            .arg(smin_y)
            .name("smin")
            .build()?
            .return_value()
            .expect("smin returns value")
            .try_into()?;
        let smin_mask =
            smin_b.build_int_and::<i8, _, _, _>(smin, i8_ty.const_int(0xf0_u8), "mask")?;
        smin_b.build_ret(smin_mask)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);

        let rev_demanded = fam.get_result::<DemandedBitsAnalysis, _>(rev_host)?;
        assert_eq!(
            bits(rev_demanded.get_operand_demanded_bits(rev.as_value(), 1)?),
            "11110000"
        );

        let swap_demanded = fam.get_result::<DemandedBitsAnalysis, _>(swap_host)?;
        assert_eq!(
            bits(swap_demanded.get_operand_demanded_bits(swap.as_value(), 1)?),
            "1111111100000000"
        );

        let fshl_demanded = fam.get_result::<DemandedBitsAnalysis, _>(fshl_host)?;
        assert_eq!(
            bits(fshl_demanded.get_operand_demanded_bits(fshl.as_value(), 1)?),
            "00000000"
        );
        assert_eq!(
            bits(fshl_demanded.get_operand_demanded_bits(fshl.as_value(), 2)?),
            "11110000"
        );
        assert_eq!(
            bits(fshl_demanded.get_operand_demanded_bits(fshl.as_value(), 3)?),
            "00000111"
        );

        let fshr_demanded = fam.get_result::<DemandedBitsAnalysis, _>(fshr_host)?;
        assert_eq!(
            bits(fshr_demanded.get_operand_demanded_bits(fshr.as_value(), 1)?),
            "00000000"
        );
        assert_eq!(
            bits(fshr_demanded.get_operand_demanded_bits(fshr.as_value(), 2)?),
            "00111100"
        );
        assert_eq!(
            bits(fshr_demanded.get_operand_demanded_bits(fshr.as_value(), 3)?),
            "00000111"
        );

        let fshr_zero_demanded = fam.get_result::<DemandedBitsAnalysis, _>(fshr_zero_host)?;
        assert_eq!(
            bits(fshr_zero_demanded.get_operand_demanded_bits(fshr_zero.as_value(), 1)?),
            "00001111"
        );
        assert_eq!(
            bits(fshr_zero_demanded.get_operand_demanded_bits(fshr_zero.as_value(), 2)?),
            "00000000"
        );
        assert_eq!(
            bits(fshr_zero_demanded.get_operand_demanded_bits(fshr_zero.as_value(), 3)?),
            "00000111"
        );

        let wide_fshl_demanded = fam.get_result::<DemandedBitsAnalysis, _>(wide_fshl_host)?;
        assert_eq!(
            wide_fshl_demanded.get_operand_demanded_bits(wide_fshl.as_value(), 1)?,
            ApInt::from_words(128, &[0x7f])
        );
        assert_eq!(
            wide_fshl_demanded.get_operand_demanded_bits(wide_fshl.as_value(), 2)?,
            ApInt::one_bit_set(128, 127)
        );
        assert_eq!(
            wide_fshl_demanded.get_operand_demanded_bits(wide_fshl.as_value(), 3)?,
            ApInt::from_words(128, &[127])
        );

        let umax_demanded = fam.get_result::<DemandedBitsAnalysis, _>(umax_host)?;
        assert_eq!(
            bits(umax_demanded.get_operand_demanded_bits(umax.as_value(), 1)?),
            "11110000"
        );
        assert_eq!(
            bits(umax_demanded.get_operand_demanded_bits(umax.as_value(), 2)?),
            "11110000"
        );

        let umin_demanded = fam.get_result::<DemandedBitsAnalysis, _>(umin_host)?;
        assert_eq!(
            bits(umin_demanded.get_operand_demanded_bits(umin.as_value(), 1)?),
            "11110000"
        );
        assert_eq!(
            bits(umin_demanded.get_operand_demanded_bits(umin.as_value(), 2)?),
            "11110000"
        );

        let smax_demanded = fam.get_result::<DemandedBitsAnalysis, _>(smax_host)?;
        assert_eq!(
            bits(smax_demanded.get_operand_demanded_bits(smax.as_value(), 1)?),
            "11110000"
        );
        assert_eq!(
            bits(smax_demanded.get_operand_demanded_bits(smax.as_value(), 2)?),
            "11110000"
        );

        let smin_demanded = fam.get_result::<DemandedBitsAnalysis, _>(smin_host)?;
        assert_eq!(
            bits(smin_demanded.get_operand_demanded_bits(smin.as_value(), 1)?),
            "11110000"
        );
        assert_eq!(
            bits(smin_demanded.get_operand_demanded_bits(smin.as_value(), 2)?),
            "11110000"
        );
        Ok(())
    })
}

/// Defensive regression for `llvm/lib/IR/Intrinsics.cpp::getIntrinsicInfoTableEntries`
/// and `llvm/lib/Analysis/DemandedBits.cpp::determineLiveOperandBits`:
/// intrinsic operand masks require a generated intrinsic callee; ordinary
/// lookalike functions stay conservative.
#[test]
fn demanded_bits_ignore_mismatched_intrinsic_declarations() -> Result<(), IrError> {
    Module::with_new("demanded-intrinsic-mismatch", |m| {
        let i16_ty = m.i16_type();
        let fn_ty = m.fn_type(i16_ty, [i16_ty.as_type()], false);
        let malformed =
            m.add_function::<i16, _>("not.llvm.bitreverse.i8", fn_ty, Linkage::External)?;
        let host_ty = m.fn_type(i16_ty, [i16_ty.as_type()], false);
        let host = m.add_function::<i16, _>("host", host_ty, Linkage::External)?;
        let entry = host.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let x: IntValue<i16> = host.param(0)?.try_into()?;
        let call: IntValue<i16> = b
            .call_builder(malformed)
            .arg(x)
            .name("rev")
            .build()?
            .return_value()
            .expect("lookalike returns value")
            .try_into()?;
        let masked = b.build_int_and::<i16, _, _, _>(call, i16_ty.const_int(0x00ff_u16), "mask")?;
        b.build_ret(masked)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);
        let demanded = fam.get_result::<DemandedBitsAnalysis, _>(host)?;
        assert_eq!(
            bits(demanded.get_operand_demanded_bits(call.as_value(), 1)?),
            "1111111111111111"
        );
        Ok(())
    })
}

/// Regression for `llvm/lib/Analysis/DemandedBits.cpp::isUseDead`: operands of
/// unreachable dead integer users have no demanded bits.
#[test]
fn operands_of_dead_integer_instruction_are_dead() -> Result<(), IrError> {
    Module::with_new("demanded-dead-use", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("dead", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let dead = b.build_int_and::<i32, _, _, _>(a, i32_ty.const_int(15_u32), "dead")?;
        b.build_ret(i32_ty.const_int(0_u32))?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);
        let demanded = fam.get_result::<DemandedBitsAnalysis, _>(f)?;

        assert!(demanded.is_instruction_dead(dead.as_value()));
        assert!(demanded.is_use_dead(dead.as_value(), 0)?);
        assert_eq!(
            bits(demanded.get_operand_demanded_bits(dead.as_value(), 0)?),
            "00000000000000000000000000000000"
        );
        Ok(())
    })
}

/// Port of `llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp`:
/// demanded bits that are all known can produce a replacement constant while
/// the demanded mask is narrower than the full scalar width.
#[test]
fn simplify_demanded_bits_replaces_known_demanded_low_bits() -> Result<(), IrError> {
    Module::with_new("demanded-simplify", |m| {
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i8_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let lhs = i32_ty.const_int(0xffff_0000_u32);
        let high =
            b.build_int_and::<i32, _, _, _>(lhs, i32_ty.const_int(0x0000_00ff_u32), "high")?;
        let lo = b.build_trunc(high, i8_ty, "lo")?;
        b.build_ret(lo)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);
        let demanded = fam.get_result::<DemandedBitsAnalysis, _>(f)?;
        let dl = m.data_layout();
        let query = ValueTrackingQuery::new(&dl);
        let simplified = simplify_demanded_bits(high.as_value(), demanded, &query)?;

        assert!(simplified.demanded_bits_changed());
        assert_eq!(
            bits(simplified.demanded_bits().clone()),
            "00000000000000000000000011111111"
        );
        let replacement = simplified.replacement().expect("replacement");
        assert_eq!(
            bits(replacement.ap_int()),
            "00000000000000000000000000000000"
        );
        Ok(())
    })
}

/// Port of `llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp`
/// as a real function transform: proven demanded constants are RAUW'd and erased.
#[test]
fn simplify_demanded_bits_pass_folds_known_demanded_low_bits() -> Result<(), IrError> {
    Module::with_new("demanded-pass", |m| {
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i8_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i8, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let lhs = i32_ty.const_int(0xffff_0000_u32);
        let high =
            b.build_int_and::<i32, _, _, _>(lhs, i32_ty.const_int(0x0000_00ff_u32), "high")?;
        let lo = b.build_trunc(high, i8_ty, "lo")?;
        b.build_ret(lo)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(SimplifyDemandedBitsPass, verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert!(text.contains("ret i8 0"), "{text}");
        assert!(!text.contains(" and "), "{text}");
        assert!(!text.contains("trunc "), "{text}");
        Ok(())
    })
}

/// Port of `llvm/test/Transforms/InstCombine/assoc-cast-assoc.ll::AndZextAnd`:
/// this is the upstream fixture called out as being handled by
/// SimplifyDemandedBits / ShrinkDemandedConstant.
#[test]
fn simplify_demanded_bits_pass_ports_and_zext_and() -> Result<(), IrError> {
    Module::with_new("assoc-cast-assoc", |m| {
        let i3_ty = m.int_type_n::<3>();
        let i5_ty = m.int_type_n::<5>();
        let fn_ty = m.fn_type(i5_ty, [i3_ty.as_type()], false);
        let f = m.add_function::<Dyn, _>("AndZextAnd", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let a: IntValue<Width<3>> = f.param(0)?.try_into()?;
        let op1_rhs = i3_ty.const_ap_int(&ApInt::from_words(3, &[3]))?;
        let op2_rhs = i5_ty.const_ap_int(&ApInt::from_words(5, &[14]))?;
        let op1 = b.build_int_and::<Width<3>, _, _, _>(a, op1_rhs, "op1")?;
        let cast = b.build_zext_dyn(op1.as_dyn(), i5_ty.as_dyn(), "cast")?;
        let op2 = b.build_int_and_dyn(cast.as_value(), op2_rhs.as_value(), "op2")?;
        b.build_ret(op2)?;

        let before = format!("{m}");
        assert!(before.contains("%op1 = and i3 %0, 3"), "{before}");
        assert!(before.contains("%op2 = and i5 %cast, 14"), "{before}");

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(SimplifyDemandedBitsPass, verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert_eq!(
            text,
            concat!(
                "; ModuleID = 'assoc-cast-assoc'\n",
                "define i5 @AndZextAnd(i3 %0) {\n",
                "entry:\n",
                "  %op1 = and i3 %0, 2\n",
                "  %cast = zext nneg i3 %op1 to i5\n",
                "  ret i5 %cast\n",
                "}\n",
            )
        );
        Ok(())
    })
}

/// Port of `llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp`
/// `SimplifyDemandedUseBits` zext arm: when simplifying the zext operand or
/// mutating its old proof in place, LLVM drops poison-generating flags because
/// the old non-negative proof may be removed.
#[test]
fn simplify_demanded_bits_pass_drops_stale_zext_nneg_after_operand_replacement()
-> Result<(), IrError> {
    Module::with_new("demanded-zext-nneg", |m| {
        let i3_ty = m.int_type_n::<3>();
        let i5_ty = m.int_type_n::<5>();
        let fn_ty = m.fn_type(i5_ty, [i3_ty.as_type()], false);
        let f = m.add_function::<Dyn, _>("DropStaleZextNNeg", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let a: IntValue<Width<3>> = f.param(0)?.try_into()?;
        let proof_mask = i3_ty.const_ap_int(&ApInt::from_words(3, &[3]))?;
        let proof = b.build_int_and::<Width<3>, _, _, _>(a, proof_mask, "proof")?;
        let cast = b.build_zext_with_flags_dyn(
            proof.as_dyn(),
            i5_ty.as_dyn(),
            ZExtFlags::new().nneg(),
            "cast",
        )?;
        let low_mask = i5_ty.const_ap_int(&ApInt::from_words(5, &[3]))?;
        let low = b.build_int_and_dyn(cast.as_value(), low_mask.as_value(), "low")?;
        b.build_ret(low)?;

        let mutate_fn_ty = m.fn_type(i5_ty, [i3_ty.as_type()], false);
        let mutate_f =
            m.add_function::<Dyn, _>("DropStaleZextNNegMutate", mutate_fn_ty, Linkage::External)?;
        let mutate_entry = mutate_f.append_basic_block(&m, "entry");
        let mutate_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(mutate_entry);
        let mutate_arg: IntValue<Width<3>> = mutate_f.param(0)?.try_into()?;
        let sign_bit = i3_ty.const_ap_int(&ApInt::from_words(3, &[4]))?;
        let sign_mut =
            mutate_b.build_int_or::<Width<3>, _, _, _>(mutate_arg, sign_bit, "sign.mut")?;
        let proof_mut =
            mutate_b.build_int_xor::<Width<3>, _, _, _>(sign_mut, sign_bit, "proof.mut")?;
        let cast_mut = mutate_b.build_zext_with_flags_dyn(
            proof_mut.as_dyn(),
            i5_ty.as_dyn(),
            ZExtFlags::new().nneg(),
            "cast.mut",
        )?;
        let low_mut =
            mutate_b.build_int_and_dyn(cast_mut.as_value(), low_mask.as_value(), "low.mut")?;
        let zero_i3 = i3_ty.const_ap_int(&ApInt::zero(3))?;
        let _extra_mut =
            mutate_b.build_int_add::<Width<3>, _, _, _>(proof_mut, zero_i3, "extra.mut")?;
        mutate_b.build_ret(low_mut)?;

        let before = format!("{m}");
        assert!(before.contains("%proof = and i3 %0, 3"), "{before}");
        assert!(
            before.contains("%cast = zext nneg i3 %proof to i5"),
            "{before}"
        );
        assert!(
            before.contains("%proof.mut = xor i3 %sign.mut, -4"),
            "{before}"
        );
        assert!(
            before.contains("%cast.mut = zext nneg i3 %proof.mut to i5"),
            "{before}"
        );

        let verified = m.verify()?;
        // The module has two definitions; the retired adaptor visited both, so
        // the single-pass driver runs over each in module order, re-verifying
        // between (a mutating pass downgrades the module).
        let mut analyses = Analyses::new();
        let after_f = run_function_pass(SimplifyDemandedBitsPass, verified, f, &mut analyses)?;
        let reverified_f = after_f.verify()?;
        let unverified = run_function_pass(
            SimplifyDemandedBitsPass,
            reverified_f,
            mutate_f,
            &mut analyses,
        )?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert!(!text.contains("zext nneg"), "{text}");
        assert!(text.contains("%cast = zext i3 %0 to i5"), "{text}");
        assert!(text.contains("%low = and i5 %cast, 3"), "{text}");
        assert!(text.contains("%low.mut = and i5 %cast.mut, 3"), "{text}");
        assert!(!text.contains("%proof ="), "{text}");
        Ok(())
    })
}

/// Port of `llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp`:
/// unused integer instruction chains with no demanded bits are erased by the
/// transform.
#[test]
fn simplify_demanded_bits_pass_erases_dead_integer_chain() -> Result<(), IrError> {
    Module::with_new("demanded-pass-dead", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let dead0 = b.build_int_and::<i32, _, _, _>(x, i32_ty.const_int(15_u32), "dead0")?;
        let _dead1 = b.build_int_xor::<i32, _, _, _>(dead0, i32_ty.const_int(3_u32), "dead1")?;
        b.build_ret(i32_ty.const_int(0_u32))?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(SimplifyDemandedBitsPass, verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert!(!text.contains("dead0"), "{text}");
        assert!(!text.contains("dead1"), "{text}");
        assert!(text.contains("ret i32 0"), "{text}");
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/DemandedBits.cpp`: variable shifts must demand
/// every source bit that may move into a demanded result bit.
#[test]
fn variable_lshr_demands_source_bits_that_can_reach_low_result() -> Result<(), IrError> {
    Module::with_new("demanded-variable-shift", |m| {
        let i1_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i1_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<bool, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let amount: IntValue<i32> = f.param(1)?.try_into()?;
        let masked = b.build_int_and::<i32, _, _, _>(a, i32_ty.const_int(256_u32), "masked")?;
        let shifted = b.build_int_lshr::<i32, _, _, _>(masked, amount, "shifted")?;
        let lo = b.build_trunc(shifted, i1_ty, "lo")?;
        b.build_ret(lo)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);
        let demanded = fam.get_result::<DemandedBitsAnalysis, _>(f)?;
        assert_eq!(
            bits(demanded.get_demanded_bits(masked.as_value())),
            "11111111111111111111111111111111"
        );

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(SimplifyDemandedBitsPass, verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert!(text.contains("and i32 %0, 256"), "{text}");
        assert!(text.contains("lshr i32 %masked, %1"), "{text}");
        Ok(())
    })
}

/// Port of `llvm/lib/Analysis/DemandedBits.cpp::GetShiftedRange`: known
/// variable-shift ranges only demand source bits that can reach demanded output
/// bits.
#[test]
fn variable_lshr_with_known_amount_range_demands_reachable_source_bits() -> Result<(), IrError> {
    Module::with_new("demanded-variable-shift-range", |m| {
        let i1_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i1_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<bool, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let amount: IntValue<i32> = f.param(1)?.try_into()?;
        let amount_range =
            b.build_int_and::<i32, _, _, _>(amount, i32_ty.const_int(3_u32), "amount.range")?;
        let shifted = b.build_int_lshr::<i32, _, _, _>(a, amount_range, "shifted")?;
        let lo = b.build_trunc(shifted, i1_ty, "lo")?;
        b.build_ret(lo)?;

        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DemandedBitsAnalysis);
        let demanded = fam.get_result::<DemandedBitsAnalysis, _>(f)?;
        assert_eq!(
            bits(demanded.get_operand_demanded_bits(shifted.as_value(), 0)?),
            "00000000000000000000000000001111"
        );
        Ok(())
    })
}
