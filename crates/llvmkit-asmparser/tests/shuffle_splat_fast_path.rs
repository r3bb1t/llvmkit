//! `getSplatValue`'s fast path inside the two `shufflevector` analysis arms.
//!
//! **No upstream unit test isolates this.** `ValueTrackingTest.cpp` exercises
//! `shufflevector` through `isGuaranteedNotToBeUndefOrPoison` and through
//! `computeKnownBits` with clean masks, and `ComputeKnownFPClassTest` has no
//! shuffle case at all — so the fixtures below are llvmkit's, written to pin the
//! one case where the fast path is load-bearing rather than a shortcut.
//!
//! That case is a splat mask carrying a poison lane. `case
//! Instruction::ShuffleVector:` in both `computeKnownBits` and
//! `computeKnownFPClass` opens with `if (Value *Splat = getSplatValue(V))`, and
//! `m_ZeroMask` accepts poison alongside `0`, so `<0, poison, 0, 0>` still
//! matches and the answer comes from the scalar. Reaching the same arm's
//! demanded-lane path instead gives up: `getShuffleDemandedElts` returns false
//! for a poison element among the demanded lanes, which is "nothing known".
//!
//! The oracle in each case is the scalar the shuffle broadcasts — the fast path
//! is correct exactly when it agrees with that value's own analysis. Both
//! fixtures also assert against a clean all-zero mask, which reaches the same
//! answer either way, so a regression that disables the fast path fails only
//! the poison-mask half and says so.

use llvmkit_ir::{
    DynBrand, FpClassTest, Module, Unverified, Value, ValueTrackingQuery, compute_known_bits,
    compute_known_fp_class_all,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match llvmkit_asmparser::parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

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

/// `<i32 0, i32 poison, i32 0, i32 0>` — a splat mask with a poison lane.
const POISON_MASK: &str = "<i32 0, i32 poison, i32 0, i32 0>";
/// `zeroinitializer` — the same splat, spelled without poison.
const CLEAN_MASK: &str = "zeroinitializer";

/// Known bits survive a splat mask that carries a poison lane.
///
/// `%m = and i32 %x, 3` leaves the top thirty bits known zero. Broadcasting it
/// must report the same thirty, whichever way the mask is spelled — the
/// shuffle copies lane 0 everywhere and cannot introduce set bits.
#[test]
fn known_bits_read_through_a_splat_mask_with_a_poison_lane() {
    for mask in [POISON_MASK, CLEAN_MASK] {
        let source = format!(
            r"
define <4 x i32> @test(i32 %x) {{
  %m = and i32 %x, 3
  %ins = insertelement <4 x i32> poison, i32 %m, i32 0
  %A = shufflevector <4 x i32> %ins, <4 x i32> poison, <4 x i32> {mask}
  ret <4 x i32> %A
}}
"
        );
        let module = parse(&source);
        let data_layout = module.data_layout();
        let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);

        let scalar = compute_known_bits(named(&module, "m"), &query).expect("known bits");
        let splat = compute_known_bits(named(&module, "A"), &query).expect("known bits");
        assert_eq!(
            splat.zero_mask(),
            scalar.zero_mask(),
            "known zeros must match the broadcast scalar\nmask: {mask}"
        );
        assert_eq!(
            splat.one_mask(),
            scalar.one_mask(),
            "known ones must match the broadcast scalar\nmask: {mask}"
        );
        assert_eq!(
            scalar.zero_mask().count_leading_ones(),
            30,
            "the fixture's own premise: `and i32 %x, 3` knows the top thirty \
             bits are zero"
        );
    }
}

/// The float class survives a splat mask that carries a poison lane.
///
/// `nofpclass(nan)` on the parameter rules NaN out of the scalar; broadcasting
/// it must rule NaN out of every lane too.
#[test]
fn the_float_class_reads_through_a_splat_mask_with_a_poison_lane() {
    for mask in [POISON_MASK, CLEAN_MASK] {
        let source = format!(
            r"
define <4 x float> @test(float nofpclass(nan) %x) {{
  %ins = insertelement <4 x float> poison, float %x, i32 0
  %A = shufflevector <4 x float> %ins, <4 x float> poison, <4 x i32> {mask}
  ret <4 x float> %A
}}
"
        );
        let module = parse(&source);
        let data_layout = module.data_layout();
        let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);

        let splat = compute_known_fp_class_all(named(&module, "A"), &query);
        assert_eq!(
            splat.classes(),
            FpClassTest::ALL.difference(FpClassTest::NAN),
            "the broadcast scalar's `nofpclass(nan)` must reach every lane\nmask: {mask}"
        );
    }
}
