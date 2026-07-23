//! Typed control-flow edge bundle (`BlockCall`) + typed branch builders.
//!
//! `append_block_typed` stamps a block with a `FunctionParamList` schema and
//! hands back typed head-phi handles; `label.call(args)` (or `block.call(args)`)
//! bundles that typed target with block-arguments checked against the schema at
//! *compile* time, and `build_br_call` / `build_cond_br_call` consume the bundle
//! — lowering the (already compile-checked) args and seeding the target's
//! leading head-phis. The erased `build_br_with_args` / `build_cond_br_with_args`
//! path (parameter-erased `BlockParamsDyn`) is unchanged and still works.

use llvmkit_ir::{IRBuilder, IntPredicate, IntValue, IrError, Linkage, Module, PointerValue, Ptr};

/// `build_br_call(head.call((x,)))` seeds `head`'s leading head-phi with the
/// edge argument: the emitted `br label %head` carries `%x` into the head-phi as
/// `[ %x, %entry ]`, and the module verifies. Exercises the *label* constructor
/// `head.label().call(..)`.
#[test]
fn build_br_call_seeds_typed_head_phi_and_verifies() -> Result<(), IrError> {
    Module::with_new("block_call_br", |m| {
        let f = m
            .add_typed_function::<i32, (i32,), _>("f", Linkage::External)?
            .as_function();
        let entry = f.append_basic_block(&m, "entry");

        // head(%p: i32): a typed one-i32-parameter block. `params` is the typed
        // `(IntValue<'_, i32>,)` head-phi handle tuple.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (head, (p,)) = bwp.append_block_typed::<(i32,), _>(f, "head")?;

        // entry: %x = add i32 %a, 1 ; br head(%x)
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let x = b.build_int_add(a, 1_i32, "x")?;
        // `.call((x,))` is compile-checked against the block's `(i32,)` schema.
        b.build_br_call(head.label().call((x,)))?;

        // head: ret %p (the head-phi carrying the branch argument).
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(head);
        b.build_ret(p)?;

        let text = format!("{m}");
        assert!(
            text.contains("br label %head"),
            "expected `br label %head`, got:\n{text}"
        );
        assert!(
            text.contains("phi i32 [ %x, %entry ]"),
            "expected seeded head-phi `phi i32 [ %x, %entry ]`, got:\n{text}"
        );

        m.verify()?;
        Ok(())
    })
}

/// The `block.call(args)` convenience (borrows the block via its label) forms
/// the same edge: a two-parameter typed target seeded from a `(i32, Ptr)` tuple
/// verifies, with both head-phis carrying their edge argument.
#[test]
fn block_call_convenience_two_params_verifies() -> Result<(), IrError> {
    Module::with_new("block_call_two", |m| {
        let f = m
            .add_typed_function::<i32, (i32, Ptr), _>("f", Linkage::External)?
            .as_function();
        let entry = f.append_basic_block(&m, "entry");

        // head(%pi: i32, %pp: ptr): a typed two-parameter block.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (head, (pi, _pp)) = bwp.append_block_typed::<(i32, Ptr), _>(f, "head")?;

        // entry: br head(%a, %ptr) — seed both head-phis, in schema order.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let ptr: PointerValue = f.param(1)?.try_into()?;
        // `block.call((..))` borrows `head`, so it stays usable below.
        b.build_br_call(head.call((a, ptr)))?;

        // head: ret %pi.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(head);
        b.build_ret(pi)?;

        let text = format!("{m}");
        assert!(
            text.contains("phi i32 [ %"),
            "expected an i32 head-phi seeded from entry, got:\n{text}"
        );
        assert!(
            text.contains("phi ptr [ %"),
            "expected a ptr head-phi seeded from entry, got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}

/// `build_cond_br_call` seeds each successor's head-phi from its own edge's
/// arguments: a diamond feeds `%x` to `then(%pt)` and `%y` to `else(%pe)`, both
/// with `entry` as predecessor, so both incomings print and the module verifies.
#[test]
fn build_cond_br_call_two_targets_verify() -> Result<(), IrError> {
    Module::with_new("block_call_condbr", |m| {
        let f = m
            .add_typed_function::<i32, (i32,), _>("f", Linkage::External)?
            .as_function();
        let entry = f.append_basic_block(&m, "entry");

        // then(%pt: i32) and else(%pe: i32): two typed one-parameter blocks.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (then_bb, (pt,)) = bwp.append_block_typed::<(i32,), _>(f, "then")?;
        let (else_bb, (pe,)) = bwp.append_block_typed::<(i32,), _>(f, "else")?;

        // entry: %x = add %a, 1 ; %y = add %a, 2 ;
        //        br (%a == 0) ? then(%x) : else(%y)
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let x = b.build_int_add(a, 1_i32, "x")?;
        let y = b.build_int_add(a, 2_i32, "y")?;
        let cond = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
        b.build_cond_br_call(cond, then_bb.call((x,)), else_bb.call((y,)))?;

        // then: ret %pt ; else: ret %pe.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        b.build_ret(pt)?;
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(else_bb);
        b.build_ret(pe)?;

        let text = format!("{m}");
        assert!(
            text.contains("[ %x, %entry ]"),
            "expected then-edge incoming `[ %x, %entry ]`, got:\n{text}"
        );
        assert!(
            text.contains("[ %y, %entry ]"),
            "expected else-edge incoming `[ %y, %entry ]`, got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}

/// `build_cond_br_call` is generic over each edge's schema independently: a
/// `then` edge with a `(i32,)` schema and an `else` edge with the arity-0 `()`
/// schema coexist in one call (`ThenP != ElseP`) and verify.
#[test]
fn build_cond_br_call_distinct_schemas_per_edge() -> Result<(), IrError> {
    Module::with_new("block_call_condbr_mixed", |m| {
        let i32_ty = m.i32_type();
        let f = m
            .add_typed_function::<i32, (i32,), _>("f", Linkage::External)?
            .as_function();
        let entry = f.append_basic_block(&m, "entry");

        // then(%pt: i32) is typed `(i32,)`; join() is typed `()` (no head-phis).
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (then_bb, (pt,)) = bwp.append_block_typed::<(i32,), _>(f, "then")?;
        let (join_bb, ()) = bwp.append_block_typed::<(), _>(f, "join")?;

        // entry: br (%a == 0) ? then(%a) : join()
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let cond = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
        b.build_cond_br_call(cond, then_bb.call((a,)), join_bb.call(()))?;

        // then: ret %pt ; join: ret 0.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        b.build_ret(pt)?;
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join_bb);
        b.build_ret(i32_ty.const_int(0_i32))?;

        let text = format!("{m}");
        assert!(
            text.contains("phi i32 [ %0, %entry ]"),
            "expected then-edge head-phi seeded with the function arg %0, got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}

/// Regression: the erased `build_br_with_args` path (parameter-erased
/// `BlockParamsDyn` block from `append_block_with_params`) is unchanged and
/// still seeds head-phis and verifies, coexisting with the typed `BlockCall`
/// surface.
#[test]
fn erased_build_br_with_args_still_works() -> Result<(), IrError> {
    Module::with_new("block_call_erased", |m| {
        let i32_ty = m.i32_type();
        let f = m
            .add_typed_function::<i32, (i32,), _>("f", Linkage::External)?
            .as_function();
        let entry = f.append_basic_block(&m, "entry");

        let bwp = IRBuilder::new_for::<i32>(&m);
        let (hdr, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "hdr")?;
        let hdr_label = hdr.label();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let x = b.build_int_add(a, 1_i32, "x")?;
        b.build_br_with_args(hdr_label, &[x.into_erased()])?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(hdr);
        let p: IntValue<i32> = params[0].try_into()?;
        b.build_ret(p)?;

        let text = format!("{m}");
        assert!(
            text.contains("phi i32 [ %x, %entry ]"),
            "expected erased head-phi `phi i32 [ %x, %entry ]`, got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}
