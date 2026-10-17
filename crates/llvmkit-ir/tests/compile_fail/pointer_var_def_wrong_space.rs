//! llvmkit-specific compile-fail (D3): a pointer variable takes values in its
//! own address space, so a `Space<1>` pointer cannot define a variable
//! declared in address space 0 — a type error rather than
//! `def_pointer_var`'s run-time `AddressSpaceMismatch`.

use llvmkit_ir::{IrError, Linkage, Ptr, SsaBuilder, SsaState, module_new};

fn main() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let typed = m.add_typed_function::<(), (Ptr<1>,), _>("f", Linkage::External)?;
    let f = typed.as_function();
    let mut state = SsaState::for_function(&m, m.view(f))?;
    let mut b = SsaBuilder::for_function(&m, m.view(f), &mut state)?;
    let entry = b.create_block("entry");
    let zero = b.declare_pointer_var("zero");
    b.switch_to_block(entry)?;
    let (p,) = m.view(typed).params();
    b.def_pointer_var(zero, p)?;
    Ok(())
}
