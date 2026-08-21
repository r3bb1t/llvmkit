//! Aggregate / vector op coverage: `extractvalue`, `insertvalue`,
//! `extractelement`, `insertelement`, `shufflevector`.
//!
//! Every test cites its upstream source per Doctrine D11, except the one
//! that says plainly it has none: llvmkit's cross-module tag has no LLVM
//! counterpart to port.

use llvmkit_ir::ShuffleMaskElem::{Lane, Poison};
use llvmkit_ir::{
    Dyn, DynBrand, IntValue, IrBuilder, IrError, Linkage, Module, ShuffleVectorInst, module_new,
};

// --------------------------------------------------------------------------
// extractvalue
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1549:
/// `extractvalue { i8, i32 } %up, 0`. Locks the print form and result
/// type for an unpacked struct extract.
#[test]
fn extract_value_struct_field0() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()]);
    let fn_ty = m.function_type(void_ty.as_type(), [s_ty.as_type()]);
    let f = m.add_function_dyn("instructions.aggregateops", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let up = m.view(f).param(0)?;
    let _ = b.extract_value(up, [0u32], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    // Mirrors `; CHECK: extractvalue { i8, i32 } %up, 0` (line 1550).
    assert!(
        text.contains("extractvalue { i8, i32 } %0, 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 1553:
/// `extractvalue [3 x i8] %arr, 2`.
#[test]
fn extract_value_array_index() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let arr_ty = m.array_type(i8_ty, 3);
    let fn_ty = m.function_type(void_ty.as_type(), [arr_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arr = m.view(f).param(0)?;
    let _ = b.extract_value(arr, [2u32], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("extractvalue [3 x i8] %0, 2\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 1555:
/// `extractvalue { i8, { i32 } } %n, 1, 0`. Verifies the multi-index
/// path walks struct → struct → leaf.
#[test]
fn extract_value_nested_indices() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let inner = m.struct_type([i32_ty.as_type()]);
    let outer = m.struct_type([i8_ty.as_type(), inner.as_type()]);
    let fn_ty = m.function_type(void_ty.as_type(), [outer.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n = m.view(f).param(0)?;
    let _ = b.extract_value(n, [1u32, 0u32], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("extractvalue { i8, { i32 } } %0, 1, 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `ExtractValueInst::init` (`lib/IR/Instructions.cpp`): LLVM rejects
/// `extractvalue` with an empty index list. The typed `extract_value`
/// upgrades this to a compile-time `const { assert!(N > 0) }` failure (see
/// `tests/compile_fail/extract_value_empty_indices.rs`); `extract_value_dyn`
/// keeps the runtime check for slice/`Vec`-driven index lists, ported from
/// the assembler diagnostic in `test/Assembler/extractvalue-no-idx.ll`.
#[test]
fn extract_value_dyn_rejects_empty_indices() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()]);
    let fn_ty = m.function_type(void_ty.as_type(), [s_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let up = m.view(f).param(0)?;
    let err = b
        .extract_value_dyn(up, &[], "bad")
        .expect_err("empty extractvalue indices must be rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "extractvalue indices must not be empty",
        }
    );
    assert_eq!(b.insert_block().instructions().len(), 0);
    b.ret_void()?;
    Ok(())
}

/// Ports `test/Assembler/extractvalue-invalid-idx.ll` (PR4170):
/// `extractvalue [0 x i32] undef, 0` is rejected because index 0 is
/// out of range for a zero-element array. Mirrors
/// `ExtractValueInst::getIndexedType` (`lib/IR/Instructions.cpp`),
/// which returns null (rather than clamping) once `Index >=
/// AT->getNumElements()`.
#[test]
fn extract_value_rejects_out_of_range_array_index() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let arr_ty = m.array_type(i32_ty, 0);
    let fn_ty = m.function_type(void_ty.as_type(), [arr_ty.as_type()]);
    let f = m.add_function_dyn("test", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let undef = arr_ty.as_type().undef();
    let err = b
        .extract_value(undef, [0u32], "")
        .expect_err("index 0 into a 0-element array must be rejected");
    assert_eq!(
        err,
        IrError::AggregateIndexOutOfRange { index: 0, count: 0 }
    );
    assert_eq!(b.insert_block().instructions().len(), 0);
    b.ret_void()?;
    Ok(())
}

/// Ports `test/Assembler/extractvalue-invalid-idx.ll` (PR4170), struct variant:
/// `extractvalue { i8, i32 } undef, 2` is rejected because index 2 is
/// out of range for a 2-field struct. Mirrors
/// `ExtractValueInst::getIndexedType` (`lib/IR/Instructions.cpp`),
/// which returns null (rather than clamping) once `Index >=
/// ST->getNumElements()`.
#[test]
fn extract_value_rejects_out_of_range_struct_index() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()]);
    let fn_ty = m.function_type(void_ty.as_type(), [s_ty.as_type()]);
    let f = m.add_function_dyn("test", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let undef = s_ty.as_type().undef();
    let err = b
        .extract_value(undef, [2u32], "")
        .expect_err("index 2 into a 2-field struct must be rejected");
    assert_eq!(
        err,
        IrError::AggregateIndexOutOfRange { index: 2, count: 2 }
    );
    assert_eq!(b.insert_block().instructions().len(), 0);
    b.ret_void()?;
    Ok(())
}

// --------------------------------------------------------------------------
// insertvalue
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1558:
/// `insertvalue { i8, i32 } %up, i8 1, 0`.
#[test]
fn insert_value_struct_field0() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()]);
    let fn_ty = m.function_type(void_ty.as_type(), [s_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let up = m.view(f).param(0)?;
    let one = i8_ty.const_int(1_i8);
    let _ = b.insert_value(up, one, [0u32], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("insertvalue { i8, i32 } %0, i8 1, 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 1562:
/// `insertvalue [3 x i8] %arr, i8 0, 0`.
#[test]
fn insert_value_array_index_zero() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let arr_ty = m.array_type(i8_ty, 3);
    let fn_ty = m.function_type(void_ty.as_type(), [arr_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arr = m.view(f).param(0)?;
    let zero = i8_ty.const_int(0_i8);
    let _ = b.insert_value(arr, zero, [0u32], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("insertvalue [3 x i8] %0, i8 0, 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `InsertValueInst::init` (`lib/IR/Instructions.cpp`): LLVM rejects
/// `insertvalue` with an empty index list. The typed `insert_value`
/// upgrades this to a compile-time `const { assert!(N > 0) }` failure (see
/// `tests/compile_fail/extract_value_empty_indices.rs` for the shared
/// pattern); `insert_value_dyn` keeps the runtime check for
/// slice/`Vec`-driven index lists, ported from the assembler diagnostic in
/// `test/Assembler/extractvalue-no-idx.ll`.
#[test]
fn insert_value_dyn_rejects_empty_indices() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()]);
    let fn_ty = m.function_type(void_ty.as_type(), [s_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let up = m.view(f).param(0)?;
    let err = b
        .insert_value_dyn(up, up, &[], "bad")
        .expect_err("empty insertvalue indices must be rejected");
    assert_eq!(
        err,
        IrError::InvalidOperation {
            message: "insertvalue indices must not be empty",
        }
    );
    assert_eq!(b.insert_block().instructions().len(), 0);
    b.ret_void()?;
    Ok(())
}

// --------------------------------------------------------------------------
// extractelement
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1535:
/// `extractelement <4 x float> %vec, i8 0`. Locks the print form for
/// vector + integer-indexed extract.
#[test]
fn extract_element_vector_i8_index() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(f32_ty, 4);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type()]);
    let f = m.add_function_dyn("instructions.vectorops", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let vec = m.view(f).param(0)?;
    let zero = i8_ty.const_int(0_i8);
    let _ = b.extract_element(vec, zero, "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("extractelement <4 x float> %0, i8 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// insertelement
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1537:
/// `insertelement <4 x float> %vec, float 3.500000e+00, i8 0`.
#[test]
fn insert_element_vector_float_at_i8() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(f32_ty, 4);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let vec = m.view(f).param(0)?;
    let three_five = f32_ty.const_float(3.5_f32);
    let zero = i8_ty.const_int(0_i8);
    let _ = b.insert_element(vec, three_five, zero, "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("insertelement <4 x float> %0, float 3.500000e+00, i8 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// shufflevector
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1539:
/// `shufflevector <4 x float> %vec, <4 x float> %vec2, <2 x i32> zeroinitializer`.
/// Locks the all-zero mask print form (`zeroinitializer`).
#[test]
fn shuffle_vector_zeroinitializer_mask() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(f32_ty, 4);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type(), vec_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v0 = m.view(f).param(0)?;
    let v1 = m.view(f).param(1)?;
    let _ = b.shuffle_vector(v0, v1, &[Lane(0), Lane(0)], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("shufflevector <4 x float> %0, <4 x float> %1, <2 x i32> zeroinitializer\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors the explicit-mask print path in `printShuffleMask`
/// (`lib/IR/AsmWriter.cpp`). The shape is exercised by
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, ShuffleMaskQueries)`,
/// which constructs explicit-element masks like `{C0, CU, C2, C3, C4}`
/// (mixing constant integers with `undef`/poison entries). We assert
/// that an explicit-element mask emits the canonical `<i32 N, ...>`
/// form rather than the `zeroinitializer` / `poison` short forms.
#[test]
fn shuffle_vector_explicit_mask_print() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(i32_ty, 4);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type(), vec_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v0 = m.view(f).param(0)?;
    let v1 = m.view(f).param(1)?;
    let _ = b.shuffle_vector(v0, v1, &[Lane(1), Lane(1), Lane(0), Lane(0)], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    // Asserts the canonical `<<N> x i32> <i32 e0, ...>` body that the
    // upstream `printShuffleMask` produces for non-zero, non-poison masks.
    assert!(
        text.contains(
            "shufflevector <4 x i32> %0, <4 x i32> %1, <4 x i32> <i32 1, i32 1, i32 0, i32 0>\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/vscale-round-trip.ll`'s `@non_const_shufflevector`
/// through the builder rather than the parser:
/// `%res = shufflevector <vscale x 4 x i32> %lhs, <vscale x 4 x i32> %rhs, <vscale x 4 x i32> zeroinitializer`.
///
/// Locks two things `ShuffleVectorInst` decides together: that
/// `isValidOperands`' scalable branch admits an all-`Lane(0)` mask, and that
/// the `ArrayRef<int>` constructor builds
/// `VectorType::get(EltTy, Mask.size(), isa<ScalableVectorType>(V1->getType()))`
/// — so the result type is scalable and `printShuffleMask` writes the
/// `vscale x ` prefix.
#[test]
fn shuffle_vector_scalable_zero_mask_splat() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let vec_ty = m.scalable_vector_type(i32_ty, 4);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type(), vec_ty.as_type()]);
    let f = m.add_function_dyn("non_const_shufflevector", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v0 = m.view(f).param(0)?;
    let v1 = m.view(f).param(1)?;
    let _ = b.shuffle_vector(v0, v1, &[Lane(0), Lane(0), Lane(0), Lane(0)], "res")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "%res = shufflevector <vscale x 4 x i32> %0, <vscale x 4 x i32> %1, <vscale x 4 x i32> zeroinitializer\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

/// The other mask `ShuffleVectorInst::isValidOperands`' scalable branch
/// admits: `Mask[0] == PoisonMaskElem` with `all_equal(Mask)`.
///
/// No upstream `.ll` fixture writes it — `test/Bitcode/vscale-round-trip.ll`
/// covers only the zero mask — so the source is the routine itself plus
/// `ShuffleVectorInst::convertShuffleMaskForBitcode`, whose scalable arm
/// answers `PoisonValue::get(VecTy)` when `Mask[0] != 0`, which is the
/// `poison` spelling `printShuffleMask` then emits.
#[test]
fn shuffle_vector_scalable_poison_mask() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let vec_ty = m.scalable_vector_type(i32_ty, 4);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type(), vec_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v0 = m.view(f).param(0)?;
    let v1 = m.view(f).param(1)?;
    let _ = b.shuffle_vector(v0, v1, &[Poison, Poison, Poison, Poison], "")?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "shufflevector <vscale x 4 x i32> %0, <vscale x 4 x i32> %1, <vscale x 4 x i32> poison\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

/// `ShuffleVectorInst::isValidOperands`' scalable branch, negative half: a
/// scalable operand with a mask that is neither all-zero nor all-poison is
/// refused by `(Mask[0] != 0 && Mask[0] != PoisonMaskElem) || !all_equal(Mask)`.
///
/// The two cases are its two disjuncts: `Lane(1)` fails the first, and a
/// mixed `Lane(0)` / `Poison` mask fails `all_equal`. `ShuffleMaskElem`'s
/// `Poison` is upstream's `PoisonMaskElem`.
#[test]
fn shuffle_vector_scalable_rejects_a_non_splat_mask() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let vec_ty = m.scalable_vector_type(i32_ty, 4);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type(), vec_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v0 = m.view(f).param(0)?;
    let v1 = m.view(f).param(1)?;
    for mask in [
        [Lane(1), Lane(1), Lane(1), Lane(1)],
        [Lane(0), Poison, Lane(0), Lane(0)],
    ] {
        assert!(
            matches!(
                b.shuffle_vector(v0, v1, &mask, ""),
                Err(IrError::InvalidOperation {
                    message: "invalid shufflevector operands"
                })
            ),
            "{mask:?}"
        );
    }
    Ok(())
}

/// `ShuffleVectorInst::isValidOperands`' mask-range clause,
/// `if (Elem != PoisonMaskElem && Elem >= V1Size * 2) return false;`, which
/// llvmkit's instruction path did not implement at all: a lane at or past
/// `2 * V1Size` names neither source vector.
///
/// The constant-expression twin is
/// `crates/llvmkit-asmparser/tests/parser_constants.rs::constant_expr_shufflevector_rejects_out_of_range_mask`;
/// this is the instruction form of the same rule.
#[test]
fn shuffle_vector_rejects_an_out_of_range_mask_lane() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(i32_ty, 2);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ty.as_type(), vec_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v0 = m.view(f).param(0)?;
    let v1 = m.view(f).param(1)?;
    // `V1Size * 2` is 4, so `Lane(3)` is the last legal lane and `Lane(4)` the
    // first illegal one.
    assert!(b.shuffle_vector(v0, v1, &[Lane(3), Lane(0)], "ok").is_ok());
    assert!(matches!(
        b.shuffle_vector(v0, v1, &[Lane(4), Lane(0)], ""),
        Err(IrError::InvalidOperation {
            message: "invalid shufflevector operands"
        })
    ));
    Ok(())
}

/// Ports `test/Assembler/constant-splat.ll`'s `@ret_scalable_vector_ptr`
/// through the builder rather than the parser. Upstream writes
/// `ret <vscale x 4 x ptr> splat (ptr @my_global)` and its CHECK pins the
/// expansion, which is the constant expression this test builds directly:
/// `shufflevector (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> poison, ptr @my_global, i64 0), <vscale x 4 x ptr> poison, <vscale x 4 x i32> zeroinitializer)`.
///
/// The point is the *unfolded* scalable shuffle. `ConstantFoldShuffleVectorInstruction`'s
/// all-zero-mask arm folds to `ConstantAggregateZero` only when lane 0 is null,
/// and reaches `ConstantVector::getSplat` only for a fixed mask; a scalable
/// operand with a non-null lane 0 therefore falls through to its
/// `if (isa<ScalableVectorType>(V1VTy)) return nullptr;` and the expression
/// survives. `printShuffleMask` then writes the mask as `zeroinitializer`.
///
/// Reachable through `IrBuilder::shuffle_vector` only since
/// `ShuffleVectorInst::isValidOperands` was ported -- before that the builder
/// refused every scalable operand, so the folder branch was dead here and the
/// shape existed only on the constant-expression path.
#[test]
fn shuffle_vector_scalable_constant_operand_survives_folding() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let my_global = m.add_global_uninitialized("my_global", i32_ty.as_type())?;
    let global_ptr = m.view(my_global).as_global_constant_ptr();
    let vec_ty = m.scalable_vector_type(global_ptr.ty(), 4);
    let no_parameters: [llvmkit_ir::Type<'_, _>; 0] = [];
    let fn_ty = m.function_type(vec_ty.as_type(), no_parameters);
    let f = m.add_function_dyn("ret_scalable_vector_ptr", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let poison = vec_ty.as_type().poison();
    let inserted = b.insert_element(poison, global_ptr, i64_ty.const_int(0_i64), "")?;
    let shuffled = b.shuffle_vector(
        m.view(inserted),
        poison,
        &[Lane(0), Lane(0), Lane(0), Lane(0)],
        "",
    )?;
    b.ret(m.view(shuffled))?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "ret <vscale x 4 x ptr> shufflevector (<vscale x 4 x ptr> insertelement (<vscale x 4 x ptr> poison, ptr @my_global, i64 0), <vscale x 4 x ptr> poison, <vscale x 4 x i32> zeroinitializer)
"
        ),
        "got:
{text}"
    );
    Ok(())
}

/// **No upstream counterpart.** LLVM's `ShuffleVectorInst::isValidOperands`
/// takes three `Value *`s from one `LLVMContext` and has nothing to check here;
/// module identity is llvmkit's own invariant, so there is no fixture to port
/// and no upstream behaviour to mirror. What is being locked is the guard
/// itself.
///
/// Two [`module_new!`](llvmkit_ir::module_new) modules cannot express the
/// mistake — their distinct generated brand types make the cross-module call a
/// compile error, so the runtime check is unreachable. Two
/// [`llvmkit_ir::DynBrand`] modules share one brand type, which is precisely
/// why the [`ModuleId`](llvmkit_ir::ModuleId) tag has to hold the line.
///
/// Without the tag test the mask's slot is looked up in V1's arena, and both
/// outcomes are wrong: a slot past the end reaches `Context::value_data`'s
/// `unreachable!("invalid ValueSlot: out of arena range (cross-module
/// mixing?)")`, and a slot that happens to be in range reads a different value
/// entirely and answers about that. Module A here is given extra constants so
/// its mask lands past the end of B's arena, i.e. on the panicking half.
#[test]
fn shuffle_vector_operands_reject_a_mask_from_another_module() -> Result<(), IrError> {
    let a = Module::dynamic("shuffle-mask-a");
    let b = Module::dynamic("shuffle-mask-b");

    // Module A: padding constants first, so the mask below lands at an arena
    // index past anything module B holds.
    let a_i32 = a.i32_type();
    for value in 0..64_i32 {
        let _ = a_i32.const_int(value);
    }
    let a_mask_ty = a.vector_type(a_i32.as_type(), 4);
    let a_mask = a_mask_ty
        .const_vector([
            a_i32.const_int(0_i32),
            a_i32.const_int(1_i32),
            a_i32.const_int(2_i32),
            a_i32.const_int(3_i32),
        ])?
        .as_erased();

    // Module B: two well-formed `<4 x i32>` operands and its own mask, which is
    // the same shape as A's so nothing but the tag differs.
    let b_i32 = b.i32_type();
    let b_vec_ty = b.vector_type(b_i32.as_type(), 4);
    let b_lhs = b_vec_ty.as_type().poison().as_erased();
    let b_rhs = b_vec_ty.as_type().poison().as_erased();
    let b_mask = b_vec_ty
        .const_vector([
            b_i32.const_int(0_i32),
            b_i32.const_int(1_i32),
            b_i32.const_int(2_i32),
            b_i32.const_int(3_i32),
        ])?
        .as_erased();

    assert!(
        !ShuffleVectorInst::is_valid_operands_with_constant_mask(b_lhs, b_rhs, a_mask),
        "a mask minted in another module must not be a valid shufflevector operand"
    );
    // B's own mask is accepted at the same call, so the rejection is about the
    // tag and nothing else.
    assert!(ShuffleVectorInst::is_valid_operands_with_constant_mask(
        b_lhs, b_rhs, b_mask
    ));
    Ok(())
}

// Suppress unused-import warning if a marker drifts.
const _: fn() = || {
    let _ = std::any::TypeId::of::<IntValue<'static, i32, DynBrand>>();
};
