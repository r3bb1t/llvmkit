//! llvmkit typestate compile-fail (Doctrine D1), Task 19.
//!
//! `SsaBuilder::def_int_var` (and its `use_*`/terminator siblings) is
//! defined ONLY in the `Positioned`-state impl block
//! (`crates/llvmkit-ir/src/ssa_builder.rs`) -- an `Unpositioned` builder
//! (fresh from `SsaBuilder::for_function`, or just returned by a
//! terminator/`switch_to_block`) exposes none of them. A caller must
//! `switch_to_block` first. Closest upstream behaviour: LLVM's
//! `IRBuilder` has no on-the-fly SSA layer to compare against; the
//! nearest functional idea is `IRBuilderBase::GetInsertBlock()` being
//! required before `CreateAlloca`/etc. can append anywhere, enforced
//! here at the type level instead of via a null/assert check.

use llvmkit_ir::{Linkage, Module};

fn main() {
    Module::with_new("ssa-def-unpositioned", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External).unwrap();
        let mut b = llvmkit_ir::SsaBuilder::for_function(&m, f).unwrap();
        let _entry = b.create_block("entry");
        let x = b.declare_int_var::<i32, _>("x");

        // `b` is `Unpositioned` here -- `def_int_var` does not exist on
        // this state, only on `Positioned`.
        b.def_int_var(x, 1_i32).unwrap();
    });
}
