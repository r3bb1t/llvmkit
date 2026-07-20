//! Typed block-parameter schema (`append_block_typed`): a block declared with
//! a [`FunctionParamList`]-shaped parameter tuple is stamped with that tuple as
//! its `Params` marker and hands back *typed* parameter handles sourced from the
//! block's leading head-phis — the block-argument analog of a typed function's
//! `params()`. The erased [`IRBuilder::append_block_with_params`] keeps
//! producing the parameter-erased `Vec<Value>` form, unchanged.

use llvmkit_ir::{
    BasicBlock, BasicBlockLabel, BlockParamsDyn, IRBuilder, IntValue, IrError, Linkage, Module,
    PointerValue, Ptr, Unterminated, Value,
};

/// `append_block_typed::<(i32, Ptr)>` returns a `BasicBlock<…, (i32, Ptr)>` and
/// a typed `(IntValue<'_, i32>, PointerValue<'_>)` tuple. The explicit binding
/// annotation is the compile-time assertion that the block carries the tuple as
/// its `Params` marker and the handles are the schema's per-position `Value`
/// types; the runtime `ty()` checks (and the printed head-phis) prove those
/// handles are the block's leading head-phis, typed i32 / ptr in order.
#[test]
fn append_block_typed_yields_typed_params_from_head_phis() -> Result<(), IrError> {
    Module::with_new("block_params_typed", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)?
            .as_function();

        let b = IRBuilder::new_for::<()>(&m);

        // Compile-time assertion: the returned block is stamped with the
        // `(i32, Ptr)` schema and the values are that schema's typed handles.
        // Split into per-binding annotations so each named type stays simple.
        let (head, params) = b.append_block_typed::<(i32, Ptr), _>(f, "head")?;
        let head: BasicBlock<'_, (), Unterminated, _, (i32, Ptr)> = head;
        let (p0, p1): (IntValue<'_, i32>, PointerValue<'_>) = params;

        // Runtime: each handle's IR type matches its schema position, and both
        // are sourced from the freshly built head-phis.
        assert_eq!(p0.as_value().ty(), i32_ty.as_type());
        assert_eq!(p1.as_value().ty(), ptr_ty.as_type());

        // Compile-time assertion: the typed block's label threads `Params`, so a
        // typed branch target keeps its `(i32, Ptr)` promise.
        let label: BasicBlockLabel<'_, (), _, (i32, Ptr)> = head.label();
        assert_eq!(label.as_value().name().as_deref(), Some("head"));

        // The parameters are the block's *leading head-phis*: they print as
        // `phi i32` / `phi ptr` at the block head, in declaration order.
        let text = format!("{m}");
        let i32_pos = text.find("phi i32").expect("i32 head-phi printed");
        let ptr_pos = text.find("phi ptr").expect("ptr head-phi printed");
        assert!(
            i32_pos < ptr_pos,
            "head-phis must print in schema order (i32 before ptr), got:\n{text}"
        );
        Ok(())
    })
}

/// The typed constructor is additive: the erased
/// [`IRBuilder::append_block_with_params`] still returns the parameter-erased
/// block (`Params` defaulting to [`BlockParamsDyn`]) paired with a
/// `Vec<Value>` of untyped head-phi results — unchanged by this slice.
#[test]
fn append_block_with_params_stays_erased() -> Result<(), IrError> {
    Module::with_new("block_params_erased", |m| {
        let i32_ty = m.i32_type();
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)?
            .as_function();

        let b = IRBuilder::new_for::<()>(&m);

        // Compile-time assertion: the erased sibling yields a `BlockParamsDyn`
        // block and an untyped `Vec<Value>`, exactly as before.
        let (erased, params): (
            BasicBlock<'_, (), Unterminated, _, BlockParamsDyn>,
            Vec<Value<'_, _>>,
        ) = b.append_block_with_params(f, &[i32_ty.as_type()], "erased")?;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].ty(), i32_ty.as_type());
        assert_eq!(erased.label().as_value().name().as_deref(), Some("erased"));
        Ok(())
    })
}

/// A zero-parameter typed block (`()`) is the arity-0 typed inhabitant: it is a
/// distinct `Params` marker from `BlockParamsDyn`, carries no head-phis, and
/// returns the unit value tuple.
#[test]
fn append_block_typed_unit_params() -> Result<(), IrError> {
    Module::with_new("block_params_unit", |m| {
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)?
            .as_function();

        let b = IRBuilder::new_for::<()>(&m);

        let (head, ()): (BasicBlock<'_, (), Unterminated, _, ()>, ()) =
            b.append_block_typed::<(), _>(f, "head")?;

        // No head-phis were materialised.
        assert_eq!(head.instructions().count(), 0);
        let label: BasicBlockLabel<'_, (), _, ()> = head.label();
        assert_eq!(label.as_value().name().as_deref(), Some("head"));
        Ok(())
    })
}
