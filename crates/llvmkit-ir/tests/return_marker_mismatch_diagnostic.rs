//! Regression for the degenerate `ReturnTypeMismatch` diagnostic (Cycle A,
//! slice A1). The return-marker checks in `module.rs` used to build the
//! error with `expected` and `got` set to the *same expression* (the
//! signature's own return-type label), so the two fields were always equal and
//! the diagnostic told the caller nothing. The fix derives `expected` from the
//! demanded marker `R` (`marker_kind_label::<R>()`) while `got` stays the
//! signature's actual return kind, so a genuine kind mismatch now reports two
//! distinct labels.
//!
//! This exercises the durable `function_by_name_typed` lookup path (the one
//! that survives the later typed-first `add_function` rework), so it is not a
//! throwaway. The second test covers the OTHER surviving runtime marker gate:
//! `FunctionBuilder::build` — after the cycle-B strict cut, the one public
//! constructor where a user-supplied `FunctionType` and an independently
//! chosen `R` still meet (`add_typed_function` derives its signature from the
//! markers, so it cannot mismatch by construction).

use llvmkit_ir::{IrError, Linkage, Module, Ptr, TypeKindLabel};

#[test]
fn return_marker_mismatch_reports_distinct_expected_and_got() -> Result<(), IrError> {
    Module::with_new("m", |m| {
        // A function that genuinely returns `i32`.
        m.add_typed_function::<i32, (), _>("f", Linkage::External)?;

        // Look it up demanding a *pointer* return marker: a real kind mismatch,
        // so the two diagnostic fields must differ.
        let err = m
            .function_by_name_typed::<Ptr>("f")
            .expect_err("an i32 function looked up as `Ptr` must mismatch");

        match err {
            IrError::ReturnTypeMismatch { expected, got } => {
                assert_ne!(
                    expected, got,
                    "expected and got must differ — the degenerate same-expression bug"
                );
                assert_eq!(expected, TypeKindLabel::Pointer, "demanded marker R = Ptr");
                assert_eq!(
                    got,
                    TypeKindLabel::Integer,
                    "signature actually returns i32"
                );
            }
            other => panic!("expected ReturnTypeMismatch, got {other:?}"),
        }

        Ok(())
    })
}

/// `FunctionBuilder::build` keeps the `signature_matches_marker` gate: an
/// `i32` marker asserted over a `void`-returning user-supplied signature is
/// rejected at build time with distinct `expected`/`got` labels. This is the
/// permanent lock on the last runtime marker check on a declaration path
/// (`ModuleCore::add_function_checked`).
#[test]
fn function_builder_rejects_mismatched_return_marker() -> Result<(), IrError> {
    Module::with_new("m", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let err = m
            .function_builder::<i32, _>("bad", fn_ty)
            .build()
            .expect_err("i32 marker over a void signature must mismatch");

        match err {
            IrError::ReturnTypeMismatch { expected, got } => {
                assert_eq!(expected, TypeKindLabel::Integer, "demanded marker R = i32");
                assert_eq!(got, TypeKindLabel::Void, "signature actually returns void");
            }
            other => panic!("expected ReturnTypeMismatch, got {other:?}"),
        }

        Ok(())
    })
}
