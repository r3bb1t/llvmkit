//! `CallBase::getRange` as `computeKnownBits` reads it.
//!
//! Upstream's `computeKnownBitsFromOperator` `Call` / `Invoke` arm asks
//! `if (std::optional<ConstantRange> Range = CB->getRange()) Known =
//! Known.unionWith(Range->toKnownBits());`, and `CallBase::getRange`
//! (`Instructions.cpp`) is the call site's `range` return attribute
//! intersected with `getCalledFunction()`'s, or whichever of the two exists.

use core::cmp::Ordering;

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, KnownBits, Module, Unverified, Value, ValueTrackingQuery, compute_known_bits,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The instruction named `%name` in the function `@function`.
fn named_in<'m>(
    module: &'m Module<DynBrand, Unverified>,
    function: &str,
    name: &str,
) -> Value<'m, DynBrand> {
    let function = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    module
        .view(function)
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

fn known_bits(module: &Module<DynBrand, Unverified>, function: &str, name: &str) -> KnownBits {
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    compute_known_bits(named_in(module, function, name), &query).expect("query succeeds")
}

/// Whether `simplifyShift`'s known-bits gate folds a shift by `known` to
/// `poison`: `KnownAmt.getMinValue().uge(KnownAmt.getBitWidth())`
/// (`InstructionSimplify.cpp`).
fn shift_amount_is_known_out_of_range(known: &KnownBits) -> bool {
    known
        .min_value()
        .unsigned_cmp_u64(u64::from(known.bit_width()))
        != Ordering::Less
}

/// Mirrors `@shl_amount_is_known_bogus_range_return` and
/// `@neg_shl_amount_is_known_bogus_range_return` from
/// `llvm/test/Transforms/InstSimplify/shift-knownbits.ll`, IR unchanged. The
/// first `CHECK` block folds the `shl` to `poison` and the second keeps it.
/// `instsimplify` decides both at `simplifyShift`'s known-bits gate, which is
/// asked here of the shift amount `%b`. The range sits on the callee's
/// declaration only, so only `getRange`'s `getCalledFunction()` half reaches
/// it.
#[test]
fn a_callee_s_declared_range_bounds_the_shift_amount() {
    let module = parse(
        r"
declare range(i32 32, 64) i32 @returns_out_of_range_helper()
declare range(i32 0, 32) i32 @returns_in_range_helper()

define i32 @shl_amount_is_known_bogus_range_return(i32 %a) {
  %b = call i32 @returns_out_of_range_helper()
  %shl = shl i32 %a, %b
  ret i32 %shl
}

define i32 @neg_shl_amount_is_known_bogus_range_return(i32 %a) {
  %b = call i32 @returns_in_range_helper()
  %shl = shl i32 %a, %b
  ret i32 %shl
}
",
    );
    assert!(shift_amount_is_known_out_of_range(&known_bits(
        &module,
        "shl_amount_is_known_bogus_range_return",
        "b"
    )));
    assert!(!shift_amount_is_known_out_of_range(&known_bits(
        &module,
        "neg_shl_amount_is_known_bogus_range_return",
        "b"
    )));
}

/// A call site's `range` and its callee's are intersected.
///
/// **No upstream counterpart.** The oracle is `CallBase::getRange`'s `if
/// (CallAttr.isValid() && FnAttr.isValid()) return
/// CallAttr.getRange().intersectWith(FnAttr.getRange());`. `[32, 128)` on the
/// call site and `[0, 64)` on the callee meet at `[32, 64)`, whose known bits
/// are bit 5 set and bits 6 to 31 clear. Either range alone leaves bit 5
/// unknown, which `%call.site.only` and `%callee.only` show as the controls.
#[test]
fn a_call_site_range_and_a_callee_range_are_intersected() {
    let module = parse(
        r"
declare range(i32 0, 64) i32 @bounded()
declare i32 @unbounded()

define void @test() {
  %both = call range(i32 32, 128) i32 @bounded()
  %call.site.only = call range(i32 32, 128) i32 @unbounded()
  %callee.only = call i32 @bounded()
  ret void
}
",
    );
    assert_eq!(
        known_bits(&module, "test", "both").to_string(),
        "000000000000000000000000001?????"
    );
    assert_eq!(
        known_bits(&module, "test", "call.site.only").to_string(),
        "0000000000000000000000000???????"
    );
    assert_eq!(
        known_bits(&module, "test", "callee.only").to_string(),
        "00000000000000000000000000??????"
    );
}
