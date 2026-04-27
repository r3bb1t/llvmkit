//! Smoke test for [`crate::Width<N>`] -- the const-generic marker for
//! arbitrary integer widths.
//!
//! ## Upstream provenance
//!
//! Ports the constructive subset of
//! `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateStepVectorI3)`,
//! specifically the `IntegerType::get(Ctx, 3)` line. The full
//! `stepvector` round-trip lands with the vector / intrinsic phase.
//! Per-test citations below.

use llvmkit_ir::{Module, Width};

/// Mirrors the constructive line `Type *I3 = IntegerType::get(Ctx, 3);`
/// from `IRBuilderTest::CreateStepVectorI3`. Verifies our
/// const-generic constructor produces a typed handle whose runtime
/// width matches the const-generic parameter and whose AsmWriter
/// print form is the canonical `i3`.
#[test]
fn int_type_n_constructor_matches_upstream_int3() {
    let m = Module::new("c");
    let i3_ty = m.int_type_n::<3>();
    assert_eq!(i3_ty.bit_width(), 3);
    // LangRef: integer types print as `i<N>`. Our AsmWriter must
    // match.
    assert_eq!(format!("{}", i3_ty.as_type()), "i3");
}

/// Type-level: `Width<N>` participates in the same trait surface as
/// the Rust-scalar markers. This test just witnesses that
/// [`crate::Module::add_function::<Width<N>>`] compiles, mirroring
/// the upstream pattern of using arbitrary-width types in function
/// signatures (used pervasively in `CodeGen/` and `Transforms/`
/// tests, e.g. `srem-seteq-illegal-types.ll`).
#[test]
fn width_marker_works_as_return_marker() -> Result<(), llvmkit_ir::IrError> {
    let m = Module::new("c");
    let i17_ty = m.int_type_n::<17>();
    let fn_ty = m.fn_type(i17_ty, [i17_ty.as_type()], false);
    let _f = m.add_function::<Width<17>>("identity17", fn_ty, llvmkit_ir::Linkage::External)?;
    Ok(())
}
