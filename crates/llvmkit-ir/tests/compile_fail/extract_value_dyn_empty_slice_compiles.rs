//! Companion `pass` fixture for `extract_value_empty_indices.rs` (Doctrine
//! D3). A `const { assert!(N > 0) }` failure is a monomorphisation-time
//! (codegen) diagnostic, invisible under `cargo check`; trybuild only runs
//! `cargo build` for the whole harness once at least one `t.pass(...)` case
//! is registered (`trybuild::cargo::build_dependencies`'s
//! `if project.has_pass { "build" } else { "check" }`). This fixture
//! flips that switch: `build_extract_value_dyn(agg, &[], name)` compiles
//! fine (the empty-slice rejection is a runtime `IrError`, not a
//! compile-time one), proving the harness now exercises full codegen so
//! the sibling `compile_fail` case is actually meaningful.
use llvmkit_ir::{IrError, Linkage, Module};

fn main() -> Result<(), IrError> {
    Module::with_new("m", |m| {
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let s_ty = m.struct_type([i8_ty.as_type(), i32_ty.as_type()], false);
        let fn_ty = m.fn_type(void_ty.as_type(), [s_ty.as_type()], false);
        let f = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = llvmkit_ir::IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let up = f.param(0)?;
        // Compiles fine: the empty-slice rejection is the runtime
        // `IrError::InvalidOperation` kept by `build_extract_value_dyn`,
        // not a compile-time error.
        let _ = b.build_extract_value_dyn(up, &[], "bad");
        Ok(())
    })
}
