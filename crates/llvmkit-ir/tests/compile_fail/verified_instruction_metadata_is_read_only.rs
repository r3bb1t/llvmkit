//! llvmkit typestate compile-fail (D1, D8): an instruction view minted from a
//! verified module is read-only — `set_metadata` is not on it, whatever token is
//! offered.
//!
//! `set_metadata` takes a `&Module<B, Unverified>` token, and two modules that
//! share a brand (every `Module::dynamic` is `DynBrand`) type-check against
//! each other's — so before the capability parameter a second, unverified
//! module's token was enough to attach that module's node to a verified
//! module's instruction. Now the view's own capability decides:
//! `set_metadata` requires `C: CanMutate`, and a verified module's views are
//! `ReadOnly`.
//!
//! No upstream counterpart: `Instruction::setMetadata` is a plain non-const
//! method and LLVM has no verified-module typestate.
use llvmkit_ir::{
    InstructionView, IrBuilder, Linkage, MetadataAttachmentKind, MetadataId, Module, NoFolder,
};

fn main() {
    let a = Module::dynamic("a");
    let i32_ty = a.i32_type();
    let f = a
        .add_function_dyn("f", a.function_type_no_parameters(i32_ty), Linkage::External)
        .expect("f");
    let entry = a.view(f).append_basic_block(&a, "entry");
    let b0 = IrBuilder::with_folder(&a, NoFolder).position_at_end(entry);
    let sum = b0
        .int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "s")
        .expect("add");
    b0.ret(a.view(sum)).expect("ret");
    let a = a.verify().expect("verifies");
    let b = Module::dynamic("b");
    let node = b.metadata_tuple(Vec::<MetadataId<_>>::new()).expect("node");
    let instruction =
        InstructionView::try_from(a.view(sum).as_erased()).expect("the add is an instruction");
    instruction.set_metadata(&b, MetadataAttachmentKind::Dbg, node).expect("set");
}
