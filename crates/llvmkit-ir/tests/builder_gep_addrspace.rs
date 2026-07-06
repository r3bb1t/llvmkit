//! GEP result-type address-space preservation.
//!
//! Ports the constructive subset of
//! `test/Assembler/2007-12-11-AddressSpaces.ll` (GEP through
//! `ptr addrspace(N)` operands): the GEP result pointer must live in
//! the SAME address space as its base pointer, mirroring
//! `GetElementPtrInst::getGEPReturnType` (`IR/Instructions.h`).

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

/// Mirrors `test/Assembler/2007-12-11-AddressSpaces.ll` lines 20-26
/// (`define ptr addrspace(11) @bar(ptr addrspace(33) %x)`): a GEP
/// through a `ptr addrspace(33)` base must itself print as
/// `ptr addrspace(33)` in the GEP operand position, and the result
/// value must be usable as a `ptr addrspace(33)` operand downstream.
#[test]
fn gep_result_preserves_base_pointer_address_space() -> Result<(), IrError> {
    Module::with_new("gep_addrspace", |m| {
        let i32_ty = m.i32_type();
        let ptr_as33 = m.ptr_type(33);
        let fn_ty = m.fn_type(m.void_type().as_type(), [ptr_as33.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
        let one = i32_ty.const_int(1_i32).as_dyn();
        let gep = b.build_inbounds_gep(i32_ty, p, [one], "q")?;
        let (_entry, _ret) = b.build_ret_void();

        let printed = format!("{m}");
        assert!(
            printed.contains("getelementptr inbounds i32, ptr addrspace(33) %0"),
            "GEP must keep the base address space; got:\n{printed}"
        );

        // The result type itself must be `ptr addrspace(33)`, not plain `ptr`.
        assert_eq!(
            gep.ty().as_type().to_string(),
            "ptr addrspace(33)",
            "GEP result type must preserve the base pointer's address space"
        );
        Ok(())
    })
}
