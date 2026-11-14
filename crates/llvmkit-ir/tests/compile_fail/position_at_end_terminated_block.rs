//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Closest upstream behaviour: `Verifier::visitBasicBlock` in
//! `lib/IR/Verifier.cpp` rejects a block that has a non-terminator
//! after its terminator at runtime. llvmkit moves the rule to the
//! type system: once a builder consumes its insertion block via a
//! terminator-emitting build, the block is `Terminated` and cannot be
//! re-positioned. Both positioning entries are pinned, each with its own
//! terminated block: `position_at_end` (on a `module_new!` module, whose
//! brand is a `UniqueBrand`) and its checked twin `try_position_at_end`.
//! Each accepts only `BasicBlock<R, Unterminated>`, so each call is `E0308`.

use llvmkit_ir::{IrBuilder, IrError, Linkage, module_new};

fn main() -> Result<(), IrError> {
    let mut module = module_new!("c")?;
    let mut key = module.layout_token();
    let m = key.module();
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)?
        .as_function();
    let entry = m.view(f).append_basic_block(m, "entry");
    let other = m.view(f).append_basic_block(m, "other");
    let (terminated, _) = IrBuilder::new_for::<()>(&mut key).position_at_end(entry).ret_void();
    let (terminated_other, _) = IrBuilder::new_for::<()>(&mut key).position_at_end(other).ret_void();
    // Each block now carries `Terminated`, which neither entry accepts.
    let _ = IrBuilder::new_for::<()>(&mut key).position_at_end(terminated);
    let _ = IrBuilder::new_for::<()>(&mut key).try_position_at_end(terminated_other);
    Ok(())
}
