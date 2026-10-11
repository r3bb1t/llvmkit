//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` block label cannot
//! author a branch edge.
//!
//! `BasicBlockLabel::call` admits its arguments into the label's module, which
//! authors an edge rather than reading one, so it carries
//! `where C: CanMutate`. A `Module<B, Verified>`'s `view` of a block id is a
//! `ReadOnly` label, and the call is refused by `CanMutate`'s own message.
//! The argument comes from a second unverified module of the same brand, so
//! nothing but the label's capability refuses the call.
//!
//! The method's body lowers through `proven_mutable()`, which the bound
//! licenses, so dropping the bound alone breaks the crate's build. This
//! fixture is what fails if the bound goes and the body is routed around it.
//!
//! The program is never run: only the call has to type-check for the law to
//! be tested.
//!
//! No upstream counterpart: LLVM has no block arguments and no verified-module
//! typestate.
use llvmkit_ir::{Dyn, IrBuilder, Linkage, Module, Type};

fn main() {
    // A second unverified module of the same brand: its constant is `Mutable`
    // and type-checks as the call's argument.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m
        .add_function_dyn(
            "f",
            m.function_type(i32_ty, Vec::<Type<_>>::new()),
            Linkage::External,
        )
        .expect("f");
    let head = {
        let b = IrBuilder::new_for::<Dyn>(&m);
        let (head, _params) = b
            .append_block_typed::<(i32,), _, _>(m.view(f), "head")
            .expect("head");
        head.id()
    };
    let m = m.verify().expect("verifies");
    let seven = other.i32_type().const_int(7_i32);
    let _ = m.view(head).call((seven,));
}
