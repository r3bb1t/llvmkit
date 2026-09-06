//! `IrError::TypeIdentityMismatch` and the `RenderedType` it carries.
//!
//! No upstream counterpart. LLVM's diagnostics interpolate a `Type *` into a
//! `Twine` and print it at render time (`llvm/lib/IR/Verifier.cpp`'s `Check`,
//! `llvm/lib/AsmParser/LLParser.cpp`'s `error`), so upstream never faces the
//! question these tests pin: llvmkit's errors are values that outlive the
//! module borrow, so a type must be captured eagerly, and the shape of that
//! capture is llvmkit's own choice.
//!
//! ## The defect these exist for
//!
//! Twenty production sites compared two *runtime* types and reported two
//! `TypeKindLabel`s. Two types of one kind share a label, so those sites
//! rendered "type mismatch: expected struct, got struct" — a sentence naming
//! no fact about either operand, which reads the same however the operands
//! are swapped. Two were pinned by *passing* assertions in `struct_schema.rs`,
//! which is why the class survived: an oracle asserting the contentless answer
//! is satisfied by any wrong answer of the same shape.
//!
//! The sweep below is the check that was missing. It asserts on the rendered
//! text rather than the fields, because the rendered text is what the defect
//! produced.

use llvmkit_ir::{Dyn, IntDyn, IntValue, IrBuilder, IrError, Linkage, TypeKindLabel, module_new};

/// `Type::rendered` is a projection of one type, so its two halves cannot
/// disagree.
///
/// `RenderedType`'s fields are private for exactly this reason: a public pair
/// would admit `{ kind: Integer, spelling: "float" }`, which no `Type` can
/// produce. Pinned over one type of every shape the diagnostics reach.
#[test]
fn rendered_type_agrees_with_the_type_it_came_from() -> Result<(), IrError> {
    let m = module_new!("rendered")?;
    let i32_ty = m.i32_type().as_type();
    let types = [
        m.void_type().as_type(),
        i32_ty,
        m.f64_type().as_type(),
        m.ptr_type(0).as_type(),
        m.array_type_n::<i32, 4>().as_type(),
        m.vector_type_n::<i32, 4>().as_type(),
        m.struct_type([i32_ty, i32_ty]).as_type(),
    ];

    for ty in types {
        let rendered = ty.rendered();
        assert_eq!(rendered.kind(), ty.kind_label());
        assert_eq!(rendered.spelling(), ty.to_string());
        assert_eq!(rendered.to_string(), ty.to_string());
    }
    Ok(())
}

/// Every identity mismatch names both sides distinctly.
///
/// Each case is a different production site, and each pairs two types that
/// share a `TypeKindLabel` — the only configuration in which the old shape
/// went contentless. A site that regresses to reporting kinds fails here,
/// because both sides would then render the same word.
#[test]
fn same_kind_operands_still_render_distinctly() -> Result<(), IrError> {
    let m = module_new!("identity")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();

    let mut rejections: Vec<IrError> = Vec::new();

    let struct_i32 = m.struct_type([i32_ty.as_type()]);
    let struct_i64 = m.struct_type([i64_ty.as_type()]);

    // `StructType::const_struct` — a field constant against the struct's own
    // field type. Both are literal structs, so both label `struct`.
    let outer = m.struct_type([struct_i32.as_type()]);
    let wrong_field = struct_i64.const_struct([i64_ty.const_int(0i64)])?;
    rejections.push(
        outer
            .const_struct([wrong_field])
            .expect_err("a field of the wrong struct type is rejected"),
    );

    // `GlobalVariable::set_initializer` — initializer against the global's
    // value type. Two literal structs again.
    let global = m.global_builder("g", struct_i32.as_type()).build()?;
    let wrong_body = struct_i64.const_struct([i64_ty.const_int(0i64)])?;
    rejections.push(
        m.view(global)
            .set_initializer(&m, wrong_body)
            .expect_err("an initializer of the wrong struct type is rejected"),
    );

    // `IrBuilder::select_erased` — the two arms must agree. Two integer
    // widths, so both label `integer`.
    let fn_ty = m.function_type_no_parameters(m.void_type());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    rejections.push(
        b.select_erased(
            m.bool_type().const_int(true).as_erased(),
            i32_ty.const_int(1i32).as_erased(),
            i64_ty.const_int(2i64).as_erased(),
            "arms",
        )
        .expect_err("select arms of different widths are rejected"),
    );

    for rejection in &rejections {
        let IrError::TypeIdentityMismatch { expected, got } = rejection else {
            panic!("expected a TypeIdentityMismatch, got {rejection:?}");
        };
        assert_eq!(
            expected.kind(),
            got.kind(),
            "this case is only interesting while both sides share a kind"
        );
        assert_ne!(
            expected.spelling(),
            got.spelling(),
            "a mismatch rendering the same text on both sides names no fact \
             about either operand: {rejection}"
        );
    }
    Ok(())
}

/// Refusing to set a literal struct's body is not a type mismatch.
///
/// The same audit found this reporting `TypeMismatch { expected: Struct, got:
/// Struct }` — both sides *literals*, so it rendered "type mismatch: expected
/// struct, got struct" unconditionally, for a refusal where both operands are
/// indeed structs and the fault is that a literal struct has no body to set.
/// `StructType::setBodyOrError` (`llvm/lib/IR/Type.cpp`) states the same
/// contract as `assert(isOpaque() && ...)`; a literal struct is never opaque.
///
/// No upstream counterpart: upstream asserts, so there is no value to port.
#[test]
fn a_literal_struct_has_no_settable_body() -> Result<(), IrError> {
    let m = module_new!("literal")?;
    let i32_ty = m.i32_type();
    // `struct_type` interns a *literal* struct — its body is its identity.
    let literal = m.struct_type([i32_ty.as_type()]);

    let error = m
        .set_struct_body_dyn(literal, [i32_ty.as_type()], false)
        .expect_err("a literal struct's body cannot be set");
    assert_eq!(error, IrError::LiteralStructBodyNotSettable);
    assert_eq!(
        error.to_string(),
        "a literal struct type has no settable body"
    );
    Ok(())
}

/// `TypeMismatch` keeps the job it is right for.
///
/// The split is by question asked, not by call site. Where the expectation is
/// fixed at the call site — "this must be an integer" — the guard *is* the
/// kind, so a `TypeKindLabel` is the whole answer and no same-word rendering
/// is reachable. `IntValue::try_from` on a `double` is that shape.
#[test]
fn a_kind_expectation_still_reports_a_kind() -> Result<(), IrError> {
    let m = module_new!("kinds")?;
    let float = m.f64_type().const_double(1.0).as_erased();
    let error = IntValue::<IntDyn, _>::try_from(float).expect_err("a double is not an integer");
    assert_eq!(
        error,
        IrError::TypeMismatch {
            expected: TypeKindLabel::Integer,
            got: TypeKindLabel::Double,
        }
    );
    Ok(())
}
