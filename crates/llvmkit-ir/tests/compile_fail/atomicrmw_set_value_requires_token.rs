//! llvmkit typestate compile-fail (Doctrine D1 — no mutation without a token).
//!
//! `AtomicRMWInst::set_value_operand` mutates the IR in place, so — like
//! `Instruction::replace_all_uses_with` — it requires an `&Module<Unverified>`
//! capability witness. Calling it with only the replacement value (no token)
//! must fail to compile. If the token parameter were ever dropped, this
//! fixture would start compiling and trybuild would flag the regression.

use llvmkit_ir::{
    AtomicOrdering, AtomicRMWConfig, IRBuilder, IrResult, Linkage, Module, PointerValue, SyncScope,
};
use llvmkit_ir::atomicrmw_binop::AtomicRMWBinOp;

fn main() -> IrResult<()> {
    Module::with_new::<_, _, _>("armw-token", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
        let word: PointerValue = f.param(0)?.try_into()?;
        let twelve = i32_ty.const_int(12_i32);
        let armw = b.build_atomicrmw(
            AtomicRMWBinOp::Xchg,
            word,
            twelve,
            AtomicRMWConfig::new(AtomicOrdering::Monotonic, SyncScope::System),
            "armw",
        )?;

        let replacement = i32_ty.const_int(99_i32);
        // Missing the `&Module<Unverified>` capability token.
        armw.set_value_operand(replacement.into_erased())?;

        b.build_ret_void()?;
        Ok(())
    })
}
