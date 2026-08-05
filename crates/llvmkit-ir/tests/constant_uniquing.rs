//! The constant-uniquing law.
//!
//! **No upstream counterpart.** In LLVM, uniquing is `Constant*` pointer
//! identity maintained by `LLVMContextImpl`'s per-kind maps, and no unit test
//! asserts it directly — every fold that writes `A == B` on two `Constant*`
//! depends on it silently. llvmkit reaches the same property through arena
//! ids, so the invariant is worth pinning explicitly: **two structurally equal
//! constants of the same kind are one arena node.** The nearest upstream
//! observation of the property is `TEST(ConstantsTest, UseCounts)`, which
//! builds one `ConstantInt` and hands it to two globals.
//!
//! One difference from LLVM that this file also pins: llvmkit constants are
//! uniqued **per module**, not per context, because a `Module` owns its
//! `Context` outright. Two modules never share a constant node.

use llvmkit_ir::{
    ConstantIntValue, DynBrand, IrError, Module, constant_fold_select_instruction, module_new,
};

/// Every kind that mints an interned constant answers with the same node when
/// asked twice for the same value.
#[test]
fn structurally_equal_constants_are_one_node() -> Result<(), IrError> {
    let m = module_new!("uniquing")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let ptr_ty = m.ptr_type(0);

    // Scalars, already uniqued before this cycle — regression guard.
    assert_eq!(i32_ty.const_int(7_i32), i32_ty.const_int(7_i32));
    assert_ne!(i32_ty.const_int(7_i32), i32_ty.const_int(8_i32));
    assert_eq!(f32_ty.const_float(1.5_f32), f32_ty.const_float(1.5_f32));
    assert_eq!(ptr_ty.const_null(), ptr_ty.const_null());
    assert_eq!(i32_ty.as_type().undef(), i32_ty.as_type().undef());
    assert_eq!(i32_ty.as_type().poison(), i32_ty.as_type().poison());
    // Undef and poison are different constants of the same type — and are
    // separate Rust types, so the comparison is spelled on the erased handle.
    assert_ne!(
        i32_ty.as_type().undef().as_constant(),
        i32_ty.as_type().poison().as_constant()
    );

    // The four kinds this cycle added maps for.
    let g = m.add_global_constant("g", i8_ty.const_int(0_i8))?;
    let h = m.add_global_constant("h", i8_ty.const_int(0_i8))?;

    assert_eq!(
        m.view(g).as_global_constant_ptr(),
        m.view(g).as_global_constant_ptr(),
        "GlobalValueRef uniques on (type, global)"
    );
    assert_ne!(
        m.view(g).as_global_constant_ptr(),
        m.view(h).as_global_constant_ptr(),
        "different globals key differently"
    );

    assert_eq!(
        m.view(g).ptr_offset(4),
        m.view(g).ptr_offset(4),
        "GepOffset uniques on (type, global, offset)"
    );
    assert_ne!(
        m.view(g).ptr_offset(4),
        m.view(g).ptr_offset(8),
        "the offset is part of the key"
    );
    assert_ne!(
        m.view(g).ptr_offset(4),
        m.view(h).ptr_offset(4),
        "the base global is part of the key"
    );
    assert_ne!(
        m.view(g).as_global_constant_ptr_offset(4, 0),
        m.view(g).as_global_constant_ptr_offset(4, 1),
        "the pointer type — and so the address space — is part of the key"
    );

    assert_eq!(
        m.view(g).try_delta_from(m.view(h))?,
        m.view(g).try_delta_from(m.view(h))?,
        "SymbolDelta uniques on (type, hi, lo)"
    );
    assert_ne!(
        m.view(g).try_delta_from(m.view(h))?,
        m.view(h).try_delta_from(m.view(g))?,
        "the delta is not commutative, so operand order is part of the key"
    );

    assert_eq!(
        m.view(g).try_delta_from_plus(m.view(h), 42)?,
        m.view(g).try_delta_from_plus(m.view(h), 42)?,
        "SymbolDeltaPlus uniques on (type, hi, lo, addend)"
    );
    assert_ne!(
        m.view(g).try_delta_from_plus(m.view(h), 42)?,
        m.view(g).try_delta_from_plus(m.view(h), 43)?,
        "the addend is part of the key"
    );
    Ok(())
}

/// A forward `blockaddress` placeholder is the one constant kind deliberately
/// left un-uniqued: placeholders carry no payload, so uniquing would collapse
/// every pending forward reference in a module into one node and resolving the
/// first would resolve them all.
#[test]
fn block_address_placeholders_stay_distinct() -> Result<(), IrError> {
    let m = module_new!("uniquing-placeholder")?;
    let ptr_ty = m.ptr_type(0);
    let first = m.block_address_placeholder(ptr_ty.as_type())?;
    let second = m.block_address_placeholder(ptr_ty.as_type())?;
    assert_ne!(
        first.as_constant(),
        second.as_constant(),
        "each pending forward blockaddress must remain its own node"
    );
    Ok(())
}

/// Uniquing is per module, since a `Module` owns its `Context`. Two modules
/// hold independent constant tables — the identity that folds rely on is only
/// ever asked within one module, and a foreign id is rejected by the tag check
/// rather than silently matching.
#[test]
fn uniquing_does_not_cross_modules() -> Result<(), IrError> {
    let first: Module<DynBrand> = Module::dynamic("first");
    let second: Module<DynBrand> = Module::dynamic("second");
    let a = first.i32_type().const_int(7_i32);
    let b = second.i32_type().const_int(7_i32);
    assert_ne!(
        a.as_constant(),
        b.as_constant(),
        "same value, different modules, different nodes"
    );
    Ok(())
}

/// Two users that separately ask for the same constant end up naming one
/// operand — the shape LLVM has, where one `Constant*` is shared by every user
/// in the context.
#[test]
fn separate_users_name_one_constant() -> Result<(), IrError> {
    let m = module_new!("uniquing-uses")?;
    let i8_ty = m.i8_type();
    let g = m.add_global_constant("g", i8_ty.const_int(0_i8))?;

    // Two globals whose initialiser is the same pointer-into-@g constant.
    let first = m.add_global("user0", m.view(g).ptr_offset(4))?;
    let second = m.add_global("user1", m.view(g).ptr_offset(4))?;

    let first_init = m.view(first).initializer().expect("initialised at birth");
    let second_init = m.view(second).initializer().expect("initialised at birth");
    assert_eq!(
        first_init, second_init,
        "both initialisers are the one uniqued constant"
    );
    Ok(())
}

/// The int-constant table keys on the type as well as the value, so the same
/// numeric bits at two widths stay two nodes.
#[test]
fn int_constants_key_on_width() -> Result<(), IrError> {
    let m = module_new!("uniquing-int-width")?;
    let narrow: ConstantIntValue<'_, i8, _> = m.i8_type().const_int(1_i8);
    let wide: ConstantIntValue<'_, i32, _> = m.i32_type().const_int(1_i32);
    assert_ne!(narrow.as_constant(), wide.as_constant());
    Ok(())
}

/// A behavioural consequence: `select c, X, X -> X` reaches its identity arm
/// when the two arms were built independently.
///
/// Upstream's arm is `if (V1 == V2) return V1;`
/// (`ConstantFold.cpp::ConstantFoldSelectInstruction`), sound there because
/// `Constant*` is uniqued. Before this cycle, two `ptr_offset(4)` calls on the
/// same global were two arena nodes, so the arm was unreachable for them and
/// the fold declined — an under-fold, never a wrong answer.
#[test]
fn select_with_independently_built_equal_arms_folds() -> Result<(), IrError> {
    let m = module_new!("uniquing-select")?;
    let i8_ty = m.i8_type();
    let g = m.add_global_constant("g", i8_ty.const_int(0_i8))?;
    let arm = || m.view(g).ptr_offset(4);

    let folded = constant_fold_select_instruction(
        m.bool_type().const_int(true).as_constant(),
        arm(),
        arm(),
    )?
    .expect("both arms are the same constant, so the select folds to it");
    assert_eq!(folded, arm());
    Ok(())
}
