//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! LLVM has no Rust `ModuleBrand`; the closest parity anchors are
//! `GlobalVariable::GlobalVariable` / `setInitializer` in `lib/IR/Globals.cpp`,
//! which assert initializer type compatibility, and `Verifier::visitGlobalValue`
//! in `lib/IR/Verifier.cpp`, which rejects globals referenced from a different
//! module at runtime. llvmkit makes the stronger module-provenance rule static:
//! a constant produced through one branded [`Module`] cannot initialize a global
//! in a differently branded [`Module`].

use llvmkit_ir::Module;

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let left_init = left.i32_type().const_int(1_i32);
        Module::with_new::<_, _, _>("right", |right| {
            let _ = right.add_global("g", left_init);
        });
    });
}
