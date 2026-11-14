//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Upstream's `IRBuilderBase::saveIP` on a builder with no insertion block
//! returns an empty `InsertPoint`, and `restoreIP` of one clears the builder's
//! position. llvmkit has no empty insert point: `save_insert_point` exists
//! only on a positioned builder (`S: Positioning`), so a snapshot always names
//! a block and `restore_insert_point` has no "empty" case to refuse at run
//! time. Primary error is rustc's stable `E0277`, the unsatisfied
//! `Unpositioned: Positioning` bound.

use llvmkit_ir::{Dyn, IrBuilder, Module};

fn main() {
    let mut module = Module::dynamic("c"); let mut key = module.layout_token();
    let _empty = IrBuilder::new_for::<Dyn>(&mut key).save_insert_point();
}
