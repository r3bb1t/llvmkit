//! Relocated raw-phi mechanics that cannot be expressed through the
//! block-argument authoring surface: the untyped
//! `phi_add_incoming_from_value` path, malformed-by-design incomings, and
//! the raw duplicate-incoming guards. Ported verbatim from
//! `tests/builder_fmf_and_phi.rs`; dormant until wired into the crate's
//! `#[cfg(test)]` tree.

use crate::{Dyn, IrBuilder, IrError, Linkage, PointerValue};

// --- Every edge-adding path checked (type + ambiguous duplicate) -------

/// The untyped add path used by the parser and ssa_builder must reject a
/// type-mismatched incoming at the call site, not at `verify()`. An `f64`
/// value handed to an `i32` phi is `IrError::TypeMismatch`, mirroring the
/// rule the typed `PhiInst::add_incoming` already enforces.
#[test]
fn phi_add_incoming_from_value_rejects_type_mismatch() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let i32_ty = m.i32_type();
    let f64_ty = m.f64_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b.view(b.int_phi::<i32, _>("p")?);
    let phi_val = phi.as_int_value().as_erased();
    // f64 incoming value against an i32 phi -> result-type mismatch.
    let f64_val = f64_ty.const_double(1.0).as_erased();
    let block = m.view(f).basic_blocks().next().expect("entry block handle");
    let raw = IrBuilder::new(&m);
    let err = raw
        .phi_add_incoming_from_value(phi_val, f64_val, block)
        .unwrap_err();
    assert!(
        matches!(err, IrError::TypeMismatch { .. }),
        "expected TypeMismatch, got {err:?}"
    );
    Ok(())
}

/// Same predecessor twice with DIFFERENT values is always meaningless
/// (the InstCombine #196954 bug class) — rejected at add time on the
/// untyped path with `IrError::AmbiguousPhiIncoming`.
#[test]
fn phi_add_incoming_from_value_rejects_differing_duplicate() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let _a = m.view(f).append_basic_block(&m, "a");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b.view(b.int_phi::<i32, _>("p")?);
    let phi_val = phi.as_int_value().as_erased();
    let c1 = i32_ty.const_int(1_i32).as_erased();
    let c2 = i32_ty.const_int(2_i32).as_erased();
    let raw = IrBuilder::new(&m);
    // First edge from block `a` (index 1) is accepted.
    let block_a = m.view(f).basic_blocks().nth(1).expect("block a handle");
    raw.phi_add_incoming_from_value(phi_val, c1, block_a)?;
    // A second edge from the SAME block with a DIFFERENT value is
    // rejected at the call site, not deferred to verify().
    let block_a2 = m
        .view(f)
        .basic_blocks()
        .nth(1)
        .expect("block a handle again");
    let err = raw
        .phi_add_incoming_from_value(phi_val, c2, block_a2)
        .unwrap_err();
    assert!(
        matches!(err, IrError::AmbiguousPhiIncoming { .. }),
        "expected AmbiguousPhiIncoming, got {err:?}"
    );
    Ok(())
}

/// ...and on the typed path: `phi.add_incoming(c1, a)?.add_incoming(c2, a)`
/// with `c2 != c1` is `IrError::AmbiguousPhiIncoming`.
#[test]
fn typed_add_incoming_rejects_differing_duplicate() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let a = m.view(f).append_basic_block(&m, "a");
    let a_label = a.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b.view(b.int_phi::<i32, _>("p")?);
    let err = phi
        .add_incoming(1_i32, a_label)?
        .add_incoming(2_i32, a_label)
        .unwrap_err();
    assert!(
        matches!(err, IrError::AmbiguousPhiIncoming { .. }),
        "expected AmbiguousPhiIncoming, got {err:?}"
    );
    Ok(())
}

/// The fp phi path enforces the same rule as the int path:
/// `phi.add_incoming(c1, a)?.add_incoming(c2, a)` with `c2 != c1` is
/// `IrError::AmbiguousPhiIncoming`. Discriminates the differing-duplicate
/// guard in `FpPhiInst::add_incoming` (deleting that guard makes this fail).
#[test]
fn fp_phi_add_incoming_rejects_differing_duplicate() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let f64_ty = m.f64_type();
    let fn_ty = m.function_type(f64_ty, [f64_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let a = m.view(f).append_basic_block(&m, "a");
    let a_label = a.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b.view(b.fp_phi::<f64, _>("p")?);
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
}

/// The pointer phi path enforces the same rule as the int/fp paths:
/// `phi.add_incoming(p1, a)?.add_incoming(p2, a)` with `p2 != p1` is
/// `IrError::AmbiguousPhiIncoming`. Discriminates the differing-duplicate
/// guard in `PointerPhiInst::add_incoming` (deleting that guard makes this
/// fail). Two distinct pointer params supply the two different SSA values
/// (there is no second distinct pointer constant to use).
#[test]
fn pointer_phi_add_incoming_rejects_differing_duplicate() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.function_type(ptr_ty, [ptr_ty.as_type(), ptr_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let a = m.view(f).append_basic_block(&m, "a");
    let a_label = a.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b.view(b.pointer_phi("p")?);
    let p1: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let p2: PointerValue<'_, _> = m.view(f).param(1)?.try_into()?;
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
}

/// Same predecessor twice with the SAME value stays legal — a switch with
/// two cases to one successor produces exactly this shape. Pins the
/// multi-edge exception against over-rejection by the duplicate check.
#[test]
fn same_value_duplicate_incoming_is_legal() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let a = m.view(f).append_basic_block(&m, "a");
    let a_label = a.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b.view(b.int_phi::<i32, _>("p")?);
    // `7_i32` interns to one constant id, so both edges carry the same
    // value from the same block: both accepted.
    let phi = phi
        .add_incoming(7_i32, a_label)?
        .add_incoming(7_i32, a_label)?;
    assert_eq!(phi.incoming_count(), 2);
    Ok(())
}

/// The block named in [`IrError::AmbiguousPhiIncoming`] is the one
/// `AsmWriter` prints, never an internal arena handle.
///
/// **No upstream counterpart as a test.** The message itself is llvmkit's own
/// — upstream defers this verdict to `Verifier::visitBasicBlock`'s `PHI node
/// has multiple entries for the same basic block with different incoming
/// values!`, which is the remaining half of `docs/divergences.md` entry 130.
/// What this pins *is* upstream's rule: `Verifier::CheckFailed` renders every
/// `Value` through `WriteAsOperand`, so an unnamed block is named by the
/// `SlotTracker` number `AsmWriter` would print for it, and a written name is
/// used verbatim. The fallback used to be `ValueSlot::arena_index`, an
/// internal handle that appears neither in the source nor in printed IR;
/// every other test on this error matched only the variant, which is why
/// nothing caught it. Asserting the rendered text is the guard.
#[test]
fn ambiguous_phi_names_the_block_asm_writer_prints() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    m.view(f).param(0)?.set_name(&m, "i");
    // An UNNAMED entry block: `SlotTracker::for_function` numbers it, and
    // with the one parameter named it takes slot 0 — the same `%0` the
    // vendored `test/Verifier/AmbiguousPhi.ll` writes for its own implicit
    // entry block.
    let entry = m.view(f).append_basic_block(&m, "");
    let entry_label = entry.id();
    let a = m.view(f).append_basic_block(&m, "a");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(a);
    let phi = b.view(b.int_phi::<i32, _>("p")?);
    let err = phi
        .add_incoming(1_i32, entry_label)?
        .add_incoming(2_i32, entry_label)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "phi already has an entry for block %0 with a different value"
    );
    // The number is `AsmWriter`'s, not a coincidence: the accepted first
    // edge prints its predecessor operand with the same `%0`. (The block's
    // own `; <label>:N` line is absent because `printBasicBlock` omits it
    // for a function's entry block, which is what this one is.)
    let printed = format!("{m}");
    assert!(printed.contains("[ 1, %0 ]"), "{printed}");
    Ok(())
}

/// The named-block arm of [`ambiguous_phi_names_the_block_asm_writer_prints`]:
/// a block with a written name is named by it, matching
/// `WriteAsOperand`'s `if (V->hasName())` branch.
///
/// **No upstream counterpart as a test**, for the same reason as its sibling.
#[test]
fn ambiguous_phi_names_a_named_block_by_its_name() -> Result<(), IrError> {
    let m = crate::module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let a = m.view(f).append_basic_block(&m, "a");
    let a_label = a.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b.view(b.int_phi::<i32, _>("p")?);
    let err = phi
        .add_incoming(1_i32, a_label)?
        .add_incoming(2_i32, a_label)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "phi already has an entry for block %a with a different value"
    );
    Ok(())
}
