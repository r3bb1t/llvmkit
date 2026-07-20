//! Constant-expression, blockaddress, and token-none tests.

use llvmkit_ir::{
    ConstantExprFlags, ConstantExprInRange, ConstantExprOpcode, Dyn, GepNoWrapFlags, IRBuilder,
    IrError, Linkage, Module, ModuleBrand, OverflowingConstantExprFlags,
};

fn module_text<'ctx, B: ModuleBrand, S>(m: &Module<'ctx, B, S>) -> String {
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

/// Port of `ConstantFold.cpp::ConstantFoldCastInstruction`: null operands fold
/// to the destination null value before PPC double-double bitcast target checks
/// can force the expression to remain unfolded.
#[test]
fn constant_expr_bitcast_round_trips() -> Result<(), IrError> {
    Module::with_new("constexpr_bitcast", |m| {
        let i128_ty = m.i128_type();
        let bits = i128_ty.const_int(0_i128);
        let ppc_ty = m.ppc_fp128_type();
        let bitcast = m.constant_expr(
            ppc_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [bits.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        m.add_global("p", bitcast)?;
        let text = module_text(&m);
        assert_line(
            &text,
            "@p = global ppc_fp128 0xM00000000000000000000000000000000",
        );
        m.verify_borrowed()?;
        Ok(())
    })
}

/// `llvmkit-specific subset`: `test/Assembler/ptrtoaddr.ll` covers the
/// upstream `ptrtoaddr` spelling; this test asserts the single addrspace(0)
/// printed form exposed by llvmkit's API.
#[test]
fn constant_expr_ptrtoaddr_round_trips() -> Result<(), IrError> {
    Module::with_new("constexpr_ptrtoaddr", |m| {
        let i32_ty = m.i32_type();
        let zero = i32_ty.const_int(0i32);
        let g = m.add_global("g", zero)?;
        let expr = m.constant_expr(
            m.i64_type().as_type(),
            ConstantExprOpcode::PtrToAddr,
            [g.as_global_constant_ptr().as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        m.add_global("addr", expr)?;

        let text = module_text(&m);
        assert_line(&text, "@addr = global i64 ptrtoaddr (ptr @g to i64)");
        Ok(())
    })
}

/// `llvmkit-specific subset`: `test/Assembler/pr119818.ll` and
/// `uselistorder_bb.ll` exercise blockaddress constants; this test asserts the
/// single printed `blockaddress(@f, %entry)` form exposed by llvmkit's API.
#[test]
fn blockaddress_constant_round_trips() -> Result<(), IrError> {
    Module::with_new("blockaddress_const", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let addr = m.block_address(f, &entry)?;
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let terminator = b.build_ret_void()?.1;
        assert!(terminator.is_terminator());
        m.add_global("addr", addr)?;

        let text = module_text(&m);
        assert_line(&text, "@addr = global ptr blockaddress(@f, %entry)");
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `BlockAddress::get(Function*, BasicBlock*)`: the blockaddress
/// constant has the function pointer type, including function address space.
#[test]
fn blockaddress_constant_uses_function_address_space() -> Result<(), IrError> {
    Module::with_new("blockaddress_addrspace_const", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m
            .function_builder::<(), _>("f", fn_ty)
            .linkage(Linkage::External)
            .address_space(2)
            .build()?;
        let entry = f.append_basic_block(&m, "entry");
        let addr = m.block_address(f, &entry)?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let terminator = b.build_ret_void().1;
        assert!(terminator.is_terminator());
        m.add_global("addr", addr)?;

        let text = module_text(&m);
        assert_line(
            &text,
            "@addr = global ptr addrspace(2) blockaddress(@f, %entry)",
        );
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::parseValID` `kw_none` and
/// `llvm/lib/IR/AsmWriter.cpp::writeConstantInternal`: token constants print
/// as `none`.
#[test]
fn token_none_round_trips() -> Result<(), IrError> {
    Module::with_new("token_none", |m| {
        let none = m.token_none();
        let text = format!("{}", none.as_value());
        assert_eq!(text, "token none");
        Ok(())
    })
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

/// Port of `Instructions.cpp::CastInst::castIsValid`: scalar pointers may
/// bitcast to and from one-lane fixed pointer vectors in the same address
/// space.
#[test]
fn bitcast_scalar_pointer_and_one_lane_pointer_vector_round_trip() -> Result<(), IrError> {
    Module::with_new("constexpr_ptr_vec_bitcast", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let vec_ptr_ty = m.vector_type(ptr_ty.as_type(), 1, false);
        let g = m.add_global("g", i32_ty.const_zero())?;
        let scalar = g.as_global_constant_ptr();
        let to_vec = m.constant_expr(
            vec_ptr_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [scalar.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let vector = vec_ptr_ty.const_vector::<llvmkit_ir::Constant<'_>, _>([scalar])?;
        let to_scalar = m.constant_expr(
            ptr_ty.as_type(),
            ConstantExprOpcode::BitCast,
            [vector.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        m.add_global("to_vec", to_vec)?;
        m.add_global("to_scalar", to_scalar)?;
        let text = module_text(&m);
        assert_line(
            &text,
            "@to_vec = global <1 x ptr> bitcast (ptr @g to <1 x ptr>)",
        );
        assert_line(
            &text,
            "@to_scalar = global ptr bitcast (<1 x ptr> <ptr @g> to ptr)",
        );
        Ok(())
    })
}

/// `llvmkit-specific subset` of `Verifier.cpp::Verifier::visitConstantExpr`:
/// pointer bitcasts may only bitcast to pointer types with matching address
/// spaces, but llvmkit reports its own exact diagnostic text.
#[test]
fn invalid_bitcast_constant_expr_is_rejected() -> Result<(), IrError> {
    Module::with_new("constexpr_invalid_bitcast", |m| {
        let i32_ty = m.i32_type();
        let zero = i32_ty.const_int(0i32);
        let g = m.add_global("g", zero)?;
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
    })
}

/// `llvmkit-specific subset` of `Verifier.cpp::Verifier::visitConstantExpr`:
/// getelementptr constant expressions validate aggregate index walks, but
/// llvmkit reports its own exact diagnostic text.
#[test]
fn invalid_gep_constant_expr_indices_are_rejected() -> Result<(), IrError> {
    Module::with_new("constexpr_invalid_gep", |m| {
        let i32_ty = m.i32_type();
        let struct_ty = m.struct_type([i32_ty.as_type()], false);
        let init = struct_ty.const_struct([i32_ty.const_zero().as_constant()])?;
        let g = m.add_global("g", init)?;
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
    })
}

/// Ports `ShuffleVectorInst::isValidOperands`: constant-expression mask
/// vectors are valid only when their element type is i32.
#[test]
fn invalid_shufflevector_constant_expr_non_i32_mask_is_rejected() -> Result<(), IrError> {
    Module::with_new("constexpr_bad_shuffle_mask", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let vec_i64_ty = m.vector_type(i64_ty.as_type(), 2, false);
        let one = i32_ty.const_int(1i32);
        let two = i32_ty.const_int(2i32);
        let three = i32_ty.const_int(3i32);
        let four = i32_ty.const_int(4i32);
        let lhs =
            vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
        let rhs =
            vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([three, four])?;
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
    })
}

/// Ports `ShuffleVectorInst::isValidOperands`: fixed-vector mask elements
/// must be smaller than `2 * V1Size`.
#[test]
fn invalid_shufflevector_constant_expr_out_of_range_mask_is_rejected() -> Result<(), IrError> {
    Module::with_new("constexpr_bad_shuffle_mask_range", |m| {
        let i32_ty = m.i32_type();
        let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let one = i32_ty.const_int(1i32);
        let two = i32_ty.const_int(2i32);
        let three = i32_ty.const_int(3i32);
        let four = i32_ty.const_int(4i32);
        let lhs =
            vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
        let rhs =
            vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([three, four])?;
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
    })
}

/// Mirrors `llvm/lib/IR/ConstantFold.cpp::ConstantFoldShuffleVectorInstruction`:
/// constant-expression `shufflevector` folds using the vector mask operand.
#[test]
fn shufflevector_constant_expr_uses_mask_operand_when_folding() -> Result<(), IrError> {
    Module::with_new("constexpr_shuffle_fold", |m| {
        let i32_ty = m.i32_type();
        let src_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let result_ty = m.vector_type(i32_ty.as_type(), 3, false);
        let one = i32_ty.const_int(1_i32);
        let two = i32_ty.const_int(2_i32);
        let three = i32_ty.const_int(3_i32);
        let four = i32_ty.const_int(4_i32);
        let lhs = src_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
        let rhs = src_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([three, four])?;
        let zero = i32_ty.const_zero();
        let rhs_lane_one = i32_ty.const_int(3_i32);
        let lhs_lane_one = i32_ty.const_int(1_i32);
        let mask = result_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            zero,
            rhs_lane_one,
            lhs_lane_one,
        ])?;

        let folded = m.constant_expr(
            result_ty.as_type(),
            ConstantExprOpcode::ShuffleVector,
            [lhs.as_value(), rhs.as_value(), mask.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        m.add_global("shuf", folded)?;
        let text = module_text(&m);
        assert_line(&text, "@shuf = global <3 x i32> <i32 1, i32 4, i32 2>");
        Ok(())
    })
}

/// Ports `ShuffleVectorInst::isValidOperands` / `getShuffleMask`: top-level
/// poison and scalable undef masks are undef-equivalent mask constants, and
/// constant-expression shuffles fold all-poison masks to poison results.
#[test]
fn shufflevector_constant_expr_poison_and_scalable_undef_masks_fold() -> Result<(), IrError> {
    Module::with_new("constexpr_shuffle_poison_masks", |m| {
        let i32_ty = m.i32_type();
        let fixed_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let one = i32_ty.const_int(1_i32);
        let two = i32_ty.const_int(2_i32);
        let lhs = fixed_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
        let rhs = fixed_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([one, two])?;
        let fixed_mask = fixed_ty.as_type().get_poison().as_constant();
        let folded = m.constant_expr(
            fixed_ty.as_type(),
            ConstantExprOpcode::ShuffleVector,
            [lhs.as_value(), rhs.as_value(), fixed_mask.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(folded, fixed_ty.as_type().get_poison().as_constant());

        let scalable_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let lhs = scalable_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(1_i32),
        ])?;
        let rhs = scalable_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(2_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let scalable_mask = scalable_ty.as_type().get_undef().as_constant();
        let folded = m.constant_expr(
            scalable_ty.as_type(),
            ConstantExprOpcode::ShuffleVector,
            [lhs.as_value(), rhs.as_value(), scalable_mask.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        assert_eq!(folded, scalable_ty.as_type().get_poison().as_constant());
        Ok(())
    })
}

/// llvmkit-specific verifier guard for `Constants.cpp::ConstantExpr::getShuffleVector`:
/// the parser-style third mask operand is the only shufflevector mask payload,
/// so direct API callers cannot append a fourth raw mask list.
#[test]
fn shufflevector_constant_expr_rejects_extra_raw_mask_payload() -> Result<(), IrError> {
    Module::with_new("constexpr_shuffle_extra_mask", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, false);
        let lhs = vec_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let rhs = vec_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
        ])?;
        let mask = vec_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            i32_ty.const_zero(),
            i32_ty.const_int(1_i32),
        ])?;

        let err = m
            .constant_expr(
                vec_ty.as_type(),
                ConstantExprOpcode::ShuffleVector,
                [lhs.as_value(), rhs.as_value(), mask.as_value()],
                [],
                [0_i32],
                ConstantExprFlags::none(),
            )
            .expect_err("raw mask payload is rejected for parser-style shufflevector");
        assert_eq!(
            err,
            IrError::InvalidOperation {
                message: "invalid shufflevector constant expression"
            }
        );
        Ok(())
    })
}

/// Ports `ShuffleVectorInst::isValidOperands`: scalable vector masks accept
/// undef and aggregate-zero constants; llvmkit's aggregate-zero representation
/// prints as `zeroinitializer`.
#[test]
fn scalable_shufflevector_zero_mask_is_accepted() -> Result<(), IrError> {
    Module::with_new("constexpr_scalable_shuffle_zero_mask", |m| {
        let i32_ty = m.i32_type();
        let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let lhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1i32),
            i32_ty.const_int(2i32),
        ])?;
        let rhs = vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(3i32),
            i32_ty.const_int(4i32),
        ])?;
        let zero = i32_ty.const_zero();
        let mask =
            vec_i32_ty.const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([zero, zero])?;

        let expr = m.constant_expr(
            vec_i32_ty.as_type(),
            ConstantExprOpcode::ShuffleVector,
            [lhs.as_value(), rhs.as_value(), mask.as_value()],
            [],
            [],
            ConstantExprFlags::none(),
        )?;
        let text = format!("{}", expr.as_value());
        assert!(text.contains("zeroinitializer"), "{text}");
        Ok(())
    })
}

/// Ports `ConstantExpr::isSupportedGetElementPtr` and recursive
/// `Type::isScalableTy`: aggregate source types containing scalable vectors
/// are invalid constant-GEP base elements.
#[test]
fn invalid_gep_constant_expr_scalable_aggregate_source_is_rejected() -> Result<(), IrError> {
    Module::with_new("constexpr_scalable_gep_source", |m| {
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
    })
}

/// Ports `Constants.cpp::ConstantExprKeyType`: empty subclass flag payloads do
/// not create distinct constant-expression arena entries.
#[test]
fn empty_constant_expr_flags_are_canonicalized_before_interning() -> Result<(), IrError> {
    Module::with_new("constexpr_empty_flags", |m| {
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
            ConstantExprFlags::Overflowing(OverflowingConstantExprFlags::none()),
        )?;

        assert_eq!(plain, empty_flags);
        Ok(())
    })
}
/// Ports `Constants.cpp::ConstantExpr::getGetElementPtr`: once the required
/// GEP result is vector-typed, scalar sequential indices are splatted before
/// the `ConstantExprKeyType` lookup.
#[test]
fn vector_gep_scalar_sequential_indices_are_splatted_before_interning() -> Result<(), IrError> {
    Module::with_new("constexpr_vector_gep_splat", |m| {
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
        m.add_global("slot", gep)?;

        let text = format!("{m}");
        assert!(
            text.contains("@slot = global <2 x ptr> getelementptr ([4 x i8], ptr null, <2 x i64> <i64 0, i64 1>, <2 x i64> zeroinitializer)"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Ports `Constants.cpp::ConstantExpr::getGetElementPtr`: vector index element
/// counts are checked against the requested GEP result before struct-index
/// splats are scalarized.
#[test]
fn vector_gep_struct_index_width_mismatch_is_rejected() -> Result<(), IrError> {
    Module::with_new("constexpr_vector_gep_struct_index_width", |m| {
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
    })
}

/// llvmkit-specific subset of `LLParser::parseValID`/`ConstantsContext.h`:
/// public `ConstantExprInRange` construction canonicalizes APInt words outside
/// the declared bit width before interning.
#[test]
fn constant_expr_gep_inrange_words_are_truncated_before_interning() -> Result<(), IrError> {
    Module::with_new("constexpr_inrange_canonical_words", |m| {
        let i8_ty = m.i8_type();
        let g = m.add_global("g", i8_ty.const_zero())?;
        let ptr = g.as_global_constant_ptr();
        let offset = m.i64_type().const_int(1i64);
        let canonical_range = ConstantExprInRange::new(Box::from([0]), Box::from([1]), 64);
        let high_word_range =
            ConstantExprInRange::new(Box::from([0, u64::MAX]), Box::from([1, u64::MAX]), 64);

        let canonical = m.constant_expr_with_options(
            m.ptr_type(0).as_type(),
            ConstantExprOpcode::GetElementPtr,
            [ptr.as_value(), offset.as_value()],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new()
                .source_ty(i8_ty.as_type())
                .flags(ConstantExprFlags::gep_with_in_range(
                    GepNoWrapFlags::empty(),
                    canonical_range,
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
                .flags(ConstantExprFlags::gep_with_in_range(
                    GepNoWrapFlags::empty(),
                    high_word_range,
                )),
        )?;

        assert_eq!(canonical, high_word);
        Ok(())
    })
}

/// llvmkit-specific subset of `LLParser::parseValID` constant-GEP `inrange`
/// handling: the parser truncates/extents endpoints to the base pointer's
/// index width before constructing the range, so the public constructor rejects
/// non-canonical widths.
#[test]
fn constant_expr_gep_inrange_width_must_match_base_index_width() -> Result<(), IrError> {
    Module::with_new("constexpr_inrange_width", |m| {
        let i8_ty = m.i8_type();
        let g = m.add_global("g", i8_ty.const_zero())?;
        let offset = m.i64_type().const_int(1i64);
        let wrong_width_range = ConstantExprInRange::new(Box::from([0]), Box::from([1]), 32);

        let err = m
            .constant_expr_with_options(
                m.ptr_type(0).as_type(),
                ConstantExprOpcode::GetElementPtr,
                [g.as_global_constant_ptr().as_value(), offset.as_value()],
                [],
                [],
                llvmkit_ir::ConstantExprOptions::new()
                    .source_ty(i8_ty.as_type())
                    .flags(ConstantExprFlags::gep_with_in_range(
                        GepNoWrapFlags::empty(),
                        wrong_width_range,
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
    })
}

/// Ports `GetElementPtrInst::getGEPReturnType` plus
/// `LLParser::convertValIDToValue`: a vector GEP result preserves the base
/// pointer address space, and a mismatched annotated result type is rejected.
#[test]
fn invalid_gep_constant_expr_address_space_mismatch_is_rejected() -> Result<(), IrError> {
    Module::with_new("constexpr_invalid_gep_addrspace", |m| {
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
    })
}

/// Ports `Constants.cpp::ConstantPtrAuth::get`: the C++ signature requires
/// pointer-shaped operands, and llvmkit reports the equivalent runtime
/// diagnostic for its generic Rust input API. Writer elision removes only
/// trailing defaults.
#[test]
fn ptrauth_constructor_requires_five_operand_shape() -> Result<(), IrError> {
    Module::with_new("ptrauth_constructor", |m| {
        let i8_ty = m.i8_type();
        let g = m.add_global("g", i8_ty.const_zero())?;
        let ptr = g.as_global_constant_ptr();
        let key = m.i32_type().const_zero();
        let disc = m.i64_type().const_int(1i64);
        let addr_disc = m.ptr_type(0).const_null();
        let signed = m.ptr_auth(ptr, key, disc, addr_disc, ptr)?;
        m.add_global("signed", signed)?;

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

        let constant_expr_pointer = m.constant_expr_with_options(
            m.ptr_type(0).as_type(),
            ConstantExprOpcode::GetElementPtr,
            [addr_disc.as_value(), disc.as_value()],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new().source_ty(i8_ty.as_type()),
        )?;
        let bad_deactivation = m.ptr_auth(ptr, key, disc, addr_disc, constant_expr_pointer);
        assert_eq!(
            bad_deactivation,
            Err(IrError::InvalidOperation {
                message: "constant ptrauth deactivation symbol must be a global value or null",
            })
        );

        let defaulted = m.ptr_auth(ptr, key, m.i64_type().const_zero(), addr_disc, addr_disc)?;
        m.add_global("defaulted", defaulted)?;
        let text = module_text(&m);
        assert_line(&text, "@defaulted = global ptr ptrauth (ptr @g, i32 0)");
        Ok(())
    })
}
