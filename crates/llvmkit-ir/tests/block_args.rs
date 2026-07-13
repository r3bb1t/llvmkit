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

/// Multiple params: `params[i]` is the i-th param type, and the head-phis
/// print in that same order — the ordering contract the block-argument branch
/// builders rely on to line up carried values with target params.
#[test]
fn append_block_with_params_preserves_param_order() -> Result<(), IrError> {
    Module::with_new("block_args_order", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;

        let b = IRBuilder::new_for::<i32>(&m);
        let (_hdr, params) =
            b.append_block_with_params(f, &[i32_ty.as_type(), i64_ty.as_type()], "hdr")?;

        // params vector mirrors the requested type order.
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].ty(), i32_ty.as_type());
        assert_eq!(params[1].ty(), i64_ty.as_type());

        // The two head-phis print in the same order (i32 before i64).
        let text = format!("{m}");
        let i32_pos = text.find("phi i32").expect("i32 head-phi printed");
        let i64_pos = text.find("phi i64").expect("i64 head-phi printed");
        assert!(
            i32_pos < i64_pos,
            "head-phis must print in param order (i32 before i64), got:\n{text}"
        );
        Ok(())
    })
}
