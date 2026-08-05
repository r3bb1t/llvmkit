//! P1 typed-operand coverage.
//!
//! Pointer operands rediscovered through `InstructionKind` come back as
//! [`PointerValue`] rather than the erased `Value`, and a direct call's
//! callee classifies as [`Callee::Direct`] carrying a [`FunctionValue`].

use llvmkit_ir::cmp_predicate::{CmpPredicate, IntPredicate};
use llvmkit_ir::instr_types::BinaryOpcode;
use llvmkit_ir::{
    Callee, Classified, Dyn, InstructionKind, InstructionView, IntValue, IrBuilder, IrError,
    Linkage, PointerValue, TerminatorKind, Value, module_new,
};

/// A rediscovered `load`'s pointer operand is statically a `PointerValue`.
#[test]
fn load_pointer_operand_is_typed() -> Result<(), IrError> {
    let m = module_new!("typed_load_ptr")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let loaded = b.build_load(i32_ty, p, "v")?;

    let view = InstructionView::try_from(b.view(loaded))?;
    let Some(InstructionKind::Load(load)) = view.kind() else {
        panic!("expected a load instruction");
    };
    // `pointer()` returns `PointerValue`, not an erased `Value`.
    let ptr: PointerValue<'_, _> = load.pointer();
    assert_eq!(ptr.into_erased(), p.into_erased());
    Ok(())
}

/// A direct call classifies its callee as `Direct` carrying the function.
#[test]
fn direct_call_callee_is_direct() -> Result<(), IrError> {
    let m = module_new!("direct_call")?;
    let i32_ty = m.i32_type();
    let callee_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(caller).param(0)?.try_into()?;
    let call = b.build_call_dyn(callee, [x.into_erased()], "r")?;

    match b.view(call).classify_callee() {
        Callee::Direct(function) => {
            assert_eq!(function.into_erased(), b.view(callee).into_erased())
        }
        Callee::Indirect(_) => panic!("expected a direct call to classify as Direct"),
    }
    Ok(())
}

/// `classify()` is total: a non-terminator lands in `Inst`, a terminator
/// in `Term`, with no overloaded `None` to forget.
#[test]
fn classify_is_total() -> Result<(), IrError> {
    let m = module_new!("classify_total")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let y: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let sum = b.build_int_add::<i32, _, _, _>(x, y, "s")?;
    b.build_ret(sum)?;

    let sum_view = InstructionView::try_from(m.view(sum).into_erased())?;
    assert!(matches!(
        sum_view.classify(),
        Classified::Inst(InstructionKind::Add(_))
    ));

    // The block terminator classifies as Term(Ret) — the case the
    // split kind()/terminator_kind() pair makes easy to miss.
    let term = m
        .view(f)
        .basic_blocks()
        .next()
        .unwrap()
        .terminator()
        .expect("entry has a terminator");
    assert!(matches!(
        term.classify(),
        Classified::Term(TerminatorKind::Ret(_))
    ));
    Ok(())
}

/// `as_binary_op` groups any arithmetic opcode, and `as_cmp` groups
/// `icmp`/`fcmp` behind a unified predicate.
#[test]
fn binop_and_cmp_groupings() -> Result<(), IrError> {
    let m = module_new!("groupings")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    // Non-constant operands so the folder leaves real instructions.
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let y: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let sum = b.build_int_add::<i32, _, _, _>(x, y, "s")?;
    let cmp = b.build_icmp_slt::<i32, _, _, _>(x, y, "c")?;

    let sum_view = InstructionView::try_from(b.view(sum).into_erased())?;
    let bop = sum_view
        .kind()
        .and_then(|k| k.as_binary_op())
        .expect("add classifies as a binary op");
    assert_eq!(bop.opcode(), BinaryOpcode::Add);
    assert!(bop.is_commutative());
    assert_eq!(bop.lhs(), x.into_erased());
    assert_eq!(bop.rhs(), y.into_erased());

    let cmp_view = InstructionView::try_from(b.view(cmp).into_erased())?;
    let cv = cmp_view
        .kind()
        .and_then(|k| k.as_cmp())
        .expect("icmp classifies as a cmp");
    assert_eq!(cv.predicate(), CmpPredicate::Int(IntPredicate::Slt));
    assert!(cv.is_integer());
    assert_eq!(cv.lhs(), x.into_erased());
    Ok(())
}

/// An indirect call (callee is a function-pointer argument) classifies as
/// `Indirect` carrying a `PointerValue`.
#[test]
fn indirect_call_callee_is_indirect() -> Result<(), IrError> {
    let m = module_new!("indirect_call")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    // define i32 @caller(ptr %fp) { %r = call i32 %fp(); ret ... }
    let caller_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let fp: PointerValue<'_, _> = m.view(caller).param(0)?.try_into()?;
    let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new(), false);
    let call = b.build_indirect_call_dyn::<i32, _, Value<'_, _>, _, _>(
        callee_ty,
        fp,
        Vec::<Value<'_, _>>::new(),
        "r",
    )?;

    match b.view(call).classify_callee() {
        Callee::Indirect(pointer) => assert_eq!(pointer.into_erased(), fp.into_erased()),
        Callee::Direct(_) => panic!("expected an indirect call to classify as Indirect"),
    }
    Ok(())
}
