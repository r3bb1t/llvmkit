//! llvmkit-specific compile-fail (Doctrine D7 / no-silent-erasure) for the
//! 0.0.4 value-id family, not a 1:1 LLVM test port.
//!
//! The three *typed* ids ([`IntValueId`](llvmkit_ir::IntValueId) /
//! [`FloatValueId`](llvmkit_ir::FloatValueId) /
//! [`PointerValueId`](llvmkit_ir::PointerValueId)) lift into their handle at a
//! builder operand position ([`IntoIntValue`](llvmkit_ir::IntoIntValue) &c).
//! The *erased* [`ValueId`](llvmkit_ir::ValueId) deliberately does NOT: an
//! erased id carries no static value category, so lifting it silently to a
//! typed operand would re-introduce the very silent erasure the id family
//! forbids. Recovering a typed handle from an erased id must be *spelled*
//! ([`Module::try_view`](llvmkit_ir::Module::try_view)), never implicit.
//!
//! This locks the bound directly: a helper that requires
//! `IntoIntValue<'ctx, i32, B>` cannot be fed an erased `ValueId<B>`. The
//! primary error is our own unsatisfied `IntoIntValue` trait bound — an
//! llvmkit-authored bound, stable across rustc versions — not an incidental
//! inference failure (`B` is pinned by the argument's own brand).

use llvmkit_ir::{IntValue, IntoIntValue, Linkage, Module, ModuleBrand, ModuleRef, ValueId};

/// Only accepts operands that lift into an `IntValue<i32>`. The bound is on the
/// concrete `ValueId<B>` so `B` is fully determined by the caller — the sole
/// failing obligation is `ValueId<B>: IntoIntValue<'ctx, i32, B>`.
fn needs_int_operand<'ctx, B>(_id: ValueId<B>, _m: ModuleRef<'ctx, B>)
where
    B: ModuleBrand + 'ctx,
    ValueId<B>: IntoIntValue<'ctx, i32, B>,
{
}

fn main() {
    let m = Module::dynamic("c");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();

    // Mint an *erased* id from a typed handle.
    let a: IntValue<i32, _> = m.view(f).param(0).unwrap().try_into().unwrap();
    let erased: ValueId<_> = a.into_erased().id();

    // `ValueId<B>` has no `IntoIntValue` impl: erased -> typed must be
    // spelled with `try_view`, never lifted as an operand.
    needs_int_operand(erased, ModuleRef::from(m.as_view()));
}
