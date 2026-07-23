//! Compile-fail lock for `build_extract_value(agg, [], "x")` (Doctrine D3).
//! `ExtractValueInst::init` (`lib/IR/Instructions.cpp`) asserts a non-empty
//! index list at runtime; the typed `build_extract_value<V, const N: usize,
//! Name>` pulls that assertion forward to monomorphisation via
//! `const { assert!(N > 0) }`, so `N == 0` fails to compile instead of
//! producing the `IrError::InvalidOperation` returned by
//! `build_extract_value_dyn` (which keeps the runtime check; see
//! `test/Assembler/extractvalue-no-idx.ll`).

use llvmkit_ir::{IrError, Linkage, Module};

fn main() -> Result<(), IrError> {
    Module::with_new("m", |m| {
        let i8_ty = m.i8_type();
        let void_ty = m.void_type();
        let s_ty = m.struct_type([i8_ty.as_type(), m.i32_type().as_type()], false);
        let fn_ty = m.fn_type(void_ty.as_type(), [s_ty.as_type()], false);
        m.add_function_dyn("g", fn_ty, Linkage::External)?;
        let f = m.function_by_name::<()>("g")?.expect("declared above");
        let entry = f.append_basic_block(&m, "entry");
        let b = llvmkit_ir::IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let up = f.param(0)?;
        let _bad = b.build_extract_value(up, [], "x")?;
        Ok(())
    })
}
