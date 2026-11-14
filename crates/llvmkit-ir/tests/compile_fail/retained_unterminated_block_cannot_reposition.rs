//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! A terminator builder consumes its positioned builder and returns a
//! `Terminated` block view. Retaining an earlier `Unterminated` block handle
//! must not permit a second builder to append after the terminator: each
//! positioning entry moves its block, so a second use is `E0382`. Both are
//! pinned, each with its own block: `position_at_end` (on a `module_new!`
//! module, whose brand is a `UniqueBrand`) and its checked twin
//! `try_position_at_end`.

use llvmkit_ir::{IrBuilder, IrError, Linkage, module_new};

fn main() -> Result<(), IrError> {
    let mut module = module_new!("retained-block")?;
    let mut key = module.layout_token();
    let m = key.module();
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)?
        .as_function();
    let entry = m.view(f).append_basic_block(m, "entry");
    let other = m.view(f).append_basic_block(m, "other");

    IrBuilder::new_for::<()>(&mut key).position_at_end(entry).ret_void();
    IrBuilder::new_for::<()>(&mut key).try_position_at_end(other)?.ret_void();

    let _ = IrBuilder::new_for::<()>(&mut key).position_at_end(entry);
    let _ = IrBuilder::new_for::<()>(&mut key).try_position_at_end(other);
    Ok(())
}
