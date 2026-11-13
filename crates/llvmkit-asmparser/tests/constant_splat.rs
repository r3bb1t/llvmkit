//! `Constant::getSplatValue` (`llvm/lib/IR/Constants.cpp`).
//!
//! **No upstream unit test covers this directly.** `ConstantsTest.cpp` has no
//! `getSplatValue` case, so these fixtures are llvmkit's, with expectations
//! read off `Constant::getSplatValue` and `ConstantVector::getSplatValue`.
//!
//! The `allow_poison` flag is the point. Upstream defaults it to `false`, and
//! every `ConstantFold.cpp` call site takes that default; only analyses that
//! ask for it — `VectorUtils`' `getSplatValue`, which this crate's
//! `shufflevector` precision work depends on — pass `true`. Getting that
//! backwards would silently loosen constant folding, so both directions are
//! pinned here.

use llvmkit_asmparser::parser;
use llvmkit_ir::{Constant, DynBrand, Module, ReadOnly, Unverified};

/// The initializer is reached through the module's read view, so it is
/// `ReadOnly`; the splat query only reads it.
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

/// A vector whose lanes all agree is a splat under either mode.
///
/// Ports the loop in `ConstantVector::getSplatValue`, whose first check is
/// `OpC == Elt` and needs no poison tolerance to succeed.
#[test]
fn a_uniform_vector_is_a_splat_either_way() {
    let module = parser::parse_dynamic("@g = global <4 x i32> <i32 7, i32 7, i32 7, i32 7>")
        .expect("parses");
    let value = initializer(&module, "g");
    assert!(value.splat_value(true).is_some(), "poison allowed");
    assert!(value.splat_value(false).is_some(), "poison rejected");
}

/// A poison lane agrees with any other lane only when `allow_poison` is set.
///
/// This is the case the `shufflevector` fast path needs, and the case strict
/// mode must keep refusing.
#[test]
fn a_poison_lane_is_tolerated_only_when_allowed() {
    let module = parser::parse_dynamic("@g = global <4 x i32> <i32 7, i32 poison, i32 7, i32 7>")
        .expect("parses");
    let value = initializer(&module, "g");
    assert!(value.splat_value(true).is_some(), "poison allowed");
    assert!(
        value.splat_value(false).is_none(),
        "strict mode must refuse a poison lane"
    );
}

/// Two genuinely different lanes are not a splat, whatever the flag says.
#[test]
fn differing_lanes_are_never_a_splat() {
    let module = parser::parse_dynamic("@g = global <4 x i32> <i32 7, i32 8, i32 7, i32 7>")
        .expect("parses");
    let value = initializer(&module, "g");
    assert!(value.splat_value(true).is_none());
    assert!(value.splat_value(false).is_none());
}

/// An all-poison vector answers poison, and a zeroinitializer answers zero.
///
/// Ports `Constant::getSplatValue`'s first two arms, which short-circuit
/// before the element loop: `isa<PoisonValue>` yields poison of the element
/// type, and `isa<ConstantAggregateZero>` yields that type's null value.
#[test]
fn the_whole_vector_shortcuts_answer_before_the_element_loop() {
    let module = parser::parse_dynamic(
        "@p = global <4 x i32> poison\n@z = global <4 x i32> zeroinitializer",
    )
    .expect("parses");

    // Upstream returns poison of the *element* type here, not the vector.
    let poison = initializer(&module, "p")
        .splat_value(false)
        .expect("an all-poison vector splats");
    assert!(
        poison.ty().is_integer(),
        "the answer is the element type, not the vector"
    );

    let zero = initializer(&module, "z")
        .splat_value(false)
        .expect("zeroinitializer splats");
    assert!(zero.ty().is_integer());
}

/// A `splat` of a lane count past addressable memory parses, splats and prints
/// back, and a written-out list of equal lanes is the same constant as its
/// `splat` spelling.
///
/// **No upstream counterpart**: upstream's `LLParser::convertValIDToValue`
/// reaches `ConstantVector::getSplat` for `splat (…)`, which builds every lane,
/// so `@huge` costs one constant only here (hardening, `ConstantData::Splat`).
/// The round trip and the shared identity are what upstream answers: `@list`
/// and `@spelled` are one `ConstantDataVector`, which `writeConstantInternal`
/// prints as `splat (i32 7)`.
#[test]
fn a_splat_of_any_length_is_one_constant() {
    let source = "@huge = global <4294967295 x i32> splat (i32 1)\n\
                  @list = global <4 x i32> <i32 7, i32 7, i32 7, i32 7>\n\
                  @spelled = global <4 x i32> splat (i32 7)\n";
    let module = parser::parse_dynamic(source).expect("parses");
    let huge = initializer(&module, "huge");
    assert_eq!(
        huge.splat_value(false).map(|one| one.to_string()),
        Some("i32 1".to_string())
    );
    assert_eq!(
        initializer(&module, "list"),
        initializer(&module, "spelled")
    );
    let printed = format!("{module}");
    assert!(
        printed.contains("@huge = global <4294967295 x i32> splat (i32 1)"),
        "{printed}"
    );
    assert!(
        printed.contains("@list = global <4 x i32> splat (i32 7)"),
        "{printed}"
    );
}

/// `computeKnownBits`' vector constant arms, which upstream answers above its
/// `Depth == MaxAnalysisRecursionDepth` cutoff, so they answer at depth 0 of
/// a query whose limit is 0:
///
/// - `m_APInt` matches a splat of a `ConstantInt`, fixed or scalable, stored
///   or written out as a `shufflevector` expression (both through
///   `getSplatValue`), and answers `makeConstant`;
/// - a fixed `ConstantVector` splat of a global is no `ConstantInt` lane, so
///   it answers `resetAll`, whatever the global's alignment; a scalable one
///   is a `shufflevector` `Operator`, whose arm recurses into the splat value
///   (`getSplatValue`) below the cutoff;
/// - an element list intersects its `ConstantInt` lanes, and `resetAll`s at
///   a lane that is none, without recursing into it.
///
/// **No upstream unit test isolates these arms**; the expectations are read
/// off `computeKnownBits(const Value *, const APInt &, KnownBits &, …)` in
/// `llvm/lib/Analysis/ValueTracking.cpp`.
#[test]
fn known_bits_of_a_vector_constant_are_read_off_its_lanes() {
    use llvmkit_ir::{DataLayout, ValueTrackingQuery, compute_known_bits};
    let module = parser::parse_dynamic(concat!(
        "@g = global i64 0, align 8\n",
        "@fixed = global <4 x i32> splat (i32 8)\n",
        "@scalable_pointers = global <vscale x 2 x ptr> splat (ptr inttoptr (i64 16 to ptr))\n",
        "@fixed_pointers = global <2 x ptr> splat (ptr inttoptr (i64 16 to ptr))\n",
        "@scalable = global <vscale x 4 x i32> splat (i32 5)\n",
        "@pointers = global <4 x ptr> splat (ptr @g)\n",
        "@list = global <2 x i32> <i32 4, i32 12>\n",
        "@mixed = global <2 x i64> <i64 4, i64 ptrtoint (ptr @g to i64)>\n",
        "@expression = global <vscale x 4 x i32> shufflevector (<vscale x 4 x i32> insertelement (<vscale x 4 x i32> poison, i32 3, i32 0), <vscale x 4 x i32> poison, <vscale x 4 x i32> zeroinitializer)\n",
    ))
    .expect("parses");
    let data_layout = DataLayout::default();
    let query = ValueTrackingQuery::new(&data_layout).with_max_depth(0);
    let known = |name: &str| {
        compute_known_bits(initializer(&module, name).as_erased(), &query).expect("known bits")
    };
    assert_eq!(
        known("fixed")
            .constant()
            .map(|value| value.to_string_radix(10, llvmkit_ir::Signedness::Unsigned)),
        Some("8".to_string())
    );
    assert_eq!(
        known("scalable")
            .constant()
            .map(|value| value.to_string_radix(10, llvmkit_ir::Signedness::Unsigned)),
        Some("5".to_string())
    );
    assert!(known("pointers").is_unknown());
    let list = known("list");
    assert!(!list.is_unknown());
    assert_eq!(list.count_min_trailing_zeros(), 2);
    assert!(known("mixed").is_unknown());
    // A scalable splat of a pointer is a `shufflevector` `Operator`, below
    // the cutoff: at depth 0 of a limit-0 query it is unknown, and at the
    // default limit its `getSplatValue` recursion reads the element, here
    // `inttoptr (i64 16 to ptr)`, whose cast arm knows every bit. A fixed one
    // is a `ConstantVector` with no `ConstantInt` lane, reset above the
    // cutoff at any limit.
    assert!(known("scalable_pointers").is_unknown());
    let deep = ValueTrackingQuery::new(&data_layout);
    let deep_known = |name: &str| {
        compute_known_bits(initializer(&module, name).as_erased(), &deep).expect("known bits")
    };
    assert_eq!(
        deep_known("scalable_pointers")
            .constant()
            .map(|value| value.to_string_radix(10, llvmkit_ir::Signedness::Unsigned)),
        Some("16".to_string())
    );
    assert!(deep_known("fixed_pointers").is_unknown());
    // `m_APInt` reads a splat written as a `shufflevector` expression (an
    // `i32` index, so no stored splat) through the same `getSplatValue`.
    assert_eq!(
        known("expression")
            .constant()
            .map(|value| value.to_string_radix(10, llvmkit_ir::Signedness::Unsigned)),
        Some("3".to_string())
    );
}

/// A scalable splat is upstream's `shufflevector` expression, so the readers
/// that tell a `ConstantExpr` apart answer it as one:
///
/// - `isBytewiseValue` admits no expression but `inttoptr`, so a scalable
///   splat of `i32 16843009` (bytes `01 01 01 01`) is no byte splat, where a
///   fixed one is;
/// - `ConstantFoldSelectInstruction`'s `NotPoison` refuses a `ConstantExpr`,
///   so `select` of `undef` and a scalable splat is not folded to the splat;
/// - `getSplat`'s expression written out (`i64 0` index) is the same constant
///   as `splat (…)`, and an `i32` index another one, printed through
///   `writeConstantInternal`'s `shufflevector` splat shorthand.
///
/// **No upstream unit test isolates these arms**; the expectations are read
/// off `isBytewiseValue` (`llvm/lib/Analysis/ValueTracking.cpp`),
/// `ConstantFoldSelectInstruction` (`llvm/lib/IR/ConstantFold.cpp`),
/// `ConstantVector::getSplat` and `writeConstantInternal`.
#[test]
fn a_scalable_splat_is_read_as_the_expression_it_is_upstream() {
    use llvmkit_ir::{
        BytewiseValue, DataLayout, constant_fold_select_instruction, is_bytewise_value,
    };
    let module = parser::parse_dynamic(concat!(
        "@g = global i8 0\n",
        "@fixed = global <4 x i32> splat (i32 16843009)\n",
        "@scalable = global <vscale x 4 x i32> splat (i32 16843009)\n",
        "@undefined = global <vscale x 4 x i32> undef\n",
        "@condition = global i1 ptrtoint (ptr @g to i1)\n",
        "@short = global <vscale x 2 x i32> splat (i32 1)\n",
        "@long = global <vscale x 2 x i32> shufflevector (<vscale x 2 x i32> insertelement (<vscale x 2 x i32> poison, i32 1, i64 0), <vscale x 2 x i32> poison, <vscale x 2 x i32> zeroinitializer)\n",
        "@other = global <vscale x 2 x i32> shufflevector (<vscale x 2 x i32> insertelement (<vscale x 2 x i32> poison, i32 1, i32 0), <vscale x 2 x i32> poison, <vscale x 2 x i32> zeroinitializer)\n",
    ))
    .expect("parses");
    let data_layout = DataLayout::default();
    assert!(matches!(
        is_bytewise_value(initializer(&module, "fixed").as_erased(), &data_layout),
        Some(BytewiseValue::Byte(1))
    ));
    assert!(
        is_bytewise_value(initializer(&module, "scalable").as_erased(), &data_layout).is_none()
    );

    let condition = initializer(&module, "condition");
    let undefined = initializer(&module, "undefined");
    let scalable = initializer(&module, "scalable");
    assert_eq!(
        constant_fold_select_instruction(condition, undefined, scalable).expect("one module"),
        None
    );

    assert_eq!(initializer(&module, "short"), initializer(&module, "long"));
    assert_ne!(initializer(&module, "short"), initializer(&module, "other"));
    let printed = format!("{module}");
    for name in ["short", "long", "other"] {
        assert!(
            printed.contains(&format!(
                "@{name} = global <vscale x 2 x i32> splat (i32 1)"
            )),
            "{printed}"
        );
    }
}

/// `computeKnownBits`' first test, `if (!DemandedElts) { Known.resetAll();
/// return; }`, comes before its constant arms: a vector zero with no lane
/// demanded is unknown, not zero.
///
/// **No upstream unit test isolates it**; read off `computeKnownBits(const
/// Value *, const APInt &, KnownBits &, …)` in
/// `llvm/lib/Analysis/ValueTracking.cpp`.
#[test]
fn known_bits_with_no_demanded_lane_are_unknown() {
    use llvmkit_ir::{ApInt, DataLayout, ValueTrackingQuery, compute_known_bits};
    let module =
        parser::parse_dynamic("@zero = global <4 x i32> zeroinitializer\n").expect("parses");
    let data_layout = DataLayout::default();
    let none = ApInt::zero(4);
    let query = ValueTrackingQuery::new(&data_layout).with_demanded_elements(&none);
    let known =
        compute_known_bits(initializer(&module, "zero").as_erased(), &query).expect("known bits");
    assert!(known.is_unknown());
    let all = ApInt::all_ones(4);
    let query = ValueTrackingQuery::new(&data_layout).with_demanded_elements(&all);
    let known =
        compute_known_bits(initializer(&module, "zero").as_erased(), &query).expect("known bits");
    assert!(known.is_constant());
}

/// `FlushFPConstant`'s `ConstantVector` loop answers `nullptr` for a lane that
/// is no `ConstantFP` or `undef`, a constant expression included; only the
/// whole operand being a `ConstantExpr` is returned as it is.
///
/// **No upstream unit test isolates it**; read off `FlushFPConstant`
/// (`llvm/lib/Analysis/ConstantFolding.cpp`).
#[test]
fn a_flushed_list_with_an_expression_lane_answers_nothing() {
    use llvmkit_ir::{DenormalMode, DenormalModeKind, DenormalModeSide, flush_fp_constant};
    let module = parser::parse_dynamic(concat!(
        "@g = global i8 0\n",
        "@list = global <2 x float> <float 0x36A0000000000000, float bitcast (i32 ptrtoint (ptr @g to i32) to float)>\n",
    ))
    .expect("parses");
    let mode = DenormalMode::new(
        DenormalModeKind::PreserveSign,
        DenormalModeKind::PreserveSign,
    );
    assert_eq!(
        flush_fp_constant(initializer(&module, "list"), mode, DenormalModeSide::Input)
            .expect("one module"),
        None
    );
}

/// `ConstantFoldBinaryInstruction`'s identity fold compares by pointer with
/// `ConstantExpr::getBinOpIdentity`'s splat (`C2 == Identity`), so `mul` by
/// the uniqued `splat (i32 1)` folds and `mul` by a `shufflevector`
/// expression that only splats `1` (an `undef` base and an `i32` index, so
/// another constant) does not: the other operand, a scalable
/// `insertelement` at lane 1, is no splat, so the splat fast path does not
/// apply either, and upstream's scalable bail-out answers nothing.
///
/// **No upstream unit test isolates it**; read off
/// `ConstantFoldBinaryInstruction` (`llvm/lib/IR/ConstantFold.cpp`) and
/// `ConstantVector::getSplat` (`llvm/lib/IR/Constants.cpp`).
#[test]
fn only_the_uniqued_splat_is_a_binary_identity() {
    use llvmkit_ir::{BinaryOpcode, constant_fold_binary_instruction};
    let module = parser::parse_dynamic(concat!(
        "@g = global i8 0\n",
        "@value = global <vscale x 4 x i32> insertelement (<vscale x 4 x i32> poison, i32 5, i64 1)\n",
        "@one = global <vscale x 4 x i32> splat (i32 1)\n",
        "@spelled = global <vscale x 4 x i32> shufflevector (<vscale x 4 x i32> insertelement (<vscale x 4 x i32> undef, i32 1, i32 0), <vscale x 4 x i32> undef, <vscale x 4 x i32> zeroinitializer)\n",
    ))
    .expect("parses");
    let value = initializer(&module, "value");
    assert_eq!(
        constant_fold_binary_instruction(BinaryOpcode::Mul, value, initializer(&module, "one"))
            .expect("one module"),
        Some(value)
    );
    assert_eq!(
        constant_fold_binary_instruction(BinaryOpcode::Mul, value, initializer(&module, "spelled"))
            .expect("one module"),
        None
    );
}

/// `m_ImmConstant` (`immconstant_ty::isImmConstant`, `PatternMatch.h`) takes
/// a vector whose `getSplatValue(/*AllowPoison=*/true)` is free of constant
/// expressions, though the vector is a `shufflevector` expression: both arms
/// of the `select` (a written-out splat with an `i32` index, and a scalable
/// `splat`, which upstream stores as that expression) are possible values.
///
/// **No upstream unit test isolates it**; read off `isImmConstant` and the
/// `Push` lambda of `collectPossibleValues` (`llvm/lib/Analysis/ValueTracking.cpp`).
#[test]
fn a_splat_expression_is_an_immediate_constant() {
    use llvmkit_ir::{DataLayout, ValueTrackingQuery, collect_possible_values};
    let module = parser::parse_dynamic(concat!(
        "define <vscale x 4 x i32> @f(i1 %c) {\n",
        "  %s = select i1 %c, <vscale x 4 x i32> shufflevector (<vscale x 4 x i32> insertelement (<vscale x 4 x i32> poison, i32 7, i32 0), <vscale x 4 x i32> poison, <vscale x 4 x i32> zeroinitializer), <vscale x 4 x i32> splat (i32 8)\n",
        "  ret <vscale x 4 x i32> %s\n",
        "}\n",
    ))
    .expect("parses");
    let select = module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some("s"))
        .expect("%s")
        .to_erased();
    let data_layout = DataLayout::default();
    let query = ValueTrackingQuery::new(&data_layout);
    let values = collect_possible_values(select, 4, true, &query).expect("analysis runs");
    assert_eq!(values.map(|values| values.len()), Some(2));
}
