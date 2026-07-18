//! Regression for the degenerate `ReturnTypeMismatch` diagnostic (Cycle A,
//! slice A1). The three return-marker checks in `module.rs` used to build the
//! error with `expected` and `got` set to the *same expression* (the
//! signature's own return-type label), so the two fields were always equal and
//! the diagnostic told the caller nothing. The fix derives `expected` from the
//! demanded marker `R` (`marker_kind_label::<R>()`) while `got` stays the
//! signature's actual return kind, so a genuine kind mismatch now reports two
//! distinct labels.
//!
//! This exercises the durable `function_by_name_typed` lookup path (the one
//! that survives the later typed-first `add_function` rework), so it is not a
//! throwaway.

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
