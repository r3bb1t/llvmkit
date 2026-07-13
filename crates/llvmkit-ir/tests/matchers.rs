//! Pattern-match DSL coverage — patterns transliterated from real
//! InstCombine folds (`orig_cpp/.../lib/Transforms/InstCombine`).

use llvmkit_ir::matchers::*;
use llvmkit_ir::{
    IRBuilder, IntDyn, IntValue, IrError, Linkage, Module, PhiKind, PointerValue, Value,
};

/// Helper: rediscover an instruction's `InstructionView` from its result.
fn view_of<'ctx>(v: llvmkit_ir::Value<'ctx>) -> llvmkit_ir::InstructionView<'ctx> {
    llvmkit_ir::InstructionView::try_from(v).expect("value is an instruction")
}

/// `InstCombineAddSub.cpp:878` — `add (sub X, Y), -1`. The pattern
/// `m_add(m_one_use(m_sub(m_value(), m_value())), m_all_ones())` binds
/// `(X, Y)` in order.
#[test]
fn add_sub_allones_binds_operands() -> Result<(), IrError> {
    Module::with_new("m_add_sub", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let y: IntValue<i32> = f.param(1)?.try_into()?;
        let sub = b.build_int_sub::<i32, _, _, _>(x, y, "s")?;
        let neg_one = i32_ty.const_int(-1_i32);
        let add = b.build_int_add::<i32, _, _, _>(sub, neg_one, "r")?;

        let view = view_of(add.as_value());
        let (mx, my) = m_add(m_one_use(m_sub(m_value(), m_value())), m_all_ones())
            .match_view(&view)
            .expect("pattern should match");
        assert_eq!(mx, x.as_value());
        assert_eq!(my, y.as_value());
        Ok(())
    })
}

/// The `m_one_use` gate: if the `sub` feeds two users, the same pattern
/// no longer matches.
#[test]
fn one_use_gate_rejects_multi_use_subexpr() -> Result<(), IrError> {
    Module::with_new("m_one_use", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let y: IntValue<i32> = f.param(1)?.try_into()?;
        let sub = b.build_int_sub::<i32, _, _, _>(x, y, "s")?;
        let neg_one = i32_ty.const_int(-1_i32);
        let add = b.build_int_add::<i32, _, _, _>(sub, neg_one, "r")?;
        // Second user of `sub`, so it no longer has one use.
        let _other = b.build_int_add::<i32, _, _, _>(sub, x, "o")?;

        let view = view_of(add.as_value());
        assert!(
            m_add(m_one_use(m_sub(m_value(), m_value())), m_all_ones())
                .match_view(&view)
                .is_none()
        );
        // Without the gate it still matches.
        assert!(
            m_add(m_sub(m_value(), m_value()), m_all_ones())
                .match_view(&view)
                .is_some()
        );
        Ok(())
    })
}

/// Commutative matching: `m_c_add(m_specific(x), m_value())` matches an
/// `add %y, %x` whose operands are in the *other* order.
#[test]
fn commutative_add_matches_swapped_operands() -> Result<(), IrError> {
    Module::with_new("m_c_add", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let y: IntValue<i32> = f.param(1)?.try_into()?;
        // add %y, %x  (x is the second operand)
        let add = b.build_int_add::<i32, _, _, _>(y, x, "r")?;

        let view = view_of(add.as_value());
        // Non-commutative fails (x is not operand 0)...
        assert!(
            m_add(m_specific(x.as_value()), m_value())
                .match_view(&view)
                .is_none()
        );
        // ...commutative succeeds and binds the other operand (y).
        let (bound,) = m_c_add(m_specific(x.as_value()), m_value())
            .match_view(&view)
            .expect("commutative add should match");
        assert_eq!(bound, y.as_value());
        Ok(())
    })
}

/// `m_not` matches `xor %v, -1` and `m_neg` matches `sub 0, %v`.
#[test]
fn not_and_neg_sugar() -> Result<(), IrError> {
    Module::with_new("m_not_neg", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let v: IntValue<i32> = f.param(0)?.try_into()?;
        let not = b.build_int_xor::<i32, _, _, _>(v, i32_ty.const_int(-1_i32), "n")?;
        let neg = b.build_int_sub::<i32, _, _, _>(i32_ty.const_int(0_i32), v, "g")?;

        let (nv,) = m_not(m_value())
            .match_view(&view_of(not.as_value()))
            .expect("m_not should match xor v, -1");
        assert_eq!(nv, v.as_value());

        let (gv,) = m_neg(m_value())
            .match_view(&view_of(neg.as_value()))
            .expect("m_neg should match sub 0, v");
        assert_eq!(gv, v.as_value());
        Ok(())
    })
}

/// The P1↔P2 synergy: "load of a gep". `m_load(m_gep(m_value()))` binds the
/// GEP base pointer.
#[test]
fn load_of_gep_binds_base() -> Result<(), IrError> {
    Module::with_new("m_load_gep", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let base: PointerValue = f.param(0)?.try_into()?;
        let idx: IntValue<IntDyn> = f.param(1)?.try_into()?;
        let gep = b.build_gep(i32_ty, base, [idx], "p")?;
        let load = b.build_load(i32_ty, gep, "v")?;

        let (bound,): (Value,) = m_load(m_gep(m_value()))
            .match_view(&view_of(load))
            .expect("load-of-gep should match");
        assert_eq!(bound, base.as_value());
        Ok(())
    })
}

/// Constant predicates over materialised constants.
#[test]
fn constant_predicates() -> Result<(), IrError> {
    Module::with_new("m_const", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        // Materialised constants; matched directly as Values.
        let zero = i32_ty.const_int(0_i32).as_value();
        let one = i32_ty.const_int(1_i32).as_value();
        let all_ones = i32_ty.const_int(-1_i32).as_value();
        let eight = i32_ty.const_int(8_i32).as_value();

        assert!(Matcher::<'_, _>::try_match(&m_zero(), zero).is_some());
        assert!(Matcher::<'_, _>::try_match(&m_one(), one).is_some());
        assert!(Matcher::<'_, _>::try_match(&m_all_ones(), all_ones).is_some());
        assert!(Matcher::<'_, _>::try_match(&m_power2(), eight).is_some());
        assert!(Matcher::<'_, _>::try_match(&m_power2(), one).is_some());
        assert!(Matcher::<'_, _>::try_match(&m_negative(), all_ones).is_some());
        assert!(Matcher::<'_, _>::try_match(&m_negative(), one).is_none());
        // A non-constant (the parameter) matches no constant predicate.
        assert!(Matcher::<'_, _>::try_match(&m_zero(), x.as_value()).is_none());

        // m_ap_int binds the value.
        let (ap,) = Matcher::<'_, _>::try_match(&m_ap_int(), eight).expect("const int");
        assert_eq!(ap.try_sext_i128(), Some(8));

        // m_specific_int matches an exact value.
        assert!(Matcher::<'_, _>::try_match(&m_specific_int(8), eight).is_some());
        assert!(Matcher::<'_, _>::try_match(&m_specific_int(9), eight).is_none());
        Ok(())
    })
}

/// Two-step binding mirrors InstCombine's `m_Specific` reuse across two
/// `match()` calls: bind `(a, b)` from the `or`, then require the same
/// values inside the `and`.
#[test]
fn two_step_specific_reuse() -> Result<(), IrError> {
    Module::with_new("m_two_step", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let y: IntValue<i32> = f.param(1)?.try_into()?;
        let or = b.build_int_or::<i32, _, _, _>(x, y, "o")?;
        let and = b.build_int_and::<i32, _, _, _>(x, y, "a")?;

        // Step 1: bind (a, b) from the `or`.
        let (a, bb) = m_or(m_value(), m_value())
            .match_view(&view_of(or.as_value()))
            .expect("or matches");
        // Step 2: require the `and` to use exactly those, in either order.
        assert!(
            m_c_and(m_specific(a), m_specific(bb))
                .match_view(&view_of(and.as_value()))
                .is_some()
        );
        Ok(())
    })
}

/// `m_phi()` matches any phi and binds its result-typed [`PhiKind`]
/// discriminator — an `i32` phi surfaces as `PhiKind::Int`.
#[test]
fn m_phi_binds_phi_kind() -> Result<(), IrError> {
    Module::with_new("m_phi_bind", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");

        // join(%p: i32): two unconditional predecessors merge here, each
        // carrying its own constant into the head-phi: `[1, entry], [2, other]`.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (join, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "join")?;
        let join_label = join.label();

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_br_with_args(join_label, &[i32_ty.const_int(1_i32).as_value()])?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(other)
            .build_br_with_args(join_label, &[i32_ty.const_int(2_i32).as_value()])?;

        let view = view_of(params[0]);
        let (kind,) = m_phi().match_view(&view).expect("phi matches");
        assert!(matches!(kind, PhiKind::Int(_)));
        Ok(())
    })
}

/// `m_phi()` rejects a non-phi instruction (an `add`).
#[test]
fn m_phi_rejects_non_phi() -> Result<(), IrError> {
    Module::with_new("m_phi_reject", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let y: IntValue<i32> = f.param(1)?.try_into()?;
        let add = b.build_int_add::<i32, _, _, _>(x, y, "r")?;

        let view = view_of(add.as_value());
        assert!(m_phi().match_view(&view).is_none());
        Ok(())
    })
}

/// `m_phi()` composes under [`m_one_use`]: a phi whose result has exactly
/// one use still matches through the gate.
#[test]
fn m_phi_composes_with_m_one_use() -> Result<(), IrError> {
    Module::with_new("m_phi_one_use", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");

        // join(%p: i32): two unconditional predecessors merge here, carrying
        // `[1, entry], [2, other]` into the head-phi.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (join, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "join")?;
        let join_label = join.label();

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_br_with_args(join_label, &[i32_ty.const_int(1_i32).as_value()])?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(other)
            .build_br_with_args(join_label, &[i32_ty.const_int(2_i32).as_value()])?;

        // Exactly one use of the phi result: the return.
        let p: IntValue<i32> = params[0].try_into()?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(join)
            .build_ret(p)?;

        let view = view_of(params[0]);
        assert!(m_one_use(m_phi()).match_view(&view).is_some());
        Ok(())
    })
}
