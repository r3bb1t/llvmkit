//! `SsaBuilder` public-surface coverage: construction, `create_block`,
//! the `declare_*` variable family, `seal_block`, and (this session) the
//! full positioned/unpositioned lifecycle -- `switch_to_block`, `ins()`,
//! `current_block()`, `def_*_var`/`use_*_var`, the terminator family
//! (`br`/`cond_br`/`switch`/`ret`/`ret_void`/`unreachable`), and
//! `finish()`. See `crates/llvmkit-ir/src/ssa_builder.rs`'s
//! `#[cfg(test)] mod tests` for engine-level coverage the private
//! surface allows from inside the crate.
//!
//! ## Upstream provenance
//!
//! `SsaBuilder` is llvmkit-specific: LLVM's `IRBuilder` has no on-the-fly
//! SSA layer. The nearest functional relatives are `cranelift-frontend`'s
//! `FunctionBuilder` (construction ergonomics: `declare_var`/`create_block`)
//! and `llvm/lib/Transforms/Utils/SSAUpdater.cpp` (the completion
//! semantics: phi insertion driven by recorded CFG edges;
//! `SSAUpdater::GetValueInMiddleOfBlock`'s single-predecessor fast path is
//! the closest functional analogue to this file's `single_pred_read_emits_no_phi`).
//! Every test below is `llvmkit-specific` per `UPSTREAM.md`'s category
//! convention unless noted otherwise.

use llvmkit_ir::{IntPredicate, IntValue, IrError, Linkage, Module, NoFolder, SsaBuilder};
use proptest::prelude::*;

/// llvmkit-specific: locks `SsaBuilder::for_function`'s happy path --
/// construction succeeds against a function with no existing body.
#[test]
fn for_function_succeeds_on_empty_function() -> Result<(), IrError> {
    Module::with_new("ssa-construct", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let _b = SsaBuilder::for_function(&m, f)?;
        Ok(())
    })
}

/// llvmkit-specific: locks `SsaFunctionHasBlocks` -- the layer must
/// observe every CFG edge from birth, so grafting onto a function that
/// already has a body (even just an empty entry block) is rejected.
#[test]
fn for_function_rejects_function_with_existing_body() -> Result<(), IrError> {
    Module::with_new("ssa-construct-nonempty", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let _entry = f.append_basic_block(&m, "entry");
        match SsaBuilder::for_function(&m, f) {
            Err(IrError::SsaFunctionHasBlocks) => Ok(()),
            Ok(_) => panic!("expected SsaFunctionHasBlocks, got Ok"),
            Err(other) => panic!("expected SsaFunctionHasBlocks, got {other:?}"),
        }
    })
}

/// llvmkit-specific: locks `SsaBuilder::with_folder_for_function` against
/// a caller-supplied folder ([`NoFolder`]), mirroring the plain
/// `IRBuilder::with_folder` construction path this layer builds on top of.
#[test]
fn with_folder_for_function_accepts_custom_folder() -> Result<(), IrError> {
    Module::with_new("ssa-construct-folder", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let _b = SsaBuilder::with_folder_for_function(&m, f, NoFolder)?;
        Ok(())
    })
}

/// llvmkit-specific: `create_block`'s FIRST call names the entry block
/// and produces a real, appended, empty basic block. Mirrors
/// `Function::getEntryBlock` -- the first block a function gains is
/// always its entry per LLVM's IR model.
#[test]
fn create_block_appends_named_block_to_function() -> Result<(), IrError> {
    Module::with_new("ssa-create-block", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        assert_eq!(entry.label().as_value().name().as_deref(), Some("entry"));

        let second = b.create_block("second");
        assert_eq!(second.label().as_value().name().as_deref(), Some("second"));

        let entry_fn = f
            .entry_block()
            .expect("create_block's first call names the function's entry block");
        assert_eq!(entry_fn.name().as_deref(), Some("entry"));
        Ok(())
    })
}

/// llvmkit-specific: `seal_block` succeeds exactly once per block; a
/// second call on the same block is `SsaBlockAlreadySealed`.
#[test]
fn seal_block_succeeds_once_then_errors() -> Result<(), IrError> {
    Module::with_new("ssa-seal-once", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let _entry = b.create_block("entry");
        let second = b.create_block("second");
        b.seal_block(second)?;
        match b.seal_block(second) {
            Err(IrError::SsaBlockAlreadySealed { .. }) => Ok(()),
            Ok(()) => panic!("expected SsaBlockAlreadySealed, got Ok"),
            Err(other) => panic!("expected SsaBlockAlreadySealed, got {other:?}"),
        }
    })
}

/// llvmkit-specific: locks `SsaForeignBlock` -- a block handle produced
/// by one `SsaBuilder` cannot be sealed through a different builder,
/// even within the same module. `owner: SsaBuilderId` is the runtime
/// mechanism (a generative per-builder brand was rejected per the
/// module docs: it would force nested closures per function body).
#[test]
fn seal_block_rejects_block_from_different_builder() -> Result<(), IrError> {
    Module::with_new("ssa-foreign-block", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f1 = m.add_function::<(), _>("f1", fn_ty, Linkage::External)?;
        let f2 = m.add_function::<(), _>("f2", fn_ty, Linkage::External)?;

        let mut b1 = SsaBuilder::for_function(&m, f1)?;
        let _entry1 = b1.create_block("entry");
        let other1 = b1.create_block("other");

        let mut b2 = SsaBuilder::for_function(&m, f2)?;
        let _entry2 = b2.create_block("entry");

        match b2.seal_block(other1) {
            Err(IrError::SsaForeignBlock) => Ok(()),
            Ok(()) => panic!("expected SsaForeignBlock, got Ok"),
            Err(other) => panic!("expected SsaForeignBlock, got {other:?}"),
        }
    })
}

/// llvmkit-specific: locks the full `declare_*` family's return-handle
/// shape (strict + poison + dyn variants) across all three categories
/// (int/float/pointer), plus that each declared handle reports the
/// declaring builder as its owner and the right module.
#[test]
fn declare_var_family_covers_every_category_and_variant() -> Result<(), IrError> {
    Module::with_new("ssa-declare-all", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;

        let strict_int = b.declare_int_var::<i32, _>("i");
        let poison_int = b.declare_int_var_poison::<i64, _>("ip");
        let dyn_int_ty = m.custom_width_int_type(17)?;
        let dyn_int = b.declare_int_var_dyn(dyn_int_ty, "idyn");
        let dyn_int_poison = b.declare_int_var_dyn_poison(dyn_int_ty, "idynp");

        let strict_float = b.declare_float_var::<f32, _>("x");
        let poison_float = b.declare_float_var_poison::<f64, _>("xp");
        let dyn_float_ty = m.half_type().as_type().try_into().unwrap_or_else(|_| {
            unreachable!("half_type's erased Type is always a valid FloatType<FloatDyn>")
        });
        let dyn_float = b.declare_float_var_dyn(dyn_float_ty, "xdyn");
        let dyn_float_poison = b.declare_float_var_dyn_poison(dyn_float_ty, "xdynp");

        let strict_ptr = b.declare_pointer_var("p");
        let poison_ptr = b.declare_pointer_var_poison("pp");
        let addrspace_ty = m.ptr_type(1);
        let addrspace_ptr = b.declare_pointer_var_in_addrspace(addrspace_ty, "pas");
        let addrspace_ptr_poison =
            b.declare_pointer_var_in_addrspace_poison(addrspace_ty, "paspoison");

        let owner = b.id();
        assert_eq!(strict_int.owner(), owner);
        assert_eq!(poison_int.owner(), owner);
        assert_eq!(dyn_int.owner(), owner);
        assert_eq!(dyn_int_poison.owner(), owner);
        assert_eq!(strict_float.owner(), owner);
        assert_eq!(poison_float.owner(), owner);
        assert_eq!(dyn_float.owner(), owner);
        assert_eq!(dyn_float_poison.owner(), owner);
        assert_eq!(strict_ptr.owner(), owner);
        assert_eq!(poison_ptr.owner(), owner);
        assert_eq!(addrspace_ptr.owner(), owner);
        assert_eq!(addrspace_ptr_poison.owner(), owner);

        assert_eq!(strict_int.module().id(), m.id());
        assert_eq!(strict_ptr.module().id(), m.id());
        Ok(())
    })
}

/// llvmkit-specific: `SsaBlock::label()` is the escape hatch back to a
/// plain [`llvmkit_ir::BasicBlockLabel`] -- e.g. for feeding a branch
/// target built through the ordinary `IRBuilder` surface once the
/// public def/use/terminator API lands. Locks that the label survives
/// the round trip and names the same underlying block.
#[test]
fn ssa_block_label_round_trips_to_basic_block_label() -> Result<(), IrError> {
    Module::with_new("ssa-block-label", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let label = entry.label();
        assert_eq!(label.as_value().name().as_deref(), Some("entry"));
        assert_eq!(label.as_value().id(), entry.label().as_value().id());
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Positioned/unpositioned lifecycle: switch_to_block, ins(), def/use,
// terminators, finish
// --------------------------------------------------------------------------

/// Closest upstream functional reference: `SSAUpdater::GetValueInMiddleOfBlock`'s
/// single-predecessor fast path (no PHI insertion needed when a block's
/// unique predecessor already carries the definition). Entry defines `x`
/// and branches to a second block that reads it straight back: the read
/// resolves directly to the entry-block definition with no phi anywhere
/// in the function.
#[test]
fn single_pred_read_emits_no_phi() -> Result<(), IrError> {
    Module::with_new("ssa-single-pred", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let second = b.create_block("second");

        let x = b.declare_int_var::<i32, _>("x");

        let mut b = b.switch_to_block(entry)?;
        b.def_int_var(x, 7_i32)?;
        let b = b.br(second)?;

        let mut b = b.switch_to_block(second)?;
        b.seal_block(second)?;
        let read = b.use_int_var(x)?;
        let b = b.ret(read)?;
        b.finish()?;

        let text = format!("{m}");
        assert!(!text.contains("phi"), "expected no phi, got:\n{text}");
        assert!(
            text.contains("ret i32 7"),
            "expected the read to resolve to the entry def, got:\n{text}"
        );
        Ok(())
    })
}

/// Braun et al. 2013's central multi-predecessor example (Fig. 2/4): a
/// classic if/then/else diamond where both arms define the SAME
/// variable with DIFFERENT values. The join block's read must place
/// exactly one phi merging both arms' definitions -- not two phis, not
/// zero.
#[test]
fn diamond_merge_places_single_phi_at_join() -> Result<(), IrError> {
    Module::with_new("ssa-diamond", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let left = b.create_block("left");
        let right = b.create_block("right");
        let join = b.create_block("join");

        let x = b.declare_int_var::<i32, _>("x");

        let b = b.switch_to_block(entry)?;
        let n: IntValue<i32> = f.param(0)?.try_into()?;
        let cond = b
            .ins()
            .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "cond")?;
        let mut b = b.cond_br(cond, left, right)?;
        b.seal_block(left)?;
        b.seal_block(right)?;

        let mut b = b.switch_to_block(left)?;
        b.def_int_var(x, 1_i32)?;
        let b = b.br(join)?;

        let mut b = b.switch_to_block(right)?;
        b.def_int_var(x, 2_i32)?;
        let b = b.br(join)?;

        let mut b = b.switch_to_block(join)?;
        b.seal_block(join)?;
        let read = b.use_int_var(x)?;
        let b = b.ret(read)?;
        b.finish()?;

        let text = format!("{m}");
        let phi_count = text.matches("phi i32").count();
        assert_eq!(phi_count, 1, "expected exactly one phi, got:\n{text}");
        assert!(
            text.contains("[ 1, %left ]") && text.contains("[ 2, %right ]"),
            "expected both incoming arms in the join phi, got:\n{text}"
        );
        Ok(())
    })
}

/// Braun et al. 2013's incomplete-phi + completion flow (Fig. 4), ported
/// through the full public lifecycle against the `factorial_example.rs`
/// loop shape: the loop block's accumulator/counter are read (via
/// `use_int_var`) before the loop's own back-edge is recorded, so each
/// read creates an OPERANDLESS phi (the block is not yet sealed -- its
/// predecessor set is incomplete until the back-edge is known). Sealing
/// the loop block after its own `cond_br` back-edge completes both
/// phis with their entry + back-edge incoming values.
#[test]
fn loop_backedge_completes_incomplete_phi_on_seal() -> Result<(), IrError> {
    Module::with_new("ssa-loop-factorial", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("factorial", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let base = b.create_block("base");
        let loop_bb = b.create_block("loop");
        let exit = b.create_block("exit");

        let acc_var = b.declare_int_var::<i32, _>("acc");
        let i_var = b.declare_int_var::<i32, _>("i");

        let mut b = b.switch_to_block(entry)?;
        let n: IntValue<i32> = f.param(0)?.try_into()?;
        let is_zero =
            b.ins()
                .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
        // The loop's own entry-edge incoming values: `acc` starts at 1,
        // `i` starts at the parameter `n` (mirrors the factorial
        // example's `acc_phi.add_incoming(1_i32, entry_label)` /
        // `i_phi.add_incoming(n, entry_label)`). These defs belong in
        // `entry` -- `def_int_var` records the CURRENT block's
        // definition, and the entry-edge value is entry's, not the loop
        // block's.
        b.def_int_var(acc_var, 1_i32)?;
        b.def_int_var(i_var, n)?;
        let mut b = b.cond_br(is_zero, base, loop_bb)?;
        b.seal_block(base)?;

        let b = b.switch_to_block(base)?;
        let one = m.i32_type().const_int(1_i32);
        let b = b.ret(one)?;

        let mut b = b.switch_to_block(loop_bb)?;
        // Read BEFORE this block's own back-edge is recorded: `loop_bb`
        // is still unsealed (its only known predecessor so far is
        // `entry`; the self back-edge is recorded only once this
        // block's own terminator below runs), so each read creates an
        // operandless phi.
        let acc = b.use_int_var(acc_var)?;
        let i = b.use_int_var(i_var)?;

        let before_seal_text = format!("{m}");
        assert!(
            before_seal_text.contains("%acc = phi i32 \n")
                && before_seal_text.contains("%i = phi i32 \n"),
            "expected both phis operandless (no `[ ... ]` incoming yet) before seal, got:\n\
             {before_seal_text}"
        );

        let next_acc = b.ins().build_int_mul(acc, i, "next_acc")?;
        let next_i = b.ins().build_int_sub(i, 1_i32, "next_i")?;
        let done =
            b.ins()
                .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, next_i, 0_i32, "done")?;
        b.def_int_var(acc_var, next_acc)?;
        b.def_int_var(i_var, next_i)?;
        let mut b = b.cond_br(done, exit, loop_bb)?;

        // The loop block's predecessor set (entry, loop-self) is now
        // fully known: seal completes both incomplete phis.
        b.seal_block(loop_bb)?;

        let after_seal_text = format!("{m}");
        assert!(
            after_seal_text.contains("[ %0, %entry ]"),
            "expected the i phi's entry incoming (the function parameter) after seal, got:\n\
             {after_seal_text}"
        );
        assert!(
            after_seal_text.contains("[ %next_i, %loop ]"),
            "expected the i phi's back-edge incoming after seal, got:\n{after_seal_text}"
        );
        assert!(
            after_seal_text.contains("[ %next_acc, %loop ]"),
            "expected the acc phi's back-edge incoming after seal, got:\n{after_seal_text}"
        );

        let mut b = b.switch_to_block(exit)?;
        b.seal_block(exit)?;
        let read = b.use_int_var(acc_var)?;
        let b = b.ret(read)?;
        b.finish()?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// A strict (non-poison) variable read on a path that chases all the way
/// back to the (sealed, predecessor-less) entry block with no
/// intervening write anywhere is `Err(SsaUseOfUndefinedVariable)`. No
/// single upstream C++ unit test covers this exactly -- `mem2reg`/
/// `SSAUpdater` assume the caller already proved definedness via
/// dominance analysis on EXISTING IR, whereas this layer is documenting
/// new IR into existence and must reject the same case itself.
#[test]
fn strict_use_before_def_is_typed_error() -> Result<(), IrError> {
    Module::with_new("ssa-strict-undef", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let x = b.declare_int_var::<i32, _>("x");

        let mut b = b.switch_to_block(entry)?;
        match b.use_int_var(x) {
            Err(IrError::SsaUseOfUndefinedVariable { .. }) => Ok(()),
            Ok(_) => panic!("expected SsaUseOfUndefinedVariable, got Ok"),
            Err(other) => panic!("expected SsaUseOfUndefinedVariable, got {other:?}"),
        }
    })
}

/// Poison twin of [`strict_use_before_def_is_typed_error`]: a
/// `declare_int_var_poison` variable read on the same def-less path
/// yields `poison i32` instead of an error (D10's explicit opt-in
/// escape hatch). Locks the `poison` token in the printed IR -- the
/// read must actually MATERIALISE a `poison` constant use, not just
/// avoid erroring.
#[test]
fn poison_variable_reads_poison_on_undef_path() -> Result<(), IrError> {
    Module::with_new("ssa-poison-undef", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let x = b.declare_int_var_poison::<i32, _>("x");

        let mut b = b.switch_to_block(entry)?;
        let read = b.use_int_var(x)?;
        let b = b.ret(read)?;
        b.finish()?;

        let text = format!("{m}");
        assert!(
            text.contains("ret i32 poison"),
            "expected the undef-path read to materialise poison, got:\n{text}"
        );
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Whole-branch-review regression (D10): a dead (function-entry-
/// unreachable) two-block cycle `loop1 <-> loop2` reads a STRICT i32
/// variable declared AFTER a same-type POISON variable. The read happens
/// while `loop1` is still unsealed, seeding an operandless "incomplete"
/// phi (Braun's `incompletePhis`); once both blocks' `br`s complete the
/// cycle and `loop1` is sealed, `add_phi_operands` discovers the phi's
/// only reachable operand is itself (the cycle has no external
/// predecessor) and collapses it via `try_remove_trivial_phi` into
/// `undefined_phi_replacement`.
///
/// `undefined_phi_replacement` used to recover the phi's owning variable
/// by TYPE ALONE (`vars.iter().find(|v| v.ty == ty)`), so with a
/// same-type POISON variable declared FIRST, the strict variable's dead
/// phi would silently resolve to `poison` (masking the D10 violation)
/// instead of erroring -- or, if the naming happened to differ, the
/// error would misname the poison variable rather than the strict one
/// actually being read. This must instead name the STRICT variable and
/// error, never poison. No upstream analogue: LLVM's `IRBuilder` has no
/// on-the-fly SSA layer and never faces this ambiguity (see the module
/// doc's `SSAUpdater` comparison).
#[test]
fn dead_cycle_phi_names_the_actual_strict_variable_not_same_type_poison() -> Result<(), IrError> {
    Module::with_new("ssa-dead-cycle-strict-vs-poison", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let loop1 = b.create_block("loop1");
        let loop2 = b.create_block("loop2");

        // Same-type (i32) poison variable declared FIRST -- the buggy
        // type-only lookup would match this one regardless of which
        // variable's phi is actually being resolved.
        let _poison_var = b.declare_int_var_poison::<i32, _>("poisoned");
        let strict_var = b.declare_int_var::<i32, _>("strict");

        // Entry is unreachable into the cycle below -- it just returns
        // on its own, keeping `loop1`/`loop2` a genuinely dead cycle.
        let b = b.switch_to_block(entry)?;
        let b = b.ret(0_i32)?;

        // `loop1` is not sealed yet (its predecessor set -- `loop2`, plus
        // its own eventual back-edge -- is not yet fully recorded), so
        // this read seeds an operandless incomplete phi rather than
        // erroring or resolving immediately.
        let mut b = b.switch_to_block(loop1)?;
        let _read = b.use_int_var(strict_var)?;
        let b = b.br(loop2)?;

        // Completes the cycle: `loop2`'s only predecessor is `loop1`.
        let b = b.switch_to_block(loop2)?;
        let mut b = b.br(loop1)?;
        b.seal_block(loop2)?;

        // `loop1`'s predecessor set (`loop2` only -- there is no
        // external entry into this cycle) is now fully known. Sealing
        // completes the incomplete phi seeded above; that phi's only
        // operand chases back to itself (the dead cycle has no other
        // source), so `try_remove_trivial_phi` falls through to
        // `undefined_phi_replacement`.
        match b.seal_block(loop1) {
            Err(IrError::SsaUseOfUndefinedVariable { variable, .. }) => {
                assert_eq!(
                    variable, "strict",
                    "expected the error to name the STRICT variable actually read, not a \
                     same-type poison variable matched by type alone"
                );
                Ok(())
            }
            Ok(()) => panic!(
                "expected SsaUseOfUndefinedVariable naming \"strict\", got Ok -- the dead \
                 cycle's strict read was silently satisfied (likely resolved to poison)"
            ),
            Err(other) => {
                panic!("expected SsaUseOfUndefinedVariable naming \"strict\", got {other:?}")
            }
        }
    })
}

/// `br`ing into the entry block is rejected: `create_block`'s first call
/// auto-seals the entry block (per `Verifier::visitFunction`'s "entry
/// has no predecessors" invariant), so ANY edge recorded into it --
/// including a `br` issued from a later block -- errors with
/// `SsaBranchToSealedBlock` rather than silently accepting a
/// predecessor edge the Braun engine already promised itself entry
/// would never have.
#[test]
fn branch_to_sealed_block_rejected() -> Result<(), IrError> {
    Module::with_new("ssa-branch-sealed", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let second = b.create_block("second");

        let b = b.switch_to_block(entry)?;
        let b = b.br(second)?;

        let b = b.switch_to_block(second)?;
        match b.br(entry) {
            Err(IrError::SsaBranchToSealedBlock { .. }) => Ok(()),
            Ok(_) => panic!("expected SsaBranchToSealedBlock, got Ok"),
            Err(other) => panic!("expected SsaBranchToSealedBlock, got {other:?}"),
        }
    })
}

/// `double_seal_rejected`: sealing the SAME block twice through the
/// public lifecycle (rather than the in-crate `#[cfg(test)]` white-box
/// coverage) is `SsaBlockAlreadySealed`. Braun's algorithm assumes each
/// block is sealed exactly once, after which its predecessor set is
/// considered final.
#[test]
fn double_seal_rejected() -> Result<(), IrError> {
    Module::with_new("ssa-double-seal-public", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let _entry = b.create_block("entry");
        let second = b.create_block("second");
        b.seal_block(second)?;
        match b.seal_block(second) {
            Err(IrError::SsaBlockAlreadySealed { .. }) => Ok(()),
            Ok(()) => panic!("expected SsaBlockAlreadySealed, got Ok"),
            Err(other) => panic!("expected SsaBlockAlreadySealed, got {other:?}"),
        }
    })
}

/// `foreign_variable_rejected`: a declared variable handle from one
/// `SsaBuilder` used against a different builder's def/use surface is a
/// typed runtime error (`check_owner_var`, the sibling check to
/// `check_owner_block`'s existing `SsaForeignBlock` coverage).
#[test]
fn foreign_variable_rejected() -> Result<(), IrError> {
    Module::with_new("ssa-foreign-variable", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f1 = m.add_function::<(), _>("f1", fn_ty, Linkage::External)?;
        let f2 = m.add_function::<(), _>("f2", fn_ty, Linkage::External)?;

        let mut b1 = SsaBuilder::for_function(&m, f1)?;
        let x1 = b1.declare_int_var::<i32, _>("x");

        let mut b2 = SsaBuilder::for_function(&m, f2)?;
        let entry2 = b2.create_block("entry");
        let mut b2 = b2.switch_to_block(entry2)?;

        match b2.def_int_var(x1, 1_i32) {
            Err(IrError::SsaForeignVariable) => {}
            Ok(_) => panic!("expected SsaForeignVariable, got Ok"),
            Err(other) => panic!("expected SsaForeignVariable, got {other:?}"),
        }
        match b2.use_int_var(x1) {
            Err(IrError::SsaForeignVariable) => Ok(()),
            Ok(_) => panic!("expected SsaForeignVariable, got Ok"),
            Err(other) => panic!("expected SsaForeignVariable, got {other:?}"),
        }
    })
}

/// `finish_reports_unfilled_block`: a block that was `create_block`d but
/// never received a terminator is reported by `finish` as
/// `SsaUnfilledBlock`, even though every OTHER block in the function is
/// properly filled.
#[test]
fn finish_reports_unfilled_block() -> Result<(), IrError> {
    Module::with_new("ssa-unfilled", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let _unfilled = b.create_block("unfilled");

        let b = b.switch_to_block(entry)?;
        let b = b.ret_void();

        match b.finish() {
            Err(IrError::SsaUnfilledBlock { .. }) => Ok(()),
            Ok(()) => panic!("expected SsaUnfilledBlock, got Ok"),
            Err(other) => panic!("expected SsaUnfilledBlock, got {other:?}"),
        }
    })
}

/// `switch_to_block` rejects repositioning into an already-filled
/// (terminated) block -- the linear insertion capability was already
/// consumed by that block's own terminator.
#[test]
fn switch_to_block_rejects_already_filled_block() -> Result<(), IrError> {
    Module::with_new("ssa-switch-filled", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");

        let b = b.switch_to_block(entry)?;
        let b = b.ret_void();

        match b.switch_to_block(entry) {
            Err(IrError::SsaBlockAlreadyFilled { .. }) => Ok(()),
            Ok(_) => panic!("expected SsaBlockAlreadyFilled, got Ok"),
            Err(other) => panic!("expected SsaBlockAlreadyFilled, got {other:?}"),
        }
    })
}

/// A dyn-declared int variable (`declare_int_var_dyn`, no static width)
/// rejects a def whose LIFTED value has a different width than the
/// variable's own pinned type -- the type-validation invariant
/// (`task_ff09d3e3`, Task 17 review follow-up) that the trivial-phi
/// RAUW path depends on. `IntoIntValue<IntDyn>` happily lifts ANY
/// width, which is the case `def_int_var`'s check was originally written
/// for (mirrors
/// `hostile_native_typed_override_wrong_width_rejected_by_accept_folded_int`
/// in `ir_builder.rs`, the analogous fold-result seam).
///
/// The check is no longer keyed on the marker being erased: the static
/// half is covered by `def_int_var_rejects_forged_static_width_handle`
/// (`src/ssa_builder.rs`), which needs crate-internal `from_value_unchecked`
/// to forge its handle and so cannot live out here.
///
/// Both sides are integers, so the widths are reported rather than a
/// `TypeMismatch { expected: Integer, got: Integer }` that could not say
/// which width was wrong (`Type::require_match`).
#[test]
fn dyn_int_var_wrong_width_def_rejected() -> Result<(), IrError> {
    Module::with_new("ssa-dyn-wrong-width", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");

        let i32_dyn_ty = m.custom_width_int_type(32)?;
        let i64_dyn_ty = m.custom_width_int_type(64)?;
        let x = b.declare_int_var_dyn(i32_dyn_ty, "x");

        let mut b = b.switch_to_block(entry)?;
        let wrong_width_const = i64_dyn_ty.const_int_checked(1_i64)?;
        match b.def_int_var(x, wrong_width_const) {
            Err(IrError::OperandWidthMismatch { lhs: 32, rhs: 64 }) => Ok(()),
            Ok(()) => panic!("expected OperandWidthMismatch {{ lhs: 32, rhs: 64 }}, got Ok"),
            Err(other) => {
                panic!("expected OperandWidthMismatch {{ lhs: 32, rhs: 64 }}, got {other:?}")
            }
        }
    })
}

/// Float twin of [`dyn_int_var_wrong_width_def_rejected`]: a dyn-declared
/// float variable (`declare_float_var_dyn`, marker `FloatDyn`) rejects a
/// def whose lifted value has a different IEEE kind than the variable's
/// own pinned type -- the same invariant, one marker family over. The
/// static half is covered by
/// `def_float_var_rejects_forged_static_kind_handle` (`src/ssa_builder.rs`).
///
/// Stays `TypeMismatch` where the int twin now reports widths:
/// `TypeKindLabel` has a distinct variant per float kind, so these labels
/// already name both sides precisely (`Type::require_match`).
#[test]
fn dyn_float_var_wrong_kind_def_rejected() -> Result<(), IrError> {
    Module::with_new("ssa-dyn-float-wrong-kind", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");

        let half_dyn_ty = m.half_type().as_dyn();
        let double_dyn_ty = m.f64_type().as_dyn();
        let x = b.declare_float_var_dyn(half_dyn_ty, "x");

        let mut b = b.switch_to_block(entry)?;
        let wrong_kind_const = double_dyn_ty.const_from_bits(0);
        match b.def_float_var(x, wrong_kind_const) {
            Err(IrError::TypeMismatch { .. }) => Ok(()),
            Ok(()) => panic!("expected TypeMismatch, got Ok"),
            Err(other) => panic!("expected TypeMismatch, got {other:?}"),
        }
    })
}

/// Pointer twin of [`dyn_int_var_wrong_width_def_rejected`]: a pointer
/// variable declared in one address space
/// (`declare_pointer_var_in_addrspace`) rejects a def whose lifted
/// value is a pointer in a DIFFERENT address space. `PointerValue` never
/// statically pins an address space, so this side never had a static
/// marker to key on -- it is unconditional for the same reason the int
/// and float sides now are: nothing between the caller and `current_def`
/// re-checks the claim otherwise.
///
/// The error must NAME both address spaces. `TypeKindLabel::Pointer` carries
/// no address space, so the `TypeMismatch { expected: Pointer, got: Pointer }`
/// this used to report was true and useless -- the same defect the int side
/// sheds by reporting `OperandWidthMismatch`. It gets its own error rather
/// than borrowing that one because an address space is not a width
/// (`Type::require_match`).
#[test]
fn pointer_var_wrong_addrspace_def_rejected() -> Result<(), IrError> {
    Module::with_new("ssa-ptr-wrong-addrspace", |m| {
        let void_ty = m.void_type();
        let ptr_as1_ty = m.ptr_type(1);
        let fn_ty = m.fn_type(void_ty, [ptr_as1_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");

        let addrspace0_ty = m.ptr_type(0);
        let px = b.declare_pointer_var_in_addrspace(addrspace0_ty, "px");

        let mut b = b.switch_to_block(entry)?;
        // The parameter is a pointer in addrspace 1; `px` was declared
        // in addrspace 0.
        let wrong_addrspace_ptr = f.param(0)?;
        let err = b
            .def_pointer_var(px, wrong_addrspace_ptr)
            .expect_err("a wrong-address-space def must be rejected");

        assert_eq!(
            err,
            IrError::AddressSpaceMismatch {
                expected: 0,
                got: 1
            }
        );
        assert_eq!(
            err.to_string(),
            "pointer address space mismatch: expected addrspace(0), got addrspace(1)"
        );
        Ok(())
    })
}

/// `switch`'s edge-recording is per case OCCURRENCE, duplicates
/// preserved: two case entries targeting the SAME block record TWO
/// predecessor edges into it, matching `crate::cfg::FunctionCfg`'s own
/// switch successor list (default, then every case target in order, no
/// deduplication) -- which is exactly what the verifier's phi
/// entry-count-vs-predecessor-count-with-multiplicity check counts
/// against.
///
/// Two case values from the SAME switch necessarily share one source
/// block's `current_def` -- Braun's trivial-phi elimination would
/// (correctly) collapse a phi merging only those two identical-value
/// edges down to the value itself, which would make a naive
/// "count phi incoming entries" assertion pass even if edges were
/// wrongly deduplicated to one. To force a surviving phi that actually
/// exercises multiplicity, `shared` also gets a THIRD, differently-
/// valued edge from an unrelated block `pre` (entry picks between `pre`
/// and the switch source via `cond_br` on a second parameter): the
/// resulting phi must show the switch's source block TWICE, once per
/// case occurrence, alongside `pre` once -- three incoming entries in
/// total, not two (deduplicated) or four (over-counted).
#[test]
fn switch_records_one_edge_per_case_occurrence() -> Result<(), IrError> {
    Module::with_new("ssa-switch-multiplicity", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let pre = b.create_block("pre");
        let switch_source = b.create_block("switch_source");
        let shared = b.create_block("shared");
        let default_bb = b.create_block("default_bb");

        let x = b.declare_int_var::<i32, _>("x");

        let b = b.switch_to_block(entry)?;
        let mode: IntValue<i32> = f.param(0)?.try_into()?;
        let take_pre =
            b.ins()
                .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, mode, 0_i32, "take_pre")?;
        let mut b = b.cond_br(take_pre, pre, switch_source)?;
        b.seal_block(pre)?;
        b.seal_block(switch_source)?;

        let mut b = b.switch_to_block(pre)?;
        b.def_int_var(x, 999_i32)?;
        let b = b.br(shared)?;

        let mut b = b.switch_to_block(switch_source)?;
        let n: IntValue<i32> = f.param(1)?.try_into()?;
        b.def_int_var(x, 100_i32)?;
        let case0 = 0_i32;
        let case1 = 1_i32;
        let mut b = b.switch(n, default_bb, [(case0, shared), (case1, shared)])?;
        b.seal_block(default_bb)?;
        b.seal_block(shared)?;

        let b = b.switch_to_block(default_bb)?;
        let b = b.ret(0_i32)?;

        let mut b = b.switch_to_block(shared)?;
        let read = b.use_int_var(x)?;
        let b = b.ret(read)?;
        b.finish()?;

        let text = format!("{m}");
        let phi_line = text
            .lines()
            .find(|l| l.contains("phi i32"))
            .unwrap_or_else(|| panic!("expected a surviving phi in `shared`, got:\n{text}"));
        let incoming_count = phi_line.matches('[').count();
        assert_eq!(
            incoming_count, 3,
            "expected three incoming entries (two from the switch's case \
             occurrences, one from `pre`), got line:\n{phi_line}\nfull IR:\n{text}"
        );
        let switch_source_occurrences = phi_line.matches("%switch_source").count();
        assert_eq!(
            switch_source_occurrences, 2,
            "expected `switch_source` to appear TWICE in the phi's incoming \
             list (one per case edge, not deduplicated), got line:\n{phi_line}"
        );
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Sweeps every CFG shape this session's tests build (straight-line
/// single-pred, if/then/else diamond, loop with a back-edge, switch with
/// a shared destination) through `Module::verify_borrowed`, confirming
/// every auto-SSA-constructed module is well-formed IR -- not just that
/// the Braun engine's own bookkeeping is internally consistent.
#[test]
fn every_auto_ssa_module_verifies() -> Result<(), IrError> {
    // Straight-line single-predecessor chain.
    Module::with_new("ssa-verify-straight-line", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let second = b.create_block("second");
        let x = b.declare_int_var::<i32, _>("x");

        let mut b = b.switch_to_block(entry)?;
        b.def_int_var(x, 7_i32)?;
        let b = b.br(second)?;

        let mut b = b.switch_to_block(second)?;
        b.seal_block(second)?;
        let read = b.use_int_var(x)?;
        let b = b.ret(read)?;
        b.finish()?;

        m.verify_borrowed()
    })?;

    // If/then/else diamond.
    Module::with_new("ssa-verify-diamond", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let left = b.create_block("left");
        let right = b.create_block("right");
        let join = b.create_block("join");
        let x = b.declare_int_var::<i32, _>("x");

        let b = b.switch_to_block(entry)?;
        let n: IntValue<i32> = f.param(0)?.try_into()?;
        let cond = b
            .ins()
            .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "cond")?;
        let mut b = b.cond_br(cond, left, right)?;
        b.seal_block(left)?;
        b.seal_block(right)?;

        let mut b = b.switch_to_block(left)?;
        b.def_int_var(x, 1_i32)?;
        let b = b.br(join)?;

        let mut b = b.switch_to_block(right)?;
        b.def_int_var(x, 2_i32)?;
        let b = b.br(join)?;

        let mut b = b.switch_to_block(join)?;
        b.seal_block(join)?;
        let read = b.use_int_var(x)?;
        let b = b.ret(read)?;
        b.finish()?;

        m.verify_borrowed()
    })?;

    // Loop with a back-edge (factorial shape).
    Module::with_new("ssa-verify-loop", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("factorial", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let base = b.create_block("base");
        let loop_bb = b.create_block("loop");
        let exit = b.create_block("exit");
        let acc_var = b.declare_int_var::<i32, _>("acc");
        let i_var = b.declare_int_var::<i32, _>("i");

        let mut b = b.switch_to_block(entry)?;
        let n: IntValue<i32> = f.param(0)?.try_into()?;
        let is_zero =
            b.ins()
                .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
        b.def_int_var(acc_var, 1_i32)?;
        b.def_int_var(i_var, n)?;
        let mut b = b.cond_br(is_zero, base, loop_bb)?;
        b.seal_block(base)?;

        let b = b.switch_to_block(base)?;
        let b = b.ret(1_i32)?;

        let mut b = b.switch_to_block(loop_bb)?;
        let acc = b.use_int_var(acc_var)?;
        let i = b.use_int_var(i_var)?;
        let next_acc = b.ins().build_int_mul(acc, i, "next_acc")?;
        let next_i = b.ins().build_int_sub(i, 1_i32, "next_i")?;
        let done =
            b.ins()
                .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, next_i, 0_i32, "done")?;
        b.def_int_var(acc_var, next_acc)?;
        b.def_int_var(i_var, next_i)?;
        let mut b = b.cond_br(done, exit, loop_bb)?;
        b.seal_block(loop_bb)?;

        let mut b = b.switch_to_block(exit)?;
        b.seal_block(exit)?;
        let read = b.use_int_var(acc_var)?;
        let b = b.ret(read)?;
        b.finish()?;

        m.verify_borrowed()
    })?;

    // Switch with a shared destination, plus `unreachable` on the
    // "impossible" default arm and `ret_void`/float/pointer def-use on
    // the shared arm -- rounds out coverage of every terminator kind
    // and every def/use category this session's Positioned surface
    // exposes.
    Module::with_new("ssa-verify-switch-mixed", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let f64_ty = m.f64_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(
            void_ty,
            [i32_ty.as_type(), f64_ty.as_type(), ptr_ty.as_type()],
            false,
        );
        let f = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let case_bb = b.create_block("case_bb");
        let unreachable_bb = b.create_block("unreachable_bb");
        let fx = b.declare_float_var::<f64, _>("fx");
        let px = b.declare_pointer_var("px");

        let mut b = b.switch_to_block(entry)?;
        let n: IntValue<i32> = f.param(0)?.try_into()?;
        let fparam = f.param(1)?;
        let pparam = f.param(2)?;
        b.def_float_var(fx, fparam)?;
        b.def_pointer_var(px, pparam)?;
        let case0 = 0_i32;
        let mut b = b.switch(n, unreachable_bb, [(case0, case_bb)])?;
        b.seal_block(case_bb)?;
        b.seal_block(unreachable_bb)?;

        let b = b.switch_to_block(unreachable_bb)?;
        let b = b.unreachable();

        let mut b = b.switch_to_block(case_bb)?;
        let _f_read = b.use_float_var(fx)?;
        let _p_read = b.use_pointer_var(px)?;
        let b = b.ret_void();
        b.finish()?;

        m.verify_borrowed()
    })?;

    Ok(())
}

/// llvmkit-specific (Doctrine D11), review follow-up: `switch`'s case
/// constants are lifted via `IntoConstantInt<W>` in a pre-pass, BEFORE
/// `self.inner` is taken or any IR is emitted -- including for a dyn
/// (`IntDyn`) condition, where the lift is still a genuine runtime fit
/// check (a dyn switch's width is only known at runtime, so this seam
/// cannot be pushed to compile time; see D3). Here an 8-bit dyn
/// condition paired with an out-of-range `i32` case literal must fail
/// with `IrError::ImmediateOverflow` from the pre-pass lift, and --
/// unlike the old `IsValue`-bounded shape, where `SwitchInst::add_case`
/// would only catch a bad case AFTER `build_switch_dyn` had already emitted
/// the terminator with its default target -- the printed module must
/// show NO `switch` instruction at all: the failure happens strictly
/// before the terminator is built.
#[test]
fn switch_dyn_condition_bad_width_case_rejected_before_emit() -> Result<(), IrError> {
    Module::with_new("ssa-dyn-bad-case-preemit", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let default_bb = b.create_block("default_bb");
        let case_bb = b.create_block("case_bb");

        let dyn_ty = m.custom_width_int_type(8)?;
        let cond = dyn_ty.const_int_checked(3_i64)?;

        let b = b.switch_to_block(entry)?;
        // 1000 does not fit in the condition's actual 8-bit runtime
        // width -- the pre-pass lift via `IntoConstantInt<IntDyn>` must
        // reject it before `build_switch_dyn` ever runs.
        let bad_case = 1000_i32;
        match b.switch(cond, default_bb, [(bad_case, case_bb)]) {
            Err(IrError::ImmediateOverflow { bits: 8, .. }) => {}
            Ok(_) => panic!("expected ImmediateOverflow, got Ok"),
            Err(other) => panic!("expected ImmediateOverflow, got {other:?}"),
        }

        let text = format!("{m}");
        assert!(
            !text.contains("switch"),
            "no switch instruction should have been emitted, got:\n{text}"
        );
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Property test (Task 19 Step 3): random bounded reducible CFGs always
// verify; strict-undef schedules always yield `SsaUseOfUndefinedVariable`,
// never invalid IR.
// --------------------------------------------------------------------------
//
// Rather than a fully general random CFG generator (unbounded blocks/edges
// risks generating shapes the typed Positioned/Unpositioned lifecycle makes
// awkward to drive generically, and risks non-reducible CFGs the Braun
// engine was never designed for), this sweeps a small TABLE of the same
// bounded shapes `every_auto_ssa_module_verifies` above already hand-locks
// (straight-line chain, if/then/else diamond, loop with a back-edge,
// switch with a shared destination) -- each shape capped at <= 8 blocks
// and parameterised by a random var count (1..=4) and random small integer
// literals, always following seal-after-all-preds discipline and always
// calling `finish()`. This is the "table of shapes + random schedule
// choices" the brief sanctions over a fully general generator.
//
// llvmkit-specific: `llvm/lib/IR/Verifier.cpp` has no analogous randomised
// construction fuzzer of its own on-the-fly SSA layer (LLVM's IR is always
// built by a frontend that already knows dominance; this engine documents
// new IR into existence and must prove its own output well-formed no
// matter which of these bounded schedules produced it).

/// One of the four bounded CFG shapes every generated case builds. Mirrors
/// `every_auto_ssa_module_verifies`'s four hand-written shapes above,
/// parameterised instead of hard-coded.
#[derive(Debug, Clone, Copy)]
enum ShapeKind {
    StraightLine,
    Diamond,
    Loop,
    SwitchShared,
}

/// A single generated test case: which shape, how many `i32` variables
/// (<= 4) participate, small literal values to write into them, and --
/// for the strict-undef sweep -- which variable index (if any) is left
/// unwritten on a reachable path to force `SsaUseOfUndefinedVariable`.
#[derive(Debug, Clone)]
struct GeneratedCase {
    shape: ShapeKind,
    var_count: usize,
    literals: Vec<i32>,
    /// `Some(index)` selects a strict-undef schedule for that variable
    /// (skips its def on one path); `None` means every variable is
    /// defined on every path before its read (the well-defined sweep).
    undef_var: Option<usize>,
}

fn shape_kind_strategy() -> impl Strategy<Value = ShapeKind> {
    prop_oneof![
        Just(ShapeKind::StraightLine),
        Just(ShapeKind::Diamond),
        Just(ShapeKind::Loop),
        Just(ShapeKind::SwitchShared),
    ]
}

/// Bounds: `var_count` in 1..=4 (brief's "<=4 int vars"); each shape below
/// tops out at 5 blocks (switch: entry/pre/case/default/shared), well
/// under the brief's "<=8 blocks" cap; literals are kept small (the
/// values themselves are irrelevant to well-formedness, only their
/// presence as distinct phi incoming operands matters). The undef-sweep
/// arm's `undef_var` is sampled uniformly from `0..var_count` (rather
/// than pinned to variable 0) -- the doc comments on each `build_*`
/// function below advertise a "randomised which-variable" schedule, so
/// every declared variable, not just the first, must actually get
/// exercised as the undefined one across the property's shrink space.
fn generated_case_strategy(allow_undef: bool) -> impl Strategy<Value = GeneratedCase> {
    (
        shape_kind_strategy(),
        1_usize..=4,
        prop::collection::vec(-100_i32..100, 4),
    )
        .prop_flat_map(move |(shape, var_count, literals)| {
            let undef_var_strategy: proptest::prelude::BoxedStrategy<Option<usize>> = if allow_undef
            {
                proptest::option::of(0_usize..var_count).boxed()
            } else {
                Just(None).boxed()
            };
            undef_var_strategy.prop_map(move |undef_var| GeneratedCase {
                shape,
                var_count,
                literals: literals.clone(),
                undef_var,
            })
        })
}

/// What building one [`GeneratedCase`] observed. A strict-undef schedule
/// must hit `HitExpectedUndef` (the module is intentionally left
/// unfinished the moment the expected error fires -- never `finish()`ed,
/// never handed to `verify_borrowed()`, since an incomplete module is
/// not the thing under test on that path); a well-defined schedule must
/// hit `Finished` (the module is fully built and ready to verify).
/// Anything else (an unexpected error propagated via `?`) fails the
/// case outright.
enum BuildOutcome {
    Finished,
    HitExpectedUndef,
}

/// Shared read-loop: reads every variable in `vars`, in order. If
/// `case.undef_var` is set and its read fails with
/// `SsaUseOfUndefinedVariable`, returns `HitExpectedUndef` immediately
/// (the remaining variables are never read -- the expected error already
/// fired). Any other error propagates via `?`. On a well-defined case
/// every read succeeds and the collected values are returned for the
/// caller to finish building with (e.g. feeding the last one to `ret`).
fn read_all_vars<'m, 'ctx, B, F, R>(
    b: &mut SsaBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
    vars: &[llvmkit_ir::IntVariable<'ctx, i32, B>],
    undef_var: Option<usize>,
) -> Result<Result<Vec<IntValue<'ctx, i32, B>>, ()>, IrError>
where
    B: llvmkit_ir::ModuleBrand + 'ctx,
    F: llvmkit_ir::ir_builder::folder::IRBuilderFolder<'ctx, B> + Clone,
    R: llvmkit_ir::marker::ReturnMarker,
{
    let mut reads = Vec::with_capacity(vars.len());
    for (i, var) in vars.iter().enumerate() {
        match (b.use_int_var(*var), undef_var == Some(i)) {
            (Ok(v), _) => reads.push(v),
            (Err(IrError::SsaUseOfUndefinedVariable { .. }), true) => return Ok(Err(())),
            (Err(other), _) => return Err(other),
        }
    }
    Ok(Ok(reads))
}

/// Builds (and, on the well-defined path, finishes) the `StraightLine`
/// shape (`entry -> mid -> exit`): every variable is defined in `entry`,
/// `mid` is a pass-through, `exit` reads every variable. With
/// `undef_var = Some(idx)`, variable `idx`'s def in `entry` is skipped, so
/// `exit`'s read of it chases back through `mid` to the sealed,
/// predecessor-less function entry and must error.
fn build_straight_line(m: &Module<'_>, case: &GeneratedCase) -> Result<BuildOutcome, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let mut b = SsaBuilder::for_function(m, f)?;
    let entry = b.create_block("entry");
    let mid = b.create_block("mid");
    let exit = b.create_block("exit");

    let vars: Vec<_> = (0..case.var_count)
        .map(|i| b.declare_int_var::<i32, _>(format!("v{i}")))
        .collect();

    let mut b = b.switch_to_block(entry)?;
    for (i, var) in vars.iter().enumerate() {
        if case.undef_var != Some(i) {
            b.def_int_var(*var, case.literals[i])?;
        }
    }
    let b = b.br(mid)?;

    let mut b = b.switch_to_block(mid)?;
    b.seal_block(mid)?;
    let b = b.br(exit)?;

    let mut b = b.switch_to_block(exit)?;
    b.seal_block(exit)?;
    let Ok(reads) = read_all_vars(&mut b, &vars, case.undef_var)? else {
        return Ok(BuildOutcome::HitExpectedUndef);
    };
    let b = b.ret(reads[0])?;
    b.finish()?;
    Ok(BuildOutcome::Finished)
}

/// Builds (and, on the well-defined path, finishes) the `Diamond` shape
/// (`entry -> {left, right} -> join`): every variable gets a DIFFERENT
/// literal on each arm (forcing a real, surviving phi at `join`, not a
/// trivially-eliminated one). With `undef_var = Some(idx)`, variable
/// `idx`'s def on the `right` arm is skipped, so `join`'s read chases a
/// phi operand back through the sealed `right` block to function entry and
/// must error.
fn build_diamond(m: &Module<'_>, case: &GeneratedCase) -> Result<BuildOutcome, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let mut b = SsaBuilder::for_function(m, f)?;
    let entry = b.create_block("entry");
    let left = b.create_block("left");
    let right = b.create_block("right");
    let join = b.create_block("join");

    let vars: Vec<_> = (0..case.var_count)
        .map(|i| b.declare_int_var::<i32, _>(format!("v{i}")))
        .collect();

    let b = b.switch_to_block(entry)?;
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let cond = b
        .ins()
        .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "cond")?;
    let mut b = b.cond_br(cond, left, right)?;
    b.seal_block(left)?;
    b.seal_block(right)?;

    let mut b = b.switch_to_block(left)?;
    for (i, var) in vars.iter().enumerate() {
        b.def_int_var(*var, case.literals[i])?;
    }
    let b = b.br(join)?;

    let mut b = b.switch_to_block(right)?;
    for (i, var) in vars.iter().enumerate() {
        if case.undef_var != Some(i) {
            b.def_int_var(*var, case.literals[i].wrapping_add(1))?;
        }
    }
    let b = b.br(join)?;

    let mut b = b.switch_to_block(join)?;
    b.seal_block(join)?;
    let Ok(reads) = read_all_vars(&mut b, &vars, case.undef_var)? else {
        return Ok(BuildOutcome::HitExpectedUndef);
    };
    let b = b.ret(reads[0])?;
    b.finish()?;
    Ok(BuildOutcome::Finished)
}

/// Builds (and, on the well-defined path, finishes) the `Loop` shape
/// (factorial's own CFG: `entry -> {base, loop}`, `loop -> {exit, loop}`
/// back-edge): every variable is defined in `entry` (the loop's
/// entry-edge incoming value) and redefined in `loop` (the back-edge
/// incoming value) before `loop`'s own back-edge is recorded, exercising
/// Braun's incomplete-phi-on-seal path.
///
/// Unlike the other three shapes, `loop`'s own read happens in an
/// UNSEALED block (its only known predecessor so far is `entry`; the
/// self back-edge isn't recorded until this block's own terminator
/// runs) -- Braun's `readVariableRecursive` never errors on an unsealed
/// block, it always places an operandless "incomplete" phi instead
/// (`read_variable_in`'s not-sealed branch). So with `undef_var =
/// Some(idx)`, variable `idx`'s read inside `loop` itself always succeeds; the
/// deferred error only surfaces once `loop` is SEALED (completing that
/// phi via `add_phi_operands`, which chases the entry-edge operand back
/// to the sealed, predecessor-less function entry) -- i.e. at
/// `seal_block(loop_bb)`, not at `use_int_var`.
fn build_loop(m: &Module<'_>, case: &GeneratedCase) -> Result<BuildOutcome, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("factorial", fn_ty, Linkage::External)?;
    let mut b = SsaBuilder::for_function(m, f)?;
    let entry = b.create_block("entry");
    let base = b.create_block("base");
    let loop_bb = b.create_block("loop");
    let exit = b.create_block("exit");

    let vars: Vec<_> = (0..case.var_count)
        .map(|i| b.declare_int_var::<i32, _>(format!("v{i}")))
        .collect();

    let mut b = b.switch_to_block(entry)?;
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let is_zero = b
        .ins()
        .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
    for (i, var) in vars.iter().enumerate() {
        if case.undef_var != Some(i) {
            b.def_int_var(*var, case.literals[i])?;
        }
    }
    let mut b = b.cond_br(is_zero, base, loop_bb)?;
    b.seal_block(base)?;

    let b = b.switch_to_block(base)?;
    let b = b.ret(1_i32)?;

    // `loop_bb` is unsealed here: every read below unconditionally
    // succeeds (operandless incomplete phi), REGARDLESS of `undef_var`.
    let mut b = b.switch_to_block(loop_bb)?;
    let reads: Vec<_> = vars
        .iter()
        .map(|var| b.use_int_var(*var))
        .collect::<Result<_, IrError>>()?;
    let done = b
        .ins()
        .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, reads[0], 0_i32, "done")?;
    for (i, var) in vars.iter().enumerate() {
        let next = b.ins().build_int_add(reads[i], case.literals[i], "next")?;
        b.def_int_var(*var, next)?;
    }
    let mut b = b.cond_br(done, exit, loop_bb)?;
    // The deferred undef error (if any) surfaces HERE: sealing `loop_bb`
    // completes its incomplete phis, chasing the entry-edge operand back
    // to the sealed, predecessor-less function entry.
    match (b.seal_block(loop_bb), case.undef_var) {
        (Ok(()), _) => {}
        (Err(IrError::SsaUseOfUndefinedVariable { .. }), Some(_)) => {
            return Ok(BuildOutcome::HitExpectedUndef);
        }
        (Err(other), _) => return Err(other),
    }

    let mut b = b.switch_to_block(exit)?;
    b.seal_block(exit)?;
    let read = b.use_int_var(vars[0])?;
    let b = b.ret(read)?;
    b.finish()?;
    Ok(BuildOutcome::Finished)
}

/// Builds (and, on the well-defined path, finishes) the `SwitchShared`
/// shape (`entry -> switch {case0 -> shared, default -> default_bb}`):
/// mirrors `switch_records_one_edge_per_case_occurrence`'s shape, minus
/// the duplicate-case multiplicity focus (this sweep only needs a real
/// multi-pred join fed by a `switch` terminator, the third CFG-edge kind
/// alongside `br`/`cond_br`). With `undef_var = Some(idx)`, variable
/// `idx`'s def before the switch is skipped, so `shared`'s read must error.
fn build_switch_shared(m: &Module<'_>, case: &GeneratedCase) -> Result<BuildOutcome, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let mut b = SsaBuilder::for_function(m, f)?;
    let entry = b.create_block("entry");
    let shared = b.create_block("shared");
    let default_bb = b.create_block("default_bb");

    let vars: Vec<_> = (0..case.var_count)
        .map(|i| b.declare_int_var::<i32, _>(format!("v{i}")))
        .collect();

    let mut b = b.switch_to_block(entry)?;
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    for (i, var) in vars.iter().enumerate() {
        if case.undef_var != Some(i) {
            b.def_int_var(*var, case.literals[i])?;
        }
    }
    let case0 = 0_i32;
    let mut b = b.switch(n, default_bb, [(case0, shared)])?;
    b.seal_block(shared)?;
    b.seal_block(default_bb)?;

    let b = b.switch_to_block(default_bb)?;
    let b = b.ret(0_i32)?;

    let mut b = b.switch_to_block(shared)?;
    let Ok(reads) = read_all_vars(&mut b, &vars, case.undef_var)? else {
        return Ok(BuildOutcome::HitExpectedUndef);
    };
    let b = b.ret(reads[0])?;
    b.finish()?;
    Ok(BuildOutcome::Finished)
}

fn build_case(m: &Module<'_>, case: &GeneratedCase) -> Result<BuildOutcome, IrError> {
    match case.shape {
        ShapeKind::StraightLine => build_straight_line(m, case),
        ShapeKind::Diamond => build_diamond(m, case),
        ShapeKind::Loop => build_loop(m, case),
        ShapeKind::SwitchShared => build_switch_shared(m, case),
    }
}

proptest! {
    /// Every well-defined (no `undef_var`) generated module -- across all
    /// four bounded shapes, 1..=4 vars, random small literals -- passes
    /// `verify_borrowed()`. This is the brief's "random reducible CFGs ...
    /// every generated module passes `verify_borrowed()`" requirement.
    #[test]
    fn every_well_defined_generated_module_verifies(
        case in generated_case_strategy(false)
    ) {
        let result = Module::with_new("prop-ssa-well-defined", |m| {
            match build_case(&m, &case)? {
                BuildOutcome::Finished => m.verify_borrowed(),
                // `generated_case_strategy(false)` never sets `undef_var`,
                // so every shape's undef arm is unreachable here.
                BuildOutcome::HitExpectedUndef => unreachable!(
                    "well-defined case {case:?} unexpectedly hit the undef arm"
                ),
            }
        });
        prop_assert!(
            result.is_ok(),
            "well-defined case {case:?} failed to build+verify: {result:?}"
        );
    }

    /// Every generated case -- whether well-defined OR strict-undef --
    /// either verifies successfully (well-defined) or cleanly hits
    /// `SsaUseOfUndefinedVariable` (strict-undef) -- never any other
    /// error, and a strict-undef case's necessarily-incomplete module is
    /// never handed to the verifier at all (it was never `finish()`ed,
    /// so there is no verifiable module to assert well-formedness of on
    /// that path -- the typed error at construction time IS the
    /// well-formedness guarantee). This is the brief's "strict-undef
    /// schedules yield SsaUseOfUndefinedVariable, never invalid IR"
    /// requirement: the two outcomes (`Finished` -> must verify,
    /// `HitExpectedUndef` -> must not reach `finish()`/`verify_borrowed`
    /// at all) are jointly exhaustive over every case this strategy can
    /// generate.
    #[test]
    fn undef_schedules_only_ever_yield_typed_undefined_variable_error(
        case in generated_case_strategy(true)
    ) {
        let is_undef_case = case.undef_var.is_some();
        let result: Result<Option<Result<(), IrError>>, IrError> =
            Module::with_new("prop-ssa-undef-sweep", |m| {
                match build_case(&m, &case)? {
                    BuildOutcome::Finished => Ok(Some(m.verify_borrowed())),
                    BuildOutcome::HitExpectedUndef => Ok(None),
                }
            });
        match (is_undef_case, result) {
            (true, Ok(None)) => {}
            (true, other) => prop_assert!(
                false,
                "strict-undef case {case:?} did not cleanly hit \
                 SsaUseOfUndefinedVariable before any finish/verify \
                 call: {other:?}"
            ),
            (false, Ok(Some(verify_result))) => prop_assert!(
                verify_result.is_ok(),
                "well-defined case {case:?} failed to verify: {verify_result:?}"
            ),
            (false, other) => prop_assert!(
                false,
                "well-defined case {case:?} failed to build: {other:?}"
            ),
        }
    }
}

/// Upstream-parity review regression (Braun alg. 3): the `same == None`
/// arm of `tryRemoveTrivialPhi` still runs `phi.replaceBy(same)` -- for a
/// poison variable, the layer must reroute every USER of the collapsing
/// phi to the poison constant before erasing it. The buggy path erased
/// without RAUW, leaving `%sum = add i32 %v, 1` where `%v` no longer
/// exists (rejected by `llvm-as` "use of undefined value" and by
/// llvmkit's own `UseBeforeDef` verifier rule).
#[test]
fn dead_block_poison_read_user_is_rerouted_to_poison() -> Result<(), IrError> {
    Module::with_new("ssa-dead-poison-user", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let dead = b.create_block("dead");
        let x = b.declare_int_var_poison::<i32, _>("x");

        let b = b.switch_to_block(entry)?;
        let b = b.ret(0_i32)?;

        // `dead` is unsealed at read time, so the read seeds an incomplete
        // phi that a REAL instruction then consumes.
        let mut b = b.switch_to_block(dead)?;
        let read = b.use_int_var(x)?;
        let sum = b.ins().build_int_add(read, 1_i32, "sum")?;
        let b = b.ret(sum)?;

        // finish() seals `dead` with zero predecessors: the incomplete phi
        // collapses through `undefined_phi_replacement`'s poison arm, which
        // must RAUW `%sum`'s operand to poison before erasing the phi.
        b.finish()?;

        let text = format!("{m}");
        assert!(
            text.contains("%sum = add i32 poison, 1"),
            "expected the phi's user to be rerouted to poison, got:\n{text}"
        );
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Two-block dead-cycle variant of
/// [`dead_block_poison_read_user_is_rerouted_to_poison`]: the poison twin
/// of `dead_cycle_phi_names_the_actual_strict_variable_not_same_type_poison`.
/// Sealing the cycle collapses the self-referential phi; its live user in
/// `loop1` must end up reading poison, and the module must verify.
#[test]
fn dead_cycle_poison_read_with_live_user_resolves_to_poison() -> Result<(), IrError> {
    Module::with_new("ssa-dead-cycle-poison-user", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let loop1 = b.create_block("loop1");
        let loop2 = b.create_block("loop2");
        let x = b.declare_int_var_poison::<i32, _>("x");

        let b = b.switch_to_block(entry)?;
        let b = b.ret(0_i32)?;

        let mut b = b.switch_to_block(loop1)?;
        let read = b.use_int_var(x)?;
        let _sum = b.ins().build_int_add(read, 1_i32, "sum")?;
        let b = b.br(loop2)?;

        let b = b.switch_to_block(loop2)?;
        let mut b = b.br(loop1)?;
        b.seal_block(loop2)?;
        b.seal_block(loop1)?;
        b.finish()?;

        let text = format!("{m}");
        assert!(
            text.contains("%sum = add i32 poison, 1"),
            "expected the dead-cycle phi's user to be rerouted to poison, got:\n{text}"
        );
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Upstream-parity review regression: `read_variable_in`'s single-pred
/// chase must TERMINATE on a closed cycle of sealed single-pred blocks
/// with no def (constructible only in an unreachable region). Braun's
/// recursion diverges on this input; the library routes it to the
/// undefined-read handling instead. Strict variable: typed error.
#[test]
fn sealed_single_pred_cycle_read_errors_for_strict_variable() -> Result<(), IrError> {
    Module::with_new("ssa-sealed-cycle-strict", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let cyc_a = b.create_block("cyc_a");
        let cyc_b = b.create_block("cyc_b");
        let exit = b.create_block("exit");
        let x = b.declare_int_var::<i32, _>("x");

        let b = b.switch_to_block(entry)?;
        let b = b.ret(0_i32)?;

        // Dead 2-cycle with an exit edge: `exit`'s one predecessor chain
        // is cyc_a -> cyc_b -> cyc_a -> ... with no def anywhere.
        let b = b.switch_to_block(cyc_a)?;
        let b = b.cond_br(true, cyc_b, exit)?;
        let b = b.switch_to_block(cyc_b)?;
        let mut b = b.br(cyc_a)?;
        b.seal_block(cyc_a)?;
        b.seal_block(cyc_b)?;
        b.seal_block(exit)?;

        let mut b = b.switch_to_block(exit)?;
        match b.use_int_var(x) {
            Err(IrError::SsaUseOfUndefinedVariable { variable, .. }) => {
                assert_eq!(variable, "x");
                Ok(())
            }
            Ok(_) => panic!("expected SsaUseOfUndefinedVariable, got a resolved read"),
            Err(other) => panic!("expected SsaUseOfUndefinedVariable, got {other:?}"),
        }
    })
}

/// Poison twin of [`sealed_single_pred_cycle_read_errors_for_strict_variable`]:
/// the terminating chase resolves the read to poison, the function
/// completes, and the module verifies.
#[test]
fn sealed_single_pred_cycle_read_resolves_poison_variable() -> Result<(), IrError> {
    Module::with_new("ssa-sealed-cycle-poison", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let cyc_a = b.create_block("cyc_a");
        let cyc_b = b.create_block("cyc_b");
        let exit = b.create_block("exit");
        let x = b.declare_int_var_poison::<i32, _>("x");

        let b = b.switch_to_block(entry)?;
        let b = b.ret(0_i32)?;

        let b = b.switch_to_block(cyc_a)?;
        let b = b.cond_br(true, cyc_b, exit)?;
        let b = b.switch_to_block(cyc_b)?;
        let mut b = b.br(cyc_a)?;
        b.seal_block(cyc_a)?;
        b.seal_block(cyc_b)?;
        b.seal_block(exit)?;

        let mut b = b.switch_to_block(exit)?;
        let read = b.use_int_var(x)?;
        let b = b.ret(read)?;
        b.finish()?;

        let text = format!("{m}");
        assert!(
            text.contains("ret i32 poison"),
            "expected the cycle read to resolve to poison, got:\n{text}"
        );
        m.verify_borrowed()?;
        Ok(())
    })
}
