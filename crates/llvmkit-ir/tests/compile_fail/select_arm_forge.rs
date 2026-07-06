//! A downstream crate must NOT be able to forge a typed handle through
//! `SelectArm::from_select_value` — the `SelectNarrow` evidence token has no
//! public constructor (private field, `pub(crate)` constructor). Mirrors the
//! `intrinsic_id_raw_constructor_private` capability-token fixture family
//! (D2 evidence-token capability, follows the `ValidatedStructValue`
//! precedent in `struct_schema.rs`).
//!
//! Closest upstream behaviour: `llvm/include/llvm/IR/IRBuilder.h::IRBuilderBase::CreateSelect`
//! (and the wider `CreateSelect` family) narrows a select's arm types at
//! runtime; llvmkit's `SelectArm` sealed-trait evidence proves arm-type
//! agreement at compile time instead, so there must be no way to mint that
//! evidence except through the real `IRBuilder::build_select` construction
//! path.

use llvmkit_ir::SelectNarrow;

fn main() {
    // ERROR: no way to mint a SelectNarrow token outside llvmkit-ir.
    let _forged = SelectNarrow { _private: core::marker::PhantomData };
}
