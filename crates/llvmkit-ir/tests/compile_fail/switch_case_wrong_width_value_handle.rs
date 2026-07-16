//! llvmkit-specific compile-fail (Doctrine D7/D4) for the typed `switch`
//! condition width (`SwitchInst<W>`), proving the width gate specifically —
//! not a 1:1 LLVM test port.
//!
//! Companion to `switch_case_wrong_width.rs`: that fixture uses a bare
//! `5_i64` literal, which is `!IsValue`, so it would fail under *either* a
//! hypothetical `IsValue` bound or the real `IntoIntValue` bound. This
//! fixture instead passes an already-materialised `IntValue<'ctx, i64, B>`
//! **value handle**. That handle IS `IsValue<'ctx, B>` (so it would type-check
//! under the erased `IntDyn` switch's `add_case`, whose bound is `IsValue`),
//! but it is NOT `IntoIntValue<'ctx, i32, B>` (an `i64` handle lifts only to
//! `i64`/`i128`, never narrows to `i32`). So the *only* thing that can reject
//! it on a `W = i32` switch is the typed `add_case`'s `IntoIntValue<'ctx, W, B>`
//! width bound — which is exactly what this fixture locks: rustc surfaces the
//! unsatisfied `IntoIntValue<'_, i32, _>` bound, an llvmkit-authored trait
//! bound stable across rustc versions.

use llvmkit_ir::{IRBuilder, IntValue, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(
            void_ty.as_type(),
            [i32_ty.as_type(), i64_ty.as_type()],
            false,
        );
        let f = m
            .add_function::<(), _>("f", fn_ty, Linkage::External)
            .unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();

        // `W` is inferred as `i32` from the typed condition.
        let cond: IntValue<i32> = f.param(0).unwrap().try_into().unwrap();
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = b.build_switch_typed(cond, dest_label, "").unwrap();

        // A typed `i64` *value handle* (not a literal): the second parameter
        // narrowed to `IntValue<i64>`. It IS `IsValue`, but does not implement
        // `IntoIntValue<'_, i32, _>`, so it cannot be a case value on an
        // `i32`-width switch: `.add_case` does not type-check.
        let case: IntValue<i64> = f.param(1).unwrap().try_into().unwrap();
        let _ = switch.add_case(case, dest_label);
    });
}
