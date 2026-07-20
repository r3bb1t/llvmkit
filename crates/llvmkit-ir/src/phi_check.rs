//! Shared phi/predecessor coherence core.
//!
//! [`check_phi_incoming`] is the single authoritative per-phi coherence
//! algorithm, factored out of the verifier so the parser can run the
//! *identical* check at end-of-function-parse without the two ever
//! drifting. It mirrors the phi checks in `Verifier::visitPHINode`
//! (`llvm/lib/IR/Verifier.cpp`): entry-count vs predecessor-count (with
//! multiplicity), predecessor membership, per-block multiplicity,
//! same-block/differing-value rejection, then incoming/result type
//! equality — **in that order**, so both callers report the same first
//! failure.
//!
//! The core is deliberately `self`-free and diagnostic-free: it returns a
//! [`PhiViolation`] carrying the raw `ValueId`/`TypeId` at fault, and each
//! caller maps the variant to its own rendered message (the verifier back
//! to its byte-identical `VerifierRule` diagnostics, the parser to a
//! source-located parse error).

use std::collections::HashMap;

use crate::function::FunctionValue;
use crate::instruction::InstructionKindData;
use crate::marker::Dyn;
use crate::module::{Module, ModuleBrand, Unverified};
use crate::r#type::{Type, TypeId};
use crate::value::{IsValue, ValueId, ValueKindData};

/// A single coherence violation for one phi, identified by the raw
/// `ValueId`/`TypeId` at fault. The verifier maps each variant back to
/// its existing `VerifierRule` diagnostic; the parser renders its own
/// source-located message. Check order (and therefore which variant a
/// malformed phi yields first) is fixed by [`check_phi_incoming`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhiViolation {
    /// Incoming-entry count differs from the block's predecessor count
    /// (both counted with edge multiplicity).
    CountMismatch { entries: usize, preds: usize },
    /// An incoming entry names a block that is not a predecessor at all.
    NotAPredecessor { block: ValueId },
    /// More incoming entries claim `block` than there are CFG edges from
    /// it into the phi's block.
    TooManyFromBlock { block: ValueId },
    /// Two incoming entries for the same `block` carry different values.
    AmbiguousValues { block: ValueId },
    /// An incoming value's type differs from the phi's result type.
    IncomingTypeMismatch { block: ValueId, value_ty: TypeId },
}

/// One phi's coherence against its block's predecessor multiset.
///
/// Single source of truth shared by the verifier (mapped to
/// `VerifierRule`) and the parser (mapped to a source-located parse
/// error) — the two cannot drift. `incoming` is `(value, predecessor
/// block)`; `preds` is the predecessor multiset (duplicate CFG edges
/// preserved); `value_ty_of` resolves an operand's type on demand.
///
/// Checks run in a fixed order — count, then predecessor membership,
/// then per-block multiplicity, then same-block/differing-value, then
/// incoming/result type — and the first failure is returned, so every
/// caller reports the same first violation.
pub(crate) fn check_phi_incoming(
    result_ty: TypeId,
    incoming: &[(ValueId, ValueId)],
    preds: &[ValueId],
    value_ty_of: &dyn Fn(ValueId) -> TypeId,
) -> Result<(), PhiViolation> {
    // Entry-count must equal predecessor-count (with multiplicity).
    if incoming.len() != preds.len() {
        return Err(PhiViolation::CountMismatch {
            entries: incoming.len(),
            preds: preds.len(),
        });
    }
    // Every incoming block must be a real predecessor of the phi's block
    // (with multiplicity).
    let mut pred_counts: HashMap<ValueId, u32> = HashMap::new();
    for p in preds {
        *pred_counts.entry(*p).or_insert(0) += 1;
    }
    let mut seen: HashMap<ValueId, ValueId> = HashMap::new();
    for &(val_id, block_id) in incoming {
        let Some(slot) = pred_counts.get_mut(&block_id) else {
            return Err(PhiViolation::NotAPredecessor { block: block_id });
        };
        if *slot == 0 {
            // Already exhausted -- more incoming than CFG edges from this
            // block.
            return Err(PhiViolation::TooManyFromBlock { block: block_id });
        }
        *slot -= 1;
        // Duplicate predecessor with differing values.
        if let Some(prev) = seen.insert(block_id, val_id)
            && prev != val_id
        {
            return Err(PhiViolation::AmbiguousValues { block: block_id });
        }
        // Incoming-value type must match phi result type.
        let val_ty = value_ty_of(val_id);
        if val_ty != result_ty {
            return Err(PhiViolation::IncomingTypeMismatch {
                block: block_id,
                value_ty: val_ty,
            });
        }
    }
    Ok(())
}

/// A phi-coherence failure surfaced across the crate boundary.
///
/// Internal contract for llvmkit-asmparser; not public API, may change
/// without notice.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct PhiCoherenceError {
    /// Arena id of the offending phi instruction, so the caller can anchor a
    /// diagnostic at that phi regardless of whether it is named or numbered.
    pub phi_id: crate::value::ValueId,
    /// Rendered coherence-failure message.
    pub message: String,
}

/// Render a [`PhiViolation`] to its human-readable message. Shared by the
/// parser's end-of-function coherence check and the pass-side
/// [`FnReshape::insert_phi`](crate::FnReshape::insert_phi), so the two report
/// the same diagnostic for the same failure and cannot drift.
pub(crate) fn render_phi_violation<'ctx, B: ModuleBrand>(
    violation: &PhiViolation,
    result_ty: TypeId,
    module: &Module<'ctx, B, Unverified>,
) -> String {
    let ctx = module.core_ref().context();
    match violation {
        PhiViolation::CountMismatch { entries, preds } => {
            format!("phi has {entries} incoming entries but block has {preds} predecessors")
        }
        PhiViolation::NotAPredecessor { block } => format!(
            "phi incoming block %{} is not a predecessor",
            ctx.block_diag_name(*block)
        ),
        PhiViolation::TooManyFromBlock { block } => format!(
            "phi has too many incoming entries from block %{}",
            ctx.block_diag_name(*block)
        ),
        PhiViolation::AmbiguousValues { block } => format!(
            "phi has multiple entries for block %{} with different values",
            ctx.block_diag_name(*block)
        ),
        PhiViolation::IncomingTypeMismatch { block, value_ty } => format!(
            "phi expects {} but incoming from %{} is {}",
            Type::new(result_ty, module.module_ref()),
            ctx.block_diag_name(*block),
            Type::new(*value_ty, module.module_ref()),
        ),
    }
}

/// Run the shared phi-coherence check over every phi in `function`,
/// recomputing the CFG predecessor multiset from the current
/// terminators.
///
/// Internal contract for llvmkit-asmparser; not public API, may change
/// without notice. The parser calls this at end-of-function-parse so it
/// applies the exact same coherence algorithm as the verifier. Returns
/// the first violation encountered (block order, then phi order).
#[doc(hidden)]
pub fn check_function_phi_coherence<'ctx, B: ModuleBrand>(
    module: &Module<'ctx, B, Unverified>,
    function: FunctionValue<'ctx, Dyn, B>,
) -> Result<(), PhiCoherenceError> {
    let ctx = module.core_ref().context();

    // Predecessor multiset per block: walk every block's terminator
    // successors and invert the edges (duplicate CFG edges preserved so
    // multiplicity matches the verifier's map).
    let mut predecessors: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    for block in function.basic_blocks() {
        let block_id = block.as_value().id;
        for succ in crate::cfg::block_successors(&block) {
            predecessors
                .entry(succ.as_value().id)
                .or_default()
                .push(block_id);
        }
    }

    let value_ty_of = |id: ValueId| ctx.value_data(id).ty;

    for block in function.basic_blocks() {
        let block_id = block.as_value().id;
        let preds: &[ValueId] = predecessors
            .get(&block_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        for inst in block.instructions() {
            // Phi nodes are grouped at the top of the block; stop at the
            // first non-phi.
            let phi = match &inst.as_value().data().kind {
                ValueKindData::Instruction(i) => match &i.kind {
                    InstructionKindData::Phi(p) => p,
                    _ => break,
                },
                _ => break,
            };
            let result_ty = inst.ty().id;
            let incoming: Vec<(ValueId, ValueId)> = phi
                .incoming
                .borrow()
                .iter()
                .map(|(v, b)| (v.get(), *b))
                .collect();
            if let Err(violation) = check_phi_incoming(result_ty, &incoming, preds, &value_ty_of) {
                return Err(PhiCoherenceError {
                    phi_id: inst.id(),
                    message: render_phi_violation(&violation, result_ty, module),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PhiViolation, check_phi_incoming};
    use crate::r#type::TypeId;
    use crate::value::ValueId;

    fn vid(n: usize) -> ValueId {
        ValueId::from_index(n)
    }

    fn tid(n: usize) -> TypeId {
        TypeId::from_index(n)
    }

    #[test]
    fn count_mismatch_reported_first() {
        let a = vid(1);
        let v = vid(2);
        let ty = tid(1);
        let ty_of = |_id: ValueId| ty;
        // One incoming entry, two predecessors.
        let r = check_phi_incoming(ty, &[(v, a)], &[a, a], &ty_of);
        assert!(matches!(
            r,
            Err(PhiViolation::CountMismatch {
                entries: 1,
                preds: 2
            })
        ));
    }

    #[test]
    fn incoming_block_not_a_predecessor() {
        let a = vid(1);
        let b = vid(3);
        let v = vid(2);
        let ty = tid(1);
        let ty_of = |_id: ValueId| ty;
        // Incoming claims block B, but only A is a predecessor.
        let r = check_phi_incoming(ty, &[(v, b)], &[a], &ty_of);
        assert!(matches!(r, Err(PhiViolation::NotAPredecessor { block }) if block == b));
    }

    #[test]
    fn too_many_incoming_from_one_block() {
        let a = vid(1);
        let b = vid(2);
        let v = vid(3);
        let ty = tid(1);
        let ty_of = |_id: ValueId| ty;
        // Preds [A, B] (count 2); both incoming entries claim A, so the
        // second exhausts A's slot -> too-many. (The count check passes:
        // 2 == 2, so this genuinely reaches the multiplicity check. Note
        // the brief's literal example preds=[A,A]/incoming=[(v,A)x3] would
        // trip the count check first, per the fixed check order.)
        let r = check_phi_incoming(ty, &[(v, a), (v, a)], &[a, b], &ty_of);
        assert!(matches!(r, Err(PhiViolation::TooManyFromBlock { block }) if block == a));
    }

    #[test]
    fn ambiguous_values_for_same_block() {
        let a = vid(1);
        let v1 = vid(2);
        let v2 = vid(3);
        let ty = tid(1);
        let ty_of = |_id: ValueId| ty;
        // Preds [A, A]; two entries from A carrying different values.
        let r = check_phi_incoming(ty, &[(v1, a), (v2, a)], &[a, a], &ty_of);
        assert!(matches!(r, Err(PhiViolation::AmbiguousValues { block }) if block == a));
    }

    #[test]
    fn incoming_type_mismatch() {
        let a = vid(1);
        let v = vid(2);
        let result_ty = tid(1);
        let value_ty = tid(2);
        // The incoming value has a different type from the phi result.
        let ty_of = |_id: ValueId| value_ty;
        let r = check_phi_incoming(result_ty, &[(v, a)], &[a], &ty_of);
        assert!(matches!(
            r,
            Err(PhiViolation::IncomingTypeMismatch { block, value_ty: got })
                if block == a && got == value_ty
        ));
    }

    #[test]
    fn valid_including_same_value_duplicate_over_multi_edge() {
        let a = vid(1);
        let v = vid(2);
        let ty = tid(1);
        let ty_of = |_id: ValueId| ty;
        // Preds [A, A] (a legal multi-edge, e.g. a switch with two cases
        // to the phi's block); a single value repeated once per edge is
        // well-formed.
        let r = check_phi_incoming(ty, &[(v, a), (v, a)], &[a, a], &ty_of);
        assert!(r.is_ok());
    }
}
