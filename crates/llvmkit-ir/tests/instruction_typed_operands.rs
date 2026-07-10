//! P1 typed-operand coverage.
//!
//! Pointer operands rediscovered through `InstructionKind` come back as
//! [`PointerValue`] rather than the erased `Value`, and a direct call's
//! callee classifies as [`Callee::Direct`] carrying a [`FunctionValue`].

use llvmkit_ir::cmp_predicate::{CmpPredicate, IntPredicate};
use llvmkit_ir::instr_types::BinaryOpcode;
use llvmkit_ir::{
    Callee, Classified, IRBuilder, InstructionKind, InstructionView, IntValue, IrError, Linkage,
    Module, PointerValue, TerminatorKind, Value,
};

/// A rediscovered `load`'s pointer operand is statically a `PointerValue`.
#[test]
fn load_pointer_operand_is_typed() -> Result<(), IrError> {
    Module::with_new("typed_load_ptr", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let p: PointerValue = f.param(0)?.try_into()?;
        let loaded = b.build_load(i32_ty, p, "v")?;

        let view = InstructionView::try_from(loaded)?;
        let Some(InstructionKind::Load(load)) = view.kind() else {
            panic!("expected a load instruction");
        };
        // `pointer()` returns `PointerValue`, not an erased `Value`.
        let ptr: PointerValue = load.pointer();
        assert_eq!(ptr.as_value(), p.as_value());
        Ok(())
    })
}

/// A direct call classifies its callee as `Direct` carrying the function.
#[test]
fn direct_call_callee_is_direct() -> Result<(), IrError> {
    Module::with_new("direct_call", |m| {
        let i32_ty = m.i32_type();
        let callee_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let callee = m.add_function::<i32, _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let caller = m.add_function::<i32, _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let call = b.build_call_dyn(callee, [x.as_value()], "r")?;

        match call.classify_callee() {
            Callee::Direct(function) => assert_eq!(function.as_value(), callee.as_value()),
            Callee::Indirect(_) => panic!("expected a direct call to classify as Direct"),
        }
        Ok(())
    })
}

/// `classify()` is total: a non-terminator lands in `Inst`, a terminator
/// in `Term`, with no overloaded `None` to forget.
#[test]
fn classify_is_total() -> Result<(), IrError> {
    Module::with_new("classify_total", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let y: IntValue<i32> = f.param(1)?.try_into()?;
        let sum = b.build_int_add::<i32, _, _, _>(x, y, "s")?;
        b.build_ret(sum)?;

        let sum_view = InstructionView::try_from(sum.as_value())?;
        assert!(matches!(
            sum_view.classify(),
            Classified::Inst(InstructionKind::Add(_))
        ));

        // The block terminator classifies as Term(Ret) — the case the
        // split kind()/terminator_kind() pair makes easy to miss.
        let term = f
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
    })
}

/// `as_binary_op` groups any arithmetic opcode, and `as_cmp` groups
/// `icmp`/`fcmp` behind a unified predicate.
#[test]
fn binop_and_cmp_groupings() -> Result<(), IrError> {
    Module::with_new("groupings", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        // Non-constant operands so the folder leaves real instructions.
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let y: IntValue<i32> = f.param(1)?.try_into()?;
        let sum = b.build_int_add::<i32, _, _, _>(x, y, "s")?;
        let cmp = b.build_icmp_slt::<i32, _, _, _>(x, y, "c")?;

        let sum_view = InstructionView::try_from(sum.as_value())?;
        let bop = sum_view
            .kind()
            .and_then(|k| k.as_binary_op())
            .expect("add classifies as a binary op");
        assert_eq!(bop.opcode(), BinaryOpcode::Add);
        assert!(bop.is_commutative());
        assert_eq!(bop.lhs(), x.as_value());
        assert_eq!(bop.rhs(), y.as_value());

        let cmp_view = InstructionView::try_from(cmp.as_value())?;
        let cv = cmp_view
            .kind()
            .and_then(|k| k.as_cmp())
            .expect("icmp classifies as a cmp");
        assert_eq!(cv.predicate(), CmpPredicate::Int(IntPredicate::Slt));
        assert!(cv.is_integer());
        assert_eq!(cv.lhs(), x.as_value());
        Ok(())
    })
}

/// An indirect call (callee is a function-pointer argument) classifies as
/// `Indirect` carrying a `PointerValue`.
#[test]
fn indirect_call_callee_is_indirect() -> Result<(), IrError> {
    Module::with_new("indirect_call", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        // define i32 @caller(ptr %fp) { %r = call i32 %fp(); ret ... }
        let caller_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
        let caller = m.add_function::<i32, _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let fp: PointerValue = caller.param(0)?.try_into()?;
        let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let call =
            b.build_indirect_call_dyn::<i32, _, Value, _>(callee_ty, fp, Vec::<Value>::new(), "r")?;

        match call.classify_callee() {
            Callee::Indirect(pointer) => assert_eq!(pointer.as_value(), fp.as_value()),
            Callee::Direct(_) => panic!("expected an indirect call to classify as Indirect"),
        }
        Ok(())
    })
}
