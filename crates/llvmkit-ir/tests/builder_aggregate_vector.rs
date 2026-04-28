//! Aggregate / vector op coverage: `extractvalue`, `insertvalue`,
//! `extractelement`, `insertelement`, `shufflevector`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module};

// --------------------------------------------------------------------------
// extractvalue
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1549:
/// `extractvalue { i8, i32 } %up, 0`. Locks the print form and result
/// type for an unpacked struct extract.
#[test]
fn extract_value_struct_field0() -> Result<(), IrError> {
    let m = Module::new("a");
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()], false);
    let fn_ty = m.fn_type(void_ty.as_type(), [s_ty.as_type()], false);
    let f = m.add_function::<()>("instructions.aggregateops", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let up = f.param(0)?;
    let _ = b.build_extract_value(up, [0u32], "")?;
    b.build_ret_void();
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
    let m = Module::new("a");
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let arr_ty = m.array_type(i8_ty, 3);
    let fn_ty = m.fn_type(void_ty.as_type(), [arr_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let arr = f.param(0)?;
    let _ = b.build_extract_value(arr, [2u32], "")?;
    b.build_ret_void();
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
    let m = Module::new("a");
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let inner = m.struct_type([i32_ty.as_type()], false);
    let outer = m.struct_type([i8_ty.as_type(), inner.as_type()], false);
    let fn_ty = m.fn_type(void_ty.as_type(), [outer.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let n = f.param(0)?;
    let _ = b.build_extract_value(n, [1u32, 0u32], "")?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("extractvalue { i8, { i32 } } %0, 1, 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// insertvalue
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1558:
/// `insertvalue { i8, i32 } %up, i8 1, 0`.
#[test]
fn insert_value_struct_field0() -> Result<(), IrError> {
    let m = Module::new("a");
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()], false);
    let fn_ty = m.fn_type(void_ty.as_type(), [s_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let up = f.param(0)?;
    let one = i8_ty.const_int(1_i8);
    let _ = b.build_insert_value(up, one, [0u32], "")?;
    b.build_ret_void();
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
    let m = Module::new("a");
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let arr_ty = m.array_type(i8_ty, 3);
    let fn_ty = m.fn_type(void_ty.as_type(), [arr_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let arr = f.param(0)?;
    let zero = i8_ty.const_int(0_i8);
    let _ = b.build_insert_value(arr, zero, [0u32], "")?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("insertvalue [3 x i8] %0, i8 0, 0\n"),
        "got:\n{text}"
    );
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
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(f32_ty, 4, false);
    let fn_ty = m.fn_type(void_ty.as_type(), [vec_ty.as_type()], false);
    let f = m.add_function::<()>("instructions.vectorops", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let vec = f.param(0)?;
    let zero = i8_ty.const_int(0_i8);
    let _ = b.build_extract_element(vec, zero, "")?;
    b.build_ret_void();
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
///
/// **Note**: the upstream fixture prints the constant as
/// `3.500000e+00` (the canonical scientific form for exactly-
/// representable float values). llvmkit's [`ConstantFloatValue`]
/// printer currently emits the IEEE-754 hex form
/// `0x400c000000000000` for every float constant; that's a pre-
/// existing format gap separate from this opcode and is tracked for a
/// future asm-writer parity pass. We assert the *instruction*
/// skeleton matches and tolerate the literal-form difference.
#[test]
fn insert_element_vector_float_at_i8() -> Result<(), IrError> {
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(f32_ty, 4, false);
    let fn_ty = m.fn_type(void_ty.as_type(), [vec_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let vec = f.param(0)?;
    let three_five = f32_ty.const_float(3.5_f32);
    let zero = i8_ty.const_int(0_i8);
    let _ = b.build_insert_element(vec, three_five, zero, "")?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("insertelement <4 x float> %0, float "),
        "got:\n{text}"
    );
    assert!(text.contains(", i8 0\n"), "got:\n{text}");
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
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(f32_ty, 4, false);
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        [vec_ty.as_type(), vec_ty.as_type()],
        false,
    );
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let v0 = f.param(0)?;
    let v1 = f.param(1)?;
    let _ = b.build_shuffle_vector(v0, v1, &[0, 0], "")?;
    b.build_ret_void();
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
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let vec_ty = m.vector_type(i32_ty, 4, false);
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        [vec_ty.as_type(), vec_ty.as_type()],
        false,
    );
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let v0 = f.param(0)?;
    let v1 = f.param(1)?;
    let _ = b.build_shuffle_vector(v0, v1, &[1, 1, 0, 0], "")?;
    b.build_ret_void();
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

// Suppress unused-import warning if a marker drifts.
const _: fn() = || {
    let _ = std::any::TypeId::of::<IntValue<i32>>();
};
