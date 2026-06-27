//! IRBuilder folder strategy tests.
//!
//! Source-derived folder behavior from `llvm/include/llvm/IR/ConstantFolder.h`,
//! `llvm/include/llvm/IR/IRBuilder.h`, and `llvm/include/llvm/IR/NoFolder.h`,
//! plus exact `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`.

use llvmkit_ir::instr_types::CastOpcode;
use llvmkit_ir::{
    BinaryOpcode, CmpPredicate, Constant, ConstantFloatValue, ConstantFolder, FastMathFlags,
    GepNoWrapFlags, IRBuilder, IRBuilderFolder, Instruction, InstructionKind, IntDyn, IntValue,
    IntrinsicId, IrError, IrResult, Linkage, Module, MulFlags, NoFolder, PointerValue, ShlFlags,
    Type, UDivFlags, UnaryOpcode, Value, constant_fold_binary_instruction,
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

impl<'ctx> IRBuilderFolder<'ctx> for ReturningFolder<'ctx> {
    fn fold_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_exact_bin_op(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _is_exact: bool,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_no_wrap_bin_op(
        &self,
        opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        has_nuw: bool,
        has_nsw: bool,
    ) -> IrResult<Option<Value<'ctx>>> {
        match self.result {
            FolderReturn::NoWrapCall {
                opcode: expected_opcode,
                has_nuw: expected_nuw,
                has_nsw: expected_nsw,
                value,
            } if opcode == expected_opcode
                && has_nuw == expected_nuw
                && has_nsw == expected_nsw =>
            {
                Ok(Some(value))
            }
            _ => self.fold(),
        }
    }

    fn fold_bin_op_fmf(
        &self,
        _opcode: BinaryOpcode,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_un_op_fmf(
        &self,
        _opcode: UnaryOpcode,
        _value: Value<'ctx>,
        _fmf: FastMathFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_cmp(
        &self,
        _predicate: CmpPredicate,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_gep(
        &self,
        _source_ty: Type<'ctx>,
        _ptr: Value<'ctx>,
        _indices: &[Value<'ctx>],
        _no_wrap: GepNoWrapFlags,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_select(
        &self,
        _cond: Value<'ctx>,
        _true_value: Value<'ctx>,
        _false_value: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_extract_value(
        &self,
        _aggregate: Value<'ctx>,
        _indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_insert_value(
        &self,
        _aggregate: Value<'ctx>,
        _value: Value<'ctx>,
        _indices: &[u32],
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_extract_element(
        &self,
        _vector: Value<'ctx>,
        _index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_insert_element(
        &self,
        _vector: Value<'ctx>,
        _new_element: Value<'ctx>,
        _index: Value<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_shuffle_vector(
        &self,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _mask: &[i32],
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_cast(
        &self,
        _opcode: CastOpcode,
        _value: Value<'ctx>,
        _dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn fold_binary_intrinsic(
        &self,
        _id: IntrinsicId,
        _lhs: Value<'ctx>,
        _rhs: Value<'ctx>,
        _ty: Type<'ctx>,
        _fmf_source: Option<&Instruction<'ctx>>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn create_pointer_cast(
        &self,
        _value: Constant<'ctx>,
        _dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
    }

    fn create_pointer_bitcast_or_addrspace_cast(
        &self,
        _value: Constant<'ctx>,
        _dest_ty: Type<'ctx>,
    ) -> IrResult<Option<Value<'ctx>>> {
        self.fold()
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
        assert_eq!(entry.instructions().len(), 0);
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
        assert_eq!(entry.instructions().len(), 0);
        Ok(())
    })
}

/// llvmkit-specific subset of `ConstantFolder.h::FoldExactBinOp`: exact-capable
/// all-constant integer ops fold poison when exactness would be violated.
#[test]
fn constant_folder_exact_udiv_inexact_constants_fold_to_poison() -> Result<(), IrError> {
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
            i32_ty.as_type().get_poison().as_constant()
        );
        assert_eq!(entry.instructions().len(), 0);
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
            .fold_no_wrap_bin_op(
                BinaryOpcode::Mul,
                lhs.as_value(),
                rhs.as_value(),
                true,
                false,
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
            .fold_no_wrap_bin_op(
                BinaryOpcode::Shl,
                lhs.as_value(),
                rhs.as_value(),
                true,
                true,
            )?
            .expect("all-constant no-wrap shl folds");

        assert_eq!(Constant::try_from(folded)?, expected);
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
        assert_eq!(entry.instructions().len(), 0);
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
        assert_eq!(entry.instructions().len(), 0);
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
        assert!(Instruction::try_from(add.as_value()).is_ok());
        assert_eq!(entry.instructions().len(), 1);
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
        let instruction = Instruction::try_from(result.as_value())?;
        let Some(InstructionKind::UDiv(udiv)) = instruction.kind() else {
            panic!("expected udiv instruction");
        };

        assert_eq!(udiv.lhs(), lhs.as_value());
        assert_eq!(udiv.rhs(), rhs.as_value());
        assert!(!udiv.is_exact());
        assert_eq!(result.as_value().name().as_deref(), Some("q"));
        assert_eq!(entry.instructions().len(), 1);
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
        let instruction = Instruction::try_from(result.as_value())?;
        let Some(InstructionKind::Cast(cast)) = instruction.kind() else {
            panic!("expected ptrtoaddr cast instruction");
        };

        assert_eq!(cast.opcode(), CastOpcode::PtrToAddr);
        assert_eq!(cast.src(), ptr.as_value());
        let typed_result: IntValue<IntDyn> = result;
        assert_eq!(typed_result.ty().bit_width(), 32);
        assert_eq!(typed_result.as_value().name().as_deref(), Some("addr"));
        assert_eq!(entry.instructions().len(), 1);
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

        assert!(Instruction::try_from(result.as_value()).is_ok());
        assert_eq!(entry.instructions().len(), 1);
        Ok(())
    })
}

/// `llvmkit-specific subset` of `IRBuilderFolder.h`: custom folders may return
/// an existing value, but the builder rejects a value with the wrong type.
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

        assert!(matches!(err, IrError::TypeMismatch { .. }));
        assert_eq!(entry.instructions().len(), 0);
        Ok(())
    })
}
