//! `CallBase::getReturnedArgOperand` and the two analyses that read it.
//!
//! Upstream has one routine — `getArgOperandWithAttribute(Attribute::Returned)`
//! in `llvm/lib/IR/Instructions.cpp` — reached from `computeKnownBits`'s
//! `Call`/`Invoke` arm (`ValueTracking.cpp`) and from
//! `getArgumentAliasingToReturnedPointer` / `getUnderlyingObject`
//! (`llvm/lib/Analysis/ValueTracking.cpp` and `llvm/lib/IR/Value.cpp`). llvmkit
//! reaches it from the same two places, through the single
//! `value_tracking.rs::returned_arg_operand`.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, MAX_LOOKUP_SEARCH_DEPTH, Module, Unverified, Value, ValueTrackingQuery,
    argument_aliasing_to_returned_pointer, compute_known_bits, underlying_object,
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

/// Port of
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp::TEST_F(ComputeKnownBitsTest, ComputeKnownBitsReturnedRangeConflict)`.
///
/// The `returned` here sits on the **declaration** and the call site does not
/// repeat it, so the answer depends entirely on
/// `getArgOperandWithAttribute`'s second leg —
/// `F->getAttributes().hasAttrSomewhere(Kind, &Index)`. Without that leg the
/// range metadata is the only input and the call reads as 32; with it the two
/// disagree and upstream discards everything, which is the `(0, 0)` below.
#[test]
fn compute_known_bits_returned_range_conflict() {
    let module = parse(
        r"
declare i16 @foo(i16 returned)

define i16 @test() {
  %A = call i16 @foo(i16 4095), !range !{i16 32, i16 33}
  ret i16 %A
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    let known = compute_known_bits(named(&module, "A"), &query).expect("query succeeds");
    // The call returns 32 according to range metadata, but 4095 according to
    // the returned arg operand. Given the conflicting information we expect
    // that the known bits information simply is cleared.
    assert!(known.is_unknown(), "expected all bits unknown, got {known}");
}

/// Both legs of `getArgOperandWithAttribute`, at both parameter positions,
/// through both of llvmkit's readers.
///
/// **No upstream counterpart.** Upstream cannot state this law: it has exactly
/// one `getReturnedArgOperand`, so there is nothing for a second reader to
/// disagree with. llvmkit had two, and each was correct precisely where the
/// other was wrong — `value_tracking.rs` had no callee leg, and
/// `pointer_analysis.rs` read a call site's per-argument storage with the
/// *function*'s key (`AttrIndex::Param(index)` rather than `Param(0)`) and so
/// saw `returned` only on parameter 0. Divergence 64 recorded the first and
/// asserted the second was a correct twin. The four columns below are the
/// matrix that catches either mistake: the `_site_` rows fail against a
/// `Param(index)` reader for `index > 0`, and the `_callee_` rows fail against
/// a reader with no callee fallback.
#[test]
fn returned_is_found_on_either_the_call_site_or_the_callee_at_any_position() {
    let module = parse(
        r"
declare ptr @callee_returns_p1(ptr, ptr returned)
declare ptr @callee_returns_p0(ptr returned, ptr)
declare ptr @plain(ptr, ptr)

define ptr @pointers(ptr %x, ptr %y) {
  %callee_p1 = call ptr @callee_returns_p1(ptr %x, ptr %y)
  %callee_p0 = call ptr @callee_returns_p0(ptr %x, ptr %y)
  %site_p1 = call ptr @plain(ptr %x, ptr returned %y)
  %site_p0 = call ptr @plain(ptr returned %x, ptr %y)
  %none = call ptr @plain(ptr %x, ptr %y)
  ret ptr %callee_p1
}
",
    );

    // `getArgumentAliasingToReturnedPointer`, whose first act is
    // `getReturnedArgOperand`.
    let alias = |name: &str| -> Option<String> {
        let view = module.as_view();
        let instruction = view
            .functions()
            .flat_map(|function| function.basic_blocks())
            .flat_map(|block| block.instructions())
            .find(|instruction| instruction.name().as_deref() == Some(name))
            .unwrap_or_else(|| panic!("fixture defines %{name}"));
        argument_aliasing_to_returned_pointer(&instruction, false)
            .and_then(|value| value.name().map(|name| name.to_string()))
    };
    // `getUnderlyingObject`, which peels a call through the same routine.
    let object = |name: &str| -> Option<String> {
        underlying_object(named(&module, name), MAX_LOOKUP_SEARCH_DEPTH)
            .name()
            .map(|name| name.to_string())
    };

    for (name, expected) in [
        ("callee_p1", Some("y")),
        ("callee_p0", Some("x")),
        ("site_p1", Some("y")),
        ("site_p0", Some("x")),
        ("none", None),
    ] {
        let expected = expected.map(str::to_string);
        assert_eq!(alias(name), expected, "argument_aliasing at %{name}");
        // A call with no `returned` argument is its own underlying object.
        let expected_object = expected.clone().or_else(|| Some(name.to_string()));
        assert_eq!(
            object(name),
            expected_object,
            "underlying_object at %{name}"
        );
    }

    // The same matrix through `computeKnownBits`, whose `returned` arm is the
    // other reader. A constant argument makes the answer observable.
    let module = parse(
        r"
declare i32 @icallee_returns_p1(i32, i32 returned)
declare i32 @icallee_returns_p0(i32 returned, i32)
declare i32 @iplain(i32, i32)

define void @integers(i32 %p) {
  %callee_p1 = call i32 @icallee_returns_p1(i32 %p, i32 8)
  %callee_p0 = call i32 @icallee_returns_p0(i32 8, i32 %p)
  %site_p1 = call i32 @iplain(i32 %p, i32 returned 8)
  %site_p0 = call i32 @iplain(i32 returned 8, i32 %p)
  %none = call i32 @iplain(i32 %p, i32 8)
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    let eight = "00000000000000000000000000001000";
    for (name, expected) in [
        ("callee_p1", eight),
        ("callee_p0", eight),
        ("site_p1", eight),
        ("site_p0", eight),
        ("none", "????????????????????????????????"),
    ] {
        let known = compute_known_bits(named(&module, name), &query).expect("query succeeds");
        assert_eq!(known.to_string(), expected, "known bits at %{name}");
    }
}
