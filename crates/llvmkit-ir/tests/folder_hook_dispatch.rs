//! Which `IrBuilderFolder` hook each integer-binop emitter calls.
//!
//! **No upstream counterpart.** `llvm/unittests/IR/IRBuilderTest.cpp` has no
//! fixture that observes hook selection: upstream's `IRBuilderFolder` is a
//! `virtual` interface and the choice is visible only to a folder written to
//! watch for it. The rule itself is upstream's, and is read off
//! `llvm/include/llvm/IR/IRBuilder.h` directly:
//!
//! - `CreateAdd` / `CreateSub` / `CreateMul` / `CreateShl` call
//!   `Folder.FoldNoWrapBinOp(Opc, LHS, RHS, HasNUW, HasNSW)`, whose two `bool`
//!   parameters default to `false` — so the *flagless* emitters reach that hook
//!   too, with an empty flag pair.
//! - `CreateUDiv` / `CreateSDiv` / `CreateLShr` / `CreateAShr` call
//!   `Folder.FoldExactBinOp(Opc, LHS, RHS, isExact)`, whose `isExact` likewise
//!   defaults to `false`.
//! - `CreateURem` / `CreateSRem` / `CreateAnd` / `CreateOr` / `CreateXor` call
//!   the plain `Folder.FoldBinOp(Opc, LHS, RHS)`.
//!
//! The dispatch is keyed on the **opcode**, never on whether a flag happens to
//! be set. Results are identical under the shipped `ConstantFolder` and
//! `NoFolder`, so a third-party folder overriding only one of the three hooks
//! is the only thing that can see the difference — which is what
//! `RecordingFolder` below is.

use core::cell::RefCell;
use llvmkit_ir::{
    AshrFlags, BinaryOpcode, ExactFlags, IntBinOpFlags, IntValue, IrBuilder, IrBuilderFolder,
    IrError, IrResult, Linkage, ModuleBrand, OverflowFlags, UdivFlags, Value, module_new,
};
use std::rc::Rc;

/// The shared call log. `IrBuilder::with_folder` takes the folder by value and
/// exposes no accessor, so the log lives behind an `Rc` the test keeps a handle
/// to.
type CallLog = Rc<RefCell<Vec<(BinaryOpcode, Hook)>>>;

/// Which of the three binop hooks was called, with the flag values it carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hook {
    /// `FoldBinOp`
    Plain,
    /// `FoldNoWrapBinOp(.., HasNUW, HasNSW)`
    NoWrap { nuw: bool, nsw: bool },
    /// `FoldExactBinOp(.., IsExact)`
    Exact { exact: bool },
}

/// A folder that records the hook it was asked through and always declines.
///
/// Declining keeps the emitters on their instruction-building path, so a test
/// observes only the dispatch and nothing else changes.
struct RecordingFolder {
    calls: CallLog,
}

impl<'ctx, B: ModuleBrand + 'ctx> IrBuilderFolder<'ctx, B> for RecordingFolder {
    fn fold_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        self.calls.borrow_mut().push((opcode, Hook::Plain));
        Ok(None)
    }

    fn fold_no_wrap_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        flags: OverflowFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        self.calls.borrow_mut().push((
            opcode,
            Hook::NoWrap {
                nuw: flags.has_nuw(),
                nsw: flags.has_nsw(),
            },
        ));
        Ok(None)
    }

    fn fold_exact_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        _lhs: Value<'ctx, B>,
        _rhs: Value<'ctx, B>,
        exact: ExactFlags,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        self.calls.borrow_mut().push((
            opcode,
            Hook::Exact {
                exact: exact.is_exact(),
            },
        ));
        Ok(None)
    }
}

/// The flagless typed emitters pick their hook by opcode, exactly as
/// `IRBuilder::CreateAdd` and friends do with their defaulted flag arguments.
#[test]
fn flagless_typed_emitters_reach_the_opcode_s_hook() -> Result<(), IrError> {
    let m = module_new!("folder-hook-flagless")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let calls: CallLog = CallLog::default();
    let b = IrBuilder::with_folder(
        &m,
        RecordingFolder {
            calls: Rc::clone(&calls),
        },
    )
    .position_at_end(entry);
    // Non-constant operands, so the shipped folders would decline too and the
    // test observes dispatch rather than folding.
    let x = IntValue::<i32, _>::try_from(m.view(f).param(0)?)?;
    let y = IntValue::<i32, _>::try_from(m.view(f).param(1)?)?;

    b.int_add::<i32, _, _, _>(x, y, "add")?;
    b.int_sub::<i32, _, _, _>(x, y, "sub")?;
    b.int_mul::<i32, _, _, _>(x, y, "mul")?;
    b.int_shl::<i32, _, _, _>(x, y, "shl")?;
    b.int_udiv::<i32, _, _, _>(x, y, "udiv")?;
    b.int_sdiv::<i32, _, _, _>(x, y, "sdiv")?;
    b.int_lshr::<i32, _, _, _>(x, y, "lshr")?;
    b.int_ashr::<i32, _, _, _>(x, y, "ashr")?;
    b.int_urem::<i32, _, _, _>(x, y, "urem")?;
    b.int_srem::<i32, _, _, _>(x, y, "srem")?;
    b.int_and::<i32, _, _, _>(x, y, "and")?;
    b.int_xor::<i32, _, _, _>(x, y, "xor")?;

    let empty_wrap = Hook::NoWrap {
        nuw: false,
        nsw: false,
    };
    let empty_exact = Hook::Exact { exact: false };
    assert_eq!(
        *calls.borrow(),
        vec![
            (BinaryOpcode::Add, empty_wrap),
            (BinaryOpcode::Sub, empty_wrap),
            (BinaryOpcode::Mul, empty_wrap),
            (BinaryOpcode::Shl, empty_wrap),
            (BinaryOpcode::Udiv, empty_exact),
            (BinaryOpcode::Sdiv, empty_exact),
            (BinaryOpcode::Lshr, empty_exact),
            (BinaryOpcode::Ashr, empty_exact),
            (BinaryOpcode::Urem, Hook::Plain),
            (BinaryOpcode::Srem, Hook::Plain),
            (BinaryOpcode::And, Hook::Plain),
            (BinaryOpcode::Xor, Hook::Plain),
        ]
    );
    Ok(())
}

/// `CreateUDiv(.., isExact)` threads the exactness bit into `FoldExactBinOp`;
/// so does `int_udiv_with_flags`, in both settings.
#[test]
fn exact_flag_reaches_the_exact_hook() -> Result<(), IrError> {
    let m = module_new!("folder-hook-exact")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let calls: CallLog = CallLog::default();
    let b = IrBuilder::with_folder(
        &m,
        RecordingFolder {
            calls: Rc::clone(&calls),
        },
    )
    .position_at_end(entry);
    let x = IntValue::<i32, _>::try_from(m.view(f).param(0)?)?;
    let y = IntValue::<i32, _>::try_from(m.view(f).param(1)?)?;

    b.int_udiv_with_flags::<i32, _, _, _>(x, y, UdivFlags::new().exact(), "q")?;
    b.int_udiv_with_flags::<i32, _, _, _>(x, y, UdivFlags::new(), "q2")?;
    b.int_ashr_with_flags::<i32, _, _, _>(x, y, AshrFlags::new().exact(), "s")?;

    assert_eq!(
        *calls.borrow(),
        vec![
            (BinaryOpcode::Udiv, Hook::Exact { exact: true }),
            (BinaryOpcode::Udiv, Hook::Exact { exact: false }),
            (BinaryOpcode::Ashr, Hook::Exact { exact: true }),
        ]
    );
    Ok(())
}

/// The erased entry point — the one the `.ll` parser uses — makes the same
/// opcode-keyed choice and passes on the flags it was handed. It used to call
/// `fold_bin_op_dyn` unconditionally, so `nuw` / `nsw` / `exact` never reached
/// a folder through this path even when they were set.
#[test]
fn erased_emitter_dispatches_on_opcode_and_forwards_flags() -> Result<(), IrError> {
    let m = module_new!("folder-hook-erased")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let calls: CallLog = CallLog::default();
    let b = IrBuilder::with_folder(
        &m,
        RecordingFolder {
            calls: Rc::clone(&calls),
        },
    )
    .position_at_end(entry);
    let x = m.view(f).param(0)?.as_erased();
    let y = m.view(f).param(1)?.as_erased();

    b.int_binop_erased(
        BinaryOpcode::Add,
        x,
        y,
        IntBinOpFlags::new().nuw().nsw(),
        "add",
    )?;
    b.int_binop_erased(BinaryOpcode::Add, x, y, IntBinOpFlags::new(), "add2")?;
    b.int_binop_erased(
        BinaryOpcode::Lshr,
        x,
        y,
        IntBinOpFlags::new().exact(),
        "shr",
    )?;
    b.int_binop_erased(
        BinaryOpcode::Or,
        x,
        y,
        IntBinOpFlags::new().disjoint(),
        "or",
    )?;

    assert_eq!(
        *calls.borrow(),
        vec![
            (
                BinaryOpcode::Add,
                Hook::NoWrap {
                    nuw: true,
                    nsw: true
                }
            ),
            (
                BinaryOpcode::Add,
                Hook::NoWrap {
                    nuw: false,
                    nsw: false
                }
            ),
            (BinaryOpcode::Lshr, Hook::Exact { exact: true }),
            (BinaryOpcode::Or, Hook::Plain),
        ]
    );
    Ok(())
}

/// A flag the opcode does not accept is dropped before the hook sees it —
/// `IntBinOpFlags` is filtered through `BinaryOpcode::accepted_flags`, which is
/// what keeps `exact` off an `add` and `nuw` off an `lshr`. Upstream has no
/// equivalent path (its emitters are per-opcode, so the flag is unspellable),
/// which is why this arm has no upstream counterpart either.
#[test]
fn erased_emitter_drops_flags_the_opcode_does_not_accept() -> Result<(), IrError> {
    let m = module_new!("folder-hook-erased-drop")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let calls: CallLog = CallLog::default();
    let b = IrBuilder::with_folder(
        &m,
        RecordingFolder {
            calls: Rc::clone(&calls),
        },
    )
    .position_at_end(entry);
    let x = m.view(f).param(0)?.as_erased();
    let y = m.view(f).param(1)?.as_erased();

    b.int_binop_erased(BinaryOpcode::Add, x, y, IntBinOpFlags::new().exact(), "add")?;
    b.int_binop_erased(BinaryOpcode::Lshr, x, y, IntBinOpFlags::new().nuw(), "shr")?;

    assert_eq!(
        *calls.borrow(),
        vec![
            (
                BinaryOpcode::Add,
                Hook::NoWrap {
                    nuw: false,
                    nsw: false
                }
            ),
            (BinaryOpcode::Lshr, Hook::Exact { exact: false }),
        ]
    );
    Ok(())
}
