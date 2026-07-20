//! `Display` coverage for the public value/constant handles.
//!
//! Every typed handle in the surface widens to an erased [`Value`] (or, for
//! module-level globals, prints its own definition line). This file locks the
//! invariant that a handle's `Display` **agrees** with the path it delegates
//! to -- printing through `IntValue<i32>` must produce the same bytes as
//! printing through its `.as_value()`, and printing a `FunctionValue` or
//! `GlobalVariable` must produce exactly the text that appears for it in
//! `format!("{m}")` module output. Non-empty output is not the property under
//! test; byte-for-byte agreement is.
//!
//! ## Upstream provenance
//!
//! The printed forms mirror `llvm/lib/IR/AsmWriter.cpp`:
//! `AssemblyWriter::printGlobal` for the `@g = global ...` line,
//! `printFunction` for the `define`/`declare` header, and `WriteAsOperand`
//! for the `<type> <ref>` operand form. `APInt`'s signed-decimal rendering
//! mirrors `APInt::toString` in `llvm/lib/Support/APInt.cpp`.

use llvmkit_ir::{
    ApInt, ApIntSignedness, Dyn, IRBuilder, IntValue, IrError, Linkage, Module, PointerValue,
};

// --------------------------------------------------------------------------
// FunctionValue -- prints its full definition, not its operand form
// --------------------------------------------------------------------------

/// A body-less function prints the one-line `declare` form. Mirrors the
/// `header == "declare"` early return in `AssemblyWriter::printFunction`.
#[test]
fn function_value_prints_declare_line() -> Result<(), IrError> {
    Module::with_new("declare_display", |m| {
        let void = m.void_type();
        let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("ext", fn_ty, Linkage::External)?;

        assert_eq!(format!("{f}"), "declare void @ext()\n");
        Ok(())
    })
}

/// A function with basic blocks prints the full `define` form, and that text
/// is exactly the slice the module writer emits for it.
#[test]
fn function_value_define_matches_module_output() -> Result<(), IrError> {
    Module::with_new("define_display", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function_dyn("add", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let lhs: IntValue<i32> = f.param(0)?.try_into()?;
        let rhs: IntValue<i32> = f.param(1)?.try_into()?;
        let sum = b.build_int_add(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        let printed = format!("{f}");
        let expected = "define i32 @add(i32 %0, i32 %1) {\n\
            entry:\n\
            \x20\x20%sum = add i32 %0, %1\n\
            \x20\x20ret i32 %sum\n\
            }\n";
        assert_eq!(printed, expected, "got:\n{printed}");

        // The handle's own text is byte-identical to the module's rendering
        // of it -- Display is the same writer, not a parallel one.
        let module_text = format!("{m}");
        assert!(module_text.contains(&printed), "got:\n{module_text}");
        Ok(())
    })
}

// --------------------------------------------------------------------------
// GlobalVariable -- definition line, matching its GlobalAlias/GlobalIFunc
// siblings rather than the `ptr @g` operand form
// --------------------------------------------------------------------------

/// Mirrors `AssemblyWriter::printGlobal`: the definition line carries no
/// trailing newline (the module loop supplies it).
#[test]
fn global_variable_prints_definition_line() -> Result<(), IrError> {
    Module::with_new("global_display", |m| {
        let i32_ty = m.i32_type();
        let g = m.add_global("g1", i32_ty.const_zero())?;

        assert_eq!(format!("{g}"), "@g1 = global i32 0");

        let module_text = format!("{m}");
        assert!(
            module_text.contains(&format!("{g}\n")),
            "got:\n{module_text}"
        );

        // The operand form stays available through the erased handle and is
        // deliberately different from the definition form.
        assert_eq!(format!("{}", g.as_value()), "ptr @g1");
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Value handles -- Display must agree with the erased `.as_value()` path
// --------------------------------------------------------------------------

/// The core invariant: each typed handle prints exactly what its erased
/// [`Value`] prints. A handle that disagreed here would silently produce IR
/// text that the module writer never emits.
#[test]
fn typed_handles_agree_with_erased_value() -> Result<(), IrError> {
    Module::with_new("agreement", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let ptr_ty = m.ptr_type(0);

        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let doubled = b.build_int_add(x, x, "doubled")?;
        b.build_ret(doubled)?;

        // Argument.
        let arg = f.param(0)?;
        assert_eq!(format!("{arg}"), format!("{}", arg.as_value()));

        // IntValue<W> -- both an instruction result and a parameter.
        assert_eq!(format!("{doubled}"), format!("{}", doubled.as_value()));
        assert_eq!(format!("{x}"), format!("{}", x.as_value()));

        // ConstantIntValue<W>.
        let c_int = i32_ty.const_int(42_i32);
        assert_eq!(format!("{c_int}"), format!("{}", c_int.as_value()));

        // ConstantFloatValue<K>.
        let c_float = f32_ty.const_float(3.5_f32);
        assert_eq!(format!("{c_float}"), format!("{}", c_float.as_value()));

        // ConstantPointerNull / UndefValue / PoisonValue -- the
        // `decl_constant_handle!` family.
        let null = ptr_ty.const_null();
        assert_eq!(format!("{null}"), format!("{}", null.as_value()));
        let undef = i32_ty.as_type().get_undef();
        assert_eq!(format!("{undef}"), format!("{}", undef.as_value()));
        let poison = i32_ty.as_type().get_poison();
        assert_eq!(format!("{poison}"), format!("{}", poison.as_value()));

        // PointerValue -- the `decl_value_handle!` family.
        let p = PointerValue::try_from(null.as_value())?;
        assert_eq!(format!("{p}"), format!("{}", p.as_value()));

        // The erased `Constant` handle agrees with the erased `Value` too.
        let c_erased = c_int.as_constant();
        assert_eq!(format!("{c_erased}"), format!("{}", c_erased.as_value()));
        Ok(())
    })
}

/// Exact-string locks for the constant forms that are stable across the
/// surface. Mirrors `WriteAsOperand`: `<type> <literal>`.
#[test]
fn constant_handles_print_expected_operand_text() -> Result<(), IrError> {
    Module::with_new("constant_display", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);

        assert_eq!(format!("{}", i32_ty.const_int(42_i32)), "i32 42");
        assert_eq!(format!("{}", i32_ty.const_int(-7_i32)), "i32 -7");
        assert_eq!(format!("{}", i32_ty.const_zero()), "i32 0");
        assert_eq!(format!("{}", ptr_ty.const_null()), "ptr null");
        assert_eq!(format!("{}", i32_ty.as_type().get_undef()), "i32 undef");
        assert_eq!(format!("{}", i32_ty.as_type().get_poison()), "i32 poison");

        // `i1` constants print as `true`/`false`, not `1`/`0`. Mirrors the
        // `bits == 1` special case in the assembly writer.
        let i1_ty = m.bool_type();
        assert_eq!(format!("{}", i1_ty.const_int(true)), "i1 true");
        assert_eq!(format!("{}", i1_ty.const_int(false)), "i1 false");
        Ok(())
    })
}

// --------------------------------------------------------------------------
// ApInt -- signed decimal, no bit width
// --------------------------------------------------------------------------

/// `ApInt` prints the two's-complement signed reading of its bits in decimal,
/// with no width suffix. Mirrors `APInt::toString(10, /*Signed=*/true)`.
#[test]
fn ap_int_prints_signed_decimal() -> Result<(), IrError> {
    assert_eq!(format!("{}", ApInt::zero(32)), "0");
    assert_eq!(format!("{}", ApInt::zero(1)), "0");

    // All-ones reads as -1 at every width -- the signed interpretation, not
    // the unsigned magnitude.
    assert_eq!(format!("{}", ApInt::all_ones(1)), "-1");
    assert_eq!(format!("{}", ApInt::all_ones(32)), "-1");
    assert_eq!(format!("{}", ApInt::all_ones(129)), "-1");

    // Wide values print in full precision, not truncated to a machine word.
    let wide = ApInt::from_string(257, "340282366920938463463374607431768211456", 10)?;
    assert_eq!(format!("{wide}"), "340282366920938463463374607431768211456");
    assert_eq!(
        format!("{}", wide.negate()),
        "-340282366920938463463374607431768211456"
    );

    // Display is exactly `to_string_radix(10, Signed)` -- one source of truth.
    let v = ApInt::from_string(64, "12345", 10)?;
    assert_eq!(
        format!("{v}"),
        v.to_string_radix(10, ApIntSignedness::Signed)
    );
    assert_eq!(format!("{v}"), "12345");

    Ok(())
}

/// The assembly writer's integer-constant literal and `ApInt`'s `Display` are
/// the same rendering, so a constant's printed body matches the `ApInt` form.
#[test]
fn ap_int_display_matches_constant_literal() -> Result<(), IrError> {
    Module::with_new("apint_agreement", |m| {
        let i32_ty = m.i32_type();
        let printed = format!("{}", i32_ty.const_int(-12345_i32));
        let magnitude = ApInt::from_string(32, "12345", 10)?.negate();

        assert_eq!(printed, format!("i32 {magnitude}"));
        Ok(())
    })
}
