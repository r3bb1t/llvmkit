//! Ports `test/Transforms/InstSimplify/ConstProp/loads.ll` whole, by upstream's
//! pipeline: parse the fixture, set the layout each `RUN` line's `-data-layout`
//! sets, run `InstSimplifyPass` over every function, and compare the printed
//! module with the fixture's own `CHECK`, `LE` and `BE` lines.
//!
//! **One fixture is left out, and nothing else.**
//! `tests/fixtures/upstream/Transforms/InstSimplify/ConstProp/loads.ll` is
//! upstream's file but for `@g9` and `@test_array_of_zero_size_array`:
//! `[4294967295 x [0 x i32]] zeroinitializer` costs one `ValueSlot` per element
//! here (`docs/divergences.md` D28), so parsing it is not attempted. The copy
//! says so in its header too.
//!
//! The file also holds the other fixtures that drive the same routines, and the
//! layouts none of them reaches.

pub mod support;

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::{Analyses, DataLayout, InstSimplifyPass, Module, run_function_pass};
use support::{check_directives, read_check_directives_with_prefixes};

const LOADS: &str = include_str!("fixtures/upstream/Transforms/InstSimplify/ConstProp/loads.ll");
const ALLONES: &str =
    include_str!("fixtures/upstream/Transforms/InstSimplify/ConstProp/allones.ll");
const GEP_ALIAS_GEP_LOAD: &str =
    include_str!("fixtures/upstream/Transforms/InstSimplify/ConstProp/gep-alias-gep-load.ll");
const BITCAST_VECTOR_FOLD: &str =
    include_str!("fixtures/upstream/Transforms/InstSimplify/bitcast-vector-fold.ll");
const CONSTANT_FOLD_INTTOPTR_ADD: &str =
    include_str!("fixtures/upstream/Transforms/InstSimplify/constant-fold-inttoptr-add.ll");

/// One `RUN` line of a fixture: the layout its `-data-layout` sets, if it has
/// one, and its `--check-prefixes`, `CHECK` if it has none.
struct RunLine<'a> {
    data_layout: Option<&'a str>,
    prefixes: Vec<&'a str>,
}

/// The fixture's `RUN` lines, so neither the layouts nor the prefixes are
/// retyped here.
fn run_lines(source: &str) -> Vec<RunLine<'_>> {
    source
        .lines()
        .filter(|line| line.starts_with("; RUN:"))
        .map(|line| RunLine {
            data_layout: line
                .split_once("-data-layout=\"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(layout, _)| layout),
            prefixes: line
                .split_once("--check-prefixes=")
                .map_or(vec!["CHECK"], |(_, prefixes)| prefixes.split(',').collect()),
        })
        .collect()
}

/// `opt -data-layout=... -passes=instsimplify -S`. `opt` asks for the layout
/// before the parser reads a function body, which is what gives a `load` with
/// no `align` its alignment, so the layout goes on the module first.
fn instsimplify(source: &str, data_layout: Option<&str>) -> String {
    let module = Module::dynamic("instsimplify");
    if let Some(layout) = data_layout {
        module.set_data_layout(DataLayout::parse(layout).expect("the RUN line's layout parses"));
    }
    Parser::new(source.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    let functions: Vec<_> = module
        .as_view()
        .functions()
        .filter_map(|function| function.name())
        .map(|name| {
            module
                .function_dyn(&name)
                .unwrap_or_else(|| panic!("@{name} is defined"))
        })
        .collect();
    let mut verified = module.verify().expect("parsed IR verifies");
    for function in functions {
        let mut analyses = Analyses::new();
        verified = run_function_pass(InstSimplifyPass, verified, function, &mut analyses)
            .expect("pass runs")
            .verify()
            .expect("output re-verifies");
    }
    format!("{verified}")
}

/// Every `RUN` line of `source`, each with its own prefixes.
fn check_every_run_line(source: &str) {
    for run in run_lines(source) {
        check_directives(
            &instsimplify(source, run.data_layout),
            &read_check_directives_with_prefixes(source, &run.prefixes),
        );
    }
}

/// Ports `test/Transforms/InstSimplify/ConstProp/loads.ll` but for
/// `test_array_of_zero_size_array`, on both of its `RUN` lines: `e-` with
/// `CHECK,LE`, and `E-` with `CHECK,BE`.
#[test]
fn loads_ll_passes_on_both_run_lines() {
    assert_eq!(run_lines(LOADS).len(), 2);
    check_every_run_line(LOADS);
}

/// Ports `test/Transforms/InstSimplify/ConstProp/allones.ll` at the routine.
/// Its `RUN` line is `early-cse`, which llvmkit has not got. The folds are
/// `ConstantFoldLoadFromConstPtr`'s, which `EarlyCSE` reaches through
/// `simplifyInstruction`, so this drives `InstSimplifyPass` over the same text
/// and compares the same `CHECK` lines.
#[test]
fn allones_ll_folds_at_the_load_routine() {
    check_every_run_line(ALLONES);
}

/// Ports `test/Transforms/InstSimplify/ConstProp/gep-alias-gep-load.ll`:
/// `getUnderlyingObject` and `stripAndAccumulateConstantOffsets` step through a
/// non-interposable alias, and stop at a `weak` one and at a `weak` constant.
#[test]
fn gep_alias_gep_load_ll_steps_through_an_alias() {
    check_every_run_line(GEP_ALIAS_GEP_LOAD);
}

/// Ports `test/Transforms/InstSimplify/bitcast-vector-fold.ll`, the fixture of
/// `FoldBitCast`'s vector arms. `FoldReinterpretLoadFromConst` reaches them for
/// a vector load, and `loads.ll` has only the one, so they are driven here. Its
/// second `RUN` line sets two hidden options that change how a splat is stored
/// upstream and no printed line. llvmkit has no option surface, so that line
/// runs the same pipeline as the first, against the same `CHECK` lines.
#[test]
fn bitcast_vector_fold_ll_passes() {
    check_every_run_line(BITCAST_VECTOR_FOLD);
}

/// Ports `test/Transforms/InstSimplify/constant-fold-inttoptr-add.ll`, the fixture
/// of `stripAndAccumulateConstantOffsets`' `LookThroughIntToPtr`, which
/// `ConstantFoldCompareInstOperands` asks for on an equality.
#[test]
fn constant_fold_inttoptr_add_ll_passes() {
    check_every_run_line(CONSTANT_FOLD_INTTOPTR_ADD);
}

/// **No upstream counterpart**: `stripAndAccumulateConstantOffsets` stops at a
/// `getelementptr` whose offset does not fit the width it was given, which
/// stepping through an `addrspacecast` can bring about. The load is through a
/// 16-bit pointer and the offset needs 18 bits, so the walk stops short of the
/// global and the load stays. Read at 16 bits, the same offset is byte 4464 of
/// the zero array, and the load would fold to a value that is not there.
/// `%near` is the positive control.
#[test]
fn a_walk_stops_at_an_offset_wider_than_its_pointer() {
    let printed = instsimplify(
        r#"
target datalayout = "p:64:64:64-p1:16:16:16"

@g = constant { [70000 x i8], i8 } { [70000 x i8] zeroinitializer, i8 1 }

define i8 @far() {
  %v = load i8, ptr addrspace(1) addrspacecast (ptr getelementptr (i8, ptr @g, i64 70000) to ptr addrspace(1))
  ret i8 %v
}

define i8 @near() {
  %v = load i8, ptr addrspace(1) addrspacecast (ptr getelementptr (i8, ptr @g, i64 4464) to ptr addrspace(1))
  ret i8 %v
}
"#,
        None,
    );
    let body = |name: &str| {
        let start = printed
            .find(&format!("@{name}("))
            .unwrap_or_else(|| panic!("@{name} is printed"));
        let rest = &printed[start..];
        rest[..rest.find('}').expect("a function ends")].to_owned()
    };
    assert!(body("far").contains("= load i8"), "{printed}");
    assert!(body("near").contains("ret i8 0"), "{printed}");
}

/// Ports `crash_on_zeroinit`, `crash_on_poison`, `partial_load` and `load_vec3`
/// of `test/Transforms/InstSimplify/load.ll`. `partial_load` is the fixture of
/// `FoldReinterpretLoadFromConst`'s negative offset: the load starts one element
/// before `@GV`, so the bytes before the constant are zero. The two functions
/// left out load through a non-constant `getelementptr`, which is the arm of
/// `simplifyLoadInst` that `InstSimplifyPass` has not got.
#[test]
fn a_load_outside_a_constant_answers_as_load_ll_does() {
    let source = r#"
@zeroinit = constant {} zeroinitializer
@poison = constant {} poison

define i32 @crash_on_zeroinit() {
; CHECK-LABEL: @crash_on_zeroinit(
; CHECK-NEXT:    ret i32 poison
;
  %load = load i32, ptr @zeroinit
  ret i32 %load
}

define i32 @crash_on_poison() {
; CHECK-LABEL: @crash_on_poison(
; CHECK-NEXT:    ret i32 poison
;
  %load = load i32, ptr @poison
  ret i32 %load
}

@GV = private constant [8 x i32] [i32 42, i32 43, i32 44, i32 45, i32 46, i32 47, i32 48, i32 49]

define <8 x i32> @partial_load() {
; CHECK-LABEL: @partial_load(
; CHECK-NEXT:    ret <8 x i32> <i32 0, i32 42, i32 43, i32 44, i32 45, i32 46, i32 47, i32 48>
;
  %load = load <8 x i32>, ptr getelementptr ([8 x i32], ptr @GV, i64 0, i64 -1)
  ret <8 x i32> %load
}

@constvec = internal constant <3 x float> <float 0xBFDA20BC40000000, float 0xBFE6A09EE0000000, float 0x3FE279A760000000>

; This does an out of bounds load from the global constant
define <3 x float> @load_vec3() {
; CHECK-LABEL: @load_vec3(
; CHECK-NEXT:    ret <3 x float> poison
;
  %1 = load <3 x float>, ptr getelementptr inbounds (<3 x float>, ptr @constvec, i64 1)
  ret <3 x float> %1
}
"#;
    check_directives(
        &instsimplify(source, None),
        &read_check_directives_with_prefixes(source, &["CHECK"]),
    );
}

/// **No upstream counterpart as a fixture**, one assertion per arm of
/// `ReadDataFromGlobal` and `FoldReinterpretLoadFromConst` no fixture here pins.
/// `bytes_loaded > 32` declines. An `undef` element reads as zero bytes. An
/// `inttoptr` of a pointer-sized integer reads through its operand, and one of
/// another width does not. `fp128` is a `ConstantFP` with no integer of its
/// size to read as, and `bfloat` is another. A `ptr null` is no arm at all,
/// which is `ReadDataFromGlobal`'s "unknown initializer type": it reads as
/// nothing, even though it is zero.
#[test]
fn the_arms_of_a_read_answer_as_read_data_from_global_does() {
    let printed = instsimplify(
        r#"
@wide = constant [40 x i8] c"\01\02\03\04\05\06\07\08\01\02\03\04\05\06\07\08\01\02\03\04\05\06\07\08\01\02\03\04\05\06\07\08\01\02\03\04\05\06\07\08"
@with_undef = constant { i16, i16 } { i16 undef, i16 1 }
@pointer_int = constant { ptr, i32 } { ptr inttoptr (i64 7 to ptr), i32 1 }
@pointer_narrow = constant { ptr, i32 } { ptr inttoptr (i32 7 to ptr), i32 1 }
@quad = constant { fp128, i8 } { fp128 0xL00000000000000013FFF000000000000, i8 1 }
@half_float = constant { bfloat, i8 } { bfloat 0xR3F80, i8 1 }
@null_field = constant <{ i32, ptr }> <{ i32 1, ptr null }>

define i256 @thirty_two_bytes() {
  %v = load i256, ptr @wide
  ret i256 %v
}

define i264 @thirty_three_bytes() {
  %v = load i264, ptr @wide
  ret i264 %v
}

define i32 @undef_element() {
  %v = load i32, ptr @with_undef
  ret i32 %v
}

define i32 @pointer_sized_inttoptr() {
  %v = load i32, ptr @pointer_int
  ret i32 %v
}

define i32 @narrow_inttoptr() {
  %v = load i32, ptr @pointer_narrow
  ret i32 %v
}

define i32 @from_fp128() {
  %v = load i32, ptr @quad
  ret i32 %v
}

define i8 @from_bfloat() {
  %v = load i8, ptr @half_float
  ret i8 %v
}

define i64 @from_null_field() {
  %v = load i64, ptr @null_field
  ret i64 %v
}
"#,
        Some("e-p:64:64:64"),
    );
    let body = |name: &str| {
        let start = printed
            .find(&format!("@{name}("))
            .unwrap_or_else(|| panic!("@{name} is printed"));
        let rest = &printed[start..];
        rest[..rest.find('}').expect("a function ends")].to_owned()
    };
    let folds_to = |name: &str, expected: &str| {
        assert!(body(name).contains(expected), "{printed}");
        assert!(!body(name).contains("= load"), "{printed}");
    };
    let stays = |name: &str| assert!(body(name).contains("= load"), "{printed}");

    assert!(!body("thirty_two_bytes").contains("= load"), "{printed}");
    stays("thirty_three_bytes");
    folds_to("undef_element", "ret i32 65536");
    folds_to("pointer_sized_inttoptr", "ret i32 7");
    stays("narrow_inttoptr");
    stays("from_fp128");
    stays("from_bfloat");
    stays("from_null_field");
}

/// **No upstream counterpart**: the layout is
/// `InstSimplify/ConstProp/inttoptr-gep-nonintegral.ll`'s, but that fixture has
/// no load, so it does not reach this fold. No fixture under `test/Transforms`
/// loads a `pu`, `pe` or narrow-index pointer from an integer.
///
/// `ConstantFoldLoadThroughBitcast` and `FoldReinterpretLoadFromConst` each
/// refuse to cast a non-integral pointer into being, and `isNonIntegralPointerType`
/// answers for all three kinds of space: `p1` has an index narrower than its
/// pointer, `pu2` an unstable representation and `pe3` external state.
/// `%normal` is the positive control on each route. A zero is the exception
/// upstream's comment names: it "can coerce non-integral pointers legally".
#[test]
fn a_non_integral_pointer_is_not_cast_into_being() {
    let printed = instsimplify(
        r#"
target datalayout = "p:16:16:16:16-p1:16:16:16:8-pu2:16:16:16:8-pe3:16:16:16:8"

@int = constant i16 5
@bytes = constant [2 x i8] c"\05\00"
@zero = constant i16 0

define ptr @int_normal() {
  %v = load ptr, ptr @int
  ret ptr %v
}

define ptr addrspace(1) @int_narrow_index() {
  %v = load ptr addrspace(1), ptr @int
  ret ptr addrspace(1) %v
}

define ptr addrspace(2) @int_unstable() {
  %v = load ptr addrspace(2), ptr @int
  ret ptr addrspace(2) %v
}

define ptr addrspace(3) @int_external_state() {
  %v = load ptr addrspace(3), ptr @int
  ret ptr addrspace(3) %v
}

define ptr @bytes_normal() {
  %v = load ptr, ptr @bytes
  ret ptr %v
}

define ptr addrspace(1) @bytes_narrow_index() {
  %v = load ptr addrspace(1), ptr @bytes
  ret ptr addrspace(1) %v
}

define ptr addrspace(2) @bytes_unstable() {
  %v = load ptr addrspace(2), ptr @bytes
  ret ptr addrspace(2) %v
}

define ptr addrspace(3) @bytes_external_state() {
  %v = load ptr addrspace(3), ptr @bytes
  ret ptr addrspace(3) %v
}

define ptr addrspace(2) @zero_unstable() {
  %v = load ptr addrspace(2), ptr @zero
  ret ptr addrspace(2) %v
}
"#,
        None,
    );
    let body = |name: &str| {
        let start = printed
            .find(&format!("@{name}("))
            .unwrap_or_else(|| panic!("@{name} is printed"));
        let rest = &printed[start..];
        rest[..rest.find('}').expect("a function ends")].to_owned()
    };

    assert!(
        body("int_normal").contains("ret ptr inttoptr (i16 5 to ptr)"),
        "{printed}"
    );
    assert!(
        body("bytes_normal").contains("ret ptr inttoptr (i16 5 to ptr)"),
        "{printed}"
    );
    for name in [
        "int_narrow_index",
        "int_unstable",
        "int_external_state",
        "bytes_narrow_index",
        "bytes_unstable",
        "bytes_external_state",
    ] {
        assert!(body(name).contains("= load ptr addrspace("), "{printed}");
        assert!(!body(name).contains("inttoptr"), "{printed}");
    }
    assert!(
        body("zero_unstable").contains("ret ptr addrspace(2) null"),
        "{printed}"
    );
}
