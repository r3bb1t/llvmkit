//! llvmkit-specific compile-fail (D7): `IrBuilder::position_at_end` is
//! unchecked only on a `UniqueBrand` module, whose registry keeps one live
//! module per brand, so the block it is handed is the builder's own.
//! `Module::dynamic`'s `DynBrand` names many live modules, and a block of one
//! of them handed to a builder over another would write that builder's
//! instructions into a block of the wrong arena. So `position_at_end` fails on
//! `UniqueBrand`'s own message here; the checked `try_position_at_end` is the
//! form for it (`cross_module_handles.rs::a_block_argument_edge_rejects_a_value_or_predecessor_from_another_module`
//! pins its refusal). No upstream counterpart: LLVM's `Module` has no brand,
//! and `IRBuilderBase::SetInsertPoint(BasicBlock *)` takes any block.

use llvmkit_ir::{Dyn, IrBuilder, IrError, Linkage, Module};

fn main() -> Result<(), IrError> {
    let mut module = Module::dynamic("m");
    let mut key = module.layout_token();
    let m = key.module();
    let f = m.add_function_dyn("f", m.function_type_no_parameters(m.void_type())?, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let _ = IrBuilder::new_for::<Dyn>(&mut key).position_at_end(entry);
    Ok(())
}
