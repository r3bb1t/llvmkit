//! Renaming a global value through its module's value symbol table.
//!
//! Upstream renames a function, global variable, alias or ifunc with
//! `Value::setName`: `Value::setNameImpl` asks the file-static `getSymTab` for
//! the table to update — the parent `Module`'s, for a `GlobalValue` —
//! `ValueSymbolTable::createValueName` takes the new name or uniques a clash
//! through `ValueSymbolTable::makeUniqueName`, the old entry is removed, and
//! `Value::setName` then runs `Function::updateAfterNameChange` on a function.
//!
//! The one upstream unit test that renames a global,
//! `unittests/IR/ValueTest.cpp::TEST(ValueTest, setNameShrink)`, needs the
//! parser and is ported whole in `llvmkit-asmparser/tests/value_test.rs`.
//! (`rg -n "setName\(" llvm/unittests/IR/*.cpp` finds `F->setName` there and
//! otherwise only argument and local renames.) Every case below is therefore
//! llvmkit-specific, and each expected answer is derived by reading the
//! routine its doc comment names.

use llvmkit_ir::{HasName, IntrinsicId, IrError, Linkage, Module, SetName, Type};

/// A rename onto a name another global holds is uniqued with a dot and the
/// table's counter, and the counter is the *module's*: one table holds every
/// kind of global value, so a function renamed onto the same name takes the
/// next number. The names the renames give up are free again. Derived from
/// `ValueSymbolTable::createValueName` (a failed `vmap.insert` falls to
/// `makeUniqueName`) and `ValueSymbolTable::makeUniqueName` (a `GlobalValue`
/// appends `"."`, then `++LastUnique`). Positive control: a free name is taken
/// verbatim.
#[test]
fn a_taken_name_is_uniqued_with_a_dot_and_the_module_counter() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let a = m.add_global("a", i32_ty.const_int(0_i32)).expect("@a");
    let b = m.add_global("b", i32_ty.const_int(1_i32)).expect("@b");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(m.void_type()),
            Linkage::External,
        )
        .expect("@f");

    m.view(b).set_name(&m, "fresh");
    assert_eq!(m.view(b).name().as_deref(), Some("fresh"));

    m.view(b).set_name(&m, "a");
    assert_eq!(m.view(b).name().as_deref(), Some("a.1"));
    m.view(f).set_name(&m, "a");
    assert_eq!(m.view(f).name().as_deref(), Some("a.2"));
    assert_eq!(m.view(a).name().as_deref(), Some("a"));

    assert_eq!(m.global("a"), Some(a));
    assert_eq!(m.global("a.1"), Some(b));
    assert_eq!(m.function_dyn("a.2"), Some(f));
    // One table, but each lookup is `dyn_cast_or_null` to its own kind:
    // `Module::getNamedGlobal` does not answer for a function, nor
    // `Module::getFunction` for a global variable.
    assert_eq!(m.global("a.2"), None);
    assert_eq!(m.function_dyn("a"), None);
    assert_eq!(m.global("b"), None);
    assert_eq!(m.global("fresh"), None);
    assert_eq!(m.function_dyn("f"), None);
    m.add_global("b", i32_ty.const_int(2_i32))
        .expect("a name a rename gave up is free");
}

/// On a module whose target is NVPTX a clash is uniqued with the bare number:
/// `ValueSymbolTable::makeUniqueName` appends the dot only when
/// `!(M && M->getTargetTriple().isNVPTX())`, and `Triple::isNVPTX` is the
/// architecture `nvptx` or `nvptx64` — the first `-`-separated component, as
/// `Triple::Triple` parses it. The triple is read at each rename, so the same
/// module uses the dot before its triple is set (the positive control).
#[test]
fn an_nvptx_module_uniques_a_clash_without_the_dot() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    m.add_global("a", i32_ty.const_int(0_i32)).expect("@a");
    let b = m.add_global("b", i32_ty.const_int(1_i32)).expect("@b");
    let c = m.add_global("c", i32_ty.const_int(2_i32)).expect("@c");

    m.view(b).set_name(&m, "a");
    assert_eq!(m.view(b).name().as_deref(), Some("a.1"));

    m.set_target_triple("nvptx64-nvidia-cuda");
    m.view(c).set_name(&m, "a");
    assert_eq!(m.view(c).name().as_deref(), Some("a2"));
}

/// Renaming a global to the name it already has changes nothing and does not
/// advance the counter: `Value::setNameImpl` returns at `getName() == NameRef`
/// before `createValueName` could see a clash with the value itself.
#[test]
fn renaming_a_global_to_its_own_name_is_a_no_op() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let a = m.add_global("a", i32_ty.const_int(0_i32)).expect("@a");
    let b = m.add_global("b", i32_ty.const_int(1_i32)).expect("@b");

    m.view(a).set_name(&m, "a");
    assert_eq!(m.view(a).name().as_deref(), Some("a"));
    m.view(b).set_name(&m, "a");
    assert_eq!(m.view(b).name().as_deref(), Some("a.1"));
}

/// Clearing a global's name removes it from the table: `Value::setNameImpl`
/// removes the old `ValueName` and returns for an empty name. The global is
/// then unnamed — the printer numbers it — and its old name is free.
#[test]
fn clearing_a_global_name_frees_the_name_and_prints_a_slot() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let g = m.add_global("g", i32_ty.const_int(7_i32)).expect("@g");

    m.view(g).clear_name(&m);
    assert_eq!(m.view(g).name(), None);
    assert_eq!(m.global("g"), None);
    let text = format!("{m}");
    assert!(text.contains("@0 = global i32 7\n"), "{text}");
    m.add_global("g", i32_ty.const_int(8_i32))
        .expect("the cleared name is free");
}

/// Every kind of global value renames through the one table: the old name no
/// longer looks anything up, the new one finds the same value, and the
/// printer writes the new name at the definition and at each use.
#[test]
fn every_global_kind_renames_and_prints_under_its_new_name() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let g = m.add_global("g", i32_ty.const_int(0_i32)).expect("@g");
    let resolver = m
        .add_function_dyn(
            "resolver",
            m.function_type_no_parameters(ptr_ty),
            Linkage::External,
        )
        .expect("@resolver");
    let alias = m
        .alias_builder("al", i32_ty.as_type(), m.view(g))
        .build()
        .expect("@al");
    let ifunc = m
        .ifunc_builder(
            "ifn",
            i32_ty.as_type(),
            m.view(resolver).as_global_constant_ptr(),
        )
        .build()
        .expect("@ifn");

    m.view(g).set_name(&m, "g2");
    m.view(resolver).set_name(&m, "resolver2");
    m.view(alias).set_name(&m, "al2");
    m.view(ifunc).set_name(&m, "ifn2");

    assert_eq!(m.global("g"), None);
    assert_eq!(m.global("g2"), Some(g));
    assert_eq!(m.function_dyn("resolver"), None);
    assert_eq!(m.function_dyn("resolver2"), Some(resolver));
    assert_eq!(m.alias("al"), None);
    assert_eq!(m.alias("al2"), Some(alias));
    assert_eq!(m.ifunc("ifn"), None);
    assert_eq!(m.ifunc("ifn2"), Some(ifunc));

    let text = format!("{m}");
    assert!(text.contains("@g2 = global i32 0\n"), "{text}");
    assert!(text.contains("@al2 = alias i32, ptr @g2\n"), "{text}");
    assert!(
        text.contains("@ifn2 = ifunc i32, ptr @resolver2\n"),
        "{text}"
    );
    assert!(text.contains("declare ptr @resolver2()\n"), "{text}");
    for old in ["@g ", "@g\n", "@resolver(", "@resolver\n", "@al ", "@ifn "] {
        assert!(!text.contains(old), "{old:?} survived the rename:\n{text}");
    }
}

/// The erased handle renames a global as the typed one does — `Value::setName`
/// is one routine — and so does the `ptr @g` constant that stands for the
/// global, because upstream's `GlobalValue` *is* that constant
/// (`docs/divergences.md` D3). Reading the name through the constant answers
/// the global's name, as `getName` on `@g` does.
#[test]
fn a_global_renames_through_its_erased_handle_and_its_constant() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let g = m.add_global("g", i32_ty.const_int(0_i32)).expect("@g");

    m.view(g).as_erased().set_name(&m, "via_value");
    assert_eq!(m.view(g).name().as_deref(), Some("via_value"));

    let constant = m.view(g).as_global_constant_ptr();
    constant.set_name(&m, "via_constant");
    assert_eq!(m.view(g).name().as_deref(), Some("via_constant"));
    assert_eq!(constant.name().as_deref(), Some("via_constant"));
    assert_eq!(m.global("via_constant"), Some(g));
}

/// Renaming a function recomputes its intrinsic identity from the new name, as
/// `Value::setName` does through `Function::updateAfterNameChange`: a name
/// without the `llvm.` prefix is no intrinsic, and an intrinsic's name — on a
/// function with that intrinsic's signature — is that intrinsic, whether the
/// function began as a declaration of it or as a plain function. Upstream has
/// no unit test for it: `rg -n "updateAfterNameChange" llvm/unittests` is
/// empty.
#[test]
fn renaming_a_function_recomputes_its_intrinsic_identity() {
    let m = Module::dynamic("m");
    let trap = m
        .get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new())
        .expect("llvm.trap");
    assert_eq!(m.view(trap).intrinsic_id(), Some(IntrinsicId::TRAP));

    m.view(trap).set_name(&m, "not_trap");
    assert_eq!(m.view(trap).intrinsic_id(), None);
    m.view(trap).set_name(&m, "llvm.trap");
    assert_eq!(m.view(trap).intrinsic_id(), Some(IntrinsicId::TRAP));

    let plain = m
        .add_function_dyn(
            "plain",
            m.function_type_no_parameters(m.void_type()),
            Linkage::External,
        )
        .expect("@plain");
    assert_eq!(m.view(plain).intrinsic_id(), None);
    m.view(plain).set_name(&m, "llvm.debugtrap");
    assert_eq!(
        m.view(plain).intrinsic_id(),
        IntrinsicId::lookup("llvm.debugtrap")
    );
    assert!(m.view(plain).intrinsic_id().is_some());
}

/// A global variable that holds an intrinsic's name blocks the intrinsic's
/// declaration. Upstream's `Intrinsic::getOrInsertDeclaration` would
/// `cast<Function>` the variable `Module::getOrInsertFunction` returns for the
/// name — an assertion — and llvmkit refuses instead. While each kind of
/// global kept its own name map, llvmkit declared a second global under the
/// same name. Positive control: without the variable the declaration succeeds.
#[test]
fn a_global_variable_holding_an_intrinsic_name_blocks_its_declaration() {
    let free = Module::dynamic("free");
    free.get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new())
        .expect("a free name declares the intrinsic");

    let m = Module::dynamic("m");
    m.add_global("llvm.trap", m.i32_type().const_int(0_i32))
        .expect("a global variable may take an llvm. name");
    let before = format!("{m}");
    let refused =
        m.get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new());
    assert!(
        matches!(&refused, Err(IrError::DuplicateFunctionName { name }) if name == "llvm.trap"),
        "{refused:?}"
    );
    assert_eq!(
        format!("{m}"),
        before,
        "a refused declaration must not mutate"
    );
}
