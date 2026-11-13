//! A splat of 4294967295 lanes through the folds and analyses found walking
//! a splat's lanes (`docs/divergences.md` D40 names the searches that found
//! them; a search is not a proof that no other walk exists).
//!
//! **No upstream counterpart.** Upstream stores every lane of a fixed vector,
//! so `<4294967295 x i32> splat (i32 7)` costs it the lanes before any fold
//! runs; llvmkit stores the one element (`ConstantData::Splat`). These tests
//! pin the half of that hardening the folds own: each entry here walked every
//! lane once, so each test would allocate or loop over 2^32 lanes, exhausting
//! memory or the CI time limit, if one walked them again. Where upstream's
//! answer needs every lane built, llvmkit declines past `MAX_EXPANDED_LANES`
//! instead, which is `docs/divergences.md` D40; those cases pin the decline.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    CastOpcode, Constant, DataLayout, DynBrand, InstructionView, Module, NswRequirement,
    PoisonPolicy, ReadOnly, Unverified, User, Value, ValueTrackingQuery,
    can_create_undef_or_poison, constant_fold_cast_operand, constant_fold_insert_value_instruction,
    constant_fold_select_instruction, is_known_negation, is_known_not_undef_or_poison,
    mask_is_all_one_or_undefined, mask_is_all_zero_or_undefined,
    possibly_demanded_elements_in_mask,
};

pub mod support;

/// [`support::deadline::within_a_minute`], naming what a regression here
/// would be doing.
fn within_a_minute(body: impl FnOnce() + Send + 'static) {
    support::deadline::within_a_minute("a long splat's lanes are being walked", body);
}

/// A splat condition whose lane is no `ConstantInt`, and two arms.
const LONG_SPLATS: &str = concat!(
    "@g = global i8 0\n",
    "@condition = global <4294967295 x i1> splat (i1 ptrtoint (ptr @g to i1))\n",
    "@ones = global <4294967295 x i32> splat (i32 1)\n",
    "@twos = global <4294967295 x i32> splat (i32 2)\n",
);

/// Each test parses only the constants it asks about, so a fold that walks
/// a long splat again fails the test that names it.
fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}"),
    }
}

fn initializer<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> Constant<'m, DynBrand, ReadOnly> {
    module
        .as_view()
        .globals()
        .find(|global| global.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines @{name}"))
        .initializer()
        .unwrap_or_else(|| panic!("@{name} has an initializer"))
}

fn named_instruction<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> InstructionView<'m, DynBrand, ReadOnly> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

fn named_value<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> Value<'m, DynBrand, ReadOnly> {
    named_instruction(module, name).to_erased()
}

/// `extractelement` reads the one lane it is asked for:
/// `ConstantFoldExtractElementInstruction`'s `getAggregateElement(CIdx)`.
/// `shufflevector` reads one source lane per mask lane, so a two-lane mask
/// over two long splats costs two lanes and folds to `<2 x i32> splat (i32
/// 7)`, as upstream's per-lane `getExtractElement` loop answers.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_fold_that_reads_lanes_reads_only_those() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@extract = global i32 extractelement (<4294967295 x i32> splat (i32 7), i64 5)\n",
            "@shuffle = global <2 x i32> shufflevector (<4294967295 x i32> splat (i32 7), ",
            "<4294967295 x i32> splat (i32 8), <2 x i32> <i32 3, i32 0>)\n",
        ));
        assert_eq!(initializer(&module, "extract").to_string(), "i32 7");
        assert_eq!(
            initializer(&module, "shuffle").to_string(),
            "<2 x i32> splat (i32 7)"
        );
    });
}

/// `insertelement` into a long splat would build every lane, which upstream
/// does; llvmkit declines past `MAX_EXPANDED_LANES` and keeps the constant
/// expression, a divergence in the printed form (`docs/divergences.md` D40).
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_fold_that_builds_every_lane_of_a_long_splat_declines() {
    within_a_minute(|| {
        let module = parse(
            "@insert = global <4294967295 x i32> insertelement (<4294967295 x i32> splat (i32 7), i32 1, i64 0)",
        );
        assert_eq!(
            initializer(&module, "insert").to_string(),
            "<4294967295 x i32> insertelement (<4294967295 x i32> splat (i32 7), i32 1, i64 0)"
        );
    });
}

/// `ConstantFoldSelectInstruction` over a long splat condition: its lane is
/// no `ConstantInt`, so upstream's loop `break`s at lane 0 and the fold
/// answers nothing. With equal arms upstream's loop keeps every lane and
/// rebuilds the arm; llvmkit skips the walk past `MAX_EXPANDED_LANES` and the
/// `V1 == V2` check after it gives the same constant.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn select_over_a_long_splat_condition_walks_no_lanes() {
    within_a_minute(|| {
        let module = parse(LONG_SPLATS);
        let condition = initializer(&module, "condition");
        let ones = initializer(&module, "ones");
        let twos = initializer(&module, "twos");
        assert_eq!(
            constant_fold_select_instruction(condition, ones, twos).expect("one module"),
            None
        );
        assert_eq!(
            constant_fold_select_instruction(condition, ones, ones).expect("one module"),
            Some(ones)
        );
    });
}

/// The every-lane and any-lane analyses ask a long splat's one lane once:
/// `isGuaranteedNotToBeUndefOrPoison`'s `containsConstantExpression`,
/// `maskIsAllOneOrUndef` / `maskIsAllZeroOrUndef`, the shift-amount range
/// check in `canCreateUndefOrPoison`, and `cst_pred_ty`'s non-splat walk under
/// `isKnownNegation`'s `m_ZeroInt()`, which a long `poison` reaches.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn lane_tests_ask_a_long_splat_once() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@g = global i8 0\n",
            "@condition = global <4294967295 x i1> splat (i1 ptrtoint (ptr @g to i1))\n",
            "@ones = global <4294967295 x i32> splat (i32 1)\n",
            "define <4294967295 x i32> @shift(<4294967295 x i32> %x) {\n",
            "  %shifted = shl <4294967295 x i32> %x, splat (i32 3)\n",
            "  %negated = sub <4294967295 x i32> poison, %x\n",
            "  ret <4294967295 x i32> %shifted\n",
            "}\n",
        ));
        let data_layout = DataLayout::default();
        let query = ValueTrackingQuery::new(&data_layout);
        let condition = initializer(&module, "condition").as_erased();
        let ones = initializer(&module, "ones").as_erased();
        assert!(!is_known_not_undef_or_poison(condition, &query).expect("analysis runs"));
        assert!(is_known_not_undef_or_poison(ones, &query).expect("analysis runs"));
        assert!(!mask_is_all_one_or_undefined(condition));
        assert!(!mask_is_all_zero_or_undefined(condition));
        let shifted = named_value(&module, "shifted");
        assert!(!can_create_undef_or_poison(shifted, true));
        // `m_ZeroInt()` on a long `poison`: every lane is skipped, so no lane
        // is a zero and `sub poison, %x` is no negation of `%x`.
        let negated = named_value(&module, "negated");
        let x = named_instruction(&module, "negated")
            .operand(1)
            .expect("sub has two operands");
        assert!(!is_known_negation(
            negated,
            x,
            NswRequirement::NotRequired,
            PoisonPolicy::Allow
        ));
    });
}

/// `FoldBitCast` (`Analysis/ConstantFolding.cpp`) from a long splat to a
/// vector of half as many lanes would merge every lane pair, which upstream
/// does; llvmkit answers the `bitcast` expression, upstream's own answer for
/// a lane it cannot merge (D40). A zero is still folded by
/// `ConstantFoldLoadFromUniformValue` ahead of the walk.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_lane_changing_bitcast_of_a_long_splat_declines() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@wide = global <4294967294 x i32> splat (i32 1)\n",
            "@narrow = global <2147483647 x i64> zeroinitializer\n",
        ));
        let data_layout = DataLayout::default();
        let wide = initializer(&module, "wide");
        let narrow = initializer(&module, "narrow");
        let folded =
            constant_fold_cast_operand(CastOpcode::BitCast, wide, narrow.ty(), &data_layout)
                .expect("one module")
                .expect("a bitcast always answers");
        assert_eq!(
            folded.to_string(),
            "<2147483647 x i64> bitcast (<4294967294 x i32> splat (i32 1) to <2147483647 x i64>)"
        );
    });
}

/// `possiblyDemandedEltsInMask` over a `ConstantVector` splat of 2^30 lanes
/// asks its one lane: the lane is no null, so every lane stays demanded, as
/// upstream's walk over the stored lanes leaves them. The answer is one bit
/// per lane (128 MiB here), which upstream's is too, so the splat is 2^30
/// lanes rather than 2^32; a walk of its lanes would still run past the
/// deadline. The shortcut is `Constant::has_uniform_lanes` (D40).
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_splat_mask_demands_what_its_lanes_demand() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@g = global i8 0\n",
            "@mask = global <1073741824 x i1> splat (i1 ptrtoint (ptr @g to i1))\n",
        ));
        let mask = initializer(&module, "mask").as_erased();
        let demanded = possibly_demanded_elements_in_mask(mask).expect("a fixed mask");
        assert!(demanded.is_all_ones());
    });
}

/// A lane-splitting `bitcast` of a short element list builds more lanes than
/// the list stores, so past `MAX_EXPANDED_LANES` it answers the `bitcast`
/// expression too (D40): two `i65536` lanes are 131072 `i1` lanes, which
/// upstream's `FoldBitCast` builds one by one. A list split into no more
/// lanes than that is still folded, as upstream folds it.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_bitcast_that_splits_a_short_list_into_many_lanes_declines() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@pair = global <2 x i65536> <i65536 1, i65536 2>\n",
            "@bits = global <131072 x i1> zeroinitializer\n",
            "@small = global <2 x i16> <i16 1, i16 2>\n",
            "@bytes = global <4 x i8> zeroinitializer\n",
        ));
        let data_layout = DataLayout::default();
        let pair = initializer(&module, "pair");
        let bits = initializer(&module, "bits");
        let folded = constant_fold_cast_operand(CastOpcode::BitCast, pair, bits.ty(), &data_layout)
            .expect("one module")
            .expect("a bitcast always answers");
        assert!(
            folded
                .to_string()
                .starts_with("<131072 x i1> bitcast (<2 x i65536> <i65536 1, i65536 2> to"),
            "{folded}"
        );
        let small = initializer(&module, "small");
        let bytes = initializer(&module, "bytes");
        let folded =
            constant_fold_cast_operand(CastOpcode::BitCast, small, bytes.ty(), &data_layout)
                .expect("one module")
                .expect("a bitcast always answers");
        assert_eq!(folded.to_string(), "<4 x i8> <i8 1, i8 0, i8 2, i8 0>");
    });
}

/// `foldConstVectorToAPInt` shifts every lane into the scalar, so a
/// vector-to-scalar `bitcast` of a long splat answers the `bitcast`
/// expression past `MAX_EXPANDED_LANES` (D40) rather than walk 4194304 lanes
/// into an `i8388608`, each shift costing the result's words.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_bitcast_of_a_long_splat_to_a_scalar_declines() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@lanes = global <4194304 x i2> splat (i2 1)\n",
            "@scalar = global i8388608 0\n",
        ));
        let data_layout = DataLayout::default();
        let lanes = initializer(&module, "lanes");
        let scalar = initializer(&module, "scalar");
        let folded =
            constant_fold_cast_operand(CastOpcode::BitCast, lanes, scalar.ty(), &data_layout)
                .expect("one module")
                .expect("a bitcast always answers");
        assert_eq!(
            folded.to_string(),
            "i8388608 bitcast (<4194304 x i2> splat (i2 1) to i8388608)"
        );
    });
}

/// A `shufflevector` expression with a long uniform mask folds from the one
/// lane every mask lane names, as `ConstantFoldShuffleVectorInstruction`
/// folds it (its all-poison and all-zero arms, and the per-lane loop whose
/// repeated value `ConstantVector::get` makes a splat), without decoding the
/// mask's 4294967295 lanes.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_shufflevector_with_a_long_uniform_mask_folds_from_one_lane() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@one = global <4294967295 x i32> shufflevector (<2 x i32> <i32 1, i32 2>, ",
            "<2 x i32> <i32 3, i32 4>, <4294967295 x i32> splat (i32 1))\n",
            "@three = global <4294967295 x i32> shufflevector (<2 x i32> <i32 1, i32 2>, ",
            "<2 x i32> <i32 3, i32 4>, <4294967295 x i32> splat (i32 2))\n",
            "@zero = global <4294967295 x i32> shufflevector (<2 x i32> <i32 1, i32 2>, ",
            "<2 x i32> <i32 3, i32 4>, <4294967295 x i32> zeroinitializer)\n",
            "@none = global <4294967295 x i32> shufflevector (<2 x i32> <i32 1, i32 2>, ",
            "<2 x i32> <i32 3, i32 4>, <4294967295 x i32> poison)\n",
        ));
        assert_eq!(
            initializer(&module, "one").to_string(),
            "<4294967295 x i32> splat (i32 2)"
        );
        assert_eq!(
            initializer(&module, "three").to_string(),
            "<4294967295 x i32> splat (i32 3)"
        );
        assert_eq!(
            initializer(&module, "zero").to_string(),
            "<4294967295 x i32> splat (i32 1)"
        );
        assert_eq!(
            initializer(&module, "none").to_string(),
            "<4294967295 x i32> poison"
        );
    });
}

/// The cap is asked of a fold's operands together: `add` of a stored list of
/// 70000 lanes and a splat of as many walks the splat beside the list, which
/// costs no more than the list did, and folds as upstream's per-lane loop in
/// `ConstantFoldBinaryInstruction` does (D40).
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn a_long_splat_beside_a_stored_list_is_walked() {
    within_a_minute(|| {
        let lanes: Vec<String> = (0..70000).map(|lane| format!("i32 {lane}")).collect();
        let source = format!(
            "@sum = global <70000 x i32> add (<70000 x i32> <{}>, <70000 x i32> splat (i32 1))\n",
            lanes.join(", ")
        );
        let module = parse(&source);
        let sum = initializer(&module, "sum");
        assert_eq!(
            sum.aggregate_element(0).map(|lane| lane.to_string()),
            Some("i32 1".to_string())
        );
        assert_eq!(
            sum.aggregate_element(69999).map(|lane| lane.to_string()),
            Some("i32 70000".to_string())
        );
    });
}

/// `ConstantFoldInsertValueInstruction` rebuilds every element of the array it
/// inserts into. Past `MAX_EXPANDED_LANES` elements, an array that stores
/// none (a zero, `undef`, `poison`) declines rather than build them, so
/// `insertvalue` into `[70000 x i32] zeroinitializer` keeps the instruction,
/// where upstream folds it (D40); at 100 elements it folds as upstream does.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn an_insertvalue_into_a_long_zero_array_declines() {
    within_a_minute(|| {
        let module = parse(concat!(
            "@long = global [70000 x i32] zeroinitializer\n",
            "@short = global [100 x i32] zeroinitializer\n",
            "@one = global i32 1\n",
        ));
        let one = module
            .as_view()
            .globals()
            .find(|global| global.name().as_deref() == Some("one"))
            .and_then(|global| global.initializer())
            .expect("@one");
        assert_eq!(
            constant_fold_insert_value_instruction(initializer(&module, "long"), one, &[0])
                .expect("one module"),
            None
        );
        let short =
            constant_fold_insert_value_instruction(initializer(&module, "short"), one, &[0])
                .expect("one module")
                .expect("upstream folds it");
        assert_eq!(
            short
                .aggregate_element(0)
                .map(|element| element.to_string()),
            Some("i32 1".to_string())
        );
        assert_eq!(
            short
                .aggregate_element(99)
                .map(|element| element.to_string()),
            Some("i32 0".to_string())
        );
    });
}

/// An elementwise fold whose every operand stores one value for all its lanes
/// (here `undef` and a splat) folds lane 0 once and splats the answer past
/// `MAX_EXPANDED_LANES`: every lane of upstream's per-lane loop in
/// `ConstantFoldBinaryInstruction` folds `add undef, 1` to `undef`, and
/// `ConstantVector::get` of 4294967295 `undef`s is the whole-vector `undef`.
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn an_elementwise_fold_of_uniform_operands_answers_from_one_lane() {
    within_a_minute(|| {
        let module = parse(
            "@sum = global <4294967295 x i32> add (<4294967295 x i32> undef, <4294967295 x i32> splat (i32 1))\n",
        );
        assert_eq!(
            initializer(&module, "sum").to_string(),
            "<4294967295 x i32> undef"
        );
    });
}

/// A struct's type stores every field, so `insertvalue` into a zero struct of
/// 70000 fields walks what was stored and folds, as
/// `ConstantFoldInsertValueInstruction` does; only an array or vector, which
/// stores no elements, is held to `MAX_EXPANDED_LANES` (D40).
///
/// **No upstream counterpart** (see the module comment).
#[test]
fn an_insertvalue_into_a_long_zero_struct_folds() {
    within_a_minute(|| {
        let fields = vec!["i32"; 70000].join(", ");
        let module = parse(&format!(
            "@record = global {{ {fields} }} zeroinitializer\n@one = global i32 1\n"
        ));
        let one = initializer(&module, "one");
        let folded =
            constant_fold_insert_value_instruction(initializer(&module, "record"), one, &[0])
                .expect("one module")
                .expect("upstream folds it");
        assert_eq!(
            folded.aggregate_element(0).map(|field| field.to_string()),
            Some("i32 1".to_string())
        );
    });
}
