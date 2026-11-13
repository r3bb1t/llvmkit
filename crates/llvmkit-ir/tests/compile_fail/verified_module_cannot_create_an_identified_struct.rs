//! llvmkit typestate compile-fail (D8) — no upstream counterpart: LLVM has no
//! verified-module typestate.
//!
//! `ModuleView::get_or_insert_named_struct` and
//! `ModuleView::anonymous_identified_struct` created a type through a read-only
//! view, and llvmkit prints every identified struct, so a verified module's
//! text changed after `verify()`. Only an unverified module creates one now.
use llvmkit_ir::Module;

fn main() {
    let m = Module::dynamic("m");
    let verified = m.verify().expect("verifies");
    let _ = verified.as_view().get_or_insert_named_struct("S");
    let _ = verified.as_view().anonymous_identified_struct();
    let _ = verified.get_or_insert_named_struct("S");
    let _ = verified.anonymous_identified_struct();
}
