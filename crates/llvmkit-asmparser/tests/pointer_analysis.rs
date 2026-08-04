//! Ports of the pointer and object analyses — tranche 5 of the
//! `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Three upstream sources drive these:
//!
//! - `INSTANTIATE_TEST_SUITE_P(IsBytewiseValueParamTests, ...)` and its
//!   `IsBytewiseValueTests` table in
//!   `llvm/unittests/Analysis/ValueTrackingTest.cpp`.
//! - `INSTANTIATE_TEST_SUITE_P(FindAllocaForValueTest, ...)` and its
//!   `FindAllocaForValueTests` table in the same file, whose two `TEST_P`s ask
//!   the same IR with `OffsetZero` clear and set.
//! - `llvm/test/Transforms/InstCombine/strlen-1.ll`, whose `CHECK` lines record
//!   the constant `strlen` folds to; that fold is gated on `GetStringLength`,
//!   so the `CHECK` line is the oracle.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    BytewiseValue, DynBrand, Module, Unverified, Value, argument_aliasing_to_returned_pointer,
    find_alloca_for_value, find_inserted_value, get_constant_string_info, get_string_length,
    get_underlying_object, get_underlying_object_aggressive, get_underlying_objects,
    get_underlying_objects_for_code_gen, is_bytewise_value,
    is_intrinsic_returning_pointer_aliasing_argument_without_capturing,
    only_used_by_lifetime_markers, only_used_by_lifetime_markers_or_droppable_instructions,
    pointer_base_with_constant_offset,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The instruction named `%name` in the module's definitions.
fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

/// The initializer of the global named `@name`.
fn global_initializer<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> Value<'m, DynBrand> {
    module
        .globals()
        .find(|global| global.name() == name)
        .unwrap_or_else(|| panic!("fixture defines @{name}"))
        .initializer()
        .unwrap_or_else(|| panic!("@{name} has an initializer"))
        .into_erased()
}

/// A pointer to the global named `@name` — the global value itself, whose type
/// is `ptr`.
fn global_pointer<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .globals()
        .find(|global| global.name() == name)
        .unwrap_or_else(|| panic!("fixture defines @{name}"))
        .into_erased()
}

/// `IsBytewiseValueTests` from `llvm/unittests/Analysis/ValueTrackingTest.cpp`,
/// each row `(expected, initializer)` exactly as upstream writes it.
///
/// Upstream's oracle is the *printed* result: it runs `isBytewiseValue`, streams
/// the returned `Value` into a string, and compares — `""` when there is none.
/// `Value` prints in the same `<type> <value>` operand form here, so the table
/// is upstream's strings verbatim and [`BytewiseValue`] is rendered back into
/// them.
///
/// **Rows excluded, and why.** Upstream's table also has
/// `ptr inttoptr (iN -1 to ptr)` rows, which need `ConstantFoldIntegerCast`
/// from `ConstantFolding.cpp`; that is the same omission the module docs record
/// for `ReadByteArrayFromGlobal`. The `i8 poison` rows for zero-sized
/// aggregates are excluded too: llvmkit has one "any byte will do" answer where
/// upstream distinguishes `undef` from `poison`, and asserting a distinction
/// this port does not make would be asserting the wrong thing.
#[test]
fn is_bytewise_value_fixtures() {
    let cases: &[(&str, &str)] = &[
        ("i8 0", "ptr null"),
        ("i8 undef", "ptr undef"),
        ("i8 0", "i8 zeroinitializer"),
        ("i8 0", "i8 0"),
        ("i8 -86", "i8 -86"),
        ("i8 -1", "i8 -1"),
        ("i8 undef", "i16 undef"),
        ("i8 0", "i16 0"),
        ("", "i16 7"),
        ("i8 -86", "i16 -21846"),
        ("i8 -1", "i16 -1"),
        ("i8 0", "i48 0"),
        ("i8 -1", "i48 -1"),
        ("i8 0", "i49 0"),
        ("", "i49 -1"),
        ("i8 0", "half 0xH0000"),
        ("i8 -85", "half 0xHABAB"),
        ("i8 0", "float 0.0"),
        ("i8 -1", "float 0xFFFFFFFFE0000000"),
        ("i8 0", "double 0.0"),
        ("i8 -15", "double 0xF1F1F1F1F1F1F1F1"),
        ("i8 undef", "[0 x i8] undef"),
        ("i8 undef", "[5 x [0 x i8]] undef"),
        ("i8 0", "[6 x i8] zeroinitializer"),
        ("i8 undef", "[6 x i8] undef"),
        ("i8 1", "[5 x i8] [i8 1, i8 1, i8 1, i8 1, i8 1]"),
        ("", "[5 x i64] [i64 1, i64 1, i64 1, i64 1, i64 1]"),
        (
            "i8 -1",
            "[5 x i64] [i64 -1, i64 -1, i64 -1, i64 -1, i64 -1]",
        ),
        ("", "[4 x i8] [i8 1, i8 2, i8 1, i8 1]"),
        ("i8 1", "[4 x i8] [i8 1, i8 undef, i8 1, i8 1]"),
        ("i8 0", "<6 x i8> zeroinitializer"),
        ("i8 undef", "<6 x i8> undef"),
        ("i8 1", "<5 x i8> <i8 1, i8 1, i8 1, i8 1, i8 1>"),
        ("", "<5 x i64> <i64 1, i64 1, i64 1, i64 1, i64 1>"),
        (
            "i8 -1",
            "<5 x i64> <i64 -1, i64 -1, i64 -1, i64 -1, i64 -1>",
        ),
    ];

    for (expected, initializer) in cases {
        let source = format!(
            "@test = global {initializer}
"
        );
        let module = parse(&source);
        let data_layout = module.data_layout();

        let actual = match is_bytewise_value(global_initializer(&module, "test"), &data_layout) {
            // Upstream mints `ConstantInt::get(i8, byte)` and prints it; the
            // byte is unsigned here and `i8` prints signed, hence the cast.
            Some(BytewiseValue::Byte(byte)) => format!("i8 {}", byte.cast_signed()),
            Some(BytewiseValue::AnyByte) => "i8 undef".to_string(),
            Some(BytewiseValue::Value(found)) => found.to_string(),
            None => String::new(),
        };
        assert_eq!(&actual, expected, "isBytewiseValue({initializer})");
    }
}

/// `FindAllocaForValueTests` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// Each row carries upstream's two expectations: the answer with `OffsetZero`
/// clear (`TEST_P(FindAllocaForValueTest, findAllocaForValue)`) and with it set
/// (`...ZeroOffset`). Upstream compares `!!AI` against a `bool`, so only
/// found-or-not is asserted, which is what `Option::is_some` gives here.
#[test]
fn find_alloca_for_value_fixtures() {
    let cases: &[(&str, bool, bool)] = &[
        (
            r"
define void @test(i1 %cond) {
entry:
  %a = alloca i32
  br label %bb1

bb1:
  %r = phi ptr [ %a, %entry ], [ %r, %bb1 ]
  br i1 %cond, label %bb1, label %exit

exit:
  ret void
}
",
            true,
            true,
        ),
        (
            r"
define void @test(i1 %cond) {
  %a = alloca i32
  %r = select i1 %cond, ptr %a, ptr %a
  ret void
}
",
            true,
            true,
        ),
        (
            r"
define void @test(i1 %cond) {
  %a = alloca i32
  %b = alloca i32
  %r = select i1 %cond, ptr %a, ptr %b
  ret void
}
",
            false,
            false,
        ),
        (
            r"
define void @test(i1 %cond) {
entry:
  %a = alloca i64
  %a32 = bitcast ptr %a to ptr
  br label %bb1

bb1:
  %x = phi ptr [ %a32, %entry ], [ %x, %bb1 ]
  %r = getelementptr i32, ptr %x, i32 1
  br i1 %cond, label %bb1, label %exit

exit:
  ret void
}
",
            true,
            false,
        ),
        (
            r"
define void @test(i1 %cond) {
entry:
  %a = alloca i64
  %a32 = bitcast ptr %a to ptr
  br label %bb1

bb1:
  %x = phi ptr [ %a32, %entry ], [ %r, %bb1 ]
  %r = getelementptr i32, ptr %x, i32 1
  br i1 %cond, label %bb1, label %exit

exit:
  ret void
}
",
            true,
            false,
        ),
        (
            r"
define void @test(i1 %cond, ptr %a) {
entry:
  %r = bitcast ptr %a to ptr
  ret void
}
",
            false,
            false,
        ),
        (
            r"
define void @test(i1 %cond) {
entry:
  %a = alloca i32
  %b = alloca i32
  br label %bb1

bb1:
  %r = phi ptr [ %a, %entry ], [ %b, %bb1 ]
  br i1 %cond, label %bb1, label %exit

exit:
  ret void
}
",
            false,
            false,
        ),
        (
            r"
declare ptr @retptr(ptr returned)

define void @test(i1 %cond) {
  %a = alloca i32
  %r = call ptr @retptr(ptr %a)
  ret void
}
",
            true,
            true,
        ),
        (
            r"
declare ptr @fun(ptr)

define void @test(i1 %cond) {
  %a = alloca i32
  %r = call ptr @fun(ptr %a)
  ret void
}
",
            false,
            false,
        ),
    ];

    for (source, any_offset, zero_offset) in cases {
        let module = parse(source);
        let r = named(&module, "r");
        assert_eq!(
            find_alloca_for_value(r, false).is_some(),
            *any_offset,
            "findAllocaForValue(%r)\n{source}"
        );
        assert_eq!(
            find_alloca_for_value(r, true).is_some(),
            *zero_offset,
            "findAllocaForValue(%r, /*OffsetZero=*/true)\n{source}"
        );
    }
}

/// The globals and expectations of
/// `llvm/test/Transforms/InstCombine/strlen-1.ll`, whose `CHECK` lines say what
/// `strlen` folds to.
///
/// `@test_simplify1` folds `strlen(@hello)` to `5`, `@test_simplify2` folds
/// `strlen(@null)` to `0`, `@test_simplify3` folds `strlen(@null_hello)` to
/// `0`. `GetStringLength` returns `len + 1`, so those become 6, 1 and 1 — the
/// `+1` is upstream's, and the fold subtracts it back out.
#[test]
fn string_length_fixtures() {
    let module = parse(
        r#"
target datalayout = "e-p:32:32:32-i1:8:8-i8:8:8-i16:16:16-i32:32:32-i64:32:64-f32:32:32-f64:32:64-v64:64:64-v128:128:128-a0:0:64-f80:128:128"

@hello = constant [6 x i8] c"hello\00"
@longer = constant [7 x i8] c"longer\00"
@null = constant [1 x i8] zeroinitializer
@null_hello = constant [7 x i8] c"\00hello\00"
@null_hello_mid = constant [13 x i8] c"hello wor\00ld\00"
"#,
    );
    let data_layout = module.data_layout();

    for (name, expected) in [
        ("hello", 6),
        ("longer", 7),
        ("null", 1),
        ("null_hello", 1),
        ("null_hello_mid", 10),
    ] {
        assert_eq!(
            get_string_length(global_pointer(&module, name), 8, &data_layout),
            Some(expected),
            "GetStringLength(@{name})"
        );
    }
}

/// The same globals read as strings.
///
/// `getConstantStringInfo` is what `GetStringLength` is built on, and the
/// `strlen-1.ll` fixture pins both: `@hello` is `"hello"`, and `@null_hello`
/// trims to the empty string at its leading nul.
#[test]
fn constant_string_info_fixtures() {
    let module = parse(
        r#"
@hello = constant [6 x i8] c"hello\00"
@null_hello = constant [7 x i8] c"\00hello\00"
@not_constant = global [6 x i8] c"hello\00"
"#,
    );
    let data_layout = module.data_layout();

    assert_eq!(
        get_constant_string_info(global_pointer(&module, "hello"), true, &data_layout),
        Some(b"hello".to_vec())
    );
    assert_eq!(
        get_constant_string_info(global_pointer(&module, "null_hello"), true, &data_layout),
        Some(Vec::new())
    );
    // Untrimmed, the whole array comes back including both nuls.
    assert_eq!(
        get_constant_string_info(global_pointer(&module, "hello"), false, &data_layout),
        Some(b"hello\0".to_vec())
    );
    // A non-constant global is not readable: the linker may replace it.
    assert_eq!(
        get_constant_string_info(global_pointer(&module, "not_constant"), true, &data_layout),
        None
    );
}

/// `getUnderlyingObject` peels the address arithmetic the
/// `FindAllocaForValueTests` fixtures build.
///
/// **No upstream counterpart as a unit test** — `getUnderlyingObject` has none;
/// LLVM exercises it through `BasicAA`. The oracle is upstream's own function
/// body, which peels `getelementptr` and `bitcast` of a pointer and stops at an
/// `alloca`, so `%r` in this fixture must reach `%a` and no further.
#[test]
fn underlying_object_peels_gep_and_bitcast() {
    let module = parse(
        r"
define void @test() {
entry:
  %a = alloca i64
  %a32 = bitcast ptr %a to ptr
  %r = getelementptr i32, ptr %a32, i32 1
  ret void
}
",
    );
    let a = named(&module, "a");
    let r = named(&module, "r");
    assert_eq!(get_underlying_object(r, 10), a);
    // An `alloca` is where the walk stops, so asking again is a fixed point.
    assert_eq!(get_underlying_object(a, 10), a);
}

/// The three walks that follow `select` and `phi` disagree in exactly the way
/// upstream's function bodies say they should.
///
/// **No upstream counterpart as a unit test.** The oracle is the bodies:
/// `getUnderlyingObjects` pushes both `select` arms and collects each answer;
/// `getUnderlyingObjectAggressive` wants one object and returns
/// `FirstObject` — the plain `getUnderlyingObject` answer, here the `select`
/// itself — when the arms disagree; `getUnderlyingObjectsForCodeGen` insists
/// every answer is an identifiable object, which two `alloca`s are.
#[test]
fn underlying_objects_split_where_the_single_object_walk_gives_up() {
    let module = parse(
        r"
define void @test(i1 %cond) {
  %a = alloca i32
  %b = alloca i32
  %r = select i1 %cond, ptr %a, ptr %b
  ret void
}
",
    );
    let a = named(&module, "a");
    let b = named(&module, "b");
    let r = named(&module, "r");

    let mut objects = get_underlying_objects(r, 10);
    objects.sort_by_key(|object| object.name());
    assert_eq!(objects, vec![a, b]);

    // One object was asked for and the arms disagree, so the fallback is the
    // plain walk's answer — the `select`, which peels no further.
    assert_eq!(get_underlying_object_aggressive(r), r);

    let mut for_code_gen = get_underlying_objects_for_code_gen(r).expect("both are allocas");
    for_code_gen.sort_by_key(|object| object.name());
    assert_eq!(for_code_gen, vec![a, b]);
}

/// A `load` is not an identifiable object, so
/// `getUnderlyingObjectsForCodeGen` fails rather than reporting it.
///
/// **No upstream counterpart as a unit test**; the oracle is the body's own
/// comment, "If getUnderlyingObjects fails to find an identifiable object,
/// getUnderlyingObjectsForCodeGen also fails for safety."
#[test]
fn underlying_objects_for_code_gen_rejects_an_unidentified_object() {
    let module = parse(
        r"
define void @test(ptr %p) {
  %r = load ptr, ptr %p
  ret void
}
",
    );
    assert_eq!(
        get_underlying_objects_for_code_gen(named(&module, "r")),
        None
    );
}

/// `GetPointerBaseWithConstantOffset` accumulates a chain of constant
/// `getelementptr` indices into one byte offset.
///
/// **No upstream counterpart as a unit test.** The oracle is the wrapper's own
/// body — `stripAndAccumulateConstantOffsets` with `AllowNonInbounds` — plus
/// arithmetic the data layout fixes: `i32` is four bytes, so `[4 x i32]` index
/// 1 then element index 2 is `16 + 8 = 24` from `%a`.
#[test]
fn pointer_base_with_constant_offset_accumulates_a_gep_chain() {
    let module = parse(
        r#"
target datalayout = "e-p:64:64-i32:32:32"

define void @test() {
  %a = alloca [2 x [4 x i32]]
  %r = getelementptr [2 x [4 x i32]], ptr %a, i64 0, i64 1, i64 2
  ret void
}
"#,
    );
    let data_layout = module.data_layout();
    let a = named(&module, "a");
    let r = named(&module, "r");
    assert_eq!(
        pointer_base_with_constant_offset(r, &data_layout, true),
        (a, 24)
    );
    // A base with nothing to peel reports itself at offset zero.
    assert_eq!(
        pointer_base_with_constant_offset(a, &data_layout, true),
        (a, 0)
    );
}

/// `onlyUsedByLifetimeMarkers` and its droppable sibling.
///
/// **No upstream counterpart as a unit test.** The oracle is the shared helper
/// `onlyUsedByLifetimeMarkersOrDroppableInstsHelper`: every user must be an
/// intrinsic, and one of the two allowed kinds. `@llvm.assume` is droppable
/// (`User::isDroppable`) but not a lifetime marker, which is exactly what
/// separates the two entry points.
#[test]
fn only_used_by_lifetime_markers_fixtures() {
    let module = parse(
        r#"
declare void @llvm.lifetime.start.p0(ptr)
declare void @llvm.lifetime.end.p0(ptr)
declare void @llvm.assume(i1)
declare void @use(ptr)

define void @test(i1 %c) {
  %marked = alloca i32
  %dropped = alloca i32
  %used = alloca i32
  call void @llvm.lifetime.start.p0(ptr %marked)
  call void @llvm.lifetime.end.p0(ptr %marked)
  call void @llvm.lifetime.start.p0(ptr %dropped)
  call void @llvm.assume(i1 %c) [ "ignore"(ptr %dropped) ]
  call void @use(ptr %used)
  ret void
}
"#,
    );
    let marked = named(&module, "marked");
    let dropped = named(&module, "dropped");
    let used = named(&module, "used");

    assert!(only_used_by_lifetime_markers(marked));
    assert!(only_used_by_lifetime_markers_or_droppable_instructions(
        marked
    ));

    // `@llvm.assume` is droppable but is not a lifetime marker.
    assert!(!only_used_by_lifetime_markers(dropped));
    assert!(only_used_by_lifetime_markers_or_droppable_instructions(
        dropped
    ));

    // An ordinary call is neither.
    assert!(!only_used_by_lifetime_markers(used));
    assert!(!only_used_by_lifetime_markers_or_droppable_instructions(
        used
    ));
}

/// `FindInsertedValue` reads a scalar back out of an `insertvalue` chain.
///
/// **No upstream counterpart as a unit test**; LLVM exercises it through
/// InstCombine's `extractvalue` folds. The oracle is the body: matching indices
/// continue into the inserted value, a mismatch continues into the aggregate
/// that was written into, and a constant aggregate is indexed directly.
#[test]
fn find_inserted_value_reads_back_an_insertvalue_chain() {
    let module = parse(
        r"
define void @test(i32 %x, i32 %y) {
  %a = insertvalue { i32, i32 } undef, i32 %x, 0
  %b = insertvalue { i32, i32 } %a, i32 %y, 1
  ret void
}
",
    );
    let x = module
        .as_view()
        .functions()
        .next()
        .expect("fixture defines a function")
        .id();
    let params: Vec<_> = module.view(x).params().collect();
    let b = named(&module, "b");

    // Index 1 is what `%b` inserted; index 0 came from `%a`.
    assert_eq!(find_inserted_value(b, &[1]), Some(params[1].into_erased()));
    assert_eq!(find_inserted_value(b, &[0]), Some(params[0].into_erased()));
    // No indices is the value itself — upstream's recursion base case.
    assert_eq!(find_inserted_value(b, &[]), Some(b));
    // An index nothing wrote reaches the `undef` the chain started from, which
    // is not an `insertvalue`, so the walk gives up.
    assert_eq!(find_inserted_value(b, &[0, 0]), None);
}

/// `getArgumentAliasingToReturnedPointer` and
/// `isIntrinsicReturningPointerAliasingArgumentWithoutCapturing`.
///
/// **No upstream counterpart as a unit test.** The oracle is the two bodies:
/// the `returned` attribute answers first, then the intrinsic list, in which
/// `launder.invariant.group` always aliases and `ptrmask` aliases only when
/// null-ness need not be preserved.
#[test]
fn argument_aliasing_to_returned_pointer_fixtures() {
    let module = parse(
        r"
declare ptr @retptr(ptr returned)
declare ptr @llvm.launder.invariant.group.p0(ptr)
declare ptr @llvm.ptrmask.p0.i64(ptr, i64)
declare ptr @plain(ptr)

define void @test(ptr %p) {
  %returned = call ptr @retptr(ptr %p)
  %laundered = call ptr @llvm.launder.invariant.group.p0(ptr %p)
  %masked = call ptr @llvm.ptrmask.p0.i64(ptr %p, i64 -8)
  %opaque = call ptr @plain(ptr %p)
  ret void
}
",
    );
    let view = module.as_view();
    let call = |name: &str| {
        view.functions()
            .flat_map(|function| function.basic_blocks())
            .flat_map(|block| block.instructions())
            .find(|instruction| instruction.name().as_deref() == Some(name))
            .unwrap_or_else(|| panic!("fixture defines %{name}"))
    };
    let p = module
        .view(
            view.functions()
                .find(|function| function.name() == "test")
                .expect("fixture defines @test")
                .id(),
        )
        .params()
        .next()
        .expect("@test takes a pointer")
        .into_erased();

    for name in ["returned", "laundered"] {
        assert_eq!(
            argument_aliasing_to_returned_pointer(&call(name), false),
            Some(p),
            "%{name} aliases its argument"
        );
    }
    assert_eq!(
        argument_aliasing_to_returned_pointer(&call("opaque"), false),
        None
    );

    // `ptrmask` may move the pointer off null, so it aliases only when
    // null-ness need not be preserved.
    assert!(
        is_intrinsic_returning_pointer_aliasing_argument_without_capturing(&call("masked"), false)
    );
    assert!(
        !is_intrinsic_returning_pointer_aliasing_argument_without_capturing(&call("masked"), true)
    );
    // `launder.invariant.group` preserves null-ness either way.
    assert!(
        is_intrinsic_returning_pointer_aliasing_argument_without_capturing(
            &call("laundered"),
            true
        )
    );
}
