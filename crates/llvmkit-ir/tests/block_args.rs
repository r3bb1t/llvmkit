//! Block-argument authoring surface for phis: `append_block_with_params`
//! creates a block whose parameters are operandless head-phis (the
//! Swift-SIL / MLIR block-argument shape). Incomings are supplied later by
//! branching to the block with the block-argument branch builders, so this
//! test only proves the block + head-phi(s) + returned param `Value`s exist
//! and print.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

/// A block appended with one `i32` parameter carries a single head-phi of
/// that type at its head, and the returned params vector surfaces that phi
/// as a `Value` of the right type.
#[test]
fn append_block_with_params_creates_head_phi() -> Result<(), IrError> {
    Module::with_new("block_args", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;

        // No positioning required: the block is created against `f`, not the
        // builder's cursor.
        let b = IRBuilder::new_for::<i32>(&m);
        let (hdr, params) = b.append_block_with_params(f, &[i32_ty.as_type()], "hdr")?;

        // (a) params vector: one entry, typed i32, backed by the head-phi.
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].ty(), i32_ty.as_type());

        // The returned block handle is the freshly-appended `hdr`.
        assert_eq!(hdr.label().as_value().name().as_deref(), Some("hdr"));

        // (b) the new block prints with a `phi i32` head-phi. `hdr` is the
        // only block carrying a phi, so its presence in the module text
        // proves the head-phi was materialised at the block's head.
        let text = format!("{m}");
        assert!(
            text.contains("phi i32"),
            "expected a `phi i32` head-phi in the printed module, got:\n{text}"
        );
        Ok(())
    })
}
