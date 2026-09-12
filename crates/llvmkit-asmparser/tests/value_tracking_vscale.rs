//! `llvm::getVScaleRange` and the two places `ValueTracking.cpp` reads it.
//!
//! **No upstream counterpart that llvmkit can express.** The only fixture that
//! drives `getVScaleRange` directly is
//! `TEST_F(ComputeKnownBitsTest, ComputeKnownBitsUnknownVScale)`
//! (`llvm/unittests/Analysis/ValueTrackingTest.cpp`), and both of its two
//! assertions are about a `CallInst` with **no parent function** — one before
//! it is inserted into a block, one after it is inserted into a block that has
//! no function. llvmkit cannot build detached IR at all (a separate recorded
//! difference), so neither half is portable and the fixture cannot be ported
//! even in part.
//!
//! The oracle below is therefore upstream's routine read directly:
//!
//! ```text
//! ConstantRange llvm::getVScaleRange(const Function *F, unsigned BitWidth) {
//!   Attribute Attr = F->getFnAttribute(Attribute::VScaleRange);
//!   if (!Attr.isValid())
//!     return ConstantRange(APInt(BitWidth, 1), APInt::getZero(BitWidth));
//!   unsigned AttrMin = Attr.getVScaleRangeMin();
//!   if ((unsigned)llvm::bit_width(AttrMin) > BitWidth)
//!     return ConstantRange::getEmpty(BitWidth);
//!   APInt Min(BitWidth, AttrMin);
//!   std::optional<unsigned> AttrMax = Attr.getVScaleRangeMax();
//!   if (!AttrMax || (unsigned)llvm::bit_width(*AttrMax) > BitWidth)
//!     return ConstantRange(Min, APInt::getZero(BitWidth));
//!   return ConstantRange(Min, APInt(BitWidth, *AttrMax) + 1);
//! }
//! ```
//!
//! one assertion per `return`.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    ApInt, ConstantRange, DynBrand, Module, Unverified, ValueTrackingQuery, compute_known_bits,
    get_vscale_range, is_known_to_be_a_power_of_two,
};

const SOURCE: &str = "\
declare i32 @llvm.vscale.i32()

define i32 @bounded() vscale_range(2,8) {
  %v = call i32 @llvm.vscale.i32()
  ret i32 %v
}

define i32 @unbounded() vscale_range(4) {
  %v = call i32 @llvm.vscale.i32()
  ret i32 %v
}

define i32 @unbounded_max() vscale_range(4,0) {
  %v = call i32 @llvm.vscale.i32()
  ret i32 %v
}

define i32 @none() {
  %v = call i32 @llvm.vscale.i32()
  ret i32 %v
}
";

fn parsed() -> Module<DynBrand, Unverified> {
    parser::parse_dynamic(SOURCE).expect("fixture parses")
}

fn range(module: &Module<DynBrand, Unverified>, name: &str, bit_width: u32) -> ConstantRange {
    let id = module
        .as_view()
        .functions()
        .find(|f| f.name() == name)
        .unwrap_or_else(|| panic!("fixture defines @{name}"))
        .id();
    get_vscale_range(module.view(id), bit_width).expect("the fixture's ranges are all well-formed")
}

fn width_32(low: u64, high: u64) -> ConstantRange {
    ConstantRange::new(
        ApInt::from_words(32, &[low]),
        ApInt::from_words(32, &[high]),
    )
    .expect("distinct endpoints")
}

/// `return ConstantRange(Min, APInt(BitWidth, *AttrMax) + 1);` — a bounded
/// `vscale_range(2,8)` is the half-open `[2, 9)`.
#[test]
fn a_bounded_range_is_min_to_max_plus_one() {
    let module = parsed();
    assert_eq!(range(&module, "bounded", 32), width_32(2, 9));
}

/// `if (!AttrMax || ...) return ConstantRange(Min, APInt::getZero(BitWidth));`
/// — both spellings of "unbounded above" give `[4, 0)`. `vscale_range(4)`
/// defaults its max to *min* in the assembler
/// (`LLParser::parseVScaleRangeArguments`), and `vscale_range(4,0)` sets the
/// packed `0` upstream reserves for unbounded; the parser maps that `0` to
/// `None`, which is the `!AttrMax` arm.
#[test]
fn an_unbounded_max_wraps_to_zero() {
    let module = parsed();
    assert_eq!(range(&module, "unbounded_max", 32), width_32(4, 0));
    // `vscale_range(4)` is `vscale_range(4,4)`, which is bounded: [4, 5).
    assert_eq!(range(&module, "unbounded", 32), width_32(4, 5));
}

/// `if (!Attr.isValid()) return ConstantRange(APInt(BitWidth, 1),
/// APInt::getZero(BitWidth));` — without the attribute, all that is known is
/// that vscale is non-zero.
#[test]
fn a_function_without_the_attribute_only_knows_vscale_is_non_zero() {
    let module = parsed();
    assert_eq!(range(&module, "none", 32), width_32(1, 0));
}

/// `if ((unsigned)llvm::bit_width(AttrMin) > BitWidth) return
/// ConstantRange::getEmpty(BitWidth);` — `vscale_range(2,8)` needs two bits, so
/// asking at `i1` width gives the empty set (the result is always poison).
#[test]
fn a_minimum_wider_than_the_bit_width_is_empty() {
    let module = parsed();
    assert!(range(&module, "bounded", 1).is_empty_set());
}

/// The `Intrinsic::vscale` arm of `computeKnownBitsFromOperator`:
/// `Known = getVScaleRange(II->getFunction(), BitWidth).toKnownBits();`.
/// `[2, 9)` over `i32` leaves the high 28 bits known zero.
#[test]
fn known_bits_of_a_vscale_call_read_the_range() {
    let module = parsed();
    let f = module
        .as_view()
        .functions()
        .find(|f| f.name() == "bounded")
        .expect("fixture defines @bounded");
    let call = f
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some("v"))
        .expect("@bounded defines %v")
        .to_erased();
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    let known = compute_known_bits(call, &query).expect("known bits of the vscale call");
    assert_eq!(known, width_32(2, 9).to_known_bits());
}

/// `if (Q.CxtI && match(V, m_VScale())) return
/// F->hasFnAttribute(Attribute::VScaleRange);` — the `vscale_range` attribute
/// is itself the proof that vscale is a power of two, and its absence is the
/// only thing that declines.
#[test]
fn a_vscale_call_is_a_power_of_two_exactly_when_the_attribute_is_present() {
    let module = parsed();
    let data_layout = module.data_layout();
    for (name, expected) in [("bounded", true), ("none", false)] {
        let f = module
            .as_view()
            .functions()
            .find(|f| f.name() == name)
            .unwrap_or_else(|| panic!("fixture defines @{name}"));
        let call = f
            .basic_blocks()
            .flat_map(|block| block.instructions())
            .find(|instruction| instruction.name().as_deref() == Some("v"))
            .unwrap_or_else(|| panic!("@{name} defines %v"));
        let query = ValueTrackingQuery::new(&data_layout).with_context_instruction(&call);
        assert_eq!(
            is_known_to_be_a_power_of_two(call.to_erased(), false, &query)
                .expect("power-of-two query"),
            expected,
            "@{name}"
        );
    }
}
