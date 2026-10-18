//! llvmkit-specific compile-fail (D3, D4): a pointer comparison takes one
//! address space for both operands, so a pointer in address space 0 and one in
//! address space 1 cannot be compared — a type error rather than the
//! verifier's run-time refusal.

use llvmkit_ir::{IntPredicate, IrBuilder, IrError, Linkage, NoFolder, Ptr, module_new};

fn main() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let f = m.add_typed_function_of::<fn(Ptr, Ptr<1>), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (p, q) = m.view(f).params();
    let _ = b.pointer_cmp(IntPredicate::Eq, p, q, "eq");
    Ok(())
}
