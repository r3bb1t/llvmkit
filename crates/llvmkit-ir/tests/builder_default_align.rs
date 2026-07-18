//! Port of `llvm/lib/IR/Instructions.cpp::computeAllocaDefaultAlign`
//! (`getPrefTypeAlign`) and `computeLoadStoreDefaultAlign`
//! (`getABITypeAlign`), plus AsmWriter's always-print-align behavior: an
//! align-less `alloca`/`load`/`store` materialises the DataLayout default and
//! prints `, align N` with the exact values the default DataLayout yields.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module, NoFolder, PointerValue};

/// `alloca` materialises `getPrefTypeAlign`. The default DataLayout gives
/// i32->4, i64->8, double->8, i1->1, and i128->8 (no i128 spec, so the walk
/// falls to the largest integer spec, i64:64).
#[test]
fn alloca_materialises_preferred_align() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let fn_ty = m.fn_type_no_params(m.void_type().as_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        b.build_alloca(m.i32_type(), "a32")?;
        b.build_alloca(m.i64_type(), "a64")?;
        b.build_alloca(m.f64_type(), "af64")?;
        b.build_alloca(m.bool_type(), "a1")?;
        b.build_alloca(m.int_type_n::<128>(), "a128")?;
        b.build_ret_void();

        let text = format!("{m}");
        assert!(text.contains("%a32 = alloca i32, align 4\n"), "{text}");
        assert!(text.contains("%a64 = alloca i64, align 8\n"), "{text}");
        assert!(text.contains("%af64 = alloca double, align 8\n"), "{text}");
        assert!(text.contains("%a1 = alloca i1, align 1\n"), "{text}");
        assert!(text.contains("%a128 = alloca i128, align 8\n"), "{text}");
        Ok(())
    })
}

/// `load`/`store` materialise `getABITypeAlign` (keyed off the loaded type
/// for load and the stored value's type for store). Note the default
/// DataLayout gives i64 an ABI align of 4 (`i64:32:64`) even though its
/// *preferred* align is 8 — so `load i64` is `align 4` while `alloca i64`
/// (preferred) is `align 8`. ptr->8, f32->4.
#[test]
fn load_store_materialise_abi_align() -> Result<(), IrError> {
    Module::with_new("ls", |m| {
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(m.void_type().as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let p: PointerValue = f.param(0)?.try_into()?;
        b.build_load(m.i64_type(), p, "l64")?;
        b.build_load(m.ptr_type(0), p, "lptr")?;
        // Store default keys off the *stored value's* type (f32 -> align 4),
        // not the pointer.
        b.build_store(m.f32_type().const_float(0.0), p)?;
        b.build_ret_void();

        let text = format!("{m}");
        assert!(
            text.contains("%l64 = load i64, ptr %0, align 4\n"),
            "{text}"
        );
        assert!(
            text.contains("%lptr = load ptr, ptr %0, align 8\n"),
            "{text}"
        );
        assert!(
            text.contains("store float 0.000000e+00, ptr %0, align 4\n"),
            "{text}"
        );
        Ok(())
    })
}

/// `IRBuilder::CreateAlloca` uses `DL.getAllocaAddrSpace()`: with a
/// DataLayout that sets alloca address space 5 (`A5`), the alloca result is
/// `ptr addrspace(5)` and its printed form carries `, addrspace(5)` after
/// the alignment.
#[test]
fn alloca_uses_datalayout_alloca_address_space() -> Result<(), IrError> {
    Module::with_new("as", |m| {
        m.set_data_layout("A5")?;
        let fn_ty = m.fn_type_no_params(m.void_type().as_type(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let p = b.build_alloca(m.i32_type(), "p")?;
        assert_eq!(p.ty().address_space(), 5);
        b.build_ret_void();

        let text = format!("{m}");
        assert!(
            text.contains("%p = alloca i32, align 4, addrspace(5)\n"),
            "{text}"
        );
        Ok(())
    })
}
