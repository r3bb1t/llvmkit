//! `remove_incoming` — the phi edge-removal mutator (slice B1g).
//!
//! Mirrors `PHINode::removeIncomingValue` (`lib/IR/Instructions.cpp`), which
//! CFG rewriters call when a predecessor edge disappears. Two upstream
//! behaviours are pinned here:
//!
//! * the vacated slot is **backfilled from the end** of the incoming list
//!   (upstream "swap with the end of the list, nuke the last"), so incoming
//!   order is not preserved; and
//! * the reverse use-list loses exactly one edge per removal, so RAUW
//!   bookkeeping stays correct.
//!
//! One behaviour is a **deliberate divergence**: upstream's default
//! `DeletePHIIfEmpty = true` destroys a phi that loses its last incoming.
//! llvmkit erases through `Instruction::erase_from_parent`, which *consumes*
//! the linear lifecycle handle so use-after-erase is a compile error, and a
//! `Copy` opcode handle cannot express that consumption — so `remove_incoming`
//! is upstream's `DeletePHIIfEmpty = false` mode and never self-erases. The
//! auto-erase behaviour ships where it can be sound, on the `ReshapeCfg` edge
//! edits (see `zero_incoming_phi`). `Module::verify` is the backstop, and the
//! last two tests pin both halves of that.
//!
//! These cases live in-crate because they build phis through the raw
//! `build_*_phi` builders, which are crate-internal (block arguments are the
//! public phi-authoring surface).

use crate::{Dyn, InstructionKind, InstructionView, IrBuilder, IrError, Linkage, VerifierRule};

/// Three predecessors, three incomings; removing index 0 backfills it from the
/// **end**, exactly as upstream's `removeIncomingValue` does. The printed phi
/// is therefore `[ 3, %c ], [ 2, %b ]` — not the order-preserving
/// `[ 2, %b ], [ 3, %c ]` — and the returned value is the one that left.
#[test]
fn remove_incoming_backfills_from_the_end_like_upstream() -> Result<(), IrError> {
    let m = crate::module_new!("phi_remove_swap")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a = m.view(f).append_basic_block(&m, "a");
    let b_bb = m.view(f).append_basic_block(&m, "b");
    let c = m.view(f).append_basic_block(&m, "c");
    let join = m.view(f).append_basic_block(&m, "join");
    let (a_lbl, b_lbl, c_lbl, join_lbl) = (a.id(), b_bb.id(), c.id(), join.id());

    for pred in [a, b_bb, c] {
        IrBuilder::new_for::<Dyn>(&m)
            .position_at_end(pred)
            .br(join_lbl)?;
    }

    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = bld
        .view(bld.int_phi::<i32, _>("p")?)
        .add_incoming(1_i32, a_lbl)?
        .add_incoming(2_i32, b_lbl)?
        .add_incoming(3_i32, c_lbl)?;
    bld.ret(phi.as_int_value())?;

    // Drop `[ 1, %a ]`, the entry at index 0.
    let removed = phi.remove_incoming(&m, 0)?;
    assert_eq!(removed, i32_ty.const_int(1_i32).as_erased());
    assert_eq!(phi.incoming_count(), 2);

    // Upstream backfills from the tail: `[ 3, %c ]` now sits at index 0.
    let text = format!("{m}");
    assert!(
        text.contains("%p = phi i32 [ 3, %c ], [ 2, %b ]"),
        "removal must backfill from the end, not shift; got:\n{text}"
    );
    Ok(())
}

/// Removing an incoming deregisters exactly one phi-use of the removed value —
/// one edge per incoming was registered on the way in, so a value that is
/// incoming on *two* edges keeps the other one. Without this, RAUW would later
/// try to rewrite an operand slot the phi no longer has.
#[test]
fn remove_incoming_deregisters_one_use_of_the_removed_value() -> Result<(), IrError> {
    let m = crate::module_new!("phi_remove_uses")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a = m.view(f).append_basic_block(&m, "a");
    let join = m.view(f).append_basic_block(&m, "join");
    let (a_lbl, join_lbl) = (a.id(), join.id());
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(a)
        .br(join_lbl)?;

    // `7` interns to one constant, so both edges from `%a` are the same
    // SSA value: two use-list entries for one value.
    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = bld
        .view(bld.int_phi::<i32, _>("p")?)
        .add_incoming(7_i32, a_lbl)?
        .add_incoming(7_i32, a_lbl)?;
    let seven = i32_ty.const_int(7_i32).as_erased();
    assert_eq!(seven.num_uses(), 2);

    phi.remove_incoming(&m, 0)?;
    assert_eq!(
        seven.num_uses(),
        1,
        "exactly one use-list edge leaves per removed incoming"
    );
    Ok(())
}

/// An index past the end is rejected rather than panicking — upstream asserts,
/// llvmkit returns the same `ArgumentIndexOutOfRange` the indexed `incoming`
/// reader returns.
#[test]
fn remove_incoming_rejects_an_out_of_range_index() -> Result<(), IrError> {
    let m = crate::module_new!("phi_remove_oob")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a = m.view(f).append_basic_block(&m, "a");
    let join = m.view(f).append_basic_block(&m, "join");
    let (a_lbl, join_lbl) = (a.id(), join.id());
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(a)
        .br(join_lbl)?;

    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = bld
        .view(bld.int_phi::<i32, _>("p")?)
        .add_incoming(1_i32, a_lbl)?;

    let err = phi.remove_incoming(&m, 1).expect_err("index 1 of 1 is oob");
    assert!(
        matches!(err, IrError::ArgumentIndexOutOfRange { index: 1, count: 1 }),
        "got {err:?}"
    );
    // An empty phi rejects index 0 the same way.
    phi.remove_incoming(&m, 0)?;
    assert!(matches!(
        phi.remove_incoming(&m, 0),
        Err(IrError::ArgumentIndexOutOfRange { index: 0, count: 0 })
    ));
    Ok(())
}

/// `remove_incoming` performs no CFG bookkeeping of its own: dropping one
/// incoming from a two-predecessor phi leaves the phi one edge short, and the
/// verifier's existing incoming-count-vs-predecessor rule
/// ([`VerifierRule::PhiPredecessorMismatch`]) is what flags it. That is the
/// backstop for the non-deleting empty-phi contract below.
#[test]
fn remove_incoming_leaves_the_verifier_to_flag_the_missing_edge() -> Result<(), IrError> {
    let m = crate::module_new!("phi_remove_verify")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a = m.view(f).append_basic_block(&m, "a");
    let b_bb = m.view(f).append_basic_block(&m, "b");
    let join = m.view(f).append_basic_block(&m, "join");
    let (a_lbl, b_lbl, join_lbl) = (a.id(), b_bb.id(), join.id());
    for pred in [a, b_bb] {
        IrBuilder::new_for::<Dyn>(&m)
            .position_at_end(pred)
            .br(join_lbl)?;
    }

    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = bld
        .view(bld.int_phi::<i32, _>("p")?)
        .add_incoming(1_i32, a_lbl)?
        .add_incoming(2_i32, b_lbl)?;
    bld.ret(phi.as_int_value())?;
    m.verify_borrowed()?;

    phi.remove_incoming(&m, 1)?;
    let err = m
        .verify_borrowed()
        .expect_err("one incoming for two predecessors must not verify");
    assert!(
        format!("{err:?}").contains(&format!("{:?}", VerifierRule::PhiPredecessorMismatch)),
        "expected the phi predecessor rule; got {err:?}"
    );
    Ok(())
}

/// **Divergence from upstream**, pinned: `PHINode::removeIncomingValue`'s
/// default `DeletePHIIfEmpty = true` destroys a phi that loses its last
/// incoming. llvmkit's `remove_incoming` is the `DeletePHIIfEmpty = false`
/// mode — it never self-erases, because erasure consumes the linear
/// `Instruction` handle and a `Copy` opcode handle cannot express that. The
/// emptied phi stays in the block and the caller owns finishing the job.
#[test]
fn remove_incoming_never_deletes_an_emptied_phi() -> Result<(), IrError> {
    let m = crate::module_new!("phi_remove_empty")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a = m.view(f).append_basic_block(&m, "a");
    let join = m.view(f).append_basic_block(&m, "join");
    let (a_lbl, join_lbl) = (a.id(), join.id());
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(a)
        .br(join_lbl)?;

    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = bld
        .view(bld.int_phi::<i32, _>("p")?)
        .add_incoming(1_i32, a_lbl)?;
    bld.ret(phi.as_int_value())?;

    phi.remove_incoming(&m, 0)?;
    assert_eq!(phi.incoming_count(), 0);
    // Still attached: the phi is rediscoverable in its block, unlike
    // upstream's `DeletePHIIfEmpty` path which would have erased it.
    let still_there = m
        .view(f)
        .basic_blocks()
        .find(|bb| bb.name().as_deref() == Some("join"))
        .expect("join block")
        .instructions()
        .any(|inst| matches!(inst.kind(), Some(InstructionKind::Phi(_))));
    assert!(still_there, "an emptied phi must not be self-erased");
    Ok(())
}

/// The removal reaches every phi flavour through the variant-independent
/// `PhiKind` rediscovery surface — the shape a pass walking a block actually
/// has. Covers the float, pointer and erased (`Other`) handles beside the
/// integer one the tests above exercise directly.
#[test]
fn remove_incoming_through_phi_kind_covers_every_flavour() -> Result<(), IrError> {
    let m = crate::module_new!("phi_remove_kinds")?;
    let i32_ty = m.i32_type();
    let f64_ty = m.f64_type();
    let ptr_ty = m.ptr_type(0);
    let vec_ty = m.vector_type(i32_ty, 2, false);
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a = m.view(f).append_basic_block(&m, "a");
    let join = m.view(f).append_basic_block(&m, "join");
    let (a_lbl, join_lbl) = (a.id(), join.id());
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(a)
        .br(join_lbl)?;

    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let fp = bld.fp_phi_dyn(f64_ty.as_dyn(), "fp")?;
    let pp = bld.pointer_phi_in_addrspace(ptr_ty, "pp")?;
    let vp = bld.phi_dyn(vec_ty.as_type(), "vp")?;
    for (phi, ty) in [
        (bld.view(fp).to_erased(), f64_ty.as_type()),
        (bld.view(pp).to_erased(), ptr_ty.as_type()),
        (bld.view(vp).to_erased(), vec_ty.as_type()),
    ] {
        bld.phi_add_incoming_from_value(phi, ty.poison(), a_lbl)?;
    }

    // Rediscover each phi and remove its single incoming through `PhiKind`.
    for phi in [
        bld.view(fp).to_erased(),
        bld.view(pp).to_erased(),
        bld.view(vp).to_erased(),
    ] {
        let Some(InstructionKind::Phi(kind)) = InstructionView::try_from(phi)?.kind() else {
            panic!("expected the phi to rediscover as InstructionKind::Phi");
        };
        assert_eq!(kind.incoming_count(), 1);
        kind.remove_incoming(&m, 0)?;
        assert_eq!(kind.incoming_count(), 0);
    }
    Ok(())
}
