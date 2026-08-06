//! Compile-fail lock for `fp_ext::<Fp128, PpcFp128, _>` (Doctrine
//! D4, D11). `Fp128` and `PpcFp128` are both 128-bit non-IEEE layouts;
//! upstream `CastInst::castIsValid` (`lib/IR/Instructions.cpp`) legalizes
//! FPExt only on a STRICT `getScalarSizeInBits` inequality (the FPExt arm
//! compares `SrcScalarBitSize < DstScalarBitSize`, not
//! `getPrimitiveSizeInBits` -- numerically identical for these scalar
//! kinds), so an equal-width pair has no valid direction. `PpcFp128` has
//! no `FloatWiderThan<Fp128>` impl, so the call fails to compile instead
//! of asserting at runtime (`FPExtInst::FPExtInst`'s `castIsValid` assert).

use llvmkit_ir::{IrError, Linkage, Module};

fn main() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let fp128_ty = m.fp128_type();
    let ppc_ty = m.ppc_fp128_type();
    let fn_ty = m.function_type(ppc_ty, [fp128_ty.as_type()]);
    let f = m.add_function_dyn("ext", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = llvmkit_ir::IrBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
    let arg: llvmkit_ir::FloatValue<llvmkit_ir::Fp128, _> = m.view(f).param(0)?.try_into()?;
    let _bad = b.fp_ext(arg, ppc_ty, "y")?;
    Ok(())
}
