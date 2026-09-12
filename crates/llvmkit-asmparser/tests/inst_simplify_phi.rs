//! Ports of the `undef` / `poison` phi blending cases from
//! `llvm/test/Transforms/InstSimplify/phi.ll` in the vendored `llvmorg-22.1.4`
//! tree.
//!
//! Those five functions are exactly the arms of `llvm::simplifyPHINode` whose
//! common value is **not** a constant — the ones
//! `constant_fold_instruction`'s `fold_phi` (the port of
//! `ConstantFoldInstruction`'s own `PHINode` arm) declines, because it bails at
//! the first non-constant incoming. A constant-valued fixture would pass on
//! either side of the change and pin nothing.
//!
//! Upstream's `RUN` line is `opt < %s -passes=instsimplify -S | FileCheck %s`,
//! so each fixture's `CHECK` block is the printed module after one
//! `instsimplify` run. The port drives the same source through this crate's
//! parser and `InstSimplifyPass`, then asserts on the printed function — the
//! same comparison, one function at a time because llvmkit runs a function pass
//! against a named function rather than a whole module.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::{Analyses, InstSimplifyPass, Module, run_function_pass};

/// Parse `src`, run `InstSimplifyPass` over `@<name>`, and print the module.
fn instsimplify(src: &str, name: &str) -> String {
    let module = Module::dynamic("instsimplify-phi");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    let function = module
        .function_dyn(name)
        .unwrap_or_else(|| panic!("@{name} is defined"));
    let verified = module.verify().expect("parsed IR verifies");
    let mut analyses = Analyses::new();
    let simplified =
        run_function_pass(InstSimplifyPass, verified, function, &mut analyses).expect("pass runs");
    format!("{}", simplified.verify().expect("output re-verifies"))
}

/// Port of `define i32 @poison(i1 %cond, i32 %v)` from
/// `test/Transforms/InstSimplify/phi.ll`. Upstream's `CHECK` block ends
/// `ret i32 [[V:%.*]]`: `phi i32 [%v, %A], [poison, %B]` folds to `%v`, gated
/// only by `valueDominatesPHI`.
#[test]
fn poison_incoming_folds_to_the_common_value() {
    let src = "\
define i32 @poison(i1 %cond, i32 %v) {
  br i1 %cond, label %A, label %B
A:
  br label %EXIT
B:
  br label %EXIT
EXIT:
  %w = phi i32 [%v, %A], [poison, %B]
  ret i32 %w
}
";
    let printed = instsimplify(src, "poison");
    assert!(
        !printed.contains("phi i32"),
        "the phi must fold away:\n{printed}"
    );
    assert!(printed.contains("ret i32 %v"), "{printed}");
}

/// Port of `define i32 @undef(i1 %cond, i32 %v)` from the same file. Upstream's
/// `CHECK` block **keeps** the phi — `[[W:%.*]] = phi i32 [ [[V:%.*]], [[A]] ],
/// [ undef, [[B]] ]` — because `simplifyPHINode` refuses to replace an `undef`
/// with a value it cannot prove is not poison, and a plain `i32` argument
/// carries no `noundef`.
#[test]
fn undef_incoming_does_not_fold_onto_a_possibly_poison_value() {
    let src = "\
define i32 @undef(i1 %cond, i32 %v) {
  br i1 %cond, label %A, label %B
A:
  br label %EXIT
B:
  br label %EXIT
EXIT:
  %w = phi i32 [%v, %A], [undef, %B]
  ret i32 %w
}
";
    let printed = instsimplify(src, "undef");
    assert!(
        printed.contains("phi i32 [ %v, %A ], [ undef, %B ]"),
        "the phi must survive:\n{printed}"
    );
}

/// Port of `define i8 @undef_poison(i1 %cond)`. Every incoming is `undef` or
/// `poison`, so `CommonValue` stays null and `HasUndefInput` wins:
/// `CHECK: ret i8 undef`.
#[test]
fn undef_and_poison_only_folds_to_undef() {
    let src = "\
define i8 @undef_poison(i1 %cond) {
  br i1 %cond, label %A, label %B
A:
  br label %EXIT
B:
  br label %EXIT
EXIT:
  %r = phi i8 [undef, %A], [poison, %B]
  ret i8 %r
}
";
    let printed = instsimplify(src, "undef_poison");
    assert!(printed.contains("ret i8 undef"), "{printed}");
}

/// Port of `define i8 @only_undef(i1 %cond)`: `CHECK: ret i8 undef`.
#[test]
fn only_undef_folds_to_undef() {
    let src = "\
define i8 @only_undef(i1 %cond) {
  br i1 %cond, label %A, label %B
A:
  br label %EXIT
B:
  br label %EXIT
EXIT:
  %r = phi i8 [undef, %A], [undef, %B]
  ret i8 %r
}
";
    let printed = instsimplify(src, "only_undef");
    assert!(printed.contains("ret i8 undef"), "{printed}");
}

/// Port of `define i8 @only_poison(i1 %cond)`: with no `undef` input,
/// `simplifyPHINode` answers `PoisonValue::get` — `CHECK: ret i8 poison`.
#[test]
fn only_poison_folds_to_poison() {
    let src = "\
define i8 @only_poison(i1 %cond) {
  br i1 %cond, label %A, label %B
A:
  br label %EXIT
B:
  br label %EXIT
EXIT:
  %r = phi i8 [poison, %A], [poison, %B]
  ret i8 %r
}
";
    let printed = instsimplify(src, "only_poison");
    assert!(printed.contains("ret i8 poison"), "{printed}");
}
