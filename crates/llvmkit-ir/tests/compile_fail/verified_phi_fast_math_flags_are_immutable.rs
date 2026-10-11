//! llvmkit typestate compile-fail (D1, D8): setting a phi's fast-math flags is
//! a mutation, and a phi a `Module<B, Verified>` mints is `ReadOnly`.
//!
//! `set_fast_math_flags` takes no `&Module<B, Unverified>` token at all, so
//! before blocks and instructions carried their capability nothing stopped it
//! on a verified module's phi: the flags the printer emits could be rewritten
//! after `verify()`. The handle's own capability is now the only guard — and
//! the one this fixture holds in place.
//!
//! No upstream counterpart: `Instruction::setFastMathFlags` is a plain
//! non-const method and LLVM has no verified-module typestate.
use llvmkit_ir::{
    Dyn, FastMathFlags, InstructionKind, InstructionView, IrBuilder, Linkage, Module, PhiKind,
};

fn main() {
    let m = Module::dynamic("m");
    let f32_ty = m.f32_type();
    let f = m
        .add_function_dyn("f", m.function_type_no_parameters(f32_ty), Linkage::External)
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let (join, params) = IrBuilder::new_for::<Dyn>(&m)
        .append_block_with_params(m.view(f), &[f32_ty.as_type()], "join")
        .expect("join");
    let join_label = join.id();
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .br_with_args(join_label, &[f32_ty.const_float(1.0_f32).as_erased()])
        .expect("br");
    let parameter = params[0].id();
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(join)
        .ret(m.view(parameter))
        .expect("ret");
    let Some(InstructionKind::Phi(PhiKind::Fp(phi))) = InstructionView::try_from(m.view(parameter))
        .expect("the parameter is an instruction")
        .kind()
    else {
        panic!("the parameter is a float phi");
    };
    let phi = phi.id();
    let m = m.verify().expect("verifies");
    m.view(phi)
        .set_fast_math_flags(FastMathFlags::fast())
        .expect("set");
}
