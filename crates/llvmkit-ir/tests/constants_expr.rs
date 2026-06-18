//! Constant-expression, blockaddress, and token-none tests.

use llvmkit_ir::{
    ConstantExprFlags, ConstantExprInRange, ConstantExprOpcode, GepNoWrapFlags, IRBuilder, IrError,
    Linkage, Module, OverflowingConstantExprFlags,
};

fn module_text(m: &Module<'_>) -> String {
    format!("{m}")
}

fn assert_line(text: &str, expected: &str) {
    for line in text.lines() {
        if line == expected {
            return;
        }
    }
    panic!("missing line `{expected}` in:\n{text}");
}

/// `llvmkit-specific subset`: exercises the `bitcast` constant-expression
/// storage and `llvm/lib/IR/AsmWriter.cpp::writeConstantInternal` print arm.
/// Upstream `ConstantExprNoFold.ll` covers no-folding for addrspacecast/GEP,
/// but not this bitcast spelling.
#[test]
fn constant_expr_bitcast_round_trips() -> Result<(), IrError> {
    let m = Module::new("constexpr_bitcast");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let g = m.add_global("g", i32_ty.as_type(), zero)?;
    let ptr_ty = m.ptr_type(0).as_type();
    let bitcast = m.constant_expr(
        ptr_ty,
        ConstantExprOpcode::BitCast,
        [g.as_global_constant_ptr().as_value()],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    m.add_global("p", ptr_ty, bitcast)?;
    let text = module_text(&m);
    assert_line(&text, "@p = global ptr bitcast (ptr @g to ptr)");
    m.verify_borrowed()?;
    Ok(())
}

/// `llvmkit-specific subset`: `test/Assembler/ptrtoaddr.ll` covers the
/// upstream `ptrtoaddr` spelling; this test asserts the single addrspace(0)
/// printed form exposed by llvmkit's API.
#[test]
fn constant_expr_ptrtoaddr_round_trips() -> Result<(), IrError> {
    let m = Module::new("constexpr_ptrtoaddr");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let g = m.add_global("g", i32_ty.as_type(), zero)?;
    let expr = m.constant_expr(
        m.i64_type().as_type(),
        ConstantExprOpcode::PtrToAddr,
        [g.as_global_constant_ptr().as_value()],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    m.add_global("addr", m.i64_type().as_type(), expr)?;

    let text = module_text(&m);
    assert_line(&text, "@addr = global i64 ptrtoaddr (ptr @g to i64)");
    Ok(())
}

/// `llvmkit-specific subset`: `test/Assembler/pr119818.ll` and
/// `uselistorder_bb.ll` exercise blockaddress constants; this test asserts the
/// single printed `blockaddress(@f, %entry)` form exposed by llvmkit's API.
#[test]
fn blockaddress_constant_round_trips() -> Result<(), IrError> {
    let m = Module::new("blockaddress_const");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let terminator = b.build_ret_void().1;
    assert!(terminator.is_terminator());
    let addr = m.block_address(f, entry)?;
    m.add_global("addr", m.ptr_type(0).as_type(), addr)?;

    let text = module_text(&m);
    assert_line(&text, "@addr = global ptr blockaddress(@f, %entry)");
    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `BlockAddress::get(Function*, BasicBlock*)`: the blockaddress
/// constant has the function pointer type, including function address space.
#[test]
fn blockaddress_constant_uses_function_address_space() -> Result<(), IrError> {
    let m = Module::new("blockaddress_addrspace_const");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m
        .function_builder::<()>("f", fn_ty)
        .linkage(Linkage::External)
        .address_space(2)
        .build()?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let terminator = b.build_ret_void().1;
    assert!(terminator.is_terminator());
    let addr = m.block_address(f, entry)?;
    m.add_global("addr", m.ptr_type(2).as_type(), addr)?;

    let text = module_text(&m);
    assert_line(
        &text,
        "@addr = global ptr addrspace(2) blockaddress(@f, %entry)",
    );
    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::parseValID` `kw_none` and
/// `llvm/lib/IR/AsmWriter.cpp::writeConstantInternal`: token constants print
/// as `none`.
#[test]
fn token_none_round_trips() -> Result<(), IrError> {
    let m = Module::new("token_none");
    let none = m.token_none();
    let text = format!("{}", none.as_value());
    assert_eq!(text, "token none");
    Ok(())
}

/// Mirrors LLVM 22 `LLParser::parseValID`: the constructible
/// `ConstantExprOpcode` surface is exactly the accepted parser branch set.
#[test]
fn constant_expr_opcode_surface_matches_llvm22_parse_val_id() {
    fn keyword(opcode: ConstantExprOpcode) -> &'static str {
        match opcode {
            ConstantExprOpcode::Add => "add",
            ConstantExprOpcode::Sub => "sub",
            ConstantExprOpcode::Xor => "xor",
            ConstantExprOpcode::GetElementPtr => "getelementptr",
            ConstantExprOpcode::ShuffleVector => "shufflevector",
            ConstantExprOpcode::InsertElement => "insertelement",
            ConstantExprOpcode::ExtractElement => "extractelement",
            ConstantExprOpcode::Trunc => "trunc",
            ConstantExprOpcode::PtrToAddr => "ptrtoaddr",
            ConstantExprOpcode::PtrToInt => "ptrtoint",
            ConstantExprOpcode::IntToPtr => "inttoptr",
            ConstantExprOpcode::BitCast => "bitcast",
            ConstantExprOpcode::AddrSpaceCast => "addrspacecast",
        }
    }
    let keywords = [
        ConstantExprOpcode::Add,
        ConstantExprOpcode::Sub,
        ConstantExprOpcode::Xor,
        ConstantExprOpcode::GetElementPtr,
        ConstantExprOpcode::ShuffleVector,
        ConstantExprOpcode::InsertElement,
        ConstantExprOpcode::ExtractElement,
        ConstantExprOpcode::Trunc,
        ConstantExprOpcode::PtrToAddr,
        ConstantExprOpcode::PtrToInt,
        ConstantExprOpcode::IntToPtr,
        ConstantExprOpcode::BitCast,
        ConstantExprOpcode::AddrSpaceCast,
    ]
    .map(keyword);
    assert_eq!(
        keywords.as_slice(),
        [
            "add",
            "sub",
            "xor",
            "getelementptr",
            "shufflevector",
            "insertelement",
            "extractelement",
            "trunc",
            "ptrtoaddr",
            "ptrtoint",
            "inttoptr",
            "bitcast",
            "addrspacecast",
        ]
    );
}

/// `llvmkit-specific subset` of `Verifier.cpp::Verifier::visitConstantExpr`:
/// pointer bitcasts may only bitcast to pointer types with matching address
/// spaces, but llvmkit reports its own exact diagnostic text.
#[test]
fn invalid_bitcast_constant_expr_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_invalid_bitcast");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let g = m.add_global("g", i32_ty.as_type(), zero)?;
    let ptr = g.as_global_constant_ptr().as_value();

    let err = m
        .constant_expr(
            m.i64_type().as_type(),
            ConstantExprOpcode::BitCast,
            [ptr],
            [],
            [],
            ConstantExprFlags::none(),
        )
        .expect_err("pointer-to-integer bitcast constexpr is rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid bitcast constant expression"
        }
    );

    m.constant_expr(
        m.ptr_type(0).as_type(),
        ConstantExprOpcode::BitCast,
        [ptr],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    Ok(())
}

/// `llvmkit-specific subset` of `Verifier.cpp::Verifier::visitConstantExpr`:
/// getelementptr constant expressions validate aggregate index walks, but
/// llvmkit reports its own exact diagnostic text.
#[test]
fn invalid_gep_constant_expr_indices_are_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_invalid_gep");
    let i32_ty = m.i32_type();
    let struct_ty = m.struct_type([i32_ty.as_type()], false);
    let init = struct_ty.const_struct([i32_ty.const_zero().as_constant()])?;
    let g = m.add_global("g", struct_ty.as_type(), init)?;
    let zero = i32_ty.const_zero();
    let one = i32_ty.const_int(1i32);

    let err = m
        .constant_expr_with_options(
            m.ptr_type(0).as_type(),
            ConstantExprOpcode::GetElementPtr,
            [
                g.as_global_constant_ptr().as_value(),
                zero.as_value(),
                one.as_value(),
            ],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new().source_ty(struct_ty.as_type()),
        )
        .expect_err("out-of-range struct GEP index is rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid getelementptr indices"
        }
    );
    Ok(())
}

/// Ports `ShuffleVectorInst::isValidOperands`: constant-expression mask
/// vectors are valid only when their element type is i32.
#[test]
fn invalid_shufflevector_constant_expr_non_i32_mask_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_bad_shuffle_mask");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2, false);
    let vec_i64_ty = m.vector_type(i64_ty.as_type(), 2, false);
    let one = i32_ty.const_int(1i32);
    let two = i32_ty.const_int(2i32);
    let three = i32_ty.const_int(3i32);
    let four = i32_ty.const_int(4i32);
    let lhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
    let rhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([three, four])?;
    let zero64 = i64_ty.const_zero();
    let one64 = i64_ty.const_int(1i64);
    let mask =
        vec_i64_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i64>, _>([zero64, one64])?;

    let err = m
        .constant_expr(
            vec_i32_ty.as_type(),
            ConstantExprOpcode::ShuffleVector,
            [lhs.as_value(), rhs.as_value(), mask.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )
        .expect_err("i64 shufflevector mask is rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid shufflevector constant expression"
        }
    );
    Ok(())
}

/// Ports `ShuffleVectorInst::isValidOperands`: fixed-vector mask elements
/// must be smaller than `2 * V1Size`.
#[test]
fn invalid_shufflevector_constant_expr_out_of_range_mask_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_bad_shuffle_mask_range");
    let i32_ty = m.i32_type();
    let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2, false);
    let one = i32_ty.const_int(1i32);
    let two = i32_ty.const_int(2i32);
    let three = i32_ty.const_int(3i32);
    let four = i32_ty.const_int(4i32);
    let lhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
    let rhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([three, four])?;
    let zero = i32_ty.const_zero();
    let out_of_range = i32_ty.const_int(4i32);
    let mask = vec_i32_ty
        .const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([zero, out_of_range])?;

    let err = m
        .constant_expr(
            vec_i32_ty.as_type(),
            ConstantExprOpcode::ShuffleVector,
            [lhs.as_value(), rhs.as_value(), mask.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )
        .expect_err("out-of-range shufflevector mask is rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid shufflevector constant expression"
        }
    );
    Ok(())
}

/// Ports `ShuffleVectorInst::isValidOperands`: scalable vector masks accept
/// only `undef` or true aggregate-zero constants, not explicit zero vectors.
#[test]
fn invalid_scalable_shufflevector_explicit_zero_mask_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_bad_scalable_shuffle_mask");
    let i32_ty = m.i32_type();
    let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2, true);
    let one = i32_ty.const_int(1i32);
    let two = i32_ty.const_int(2i32);
    let three = i32_ty.const_int(3i32);
    let four = i32_ty.const_int(4i32);
    let lhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
    let rhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([three, four])?;
    let zero = i32_ty.const_zero();
    let mask = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([zero, zero])?;

    let err = m
        .constant_expr(
            vec_i32_ty.as_type(),
            ConstantExprOpcode::ShuffleVector,
            [lhs.as_value(), rhs.as_value(), mask.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )
        .expect_err("explicit zero scalable shufflevector mask is rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid shufflevector constant expression"
        }
    );
    Ok(())
}

/// Ports `ConstantExpr::isSupportedGetElementPtr` and recursive
/// `Type::isScalableTy`: aggregate source types containing scalable vectors
/// are invalid constant-GEP base elements.
#[test]
fn invalid_gep_constant_expr_scalable_aggregate_source_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_scalable_gep_source");
    let i8_ty = m.i8_type();
    let scalable = m.vector_type(i8_ty.as_type(), 1, true);
    let source_ty = m.array_type(scalable.as_type(), 2);
    let ptr = m.ptr_type(0).const_null();
    let one = m.i64_type().const_int(1i64);

    let err = m
        .constant_expr_with_options(
            m.ptr_type(0).as_type(),
            ConstantExprOpcode::GetElementPtr,
            [ptr.as_value(), one.as_value()],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new().source_ty(source_ty.as_type()),
        )
        .expect_err("scalable aggregate GEP source is rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid base element for constant getelementptr"
        }
    );
    Ok(())
}

/// Ports `Constants.cpp::ConstantExprKeyType`: empty subclass flag payloads do
/// not create distinct constant-expression arena entries.
#[test]
fn empty_constant_expr_flags_are_canonicalized_before_interning() -> Result<(), IrError> {
    let m = Module::new("constexpr_empty_flags");
    let i32_ty = m.i32_type();
    let lhs = i32_ty.const_int(1i32);
    let rhs = i32_ty.const_int(2i32);

    let plain = m.constant_expr(
        i32_ty.as_type(),
        ConstantExprOpcode::Add,
        [lhs.as_value(), rhs.as_value()],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    let empty_flags = m.constant_expr(
        i32_ty.as_type(),
        ConstantExprOpcode::Add,
        [lhs.as_value(), rhs.as_value()],
        [],
        [],
        ConstantExprFlags::Overflowing(OverflowingConstantExprFlags {
            nuw: false,
            nsw: false,
        }),
    )?;

    assert_eq!(plain, empty_flags);
    Ok(())
}
/// Ports `Constants.cpp::ConstantExpr::getGetElementPtr`: once the required
/// GEP result is vector-typed, scalar sequential indices are splatted before
/// the `ConstantExprKeyType` lookup.
#[test]
fn vector_gep_scalar_sequential_indices_are_splatted_before_interning() -> Result<(), IrError> {
    let m = Module::new("constexpr_vector_gep_splat");
    let i8_ty = m.i8_type();
    let i64_ty = m.i64_type();
    let source_ty = m.array_type(i8_ty.as_type(), 4);
    let ptr = m.ptr_type(0).const_null();
    let zero = i64_ty.const_zero();
    let one = i64_ty.const_int(1i64);
    let vec_i64_ty = m.vector_type(i64_ty.as_type(), 2, false);
    let vector_index =
        vec_i64_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i64>, _>([zero, one])?;
    let result_ty = m.vector_type(m.ptr_type(0).as_type(), 2, false);

    let gep = m.constant_expr_with_options(
        result_ty.as_type(),
        ConstantExprOpcode::GetElementPtr,
        [ptr.as_value(), vector_index.as_value(), zero.as_value()],
        [],
        [],
        llvmkit_ir::ConstantExprOptions::new().source_ty(source_ty.as_type()),
    )?;
    m.add_global("slot", result_ty.as_type(), gep)?;

    let text = format!("{m}");
    assert!(
        text.contains("@slot = global <2 x ptr> getelementptr ([4 x i8], ptr null, <2 x i64> <i64 0, i64 1>, <2 x i64> <i64 0, i64 0>)"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `Constants.cpp::ConstantExpr::getGetElementPtr`: vector index element
/// counts are checked against the requested GEP result before struct-index
/// splats are scalarized.
#[test]
fn vector_gep_struct_index_width_mismatch_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_vector_gep_struct_index_width");
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let source_ty = m.struct_type([i8_ty.as_type()], false);
    let ptr_ty = m.ptr_type(0);
    let result_ty = m.vector_type(ptr_ty.as_type(), 4, false);
    let base = result_ty.const_vector([ptr_ty.const_null(); 4])?;
    let zero64 = i64_ty.const_zero();
    let zero32 = i32_ty.const_zero();
    let wrong_index_ty = m.vector_type(i32_ty.as_type(), 2, false);
    let wrong_struct_index = wrong_index_ty
        .const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([zero32, zero32])?;

    let err = m
        .constant_expr_with_options(
            result_ty.as_type(),
            ConstantExprOpcode::GetElementPtr,
            [
                base.as_value(),
                zero64.as_value(),
                wrong_struct_index.as_value(),
            ],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new().source_ty(source_ty.as_type()),
        )
        .expect_err("mismatched vector struct index width is rejected before scalarization");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid getelementptr constant expression"
        }
    );
    Ok(())
}

/// llvmkit-specific subset of `LLParser::parseValID`/`ConstantsContext.h`:
/// public `ConstantExprInRange` construction canonicalizes APInt words outside
/// the declared bit width before interning.
#[test]
fn constant_expr_gep_inrange_words_are_truncated_before_interning() -> Result<(), IrError> {
    let m = Module::new("constexpr_inrange_canonical_words");
    let i8_ty = m.i8_type();
    let g = m.add_global("g", i8_ty.as_type(), i8_ty.const_zero())?;
    let ptr = g.as_global_constant_ptr();
    let offset = m.i64_type().const_int(1i64);
    let canonical_range = ConstantExprInRange {
        start: Box::from([0]),
        end: Box::from([1]),
        bit_width: 64,
    };
    let high_word_range = ConstantExprInRange {
        start: Box::from([0, u64::MAX]),
        end: Box::from([1, u64::MAX]),
        bit_width: 64,
    };

    let canonical = m.constant_expr_with_options(
        m.ptr_type(0).as_type(),
        ConstantExprOpcode::GetElementPtr,
        [ptr.as_value(), offset.as_value()],
        [],
        [],
        llvmkit_ir::ConstantExprOptions::new()
            .source_ty(i8_ty.as_type())
            .flags(ConstantExprFlags::gep(
                GepNoWrapFlags::empty(),
                Some(canonical_range),
            )),
    )?;
    let high_word = m.constant_expr_with_options(
        m.ptr_type(0).as_type(),
        ConstantExprOpcode::GetElementPtr,
        [ptr.as_value(), offset.as_value()],
        [],
        [],
        llvmkit_ir::ConstantExprOptions::new()
            .source_ty(i8_ty.as_type())
            .flags(ConstantExprFlags::gep(
                GepNoWrapFlags::empty(),
                Some(high_word_range),
            )),
    )?;

    assert_eq!(canonical, high_word);
    Ok(())
}

/// llvmkit-specific subset of `LLParser::parseValID` constant-GEP `inrange`
/// handling: the parser truncates/extents endpoints to the base pointer's
/// index width before constructing the range, so the public constructor rejects
/// non-canonical widths.
#[test]
fn constant_expr_gep_inrange_width_must_match_base_index_width() -> Result<(), IrError> {
    let m = Module::new("constexpr_inrange_width");
    let i8_ty = m.i8_type();
    let g = m.add_global("g", i8_ty.as_type(), i8_ty.const_zero())?;
    let offset = m.i64_type().const_int(1i64);
    let wrong_width_range = ConstantExprInRange {
        start: Box::from([0]),
        end: Box::from([1]),
        bit_width: 32,
    };

    let err = m
        .constant_expr_with_options(
            m.ptr_type(0).as_type(),
            ConstantExprOpcode::GetElementPtr,
            [g.as_global_constant_ptr().as_value(), offset.as_value()],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new()
                .source_ty(i8_ty.as_type())
                .flags(ConstantExprFlags::gep(
                    GepNoWrapFlags::empty(),
                    Some(wrong_width_range),
                )),
        )
        .expect_err("public GEP inrange width must be canonical");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid getelementptr inrange bit width"
        }
    );
    Ok(())
}

/// Ports `GetElementPtrInst::getGEPReturnType` plus
/// `LLParser::convertValIDToValue`: a vector GEP result preserves the base
/// pointer address space, and a mismatched annotated result type is rejected.
#[test]
fn invalid_gep_constant_expr_address_space_mismatch_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_invalid_gep_addrspace");
    let i8_ty = m.i8_type();
    let target = m
        .global_builder("g", i8_ty.as_type())
        .address_space(1)
        .initializer(i8_ty.const_zero())
        .build()?;
    let i64_ty = m.i64_type();
    let vec_i64_ty = m.vector_type(i64_ty.as_type(), 2, false);
    let zero = i64_ty.const_zero();
    let one = i64_ty.const_int(1i64);
    let vector_index =
        vec_i64_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i64>, _>([zero, one])?;
    let wrong_result_ty = m.vector_type(m.ptr_type(0).as_type(), 2, false);

    let err = m
        .constant_expr_with_options(
            wrong_result_ty.as_type(),
            ConstantExprOpcode::GetElementPtr,
            [
                target.as_global_constant_ptr().as_value(),
                vector_index.as_value(),
            ],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new().source_ty(i8_ty.as_type()),
        )
        .expect_err("GEP result address space must match the base pointer");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "invalid getelementptr constant expression"
        }
    );
    Ok(())
}

/// Ports `Constants.cpp::ConstantPtrAuth::get`: the C++ signature requires
/// pointer-shaped operands, and llvmkit reports the equivalent runtime
/// diagnostic for its generic Rust input API. Writer elision removes only
/// trailing defaults.
#[test]
fn ptrauth_constructor_requires_five_operand_shape() -> Result<(), IrError> {
    let m = Module::new("ptrauth_constructor");
    let i8_ty = m.i8_type();
    let g = m.add_global("g", i8_ty.as_type(), i8_ty.const_zero())?;
    let ptr = g.as_global_constant_ptr();
    let key = m.i32_type().const_zero();
    let disc = m.i64_type().const_int(1i64);
    let addr_disc = m.ptr_type(0).const_null();
    let signed = m.ptr_auth(ptr, key, disc, addr_disc, ptr)?;
    m.add_global("signed", m.ptr_type(0).as_type(), signed)?;

    let text = module_text(&m);
    assert_line(
        &text,
        "@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr null, ptr @g)",
    );

    let bad = m.ptr_auth(ptr, key, disc, addr_disc, key);
    assert_eq!(
        bad,
        Err(IrError::InvalidOperation {
            message: "constant ptrauth deactivation symbol must be a pointer",
        })
    );

    let constant_expr_pointer = m.constant_expr(
        m.ptr_type(0).as_type(),
        ConstantExprOpcode::BitCast,
        [addr_disc.as_value()],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    let bad_deactivation = m.ptr_auth(ptr, key, disc, addr_disc, constant_expr_pointer);
    assert_eq!(
        bad_deactivation,
        Err(IrError::InvalidOperation {
            message: "constant ptrauth deactivation symbol must be a global value or null",
        })
    );

    let defaulted = m.ptr_auth(ptr, key, m.i64_type().const_zero(), addr_disc, addr_disc)?;
    m.add_global("defaulted", m.ptr_type(0).as_type(), defaulted)?;
    let text = module_text(&m);
    assert_line(&text, "@defaulted = global ptr ptrauth (ptr @g, i32 0)");
    Ok(())
}
