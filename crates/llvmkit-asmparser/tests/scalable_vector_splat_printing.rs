//! How a scalable vector splat prints: `splat (…)` for a splat of a
//! `ConstantInt` or a `ConstantFP`, and otherwise the expression
//! `ConstantVector::getSplat` builds for it, never an element list.
//!
//! Upstream builds a scalable splat as `shufflevector (insertelement (poison,
//! V, i64 0), poison, zeroinitializer)` (`ConstantVector::getSplat`'s scalable
//! branch, `llvm/lib/IR/Constants.cpp`), and `writeConstantInternal`
//! (`llvm/lib/IR/AsmWriter.cpp`) prints that expression as `splat (…)` only
//! when its splat value is a `ConstantInt` or a `ConstantFP`; any other splat
//! prints as the expression, as `test/Assembler/constant-splat.ll`'s
//! `@ret_scalable_vector_ptr` checks. llvmkit stores the splat as its one
//! element and prints it as upstream prints the expression. A scalable lane
//! count is a minimum, so no element list could spell one.
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

/// A scalable vector of uniform **pointers** prints as the expression
/// `getSplat` builds, as `test/Assembler/constant-splat.ll`'s
/// `@ret_scalable_vector_ptr` checks, and the expression read back is the
/// same splat.
#[test]
fn a_scalable_pointer_splat_prints_as_get_splats_expression() {
    let printed = round_trip(
        "@x = global i32 7\n\
         define <vscale x 4 x ptr> @f() {\n  \
           ret <vscale x 4 x ptr> splat (ptr @x)\n\
         }\n",
    );
    assert!(
        printed.contains(
            "ret <vscale x 4 x ptr> shufflevector (<vscale x 4 x ptr> insertelement \
             (<vscale x 4 x ptr> poison, ptr @x, i64 0), <vscale x 4 x ptr> poison, \
             <vscale x 4 x i32> zeroinitializer)"
        ),
        "{printed}"
    );
    assert_eq!(
        round_trip(&printed),
        printed,
        "the expression reads back as the splat"
    );
}

/// A scalable splat of `undef` — the other category `is_int_or_fp_splat_value`
/// excludes, and the exact shape `docs/future-work.md` recorded as printing
/// invalid IR — is no list at all: `LLParser`'s `t_ConstantSplat` arm builds it
/// with `ConstantVector::getSplat`, whose scalable branch answers an
/// `isa<UndefValue>` value with `UndefValue::get(VTy)`, the whole-vector
/// `undef`. llvmkit kept a list of `undef`s and printed it `splat (ptr undef)`
/// until `getSplat`'s canonicalisation moved to where a list is interned.
#[test]
fn a_scalable_undef_splat_is_the_whole_vector_undef() {
    let printed = round_trip(
        "define <vscale x 4 x ptr> @f() {\n  \
           ret <vscale x 4 x ptr> splat (ptr undef)\n\
         }\n",
    );
    assert!(
        printed.contains("ret <vscale x 4 x ptr> undef\n"),
        "{printed}"
    );
    assert!(!printed.contains("ptr undef, ptr undef"), "{printed}");
}

/// Integer and floating-point scalable splats print the `splat (…)`
/// shorthand, `writeConstantInternal`'s `isa<ConstantInt> ||
/// isa<ConstantFP>` case.
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

/// An all-zero scalable vector prints `zeroinitializer`: `getSplat`'s
/// scalable branch answers a null value with `ConstantAggregateZero::get`.
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
/// `writeConstantInternal` restricts the `splat (…)` shorthand to
/// `ConstantInt` and `ConstantFP`, and writes any other fixed
/// `ConstantVector` as its lanes.
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

/// A scalable splat uses its element through `getSplat`'s `insertelement`, as
/// upstream's `shufflevector` does, so `@x`'s uses are the same whether the
/// splat was written `splat (ptr @x)` or as the expression it prints as, and
/// the two spellings are one constant.
///
/// Ports the use structure `ConstantVector::getSplat`'s scalable branch
/// builds (`ConstantExpr::getInsertElement(PoisonV, V, …)`, then
/// `ConstantExpr::getShuffleVector(V, PoisonV, Zeros)`): `@x` is used by the
/// `insertelement`, and the `insertelement` by the `shufflevector`.
#[test]
fn a_scalable_splat_uses_its_element_through_its_insertelement() {
    let source = "@x = global i32 7\n\
                  define <vscale x 4 x ptr> @f() {\n  \
                    ret <vscale x 4 x ptr> splat (ptr @x)\n\
                  }\n";
    let uses = |text: &str| {
        let module = parser::parse_dynamic(text).expect("parses");
        let global = module.global("x").expect("@x");
        // `ptr @x` in operand position is the global's pointer constant.
        module
            .view(global)
            .as_global_constant_ptr()
            .as_erased()
            .num_uses()
    };
    // Upstream: `@x`'s one user is the `insertelement`.
    assert_eq!(uses(source), 1);
    let printed = round_trip(source);
    assert_eq!(uses(&printed), 1, "{printed}");

    let both = format!(
        "{source}define <vscale x 4 x ptr> @g() {{\n  ret <vscale x 4 x ptr> shufflevector \
         (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> poison, ptr @x, i64 0), \
         <vscale x 4 x ptr> poison, <vscale x 4 x i32> zeroinitializer)\n}}\n"
    );
    let module = parser::parse_dynamic(&both).expect("parses");
    let returned: Vec<String> = module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .map(|instruction| instruction.to_string())
        .collect();
    assert_eq!(returned.len(), 2, "{returned:?}");
    assert_eq!(returned[0], returned[1]);
    assert_eq!(uses(&both), 1, "one constant, so no second use");
}
