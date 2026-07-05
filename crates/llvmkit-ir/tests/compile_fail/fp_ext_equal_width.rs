//! Compile-fail lock for `build_fp_ext::<Fp128, PpcFp128, _>` (Doctrine
//! D4, D11). `Fp128` and `PpcFp128` are both 128-bit non-IEEE layouts;
//! upstream `CastInst::castIsValid` (`lib/IR/Instructions.cpp`) legalizes
//! FPExt only on a STRICT `getPrimitiveSizeInBits` inequality, so an
//! equal-width pair has no valid direction. `PpcFp128` has no
//! `FloatWiderThan<Fp128>` impl, so the call fails to compile instead of
//! asserting at runtime (`FPExtInst::FPExtInst`'s `castIsValid` assert).

use llvmkit_ir::{IrError, Linkage, Module};

fn main() -> Result<(), IrError> {
    Module::with_new("m", |m| {
        let fp128_ty = m.fp128_type();
        let ppc_ty = m.ppc_fp128_type();
        let fn_ty = m.fn_type(ppc_ty, [fp128_ty.as_type()], false);
        let f = m.add_function::<llvmkit_ir::PpcFp128, _>("ext", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = llvmkit_ir::IRBuilder::new_for::<llvmkit_ir::PpcFp128>(&m).position_at_end(entry);
        let arg: llvmkit_ir::FloatValue<llvmkit_ir::Fp128> = f.param(0)?.try_into()?;
        let _bad = b.build_fp_ext(arg, ppc_ty, "y")?;
        Ok(())
    })
}
