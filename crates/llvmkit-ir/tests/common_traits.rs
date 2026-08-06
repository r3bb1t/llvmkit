//! The std-trait floor of the public surface (C-COMMON-TRAITS).
//!
//! **llvmkit-specific — no upstream counterpart.** C++ has no `Debug`, no
//! `Hash` derive and no `#[must_use]`; LLVM's nearest equivalents are the
//! `print`/`dump` methods, which are the analogue of [`Display`], not of
//! [`Debug`]. What is being locked here is a Rust API property: the types a
//! caller stores in their own structs must not be the reason those structs
//! cannot derive `Debug`, and the flagship type's `Debug` must summarise
//! rather than dump.
//!
//! [`Display`]: std::fmt::Display
//! [`Debug`]: std::fmt::Debug

use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;

use llvmkit_ir::{
    BlockId, Dyn, DynBrand, IrError, Linkage, Module, ModuleBrand, ValueCategory, ValueId,
    module_new,
};

/// Compile-time witness: the parameter implements [`Debug`].
fn assert_debug<T: Debug>() {}

/// llvmkit-specific: a user struct holding a `Module` must be able to derive
/// `Debug`. The flagship type had none, which made that impossible — the
/// whole point of this check is that it is a *compile-time* property, so the
/// derive below is the assertion.
#[test]
fn a_struct_holding_a_module_can_derive_debug() {
    #[derive(Debug)]
    struct Compilation {
        module: Module<DynBrand>,
        stage: &'static str,
    }

    let unit = Compilation {
        module: Module::dynamic("holder"),
        stage: "frontend",
    };
    let rendered = format!("{unit:?}");
    assert!(rendered.starts_with("Compilation {"), "{rendered}");
    assert!(rendered.contains("Module {"), "{rendered}");
    assert_eq!(unit.stage, "frontend");
    assert_eq!(unit.module.name(), "holder");
}

/// llvmkit-specific: `Debug` on a module is a **summary**, not the IR.
/// `Display` already prints the whole `.ll` file; forwarding to it would put
/// a module's entire body inside every `dbg!` and every failed `assert_eq!`
/// on a struct that happens to hold one. Locks the four summary fields and
/// that the verification typestate is named.
#[test]
fn module_debug_summarises_and_names_the_typestate() {
    let module = module_new!("summary").expect("fresh module");
    let i32_type = module.i32_type();
    let signature = module.function_type_no_parameters(i32_type);
    module
        .add_function_dyn("f", signature, Linkage::External)
        .expect("fresh name");
    module
        .add_global_uninitialized("g", i32_type.as_type())
        .expect("fresh name");

    let rendered = format!("{module:?}");
    assert!(rendered.contains("name: \"summary\""), "{rendered}");
    assert!(rendered.contains("functions: 1"), "{rendered}");
    assert!(rendered.contains("globals: 1"), "{rendered}");
    assert!(rendered.contains("state: \"Unverified\""), "{rendered}");
    // The summary must not be the IR: `Display` prints `declare i32 @f()`.
    assert!(module.to_string().contains("declare i32 @f()"));
    assert!(!rendered.contains("declare"), "{rendered}");

    let verified = module.verify().expect("empty bodies verify");
    assert!(
        format!("{verified:?}").contains("state: \"Verified\""),
        "{verified:?}"
    );
}

/// llvmkit-specific: the analysis/pass surface a caller composes with must be
/// `Debug` too — a pass that stores a manager, a pipeline, or a query
/// configuration in its own struct should not lose `#[derive(Debug)]` because
/// of it. Compile-time only: instantiating a whole pipeline is the pass
/// tests' job, not this one's.
#[test]
fn the_pass_and_analysis_surface_is_debug() {
    assert_debug::<llvmkit_ir::Analyses<'static, DynBrand>>();
    assert_debug::<llvmkit_ir::FunctionAnalysisManager<'static, DynBrand>>();
    assert_debug::<llvmkit_ir::ModuleAnalysisManager<'static, DynBrand>>();
    assert_debug::<llvmkit_ir::PassInstrumentationCallbacks>();
    assert_debug::<llvmkit_ir::PreservedAnalyses>();
    assert_debug::<ValueCategory>();
    assert_debug::<llvmkit_ir::DemandedBits>();
}

/// llvmkit-specific: a `Debug` that reaches through a `RefCell` must use
/// `try_borrow` — a caller printing a manager *while* a callback is firing is
/// the exact situation `Debug` exists for, and a panic there would be worse
/// than a degraded field. Locks the summary shape of the callback registry.
#[test]
fn instrumentation_debug_counts_callbacks_without_printing_them() {
    let callbacks = llvmkit_ir::PassInstrumentationCallbacks::new();
    assert!(format!("{callbacks:?}").contains("before_pass: 0"));

    callbacks.register_before_pass_callback(|_, _| true);
    callbacks.register_after_analysis_callback(|_| {});
    let rendered = format!("{callbacks:?}");
    assert!(rendered.contains("before_pass: 1"), "{rendered}");
    assert!(rendered.contains("after_analysis: 1"), "{rendered}");
    assert!(rendered.contains("after_pass: 0"), "{rendered}");
}

/// llvmkit-specific: `IrError` gains `Hash` beside the `Eq` it already had,
/// so a driver collecting failures across a module can de-duplicate them in a
/// `HashSet` instead of scanning a `Vec`. Its sibling
/// `llvmkit_asmparser::ParseError` already carried `Hash`.
#[test]
fn ir_errors_deduplicate_in_a_hash_set() {
    let mut seen: HashSet<IrError> = HashSet::new();
    assert!(seen.insert(IrError::InvalidIntegerWidth { bits: 0 }));
    assert!(!seen.insert(IrError::InvalidIntegerWidth { bits: 0 }));
    assert!(seen.insert(IrError::InvalidIntegerWidth { bits: 1 << 24 }));
    assert!(seen.insert(IrError::InvalidKeyword {
        target: "linkage",
        keyword: "nope".to_string(),
    }));
    assert_eq!(seen.len(), 3);
}

/// llvmkit-specific: the id family is ordered lexicographically over
/// `(ModuleId, slot)`, which is what lets a pass key a `BTreeMap` by an id
/// and get the same iteration order on every run — a `HashMap`'s order varies
/// per process. Upstream keys such maps by `Value*` pointer identity and
/// sorts by a slot number when determinism matters; llvmkit's id *is* the
/// slot, so the order is available directly.
#[test]
fn value_and_block_ids_key_a_btree_map_deterministically() {
    let module = module_new!("ordered").expect("fresh module");
    let i32_type = module.i32_type();
    let signature = module.function_type_no_parameters(i32_type);
    let function = module
        .add_function_dyn("f", signature, Linkage::External)
        .expect("fresh name");

    let entry = module
        .view(function)
        .append_basic_block(&module, "entry")
        .id();
    let second = module
        .view(function)
        .append_basic_block(&module, "second")
        .id();
    let third = module
        .view(function)
        .append_basic_block(&module, "third")
        .id();

    // Blocks were appended in order, so their slots are, and the ids sort the
    // same way. Insert out of order to prove the map does the sorting.
    let mut by_block: BTreeMap<BlockId<Dyn, _>, &str> = BTreeMap::new();
    by_block.insert(third, "third");
    by_block.insert(entry, "entry");
    by_block.insert(second, "second");
    assert_eq!(
        by_block.values().copied().collect::<Vec<_>>(),
        ["entry", "second", "third"]
    );

    // `cmp` agrees with `eq`, as `Ord` requires.
    assert_eq!(entry.cmp(&entry), std::cmp::Ordering::Equal);
    assert!(entry < second && second < third);

    fn assert_ord<T: Ord>() {}
    assert_ord::<ValueId<DynBrand>>();
    assert_ord::<BlockId<Dyn, DynBrand>>();
}

/// llvmkit-specific: ids from *different* modules still order totally — the
/// `ModuleId` tag is the leading key and is allocated from a monotone
/// counter, so a `BTreeSet` spanning several modules has a defined order
/// rather than an arbitrary one.
#[test]
fn ids_from_different_modules_order_by_module_first() {
    fn ids_of<B: ModuleBrand>(module: &Module<B>) -> Vec<ValueId<B>> {
        let i32_type = module.i32_type();
        (0..3)
            .map(|n| i32_type.const_int(n).as_erased().id())
            .collect()
    }

    let first = Module::dynamic("first");
    let second = Module::dynamic("second");
    let (early, late) = (ids_of(&first), ids_of(&second));
    assert!(first.id() < second.id(), "ids are allocated in order");

    for a in &early {
        for b in &late {
            assert!(a < b, "every id from the older module sorts first");
        }
    }
}
