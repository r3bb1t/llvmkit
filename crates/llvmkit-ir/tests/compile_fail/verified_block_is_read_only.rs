//! llvmkit typestate compile-fail (D1, D8): a verified module's blocks are
//! `ReadOnly`, so none of a block's mutators can be called on one, and the
//! function a block hands back is `ReadOnly` too.
//!
//! `entry_block` of a function viewed through a `Module<B, Verified>` keeps the
//! function's capability. Each call below — `set_name` / `clear_name`, the same
//! through `SetName`, `splice_into`, `split_at`, `split_before`, and a setter
//! on `parent_function()` — is refused by `CanMutate`'s own message. The token
//! is a second unverified module of the same brand, which type-checks, so
//! nothing but the block's capability refuses the calls.
//!
//! `BasicBlock::call` needs a block with typed parameters, and a verified
//! module hands those out only as labels (`Module::view` of a block id), whose
//! `call` `verified_block_label_call_is_read_only.rs` locks. So its bound is
//! locked through a function generic over the capability, which cannot call it
//! for every `C`.
//!
//! The program is never run: only the calls have to type-check for the law to
//! be tested.
//!
//! No upstream counterpart: `BasicBlock::setName`, `splice` and `splitBasicBlock`
//! are plain non-const methods and LLVM has no verified-module typestate.
use llvmkit_ir::{
    BasicBlock, Capability, ConstantIntValue, Dyn, DynBrand, IrBuilder, Linkage, Module, SetName,
    Terminated,
};

fn call_at_any_capability<'a, C: Capability>(
    block: BasicBlock<'a, Dyn, Terminated, DynBrand, (i32,), C>,
    seven: ConstantIntValue<'a, i32, DynBrand>,
) {
    let _ = block.call((seven,));
}

fn main() {
    let other = Module::dynamic("other");
    let g = other
        .add_function_dyn(
            "g",
            other.function_type_no_parameters(other.void_type()),
            Linkage::External,
        )
        .expect("g");
    let dest = other.view(g).append_basic_block(&other, "dest");

    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .ret(i32_ty.const_int(0_i32))
        .expect("ret");
    let m = m.verify().expect("verifies");
    let block = || m.view(f).entry_block().expect("a definition");
    let first = block().instructions().next().expect("the ret");

    block().set_name(&other, "renamed").expect("set");
    block().clear_name(&other);
    SetName::set_name(block(), &other, "renamed").expect("set");
    block().splice_into(&other, dest).expect("splice");
    let _ = block().split_at(&other, &first, "tail");
    let _ = block().split_before(&other, &first, "tail");
    block()
        .parent_function()
        .expect("attached")
        .set_linkage(&other, Linkage::Internal);
}
