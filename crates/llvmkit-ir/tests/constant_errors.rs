//! Each typed constant refusal is the variant its entry returns.
//!
//! **No upstream counterpart.** Upstream's constant constructors
//! (`ConstantInt::get`, `BlockAddress::get`, `ConstantExpr::get*`, ...)
//! `assert` these conditions, so `unittests/IR/ConstantsTest.cpp` cannot
//! observe them; where `LLParser::parseValID` reports one, the parser tests
//! pin its text. What these tests pin is llvmkit's typed error surface: each
//! refusal names its rule, including the rules that share one message
//! (`GepConstantError::OperandTypes`, `AddressSpaceMismatch` and
//! `VectorWidthMismatch` all print "invalid getelementptr constant
//! expression"). `tests/typed_error_coverage.rs` checks that every variant is
//! asserted somewhere.

use llvmkit_ir::{
    ApInt, Constant, ConstantError, ConstantExprError, ConstantExprFlags, ConstantExprInRange,
    ConstantExprOpcode, ConstantExprOptions, ConstantFloatValue, ConstantIntValue, FloatDyn,
    GepConstantError, GepNoWrapFlags, IntDyn, IrError, Linkage, Type, module_new,
};

/// The refusal an `Err` carries, or a panic naming the entry that succeeded.
fn refusal<T>(result: Result<T, IrError>, entry: &str) -> IrError {
    match result {
        Ok(_) => panic!("{entry} must refuse"),
        Err(error) => error,
    }
}

/// A constant of integer or floating-point type that is not a `ConstantInt` /
/// `ConstantFP` does not narrow to the typed integer or float handle.
#[test]
fn a_poison_does_not_narrow_to_a_typed_scalar_constant() -> Result<(), IrError> {
    let m = module_new!("constant-errors-narrow")?;
    let int_poison: Constant<'_, _> = m.i32_type().as_type().poison().as_constant();
    let err = refusal(
        ConstantIntValue::<IntDyn, _>::try_from(int_poison),
        "an i32 poison as a ConstantInt",
    );
    assert_eq!(err, IrError::Constant(ConstantError::NotAnIntegerConstant));
    let float_poison: Constant<'_, _> = m.f32_type().as_type().poison().as_constant();
    let err = refusal(
        ConstantFloatValue::<FloatDyn, _>::try_from(float_poison),
        "a float poison as a ConstantFP",
    );
    assert_eq!(
        err,
        IrError::Constant(ConstantError::NotAFloatingPointConstant)
    );
    Ok(())
}

/// `blockaddress`, `dso_local_equivalent` and `no_cfi` refuse an operand that
/// breaks their rule, each with its own variant.
#[test]
fn global_wrapping_constants_name_their_rule() -> Result<(), IrError> {
    let m = module_new!("constant-errors-globals")?;
    let fn_ty = m.function_type(m.void_type(), Vec::<Type<'_, _>>::new())?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let g = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let _f_entry = m.view(f).append_basic_block(&m, "entry");
    let g_entry = m.view(g).append_basic_block(&m, "entry");
    let err = refusal(
        m.block_address(m.view(f), &g_entry),
        "blockaddress of @f with a block of @g",
    );
    assert_eq!(
        err,
        IrError::Constant(ConstantError::BlockAddressOfForeignBlock)
    );

    let variable = m.add_global("v", m.i32_type().const_zero())?;
    let variable_ptr = m.view(variable).as_global_constant_ptr();
    let err = refusal(
        m.dso_local_equivalent_global(variable_ptr),
        "dso_local_equivalent of a global variable",
    );
    assert_eq!(
        err,
        IrError::Constant(ConstantError::DsoLocalEquivalentTarget)
    );

    let null: Constant<'_, _> = m.ptr_type_in::<0>().const_zero().as_constant();
    let err = refusal(m.no_cfi_global(null), "no_cfi of a null pointer");
    assert_eq!(err, IrError::Constant(ConstantError::NoCfiTarget));
    Ok(())
}

/// `target_ext_none` refuses a target extension type without `HasZeroInit`,
/// and a forward-reference placeholder refuses a type that is not
/// first-class.
#[test]
fn null_and_placeholder_constants_refuse_their_types() -> Result<(), IrError> {
    let m = module_new!("constant-errors-types")?;
    let no_zero = m.target_ext_type("other", Vec::<Type<'_, _>>::new(), [])?;
    let err = refusal(
        m.target_ext_none(no_zero.as_type()),
        "none of a target type without HasZeroInit",
    );
    assert_eq!(
        err,
        IrError::Constant(ConstantError::NullOfTypeWithoutZeroInit)
    );
    let err = refusal(
        m.forward_ref_value_placeholder(m.void_type().as_type()),
        "a void placeholder",
    );
    assert_eq!(
        err,
        IrError::Constant(ConstantError::ForwardReferenceNotFirstClass)
    );
    Ok(())
}

/// A placeholder used inside a constant cannot be replaced by a value that is
/// not a constant: a constant's operands are constants.
#[test]
fn a_placeholder_inside_a_constant_refuses_a_non_constant_replacement() -> Result<(), IrError> {
    let m = module_new!("constant-errors-rauw")?;
    let i32_ty = m.i32_type();
    let array_ty = m.array_type_n::<i32, 2>();
    let fn_ty = m.function_type(m.void_type(), [i32_ty.as_type()])?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let placeholder = m.forward_ref_value_placeholder(i32_ty.as_type())?;
    let aggregate =
        array_ty.const_array([placeholder.as_constant(), i32_ty.const_int(7).into()])?;
    m.add_global("a", aggregate)?;
    let err = refusal(
        placeholder.replace_all_uses_with(m.view(f).param(0)?.as_erased()),
        "a constant's placeholder replaced by an argument",
    );
    assert_eq!(
        err,
        IrError::Constant(ConstantError::NonConstantReplacement)
    );
    Ok(())
}

/// A constant expression refuses the wrong operand count, flags of another
/// opcode, and an empty `inrange`, each with its own variant.
#[test]
fn constant_expression_shape_refusals() -> Result<(), IrError> {
    let m = module_new!("constant-errors-expr")?;
    let i32_ty = m.i32_type();
    let i8_ty = m.i8_type();
    let one = i32_ty.const_int(1_i32);

    let err = refusal(
        m.constant_expr(
            i8_ty.as_type(),
            ConstantExprOpcode::Trunc,
            [one.as_erased(), one.as_erased()],
            [],
            [],
            ConstantExprFlags::none(),
        ),
        "a two-operand trunc",
    );
    assert_eq!(
        err,
        IrError::from(ConstantExprError::OperandCount {
            opcode: ConstantExprOpcode::Trunc,
        })
    );
    let err = refusal(
        m.constant_expr(
            i8_ty.as_type(),
            ConstantExprOpcode::Trunc,
            [one.as_erased()],
            [],
            [],
            ConstantExprFlags::gep(GepNoWrapFlags::inbounds()),
        ),
        "a trunc with GEP flags",
    );
    assert_eq!(err, IrError::from(ConstantExprError::FlagsForOpcode));

    let g = m.add_global("g", i8_ty.const_zero())?;
    let base = m.view(g).as_global_constant_ptr();
    let offset = m.i64_type().const_int(1_i64);
    let empty = ConstantExprInRange::new(ApInt::from_words(64, &[1]), ApInt::from_words(64, &[1]));
    let err = refusal(
        m.constant_expr_with_options(
            m.ptr_type_in::<0>().as_type(),
            ConstantExprOpcode::GetElementPtr,
            [base.as_erased(), offset.as_erased()],
            [],
            [],
            ConstantExprOptions::new().source_ty(i8_ty.as_type()).flags(
                ConstantExprFlags::gep_with_in_range(GepNoWrapFlags::empty(), empty),
            ),
        ),
        "a GEP with an empty inrange",
    );
    assert_eq!(err, IrError::from(ConstantExprError::EmptyInRange));
    Ok(())
}

/// A constant `getelementptr` refuses a missing source type and a base that is
/// no pointer, each with its own variant.
#[test]
fn constant_gep_refusals_name_their_rule() -> Result<(), IrError> {
    let m = module_new!("constant-errors-gep")?;
    let i8_ty = m.i8_type();
    let g = m.add_global("g", i8_ty.const_zero())?;
    let base = m.view(g).as_global_constant_ptr();
    let offset = m.i64_type().const_int(1_i64);
    let err = refusal(
        m.constant_expr(
            m.ptr_type_in::<0>().as_type(),
            ConstantExprOpcode::GetElementPtr,
            [base.as_erased(), offset.as_erased()],
            [],
            [],
            ConstantExprFlags::none(),
        ),
        "a GEP without a source type",
    );
    assert_eq!(err, IrError::from(GepConstantError::MissingSourceType));

    let not_a_pointer = m.i32_type().const_int(0_i32);
    let err = refusal(
        m.constant_expr_with_options(
            m.ptr_type_in::<0>().as_type(),
            ConstantExprOpcode::GetElementPtr,
            [not_a_pointer.as_erased(), offset.as_erased()],
            [],
            [],
            ConstantExprOptions::new().source_ty(i8_ty.as_type()),
        ),
        "a GEP over an i32 base",
    );
    assert_eq!(err, IrError::from(GepConstantError::OperandTypes));
    Ok(())
}

/// The verifier refuses a `ptrtoaddr` constant whose result is not its
/// pointer's address width, which the constructor's `castIsValid` admits.
#[test]
fn the_verifier_refuses_a_ptrtoaddr_constant_of_the_wrong_width() -> Result<(), IrError> {
    let m = module_new!("constant-errors-ptrtoaddr")?;
    let g = m.add_global("g", m.i32_type().const_zero())?;
    let narrow = m.constant_expr(
        m.i32_type().as_type(),
        ConstantExprOpcode::PtrToAddr,
        [m.view(g).as_global_constant_ptr().as_erased()],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    m.add_global("addr", narrow)?;
    let err = refusal(
        m.verify_borrowed(),
        "a ptrtoaddr to i32 of a 64-bit address",
    );
    assert_eq!(err, IrError::from(ConstantExprError::PtrToAddrResultWidth));
    Ok(())
}
