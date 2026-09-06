//! `FloatType::const_ap_float`'s semantics guard, and the type-kind bijection
//! the guard's diagnostic is built from.
//!
//! No upstream counterpart: `ConstantFP::get(Type *Ty, const APFloat &V)`
//! (`llvm/lib/IR/Constants.cpp`) states this contract as
//! `assert(&V.getSemantics() == &Ty->getFltSemantics())`, so upstream has no
//! test for the rejection — an assert has no observable value to assert on.
//! llvmkit hardens the assert into an `IrError` (the no-runtime-panics rule),
//! which gives the outcome a value, and these pin it.
//!
//! Before this file the guard had no coverage at all
//! (`rg -n "const_ap_float" crates/llvmkit-ir/tests` at `ffc5895` found only
//! success-path callers), which is how its `got` field came to be the literal
//! `TypeKindLabel::Double` regardless of the value handed in.

use llvmkit_ir::{ApFloat, ApFloatSemantics, ApFloatSign, IrError, TypeKindLabel, module_new};

/// Every modeled float kind's `semantics()` round-trips back to that kind's
/// own `kind_label()` through `From<ApFloatSemantics> for TypeKindLabel`.
///
/// This is what makes `const_ap_float`'s diagnostic incapable of rendering a
/// diagonal: both sides of the `TypeMismatch` are routed through inverse
/// halves of one bijection, so `expected == got` requires
/// `value.semantics() == self.semantics()`, which the guard has already
/// excluded.
///
/// No upstream counterpart — upstream compares `fltSemantics` pointers and
/// has no `TypeKindLabel` to be in bijection with.
#[test]
fn ap_float_semantics_and_type_kind_labels_are_inverse() -> Result<(), IrError> {
    let m = module_new!("float_semantics")?;

    assert_eq!(m.half_type().semantics(), ApFloatSemantics::IeeeHalf);
    assert_eq!(
        TypeKindLabel::from(m.half_type().semantics()),
        m.half_type().as_type().kind_label()
    );

    assert_eq!(m.bfloat_type().semantics(), ApFloatSemantics::Bfloat);
    assert_eq!(
        TypeKindLabel::from(m.bfloat_type().semantics()),
        m.bfloat_type().as_type().kind_label()
    );

    assert_eq!(m.f32_type().semantics(), ApFloatSemantics::IeeeSingle);
    assert_eq!(
        TypeKindLabel::from(m.f32_type().semantics()),
        m.f32_type().as_type().kind_label()
    );

    assert_eq!(m.f64_type().semantics(), ApFloatSemantics::IeeeDouble);
    assert_eq!(
        TypeKindLabel::from(m.f64_type().semantics()),
        m.f64_type().as_type().kind_label()
    );

    assert_eq!(m.fp128_type().semantics(), ApFloatSemantics::IeeeQuad);
    assert_eq!(
        TypeKindLabel::from(m.fp128_type().semantics()),
        m.fp128_type().as_type().kind_label()
    );

    assert_eq!(
        m.x86_fp80_type().semantics(),
        ApFloatSemantics::X87DoubleExtended
    );
    assert_eq!(
        TypeKindLabel::from(m.x86_fp80_type().semantics()),
        m.x86_fp80_type().as_type().kind_label()
    );

    assert_eq!(
        m.ppc_fp128_type().semantics(),
        ApFloatSemantics::PpcDoubleDouble
    );
    assert_eq!(
        TypeKindLabel::from(m.ppc_fp128_type().semantics()),
        m.ppc_fp128_type().as_type().kind_label()
    );

    Ok(())
}

/// `const_ap_float` names the semantics of the value it was handed.
///
/// The `f64` case is the one that regressed: the old `got` was the literal
/// `TypeKindLabel::Double`, so an `f64` target rendered "expected double, got
/// double" for *every* wrong-semantics value, and every other target rendered
/// "got double" whatever it actually received.
///
/// No upstream counterpart — see the module doc comment.
#[test]
fn const_ap_float_names_the_semantics_it_received() -> Result<(), IrError> {
    let m = module_new!("float_semantics")?;
    let quad = ApFloat::zero(ApFloatSemantics::IeeeQuad, ApFloatSign::Positive);

    match m.f32_type().const_ap_float(&quad) {
        Err(error) => {
            assert_eq!(
                error,
                IrError::TypeMismatch {
                    expected: TypeKindLabel::Float,
                    got: TypeKindLabel::Fp128,
                }
            );
            assert_eq!(
                error.to_string(),
                "type mismatch: expected float, got fp128"
            );
        }
        Ok(_) => panic!("an fp128 value is not a `float` constant"),
    }

    match m.f64_type().const_ap_float(&quad) {
        Err(error) => {
            assert_eq!(
                error,
                IrError::TypeMismatch {
                    expected: TypeKindLabel::Double,
                    got: TypeKindLabel::Fp128,
                }
            );
            assert_eq!(
                error.to_string(),
                "type mismatch: expected double, got fp128"
            );
        }
        Ok(_) => panic!("an fp128 value is not a `double` constant"),
    }

    Ok(())
}

/// No float type accepts a foreign-semantics value, and no rejection renders
/// the same word on both sides.
///
/// The second half is the property the literal `got` violated. It is asserted
/// on the rendered string rather than on the fields, because the rendered
/// string is what the defect produced and what a reader would have to
/// disbelieve.
///
/// No upstream counterpart — see the module doc comment.
#[test]
fn no_float_type_accepts_foreign_semantics() -> Result<(), IrError> {
    let m = module_new!("float_semantics")?;
    // Each target is paired with a value of a *different* modeled semantics.
    let single = ApFloat::zero(ApFloatSemantics::IeeeSingle, ApFloatSign::Positive);
    let quad = ApFloat::zero(ApFloatSemantics::IeeeQuad, ApFloatSign::Positive);

    let rejections = [
        m.half_type().const_ap_float(&single).err(),
        m.bfloat_type().const_ap_float(&single).err(),
        m.f32_type().const_ap_float(&quad).err(),
        m.f64_type().const_ap_float(&single).err(),
        m.fp128_type().const_ap_float(&single).err(),
        m.x86_fp80_type().const_ap_float(&single).err(),
        m.ppc_fp128_type().const_ap_float(&single).err(),
    ];

    for rejection in &rejections {
        let Some(IrError::TypeMismatch { expected, got }) = rejection else {
            panic!("foreign semantics must be rejected as a type mismatch, got {rejection:?}");
        };
        assert_ne!(
            expected.to_string(),
            got.to_string(),
            "a rejection that names the same kind on both sides says nothing"
        );
    }

    Ok(())
}
