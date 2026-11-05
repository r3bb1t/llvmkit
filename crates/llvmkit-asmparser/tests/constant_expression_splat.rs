//! `Constant::getSplatValue`'s constant-*expression* arm.
//!
//! **No upstream unit test covers this.** `ConstantsTest.cpp` has no
//! `getSplatValue` case at all, so these fixtures are llvmkit's, with
//! expectations read off the closing `dyn_cast<ConstantExpr>` block of
//! `Constant::getSplatValue` (`llvm/lib/IR/Constants.cpp`).
//!
//! The shape is the one `ConstantVector::getSplat` builds:
//!
//! ```text
//! shufflevector (insertelement (undef, Splat, 0), undef, zeroinitializer)
//! ```
//!
//! # Why every fixture here is scalable
//!
//! For a *fixed* vector this arm is unreachable, and finding that out is what
//! these fixtures are for. llvmkit's constant folder materialises a fixed
//! shufflevector expression at construction — `<4 x ptr> shufflevector (…)`
//! becomes `<ptr @x, ptr @x, ptr @x, ptr @x>` before anything can ask it — so
//! the element-list arm answers and this one never runs. A first draft of this
//! file used `<4 x …>` throughout and its positive case passed *for the wrong
//! reason*; only the negative cases, which the folder answered `Some` for,
//! gave that away.
//!
//! A scalable vector has no lane count to materialise, so the expression
//! survives, which is exactly why upstream has this arm at all.
//!
//! # The mask is read from the operand
//!
//! `ConstantExprData` carries both a `mask` field and a third operand; only
//! the operand is live, because `validate_constant_expr_data` rejects a
//! `ShuffleVector` expression whose `mask` field is non-empty and every
//! construction site passes an empty one. Reading the wrong one would look
//! like it worked — an empty mask makes `all_of(… == 0)` vacuously true, so
//! *every* shuffle would answer "splat". The undef-mask case below is what
//! catches that.

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

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The canonical shape answers the inserted scalar.
///
/// The printed form is asserted too, because the whole point of the scalable
/// spelling is that the expression is *still an expression* when it is asked —
/// a folded initializer would take the element-list path instead and this
/// arm's coverage would be silently zero.
#[test]
fn the_canonical_constant_expression_splat_is_recognised() {
    let module = parse(
        "@x = global i32 7\n\
         @g = global <vscale x 4 x ptr> shufflevector (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> undef, ptr @x, i32 0), <vscale x 4 x ptr> undef, <vscale x 4 x i32> zeroinitializer)",
    );
    let printed = format!("{module}");
    assert!(
        printed.contains("@g = global <vscale x 4 x ptr> shufflevector ("),
        "the initializer must still be an expression, not folded away:\n{printed}"
    );

    let splat = initializer(&module, "g")
        .splat_value(false)
        .expect("the canonical constant-expression splat");
    assert!(
        splat.ty().is_pointer(),
        "the answer is the inserted scalar, not the vector"
    );
}

/// An `undef` mask makes every lane poison, and the folder says so before this
/// arm is reached.
///
/// Upstream's `all_of(Mask, … == 0)` would also fail here — `getShuffleMask`
/// reads an undef lane back as `-1` — so both routes agree that the shape is
/// not the splat idiom. They disagree only on what it *is*: llvmkit folds it to
/// `poison`, and `Constant::getSplatValue`'s first arm answers poison of the
/// element type for that, which is correct. The case is kept because it is the
/// one that revealed the folder runs first.
#[test]
fn an_undef_mask_folds_to_poison_before_this_arm_is_reached() {
    let module = parse(
        "@x = global i32 7\n\
         @g = global <vscale x 4 x ptr> shufflevector (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> undef, ptr @x, i32 0), <vscale x 4 x ptr> undef, <vscale x 4 x i32> undef)",
    );
    let printed = format!("{module}");
    assert!(
        printed.contains("@g = global <vscale x 4 x ptr> poison"),
        "expected the folder to answer poison:\n{printed}"
    );
    let splat = initializer(&module, "g")
        .splat_value(false)
        .expect("a poison vector splats to poison of the element type");
    assert!(splat.ty().is_pointer());
}

/// Inserting at a lane other than 0 makes every shuffled lane `undef`, so the
/// folder answers before this arm is reached: `ConstantFoldShuffleVectorInstruction`'s
/// all-zero-mask arm extracts lane 0, which is `undef`, and returns
/// `ConstantVector::getSplat(MaskEltCount, Elt)`, whose scalable branch answers
/// an `isa<UndefValue>` value with `UndefValue::get(VTy)`. That prints as the
/// whole-vector `undef`.
///
/// **This case has pinned two wrong answers before this one.** llvmkit
/// represents a scalable splat as `min_len` equal elements, a choice upstream
/// cannot make, and it kept a list of `undef`s too. First the printer wrote it
/// as an element list, which LLVM rejects for a scalable type:
///
/// ```text
/// @g = global <vscale x 4 x ptr> <ptr undef, ptr undef, ptr undef, ptr undef>
/// ```
///
/// then, once `asm_writer::prints_as_splat` lifted the int/fp restriction for
/// scalable types, as `splat (ptr undef)`. Upstream builds no list at all, and
/// now neither does llvmkit: `getSplat`'s canonicalisation runs where the list
/// is interned. `scalable_vector_splat_printing.rs` keeps the printer rule
/// covered for a uniform list that is not `undef`.
#[test]
fn a_scalable_uniform_undef_vector_prints_as_undef() {
    let module = parse(
        "@x = global i32 7\n\
         @g = global <vscale x 4 x ptr> shufflevector (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> undef, ptr @x, i32 1), <vscale x 4 x ptr> undef, <vscale x 4 x i32> zeroinitializer)",
    );
    let printed = format!("{module}");
    assert!(
        printed.contains("@g = global <vscale x 4 x ptr> undef\n"),
        "{printed}"
    );
    assert!(
        !printed.contains("ptr undef, ptr undef"),
        "a scalable vector has no element list to write:\n{printed}"
    );
}

/// The shuffle's second operand must be `undef` — `isa<UndefValue>` on
/// `Shuf->getOperand(1)`.
#[test]
fn a_defined_second_shuffle_operand_is_not_a_splat() {
    let module = parse(
        "@x = global i32 7\n\
         @g = global <vscale x 4 x ptr> shufflevector (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> undef, ptr @x, i32 0), <vscale x 4 x ptr> zeroinitializer, <vscale x 4 x i32> zeroinitializer)",
    );
    assert!(initializer(&module, "g").splat_value(false).is_none());
}

/// The `insertelement`'s own vector operand must be `undef` too —
/// `isa<UndefValue>(IElt->getOperand(0))`.
#[test]
fn a_defined_insert_target_is_not_a_splat() {
    let module = parse(
        "@x = global i32 7\n\
         @g = global <vscale x 4 x ptr> shufflevector (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> zeroinitializer, ptr @x, i32 0), <vscale x 4 x ptr> undef, <vscale x 4 x i32> zeroinitializer)",
    );
    assert!(initializer(&module, "g").splat_value(false).is_none());
}

/// A fixed-width vector never reaches this arm: the folder materialises the
/// expression into an element list first, and `Constant::getSplatValue`'s
/// earlier arm answers from that.
///
/// Pinned so the "why every fixture here is scalable" note above stays true —
/// if folding ever stops happening, this fails and says where to look.
#[test]
fn a_fixed_width_splat_expression_is_folded_before_it_can_be_asked() {
    let module = parse(
        "@x = global i32 7\n\
         @g = global <4 x ptr> shufflevector (<4 x ptr> insertelement (<4 x ptr> undef, ptr @x, i32 0), <4 x ptr> undef, <4 x i32> zeroinitializer)",
    );
    let printed = format!("{module}");
    assert!(
        printed.contains("@g = global <4 x ptr> <ptr @x, ptr @x, ptr @x, ptr @x>"),
        "expected the folder to materialise the expression:\n{printed}"
    );
    // Still a splat — by the element-list arm, not this one.
    assert!(initializer(&module, "g").splat_value(false).is_some());
}
