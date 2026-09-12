//! A handle minted by one module is rejected when another module would store
//! its slot.
//!
//! Two `Module::dynamic` modules share the `DynBrand` brand, so a handle from
//! one type-checks as an argument to the other's APIs. Every value, constant
//! and type handle carries only an arena slot plus its owning module, and each
//! module owns its own arenas: a slot stored into the wrong module names a
//! different value or type there, or nothing. The module tag is the backstop
//! (D7), and each test here pins one family of entry points that must check it
//! before anything is looked up, stored or linked into a use list.
//!
//! **llvmkit-specific (D7).** Upstream has no arena slot to mistake: a
//! `Value *` or `Type *` is its own identity. The nearest upstream analogue is
//! the verifier's module-provenance check on *globals*, which rejects an
//! already-built reference rather than refusing to build it:
//! `lib/IR/Verifier.cpp::Verifier::visitGlobalValue` ("Global is referenced in
//! a different module!") and `Verifier::visitInstruction` ("Referencing global
//! in another module!"). For types there is none at all: upstream uniques types
//! per `LLVMContext`, so every module of one context shares them, while
//! llvmkit gives each module its own type arena.

use llvmkit_ir::{IrError, Module};

/// `GlobalBuilder::initializer` then `build()` rejects a foreign constant and
/// installs nothing.
///
/// No upstream counterpart: `GlobalVariable::GlobalVariable`
/// (`lib/IR/Globals.cpp`) takes the initializer as a `Constant *`.
#[test]
fn a_global_builder_rejects_an_initializer_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_zero = foreign.i32_type().const_int(0i32);
    let result = home
        .global_builder("g", home.i32_type().as_type())
        .initializer(foreign_zero)
        .build();
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert!(
        home.global("g").is_none(),
        "a rejected build must not install"
    );
}

/// `GlobalVariable::set_initializer` rejects a foreign constant and leaves the
/// existing initializer in place.
///
/// No upstream counterpart: `GlobalVariable::setInitializer`
/// (`lib/IR/Globals.cpp`) stores a `Constant *`.
#[test]
fn set_initializer_rejects_a_constant_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let global = home
        .add_global("g", home.i32_type().const_int(3i32))
        .expect("global");
    let before = format!("{home}");
    let result = home
        .view(global)
        .set_initializer(&home, foreign.i32_type().const_int(9i32));
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected store must not mutate"
    );
}

/// Regression guard for the site that already checked by hand, so migrating it
/// to the checked door cannot silently drop the check.
///
/// No upstream counterpart: `ValueAsMetadata::get` (`lib/IR/Metadata.cpp`)
/// takes a `Value *`.
#[test]
fn metadata_constant_still_rejects_a_constant_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let result = home.metadata_constant(foreign.i32_type().const_int(1i32));
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
}

/// `global_builder` rejects a value type from another module and installs
/// nothing: the type's slot would name a different type in this module's
/// type arena.
///
/// No upstream counterpart: `GlobalVariable::GlobalVariable`
/// (`lib/IR/Globals.cpp`) takes a `Type *`, uniqued per `LLVMContext`.
#[test]
fn a_global_builder_rejects_a_value_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let result = home
        .global_builder("g", foreign.i32_type().as_type())
        .build();
    assert!(matches!(result, Err(IrError::ForeignType)), "{result:?}");
    assert!(
        home.global("g").is_none(),
        "a rejected build must not install"
    );
}

/// `alias_builder(..).build()` rejects a value type from another module and
/// installs nothing, even with an aliasee this module owns.
///
/// No upstream counterpart: `GlobalAlias::create` (`lib/IR/Globals.cpp`)
/// takes a `Type *`, uniqued per `LLVMContext`.
#[test]
fn alias_builder_rejects_a_value_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let target = home
        .add_global("target", home.i32_type().const_int(0i32))
        .expect("target");
    let result = home
        .alias_builder("alias", foreign.i32_type().as_type(), home.view(target))
        .build();
    assert!(matches!(result, Err(IrError::ForeignType)), "{result:?}");
    assert!(
        home.alias("alias").is_none(),
        "a rejected build must not install"
    );
}

/// `ifunc_builder(..).build()` rejects a value type from another module and
/// installs nothing, even with a resolver this module owns.
///
/// No upstream counterpart: `GlobalIFunc::create` (`lib/IR/Globals.cpp`)
/// takes a `Type *`, uniqued per `LLVMContext`.
#[test]
fn ifunc_builder_rejects_a_value_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let resolver = home
        .add_global("resolver", home.i32_type().const_int(0i32))
        .expect("resolver");
    let result = home
        .ifunc_builder("ifunc", foreign.i32_type().as_type(), home.view(resolver))
        .build();
    assert!(matches!(result, Err(IrError::ForeignType)), "{result:?}");
    assert!(
        home.ifunc("ifunc").is_none(),
        "a rejected build must not install"
    );
}
