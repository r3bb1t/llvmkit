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

/// `Constant::getAllOnesValue(Ty)`, read-only.
fn all_ones<'m>(ty: Type<'m, DynBrand>) -> Constant<'m, DynBrand, ReadOnly> {
    ty.all_ones_value()
        .expect("float, i32 and i8 have an all-ones value")
        .read_only()
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
/// `getSplatValue`, the values built as upstream builds them
/// (`Constant::getNullValue`, `Constant::getAllOnesValue`).
#[test]
fn get_splat_value_round_trip() {
    let m = Module::dynamic("splat_round_trip");
    for min in [1, 2, 8] {
        for scalable in [true, false] {
            let values = [
                null(m.f32_type().as_type()),
                all_ones(m.f32_type().as_type()),
                null(m.i32_type().as_type()),
                all_ones(m.i32_type().as_type()),
                null(m.i8_type().as_type()),
                all_ones(m.i8_type().as_type()),
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

/// A fixed splat of `@g` that is a `ConstantVector` is one use of `@g`, not
/// one per lane (`docs/divergences.md` D39).
///
/// **No upstream counterpart.** Upstream's `ConstantVector` keeps every lane
/// as an operand, so `<2 x ptr> splat (ptr @g)` is two uses of `@g` there;
/// llvmkit stores the one element and registers one use. This pins that
/// answer, so a change to the splat's use registration is a deliberate one.
#[test]
fn a_constant_vector_splat_is_one_use_of_its_element() {
    let m = Module::dynamic("splat_uses");
    let global = m.add_global("g", 0_i32).expect("a fresh name");
    let pointer = m.view(global).as_global_constant_ptr();
    let before = pointer.as_erased().num_uses();
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
    // Upstream: `before + 2`, one per lane.
    assert_eq!(pointer.as_erased().num_uses(), before + 1);
}

/// `ConstantFoldSelectInstruction`'s `NotPoison` refuses a vector that
/// `containsConstantExpression`, and a lane is an expression by its class:
/// llvmkit's compact `getelementptr inbounds (i8, ptr @g, i64 4)`
/// (`as_global_constant_ptr_offset`) is a `ConstantExpr` upstream, so a
/// splat of it does not let `select` of `undef` and the splat fold to the
/// splat. A splat of `ptr @g` does.
///
/// **No upstream unit test isolates it**; read off
/// `ConstantFoldSelectInstruction` (`llvm/lib/IR/ConstantFold.cpp`) and
/// `Constant::containsConstantExpression` (`llvm/lib/IR/Constants.cpp`).
#[test]
fn a_splat_of_a_compact_expression_is_no_select_arm_to_fold_to() {
    use llvmkit_ir::{AddressSpace, constant_fold_select_instruction};
    let m = Module::dynamic("select_arm");
    let global = m.add_global("g", 0_i64).expect("a fresh name");
    let pointer = m.view(global).as_global_constant_ptr();
    let offset = m
        .view(global)
        .as_global_constant_ptr_offset(4, AddressSpace::default());
    assert!(matches!(offset.kind(), ConstantKind::Expr(_)), "{offset}");
    let vector = m
        .vector_type(pointer.ty(), lanes(2))
        .expect("ptr is a vector element");
    let offsets = vector.const_splat(offset).expect("getSplat").as_constant();
    let plain = vector.const_splat(pointer).expect("getSplat").as_constant();
    let undefined = vector.as_type().undef().as_constant();
    let bools = m
        .vector_type(m.bool_type(), lanes(2))
        .expect("i1 is a vector element");
    let condition = m
        .constant_expr(
            bools.as_type(),
            ConstantExprOpcode::PtrToInt,
            [plain.as_erased()],
            [],
            [],
            llvmkit_ir::ConstantExprFlags::none(),
        )
        .expect("ptrtoint <2 x ptr> to <2 x i1>");
    assert_eq!(
        constant_fold_select_instruction(condition, undefined, offsets).expect("one module"),
        None
    );
    assert_eq!(
        constant_fold_select_instruction(condition, undefined, plain).expect("one module"),
        Some(plain)
    );
}
