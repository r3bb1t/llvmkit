//! IRBuilder folder strategy tests.
//!
//! Source-derived folder behavior from `llvm/include/llvm/IR/ConstantFolder.h`,
//! `llvm/include/llvm/IR/IRBuilder.h`, and `llvm/include/llvm/IR/NoFolder.h`,
//! plus exact `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`.

use llvmkit_ir::instr_types::CastOpcode;
use llvmkit_ir::{
    BinaryIntrinsic, BinaryOpcode, CastKind, Constant, ConstantFloatValue, ConstantFolder,
    ConstantIntValue, GepNoWrapFlags, IRBuilder, IRBuilderFolder, InstructionKind, InstructionView,
    IntDyn, IntPredicate, IntValue, IntWidth, IrError, IrResult, Linkage, Module, MulFlags,
    NoFolder, OverflowFlags, PointerValue, ShlFlags, Type, UDivFlags, Value,
    constant_fold_binary_instruction,
};

#[derive(Debug, Clone, Copy)]
enum FolderReturn<'ctx> {
    Value(Value<'ctx>),
    NoWrapCall {
        opcode: BinaryOpcode,
        has_nuw: bool,
        has_nsw: bool,
        value: Value<'ctx>,
    },
}

#[derive(Debug, Clone, Copy)]
struct ReturningFolder<'ctx> {
    result: FolderReturn<'ctx>,
}

impl<'ctx> ReturningFolder<'ctx> {
    fn fold(&self) -> IrResult<Option<Value<'ctx>>> {
        match self.result {
            FolderReturn::Value(value) => Ok(Some(value)),
            FolderReturn::NoWrapCall { .. } => Ok(None),
        }
    }
}

/// Only the two hooks exercised by this file's tests need a real body;
/// every other hook keeps the trait's default "decline to fold" (the
/// erased `*_dyn` hooks all default to `Ok(None)`, and the typed hooks
/// default-delegate to them), matching upstream `IRBuilderFolder.h`'s
/// posture that a custom folder overrides only what it cares about.
impl<'ctx> IRBuilderFolder<'ctx> for ReturningFolder<'ctx> {
    fn fold_bin_op_dyn(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_no_wrap_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        flags: OverflowFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        match self.result {
            FolderReturn::NoWrapCall {
                opcode: expected_opcode,
                has_nuw: expected_nuw,
                has_nsw: expected_nsw,
                value,
            } if opcode == expected_opcode
                && flags.has_nuw() == expected_nuw
                && flags.has_nsw() == expected_nsw =>
            {
                Ok(Some(value))
            }
            _ => self.fold(),
        }
    }
}

/// llvmkit-specific subset of `ConstantFolder.h::CreateFNeg`: an all-constant
/// fneg is folded by the default folder and no instruction is inserted.
#[test]
fn constant_folder_folds_fneg_constant_without_instruction() -> Result<(), IrError> {
    Module::with_new("folder-fneg", |m| {
        let f32_ty = m.f32_type();
        let fn_ty = m.fn_type(f32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<f32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);

        let result = b.build_float_neg::<f32, _, _>(f32_ty.const_float(1.0), "n")?;

        let folded = ConstantFloatValue::<f32>::try_from(Constant::try_from(result.as_value())?)?;
        assert!(folded.ap_float().is_exactly_value_f64(-1.0));
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolder.h::CreateUDiv`: all-constant
/// invalid integer divide folds to poison and no instruction is inserted.
#[test]
fn constant_folder_folds_udiv_by_zero_to_poison_without_instruction() -> Result<(), IrError> {
    Module::with_new("folder-udiv", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);

        let result =
            b.build_int_udiv::<i32, _, _, _>(i32_ty.const_int(42_i32), i32_ty.const_zero(), "q")?;

        assert_eq!(
            Constant::try_from(result.as_value())?,
            i32_ty.as_type().get_poison().as_constant()
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/ConstantFolder.h::ConstantFolder::FoldExactBinOp`
/// lines 56-67: non-desirable exact binops delegate to the plain
/// `ConstantFoldBinaryInstruction` path, so exact `udiv` does not poison an
/// inexact all-constant quotient through `ConstantFolder`.
#[test]
fn constant_folder_exact_udiv_inexact_constants_match_upstream_plain_fold() -> Result<(), IrError> {
    Module::with_new("folder-exact-udiv", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);

        let result = b.build_int_udiv_with_flags::<i32, _, _, _>(
            i32_ty.const_int(7_i32),
            i32_ty.const_int(3_i32),
            UDivFlags::new().exact(),
            "q",
        )?;

        assert_eq!(
            Constant::try_from(result.as_value())?,
            i32_ty.const_int(2_i32).as_constant()
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/include/llvm/IR/ConstantFolder.h::FoldNoWrapBinOp`:
/// all-constant non-`ConstantExpr` `mul` delegates to
/// `llvm/lib/IR/ConstantFold.cpp::ConstantFoldBinaryInstruction`.
#[test]
fn constant_folder_no_wrap_mul_delegates_to_binary_constant_fold() -> Result<(), IrError> {
    Module::with_new("folder-nowrap-mul", |m| {
        let i32_ty = m.i32_type();
        let lhs = i32_ty.const_int(6_i32);
        let rhs = i32_ty.const_int(7_i32);
        let expected = constant_fold_binary_instruction(
            BinaryOpcode::Mul,
            lhs.as_constant(),
            rhs.as_constant(),
        )?
        .expect("all-constant mul folds through ConstantFoldBinaryInstruction");
        let folded = ConstantFolder
            .fold_no_wrap_bin_op_dyn(
                BinaryOpcode::Mul,
                lhs.as_value(),
                rhs.as_value(),
                OverflowFlags::new().nuw(),
            )?
            .expect("all-constant no-wrap mul folds");

        assert_eq!(Constant::try_from(folded)?, expected);
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/include/llvm/IR/ConstantFolder.h::FoldNoWrapBinOp`:
/// all-constant non-`ConstantExpr` `shl` delegates to
/// `llvm/lib/IR/ConstantFold.cpp::ConstantFoldBinaryInstruction`.
#[test]
fn constant_folder_no_wrap_shl_delegates_to_binary_constant_fold() -> Result<(), IrError> {
    Module::with_new("folder-nowrap-shl", |m| {
        let i32_ty = m.i32_type();
        let lhs = i32_ty.const_int(1_i32);
        let rhs = i32_ty.const_int(3_i32);
        let expected = constant_fold_binary_instruction(
            BinaryOpcode::Shl,
            lhs.as_constant(),
            rhs.as_constant(),
        )?
        .expect("all-constant shl folds through ConstantFoldBinaryInstruction");
        let folded = ConstantFolder
            .fold_no_wrap_bin_op_dyn(
                BinaryOpcode::Shl,
                lhs.as_value(),
                rhs.as_value(),
                OverflowFlags::new().nuw().nsw(),
            )?
            .expect("all-constant no-wrap shl folds");

        assert_eq!(Constant::try_from(folded)?, expected);
        Ok(())
    })
}

/// llvmkit-specific direct Rust hook coverage for
/// `llvm/include/llvm/IR/ConstantFolder.h::ConstantFolder::FoldNoWrapBinOp`
/// lines 69-85: the default folder does not prefilter opcodes before delegating
/// non-desirable all-constant binops to `ConstantFoldBinaryInstruction`
/// (`llvm/lib/IR/ConstantFold.cpp` lines 598-955).
#[test]
fn constant_folder_no_wrap_direct_hook_matches_upstream_for_xor_and_and() -> Result<(), IrError> {
    Module::with_new("folder-nowrap-direct", |m| {
        let i32_ty = m.i32_type();

        let xor = ConstantFolder
            .fold_no_wrap_bin_op_dyn(
                BinaryOpcode::Xor,
                i32_ty.const_int(5_i32).as_value(),
                i32_ty.const_int(3_i32).as_value(),
                OverflowFlags::new().nuw(),
            )?
            .expect("all-constant xor folds through direct no-wrap hook");
        let xor = ConstantIntValue::<IntDyn>::try_from(Constant::try_from(xor)?)?;
        assert_eq!(xor.ap_int(), i32_ty.const_int(6_i32).ap_int());

        let and = ConstantFolder
            .fold_no_wrap_bin_op_dyn(
                BinaryOpcode::And,
                i32_ty.const_int(5_i32).as_value(),
                i32_ty.const_zero().as_value(),
                OverflowFlags::new().nuw().nsw(),
            )?
            .expect("all-constant and folds through direct no-wrap hook");
        let and = ConstantIntValue::<IntDyn>::try_from(Constant::try_from(and)?)?;
        assert_eq!(and.ap_int(), i32_ty.const_zero().ap_int());
        Ok(())
    })
}

/// llvmkit-specific direct Rust hook coverage for
/// `llvm/include/llvm/IR/ConstantFolder.h::ConstantFolder::FoldBinaryIntrinsic`
/// lines 184-188: default `ConstantFolder` declines intrinsic folding.
#[test]
fn constant_folder_binary_intrinsic_declines() -> Result<(), IrError> {
    Module::with_new("folder-intrinsic", |m| {
        let i32_ty = m.i32_type();
        assert_eq!(
            ConstantFolder.fold_binary_intrinsic_dyn(
                BinaryIntrinsic::UMax,
                i32_ty.const_int(1_i32).as_value(),
                i32_ty.const_int(2_i32).as_value(),
                i32_ty.as_type(),
            )?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific direct Rust hook coverage for
/// `llvm/include/llvm/IR/ConstantFolder.h::ConstantFolder::FoldGEP` lines
/// 107-118 and `Constants.cpp::ConstantExpr::getGetElementPtr`: a scalar
/// pointer plus vector index constructs a vector-of-pointer constant expr.
#[test]
fn constant_folder_vector_gep_nonzero_index_builds_vector_expr() -> Result<(), IrError> {
    Module::with_new("folder-vector-gep", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let ptr_vec_ty = m.vector_type(m.ptr_type(0).as_type(), 2, false);
        let index_ty = m.vector_type(i64_ty.as_type(), 2, false);
        let g = m.add_global("g", i32_ty.as_type(), i32_ty.const_zero())?;
        let index = index_ty.const_vector::<ConstantIntValue<'_, i64>, _>([
            i64_ty.const_int(1_i64),
            i64_ty.const_int(2_i64),
        ])?;

        let folded = ConstantFolder
            .fold_gep_dyn(
                i32_ty.as_type(),
                g.as_global_constant_ptr().as_value(),
                &[index.as_value()],
                GepNoWrapFlags::empty(),
            )?
            .expect("vector GEP constexpr constructed");
        let folded = Constant::try_from(folded)?;
        assert_eq!(folded.ty(), ptr_vec_ty.as_type());
        m.add_global("gep", ptr_vec_ty.as_type(), folded)?;
        let text = format!("{m}");
        assert!(
            text.contains(
                "@gep = global <2 x ptr> getelementptr (i32, ptr @g, <2 x i64> <i64 1, i64 2>)"
            ),
            "{text}"
        );
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolder.h::FoldGEP` lines 107-118
/// and `Type.cpp::Type::isScalableTy`: scalable target extension source
/// element types are unsupported for GEP constant expressions.
#[test]
fn constant_folder_gep_declines_scalable_target_ext_source_type() -> Result<(), IrError> {
    Module::with_new("folder-gep-scalable-target-ext", |m| {
        let source_ty = m.target_ext_type("aarch64.svcount", Vec::<Type>::new(), Vec::<u32>::new());
        let ptr = m.ptr_type(0).const_null().as_constant();

        assert_eq!(
            ConstantFolder.fold_gep_dyn(
                source_ty.as_type(),
                ptr.as_value(),
                &[],
                GepNoWrapFlags::empty(),
            )?,
            None
        );
        Ok(())
    })
}

/// llvmkit-specific direct Rust hook coverage for
/// `llvm/include/llvm/IR/ConstantFolder.h::ConstantFolder::FoldShuffleVector`
/// lines 165-172 and `Constants.cpp::ConstantExpr::getShuffleVector`: scalable
/// zero-mask shuffles build a scalable mask constant for the fallback constexpr.
#[test]
fn constant_folder_scalable_shuffle_builds_scalable_mask_expr() -> Result<(), IrError> {
    Module::with_new("folder-scalable-shuffle", |m| {
        let i32_ty = m.i32_type();
        let vec_ty = m.vector_type(i32_ty.as_type(), 2, true);
        let lhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
        ])?;
        let rhs = vec_ty.const_vector::<ConstantIntValue<'_, i32>, _>([
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
        ])?;

        let folded = ConstantFolder
            .fold_shuffle_vector_dyn(lhs.as_value(), rhs.as_value(), &[0, 0])?
            .expect("scalable zero-mask shuffle constexpr constructed");
        let folded = Constant::try_from(folded)?;
        assert_eq!(folded.ty(), vec_ty.as_type());
        m.add_global("shuf", vec_ty.as_type(), folded)?;
        let text = format!("{m}");
        assert!(
            text.contains("@shuf = global <vscale x 2 x i32> shufflevector (<vscale x 2 x i32> <i32 1, i32 2>, <vscale x 2 x i32> <i32 3, i32 4>, <vscale x 2 x i32> zeroinitializer)"),
            "{text}"
        );
        Ok(())
    })
}

/// llvmkit-specific direct Rust hook coverage for
/// `llvm/lib/IR/Constants.cpp::ConstantExpr::getPointerCast` and
/// `getPointerBitCastOrAddrSpaceCast` lines 2277-2300 plus
/// `llvm/lib/IR/Instructions.cpp::CastInst::castIsValid` lines 3374-3395:
/// same-address-space scalar pointer <-> fixed one-lane pointer-vector casts
/// use `bitcast`, not an invalid-cast diagnostic.
#[test]
fn constant_folder_pointer_cast_helpers_allow_one_lane_pointer_bitcasts() -> Result<(), IrError> {
    Module::with_new("folder-ptr-one-lane-bitcast", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let vec_ptr_ty = m.vector_type(ptr_ty.as_type(), 1, false);
        let g = m.add_global("g", i32_ty.as_type(), i32_ty.const_zero())?;
        let scalar = g.as_global_constant_ptr();

        let to_vec = ConstantFolder
            .create_pointer_bitcast_or_addrspace_cast(scalar, vec_ptr_ty.as_type())?
            .expect("scalar pointer to one-lane pointer vector folds");
        let to_vec_via_pointer_cast = ConstantFolder
            .create_pointer_cast(scalar, vec_ptr_ty.as_type())?
            .expect("pointer cast helper uses bitcast for one-lane vector destination");

        let vector = vec_ptr_ty.const_vector::<Constant<'_>, _>([scalar])?;
        let to_scalar = ConstantFolder
            .create_pointer_bitcast_or_addrspace_cast(vector.as_constant(), ptr_ty.as_type())?
            .expect("one-lane pointer vector to scalar pointer folds");
        let to_scalar_via_pointer_cast = ConstantFolder
            .create_pointer_cast(vector.as_constant(), ptr_ty.as_type())?
            .expect("pointer cast helper uses bitcast for scalar destination");

        m.add_global("to_vec", vec_ptr_ty.as_type(), Constant::try_from(to_vec)?)?;
        m.add_global(
            "to_vec_pc",
            vec_ptr_ty.as_type(),
            Constant::try_from(to_vec_via_pointer_cast)?,
        )?;
        m.add_global(
            "to_scalar",
            ptr_ty.as_type(),
            Constant::try_from(to_scalar)?,
        )?;
        m.add_global(
            "to_scalar_pc",
            ptr_ty.as_type(),
            Constant::try_from(to_scalar_via_pointer_cast)?,
        )?;
        let text = format!("{m}");
        assert!(
            text.contains("@to_vec = global <1 x ptr> bitcast (ptr @g to <1 x ptr>)"),
            "{text}"
        );
        assert!(
            text.contains("@to_vec_pc = global <1 x ptr> bitcast (ptr @g to <1 x ptr>)"),
            "{text}"
        );
        assert!(
            text.contains("@to_scalar = global ptr bitcast (<1 x ptr> <ptr @g> to ptr)"),
            "{text}"
        );
        assert!(
            text.contains("@to_scalar_pc = global ptr bitcast (<1 x ptr> <ptr @g> to ptr)"),
            "{text}"
        );
        Ok(())
    })
}

/// Mirrors `IRBuilder.h::CreateIsNull` -> `CreateICmpEQ` -> `Folder.FoldCmp`
/// (`IRBuilder.h::CreateICmp` line 2442) +
/// `ConstantFold.cpp::ConstantFoldCompareInstruction`: `icmp eq ptr null,
/// null` folds to `i1 true` under the default folder and no instruction is
/// inserted.
#[test]
fn constant_folder_folds_is_null_of_constant_null_without_instruction() -> Result<(), IrError> {
    Module::with_new("folder-is-null", |m| {
        let bool_ty = m.bool_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(bool_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<bool, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
        let null = PointerValue::try_from(ptr_ty.const_null().as_value())?;

        let result = b.build_is_null(null, "isn")?;

        assert_eq!(
            Constant::try_from(result.as_value())?,
            bool_ty.const_int(true).as_constant()
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// Sibling of the `CreateIsNull` fold above for `IRBuilder.h::
/// CreateIsNotNull`: `icmp ne ptr null, null` folds to `i1 false` under the
/// default folder and no instruction is inserted.
#[test]
fn constant_folder_folds_is_not_null_of_constant_null_without_instruction() -> Result<(), IrError> {
    Module::with_new("folder-is-not-null", |m| {
        let bool_ty = m.bool_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(bool_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<bool, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
        let null = PointerValue::try_from(ptr_ty.const_null().as_value())?;

        let result = b.build_is_not_null(null, "ok")?;

        assert_eq!(
            Constant::try_from(result.as_value())?,
            bool_ty.const_int(false).as_constant()
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// Mirrors `ConstantFold.cpp::evaluateICmpRelation`: a non-extern-weak
/// addrspace(0) global compared against null is known `ICMP_UGT`, so the
/// default folder resolves `icmp eq ptr @g, null` to `i1 false` (and `ne`
/// to `i1 true`) without materializing instructions.
#[test]
fn constant_folder_folds_pointer_cmp_global_vs_null_without_instruction() -> Result<(), IrError> {
    Module::with_new("folder-ptr-cmp-global", |m| {
        let bool_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let g = m.add_global("g", i32_ty.as_type(), i32_ty.const_zero())?;
        let gp = PointerValue::try_from(g.as_global_constant_ptr().as_value())?;
        let fn_ty = m.fn_type(bool_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<bool, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);

        let eq = b.build_pointer_cmp(IntPredicate::Eq, gp, ptr_ty.const_null(), "eq")?;
        let ne = b.build_pointer_cmp(IntPredicate::Ne, gp, ptr_ty.const_null(), "ne")?;

        assert_eq!(
            Constant::try_from(eq.as_value())?,
            bool_ty.const_int(false).as_constant()
        );
        assert_eq!(
            Constant::try_from(ne.as_value())?,
            bool_ty.const_int(true).as_constant()
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, InsertExtractElement)`
/// lines 1127-1138: a folded insertelement chain extracts the inserted
/// constants without materializing instructions.
#[test]
fn default_builder_folds_insert_extract_element_chain() -> Result<(), IrError> {
    Module::with_new("folder-insert-extract-element", |m| {
        let i64_ty = m.i64_type();
        let vec_ty = m.vector_type(i64_ty.as_type(), 4, false);
        let fn_ty = m.fn_type(i64_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i64, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
        let elt1 = i64_ty.const_int(-1_i64);
        let elt2 = i64_ty.const_int(-2_i64);

        let vec = b.build_insert_element::<_, _, i8, _, _>(
            vec_ty.as_type().get_poison(),
            elt1,
            m.i8_type().const_int(1_i8),
            "v1",
        )?;
        let vec = b.build_insert_element::<_, _, i32, _, _>(
            vec,
            elt2,
            m.i32_type().const_int(2_i32),
            "v2",
        )?;
        let x1 = b.build_extract_element::<_, i8, _, _>(vec, m.i8_type().const_int(1_i8), "x1")?;
        let x2 =
            b.build_extract_element::<_, i32, _, _>(vec, m.i32_type().const_int(2_i32), "x2")?;

        assert_eq!(Constant::try_from(x1)?, elt1.as_constant());
        assert_eq!(Constant::try_from(x2)?, elt2.as_constant());
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/include/llvm/IR/IRBuilder.h::CreateMul`:
/// custom folders receive `FoldNoWrapBinOp(Instruction::Mul, ...)` instead of
/// the plain binary hook.
#[test]
fn custom_folder_no_wrap_hook_receives_mul() -> Result<(), IrError> {
    Module::with_new("folder-nowrap-hook-mul", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let folded = i32_ty.const_int(99_i32).as_value();
        let b = IRBuilder::with_folder(
            &m,
            ReturningFolder {
                result: FolderReturn::NoWrapCall {
                    opcode: BinaryOpcode::Mul,
                    has_nuw: true,
                    has_nsw: false,
                    value: folded,
                },
            },
        )
        .position_at_end(entry);

        let result = b.build_int_mul_with_flags::<i32, _, _, _>(
            i32_ty.const_int(6_i32),
            i32_ty.const_int(7_i32),
            MulFlags::new().nuw(),
            "mul",
        )?;

        assert_eq!(result.as_value(), folded);
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// llvmkit-specific subset of `llvm/include/llvm/IR/IRBuilder.h::CreateShl`:
/// custom folders receive `FoldNoWrapBinOp(Instruction::Shl, ...)` instead of
/// the plain binary hook.
#[test]
fn custom_folder_no_wrap_hook_receives_shl() -> Result<(), IrError> {
    Module::with_new("folder-nowrap-hook-shl", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let folded = i32_ty.const_int(123_i32).as_value();
        let b = IRBuilder::with_folder(
            &m,
            ReturningFolder {
                result: FolderReturn::NoWrapCall {
                    opcode: BinaryOpcode::Shl,
                    has_nuw: true,
                    has_nsw: true,
                    value: folded,
                },
            },
        )
        .position_at_end(entry);

        let result = b.build_int_shl_with_flags::<i32, _, _, _>(
            i32_ty.const_int(1_i32),
            i32_ty.const_int(3_i32),
            ShlFlags::new().nuw().nsw(),
            "shl",
        )?;

        assert_eq!(result.as_value(), folded);
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`:
/// `NoFolder` materializes the instruction and preserves the requested name.
#[test]
fn no_folder_names_add_instruction_exactly() -> Result<(), IrError> {
    Module::with_new("nofolder-add", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let add = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
            "add",
        )?;

        let name = add.as_value().name();
        assert_eq!(name.as_deref(), Some("add"));
        assert!(InstructionView::try_from(add.as_value()).is_ok());
        assert_eq!(b.insert_block().instructions().len(), 1);
        Ok(())
    })
}

/// llvmkit-specific subset of `NoFolder.h`: with `NoFolder`, even all-constant
/// invalid `udiv` operands materialize a real instruction instead of poison.
#[test]
fn no_folder_emits_udiv_instruction_for_constants() -> Result<(), IrError> {
    Module::with_new("nofolder-udiv", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let lhs = i32_ty.const_int(42_i32);
        let rhs = i32_ty.const_zero();

        let result = b.build_int_udiv::<i32, _, _, _>(lhs, rhs, "q")?;
        let instruction = InstructionView::try_from(result.as_value())?;
        let Some(InstructionKind::UDiv(udiv)) = instruction.kind() else {
            panic!("expected udiv instruction");
        };

        assert_eq!(udiv.lhs(), lhs.as_value());
        assert_eq!(udiv.rhs(), rhs.as_value());
        assert!(!udiv.is_exact());
        assert_eq!(result.as_value().name().as_deref(), Some("q"));
        assert_eq!(b.insert_block().instructions().len(), 1);
        Ok(())
    })
}

/// llvmkit-specific subset of `IRBuilder.h::CreatePtrToAddr`: the builder
/// chooses the pointer address type from `DataLayout` and emits a distinct
/// `ptrtoaddr` cast, not `ptrtoint`.
#[test]
fn no_folder_emits_ptrtoaddr_instruction_with_address_type() -> Result<(), IrError> {
    Module::with_new("nofolder-ptrtoaddr", |m| {
        m.set_data_layout("p1:64:64:64:32")?;
        let i32_ty = m.i32_type();
        let ptr1_ty = m.ptr_type(1);
        let fn_ty = m.fn_type(i32_ty, [ptr1_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let ptr: PointerValue = f.param(0)?.try_into()?;

        let result = b.build_ptr_to_addr(ptr, "addr")?;
        let instruction = InstructionView::try_from(result.as_value())?;
        // Match the exact cast opcode through the nested `CastKind`; the
        // `PtrToAddr` handle exposes a statically pointer-typed `src()`.
        let Some(InstructionKind::Cast(CastKind::PtrToAddr(cast))) = instruction.kind() else {
            panic!("expected ptrtoaddr cast instruction");
        };

        assert_eq!(cast.opcode(), CastOpcode::PtrToAddr);
        let src: PointerValue = cast.src();
        assert_eq!(src.as_value(), ptr.as_value());
        let typed_result: IntValue<IntDyn> = result;
        assert_eq!(typed_result.ty().bit_width(), 32);
        assert_eq!(typed_result.as_value().name().as_deref(), Some("addr"));
        assert_eq!(b.insert_block().instructions().len(), 1);
        Ok(())
    })
}

/// llvmkit-specific subset of `NoFolder.h`: with `NoFolder`, an all-constant
/// pointer compare still materializes a real `icmp` instruction.
#[test]
fn no_folder_emits_pointer_cmp_instruction_for_constant_nulls() -> Result<(), IrError> {
    Module::with_new("nofolder-ptr-cmp", |m| {
        let bool_ty = m.bool_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(bool_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<bool, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let null = ptr_ty.const_null();

        let result = b.build_pointer_cmp(IntPredicate::Eq, null, null, "isn")?;
        let instruction = InstructionView::try_from(result.as_value())?;
        let Some(InstructionKind::ICmp(icmp)) = instruction.kind() else {
            panic!("expected icmp instruction");
        };

        assert_eq!(icmp.predicate(), IntPredicate::Eq);
        assert_eq!(icmp.lhs(), null.as_value());
        assert_eq!(icmp.rhs(), null.as_value());
        assert_eq!(result.as_value().name().as_deref(), Some("isn"));
        assert_eq!(b.insert_block().instructions().len(), 1);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolder.h`: the default folder folds
/// constants only; it does not invent InstSimplify-style `x + 0` for nonconstants.
#[test]
fn constant_folder_does_not_simplify_nonconstant_add_zero() -> Result<(), IrError> {
    Module::with_new("folder-add-zero", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x = f.param(0)?;

        let result = b.build_int_add::<i32, _, _, _>(x, i32_ty.const_zero(), "sum")?;

        assert!(InstructionView::try_from(result.as_value()).is_ok());
        assert_eq!(b.insert_block().instructions().len(), 1);
        Ok(())
    })
}

/// `llvmkit-specific subset` of `IRBuilderFolder.h`: custom folders may return
/// an existing value, but the builder rejects a value with the wrong type.
///
/// `ReturningFolder` overrides only the erased `fold_bin_op_dyn`, so
/// `build_int_add::<i32>` reaches it through the typed hook's *default* body
/// and the wrong-width result is caught by `folder::narrow_folded_int`'s
/// re-narrow. Both sides are integers, so that seam reports the two widths
/// rather than a `TypeMismatch { expected: Integer, got: Integer }` that could
/// not say which width was wrong (`Type::require_match`).
#[test]
fn custom_folder_wrong_type_is_rejected() -> Result<(), IrError> {
    Module::with_new("folder-wrong-type", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let folded = i64_ty.const_int(0_i64).as_value();
        let b = IRBuilder::with_folder(
            &m,
            ReturningFolder {
                result: FolderReturn::Value(folded),
            },
        )
        .position_at_end(entry);

        let err = b
            .build_int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "sum")
            .expect_err("wrong-type folded value is rejected");

        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// Typed-vs-dyn parity: `build_int_add::<i32>` (typed hook path,
/// `fold_int_bin_op`) and `build_int_add_dyn` (erased path, `fold_bin_op_dyn`)
/// must fold `add i32 7, 9` to the identical constant and printed module
/// under `ConstantFolder`. Closest upstream anchor: the constant-folding
/// rows of `unittests/IR/ConstantsTest.cpp` (`TEST(ConstantsTest, FoldFunctionCall)`
/// and neighboring `TEST(ConstantsTest, ...)` folds) that assert the folder
/// produces the same `ConstantInt` regardless of the call shape used to
/// reach it.
#[test]
fn typed_and_dyn_int_add_fold_to_identical_constant() -> Result<(), IrError> {
    let typed_text = Module::with_new("folder-typed-add", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);

        let result = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(7_i32),
            i32_ty.const_int(9_i32),
            "sum",
        )?;

        assert_eq!(
            Constant::try_from(result.as_value())?,
            i32_ty.const_int(16_i32).as_constant()
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok::<_, IrError>(format!("{m}"))
    })?;

    let dyn_text = Module::with_new("folder-typed-add", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);

        let result = b.build_int_add_dyn(
            i32_ty.const_int(7_i32).as_value(),
            i32_ty.const_int(9_i32).as_value(),
            "sum",
        )?;

        assert_eq!(
            Constant::try_from(result)?,
            i32_ty.const_int(16_i32).as_constant()
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok::<_, IrError>(format!("{m}"))
    })?;

    assert_eq!(typed_text, dyn_text);
    Ok(())
}

/// A folder whose `fold_bin_op_dyn` override returns a wrong-width value for
/// `IntDyn` operands must yield `IrError::TypeMismatch` through the typed
/// `build_int_add::<IntDyn, _, _, _>` path. This locks the
/// `narrow_folded_int` default-delegation seam: `WideningDynFolder` overrides
/// only the erased `fold_bin_op_dyn` hook, so `build_int_add`'s call to
/// `fold_int_bin_op` runs the trait's *default* body (`folder.rs`'s
/// `fold_int_bin_op<W>` -- there is no native override here), which forwards
/// to `fold_bin_op_dyn` and then re-narrows the erased result by `TypeId`
/// through `narrow_folded_int`. That re-narrow call is where the wrong-width
/// 64-bit replacement is rejected; the builder's own `accept_folded_int`
/// type check (`ir_builder.rs`) is never reached on this path, because
/// `fold_int_bin_op` already returns `Err(TypeMismatch)` before
/// `build_int_add` gets to call it.
///
/// `accept_folded_int` is only reachable behind a
/// *native* override of a typed hook (`fold_int_bin_op<W>` or one of its
/// siblings) that itself returns `Some(IntValue<'ctx, W, B>)` without going
/// through `narrow_folded_int` -- which `WideningDynFolder`, overriding only
/// the erased hook, deliberately is not.
///
/// Such an override *can* now be written from an external crate -- and is,
/// immediately below: [`NarrowingTypedFolder`] is that override, and
/// [`external_narrow_override_wrong_width_rejected_by_accept_folded_int`]
/// drives it to the acceptor. This paragraph is therefore executable rather
/// than aspirational; the file failing to compile would falsify it. Slice
/// 1.1's `IntWidth::narrow` is a safe, public construction of
/// `IntValue<'ctx, W, B>` from an erased `Value`, generic over arbitrary `W`
/// (`tests/generic_narrowing.rs` exercises it from this same external-test
/// position), so `Ok(Some(W::narrow(v)?))` typechecks inside a generic
/// `fold_int_bin_op<W>` override. That does not let an external folder forge
/// a width -- `narrow` checks the payload's real type against `W` -- but at
/// `W = IntDyn` the marker proves only "some integer", so such a folder
/// *can* reach `accept_folded_int` with a payload whose width contradicts
/// the operands'. Nothing breaks, and that reachability is exactly why
/// Slice 1.2 made `accept_folded_int` check *unconditionally* instead of
/// only for the erased markers: the acceptor re-checks the fold result's
/// runtime type against the operand's for every marker, so the wrong-width
/// payload is rejected there. The unconditional acceptor is what makes the
/// now-writable override safe.
///
/// What stays closed is forging a *static* `W` from outside: every
/// `TryFrom<Value>` / `IntoIntValue` impl is per concrete marker, and the
/// crate-internal `IntValue::from_value_unchecked` escape hatch -- the one
/// unchecked mint, which `ConstantFolder`'s typed hooks no longer use (all
/// nine route through `W::narrow` / `K::narrow` as of Slice 1.1) -- is
/// `pub(crate)`, unreachable from this external test crate. Confirmed by
/// the sibling compile-fail golden
/// `tests/compile_fail/folder_typed_wrong_width.rs`, which locks exactly
/// this shape (`Ok(Some(concrete_width_value))` inside a generic
/// `fold_int_bin_op<W>` override) as a compiler error
/// (`E0308: mismatched types`), and independently reproduced with a
/// `TryFrom`-based construction attempt (same wall, different error:
/// `IntValue<'ctx, W, B>: TryFrom<Value<'ctx, B>>` is not implemented for
/// generic `W`, and adding it as an extra `where` bound on the impl is
/// itself rejected -- an impl may not add bounds beyond what the trait
/// declares for a generic method). `IntWidth::narrow` does not breach that
/// wall: it is the same per-marker `TryFrom`, reached through a method the
/// `IntWidth` bound already carries, so the checked narrow rides in on a
/// bound the trait declares rather than one the impl adds -- what is new is
/// the bound, not the capability.
///
/// The wrong-width replacement value is built once in the test (where the
/// owning `Module` is available to mint a 64-bit `IntDyn` constant) and
/// carried inside the folder, so the trait-required `fold_bin_op_dyn`
/// override -- generic over any `Value<'ctx, B>` operand -- can answer with
/// it unconditionally.
#[derive(Debug, Clone, Copy)]
struct WideningDynFolder<'ctx, B: llvmkit_ir::ModuleBrand> {
    replacement: Value<'ctx, B>,
}

impl<'ctx, B: llvmkit_ir::ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B>
    for WideningDynFolder<'ctx, B>
{
    fn fold_bin_op_dyn(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        // Deliberately answers with a 64-bit constant zero regardless of the
        // (32-bit) operand width -- this IS the erased hook (it has no
        // default body to bypass), so overriding only this hook routes the
        // typed `build_int_add::<IntDyn, ...>` call through
        // `fold_int_bin_op`'s *default* body, which re-narrows this erased
        // result via `narrow_folded_int`'s TypeId check. That is the seam
        // this test exercises -- not the builder's separate
        // `accept_folded_int` type check, which only runs behind a typed
        // hook's *native* override (see the struct doc comment above for
        // why no such override is reachable from this external crate).
        Ok(Some(self.replacement))
    }
}

/// `llvm/include/llvm/IR/IRBuilderFolder.h` `Value*` folder hook contract:
/// locks the `IntValue<IntDyn>` builder-side `TypeId` re-check the
/// typed-folder rewrite (task 5) preserves for erased markers -- an erased
/// `fold_bin_op_dyn` override that answers with a wrong-width replacement
/// must still be caught by `narrow_folded_int`'s runtime check rather than
/// silently accepted.
///
/// Also the guard on `narrow_folded_int`'s *shape*: this is the case that
/// makes it irreplaceable by `W::narrow`. Here `W = IntDyn`, and
/// `IntDyn::narrow` accepts ANY integer width (the marker names none), so
/// rewriting the seam to narrow-to-the-marker would delete exactly this
/// check and let the 64-bit replacement through for 32-bit operands.
/// `narrow_folded_int` compares against `lhs`'s *runtime* type instead --
/// see `Type::require_match`.
#[test]
fn dyn_marker_fold_keeps_runtime_width_check() -> Result<(), IrError> {
    Module::with_new("folder-dyn-widen", |m| {
        let i32_dyn_ty = m.custom_width_int_type(32)?;
        let i64_dyn_ty = m.custom_width_int_type(64)?;
        let fn_ty = m.fn_type(m.i32_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let folder = WideningDynFolder {
            replacement: i64_dyn_ty.const_zero().as_value(),
        };
        let b = IRBuilder::with_folder(&m, folder).position_at_end(entry);

        let lhs = i32_dyn_ty.const_int_checked(1_i32)?;
        let rhs = i32_dyn_ty.const_int_checked(2_i32)?;

        let err = b
            .build_int_add::<IntDyn, _, _, _>(lhs, rhs, "sum")
            .expect_err("64-bit fold result for 32-bit IntDyn operands is rejected");

        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}

/// The executable proof of the reachability claim in [`WideningDynFolder`]'s
/// doc above: a folder that reaches the builder's `accept_folded_int` from
/// *outside* the crate, through a **native** `fold_int_bin_op<W>` override
/// whose body constructs its `IntValue<'ctx, W, B>` return with Slice 1.1's
/// public [`IntWidth::narrow`].
///
/// **This file compiling at all is the proof.** This is an integration test:
/// it lives outside `llvmkit-ir` and therefore has exactly a downstream
/// user's visibility. `IntValue::from_value_unchecked` -- the unchecked mint
/// the in-crate `HostileTypedFolder` (`src/ir_builder.rs::tests`) uses to
/// reach the same acceptor -- is `pub(crate)` and simply does not exist from
/// here, so no such shortcut is available and none is used. `W::narrow` is a
/// safe, checked, public construction callable behind the bare `W: IntWidth`
/// bound the trait method already declares, which is what makes
/// `Ok(Some(W::narrow(v)?))` typecheck where the equivalent `TryFrom` would
/// demand a `where` clause the trait's signature forbids adding.
///
/// Narrowing is *not* a width forgery, and the two markers show the split:
///
/// - At a **static** `W` (`i32`, `Width<7>`, ...) `narrow` checks the
///   payload's real IR type against `W` and this folder's 64-bit
///   `replacement` is rejected inside the override itself -- the builder is
///   never even asked to accept it. The static marker is self-guaranteeing
///   *on this route* precisely because `narrow` is checked.
/// - At `W = `[`IntDyn`] the marker names no width, so `narrow` proves only
///   "some integer" and *succeeds* on the 64-bit payload. The override then
///   hands `build_int_add` an `IntValue<'ctx, IntDyn, B>` whose runtime
///   width contradicts the 32-bit operands', and the builder's
///   `accept_folded_int` is the only thing left between that payload and the
///   IR.
///
/// That second bullet is why Slice 1.2 made `accept_folded_int` check
/// **unconditionally** rather than only for erased markers: the acceptor
/// compares the fold result's runtime type against the operand's for every
/// marker, so it catches what `narrow` structurally cannot at a dyn marker.
/// [`external_narrow_override_wrong_width_rejected_by_accept_folded_int`]
/// drives exactly that case.
///
/// Contrast [`WideningDynFolder`] above, which overrides only the *erased*
/// `fold_bin_op_dyn` hook and is therefore caught one seam earlier, by
/// `narrow_folded_int` inside `fold_int_bin_op`'s default body. A native
/// override replaces that default body outright, so `narrow_folded_int`
/// never runs here.
///
/// What stays closed is forging a *static* `W`: the sibling compile-fail
/// golden `tests/compile_fail/folder_typed_wrong_width.rs` locks
/// `Ok(Some(concrete_width_value))` inside a generic `fold_int_bin_op<W>`
/// override as `E0308: mismatched types`. `narrow` does not breach that
/// wall -- it returns the caller's `W`, never a concrete one.
#[derive(Debug, Clone, Copy)]
struct NarrowingTypedFolder<'ctx, B: llvmkit_ir::ModuleBrand> {
    /// An erased 64-bit constant, deliberately the wrong width for the
    /// 32-bit operands the test folds. Erased (`Value`, not
    /// `IntValue<'ctx, i64, B>`) because `narrow`'s input is an erased
    /// `Value` -- that erasure is the whole point: this is the shape a real
    /// external folder would hold after computing a replacement dynamically.
    replacement: Value<'ctx, B>,
}

impl<'ctx, B: llvmkit_ir::ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B>
    for NarrowingTypedFolder<'ctx, B>
{
    fn fold_int_bin_op<W: IntWidth>(
        &self,
        _opcode: BinaryOpcode,
        _lhs: IntValue<'ctx, W, B>,
        _rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        // The line the `WideningDynFolder` doc claims an external crate can
        // now write, written from an external crate. No `unsafe`, no
        // `pub(crate)` escape hatch, no extra `where` bound on the impl --
        // just the `W: IntWidth` bound the trait method itself declares.
        //
        // At `W = IntDyn` this narrow SUCCEEDS on a 64-bit payload (the
        // marker names no width to contradict), so a wrong-width
        // `IntValue<'ctx, IntDyn, B>` flows out of this native override and
        // straight into `accept_folded_int` -- `narrow_folded_int` is part
        // of the default body this override replaced and never runs.
        Ok(Some(W::narrow(self.replacement)?))
    }
}

/// `llvm/include/llvm/IR/IRBuilderFolder.h` typed fold hook contract, from
/// the external-folder position: locks `accept_folded_int`
/// (`src/ir_builder.rs`) as the seam that rejects a wrong-width result from
/// an **external** native `fold_int_bin_op<W>` override built with the
/// public [`IntWidth::narrow`] -- the route Slice 1.1 opened and the
/// in-crate `from_value_unchecked` siblings in
/// `src/ir_builder.rs::tests::hostile_native_typed_override_*` cannot reach
/// from a downstream user's visibility.
///
/// Trace confirming *this* line rejects, not `narrow_folded_int`:
/// `build_int_add::<IntDyn, _, _, _>` calls
/// `self.folder.fold_int_bin_op(BinaryOpcode::Add, lhs, rhs)`.
/// [`NarrowingTypedFolder`]'s override is *native*, so it runs directly and
/// the trait's default body -- the only caller of `fold_bin_op_dyn` and
/// `narrow_folded_int` -- is bypassed entirely. Inside the override,
/// `IntDyn::narrow(replacement)` succeeds: the payload is an integer, and
/// `IntDyn` asserts nothing about width. The resulting
/// `IntValue<'ctx, IntDyn, B>` (real IR type: 64-bit) returns to
/// `build_int_add`, which forwards it to `self.accept_folded_int(folded, lhs)`.
/// There, `folded.as_value().ty().id() != like.as_value().ty().id()` is
/// `true` -- 64-bit payload against `lhs`'s 32-bit custom-width type -- so
/// the acceptor returns `Err(IrError::OperandWidthMismatch { lhs: 32, rhs: 64 })`
/// and no instruction is inserted.
///
/// This is the case that makes the acceptor's unconditionality load-bearing
/// for *external* code, not just for in-crate folder bugs: `narrow` is a
/// checked construction, but at a dyn marker there is nothing for it to
/// check the width against. Deleting the acceptor's check -- or re-keying it
/// on the folder's own claim about `W`, as the pre-`bf57e17` shape did --
/// would let this 64-bit payload reach the IR as a 32-bit add's result.
#[test]
fn external_narrow_override_wrong_width_rejected_by_accept_folded_int() -> Result<(), IrError> {
    Module::with_new("external-narrow-folder", |m| {
        let i32_dyn_ty = m.custom_width_int_type(32)?;
        let i64_dyn_ty = m.custom_width_int_type(64)?;
        let fn_ty = m.fn_type(m.i32_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let folder = NarrowingTypedFolder {
            replacement: i64_dyn_ty.const_zero().as_value(),
        };
        let b = IRBuilder::with_folder(&m, folder).position_at_end(entry);

        let lhs = i32_dyn_ty.const_int_checked(1_i32)?;
        let rhs = i32_dyn_ty.const_int_checked(2_i32)?;

        let err = b
            .build_int_add::<IntDyn, _, _, _>(lhs, rhs, "sum")
            .expect_err("64-bit narrow-built fold result for 32-bit IntDyn operands is rejected");

        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    })
}
