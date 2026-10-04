//! Capability-typed handles (D1, D8): a module's verification state decides the
//! capability of the handles it mints.
//!
//! llvmkit-specific: LLVM has no verification typestate and no capability on a
//! `Value *`, so nothing upstream can be ported here.

use core::any::TypeId;
use llvmkit_ir::{
    CapabilityOf, IrBuilder, IrError, Linkage, Module, ModuleState, Mutable, NoFolder, ReadOnly,
    Unverified, Verified,
};

fn capability_of<S: ModuleState>() -> TypeId {
    TypeId::of::<S::Capability>()
}

fn capability<T: CapabilityOf>(_: T) -> TypeId {
    TypeId::of::<T::Capability>()
}

/// An unverified module mints `Mutable` handles and a verified one `ReadOnly`
/// handles — the mapping every `Module::view` reads. llvmkit-specific (D8).
#[test]
fn a_modules_state_decides_the_capability_of_its_handles() {
    assert_eq!(capability_of::<Unverified>(), TypeId::of::<Mutable>());
    assert_eq!(capability_of::<Verified>(), TypeId::of::<ReadOnly>());
}

/// A type minted through a `ModuleView` is read-only, and a type reached from
/// it stays read-only — the module view is the read grade of a module,
/// whatever its state. Positive control: the unverified module itself mints
/// `Mutable` types. The view's constructor also takes the `Mutable` element as
/// an operand, which a capability-blind bound would refuse. llvmkit-specific
/// (D1, D8).
#[test]
fn a_type_minted_through_a_module_view_is_read_only() {
    let m = Module::dynamic("m");
    assert_eq!(capability(m.i32_type()), TypeId::of::<Mutable>());
    assert_eq!(capability(m.as_view().i32_type()), TypeId::of::<ReadOnly>());
    let array = m.as_view().array_type(m.i32_type(), 4);
    assert_eq!(capability(array), TypeId::of::<ReadOnly>());
    assert_eq!(capability(array.element()), TypeId::of::<ReadOnly>());
}

/// A verified module mints `ReadOnly` values, and a value's type and its
/// erased widening keep the capability. Positive control: the same id viewed
/// through the unverified module is `Mutable`. llvmkit-specific (D1, D8).
#[test]
fn a_verified_modules_values_are_read_only() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let sum = b
        .int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "s")
        .expect("add");
    assert_eq!(capability(m.view(sum)), TypeId::of::<Mutable>());
    b.ret(m.view(sum)).expect("ret");
    let m = m.verify().expect("verifies");
    let read = m.view(sum);
    assert_eq!(capability(read), TypeId::of::<ReadOnly>());
    assert_eq!(capability(read.ty()), TypeId::of::<ReadOnly>());
    assert_eq!(capability(read.as_erased()), TypeId::of::<ReadOnly>());
}

/// Reading a value as an operand is not mutating it: a builder admits a
/// `ReadOnly` constant of its own module and re-mints it, and still refuses a
/// `ReadOnly` constant of another module that shares the brand — refused
/// before anything is built. llvmkit-specific (D1, D7).
#[test]
fn a_builder_admits_a_read_only_operand_and_refuses_a_foreign_one() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let read_only = m.as_view().i32_type().const_int(1_i32);
    assert_eq!(capability(read_only), TypeId::of::<ReadOnly>());
    let other = Module::dynamic("other");
    let foreign = other.as_view().i32_type().const_int(1_i32);

    let before = format!("{m}");
    let refused = b.int_add::<i32, _, _, _>(foreign, read_only, "bad");
    assert!(
        matches!(refused, Err(IrError::ForeignValueId)),
        "{refused:?}"
    );
    assert_eq!(format!("{m}"), before, "a refused operand must not mutate");

    let sum = b
        .int_add::<i32, _, _, _>(read_only, i32_ty.const_int(2_i32), "s")
        .expect("a read-only operand of this module is admitted");
    b.ret(m.view(sum)).expect("ret");
    let text = format!("{m}");
    assert!(text.contains("%s = add i32 1, 2"), "got:\n{text}");
}
