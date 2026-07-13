//! llvmkit typestate compile-fail (Doctrine D1 / slice 7 "the break").
//!
//! Since slice 7, block arguments (`IRBuilder::append_block_with_params` plus
//! the `build_*_with_args` branch builders) are the ONLY public way to author a
//! phi: a branch carries its successor's parameter values, so the edge and its
//! incomings move together and an incomplete or desynced phi is
//! *unrepresentable* through the public API rather than merely rejected at
//! `Module::verify()`.
//!
//! The raw typed-phi builders (`build_int_phi`, `build_fp_phi`,
//! `build_pointer_phi`) and the open-phi `add_incoming`/`finish` mutators are
//! now `pub(crate)`, so external code cannot name them. This fixture pins that
//! guarantee: the three marker-form `build_*_phi` builders are unnameable from
//! outside the crate.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m
            .add_function::<i32, _>("f", fn_ty, Linkage::External)
            .unwrap();
        let bb = f.append_basic_block(&m, "bb");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(bb);
        // The three marker-form `build_*_phi` builders are `pub(crate)` since
        // slice 7 -- none are nameable here. The public path is
        // `b.append_block_with_params(f, &[i32_ty], "bb")`.
        let _int = b.build_int_phi::<i32, _>("p");
        let _fp = b.build_fp_phi::<f64, _>("q");
        let _ptr = b.build_pointer_phi("r");
    });
}
