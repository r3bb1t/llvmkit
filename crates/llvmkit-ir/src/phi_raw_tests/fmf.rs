//! Relocated raw-phi mechanics that cannot be expressed through the
//! block-argument authoring surface: the untyped
//! `phi_add_incoming_from_value` path, malformed-by-design incomings, and
//! the raw duplicate-incoming guards. Ported verbatim from
//! `tests/builder_fmf_and_phi.rs`; dormant until wired into the crate's
//! `#[cfg(test)]` tree.

use crate::{Dyn, IRBuilder, IrError, Linkage, Module, PointerValue};

// --- Every edge-adding path checked (type + ambiguous duplicate) -------

/// The untyped add path used by the parser and ssa_builder must reject a
/// type-mismatched incoming at the call site, not at `verify()`. An `f64`
/// value handed to an `i32` phi is `IrError::TypeMismatch`, mirroring the
/// rule the typed `PhiInst::add_incoming` already enforces.
#[test]
fn phi_add_incoming_from_value_rejects_type_mismatch() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let phi = b.build_int_phi::<i32, _>("p")?;
        let phi_val = phi.as_int_value().into_erased();
        // f64 incoming value against an i32 phi -> result-type mismatch.
        let f64_val = f64_ty.const_double(1.0).into_erased();
        let block = f.basic_blocks().next().expect("entry block handle");
        let raw = IRBuilder::new(&m);
        let err = raw
            .phi_add_incoming_from_value(phi_val, f64_val, block)
            .unwrap_err();
        assert!(
            matches!(err, IrError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}"
        );
        Ok(())
    })
}

/// Same predecessor twice with DIFFERENT values is always meaningless
/// (the InstCombine #196954 bug class) — rejected at add time on the
/// untyped path with `IrError::AmbiguousPhiIncoming`.
#[test]
fn phi_add_incoming_from_value_rejects_differing_duplicate() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let _a = f.append_basic_block(&m, "a");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let phi = b.build_int_phi::<i32, _>("p")?;
        let phi_val = phi.as_int_value().into_erased();
        let c1 = i32_ty.const_int(1_i32).into_erased();
        let c2 = i32_ty.const_int(2_i32).into_erased();
        let raw = IRBuilder::new(&m);
        // First edge from block `a` (index 1) is accepted.
        let block_a = f.basic_blocks().nth(1).expect("block a handle");
        raw.phi_add_incoming_from_value(phi_val, c1, block_a)?;
        // A second edge from the SAME block with a DIFFERENT value is
        // rejected at the call site, not deferred to verify().
        let block_a2 = f.basic_blocks().nth(1).expect("block a handle again");
        let err = raw
            .phi_add_incoming_from_value(phi_val, c2, block_a2)
            .unwrap_err();
        assert!(
            matches!(err, IrError::AmbiguousPhiIncoming { .. }),
            "expected AmbiguousPhiIncoming, got {err:?}"
        );
        Ok(())
    })
}

/// ...and on the typed path: `phi.add_incoming(c1, a)?.add_incoming(c2, a)`
/// with `c2 != c1` is `IrError::AmbiguousPhiIncoming`.
#[test]
fn typed_add_incoming_rejects_differing_duplicate() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let a = f.append_basic_block(&m, "a");
        let a_label = a.label();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let phi = b.build_int_phi::<i32, _>("p")?;
        let err = phi
            .add_incoming(1_i32, a_label)?
            .add_incoming(2_i32, a_label)
            .unwrap_err();
        assert!(
            matches!(err, IrError::AmbiguousPhiIncoming { .. }),
            "expected AmbiguousPhiIncoming, got {err:?}"
        );
        Ok(())
    })
}

/// The fp phi path enforces the same rule as the int path:
/// `phi.add_incoming(c1, a)?.add_incoming(c2, a)` with `c2 != c1` is
/// `IrError::AmbiguousPhiIncoming`. Discriminates the differing-duplicate
/// guard in `FpPhiInst::add_incoming` (deleting that guard makes this fail).
#[test]
fn fp_phi_add_incoming_rejects_differing_duplicate() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(f64_ty, [f64_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let a = f.append_basic_block(&m, "a");
        let a_label = a.label();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let phi = b.build_fp_phi::<f64, _>("p")?;
        // `1.0_f64` and `2.0_f64` intern to distinct constants, so the two
        // edges from block `a` carry different values: the guard fires.
        let err = phi
            .add_incoming(1.0_f64, a_label)?
            .add_incoming(2.0_f64, a_label)
            .unwrap_err();
        assert!(
            matches!(err, IrError::AmbiguousPhiIncoming { .. }),
            "expected AmbiguousPhiIncoming, got {err:?}"
        );
        Ok(())
    })
}

/// The pointer phi path enforces the same rule as the int/fp paths:
/// `phi.add_incoming(p1, a)?.add_incoming(p2, a)` with `p2 != p1` is
/// `IrError::AmbiguousPhiIncoming`. Discriminates the differing-duplicate
/// guard in `PointerPhiInst::add_incoming` (deleting that guard makes this
/// fail). Two distinct pointer params supply the two different SSA values
/// (there is no second distinct pointer constant to use).
#[test]
fn pointer_phi_add_incoming_rejects_differing_duplicate() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(ptr_ty, [ptr_ty.as_type(), ptr_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let a = f.append_basic_block(&m, "a");
        let a_label = a.label();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let phi = b.build_pointer_phi("p")?;
        let p1: PointerValue = f.param(0)?.try_into()?;
        let p2: PointerValue = f.param(1)?.try_into()?;
        // Two distinct params are two distinct SSA values, so the two edges
        // from block `a` differ: the guard fires.
        let err = phi
            .add_incoming(p1, a_label)?
            .add_incoming(p2, a_label)
            .unwrap_err();
        assert!(
            matches!(err, IrError::AmbiguousPhiIncoming { .. }),
            "expected AmbiguousPhiIncoming, got {err:?}"
        );
        Ok(())
    })
}

/// Same predecessor twice with the SAME value stays legal — a switch with
/// two cases to one successor produces exactly this shape. Pins the
/// multi-edge exception against over-rejection by the duplicate check.
#[test]
fn same_value_duplicate_incoming_is_legal() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let a = f.append_basic_block(&m, "a");
        let a_label = a.label();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let phi = b.build_int_phi::<i32, _>("p")?;
        // `7_i32` interns to one constant id, so both edges carry the same
        // value from the same block: both accepted.
        let phi = phi
            .add_incoming(7_i32, a_label)?
            .add_incoming(7_i32, a_label)?;
        assert_eq!(phi.incoming_count(), 2);
        Ok(())
    })
}
