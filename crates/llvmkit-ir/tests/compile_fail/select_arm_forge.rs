//! A downstream crate must NOT be able to forge a typed handle through
//! `SelectArm::from_select_value` — the `SelectNarrow` evidence token has no
//! public constructor (private field, `pub(crate)` constructor). Mirrors the
//! `intrinsic_id_raw_constructor_private` capability-token fixture family.

use llvmkit_ir::SelectNarrow;

fn main() {
    // ERROR: no way to mint a SelectNarrow token outside llvmkit-ir.
    let _forged = SelectNarrow { _private: core::marker::PhantomData };
}
