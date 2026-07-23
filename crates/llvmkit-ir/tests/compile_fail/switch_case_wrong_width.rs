//! llvmkit-specific compile-fail (Doctrine D7/D4) for the typed `switch`
//! condition width (`SwitchInst<W>`), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: LLVM's verifier (`Verifier::visitSwitchInst`)
//! rejects a `switch` whose case value type disagrees with the condition's
//! type *at runtime*. llvmkit's typed `build_switch` pins the
//! condition width `W` into the type system: `SwitchInst::add_case` on a
//! width-`W` switch carries an `IntoIntValue<'ctx, W, B>` bound, so a
//! wrong-width case value cannot type-check.
//!
//! Locks the realistic "case literal is the wrong integer width" mistake: a
//! bare `5_i64` on a `W = i32` switch has no `IntoIntValue<'_, i32, _>` impl
//! (an `i64` only lifts to `i64`/`i128`, never narrows to `i32`), so rustc
//! surfaces the unsatisfied `IntoIntValue<'_, i32, _>` bound — an
//! llvmkit-authored trait bound, stable across rustc versions.

use llvmkit_ir::{Dyn, IRBuilder, IntValue, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();

        // `W` is inferred as `i32` from the typed condition.
        let cond: IntValue<i32> = f.param(0).unwrap().try_into().unwrap();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let (_sealed, switch) = b.build_switch(cond, dest_label, "").unwrap();

        // `5_i64` does not implement `IntoIntValue<'_, i32, _>`, so it cannot
        // be a case value on an `i32`-width switch: `.add_case` does not
        // type-check.
        let _ = switch.add_case(5_i64, dest_label);
    });
}
