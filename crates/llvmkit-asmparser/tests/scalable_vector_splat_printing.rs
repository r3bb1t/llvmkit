//! A scalable vector constant prints as `splat (…)`, never as an element list.
//!
//! **No upstream counterpart, and the reason is the whole point.**
//! `ConstantVector::get` (`llvm/lib/IR/Constants.cpp`) takes a fixed element
//! count, so LLVM cannot build a scalable vector constant from an element list
//! and never has to decide how to print one. llvmkit builds one deliberately —
//! it is how `constant_fold::vector_splat_constant` represents a scalable splat
//! — so the printer has to answer a question `AsmWriter.cpp` is never asked.
//!
//! The answer: a scalable vector's lane count is a *minimum*, not a count, so
//! an element list cannot describe its lanes one for one and LLVM would reject
//! the text. `splat (…)` is its only element-shaped spelling, whatever the
//! element category. `writeConstantInternal`'s `isa<ConstantInt> ||
//! isa<ConstantFP>` restriction on that shorthand is kept for *fixed* vectors,
//! where the element list is an equally legal fallback — so printed output for
//! everything LLVM can also build stays byte-identical.
//!
//! Every case round-trips (parse → **verify** → print → re-parse), because a
//! printer change producing text the parser could not read back would be no fix
//! at all.
//!
//! # Why these are function bodies and not globals
//!
//! `Verifier::visitGlobalVariable` rejects a scalable type in a global
//! ("Globals cannot contain scalable vectors"), and llvmkit ports that rule as
//! `GlobalScalableType`. A scalable vector constant can only appear as an
//! instruction operand, so that is where these put it — which also means every
//! case here passes the verifier rather than merely parsing.

use llvmkit_asmparser::parser;

/// Parse, verify, print, and re-parse the printed text — the full contract.
fn round_trip(source: &str) -> String {
    let module = match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("parses: {error:?}\n--- source ---\n{source}"),
    };
    let verified = module
        .verify()
        .unwrap_or_else(|e| panic!("verifies: {e:?}\n--- source ---\n{source}"));
    let printed = format!("{verified}");
    parser::parse_dynamic(&printed)
        .unwrap_or_else(|e| panic!("printed text re-parses: {e:?}\n--- printed ---\n{printed}"));
    printed
}

/// A scalable vector of uniform **pointers** — the category
/// `is_int_or_fp_splat_value` excludes, and one of the two that used to print
/// an element list.
#[test]
fn a_scalable_pointer_splat_prints_as_a_splat() {
    let printed = round_trip(
        "@x = global i32 7\n\
         define <vscale x 4 x ptr> @f() {\n  \
           ret <vscale x 4 x ptr> splat (ptr @x)\n\
         }\n",
    );
    assert!(
        printed.contains("ret <vscale x 4 x ptr> splat (ptr @x)"),
        "{printed}"
    );
    assert!(
        !printed.contains("ptr @x, ptr @x"),
        "a scalable vector has no element list to write:\n{printed}"
    );
}

/// A scalable vector of uniform `undef` — the other excluded category, and the
/// exact shape `docs/future-work.md` recorded as printing invalid IR.
#[test]
fn a_scalable_undef_splat_prints_as_a_splat() {
    let printed = round_trip(
        "define <vscale x 4 x ptr> @f() {\n  \
           ret <vscale x 4 x ptr> splat (ptr undef)\n\
         }\n",
    );
    assert!(
        printed.contains("ret <vscale x 4 x ptr> splat (ptr undef)"),
        "{printed}"
    );
    assert!(!printed.contains("ptr undef, ptr undef"), "{printed}");
}

/// Integer and floating-point scalable splats were already correct, and stay
/// so — this pins that the widened rule did not disturb them.
///
/// The float half is a `ret` rather than an `fadd` because the parser cannot
/// yet read a vector floating-point binary operator — the FP mirror of the
/// integer gap `int_binop_erased` closed. Recorded in
/// `docs/future-work.md`; it is unrelated to printing, and routing around it
/// keeps this case about the splat rule.
#[test]
fn scalable_int_and_float_splats_are_unchanged() {
    let integers = round_trip(
        "define <vscale x 4 x i32> @f(<vscale x 4 x i32> %a) {\n  \
           %i = add <vscale x 4 x i32> %a, splat (i32 7)\n  \
           ret <vscale x 4 x i32> %i\n\
         }\n",
    );
    assert!(integers.contains("splat (i32 7)"), "{integers}");

    let floats = round_trip(
        "define <vscale x 2 x double> @g() {\n  \
           ret <vscale x 2 x double> splat (double 1.500000e+00)\n\
         }\n",
    );
    assert!(
        floats.contains("ret <vscale x 2 x double> splat (double 1.500000e+00)"),
        "{floats}"
    );
}

/// An all-zero scalable vector prints `zeroinitializer`, which the printer
/// decides before it reaches the splat rule — so the rule did not take that
/// spelling away.
#[test]
fn a_scalable_zero_vector_still_prints_zeroinitializer() {
    let printed = round_trip(
        "define <vscale x 4 x i32> @f() {\n  \
           ret <vscale x 4 x i32> zeroinitializer\n\
         }\n",
    );
    assert!(
        printed.contains("ret <vscale x 4 x i32> zeroinitializer"),
        "{printed}"
    );
}

/// A **fixed** vector of uniform pointers keeps its element list.
///
/// This is the half that must *not* change: `writeConstantInternal` restricts
/// the `splat (…)` shorthand to `ConstantInt` and `ConstantFP`, and a fixed
/// vector has a legal element-list spelling, so widening the rule there would
/// diverge from upstream's bytes for a constant LLVM can also build.
#[test]
fn a_fixed_pointer_splat_keeps_its_element_list() {
    let printed = round_trip("@x = global i32 7\n@g = global <2 x ptr> <ptr @x, ptr @x>\n");
    assert!(
        printed.contains("@g = global <2 x ptr> <ptr @x, ptr @x>"),
        "a fixed vector keeps upstream's spelling:\n{printed}"
    );
}

/// A fixed vector of uniform integers keeps upstream's `splat (…)` shorthand,
/// which it already used.
#[test]
fn a_fixed_int_splat_keeps_the_upstream_shorthand() {
    let printed = round_trip("@g = global <4 x i32> <i32 7, i32 7, i32 7, i32 7>\n");
    assert!(
        printed.contains("@g = global <4 x i32> splat (i32 7)"),
        "{printed}"
    );
}
