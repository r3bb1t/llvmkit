//! `ConstantVector::getSplat` (`VectorType::const_splat`) and the splat it
//! builds: one constant whatever the lane count, the same constant as its
//! written-out lanes, and classified as upstream's constructor leaves it.

use core::num::NonZeroU32;

use llvmkit_ir::{
    Constant, ConstantAggregateKind, ConstantDataKind, ConstantExprOpcode, ConstantKind,
    ConstantSequentialKind, DynBrand, Module, ReadOnly, Type, Undefined,
};

fn lanes(count: u32) -> NonZeroU32 {
    NonZeroU32::new(count).expect("a lane count is nonzero")
}

/// `Constant::getNullValue(Ty)`, read-only.
fn null<'m>(ty: Type<'m, DynBrand>) -> Constant<'m, DynBrand, ReadOnly> {
    ty.null_value()
        .expect("float, i32 and i8 have a null value")
        .read_only()
}

/// Ports `TEST(ConstantsTest, GetSplatValueRoundTrip)`
/// (`llvm/unittests/IR/ConstantsTest.cpp`): for each of 1, 2 and 8 lanes,
/// scalable and fixed, and `float`, `i32` and `i8`, `getSplat` of the type's
/// null value and of its all-ones value answers that value back from
/// `getSplatValue`. Upstream's `getAllOnesValue` of `float` is the all-ones bit
/// pattern, `const_from_bits(0xFFFF_FFFF)` here.
#[test]
fn get_splat_value_round_trip() {
    let m = Module::dynamic("splat_round_trip");
    for min in [1, 2, 8] {
        for scalable in [true, false] {
            let values = [
                null(m.f32_type().as_type()),
                m.f32_type()
                    .const_from_bits(0xFFFF_FFFF)
                    .as_constant()
                    .read_only(),
                null(m.i32_type().as_type()),
                m.i32_type().const_all_ones().as_constant().read_only(),
                null(m.i8_type().as_type()),
                m.i8_type().const_all_ones().as_constant().read_only(),
            ];
            for c in values {
                let vector = if scalable {
                    m.scalable_vector_type(c.ty(), lanes(min))
                } else {
                    m.vector_type(c.ty(), lanes(min))
                }
                .expect("float, i32 and i8 are vector elements");
                let splat = vector.const_splat(c).expect("getSplat").as_constant();
                assert_eq!(
                    splat.splat_value(false).map(Constant::read_only),
                    Some(c),
                    "{splat}"
                );
            }
        }
    }
}

/// A splat costs one constant, whatever its lane count.
///
/// **No upstream counterpart**: upstream's fixed `getSplat` builds a
/// `ConstantDataVector` of every lane, so this is hardening
/// (`ConstantData::Splat`). The lanes still answer as upstream's do:
/// `getSplatValue`, `getAggregateElement` within and past the lane count, the
/// `ConstantDataVector` class, and the printed `splat (…)`.
#[test]
fn a_splat_costs_one_constant() {
    let m = Module::dynamic("long_splat");
    let one = m.i32_type().const_int(1_i32).as_constant();
    let vector = m
        .vector_type(m.i32_type(), lanes(u32::MAX))
        .expect("i32 is a vector element");
    let splat = vector.const_splat(one).expect("getSplat").as_constant();
    assert_eq!(splat.splat_value(false), Some(one));
    assert_eq!(splat.aggregate_element(7), Some(one));
    assert_eq!(splat.aggregate_element(u32::MAX - 1), Some(one));
    assert_eq!(splat.aggregate_element(u32::MAX), None);
    assert!(matches!(
        splat.kind(),
        ConstantKind::Data(ConstantDataKind::Sequential(ConstantSequentialKind::Vector))
    ));
    assert_eq!(splat.to_string(), "<4294967295 x i32> splat (i32 1)");
}

/// A splat and its written-out lanes are one constant, fixed or scalable.
///
/// **No upstream counterpart as a unit test.** The oracle is uniquing:
/// `ConstantVector::get` of two `i32 1` lanes and `getSplat(2, i32 1)` are the
/// same `ConstantDataVector` upstream, and a scalable list (llvmkit's spelling
/// of `getSplat`) is the same `shufflevector` splat as `getSplat`.
#[test]
fn two_spellings_of_a_splat_are_one_constant() {
    let m = Module::dynamic("one_splat");
    let one = m.i32_type().const_int(1_i32).as_constant();
    for vector in [
        m.vector_type(m.i32_type(), lanes(2)),
        m.scalable_vector_type(m.i32_type(), lanes(2)),
    ] {
        let vector = vector.expect("i32 is a vector element");
        let list = vector
            .const_vector([one, one])
            .expect("two lanes")
            .as_constant();
        let splat = vector.const_splat(one).expect("getSplat").as_constant();
        assert_eq!(list, splat, "{list} and {splat}");
    }
}

/// `getSplat` of a null, `undef` or `poison` element is the zero or the
/// marker, at the vector type.
///
/// **No upstream counterpart as a unit test.** The oracle is
/// `ConstantVector::getSplat`: its fixed branch hands every lane to a
/// constructor that canonicalises an all-null list to a
/// `ConstantAggregateZero` and an all-`undef` / all-`poison` one to the marker,
/// and its scalable branch tests `isNullValue`, `isa<PoisonValue>` and
/// `isa<UndefValue>` first.
#[test]
fn a_splat_of_null_undef_or_poison_is_the_canonical_constant() {
    let m = Module::dynamic("canonical_splats");
    let i32_ty = m.i32_type();
    for vector in [
        m.vector_type(i32_ty, lanes(4)),
        m.scalable_vector_type(i32_ty, lanes(4)),
    ] {
        let vector = vector.expect("i32 is a vector element");
        let null = vector
            .const_splat(i32_ty.as_type().null_value().expect("i32 has a null value"))
            .expect("getSplat")
            .as_constant();
        assert!(matches!(
            null.kind(),
            ConstantKind::Data(ConstantDataKind::AggregateZero)
        ));
        let undef = vector
            .const_splat(i32_ty.as_type().undef().as_constant())
            .expect("getSplat")
            .as_constant();
        assert!(matches!(
            undef.kind(),
            ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef))
        ));
        let poison = vector
            .const_splat(i32_ty.as_type().poison().as_constant())
            .expect("getSplat")
            .as_constant();
        assert!(matches!(
            poison.kind(),
            ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison))
        ));
    }
}

/// A splat is classified as upstream's constructor leaves it: a fixed splat of
/// a `ConstantInt` is a `ConstantDataVector`, a fixed splat of a global is a
/// `ConstantVector` (no `ConstantDataSequential` element), and a scalable splat
/// is the `shufflevector` expression `getSplat` builds. A `ConstantVector`
/// splat prints its lanes, as `writeConstantInternal` prints a splat whose
/// value is no `ConstantInt` or `ConstantFP`.
///
/// **No upstream counterpart as a unit test.** The oracles are
/// `getSequenceIfElementsMatch`, `ConstantVector::getSplat`'s scalable branch,
/// and `AsmWriter.cpp`'s `writeConstantInternal`.
#[test]
fn a_splat_is_classified_as_its_lanes_are() {
    let m = Module::dynamic("splat_classes");
    let one = m.i32_type().const_int(1_i32).as_constant();
    let fixed = m
        .vector_type(m.i32_type(), lanes(2))
        .expect("i32 is a vector element")
        .const_splat(one)
        .expect("getSplat")
        .as_constant();
    assert!(matches!(
        fixed.kind(),
        ConstantKind::Data(ConstantDataKind::Sequential(ConstantSequentialKind::Vector))
    ));

    let scalable = m
        .scalable_vector_type(m.i32_type(), lanes(2))
        .expect("i32 is a vector element")
        .const_splat(one)
        .expect("getSplat")
        .as_constant();
    assert!(matches!(
        scalable.kind(),
        ConstantKind::Expr(ConstantExprOpcode::ShuffleVector)
    ));
    assert_eq!(scalable.aggregate_element(0), None);
    assert_eq!(scalable.splat_value(false), Some(one));
    assert_eq!(scalable.to_string(), "<vscale x 2 x i32> splat (i32 1)");

    let global = m.add_global("g", 0_i32).expect("a fresh name");
    let pointer = m.view(global).as_global_constant_ptr();
    let pointers = m
        .vector_type(pointer.ty(), lanes(2))
        .expect("ptr is a vector element")
        .const_splat(pointer)
        .expect("getSplat")
        .as_constant();
    assert!(matches!(
        pointers.kind(),
        ConstantKind::Aggregate(ConstantAggregateKind::Vector)
    ));
    assert_eq!(pointers.to_string(), "<2 x ptr> <ptr @g, ptr @g>");
}
