//! Phase A4-int coverage. Verifies that the
//! [`IntoIntValue`](llvmkit_ir::IntoIntValue) trait makes `IntValue`,
//! `ConstantIntValue`, and Rust scalar literals all valid operands at
//! the same call site.
//!
//! ## Upstream provenance
//!
//! Per-test citations below. Each `#[test]` carries a doc comment naming the
//! upstream `unittests/IR/IRBuilderTest.cpp` TEST_F it ports, or marks itself
//! `llvmkit-specific:` (e.g. the Rust-literal coercion that has no C++ analogue).

use llvmkit_ir::{
    ApInt, Constant, ConstantIntValue, Dyn, IntDyn, IntValue, IrBuilder, IrError, Linkage,
    NoFolder, module_new,
};

/// llvmkit-specific: exercises `IntoIntValue` for `IntValue` LHS plus a Rust
/// `i32` literal RHS at the same `int_add` call site (no C++ analogue;
/// upstream callers always materialise a `Value*`). Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`
/// exercises `Builder.CreateAdd(getInt32(1), getInt32(2), "add")`.
#[test]
fn build_int_add_accepts_int_value_and_rust_literal() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("inc", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    // Rust literal as RHS.
    let next = b.int_add(n, 1_i32, "next")?;
    b.ret(next)?;

    let text = format!("{m}");
    assert!(text.contains("%next = add i32 %0, 1\n"), "got:\n{text}");
    Ok(())
}
/// llvmkit-specific regression for LLVM's `Value::setName` uniquing path:
/// `IRBuilderDefaultInserter::InsertHelper` calls `I->setName(Name)`, and
/// `ValueSymbolTable::createValueName` appends a function-wide bare integer
/// suffix for local-value conflicts. Closest upstream unit coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`.
#[test]
fn build_int_ops_unique_duplicate_requested_names() -> Result<(), IrError> {
    let m = module_new!("names")?;
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, [i64_ty.as_type()]);
    let f = m.add_function_dyn("names", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let sp: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;

    let first_push = b.int_sub::<i64, _, _, _>(sp, 8_i64, "push_sp")?;
    let second_push = b.int_sub::<i64, _, _, _>(first_push, 8_i64, "push_sp")?;
    let first_af = b.int_xor::<i64, _, _, _>(first_push, second_push, "af_lhs_rhs")?;
    let second_af = b.int_xor::<i64, _, _, _>(second_push, first_af, "af_lhs_rhs")?;
    b.ret(second_af)?;

    assert_eq!(m.view(first_push).name().as_deref(), Some("push_sp"));
    assert_eq!(m.view(second_push).name().as_deref(), Some("push_sp1"));
    assert_eq!(m.view(first_af).name().as_deref(), Some("af_lhs_rhs"));
    assert_eq!(m.view(second_af).name().as_deref(), Some("af_lhs_rhs2"));

    let expected = "; ModuleID = 'names'\n\
        \n\
        define i64 @names(i64 %0) {\n\
        entry:\n\
        \x20\x20%push_sp = sub i64 %0, 8\n\
        \x20\x20%push_sp1 = sub i64 %push_sp, 8\n\
        \x20\x20%af_lhs_rhs = xor i64 %push_sp, %push_sp1\n\
        \x20\x20%af_lhs_rhs2 = xor i64 %push_sp1, %af_lhs_rhs\n\
        \x20\x20ret i64 %af_lhs_rhs2\n\
        }\n";
    assert_eq!(format!("{m}"), expected);
    Ok(())
}

/// llvmkit-specific: `ConstantIntValue` LHS + `IntValue` RHS through
/// `IntoIntValue`. Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`.
#[test]
fn build_int_sub_accepts_constant_and_argument() -> Result<(), IrError> {
    let m = module_new!("s")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("dec", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let c = i32_ty.const_int(10_i32);
    // ConstantIntValue as LHS, IntValue as RHS.
    let r = b.int_sub(c, n, "r")?;
    b.ret(r)?;

    let text = format!("{m}");
    // Folder doesn't fire (one operand is non-constant); the
    // instruction must materialise.
    assert!(text.contains("%r = sub i32 10, %0\n"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: typed builder `IrBuilder::<i32>::ret` accepts a Rust
/// `i32` literal directly via `IntoIntValue`. Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)` (a
/// builder-driven module that round-trips through the AsmWriter).
#[test]
fn build_ret_accepts_rust_literal_directly() -> Result<(), IrError> {
    // `i32` builder: `b.ret(1_i32)?` works without the
    // caller materialising an `IntValue` first.
    let m = module_new!("r")?;
    let f = m.add_typed_function::<i32, (), _>("one", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    b.ret(1_i32)?;

    let text = format!("{m}");
    assert!(text.contains("ret i32 1\n"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific APInt regression for
/// `ConstantFold.cpp::ConstantFoldBinaryInstruction`'s integer `add` path:
/// wide constants must not be narrowed through `u128` by the default builder
/// folder.
#[test]
fn default_constant_folder_preserves_wide_apint_add() -> Result<(), IrError> {
    let m = module_new!("wide-fold")?;
    let ty = m.int_type_n::<257>();
    let fn_ty = m.function_type(ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("wide", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let high = ty.const_ap_int(&ApInt::one_bit_set(257, 256))?;
    let result = b.int_add(high, ty.const_zero(), "sum")?;
    let folded =
        ConstantIntValue::<IntDyn, _>::try_from(Constant::try_from(b.view(result).as_erased())?)?;
    assert_eq!(folded.ap_int(), ApInt::one_bit_set(257, 256));
    Ok(())
}

/// llvmkit-specific APInt regression for
/// `ConstantFold.cpp::ConstantFoldBinaryInstruction`'s integer `udiv` path:
/// the default builder folder must route all integer binary opcodes through
/// the shared arbitrary-precision folder, not only add/sub/mul.
#[test]
fn default_constant_folder_folds_udiv_to_constant() -> Result<(), IrError> {
    let m = module_new!("udiv-fold")?;
    let ty = m.i32_type();
    let fn_ty = m.function_type(ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("quotient", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let result = b.int_udiv(ty.const_int(9_i32), ty.const_int(3_i32), "q")?;
    let folded =
        ConstantIntValue::<IntDyn, _>::try_from(Constant::try_from(b.view(result).as_erased())?)?;
    assert_eq!(folded.ap_int().try_zext_u64(), Some(3));
    Ok(())
}

/// llvmkit-specific permanent lock for task #72 (no-silent-erasure strict
/// cut): `int_add(2i32, 3i32, "sum")` must compile with **no
/// turbofish and no width annotation**. This is only possible because a
/// Rust `i32` literal now maps to exactly one IR width (`i32`) -- the
/// literal-widening impls and the `i32 -> Width<N>` scalar impls were
/// deleted, so `W` has a single solution and is inferred from the argument
/// types alone.
///
/// The isolation is deliberate: `sum` is consumed only through the
/// width-agnostic `HasName::name` accessor, and the block is terminated
/// with an independent `0_i32` literal, so nothing downstream pins `W`. If
/// a second `IntoIntValue<W>` solution for `i32` were reintroduced, this
/// file would fail to build with `E0283` on the `int_add` call.
#[test]
fn build_int_add_infers_width_from_literals_no_turbofish() -> Result<(), IrError> {
    let m = module_new!("no-turbofish")?;
    let f = m.add_typed_function::<i32, (), _>("k", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    // `NoFolder` so the all-constant add materializes as a named
    // instruction; the default folder would collapse `2 + 3` to `5`.
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);

    // THE LOCK: two bare `i32` literals, no `::<i32>`, no annotation.
    let sum = b.int_add(2i32, 3i32, "sum")?;
    // Width-agnostic use: does not feed a width back into `sum`.
    assert_eq!(b.view(sum).name().as_deref(), Some("sum"));
    // Terminate with an independent literal so `sum`'s `W` stays
    // inferred from `int_add`'s arguments only.
    b.ret(0_i32)?;

    let text = format!("{m}");
    assert!(text.contains("%sum = add i32 2, 3\n"), "got:\n{text}");
    Ok(())
}
