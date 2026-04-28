//! Atomic load / store + bitcast coverage. Each `#[test]` cites the
//! upstream `.ll` fixture or `Verifier::visit*` arm it ports (D11).
//!
//! Tests that mirror `test/Bitcode/compatibility.ll` lock the AsmWriter
//! print form byte-for-byte against the upstream `; CHECK:` line. Tests
//! that mirror `lib/IR/Verifier.cpp::visit{Load,Store}Inst` exercise the
//! atomic-rule rejection paths.

use llvmkit_ir::{
    Align, AtomicLoadConfig, AtomicOrdering, AtomicStoreConfig, IRBuilder, IntValue, IrError,
    Linkage, Module, SyncScope, VerifierRule,
};

// --- Atomic load shapes (compatibility.ll lines 902-906) ---------------

/// Mirrors `test/Bitcode/compatibility.ll` line 902:
/// `%ld.1 = load atomic i32, ptr %word monotonic, align 4`.
#[test]
fn load_atomic_monotonic_align4() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let cfg = AtomicLoadConfig::new(
        AtomicOrdering::Monotonic,
        SyncScope::System,
        Align::new(4).expect("align 4"),
    );
    let ld = b.build_int_load_atomic::<i32, _>(word, cfg, "ld.1")?;
    b.build_ret(ld)?;
    let text = format!("{m}");
    assert!(
        text.contains("%ld.1 = load atomic i32, ptr %0 monotonic, align 4\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Bitcode/compatibility.ll` line 904:
/// `%ld.2 = load atomic volatile i32, ptr %word acquire, align 8`.
#[test]
fn load_atomic_volatile_acquire_align8() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let cfg = AtomicLoadConfig::new(
        AtomicOrdering::Acquire,
        SyncScope::System,
        Align::new(8).expect("align 8"),
    )
    .volatile();
    let ld = b.build_int_load_atomic::<i32, _>(word, cfg, "ld.2")?;
    b.build_ret(ld)?;
    let text = format!("{m}");
    assert!(
        text.contains("%ld.2 = load atomic volatile i32, ptr %0 acquire, align 8\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Bitcode/compatibility.ll` line 906:
/// `%ld.3 = load atomic volatile i32, ptr %word syncscope("singlethread") seq_cst, align 16`.
#[test]
fn load_atomic_volatile_singlethread_seq_cst_align16() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let cfg = AtomicLoadConfig::new(
        AtomicOrdering::SequentiallyConsistent,
        SyncScope::SingleThread,
        Align::new(16).expect("align 16"),
    )
    .volatile();
    let ld = b.build_int_load_atomic::<i32, _>(word, cfg, "ld.3")?;
    b.build_ret(ld)?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "%ld.3 = load atomic volatile i32, ptr %0 syncscope(\"singlethread\") seq_cst, align 16\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

// --- Atomic store shapes (compatibility.ll lines 909-913) --------------

/// Mirrors `test/Bitcode/compatibility.ll` line 909:
/// `store atomic i32 23, ptr %word monotonic, align 4`.
#[test]
fn store_atomic_monotonic_align4() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(m.void_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let val = i32_ty.const_int(23_i32);
    let cfg = AtomicStoreConfig::new(
        AtomicOrdering::Monotonic,
        SyncScope::System,
        Align::new(4).expect("align 4"),
    );
    b.build_store_atomic(val, word, cfg)?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("store atomic i32 23, ptr %0 monotonic, align 4\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Bitcode/compatibility.ll` line 911:
/// `store atomic volatile i32 24, ptr %word monotonic, align 4`.
#[test]
fn store_atomic_volatile_monotonic_align4() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(m.void_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let val = i32_ty.const_int(24_i32);
    let cfg = AtomicStoreConfig::new(
        AtomicOrdering::Monotonic,
        SyncScope::System,
        Align::new(4).expect("align 4"),
    )
    .volatile();
    b.build_store_atomic(val, word, cfg)?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("store atomic volatile i32 24, ptr %0 monotonic, align 4\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Bitcode/compatibility.ll` line 913:
/// `store atomic volatile i32 25, ptr %word syncscope("singlethread") monotonic, align 4`.
#[test]
fn store_atomic_volatile_singlethread_monotonic() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(m.void_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let val = i32_ty.const_int(25_i32);
    let cfg = AtomicStoreConfig::new(
        AtomicOrdering::Monotonic,
        SyncScope::SingleThread,
        Align::new(4).expect("align 4"),
    )
    .volatile();
    b.build_store_atomic(val, word, cfg)?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains(
            "store atomic volatile i32 25, ptr %0 syncscope(\"singlethread\") monotonic, align 4\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

// --- Verifier-rule rejections (Verifier.cpp visit{Load,Store}Inst) -----

/// Mirrors `Verifier::visitLoadInst` ("Load cannot have Release ordering").
/// Negative test: builder allows construction; verifier rejects.
#[test]
fn verifier_rejects_atomic_load_release_ordering() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let cfg = AtomicLoadConfig::new(
        AtomicOrdering::Release,
        SyncScope::System,
        Align::new(4).expect("align 4"),
    );
    let ld = b.build_int_load_atomic::<i32, _>(word, cfg, "ld")?;
    b.build_ret(ld)?;
    let err = m
        .verify_borrowed()
        .expect_err("verifier must reject release-ordered atomic load");
    let IrError::VerifierFailure { rule, .. } = err else {
        panic!("expected IrError::VerifierFailure, got {err:?}");
    };
    assert_eq!(rule, VerifierRule::AtomicLoadInvalidOrdering);
    Ok(())
}

/// Mirrors `Verifier::visitStoreInst` ("Store cannot have Acquire ordering").
#[test]
fn verifier_rejects_atomic_store_acquire_ordering() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(m.void_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let val = i32_ty.const_int(42_i32);
    let cfg = AtomicStoreConfig::new(
        AtomicOrdering::Acquire,
        SyncScope::System,
        Align::new(4).expect("align 4"),
    );
    b.build_store_atomic(val, word, cfg)?;
    b.build_ret_void();
    let err = m
        .verify_borrowed()
        .expect_err("verifier must reject acquire-ordered atomic store");
    let IrError::VerifierFailure { rule, .. } = err else {
        panic!("expected IrError::VerifierFailure, got {err:?}");
    };
    assert_eq!(rule, VerifierRule::AtomicStoreInvalidOrdering);
    Ok(())
}

/// Mirrors `Verifier::checkAtomicMemAccessSize` ("atomic memory access'
/// operand must have a power-of-two size"): atomic load of `i17` (a width
/// neither byte-sized nor a power of two) must be rejected.
#[test]
fn verifier_rejects_atomic_load_non_power_of_two_size() -> Result<(), IrError> {
    let m = Module::new("a");
    // i17 is intentionally non-power-of-two: the marker `Width<17>` projects
    // through `StaticIntWidth::ir_type`, no separate type binding is needed.
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(m.void_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    // i17 → bit width 17, not power-of-two.
    let cfg = AtomicLoadConfig::new(
        AtomicOrdering::Monotonic,
        SyncScope::System,
        Align::new(4).expect("align 4"),
    );
    let _ = b.build_int_load_atomic::<llvmkit_ir::Width<17>, _>(word, cfg, "ld")?;
    b.build_ret_void();
    let err = m
        .verify_borrowed()
        .expect_err("verifier must reject non-power-of-two atomic size");
    let IrError::VerifierFailure { rule, .. } = err else {
        panic!("expected IrError::VerifierFailure, got {err:?}");
    };
    assert_eq!(rule, VerifierRule::AtomicLoadStoreInvalidSize);
    Ok(())
}

// --- bitcast methods ---------------------------------------------------

/// Mirrors `unittests/IR/PatternMatch.cpp::TEST_F(PatternMatchTest, BitCast)`
/// (line 638-678) which exercises `IRB.CreateBitCast(double, i64)` and
/// related shapes. We test the typed-marker variant of the same construct.
#[test]
fn bitcast_int_to_fp_emits_text() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<f32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let bc = b.build_bitcast_int_to_fp::<i32, f32, _>(n, f32_ty, "bc")?;
    b.build_ret(bc)?;
    let text = format!("{m}");
    assert!(
        text.contains("%bc = bitcast i32 %0 to float\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/IR/PatternMatch.cpp::TEST_F(PatternMatchTest, BitCast)`
/// (line 638). The `fp -> int` direction uses `IRB.CreateBitCast(OneDouble,
/// IRB.getInt64Ty())` upstream (line 643).
#[test]
fn bitcast_fp_to_int_emits_text() -> Result<(), IrError> {
    let m = Module::new("a");
    let i64_ty = m.i64_type();
    let f64_ty = m.f64_type();
    let fn_ty = m.fn_type(i64_ty, [f64_ty.as_type()], false);
    let f = m.add_function::<i64>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let n: llvmkit_ir::FloatValue<f64> = f.param(0)?.try_into()?;
    let bc = b.build_bitcast_fp_to_int::<f64, i64, _>(n, i64_ty, "bc")?;
    b.build_ret(bc)?;
    let text = format!("{m}");
    assert!(
        text.contains("%bc = bitcast double %0 to i64\n"),
        "got:\n{text}"
    );
    Ok(())
}
