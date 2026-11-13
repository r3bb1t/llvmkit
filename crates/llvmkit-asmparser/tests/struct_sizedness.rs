//! A struct that holds one struct type twice is sized.
//!
//! `StructType::isSized` (`lib/IR/Type.cpp`) reads its memoised
//! `SCDB_IsSized` bit before its `Visited` set, and never removes a struct
//! from `Visited`; the bit is what makes the second visit of a finished
//! struct answer true. llvmkit's `struct_is_sized` had the set and not the
//! bit, so it took the second `%t0` of `{ %t0, %t0 }` for a cycle and called
//! the struct unsized, refusing `alloca`, `load`, `store` and `getelementptr`
//! of it with "Cannot allocate unsized type" and kin.
//!
//! Beside it, the scalable-type test the verifier asks of every global
//! (`Type::isScalableTy`, memoised upstream by `SCDB_ContainsScalableVector`),
//! which the verifier used to answer with a copy of its own.

use llvmkit_asmparser::parser;
use llvmkit_ir::{DynBrand, Module, Unverified};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}"),
    }
}

/// Parses `source`, verifies it, and checks the printed module parses back
/// to the same text.
fn round_trip(source: &str) -> String {
    let printed = format!("{}", parse(source));
    parse(&printed).verify().expect("the module verifies");
    let reprinted = format!("{}", parse(&printed));
    assert_eq!(printed, reprinted, "printing is a fixed point");
    printed
}

/// Ported from `test/Analysis/BasicAA/2003-07-03-BasicAACrash.ll`, whose
/// `RUN` line has `opt` read the file and exit 0: `%struct..RefRect` holds
/// `%struct..RefPoint` twice, and both `getelementptr`s index through it.
/// The fixture has no `CHECK` lines; what it pins is that the input is
/// accepted, which llvmkit refused at `34fdec9` with "base element of
/// getelementptr must be sized".
#[test]
fn basic_aa_crash_2003_07_03_parses() {
    let printed = round_trip(include_str!(
        "fixtures/upstream/Analysis/BasicAA/2003-07-03-BasicAACrash.ll"
    ));
    assert!(printed.contains(
        "%tmp.20311 = getelementptr %struct..RefRect, ptr null, i64 0, i32 1, i32 1, i32 2"
    ));
}

/// `{ %t0, %t0 }` is sized for each of the four instructions that refuse an
/// unsized type: `alloca`, `load`, `store` and `getelementptr`.
///
/// **No upstream counterpart** beyond the fixture above, which covers only
/// `getelementptr`: the search of `llvm/test` that found it found no file
/// pinning the other three on this shape. Each refused the module at
/// `34fdec9`.
#[test]
fn a_struct_holding_one_struct_twice_is_sized_for_every_memory_instruction() {
    let types = "%t0 = type { i8, i8 }\n%t1 = type { %t0, %t0 }\n";
    for body in [
        "  %a = alloca %t1\n",
        "  %v = load %t1, ptr %p\n",
        "  store %t1 zeroinitializer, ptr %p\n",
        "  %q = getelementptr %t1, ptr %p, i64 1, i32 1\n",
    ] {
        let source = format!("{types}define void @f(ptr %p) {{\n{body}  ret void\n}}\n");
        round_trip(&source);
    }
}

/// Ported from `test/Verifier/scalable-global-vars.ll`: each of its six
/// globals is refused with `Globals cannot contain scalable types`, naming the
/// global its `CHECK-NEXT: ptr @...` line names.
///
/// llvmkit's verifier stops at the first failure where upstream's reports all
/// six, so each global is verified in a module of its own beside the fixture's
/// type definitions; the expected names are read from the fixture's `CHECK`
/// lines. The sixth global, a struct of two `target("aarch64.svcount")`, is
/// the case the verifier's own copy of `isScalableTy` missed: it did not count
/// a target extension type whose layout is a scalable vector
/// (`Type::isScalableTargetExtTy`), and accepted the global.
#[test]
fn scalable_global_vars_are_refused() {
    let fixture = include_str!("fixtures/upstream/Verifier/scalable-global-vars.ll");
    let types: String = fixture
        .lines()
        .filter(|line| line.starts_with('%'))
        .map(|line| format!("{line}\n"))
        .collect();
    let expected: Vec<&str> = fixture
        .lines()
        .filter_map(|line| line.strip_prefix("; CHECK-NEXT: ptr @"))
        .collect();
    let globals: Vec<&str> = fixture
        .lines()
        .filter(|line| line.starts_with('@'))
        .collect();
    assert_eq!(globals.len(), expected.len(), "one CHECK pair per global");
    for (global, name) in globals.iter().zip(&expected) {
        let module = parse(&format!("{types}{global}\n"));
        let error = module.verify().expect_err("a scalable global is refused");
        let llvmkit_ir::IrError::VerifierFailure { message, .. } = error else {
            panic!("a verifier failure, not {error:?}");
        };
        assert_eq!(
            message,
            format!("Globals cannot contain scalable types (@{name})")
        );
    }
}
