//! The `getelementptr` arm of `computeKnownBitsFromOperator`.
//!
//! **No upstream counterpart.** `unittests/Analysis/ValueTrackingTest.cpp`'s
//! `getelementptr`s step over a scalar source with one index, and the only
//! scalable one in `test/Analysis/ValueTracking` is the `<vscale x 16 x i8>` of
//! `phi-known-bits.ll`'s `recursiveGEP_withPtrSub_scalableGEP`, whose `CHECK`
//! is about `isKnownNonEqual`. The oracle is the routine read directly: a walk
//! over `gep_type_iterator`, where a stride is `getSequentialElementStride`'s
//! `TypeSize`, and only `Stride.isScalable()` decides what is known about it.
//!
//! Every pointer below starts from an `alloca` aligned to 64, so its low six
//! bits are known zero and its seventh is not.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, KnownBits, Module, Unverified, Value, ValueTrackingQuery, compute_known_bits,
};

fn parsed() -> Module<DynBrand, Unverified> {
    let source = r#"
define void @f(i64 %i) {
  %p = alloca [128 x i8], align 64
  %array = getelementptr [4 x i32], ptr %p, i64 1
  %struct = getelementptr { i32, i32, i32 }, ptr %p, i64 5
  %vector = getelementptr <4 x i16>, ptr %p, i64 1
  %lane = getelementptr <2 x i48>, ptr %p, i64 0, i64 1
  %inner = getelementptr [4 x [2 x i32]], ptr %p, i64 0, i64 1, i64 1
  %field = getelementptr { i32, [2 x i32] }, ptr %p, i64 0, i32 1, i64 1
  %scalable = getelementptr <vscale x 16 x i8>, ptr %p, i64 1
  %scalable_array = getelementptr [2 x <vscale x 4 x i32>], ptr %p, i64 1
  %scalable_element = getelementptr [2 x <vscale x 4 x i32>], ptr %p, i64 0, i64 1
  %scalable_index = getelementptr <vscale x 4 x i32>, ptr %p, i64 %i
  %scalable_lane = getelementptr <vscale x 4 x i32>, ptr %p, i64 0, i64 1
  %scalable_zero = getelementptr <vscale x 4 x i32>, ptr %p, i64 0
  %scalable_field = getelementptr [2 x { <vscale x 4 x i32>, <vscale x 4 x i32> }], ptr %p, i64 0, i64 0, i32 1
  ret void
}
"#;
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}"),
    }
}

fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| module.view(function.id()).basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

fn known(module: &Module<DynBrand, Unverified>, name: &str) -> KnownBits {
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    compute_known_bits(named(module, name), &query).expect("the query succeeds")
}

/// The low `count` bits, most significant first, as `KnownBits` prints them.
fn low_bits(known: &KnownBits, count: usize) -> String {
    let text = known.to_string();
    text[text.len() - count..].to_owned()
}

/// **No upstream counterpart**, see the module docs. The first index strides
/// whole source types (`gep_type_begin`), whatever kind of type the source is;
/// only a later index steps into it. Each offset below is exact, so it lands
/// in the six known bits. A vector's lanes stride by `getTypeStoreSize`, so an
/// `i48` lane is 6 bytes on, where an array of `i48`s would be 8.
#[test]
fn the_first_index_strides_whole_source_types() {
    let module = parsed();
    for (name, offset) in [
        ("array", "010000"),
        ("struct", "111100"),
        ("vector", "001000"),
        ("lane", "000110"),
        ("inner", "001100"),
        ("field", "001000"),
    ] {
        assert_eq!(low_bits(&known(&module, name), 6), offset, "%{name}");
    }
}

/// **No upstream counterpart**, see the module docs. Pins `Stride.isScalable()`:
/// a scalable stride says the index is scaled by some multiple of its known
/// minimum, so the low bits of that minimum are known zero and nothing above
/// them is. That holds wherever the size came from, an array of scalable
/// vectors included: `%scalable_array` must not read as the fixed offset 16,
/// and `%scalable_lane`'s indexed type is an `i32`, which is the fixed path.
/// `%scalable_zero` is upstream's "Handle case when index is zero".
#[test]
fn a_scalable_stride_is_known_only_as_a_multiple_of_its_minimum() {
    let module = parsed();
    let tail = |name: &str| low_bits(&known(&module, name), 6);

    assert_eq!(tail("scalable"), "??0000");
    assert_eq!(tail("scalable_array"), "?00000");
    assert_eq!(tail("scalable_element"), "??0000");
    assert_eq!(tail("scalable_index"), "??0000");
    assert_eq!(tail("scalable_lane"), "000100");
    assert_eq!(tail("scalable_zero"), "000000");
}

/// **No upstream counterpart**: upstream reports a fatal error, because the arm
/// assigns `SL->getElementOffset(Idx)` to a `uint64_t`, whose implicit conversion
/// rejects a scalable offset. llvmkit declines (Doctrine D10): nothing is known.
#[test]
fn a_scalable_field_offset_is_unknown() {
    let module = parsed();
    assert!(known(&module, "scalable_field").is_unknown());
}
