//! Each [`BuilderError`] variant is what the builder entry that raises it
//! returns: one test per entry family, each provoking every refusal it has.
//!
//! **No upstream counterpart.** Upstream's `IRBuilder` and instruction
//! constructors `assert` these conditions instead of returning an error, so
//! `unittests/IR/IRBuilderTest.cpp` cannot observe them. The conditions
//! themselves are upstream's; what these tests pin is llvmkit's typed error
//! surface: that each refusal names its operation (an `icmp` refusal is not
//! reported as an `fcmp` one) rather than carrying a message string.
//!
//! `BuilderError::SplatLaneCountTooWide` has no test: a `NonZeroU32` lane
//! count always fits a 32- or 64-bit `usize`, so it is reachable only on a
//! 16-bit target. Every other variant is asserted here or in a sibling test,
//! which `tests/typed_error_coverage.rs` checks against the enum.

use llvmkit_ir::{
    AggregateInstruction, ArithmeticFamily, BinaryOpcode, BlockError, BuilderError, Dyn,
    FastMathFlags, FloatPredicate, IcmpFlags, IntBinOpFlags, IntPredicate, IrBuilder, IrError,
    Linkage, OperandCheck, ShuffleMaskElem, lanes, module_new,
};

/// The refusal an `Err` carries, or a panic naming the entry that succeeded.
fn refusal<T>(result: Result<T, IrError>, entry: &str) -> IrError {
    match result {
        Ok(_) => panic!("{entry} must refuse"),
        Err(error) => error,
    }
}

/// The integer binary-operator and `icmp` entries name their operation in
/// each operand refusal, and the integer entry refuses a floating-point
/// opcode as one of the other family.
#[test]
fn integer_operand_refusals_name_their_operation() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-int")?;
    let mut key = module.layout_token();
    let m = key.module();
    let fn_ty = m.function_type(
        m.void_type(),
        [
            m.i32_type().as_type(),
            m.i64_type().as_type(),
            m.f32_type().as_type(),
        ],
    )?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&mut key).position_at_end(entry);
    let i32_arg = m.view(f).param(0)?.as_erased();
    let i64_arg = m.view(f).param(1)?.as_erased();
    let float_arg = m.view(f).param(2)?.as_erased();
    let flags = IntBinOpFlags::new();

    let err = refusal(
        b.int_binop_erased(BinaryOpcode::Add, i32_arg, i64_arg, flags, "a"),
        "add of i32 and i64",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandTypesDiffer {
            operation: OperandCheck::IntegerBinaryOperator,
        })
    );
    let err = refusal(
        b.int_binop_erased(BinaryOpcode::Add, float_arg, float_arg, flags, "a"),
        "add of floats",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandType {
            operation: OperandCheck::IntegerBinaryOperator,
        })
    );
    let err = refusal(
        b.int_binop_erased(BinaryOpcode::Fadd, i32_arg, i32_arg, flags, "a"),
        "int_binop_erased of fadd",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::NotABinaryOpcode {
            family: ArithmeticFamily::Integer,
        })
    );

    let err = refusal(
        b.int_cmp_erased(IntPredicate::Eq, i32_arg, i64_arg, IcmpFlags::new(), "c"),
        "icmp of i32 and i64",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandTypesDiffer {
            operation: OperandCheck::IntegerCompare,
        })
    );
    let err = refusal(
        b.int_cmp_erased(
            IntPredicate::Eq,
            float_arg,
            float_arg,
            IcmpFlags::new(),
            "c",
        ),
        "icmp of floats",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandType {
            operation: OperandCheck::IntegerCompare,
        })
    );
    Ok(())
}

/// The floating-point binary-operator, `fcmp` and `fneg` entries name their
/// operation in each operand refusal, and the floating-point entry refuses an
/// integer opcode as one of the other family.
#[test]
fn floating_point_operand_refusals_name_their_operation() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-fp")?;
    let mut key = module.layout_token();
    let m = key.module();
    let fn_ty = m.function_type(
        m.void_type(),
        [
            m.f32_type().as_type(),
            m.f64_type().as_type(),
            m.i32_type().as_type(),
        ],
    )?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&mut key).position_at_end(entry);
    let float_arg = m.view(f).param(0)?.as_erased();
    let double_arg = m.view(f).param(1)?.as_erased();
    let i32_arg = m.view(f).param(2)?.as_erased();
    let fmf = FastMathFlags::empty();

    let err = refusal(
        b.fp_binop_erased(BinaryOpcode::Add, float_arg, float_arg, fmf, "a"),
        "fp_binop_erased of add",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::NotABinaryOpcode {
            family: ArithmeticFamily::FloatingPoint,
        })
    );
    let err = refusal(
        b.fp_binop_erased(BinaryOpcode::Fadd, float_arg, double_arg, fmf, "a"),
        "fadd of float and double",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandTypesDiffer {
            operation: OperandCheck::FloatingPointBinaryOperator,
        })
    );
    let err = refusal(
        b.fp_binop_erased(BinaryOpcode::Fadd, i32_arg, i32_arg, fmf, "a"),
        "fadd of i32",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandType {
            operation: OperandCheck::FloatingPointBinaryOperator,
        })
    );

    let err = refusal(
        b.fp_cmp_erased(FloatPredicate::Oeq, float_arg, double_arg, fmf, "c"),
        "fcmp of float and double",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandTypesDiffer {
            operation: OperandCheck::FloatingPointCompare,
        })
    );
    let err = refusal(
        b.fp_cmp_erased(FloatPredicate::Oeq, i32_arg, i32_arg, fmf, "c"),
        "fcmp of i32",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::OperandType {
            operation: OperandCheck::FloatingPointCompare,
        })
    );

    let err = refusal(b.fp_neg_erased(i32_arg, fmf, "n"), "fneg of i32");
    assert_eq!(err, IrError::from(BuilderError::FnegOperandType));
    Ok(())
}

/// `select_erased` refuses each malformed condition and arm shape with its
/// own variant, and fast-math flags on a non-floating-point result.
#[test]
fn select_refusals_name_the_broken_rule() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-select")?;
    let mut key = module.layout_token();
    let m = key.module();
    let bool_vector = m.vector_type(m.i1_type(), lanes::<2>())?;
    let int_vector = m.vector_type(m.i32_type(), lanes::<4>())?;
    let fn_ty = m.function_type(
        m.void_type(),
        [
            m.i1_type().as_type(),
            m.i32_type().as_type(),
            bool_vector.as_type(),
            int_vector.as_type(),
        ],
    )?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&mut key).position_at_end(entry);
    let flag = m.view(f).param(0)?.as_erased();
    let i32_arg = m.view(f).param(1)?.as_erased();
    let bool_vector_arg = m.view(f).param(2)?.as_erased();
    let int_vector_arg = m.view(f).param(3)?.as_erased();

    let Some(token_none) = m.token_type().as_type().null_value() else {
        panic!("`token` has the null value `token none`");
    };
    let err = refusal(
        b.select_erased(flag, token_none, token_none, "s"),
        "select of token values",
    );
    assert_eq!(err, IrError::from(BuilderError::SelectOfToken));
    let err = refusal(
        b.select_erased(i32_arg, i32_arg, i32_arg, "s"),
        "select on an i32 condition",
    );
    assert_eq!(err, IrError::from(BuilderError::SelectConditionNotI1));
    let err = refusal(
        b.select_erased(bool_vector_arg, i32_arg, i32_arg, "s"),
        "vector select of scalar arms",
    );
    assert_eq!(err, IrError::from(BuilderError::VectorSelectLengthMismatch));
    let err = refusal(
        b.select_erased(int_vector_arg, int_vector_arg, int_vector_arg, "s"),
        "vector select on a <4 x i32> condition",
    );
    assert_eq!(err, IrError::from(BuilderError::VectorSelectConditionNotI1));
    let err = refusal(
        b.select_erased_with_fmf(flag, i32_arg, i32_arg, FastMathFlags::NO_NANS, "s"),
        "nnan select of i32",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::SelectFastMathNotFloatingPoint)
    );
    Ok(())
}

/// The aggregate, shuffle and `ptrtoaddr` entries refuse their malformed
/// operands with the variant naming the instruction and the rule.
#[test]
fn aggregate_shuffle_and_ptrtoaddr_refusals() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-shapes")?;
    let mut key = module.layout_token();
    let m = key.module();
    let ptr_ty = m.ptr_type_in::<0>();
    let int_vector = m.vector_type(m.i32_type(), lanes::<4>())?;
    let fn_ty = m.function_type(
        m.void_type(),
        [
            m.i32_type().as_type(),
            ptr_ty.as_type(),
            int_vector.as_type(),
        ],
    )?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&mut key).position_at_end(entry);
    let i32_arg = m.view(f).param(0)?.as_erased();
    let ptr_arg = m.view(f).param(1)?.as_erased();
    let int_vector_arg = m.view(f).param(2)?.as_erased();

    let err = refusal(
        b.extract_value_dyn(i32_arg, &[], "e"),
        "extractvalue without indices",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::EmptyAggregateIndices {
            instruction: AggregateInstruction::ExtractValue,
        })
    );
    let err = refusal(
        b.insert_value_dyn(i32_arg, i32_arg, &[], "i"),
        "insertvalue without indices",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::EmptyAggregateIndices {
            instruction: AggregateInstruction::InsertValue,
        })
    );

    let err = refusal(
        b.shuffle_vector(
            int_vector_arg,
            int_vector_arg,
            &[ShuffleMaskElem::Lane(100)],
            "v",
        ),
        "shufflevector of an out-of-range lane",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::InvalidShuffleVectorOperands)
    );

    let err = refusal(
        b.ptr_to_addr_dyn(i32_arg, m.i64_type().as_type(), "p"),
        "ptrtoaddr of an i32",
    );
    assert_eq!(err, IrError::from(BuilderError::PtrToAddrSourceNotPointer));
    let err = refusal(
        b.ptr_to_addr_dyn(ptr_arg, m.i32_type().as_type(), "p"),
        "ptrtoaddr to a narrower integer than the address",
    );
    assert_eq!(err, IrError::from(BuilderError::PtrToAddrResultWidth));
    Ok(())
}

/// `struct_gep` refuses a field index its `i32` index constant cannot spell,
/// and carries the index it was asked for.
#[test]
fn struct_gep_refuses_an_index_wider_than_i32() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-struct-gep")?;
    let mut key = module.layout_token();
    let m = key.module();
    let s_ty = m.get_or_insert_named_struct("S");
    m.set_struct_body_dyn(s_ty, [m.i32_type().as_type()], false)?;
    let ptr_ty = m.ptr_type_in::<0>();
    let fn_ty = m.function_type(m.void_type(), [ptr_ty.as_type()])?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&mut key).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, llvmkit_ir::SpaceDyn, _> =
        m.view(f).param(0)?.try_into()?;
    let err = refusal(
        b.struct_gep(s_ty, p, u32::MAX, "g"),
        "struct_gep of u32::MAX",
    );
    assert_eq!(
        err,
        IrError::from(BuilderError::StructFieldIndexTooWide { index: u32::MAX })
    );
    Ok(())
}

/// A snapshot saved before an instruction that is then erased refuses to
/// restore. Upstream's `IRBuilder` keeps a raw iterator and is undefined
/// there.
///
/// The builder that saved the snapshot must be gone first. Erasing takes the
/// module's `LayoutToken` `&mut`, which a live builder holds, so a build
/// through a builder whose anchor was erased does not compile
/// (`tests/compile_fail/erase_anchor_while_builder_lives.rs`). A saved
/// `InsertPoint` is plain data that outlives its builder, which is why its
/// restore is where a stale anchor is still checked at run time. An
/// unpositioned builder has no insert point to save at all
/// (`tests/compile_fail/save_insert_point_unpositioned.rs`).
#[test]
fn a_snapshot_before_an_erased_anchor_refuses_to_restore() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-stale-anchor")?;
    let mut key = module.layout_token();
    let m = key.module();
    let fn_ty = m.function_type(m.void_type(), Vec::<llvmkit_ir::Type<'_, _>>::new())?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let (_, ret) = IrBuilder::new_for::<Dyn>(&mut key)
        .position_at_end(entry)
        .ret_void()?;
    let saved = IrBuilder::new_for::<Dyn>(&mut key)
        .position_before(ret.placed())?
        .save_insert_point();
    ret.erase_from_parent(&mut key)?;
    let err = refusal(
        IrBuilder::new_for::<Dyn>(&mut key).restore_insert_point(saved),
        "restoring a snapshot before an erased anchor",
    );
    assert_eq!(err, IrError::from(BlockError::AnchorNotInBlock));
    Ok(())
}

/// `position_past_allocas` refuses a declaration, which has no entry block to
/// position in. Upstream's `SetInsertPointPastAllocas` calls
/// `F->getEntryBlock()`, which is undefined on a declaration; llvmkit's typed
/// refusal is its own.
#[test]
fn position_past_allocas_refuses_a_declaration() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-past-allocas-declaration")?;
    let mut key = module.layout_token();
    let m = key.module();
    let fn_ty = m.function_type(m.void_type(), Vec::<llvmkit_ir::Type<'_, _>>::new())?;
    let declaration = m.add_function_dyn("declared", fn_ty, Linkage::External)?;
    let err = refusal(
        IrBuilder::new_for::<Dyn>(&mut key).position_past_allocas(m.view(declaration)),
        "positioning past the allocas of a declaration",
    );
    assert_eq!(err, IrError::from(BuilderError::FunctionWithoutBlocks));
    assert_eq!(
        err.to_string(),
        "cannot position past the allocas of a function with no blocks"
    );
    Ok(())
}

/// `position_past_allocas` refuses a function of another module that shares
/// the builder's brand (every `Module::dynamic` is `DynBrand`), before reading
/// any of its blocks: the module tag is the backstop the brand cannot be.
/// Upstream's `SetInsertPointPastAllocas` takes any `Function *`; llvmkit's
/// refusal is its own.
#[test]
fn position_past_allocas_refuses_another_modules_function() -> Result<(), IrError> {
    let mut builder_module = llvmkit_ir::Module::dynamic("builder-module");
    let mut builder_key = builder_module.layout_token();
    let mut other_module = llvmkit_ir::Module::dynamic("other-module");
    let mut other_key = other_module.layout_token();
    let other = other_key.module();
    let fn_ty = other.function_type(other.void_type(), Vec::<llvmkit_ir::Type<'_, _>>::new())?;
    let f = other.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = other.view(f).append_basic_block(other, "entry");
    IrBuilder::new_for::<Dyn>(&mut other_key)
        .try_position_at_end(entry)?
        .ret_void()?;
    let err = refusal(
        IrBuilder::new_for::<Dyn>(&mut builder_key).position_past_allocas(other.view(f)),
        "positioning past the allocas of another module's function",
    );
    assert_eq!(err, IrError::ForeignValueId);
    Ok(())
}

/// `phi_add_incoming_from_value` refuses a target that is not an
/// instruction, and an instruction that is not a `phi`.
#[test]
fn phi_target_refusals() -> Result<(), IrError> {
    let mut module = module_new!("builder-errors-position")?;
    let mut key = module.layout_token();
    let m = key.module();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(m.void_type(), [i32_ty.as_type()])?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let entry_id = entry.id();

    let b = IrBuilder::new_for::<Dyn>(&mut key).position_at_end(entry);
    let arg = m.view(f).param(0)?.as_erased();
    let err = refusal(
        b.phi_add_incoming_from_value(arg, arg, entry_id),
        "an incoming added to an argument",
    );
    assert_eq!(err, IrError::from(BuilderError::PhiTargetNotAnInstruction));
    let sum = b.int_binop_erased(BinaryOpcode::Add, arg, arg, IntBinOpFlags::new(), "sum")?;
    let err = refusal(
        b.phi_add_incoming_from_value(sum, arg, entry_id),
        "an incoming added to an add",
    );
    assert_eq!(err, IrError::from(BuilderError::PhiTargetNotAPhi));
    Ok(())
}
