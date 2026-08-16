//! Compile-fail lock for `field_gep::<S, I>` with an out-of-range
//! field index (Doctrine D4, D6). `CpuState` has 2 fields (indices 0/1);
//! index 7 has no `StructFieldAt<7>` impl, so the call fails to compile
//! instead of panicking or returning an `IrError` at runtime.

use llvmkit_ir::{IrError, IrStruct, Linkage, Module};

#[derive(IrStruct)]
struct CpuState {
    flags: i32,
    pc: i64,
}

fn main() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let f = m.add_typed_function::<i64, (), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = m.view(f).builder(&m).position_at_end(entry);
    let cpu = b.typed_alloca::<CpuState, _>("cpu")?;
    let _bad = b.field_gep::<CpuState, 7, _>(cpu, "x")?;
    Ok(())
}
