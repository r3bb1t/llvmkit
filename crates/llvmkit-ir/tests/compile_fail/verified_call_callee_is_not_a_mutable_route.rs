//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` route never reaches a
//! mutator — not even by hopping through a function.
//!
//! A call viewed through a `Module<B, Verified>` is `ReadOnly`. Asking it for
//! its direct callee, then that callee's entry block and an instruction in it,
//! must not hand back a `Mutable` handle: if it did, the tokenless
//! `set_fast_math_flags` below would rewrite a verified module's IR, exactly
//! what `verified_phi_fast_math_flags_are_immutable.rs` refuses on the direct
//! route.
//!
//! How it fails depends on where the capability plan stands, and either way
//! is the law holding:
//!
//! - **Before functions carry the capability** (`FunctionValue` has no `C`
//!   yet), a function can only be handed out at `Mutable`, so
//!   `CallInst::classify_callee` exists only on a `Mutable` call: on this
//!   `ReadOnly` call it is absent, and the program stops at that call.
//! - **Once functions carry it**, `classify_callee` keeps the call's
//!   capability, the callee, its blocks and their instructions are all
//!   `ReadOnly`, and the program stops at `set_fast_math_flags`'s
//!   `C: CanMutate` bound instead.
//!
//! What must never happen is that it compiles.
//!
//! The program is never run: the callee holds no phi, and only the route has
//! to type-check for the law to be tested.
//!
//! No upstream counterpart: `CallBase::getCalledFunction` returns a plain
//! `Function *` and LLVM has no verified-module typestate.
use llvmkit_ir::{
    Callee, Dyn, FastMathFlags, InstructionKind, IrBuilder, Linkage, Module, PhiKind, Value,
};

fn main() {
    let m = Module::dynamic("m");
    let f32_ty = m.f32_type();
    let g = m
        .add_function_dyn("g", m.function_type_no_parameters(f32_ty), Linkage::External)
        .expect("g");
    let g_entry = m.view(g).append_basic_block(&m, "entry");
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(g_entry)
        .ret(f32_ty.const_float(1.0_f32))
        .expect("ret");
    let f = m
        .add_function_dyn("f", m.function_type_no_parameters(f32_ty), Linkage::External)
        .expect("f");
    let f_entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(f_entry);
    let call = b
        .call_dyn(g, Vec::<Value<'_, _>>::new(), "r")
        .expect("call");
    let result = b.view(call).return_value().expect("g returns a value");
    b.ret(result).expect("ret");
    let m = m.verify().expect("verifies");
    if let Callee::Direct(callee) = m.view(call).classify_callee() {
        for instruction in callee.entry_block().expect("a definition").instructions() {
            if let Some(InstructionKind::Phi(PhiKind::Fp(phi))) = instruction.kind() {
                phi.set_fast_math_flags(FastMathFlags::fast())
                    .expect("set");
            }
        }
    }
}
