//! Atomic op coverage: `fence`, `cmpxchg`, `atomicrmw`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    AtomicOrdering, AtomicRMWBinOp, AtomicRMWFlags, CmpXchgFlags, IRBuilder, IrError, Linkage,
    MaybeAlign, Module, PointerValue, SyncScope,
};

// --------------------------------------------------------------------------
// fence
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` lines 893-898:
/// `fence acquire`, `fence release`, `fence acq_rel`. Locks the
/// canonical print form for the four valid system-scope orderings.
#[test]
fn fence_system_scope_orderings() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("instructions.atomics", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let _ = b.build_fence(AtomicOrdering::Acquire, SyncScope::System, "")?;
    let _ = b.build_fence(AtomicOrdering::Release, SyncScope::System, "")?;
    let _ = b.build_fence(AtomicOrdering::AcquireRelease, SyncScope::System, "")?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(text.contains("fence acquire\n"), "got:\n{text}");
    assert!(text.contains("fence release\n"), "got:\n{text}");
    assert!(text.contains("fence acq_rel\n"), "got:\n{text}");
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 899:
/// `fence syncscope("singlethread") seq_cst`. Locks the singlethread
/// scope qualifier print form.
#[test]
fn fence_singlethread_seq_cst() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let _ = b.build_fence(
        AtomicOrdering::SequentiallyConsistent,
        SyncScope::SingleThread,
        "",
    )?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("fence syncscope(\"singlethread\") seq_cst\n"),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// cmpxchg
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 810:
/// `cmpxchg ptr %word, i32 0, i32 4 monotonic monotonic`.
#[test]
fn cmpxchg_no_align_monotonic_monotonic() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: PointerValue = f.param(0)?.try_into()?;
    let zero = i32_ty.const_int(0_i32);
    let four = i32_ty.const_int(4_i32);
    let _ = b.build_atomic_cmpxchg(
        word,
        zero,
        four,
        llvmkit_ir::AtomicCmpXchgConfig {
            success_ordering: AtomicOrdering::Monotonic,
            failure_ordering: AtomicOrdering::Monotonic,
            sync_scope: SyncScope::System,
            flags: CmpXchgFlags::new(),
            align: MaybeAlign::NONE,
        },
        "cmpxchg_no_align.0",
    )?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("%cmpxchg_no_align.0 = cmpxchg ptr %0, i32 0, i32 4 monotonic monotonic\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 824:
/// `cmpxchg weak volatile ptr %word, i32 0, i32 11 syncscope("singlethread") seq_cst monotonic`.
/// Locks the full flag + scope print form.
#[test]
fn cmpxchg_weak_volatile_singlethread() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: PointerValue = f.param(0)?.try_into()?;
    let zero = i32_ty.const_int(0_i32);
    let eleven = i32_ty.const_int(11_i32);
    let _ = b.build_atomic_cmpxchg(
        word,
        zero,
        eleven,
        llvmkit_ir::AtomicCmpXchgConfig {
            success_ordering: AtomicOrdering::SequentiallyConsistent,
            failure_ordering: AtomicOrdering::Monotonic,
            sync_scope: SyncScope::SingleThread,
            flags: CmpXchgFlags::new().weak().volatile(),
            align: MaybeAlign::NONE,
        },
        "cx",
    )?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains(
            "%cx = cmpxchg weak volatile ptr %0, i32 0, i32 11 syncscope(\"singlethread\") seq_cst monotonic\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// atomicrmw
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 846:
/// `atomicrmw xchg ptr %word, i32 12 monotonic`.
#[test]
fn atomicrmw_xchg_monotonic() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: PointerValue = f.param(0)?.try_into()?;
    let twelve = i32_ty.const_int(12_i32);
    let _ = b.build_atomicrmw(
        AtomicRMWBinOp::Xchg,
        word,
        twelve,
        llvmkit_ir::AtomicRMWConfig {
            ordering: AtomicOrdering::Monotonic,
            sync_scope: SyncScope::System,
            flags: AtomicRMWFlags::new(),
            align: MaybeAlign::NONE,
        },
        "atomicrmw_no_align.xchg",
    )?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("%atomicrmw_no_align.xchg = atomicrmw xchg ptr %0, i32 12 monotonic\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 862:
/// `atomicrmw volatile min ptr %word, i32 20 monotonic`.
#[test]
fn atomicrmw_volatile_min_monotonic() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: PointerValue = f.param(0)?.try_into()?;
    let twenty = i32_ty.const_int(20_i32);
    let _ = b.build_atomicrmw(
        AtomicRMWBinOp::Min,
        word,
        twenty,
        llvmkit_ir::AtomicRMWConfig {
            ordering: AtomicOrdering::Monotonic,
            sync_scope: SyncScope::System,
            flags: AtomicRMWFlags::new().volatile(),
            align: MaybeAlign::NONE,
        },
        "amin",
    )?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("%amin = atomicrmw volatile min ptr %0, i32 20 monotonic\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 864:
/// `atomicrmw umax ptr %word, i32 21 syncscope("singlethread") monotonic`.
#[test]
fn atomicrmw_umax_singlethread() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let word: PointerValue = f.param(0)?.try_into()?;
    let twenty_one = i32_ty.const_int(21_i32);
    let _ = b.build_atomicrmw(
        AtomicRMWBinOp::UMax,
        word,
        twenty_one,
        llvmkit_ir::AtomicRMWConfig {
            ordering: AtomicOrdering::Monotonic,
            sync_scope: SyncScope::SingleThread,
            flags: AtomicRMWFlags::new(),
            align: MaybeAlign::NONE,
        },
        "u",
    )?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("%u = atomicrmw umax ptr %0, i32 21 syncscope(\"singlethread\") monotonic\n"),
        "got:\n{text}"
    );
    Ok(())
}
