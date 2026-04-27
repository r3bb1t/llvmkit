//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Closest upstream behaviour: LLVM's `Verifier::visitPHINode` in
//! `lib/IR/Verifier.cpp` checks the incoming-block / predecessor
//! coherence at runtime; it has no notion of "phi has been
//! finalised". llvmkit's [`PhiInst::finish`] consumes the [`Open`]
//! phi and produces a [`Closed`] view; calling `add_incoming` on the
//! closed handle is a compile error.

use llvmkit_ir::{IRBuilder, IntValue, Linkage, Module};

fn main() {
    let m = Module::new("c");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m
        .add_function::<i32>("f", fn_ty, Linkage::External)
        .unwrap();
    let entry = f.append_basic_block("entry");
    let other = f.append_basic_block("other");
    let join = f.append_basic_block("join");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    b.build_br(join).unwrap();
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(other);
    b.build_br(join).unwrap();
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
    let phi = b.build_int_phi::<i32>("p").unwrap();
    let phi = phi
        .add_incoming(1_i32, entry)
        .unwrap()
        .add_incoming(2_i32, other)
        .unwrap()
        .finish();
    // After `.finish()`, the phi is `Closed` -- `add_incoming` is gone.
    let _ = phi.add_incoming(3_i32, entry);
    let _: IntValue<i32> = phi.as_int_value();
}
