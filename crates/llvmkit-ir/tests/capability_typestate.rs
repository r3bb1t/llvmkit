//! Capability-typed handles (D1, D8): a module's verification state decides the
//! capability of the handles it mints.
//!
//! llvmkit-specific: LLVM has no verification typestate and no capability on a
//! `Value *`, so nothing upstream can be ported here.

use core::any::TypeId;
use llvmkit_ir::{
    CapabilityOf, Dyn, DynBrand, IrBuilder, IrError, Linkage, Module, ModuleState, Mutable,
    NoFolder, ReadOnly, Unverified, Value, Verified,
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

/// A verified module mints `ReadOnly` globals, aliases, ifuncs, functions,
/// typed function facades (fixed-arity and variadic), block walks and comdats
/// — through `view`, `globals` and `comdat` — and what is read off them
/// (initializer, aliasee, resolver, value type, signature, parameters, an
/// intrinsic's descriptor and its overload types, attached comdat) keeps the
/// capability. Positive control: the same handles through the unverified
/// module are `Mutable`. llvmkit-specific (D1, D8).
#[test]
fn a_verified_modules_globals_functions_and_comdats_are_read_only() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let comdat = m.get_or_insert_comdat("c");
    let g = m
        .global_builder("g", i32_ty)
        .initializer(i32_ty.const_int(7_i32))
        .comdat(comdat)
        .build()
        .expect("g");
    let alias = m
        .alias_builder("a", i32_ty, m.view(g).as_global_constant_ptr())
        .build()
        .expect("a");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type(i32_ty, [i32_ty.as_type()]),
            Linkage::External,
        )
        .expect("f");
    let ifunc = m
        .ifunc_builder("i", m.ptr_type(0), m.view(f).as_global_constant_ptr())
        .build()
        .expect("i");
    let typed = m
        .add_typed_function::<i32, (i32,), _>("t", Linkage::External)
        .expect("t");
    let varargs = m
        .add_typed_varargs_function::<i32, (i32,), _>("v", Linkage::External)
        .expect("v");
    let abs = m
        .get_or_insert_intrinsic_declaration_by_name("llvm.abs.i32")
        .expect("llvm.abs.i32");

    let mutable = TypeId::of::<Mutable>();
    assert_eq!(capability(m.view(g)), mutable);
    assert_eq!(capability(m.view(alias)), mutable);
    assert_eq!(capability(m.view(ifunc)), mutable);
    assert_eq!(capability(m.view(f)), mutable);
    assert_eq!(capability(m.view(f).into_iter()), mutable);
    assert_eq!(capability(m.view(typed)), mutable);
    assert_eq!(capability(m.view(varargs)), mutable);
    assert_eq!(
        capability(m.view(abs).intrinsic_descriptor().expect("descriptor")),
        mutable
    );
    assert_eq!(capability(m.globals().next().expect("g")), mutable);
    assert_eq!(capability(m.comdat("c").expect("c")), mutable);

    let m = m.verify().expect("verifies");
    let read_only = TypeId::of::<ReadOnly>();
    let global = m.view(g);
    assert_eq!(capability(global), read_only);
    assert_eq!(capability(global.initializer().expect("init")), read_only);
    assert_eq!(capability(global.value_type()), read_only);
    assert_eq!(capability(global.comdat().expect("comdat")), read_only);
    assert_eq!(capability(m.globals().next().expect("g")), read_only);
    assert_eq!(capability(m.comdat("c").expect("c")), read_only);
    assert_eq!(capability(m.view(alias)), read_only);
    assert_eq!(capability(m.view(alias).aliasee()), read_only);
    assert_eq!(capability(m.view(ifunc)), read_only);
    assert_eq!(capability(m.view(ifunc).resolver()), read_only);
    assert_eq!(capability(m.view(ifunc).value_type()), read_only);
    let function = m.view(f);
    assert_eq!(capability(function), read_only);
    assert_eq!(capability(function.into_iter()), read_only);
    assert_eq!(capability(function.signature()), read_only);
    assert_eq!(capability(function.param(0).expect("param")), read_only);
    assert_eq!(
        capability(function.params().next().expect("param")),
        read_only
    );
    assert_eq!(
        capability(function.param(0).expect("param").parent_function()),
        read_only
    );
    assert_eq!(capability(m.view(typed)), read_only);
    assert_eq!(capability(m.view(typed).as_function()), read_only);
    assert_eq!(capability(m.view(varargs)), read_only);
    assert_eq!(capability(m.view(varargs).as_function()), read_only);
    let descriptor = m.view(abs).intrinsic_descriptor().expect("descriptor");
    assert_eq!(capability(descriptor.overloads()[0]), read_only);
    assert_eq!(capability(descriptor), read_only);
}

/// A value type is an operand, not something mutated: `global_builder`,
/// `alias_builder`, `ifunc_builder`, `add_global_uninitialized` and
/// `add_external_global` admit a `ReadOnly` type of their own module (minted
/// through the module view) and install the global, and still refuse a
/// `ReadOnly` type of another module that shares the brand with
/// `IrError::ForeignType`, installing nothing. llvmkit-specific (D1, D7).
#[test]
fn the_global_builders_admit_a_read_only_value_type_and_refuse_a_foreign_one() {
    let m = Module::dynamic("m");
    let other = Module::dynamic("other");
    let own = m.as_view().i32_type();
    let foreign = other.as_view().i32_type();
    assert_eq!(capability(own), TypeId::of::<ReadOnly>());
    let target = m
        .add_global("target", m.i32_type().const_int(0_i32))
        .expect("target");
    let pointer = m.view(target).as_global_constant_ptr();

    let refused = m.global_builder("g", foreign).build();
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.alias_builder("a", foreign, pointer).build();
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.ifunc_builder("i", foreign, pointer).build();
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.add_global_uninitialized("u", foreign);
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.add_external_global("e", foreign);
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    assert!(m.global("g").is_none() && m.alias("a").is_none() && m.ifunc("i").is_none());
    assert!(m.global("u").is_none() && m.global("e").is_none());

    let g = m.global_builder("g", own).build().expect("g");
    let a = m.alias_builder("a", own, pointer).build().expect("a");
    let i = m.ifunc_builder("i", own, pointer).build().expect("i");
    let u = m.add_global_uninitialized("u", own).expect("u");
    let e = m.add_external_global("e", own).expect("e");
    let i32_ty = m.i32_type().as_type();
    assert_eq!(m.view(g).value_type(), i32_ty);
    assert_eq!(m.view(a).value_type(), i32_ty);
    assert_eq!(m.view(i).value_type(), i32_ty);
    assert_eq!(m.view(u).value_type(), i32_ty);
    assert_eq!(m.view(e).value_type(), i32_ty);
}

/// Naming a function as a callee is not mutating it: a builder admits a
/// `ReadOnly` function of its own module and re-mints it, and still refuses a
/// `ReadOnly` function of another module that shares the brand — refused
/// before anything is built. llvmkit-specific (D1, D7).
#[test]
fn a_builder_admits_a_read_only_callee_and_refuses_a_foreign_one() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let callee = m
        .add_function_dyn(
            "callee",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("callee");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let read_only = m.view(callee).read_only();
    assert_eq!(capability(read_only), TypeId::of::<ReadOnly>());
    let other = Module::dynamic("other");
    let other_callee = other
        .add_function_dyn(
            "callee",
            other.function_type_no_parameters(other.i32_type()),
            Linkage::External,
        )
        .expect("other callee");
    let foreign = other.view(other_callee).read_only();

    let before = format!("{m}");
    let refused = b.call_dyn::<Dyn, _, _, _, _>(foreign, Vec::<Value<'_, DynBrand>>::new(), "bad");
    assert!(
        matches!(refused, Err(IrError::ForeignValueId)),
        "{refused:?}"
    );
    assert_eq!(format!("{m}"), before, "a refused callee must not mutate");

    b.call_dyn::<Dyn, _, _, _, _>(read_only, Vec::<Value<'_, DynBrand>>::new(), "r")
        .expect("a read-only callee of this module is admitted");
    let text = format!("{m}");
    assert!(text.contains("%r = call i32 @callee()"), "got:\n{text}");
}
