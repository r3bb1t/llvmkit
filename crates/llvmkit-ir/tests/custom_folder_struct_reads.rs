//! A custom folder on the checked `try_` path of a derived field read.
//!
//! The infallible reads take only llvmkit's own folders (`TrustedFolder`),
//! whose struct folding is type-correct by construction. A folder of the
//! user's own is still consulted on every read — upstream's
//! `IRBuilderBase::CreateExtractValue` always asks its folder
//! (`Folder.FoldExtractValue`) — so its builder reads through `try_x`, which
//! checks what the folder hands back.

use core::cell::Cell;

use llvmkit_ir::{
    IrBuilder, IrBuilderFolder, IrError, IrResult, IrStruct, Linkage, ModuleBrand, Value,
    module_new,
};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

/// Counts its extract-value folds; when `answer` holds a value, hands it back
/// as the fold of every one.
struct CountingFolder<'a, 'ctx, B: ModuleBrand> {
    reads: &'a Cell<u32>,
    answer: Option<Value<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> IrBuilderFolder<'ctx, B> for CountingFolder<'_, 'ctx, B> {
    fn fold_extract_value_dyn(
        &self,
        aggregate: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (aggregate, indices);
        self.reads.set(self.reads.get() + 1);
        Ok(self.answer)
    }
}

/// Counts its insert-value folds; when `answer` holds a value, hands it back
/// as the fold of every one.
struct WriteFolder<'a, 'ctx, B: ModuleBrand> {
    writes: &'a Cell<u32>,
    answer: Option<Value<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> IrBuilderFolder<'ctx, B> for WriteFolder<'_, 'ctx, B> {
    fn fold_insert_value_dyn(
        &self,
        aggregate: Value<'ctx, B>,
        value: Value<'ctx, B>,
        indices: &[u32],
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (aggregate, value, indices);
        self.writes.set(self.writes.get() + 1);
        Ok(self.answer)
    }
}

/// One field that is neither an integer nor a pointer, the shape
/// `Type::require_match` has no special case for.
#[derive(IrStruct)]
struct Ratio {
    value: f64,
}

/// Review Focus 4's build half (llvmkit-specific; mirrors
/// `IRBuilderBase::CreateInsertValue`, which consults `Folder.FoldInsertValue`
/// on every write): a custom folder sees each field of a `try_build`, and a fold
/// of the wrong type is refused. A write's fold must have the aggregate's type,
/// so this answer is `TypeIdentityMismatch` whatever the field is: the width and
/// address-space shapes of `Type::require_match` need two integers or two
/// pointers, and the aggregate is a struct.
#[test]
fn a_custom_folder_sees_try_builds_and_its_wrong_fold_is_refused() -> Result<(), IrError> {
    let Point { x, y } = Point { x: 1, y: 2 };
    let _ = x + y;

    let m = module_new!("custom_folder_builds")?;
    let point = m.get_or_insert_struct_of::<Point>()?;

    let f = m.add_typed_function::<(), (i32,), _>("build", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let writes = Cell::new(0);
    let b = IrBuilder::with_folder(
        &m,
        WriteFolder {
            writes: &writes,
            answer: None,
        },
    )
    .position_at_end(entry);
    let (value,) = m.view(f).params();
    let _ = point.try_build(&b, value, value, "p")?;
    assert_eq!(writes.get(), 2, "the folder sees each field");
    b.ret_void();

    let g = m.add_typed_function::<(), (i32,), _>("build_wrong", Linkage::External)?;
    let entry = m.view(g).append_basic_block(&m, "entry");
    let wrong_writes = Cell::new(0);
    let wrong = IrBuilder::with_folder(
        &m,
        WriteFolder {
            writes: &wrong_writes,
            answer: Some(m.i64_type().const_int(7_i64).as_erased()),
        },
    )
    .position_at_end(entry);
    let (value,) = m.view(g).params();
    let Err(IrError::TypeIdentityMismatch { expected, got }) =
        point.try_build(&wrong, value, value, "p").map(|_| ())
    else {
        panic!("an `i64` is no fold of a `%Point`");
    };
    assert_eq!(expected.spelling(), "%Point");
    assert_eq!(got.spelling(), "i64");
    assert_eq!(wrong_writes.get(), 1);
    wrong.ret_void();
    Ok(())
}

/// Review Focus 4 (llvmkit-specific; mirrors `IRBuilderBase::CreateExtractValue`,
/// which consults `Folder.FoldExtractValue`): a wrong fold of a field that is
/// neither integer nor pointer answers `Type::require_match`'s general shape,
/// `TypeIdentityMismatch`, with both types spelled. The plan named this variant
/// for every wrong fold; `OperandWidthMismatch` is the integer field's answer and
/// the better one, which the sibling test pins.
#[test]
fn a_wrong_fold_of_a_float_field_is_refused_with_both_types() -> Result<(), IrError> {
    let Ratio { value } = Ratio { value: 0.5 };
    let _ = value;

    let m = module_new!("custom_folder_float_read")?;
    let f = m.add_typed_function::<(), (Ratio,), _>("read", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let reads = Cell::new(0);
    let wrong = IrBuilder::with_folder(
        &m,
        CountingFolder {
            reads: &reads,
            answer: Some(m.i64_type().const_int(7_i64).as_erased()),
        },
    )
    .position_at_end(entry);
    let (ratio,) = m.view(f).params();
    let Err(IrError::TypeIdentityMismatch { expected, got }) = ratio.try_value(&wrong).map(|_| ())
    else {
        panic!("an `i64` is no fold of a `double` field");
    };
    assert_eq!(expected.spelling(), "double");
    assert_eq!(got.spelling(), "i64");
    assert_eq!(reads.get(), 1);
    wrong.ret_void();
    Ok(())
}

/// Review Focus 4 of the type-constructor validity plan (llvmkit-specific;
/// mirrors `IRBuilderBase::CreateExtractValue`, which consults
/// `Folder.FoldExtractValue` on every read): a custom folder sees each `try_`
/// read of a derived field, and a fold of the wrong type — an `i64` for an
/// `i32` field — is refused by the builder's check of every folder answer
/// (`Type::require_match`, here `OperandWidthMismatch`) instead of being handed
/// back as the field.
#[test]
fn a_custom_folder_sees_try_reads_and_its_wrong_fold_is_refused() -> Result<(), IrError> {
    let Point { x, y } = Point { x: 1, y: 2 };
    let _ = x + y;

    let m = module_new!("custom_folder_reads")?;
    let f = m.add_typed_function::<(), (Point,), _>("read", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let reads = Cell::new(0);
    let b = IrBuilder::with_folder(
        &m,
        CountingFolder {
            reads: &reads,
            answer: None,
        },
    )
    .position_at_end(entry);
    let (point,) = m.view(f).params();
    let _ = point.try_x(&b)?;
    let _ = point.try_y(&b)?;
    assert_eq!(reads.get(), 2, "the folder sees each read");
    b.ret_void();

    let g = m.add_typed_function::<(), (Point,), _>("read_wrong", Linkage::External)?;
    let entry = m.view(g).append_basic_block(&m, "entry");
    let wrong_reads = Cell::new(0);
    let wrong = IrBuilder::with_folder(
        &m,
        CountingFolder {
            reads: &wrong_reads,
            answer: Some(m.i64_type().const_int(7_i64).as_erased()),
        },
    )
    .position_at_end(entry);
    let (point,) = m.view(g).params();
    // `Type::require_match`'s shape for two integers of different widths: the
    // field's 32 bits against the fold's 64.
    assert_eq!(
        point.try_x(&wrong).map(|_| ()),
        Err(IrError::OperandWidthMismatch { lhs: 32, rhs: 64 })
    );
    assert_eq!(wrong_reads.get(), 1);
    wrong.ret_void();
    Ok(())
}
