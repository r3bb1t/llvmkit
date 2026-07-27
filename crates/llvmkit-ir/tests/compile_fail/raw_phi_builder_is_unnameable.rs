//! llvmkit typestate compile-fail (Doctrine D1 / slice 7 "the break").
//!
//! Block arguments are the public way to author a phi: a branch carries its
//! successor's parameter values, so on the paths that use them the edge and its
//! incomings move together and cannot drift apart. That is a property of *those
//! paths*, not of the whole surface — `build_br` does not check arity against a
//! parameterised target, and whole-graph phi coherence stays `Module::verify()`'s
//! job. See `docs/type-safety-vs-llvm.md` §9 for the honest limit.
//!
//! What this fixture pins is narrower and absolute: the three marker-form raw
//! phi builders are not callable from another crate. They are `pub(crate)`
//! *and*, since the 0.1.0 freeze, `#[cfg(test)]` — their only callers were ever
//! the in-crate `phi_raw_tests` module, so in a dependent crate's build of
//! `llvmkit-ir` they do not exist at all. The error is `E0599` "no method
//! named", not the `E0624` "private method" this fixture asserted while they
//! were merely private.
//!
//! That is the stronger of the two claims. A private method still exists and a
//! later `pub` slip would silently expose it; a method compiled out cannot be
//! reached by any visibility mistake.

use llvmkit_ir::{Dyn, IRBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("c");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let bb = m.view(f).append_basic_block(&m, "bb");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    // None of the three exist in a non-test build of the crate. The public path
    // is `append_block_with_params` for the block, then `build_br_with_args` /
    // `build_cond_br_with_args` to carry the incomings along each edge.
    let _int = b.build_int_phi::<i32, _>("p");
    let _fp = b.build_fp_phi::<f64, _>("q");
    let _ptr = b.build_pointer_phi("r");
}
