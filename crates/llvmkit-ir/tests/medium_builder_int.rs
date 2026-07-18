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
    ApInt, Constant, ConstantIntValue, IRBuilder, IntDyn, IntValue, IrError, Linkage, Module,
    NoFolder, Width,
};

/// llvmkit-specific: exercises `IntoIntValue` for `IntValue` LHS plus a Rust
/// `i32` literal RHS at the same `build_int_add` call site (no C++ analogue;
/// upstream callers always materialise a `Value*`). Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`
/// exercises `Builder.CreateAdd(getInt32(1), getInt32(2), "add")`.
#[test]
fn build_int_add_accepts_int_value_and_rust_literal() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("inc", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let n: IntValue<i32> = f.param(0)?.try_into()?;
        // Rust literal as RHS.
        let next = b.build_int_add(n, 1_i32, "next")?;
        b.build_ret(next)?;

        let text = format!("{m}");
        assert!(text.contains("%next = add i32 %0, 1\n"), "got:\n{text}");
        Ok(())
    })
}
/// llvmkit-specific regression for LLVM's `Value::setName` uniquing path:
/// `IRBuilderDefaultInserter::InsertHelper` calls `I->setName(Name)`, and
/// `ValueSymbolTable::createValueName` appends a function-wide bare integer
/// suffix for local-value conflicts. Closest upstream unit coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`.
#[test]
fn build_int_ops_unique_duplicate_requested_names() -> Result<(), IrError> {
    Module::with_new("names", |m| {
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type()], false);
        let f = m.add_function::<i64, _>("names", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
        let sp: IntValue<i64> = f.param(0)?.try_into()?;

        let first_push = b.build_int_sub::<i64, _, _, _>(sp, 8_i64, "push_sp")?;
        let second_push = b.build_int_sub::<i64, _, _, _>(first_push, 8_i64, "push_sp")?;
        let first_af = b.build_int_xor::<i64, _, _, _>(first_push, second_push, "af_lhs_rhs")?;
        let second_af = b.build_int_xor::<i64, _, _, _>(second_push, first_af, "af_lhs_rhs")?;
        b.build_ret(second_af)?;

        assert_eq!(first_push.name().as_deref(), Some("push_sp"));
        assert_eq!(second_push.name().as_deref(), Some("push_sp1"));
        assert_eq!(first_af.name().as_deref(), Some("af_lhs_rhs"));
        assert_eq!(second_af.name().as_deref(), Some("af_lhs_rhs2"));

        let expected = "; ModuleID = 'names'\n\
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
    })
}

/// llvmkit-specific: `ConstantIntValue` LHS + `IntValue` RHS through
/// `IntoIntValue`. Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`.
#[test]
fn build_int_sub_accepts_constant_and_argument() -> Result<(), IrError> {
    Module::with_new("s", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("dec", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let n: IntValue<i32> = f.param(0)?.try_into()?;
        let c = i32_ty.const_int(10_i32);
        // ConstantIntValue as LHS, IntValue as RHS.
        let r = b.build_int_sub(c, n, "r")?;
        b.build_ret(r)?;

        let text = format!("{m}");
        // Folder doesn't fire (one operand is non-constant); the
        // instruction must materialise.
        assert!(text.contains("%r = sub i32 10, %0\n"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific: typed builder `IRBuilder::<i32>::build_ret` accepts a Rust
/// `i32` literal directly via `IntoIntValue`. Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)` (a
/// builder-driven module that round-trips through the AsmWriter).
#[test]
fn build_ret_accepts_rust_literal_directly() -> Result<(), IrError> {
    // `i32` builder: `b.build_ret(1_i32)?` works without the
    // caller materialising an `IntValue` first.
    Module::with_new("r", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<i32, _>("one", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_ret(1_i32)?;

        let text = format!("{m}");
        assert!(text.contains("ret i32 1\n"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific APInt regression for
/// `ConstantFold.cpp::ConstantFoldBinaryInstruction`'s integer `add` path:
/// wide constants must not be narrowed through `u128` by the default builder
/// folder.
#[test]
fn default_constant_folder_preserves_wide_apint_add() -> Result<(), IrError> {
    Module::with_new("wide-fold", |m| {
        let ty = m.int_type_n::<257>();
        let fn_ty = m.fn_type(ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<Width<257>, _>("wide", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Width<257>>(&m).position_at_end(entry);
        let high = ty.const_ap_int(&ApInt::one_bit_set(257, 256))?;
        let result = b.build_int_add(high, ty.const_zero(), "sum")?;
        let folded = ConstantIntValue::<IntDyn>::try_from(Constant::try_from(result.as_value())?)?;
        assert_eq!(folded.ap_int(), ApInt::one_bit_set(257, 256));
        Ok(())
    })
}

/// llvmkit-specific APInt regression for
/// `ConstantFold.cpp::ConstantFoldBinaryInstruction`'s integer `udiv` path:
/// the default builder folder must route all integer binary opcodes through
/// the shared arbitrary-precision folder, not only add/sub/mul.
#[test]
fn default_constant_folder_folds_udiv_to_constant() -> Result<(), IrError> {
    Module::with_new("udiv-fold", |m| {
        let ty = m.i32_type();
        let fn_ty = m.fn_type(ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<i32, _>("quotient", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let result = b.build_int_udiv(ty.const_int(9_i32), ty.const_int(3_i32), "q")?;
        let folded = ConstantIntValue::<IntDyn>::try_from(Constant::try_from(result.as_value())?)?;
        assert_eq!(folded.ap_int().try_zext_u64(), Some(3));
        Ok(())
    })
}

/// llvmkit-specific permanent lock for task #72 (no-silent-erasure strict
/// cut): `build_int_add(2i32, 3i32, "sum")` must compile with **no
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
/// file would fail to build with `E0283` on the `build_int_add` call.
#[test]
fn build_int_add_infers_width_from_literals_no_turbofish() -> Result<(), IrError> {
    Module::with_new("no-turbofish", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<i32, _>("k", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        // `NoFolder` so the all-constant add materializes as a named
        // instruction; the default folder would collapse `2 + 3` to `5`.
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        // THE LOCK: two bare `i32` literals, no `::<i32>`, no annotation.
        let sum = b.build_int_add(2i32, 3i32, "sum")?;
        // Width-agnostic use: does not feed a width back into `sum`.
        assert_eq!(sum.name().as_deref(), Some("sum"));
        // Terminate with an independent literal so `sum`'s `W` stays
        // inferred from `build_int_add`'s arguments only.
        b.build_ret(0_i32)?;

        let text = format!("{m}");
        assert!(text.contains("%sum = add i32 2, 3\n"), "got:\n{text}");
        Ok(())
    })
}
