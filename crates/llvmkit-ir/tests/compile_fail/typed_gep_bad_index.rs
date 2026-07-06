//! Compile-fail lock for `build_field_gep::<S, I>` with an out-of-range
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
    Module::with_new("m", |m| {
        let f = m.add_typed_function::<i64, (), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let cpu = b.build_typed_alloca::<CpuState, _>("cpu")?;
        let _bad = b.build_field_gep::<CpuState, 7, _>(cpu, "x")?;
        Ok(())
    })
}
