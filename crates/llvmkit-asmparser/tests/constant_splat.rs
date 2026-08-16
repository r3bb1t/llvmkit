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
use llvmkit_ir::{Constant, DynBrand, Module, Unverified};

fn initializer<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Constant<'m, DynBrand> {
    module
        .as_view()
        .globals()
        .find(|global| global.name() == name)
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
