//! Brands as *types*: the uniqueness registry behind
//! [`Module::branded`](llvmkit_ir::Module::branded) /
//! [`branded_once`](llvmkit_ir::Module::branded_once), the registry-exempt
//! [`DynBrand`], and the per-expansion-site
//! [`module_new!`](llvmkit_ir::module_new) macro.
//!
//! llvmkit-specific: LLVM's C++ `Module` has no compile-time identity at all —
//! a `Value *` from one module can be handed to another and the mistake shows
//! up as a miscompile. These tests lock the two halves of llvmkit's answer: the
//! brand type (compile time) and the [`ModuleId`](llvmkit_ir::ModuleId) tag
//! (run time), and in particular that the registry keeps at most one live
//! module per brand so the compile-time half can never be ambiguous.
//!
//! Every test declares its **own** brand type. The registry is process-global
//! and the test harness runs tests in parallel threads, so a shared brand would
//! make tests race each other rather than exercise the property under test.

use std::collections::HashSet;
use std::sync::{Arc, Barrier};

use llvmkit_ir::{
    Dyn, DynBrand, IntValue, IrBuilder, IrError, Linkage, Module, ModuleBrand, Unverified,
    module_new,
};

/// Declare a brand type exactly as a user would: a bare unit struct and the
/// empty impl. [`ModuleBrand`] demands nothing else — no derives — so every
/// test in this file doubles as proof that bare brands work.
macro_rules! brand {
    ($name:ident) => {
        struct $name;
        impl ModuleBrand for $name {}
    };
}

/// The headline of the relaxation, stated as its own test: a brand type that
/// implements nothing at all — not `Clone`, not `Debug` — builds, verifies,
/// and prints a module end-to-end.
#[test]
fn bare_brand_builds_a_module() {
    struct Bare;
    impl ModuleBrand for Bare {}

    let m = Module::branded::<Bare, _>("bare").expect("brand is free");
    let f = m
        .add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)
        .expect("declare");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::at_end(entry);
    let (lhs, rhs) = m.view(f).params();
    let sum = b.build_int_add(lhs, rhs, "sum").expect("add");
    b.build_ret(sum).expect("ret");
    let verified = m.verify().expect("verifies");
    assert!(format!("{verified}").contains("define i32 @add"));
}

// --------------------------------------------------------------------------
// Uniqueness: one live module per brand
// --------------------------------------------------------------------------

/// The core registry property: a brand type names *one* module at a time, so
/// a second claim fails while the incumbent is alive and succeeds once it dies.
#[test]
fn a_brand_admits_one_live_module_at_a_time() -> Result<(), IrError> {
    brand!(OneAtATime);

    let first = Module::branded::<OneAtATime, _>("first")?;
    assert!(
        matches!(
            Module::branded::<OneAtATime, _>("second"),
            Err(IrError::BrandInUse { .. })
        ),
        "a second live module must not share the brand",
    );

    drop(first);

    let second = Module::branded::<OneAtATime, _>("second")?;
    assert_eq!(second.name(), "second");
    Ok(())
}

/// Distinct brand types are independent registry keys.
#[test]
fn distinct_brands_coexist() -> Result<(), IrError> {
    brand!(Left);
    brand!(Right);

    let left = Module::branded::<Left, _>("left")?;
    let right = Module::branded::<Right, _>("right")?;
    assert_ne!(left.id(), right.id());
    Ok(())
}

/// `branded_once` retires its brand **permanently** on drop: no successor may
/// ever hold it, so an id minted by the dead module can never be replayed
/// against a live one even in principle.
#[test]
fn branded_once_retires_its_brand_permanently() -> Result<(), IrError> {
    brand!(RetiredForever);

    let once = Module::branded_once::<RetiredForever, _>("once")?;
    assert!(
        matches!(
            Module::branded::<RetiredForever, _>("nope"),
            Err(IrError::BrandInUse { .. })
        ),
        "while alive it is merely in use, not yet retired",
    );

    drop(once);

    // "Forever" is the whole point, so check it more than once.
    for _ in 0..3 {
        assert!(matches!(
            Module::branded::<RetiredForever, _>("nope"),
            Err(IrError::BrandRetired { .. })
        ));
        assert!(matches!(
            Module::branded_once::<RetiredForever, _>("nope"),
            Err(IrError::BrandRetired { .. })
        ));
    }
    Ok(())
}

/// The claim lives in a `BrandGuard` *field*, and every typestate transition
/// moves that field along — so `verify` / `unverify` neither release the brand
/// early nor release it twice.
#[test]
fn the_claim_rides_through_the_typestate_transitions() -> Result<(), IrError> {
    brand!(Typestate);

    let unverified = Module::branded::<Typestate, _>("ts")?;
    let verified = unverified.verify()?;
    assert!(
        matches!(
            Module::branded::<Typestate, _>("x"),
            Err(IrError::BrandInUse { .. })
        ),
        "verify() moved the module, not its claim",
    );

    let back = verified.unverify();
    assert!(matches!(
        Module::branded::<Typestate, _>("x"),
        Err(IrError::BrandInUse { .. })
    ));

    drop(back);
    let _reclaimed = Module::branded::<Typestate, _>("x")?;
    Ok(())
}

/// Leaking a module never releases its brand: the guard's `Drop` is the only
/// release path, so `mem::forget` is an implicit `branded_once`. Deterministic
/// and safe — and deliberately not recoverable, because a force-unregister API
/// would let two generations of storage share one brand type.
#[test]
fn leaking_a_module_consumes_its_brand_forever() {
    brand!(Leaked);

    let module = Module::branded::<Leaked, _>("leaked").expect("first claim");
    core::mem::forget(module);

    assert!(matches!(
        Module::branded::<Leaked, _>("again"),
        Err(IrError::BrandInUse { .. })
    ));
}

// --------------------------------------------------------------------------
// Threads and unwinding
// --------------------------------------------------------------------------

/// The registry is process-global: a brand released on one thread is claimable
/// on another.
#[test]
fn a_brand_freed_on_one_thread_is_claimable_on_another() {
    brand!(CrossThread);

    std::thread::spawn(|| {
        let module = Module::branded::<CrossThread, _>("a").expect("first claim");
        assert_eq!(module.name(), "a");
    })
    .join()
    .expect("first worker panicked");

    std::thread::spawn(|| {
        let module = Module::branded::<CrossThread, _>("b")
            .expect("the other thread's drop freed the brand");
        assert_eq!(module.name(), "b");
    })
    .join()
    .expect("second worker panicked");
}

/// Eight threads race for one brand and exactly one wins. The check-and-set is
/// a single map operation under the registry lock, so there is no window in
/// which two threads could both observe the brand as free.
#[test]
fn concurrent_claims_elect_exactly_one_winner() {
    brand!(Contended);
    const THREADS: usize = 8;

    let start = Arc::new(Barrier::new(THREADS));
    let tried = Arc::new(Barrier::new(THREADS));

    let workers: Vec<_> = (0..THREADS)
        .map(|_| {
            let start = Arc::clone(&start);
            let tried = Arc::clone(&tried);
            std::thread::spawn(move || {
                start.wait();
                let claimed = Module::branded::<Contended, _>("contended");
                let won = claimed.is_ok();
                // Hold the claim until every thread has had its turn, so the
                // losers really did race a *live* winner.
                tried.wait();
                drop(claimed);
                won
            })
        })
        .collect();

    let wins = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker panicked"))
        .filter(|won| *won)
        .count();
    assert_eq!(wins, 1, "exactly one of {THREADS} racing claims may win");
}

/// A panic while a branded module is live still frees the brand: the release
/// is a `Drop`, and `Drop` runs on unwind.
#[test]
fn an_unwind_releases_the_brand() {
    brand!(Unwound);

    // The panic message this prints on stderr is expected, not a failure.
    let outcome = std::panic::catch_unwind(|| {
        let _live = Module::branded::<Unwound, _>("unwound").expect("first claim");
        panic!("simulated failure with a live branded module");
    });
    assert!(outcome.is_err(), "the closure was supposed to panic");

    let recovered =
        Module::branded::<Unwound, _>("recovered").expect("unwinding must release the brand");
    assert_eq!(recovered.name(), "recovered");
}

// --------------------------------------------------------------------------
// DynBrand: registry-exempt
// --------------------------------------------------------------------------

/// `DynBrand` never touches the registry, so arbitrarily many modules of the
/// same brand type are live at once and they collect into a `Vec` — the case a
/// registry-claimed brand (`branded` / `module_new!`) structurally cannot
/// express, since it admits at most one live module per brand type.
#[test]
fn dyn_brand_modules_coexist_and_collect() {
    let modules: Vec<Module<DynBrand, Unverified>> = (0..16)
        .map(|i| Module::dynamic(format!("dyn{i}")))
        .collect();

    assert_eq!(modules.len(), 16);
    let ids: HashSet<_> = modules.iter().map(Module::id).collect();
    assert_eq!(ids.len(), 16, "every module still has its own runtime tag");
}

/// `dynamic` is infallible and repeatable — there is no state to exhaust.
#[test]
fn dynamic_never_fails() {
    for i in 0..64 {
        let module = Module::dynamic(format!("d{i}"));
        assert_eq!(module.name(), format!("d{i}"));
    }
}

// --------------------------------------------------------------------------
// module_new!: one brand per expansion site
// --------------------------------------------------------------------------

/// Two *sites* are two brands, so both modules are live simultaneously.
#[test]
fn module_new_mints_a_fresh_brand_per_expansion_site() -> Result<(), IrError> {
    let first = module_new!("first")?;
    let second = module_new!("second")?;

    assert_eq!(first.name(), "first");
    assert_eq!(second.name(), "second");
    assert_ne!(first.id(), second.id());
    Ok(())
}

/// One site is one brand however often it runs: the second loop iteration
/// collides with the first iteration's still-live module. This is the
/// documented sharp edge, and the reason loops want `DynBrand`.
#[test]
fn module_new_in_a_loop_reuses_one_brand() {
    let mut live = Vec::new();
    let mut collisions = 0usize;

    for i in 0..4 {
        match module_new!(format!("loop{i}")) {
            Ok(module) => live.push(module),
            Err(IrError::BrandInUse { .. }) => collisions += 1,
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    assert_eq!(live.len(), 1, "one expansion site is one brand");
    assert_eq!(collisions, 3);
}

/// Dropping between iterations *does* work — the brand is released each time.
/// Only holding them all at once collides.
#[test]
fn module_new_in_a_loop_succeeds_when_each_module_is_dropped() {
    for i in 0..4 {
        let module = module_new!(format!("serial{i}")).expect("previous iteration released it");
        assert_eq!(module.name(), format!("serial{i}"));
    }
}

// --------------------------------------------------------------------------
// The brand does not reach the output
// --------------------------------------------------------------------------

/// A brand is a compile-time device only: the same construction under a *named*
/// brand and under a macro-generated one prints byte-identical IR.
#[test]
fn a_named_brand_emits_byte_identical_ir() -> Result<(), IrError> {
    brand!(Emitting);

    fn build<'ctx, B: ModuleBrand + 'ctx>(
        module: &'ctx Module<B, Unverified>,
    ) -> Result<String, IrError> {
        let i32_ty = module.i32_type();
        let fn_ty = module.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = module.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = module.view(f).append_basic_block(module, "entry");
        let builder = IrBuilder::new_for::<Dyn>(module).position_at_end(entry);
        let n: IntValue<'_, i32, _> = module.view(f).param(0)?.try_into()?;
        let sum = builder.build_int_add(n, 1_i32, "sum")?;
        builder.build_ret(sum)?;
        Ok(format!("{module}"))
    }

    let generated = module_new!("same")?;
    let via_generated_brand = build(&generated)?;

    let named = Module::branded::<Emitting, _>("same")?;
    let via_named_brand = build(&named)?;

    assert_eq!(via_named_brand, via_generated_brand);
    assert!(
        via_named_brand.contains("%sum = add i32 %0, 1"),
        "{via_named_brand}"
    );
    Ok(())
}
