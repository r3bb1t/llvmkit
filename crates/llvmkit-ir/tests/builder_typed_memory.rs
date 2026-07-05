//! TypedPointerValue: Rust-side static pointee overlay on opaque `ptr`.
//! Example-locks (opaque pointers have no upstream typed analog); the IR
//! shape is anchored on the existing alloca/load/store fixtures and
//! `test/Assembler/getelementptr_struct.ll` for the field-GEP form.

use llvmkit_ir::{IrResult, IrStruct, Linkage, Module};

#[derive(IrStruct)]
struct CpuState {
    flags: i32,
    pc: i64,
}

/// Example-lock: no upstream typed-pointer-overlay test exists (opaque
/// pointers carry no compile-time pointee in C++ IRBuilder either).
/// Asserts the D3 requirement directly -- the typed `alloca`/`store`/
/// `load` overlay must print byte-identical IR to the erased path
/// (`build_alloca` + `build_store` + `build_int_load`), anchored on the
/// same alloca/load/store forms as `tests/medium_builder_int.rs`.
#[test]
fn typed_alloca_load_store_round_trip_prints_identically_to_erased() -> IrResult<()> {
    let typed = Module::with_new("m", |m| {
        let f = m.add_typed_function::<i32, (i32,), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let (x,) = f.params();
        let slot = b.build_typed_alloca::<i32, _>("slot")?;
        b.build_typed_store(x, slot)?;
        let v = b.build_typed_load(slot, "v")?; // IntValue<'_, i32, _> -- no try_into
        b.build_ret(v)?;
        Ok(format!("{m}"))
    })?;
    let erased = Module::with_new("m", |m| {
        let f = m.add_typed_function::<i32, (i32,), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let (x,) = f.params();
        let slot = b.build_alloca(m.i32_type(), "slot")?;
        b.build_store(x, slot)?;
        let v = b.build_int_load::<i32, _, _>(slot, "v")?;
        b.build_ret(v)?;
        Ok(format!("{m}"))
    })?;
    assert_eq!(typed, erased, "typed overlay must not change printed IR");
    Ok(())
}

/// Example-lock: `build_field_gep::<S, I>` is llvmkit-specific compile-time
/// field projection over opaque pointers (LLVM's own `IRBuilder` narrows
/// `CreateStructGEP`'s result type only at runtime). Print form is anchored
/// on `test/Assembler/getelementptr_struct.ll` via the existing
/// `tests/builder_gep.rs::struct_gep` fixture's exact `getelementptr
/// inbounds nuw %S, ptr %x, i32 0, i32 N` form (the `nuw` flag matches
/// `IRBuilder::CreateStructGEP` in `IRBuilder.h`, which passes
/// `GEPNoWrapFlags::inBounds() | GEPNoWrapFlags::noUnsignedWrap()`).
#[test]
fn field_gep_projects_field_type_at_compile_time() -> IrResult<()> {
    // `#[derive(IrStruct)]` is dual-purpose (Rust data + IR schema, see
    // `tests/derived_struct_schema.rs`); read both fields once so the
    // Rust-side struct is not flagged dead code by the derive's own
    // schema-only usage below.
    let cpu_state = CpuState { flags: 0, pc: 0 };
    assert_eq!(cpu_state.flags, 0);
    assert_eq!(cpu_state.pc, 0);

    Module::with_new("m", |m| {
        let f = m.add_typed_function::<i64, (), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let cpu = b.build_typed_alloca::<CpuState, _>("cpu")?;
        let pc_ptr = b.build_field_gep::<CpuState, 1, _>(cpu, "pc.ptr")?; // TypedPointerValue<i64>
        let pc = b.build_typed_load(pc_ptr, "pc")?; // IntValue<'_, i64, _>
        b.build_ret(pc)?;
        let printed = format!("{m}");
        assert!(
            printed.contains("getelementptr inbounds nuw %CpuState, ptr %cpu, i32 0, i32 1"),
            "got:\n{printed}"
        );
        Ok(())
    })
}
