//! Per-opcode instruction handles. Mirrors a slice of
//! `llvm/include/llvm/IR/Instructions.h`.
//!
//! Each handle is a thin view onto an attached instruction in some basic
//! block. Internally it stores the `(ValueSlot, ModuleRef, TypeSlot)` triple ---
//! the same shape `Value` uses --- so it does not depend on
//! [`Instruction`](crate::Instruction)'s `!Copy` lifecycle handle. Copyable handles expose
//! [`InstructionView`] for read-only rediscovery;
//! lifecycle mutation requires a builder-produced instruction or
//! [`BlockCursor`](crate::iter::BlockCursor).
//!
//! ## Why arithmetic/memory handles carry no type parameters
//!
//! `CallInst<R>` / `PhiInst<W, P>` carry markers because the builder
//! returns them typed and the marker gates real accessors. `AddInst`,
//! `LoadInst`, and the other per-opcode handles do not: the typed
//! information already lives on the value handles the builder returns
//! (D4 — `int_add::<W>` returns `IntValue<W>`), and the handles'
//! reachable constructors are rediscovery paths (`BlockCursor`,
//! `InstructionView`, `TryFrom`) which are inherently dyn-shaped — a
//! marker there would instantiate as `AddInst<IntDyn>` everywhere and
//! gate nothing.

use crate::Branded;
use core::fmt;
use core::iter::FusedIterator;

use super::align::Align;
use super::atomic_ordering::AtomicOrdering;
use super::atomicrmw_binop::AtomicRmwBinOp;
use super::basic_block::{BasicBlockLabel, IntoBasicBlockLabel, require_no_block_parameters};
use super::calling_conv::CallingConv;
use super::cmp_predicate::{CmpPredicate, FloatPredicate, IntPredicate};
use super::derived_types::FunctionType;
use super::float_kind::FloatKind;
use super::{IrError, IrResult};
// Only the crate-internal raw-phi authoring surface lifts through these, and
// that surface is `#[cfg(test)]` — block arguments are the public way to
// author a phi. See `docs/design/phi-type-guarantees-design.md`, slice 7.
#[cfg(test)]
use super::float_kind::IntoFloatValue;
use super::float_kind::{Bfloat, FloatDyn, Fp128, Half, PpcFp128, X86Fp80};
use super::fmf::FastMathFlags;
use super::function::FunctionValue;
use super::function_signature::{FunctionReturn, token::ValidatedCallResult};
use super::gep_no_wrap_flags::GepNoWrapFlags;
use super::instr_types::ShuffleMaskElem;
use super::instr_types::TailCallKind;
use super::instr_types::{
    AllocaInstData, AtomicCmpXchgInstData, AtomicRmwInstData, CallBrInstData, CallInstData,
    CatchPadInstData, CatchReturnInstData, CatchSwitchInstData, CleanupPadInstData,
    CleanupReturnInstData, ExtractElementInstData, ExtractValueInstData, FcmpInstData,
    FenceInstData, FnegInstData, FreezeInstData, GepInstData, IndirectBrInstData,
    InsertElementInstData, InsertValueInstData, InvokeInstData, LandingPadInstData, LoadInstData,
    ResumeInstData, SelectInstData, ShuffleVectorInstData, StoreInstData, SwitchInstData,
    VaArgInstData,
};
use super::instr_types::{
    BinaryOpData, BinaryOpcode, BranchInstData, BranchKind, CastOpData, CastOpcode, CmpInstData,
    LandingPadClauseKind, PhiData, ReturnOpData,
};
use super::instruction::{InstructionKindData, InstructionView};
use super::int_width::{IntDyn, IntWidth, IntoIntValue, StaticIntWidth};
use super::marker::{Dyn, Ptr, ReturnMarker};
use super::module::{Module, ModuleBrand, ModuleRef, Unverified};
use super::sync_scope::SyncScope;
use super::term_open_state::{Closed as TermClosed, Open as TermOpen, TermOpenState};
use super::r#type::{Type, TypeData, TypeSlot};
#[cfg(test)]
use super::value::IntoPointerValue;
use super::value::{
    FloatValue, IntValue, IsValue, PointerValue, Value, ValueKindData, ValueSlot, ValueUse,
};
use super::value_id::{
    AtomicCmpXchgInstId, AtomicRmwInstId, BlockId, CallInstId, FpPhiInstId, FreezeInstId,
    OtherPhiInstId, PhiInstId, PointerPhiInstId, TypedCallInstId, VaArgInstId,
};

macro_rules! decl_binop_handle {
    (
        $(#[$attr:meta])*
        $name:ident,
        $variant:ident
    ) => {
        $(#[$attr])*
        #[derive(Branded)]
        pub struct $name<'ctx, B: ModuleBrand> {
            pub(super) id: ValueSlot,
            pub(super) module: ModuleRef<'ctx, B>,
            pub(super) ty: TypeSlot,
        }

        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            #[inline]
            pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
            where
                M: Into<ModuleRef<'ctx, B>>,
            {
                Self { id, module: module.into(), ty }
            }

            fn payload(self) -> &'ctx BinaryOpData {
                let module = self.module.module();
                match &module.context().value_data(self.id).kind {
                    ValueKindData::Instruction(i) => match &i.kind {
                        InstructionKindData::$variant(b) => b,
                        _ => unreachable!(
                            concat!(stringify!($name), " invariant: kind is ", stringify!($variant))
                        ),
                    },
                    _ => unreachable!(
                        concat!(stringify!($name), " invariant: kind is Instruction")
                    ),
                }
            }

            /// Read-only erased instruction view for this opcode handle.
            #[inline]
            pub fn as_view(&self) -> InstructionView<'ctx, B> {
                InstructionView::from_parts(self.id, self.module)
            }

            /// Widen to the erased [`Value`] handle.
            ///
            /// Borrows rather than consumes.
            #[inline]
            pub fn to_erased(&self) -> Value<'ctx, B> {
                Value::from_parts(self.id, self.module, self.ty)
            }

            /// Left-hand side operand. Mirrors `getOperand(0)`.
            pub fn lhs(self) -> Value<'ctx, B> {
                let id = self.payload().lhs.get();
                let module = self.module.module();
                let data = module.context().value_data(id);
                Value::from_parts(id, self.module, data.ty)
            }

            /// Right-hand side operand. Mirrors `getOperand(1)`.
            pub fn rhs(self) -> Value<'ctx, B> {
                let id = self.payload().rhs.get();
                let module = self.module.module();
                let data = module.context().value_data(id);
                Value::from_parts(id, self.module, data.ty)
            }

            /// `nuw` flag.
            #[inline]
            pub fn has_no_unsigned_wrap(self) -> bool { self.payload().no_unsigned_wrap }

            /// `nsw` flag.
            #[inline]
            pub fn has_no_signed_wrap(self) -> bool { self.payload().no_signed_wrap }

            /// `exact` flag.
            #[inline]
            pub fn is_exact(self) -> bool { self.payload().is_exact }
        }
    };
}

decl_binop_handle!(
    /// `add` binary operator.
    AddInst, Add
);
decl_binop_handle!(
    /// `sub` binary operator.
    SubInst, Sub
);
decl_binop_handle!(
    /// `mul` binary operator.
    MulInst, Mul
);
decl_binop_handle!(
    /// `udiv` integer divide (unsigned).
    UdivInst, Udiv
);
decl_binop_handle!(
    /// `sdiv` integer divide (signed).
    SdivInst, Sdiv
);
decl_binop_handle!(
    /// `urem` integer remainder (unsigned).
    UremInst, Urem
);
decl_binop_handle!(
    /// `srem` integer remainder (signed).
    SremInst, Srem
);
decl_binop_handle!(
    /// `shl` logical left shift.
    ShlInst, Shl
);
decl_binop_handle!(
    /// `lshr` logical right shift.
    LshrInst, Lshr
);
decl_binop_handle!(
    /// `ashr` arithmetic right shift.
    AshrInst, Ashr
);
decl_binop_handle!(
    /// `and` bitwise and.
    AndInst, And
);
decl_binop_handle!(
    /// `or` bitwise or.
    OrInst, Or
);
decl_binop_handle!(
    /// `xor` bitwise xor.
    XorInst, Xor
);
decl_binop_handle!(
    /// `fadd` floating-point add.
    FaddInst, Fadd
);
decl_binop_handle!(
    /// `fsub` floating-point subtract.
    FsubInst, Fsub
);
decl_binop_handle!(
    /// `fmul` floating-point multiply.
    FmulInst, Fmul
);
decl_binop_handle!(
    /// `fdiv` floating-point divide.
    FdivInst, Fdiv
);
decl_binop_handle!(
    /// `frem` floating-point remainder.
    FremInst, Frem
);

/// Grouped view over any binary operator (`add`..`frem`). Lets a pass read
/// `lhs`/`rhs`/`opcode`/flags without matching all eighteen opcodes —
/// mirrors matching LLVM's `BinaryOperator` base then reading
/// `getOpcode()`. Obtain one via
/// [`InstructionKind::as_binary_op`](crate::InstructionKind::as_binary_op).
#[derive(Branded)]
#[branded(Debug, Clone, Copy)]
pub struct BinaryOp<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    pub(super) opcode: BinaryOpcode,
}

impl<'ctx, B: ModuleBrand + 'ctx> BinaryOp<'ctx, B> {
    pub(super) fn from_value(v: Value<'ctx, B>, opcode: BinaryOpcode) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            opcode,
        }
    }

    fn payload(&self) -> &'ctx BinaryOpData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Add(b)
                | InstructionKindData::Sub(b)
                | InstructionKindData::Mul(b)
                | InstructionKindData::Udiv(b)
                | InstructionKindData::Sdiv(b)
                | InstructionKindData::Urem(b)
                | InstructionKindData::Srem(b)
                | InstructionKindData::Shl(b)
                | InstructionKindData::Lshr(b)
                | InstructionKindData::Ashr(b)
                | InstructionKindData::And(b)
                | InstructionKindData::Or(b)
                | InstructionKindData::Xor(b)
                | InstructionKindData::Fadd(b)
                | InstructionKindData::Fsub(b)
                | InstructionKindData::Fmul(b)
                | InstructionKindData::Fdiv(b)
                | InstructionKindData::Frem(b) => b,
                _ => unreachable!("BinaryOp invariant: kind is a binary operator"),
            },
            _ => unreachable!("BinaryOp invariant: kind is Instruction"),
        }
    }

    /// The binary opcode.
    #[inline]
    pub const fn opcode(self) -> BinaryOpcode {
        self.opcode
    }
    /// Left-hand operand. Mirrors `getOperand(0)`.
    pub fn lhs(self) -> Value<'ctx, B> {
        let id = self.payload().lhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// Right-hand operand. Mirrors `getOperand(1)`.
    pub fn rhs(self) -> Value<'ctx, B> {
        let id = self.payload().rhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// `nuw` flag (meaningful only for the overflowing operators).
    #[inline]
    pub fn has_no_unsigned_wrap(self) -> bool {
        self.payload().no_unsigned_wrap
    }
    /// `nsw` flag (meaningful only for the overflowing operators).
    #[inline]
    pub fn has_no_signed_wrap(self) -> bool {
        self.payload().no_signed_wrap
    }
    /// `exact` flag (meaningful only for the exact-capable operators).
    #[inline]
    pub fn is_exact(self) -> bool {
        self.payload().is_exact
    }
    /// Whether operands may be swapped without changing the result.
    #[inline]
    pub fn is_commutative(self) -> bool {
        self.opcode.is_commutative()
    }
    /// Read-only erased instruction view.
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }
    /// Widen to the erased [`Value`] handle (the result).
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
}

/// Grouped view over `icmp`/`fcmp`. Lets a pass read `lhs`/`rhs` and a
/// unified [`CmpPredicate`] without caring which comparison it is —
/// mirrors matching LLVM's `CmpInst` base then reading `getPredicate()`.
/// Obtain one via
/// [`InstructionKind::as_cmp`](crate::InstructionKind::as_cmp).
#[derive(Branded)]
#[branded(Debug, Clone, Copy)]
pub struct Cmp<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

impl<'ctx, B: ModuleBrand + 'ctx> Cmp<'ctx, B> {
    pub(super) fn from_value(v: Value<'ctx, B>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
        }
    }

    /// The comparison predicate, tagged integer or float.
    pub fn predicate(self) -> CmpPredicate {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Icmp(c) => CmpPredicate::Int(c.predicate),
                InstructionKindData::Fcmp(c) => CmpPredicate::Float(c.predicate),
                _ => unreachable!("Cmp invariant: kind is Icmp or Fcmp"),
            },
            _ => unreachable!("Cmp invariant: kind is Instruction"),
        }
    }
    /// `true` for `icmp`, `false` for `fcmp`.
    pub fn is_integer(self) -> bool {
        matches!(self.predicate(), CmpPredicate::Int(_))
    }
    /// Left-hand operand.
    pub fn lhs(self) -> Value<'ctx, B> {
        self.operand(true)
    }
    /// Right-hand operand.
    pub fn rhs(self) -> Value<'ctx, B> {
        self.operand(false)
    }
    fn operand(self, left: bool) -> Value<'ctx, B> {
        let module = self.module.module();
        let id = match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Icmp(c) => {
                    if left {
                        c.lhs.get()
                    } else {
                        c.rhs.get()
                    }
                }
                InstructionKindData::Fcmp(c) => {
                    if left {
                        c.lhs.get()
                    } else {
                        c.rhs.get()
                    }
                }
                _ => unreachable!("Cmp invariant: kind is Icmp or Fcmp"),
            },
            _ => unreachable!("Cmp invariant: kind is Instruction"),
        };
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// Read-only erased instruction view.
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }
    /// Widen to the erased [`Value`] handle (the `i1`/vector result).
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
}

/// Common scaffolding used by every non-macro handle.
macro_rules! decl_handle_scaffold {
    ($name:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            #[inline]
            pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
            where
                M: Into<ModuleRef<'ctx, B>>,
            {
                Self {
                    id,
                    module: module.into(),
                    ty,
                }
            }

            /// Read-only erased instruction view for this opcode handle.
            #[inline]
            pub fn as_view(&self) -> InstructionView<'ctx, B> {
                InstructionView::from_parts(self.id, self.module)
            }

            /// Widen to the erased [`Value`] handle.
            ///
            /// Borrows rather than consumes.
            #[inline]
            pub fn to_erased(&self) -> Value<'ctx, B> {
                Value::from_parts(self.id, self.module, self.ty)
            }
        }
    };
}

/// Give a marker-free opcode handle the two id accessors every handle in the
/// crate shares (0.0.4): `.id()` mints the storable, module-tagged
/// instruction id its builder hands back — so a handle recovered through
/// [`Module::view`](crate::Module::view) or an [`InstructionKind`] match can go
/// back into a side table — and `.slot()` is the bare arena index.
///
/// Only opcodes with a dedicated id type appear here; the rest reach an id
/// through `to_erased().id()`, which is the honest spelling for a handle whose
/// only storable identity is its result value.
macro_rules! decl_instruction_id_accessors {
    ($( $name:ident => $id:ident ),+ $(,)?) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            #[doc = concat!(
                "Storable, module-tagged [`", stringify!($id), "`](crate::",
                stringify!($id), ") for this instruction."
            )]
            #[inline]
            pub fn id(&self) -> $id<B> {
                $id::from_raw(self.module.id(), self.id)
            }

            /// Bare arena slot of this instruction. Untagged: prefer
            /// [`id`](Self::id).
            #[inline]
            pub fn slot(&self) -> ValueSlot {
                self.id
            }
        }
    )+ };
}

decl_instruction_id_accessors!(
    FreezeInst => FreezeInstId,
    VaArgInst => VaArgInstId,
    AtomicRmwInst => AtomicRmwInstId,
    AtomicCmpXchgInst => AtomicCmpXchgInstId,
);

/// `alloca` stack-slot allocation. Mirrors `AllocaInst`
/// (`Instructions.h`).
#[derive(Branded)]
pub struct AllocaInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(AllocaInst);

impl<'ctx, B: ModuleBrand + 'ctx> AllocaInst<'ctx, B> {
    fn payload(self) -> &'ctx AllocaInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Alloca(a) => a,
                _ => unreachable!("AllocaInst invariant: kind is Alloca"),
            },
            _ => unreachable!("AllocaInst invariant: kind is Instruction"),
        }
    }
    /// Allocated element type.
    pub fn allocated_type(self) -> Type<'ctx, B> {
        Type::new(self.payload().allocated_ty, self.module)
    }
    /// Optional element-count operand (`alloca i32, i32 %n`).
    pub fn array_size(self) -> Option<Value<'ctx, B>> {
        let id = self.payload().num_elements.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, self.module, data.ty))
    }
    /// Explicit alignment, if any.
    pub fn align(self) -> Option<Align> {
        self.payload().align.align()
    }
    /// Address space of the result pointer.
    pub fn addr_space(self) -> u32 {
        self.payload().addr_space
    }
    /// `true` when this allocation carries the `inalloca` marker. Mirrors
    /// `AllocaInst::isUsedWithInAlloca` in `Instructions.h`.
    pub fn is_inalloca(self) -> bool {
        self.payload().flags.is_inalloca()
    }
    /// `true` when this allocation carries the `swifterror` marker. Mirrors
    /// `AllocaInst::isSwiftError` in `Instructions.h`.
    pub fn is_swifterror(self) -> bool {
        self.payload().flags.is_swifterror()
    }
}

/// `load` instruction. Mirrors `LoadInst` (`Instructions.h`).
#[derive(Branded)]
pub struct LoadInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(LoadInst);

impl<'ctx, B: ModuleBrand + 'ctx> LoadInst<'ctx, B> {
    fn payload(self) -> &'ctx LoadInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Load(l) => l,
                _ => unreachable!("LoadInst invariant: kind is Load"),
            },
            _ => unreachable!("LoadInst invariant: kind is Instruction"),
        }
    }
    /// The loaded type (the instruction's result type).
    #[inline]
    pub fn loaded_ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
    /// Pointer operand. Statically a pointer for this opcode, so returned
    /// as [`PointerValue`] rather than the erased [`Value`].
    pub fn pointer(self) -> PointerValue<'ctx, B> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        PointerValue::from_value_unchecked(Value::from_parts(id, self.module, data.ty))
    }
    pub fn align(self) -> Option<Align> {
        self.payload().align.align()
    }
    pub fn is_volatile(self) -> bool {
        self.payload().volatile
    }
    /// Atomic-ordering on this load. Mirrors `LoadInst::getOrdering`
    /// in `Instructions.h`. Returns `NotAtomic` for ordinary non-atomic loads.
    pub fn ordering(self) -> AtomicOrdering {
        self.payload().ordering
    }
    /// Synchronization scope on this load. Mirrors
    /// `LoadInst::getSyncScopeID` in `Instructions.h`.
    pub fn sync_scope(self) -> SyncScope {
        self.payload().sync_scope.clone()
    }
    /// `true` when this load carries a non-`NotAtomic` ordering. Mirrors
    /// `LoadInst::isAtomic` in `Instructions.h`.
    pub fn is_atomic(self) -> bool {
        self.payload().is_atomic()
    }
    /// `true` when this load has no memory-ordering side effects
    /// (non-volatile and non-atomic or `unordered`). Mirrors
    /// `LoadInst::isUnordered` in `Instructions.h`.
    pub fn is_unordered(self) -> bool {
        self.payload().is_unordered()
    }
}

/// `store` instruction. Mirrors `StoreInst` (`Instructions.h`).
#[derive(Branded)]
pub struct StoreInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(StoreInst);

impl<'ctx, B: ModuleBrand + 'ctx> StoreInst<'ctx, B> {
    fn payload(self) -> &'ctx StoreInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Store(s) => s,
                _ => unreachable!("StoreInst invariant: kind is Store"),
            },
            _ => unreachable!("StoreInst invariant: kind is Instruction"),
        }
    }
    pub fn value_operand(self) -> Value<'ctx, B> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// Pointer operand. Statically a pointer for this opcode, so returned
    /// as [`PointerValue`] rather than the erased [`Value`].
    pub fn pointer(self) -> PointerValue<'ctx, B> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        PointerValue::from_value_unchecked(Value::from_parts(id, self.module, data.ty))
    }
    pub fn align(self) -> Option<Align> {
        self.payload().align.align()
    }
    pub fn is_volatile(self) -> bool {
        self.payload().volatile
    }
    /// Atomic-ordering on this store. Mirrors `StoreInst::getOrdering`
    /// in `Instructions.h`. Returns `NotAtomic` for ordinary non-atomic stores.
    pub fn ordering(self) -> AtomicOrdering {
        self.payload().ordering
    }
    /// Synchronization scope on this store. Mirrors
    /// `StoreInst::getSyncScopeID` in `Instructions.h`.
    pub fn sync_scope(self) -> SyncScope {
        self.payload().sync_scope.clone()
    }
    /// `true` when this store carries a non-`NotAtomic` ordering. Mirrors
    /// `StoreInst::isAtomic` in `Instructions.h`.
    pub fn is_atomic(self) -> bool {
        self.payload().is_atomic()
    }
}

/// `getelementptr` instruction. Mirrors `GetElementPtrInst`
/// (`Instructions.h`).
#[derive(Branded)]
pub struct GepInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(GepInst);

impl<'ctx, B: ModuleBrand + 'ctx> GepInst<'ctx, B> {
    fn payload(self) -> &'ctx GepInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Gep(g) => g,
                _ => unreachable!("GepInst invariant: kind is GEP"),
            },
            _ => unreachable!("GepInst invariant: kind is Instruction"),
        }
    }
    /// Source-element type (the second operand of `getelementptr`).
    pub fn source_element_type(self) -> Type<'ctx, B> {
        Type::new(self.payload().source_ty, self.module)
    }
    /// Pointer operand. Mirrors `GetElementPtrInst::getPointerOperand`, which
    /// returns a bare `Value *`: a GEP base is a `ptr` **or** a `<N x ptr>`
    /// (`getGEPReturnType`'s vector arm, [`crate::IrBuilder::gep_erased`]), so
    /// this is erased. Narrowing it to [`PointerValue`] would forge a pointer
    /// claim over a vector -- `PointerValue::from_value_unchecked` checks
    /// nothing, and neither does the `PointerType` that handle's `ty()` hands
    /// back, so the mislabelling stays silent until `PointerType::address_space`
    /// panics on it.
    pub fn pointer(self) -> Value<'ctx, B> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn indices(
        self,
    ) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let module = self.module.module();
        let ids: Vec<ValueSlot> = self.payload().indices.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
    pub fn flags(self) -> GepNoWrapFlags {
        self.payload().flags
    }
}

/// The called operand of a call, split into the direct/indirect cases.
/// Returned by [`CallInst::classify_callee`].
#[derive(Branded)]
pub enum Callee<'ctx, B: ModuleBrand> {
    /// A direct call to a known function global.
    Direct(FunctionValue<'ctx, Dyn, B>),
    /// An indirect call through a function pointer.
    Indirect(PointerValue<'ctx, B>),
}

/// `call` instruction. Mirrors `CallInst` (`Instructions.h`).
///
/// The `R: ReturnMarker` parameter (default [`crate::Dyn`]) propagates
/// the callee's return shape, so a typed [`crate::IrBuilder::call_dyn`] for an `i32`
/// callee returns `CallInst<'ctx, i32>` and exposes a typed
/// `return_int_value()` accessor without a runtime
/// [`crate::IrError::TypeMismatch`].
#[derive(Branded)]
#[branded(Debug)]
pub struct CallInst<'ctx, R: ReturnMarker, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _r: core::marker::PhantomData<R>,
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> Clone for CallInst<'ctx, R, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Copy for CallInst<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> PartialEq for CallInst<'ctx, R, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Eq for CallInst<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> core::hash::Hash for CallInst<'ctx, R, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> CallInst<'ctx, R, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _r: core::marker::PhantomData,
        }
    }

    /// Read-only erased instruction view for this call.
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// Bare arena slot of this call. Untagged: prefer [`id`](Self::id).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.id
    }

    /// Storable, module-tagged [`CallInstId<R>`](crate::CallInstId) for this
    /// call — the id its builder handed back, re-mintable from a handle
    /// recovered through [`Module::view`](crate::Module::view) or an
    /// [`InstructionKind`](crate::InstructionKind) match, so a rediscovered call
    /// can go back into a
    /// side table.
    #[inline]
    pub fn id(&self) -> CallInstId<R, B> {
        CallInstId::from_raw(self.module.id(), self.id)
    }

    /// Re-tag the return marker. Crate-internal: only [`call_dyn`]
    /// flows the typed marker; [`as_dyn`] erases it.
    #[inline]
    pub(super) fn retag<R2: ReturnMarker>(self) -> CallInst<'ctx, R2, B> {
        CallInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: core::marker::PhantomData,
        }
    }

    /// Erase the return marker. Useful for storage / printing helpers
    /// that don't want to be generic in `R`.
    #[inline]
    pub fn as_dyn(self) -> CallInst<'ctx, Dyn, B> {
        self.retag::<Dyn>()
    }

    fn payload(self) -> &'ctx CallInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Call(c) => c,
                _ => unreachable!("CallInst invariant: kind is Call"),
            },
            _ => unreachable!("CallInst invariant: kind is Instruction"),
        }
    }
    /// The called operand, erased to [`Value`] (a function global for a
    /// direct call, a function pointer for an indirect one). Use
    /// [`Self::classify_callee`] to recover which.
    pub fn callee(self) -> Value<'ctx, B> {
        let id = self.payload().callee.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }

    /// Split the callee into a direct call to a known [`FunctionValue`] or
    /// an indirect call through a [`PointerValue`]. Mirrors the common
    /// `CallBase::getCalledFunction()` "is this direct?" question, but the
    /// answer is a typed enum instead of a nullable pointer.
    pub fn classify_callee(self) -> Callee<'ctx, B> {
        let callee = self.callee();
        match FunctionValue::try_from(callee) {
            Ok(function) => Callee::Direct(function),
            Err(_) => Callee::Indirect(PointerValue::from_value_unchecked(callee)),
        }
    }
    /// Function-type of the call (`FunctionType<'ctx, B>`).
    pub fn function_type(self) -> FunctionType<'ctx, B> {
        FunctionType::new(self.payload().fn_ty, self.module)
    }
    pub fn args(
        self,
    ) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let module = self.module.module();
        let ids: Vec<ValueSlot> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
    pub fn calling_conv(self) -> CallingConv {
        self.payload().calling_conv
    }
    pub fn tail_call_kind(self) -> TailCallKind {
        self.payload().tail_kind
    }
    /// Return value, or `None` for a void-returning callee. Available
    /// on every `R`; the typed `return_int_value` /
    /// `return_float_value` / `return_pointer_value` accessors below
    /// are gated to the corresponding marker so a typed callee skips
    /// the runtime narrowing.
    pub fn return_value(self) -> Option<Value<'ctx, B>> {
        let module = self.module.module();
        let ret_ty_data = module.context().type_data(self.ty);
        if matches!(ret_ty_data, TypeData::Void) {
            None
        } else {
            Some(Value::from_parts(self.id, self.module, self.ty))
        }
    }
}

// Typed-return accessors. Each impl is gated on the concrete return
// marker so a `CallInst<'ctx, i32>` exposes `return_int_value` but not
// `return_float_value`, and a `CallInst<'ctx, ()>` exposes neither.
macro_rules! call_inst_int_return {
    ($($w:ty),+ $(,)?) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> CallInst<'ctx, $w, B> {
            /// Typed result handle for an integer-returning call.
            #[inline]
            pub fn return_int_value(self) -> IntValue<'ctx, $w, B> {
                let v = Value::from_parts(self.id, self.module, self.ty);
                IntValue::<$w, B>::from_value_unchecked(v)
            }
        }
    )+ };
}
call_inst_int_return!(bool, i8, i16, i32, i64, i128, IntDyn);

macro_rules! call_inst_float_return {
    ($($k:ty),+ $(,)?) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> CallInst<'ctx, $k, B> {
            /// Typed result handle for a float-returning call.
            #[inline]
            pub fn return_float_value(self) -> FloatValue<'ctx, $k, B> {
                let v = Value::from_parts(self.id, self.module, self.ty);
                FloatValue::<$k, B>::from_value_unchecked(v)
            }
        }
    )+ };
}
call_inst_float_return!(f32, f64, Half, Bfloat, Fp128, X86Fp80, PpcFp128, FloatDyn,);

impl<'ctx, B: ModuleBrand + 'ctx> CallInst<'ctx, Ptr, B> {
    /// Typed result handle for a pointer-returning call.
    #[inline]
    pub fn return_pointer_value(self) -> PointerValue<'ctx, B> {
        PointerValue::from_value_unchecked(Value::from_parts(self.id, self.module, self.ty))
    }
}

/// Call handle whose full return schema is carried at the type level.
/// The marker on the inner [`CallInst`] is `Ret::Marker` — derived from
/// the callee by [`crate::IrBuilder::call`], never caller-asserted.
pub struct TypedCallInst<'ctx, Ret, B: ModuleBrand>
where
    Ret: FunctionReturn,
{
    inner: CallInst<'ctx, Ret::Marker, B>,
    _ret: core::marker::PhantomData<Ret>,
}

impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> Clone for TypedCallInst<'ctx, Ret, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> Copy for TypedCallInst<'ctx, Ret, B> {}
impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> PartialEq for TypedCallInst<'ctx, Ret, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> Eq for TypedCallInst<'ctx, Ret, B> {}
impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> core::hash::Hash for TypedCallInst<'ctx, Ret, B> {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}
impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> fmt::Debug for TypedCallInst<'ctx, Ret, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedCallInst")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<'ctx, Ret: FunctionReturn, B: ModuleBrand + 'ctx> TypedCallInst<'ctx, Ret, B> {
    /// Crate-internal: wrap a raw [`CallInst`] already known to have
    /// been emitted against a validated [`crate::TypedFunctionValue`]
    /// callee. Only the typed `call` family constructs this —
    /// the schema-carrying guarantee comes from the callee facade's
    /// own construction-time validation, not from anything checked
    /// here.
    #[inline]
    pub(super) fn from_call(inner: CallInst<'ctx, Ret::Marker, B>) -> Self {
        Self {
            inner,
            _ret: core::marker::PhantomData,
        }
    }

    /// Typed result. Infallible: the schema was validated when the
    /// typed callee facade was constructed. `()` for a void callee.
    #[inline]
    pub fn result(self) -> Ret::CallResult<'ctx, B> {
        let validated = ValidatedCallResult::new();
        let value = Value::from_parts(self.inner.id, self.inner.module, self.inner.ty);
        Ret::call_result_from_value(value, &validated)
    }

    /// Marker-typed handle (keeps `Ret::Marker`, drops the schema).
    #[inline]
    pub fn as_call_inst(self) -> CallInst<'ctx, Ret::Marker, B> {
        self.inner
    }

    /// Bare arena slot of this call. Untagged: prefer [`id`](Self::id).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.inner.slot()
    }

    /// Storable, module-tagged [`TypedCallInstId<Ret>`](crate::TypedCallInstId)
    /// for this call — the schema rides on the id, so viewing it recovers the
    /// infallible [`result`](Self::result) without a re-narrowing step.
    #[inline]
    pub fn id(&self) -> TypedCallInstId<Ret, B> {
        TypedCallInstId::from_raw(self.inner.module.id(), self.inner.id)
    }

    /// Fully-erased handle (D3).
    #[inline]
    pub fn as_dyn(self) -> CallInst<'ctx, Dyn, B> {
        self.inner.as_dyn()
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B> {
        self.inner.to_erased()
    }
}

/// `select` instruction. Mirrors `SelectInst` (`Instructions.h`).
#[derive(Branded)]
pub struct SelectInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(SelectInst);

impl<'ctx, B: ModuleBrand + 'ctx> SelectInst<'ctx, B> {
    fn payload(self) -> &'ctx SelectInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Select(s) => s,
                _ => unreachable!("SelectInst invariant: kind is Select"),
            },
            _ => unreachable!("SelectInst invariant: kind is Instruction"),
        }
    }
    /// Fast-math flags. Mirrors `FPMathOperator::getFastMathFlags`, which a
    /// `select` answers when its arms are floating-point.
    pub fn fast_math_flags(self) -> FastMathFlags {
        self.payload().fmf.get()
    }
    pub fn condition(self) -> Value<'ctx, B> {
        let id = self.payload().cond.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn true_value(self) -> Value<'ctx, B> {
        let id = self.payload().true_val.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn false_value(self) -> Value<'ctx, B> {
        let id = self.payload().false_val.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
}

/// `ret` terminator instruction. Mirrors `ReturnInst` in
/// `Instructions.h`.
#[derive(Branded)]
pub struct RetInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(RetInst);

impl<'ctx, B: ModuleBrand + 'ctx> RetInst<'ctx, B> {
    fn payload(self) -> &'ctx ReturnOpData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Ret(r) => r,
                _ => unreachable!("RetInst invariant: kind is Ret"),
            },
            _ => unreachable!("RetInst invariant: kind is Instruction"),
        }
    }
    /// Returned value. `None` for `ret void`.
    pub fn return_value(self) -> Option<Value<'ctx, B>> {
        let id = self.payload().value.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, self.module, data.ty))
    }
}

/// Cast instruction (`trunc`, `zext`, `sext`, `bitcast`, ...).
/// Per-opcode cast handles. Replaces the single erased `CastInst`: each of
/// LLVM's 14 cast opcodes gets its own handle so a `match` over
/// [`CastKind`](crate::CastKind) names the exact opcode (mirroring LLVM's
/// `TruncInst`/`ZExtInst`/... classes) instead of branching on a runtime
/// `CastOpcode`. Handles whose source operand is statically a pointer
/// (`ptrtoint`, `ptrtoaddr`, `addrspacecast`) return
/// [`PointerValue`] from `src()`; the rest return the erased [`Value`]
/// because their source category is not fixed by the IR grammar (e.g.
/// `bitcast`) or is not a pointer.
macro_rules! decl_cast_handle {
    (@struct $(#[$attr:meta])* $name:ident, $opcode:ident) => {
        $(#[$attr])*
        #[derive(Branded)]
        pub struct $name<'ctx, B: ModuleBrand> {
            pub(super) id: ValueSlot,
            pub(super) module: ModuleRef<'ctx, B>,
            pub(super) ty: TypeSlot,
        }

        decl_handle_scaffold!($name);

        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            fn payload(self) -> &'ctx CastOpData {
                let module = self.module.module();
                match &module.context().value_data(self.id).kind {
                    ValueKindData::Instruction(i) => match &i.kind {
                        InstructionKindData::Cast(c) => c,
                        _ => unreachable!(
                            concat!(stringify!($name), " invariant: kind is Cast")
                        ),
                    },
                    _ => unreachable!(
                        concat!(stringify!($name), " invariant: kind is Instruction")
                    ),
                }
            }

            /// The cast opcode this handle represents. Fixed by the type.
            #[inline]
            pub const fn opcode(self) -> CastOpcode {
                CastOpcode::$opcode
            }
        }
    };
    // Erased-source variant.
    ($(#[$attr:meta])* $name:ident, $opcode:ident) => {
        decl_cast_handle!(@struct $(#[$attr])* $name, $opcode);
        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            /// Source operand of the cast.
            pub fn src(self) -> Value<'ctx, B> {
                let id = self.payload().src.get();
                let module = self.module.module();
                let data = module.context().value_data(id);
                Value::from_parts(id, self.module, data.ty)
            }
        }
    };
    // Pointer-source variant (`src()` is statically a pointer).
    ($(#[$attr:meta])* $name:ident, $opcode:ident, ptr_src) => {
        decl_cast_handle!(@struct $(#[$attr])* $name, $opcode);
        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            /// Source operand of the cast. Statically a pointer for this
            /// opcode, so returned as [`PointerValue`] rather than the
            /// erased [`Value`].
            pub fn src(self) -> PointerValue<'ctx, B> {
                let id = self.payload().src.get();
                let module = self.module.module();
                let data = module.context().value_data(id);
                PointerValue::from_value_unchecked(Value::from_parts(id, self.module, data.ty))
            }
        }
    };
}

decl_cast_handle!(
    /// `trunc .. to ..` — narrow an integer.
    TruncInst, Trunc
);
decl_cast_handle!(
    /// `zext .. to ..` — zero-extend an integer.
    ZextInst, Zext
);
decl_cast_handle!(
    /// `sext .. to ..` — sign-extend an integer.
    SextInst, Sext
);
decl_cast_handle!(
    /// `fptrunc .. to ..` — narrow a float.
    FpTruncInst, FpTrunc
);
decl_cast_handle!(
    /// `fpext .. to ..` — widen a float.
    FpExtInst, FpExt
);
decl_cast_handle!(
    /// `fptoui .. to ..` — float to unsigned integer.
    FpToUiInst, FpToUi
);
decl_cast_handle!(
    /// `fptosi .. to ..` — float to signed integer.
    FpToSiInst, FpToSi
);
decl_cast_handle!(
    /// `uitofp .. to ..` — unsigned integer to float.
    UiToFpInst, UiToFp
);
decl_cast_handle!(
    /// `sitofp .. to ..` — signed integer to float.
    SiToFpInst, SiToFp
);
decl_cast_handle!(
    /// `ptrtoaddr .. to ..` — pointer to integer address bits.
    PtrToAddrInst, PtrToAddr, ptr_src
);
decl_cast_handle!(
    /// `ptrtoint .. to ..` — pointer to integer.
    PtrToIntInst, PtrToInt, ptr_src
);
decl_cast_handle!(
    /// `inttoptr .. to ..` — integer to pointer.
    IntToPtrInst, IntToPtr
);
decl_cast_handle!(
    /// `bitcast .. to ..` — same-size bit reinterpretation.
    BitCastInst, BitCast
);
decl_cast_handle!(
    /// `addrspacecast .. to ..` — address-space change on a pointer.
    AddrSpaceCastInst, AddrSpaceCast, ptr_src
);

// --------------------------------------------------------------------------
// Comparison instructions
// --------------------------------------------------------------------------

/// `icmp` integer comparison. Mirrors `IcmpInst` (`Instructions.h`).
#[derive(Branded)]
pub struct IcmpInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(IcmpInst);

impl<'ctx, B: ModuleBrand + 'ctx> IcmpInst<'ctx, B> {
    fn payload(self) -> &'ctx CmpInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Icmp(c) => c,
                _ => unreachable!("IcmpInst invariant: kind is Icmp"),
            },
            _ => unreachable!("IcmpInst invariant: kind is Instruction"),
        }
    }
    /// Integer predicate (`eq`, `slt`, `ult`, ...).
    #[inline]
    pub fn predicate(self) -> IntPredicate {
        self.payload().predicate
    }
    pub fn lhs(self) -> Value<'ctx, B> {
        let id = self.payload().lhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn rhs(self) -> Value<'ctx, B> {
        let id = self.payload().rhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
}

/// `fcmp` floating-point comparison. Mirrors `FcmpInst`
/// (`Instructions.h`).
#[derive(Branded)]
pub struct FcmpInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(FcmpInst);

impl<'ctx, B: ModuleBrand + 'ctx> FcmpInst<'ctx, B> {
    fn payload(self) -> &'ctx FcmpInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Fcmp(c) => c,
                _ => unreachable!("FcmpInst invariant: kind is Fcmp"),
            },
            _ => unreachable!("FcmpInst invariant: kind is Instruction"),
        }
    }
    /// Float predicate (`oeq`, `olt`, `une`, ...).
    #[inline]
    pub fn predicate(self) -> FloatPredicate {
        self.payload().predicate
    }
    pub fn lhs(self) -> Value<'ctx, B> {
        let id = self.payload().lhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn rhs(self) -> Value<'ctx, B> {
        let id = self.payload().rhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
}

// --------------------------------------------------------------------------
// Branch terminator
// --------------------------------------------------------------------------

/// `br` terminator. Mirrors `BranchInst` (`Instructions.h`).
#[derive(Branded)]
pub struct BranchInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(BranchInst);

impl<'ctx, B: ModuleBrand + 'ctx> BranchInst<'ctx, B> {
    fn payload(self) -> &'ctx BranchInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Br(b) => b,
                _ => unreachable!("BranchInst invariant: kind is Br"),
            },
            _ => unreachable!("BranchInst invariant: kind is Instruction"),
        }
    }
    pub fn is_conditional(self) -> bool {
        matches!(
            &*self.payload().kind.borrow(),
            BranchKind::Conditional { .. }
        )
    }
    pub fn condition(self) -> Option<Value<'ctx, B>> {
        match &*self.payload().kind.borrow() {
            BranchKind::Conditional { cond, .. } => {
                let module = self.module.module();
                let cid = cond.get();
                let data = module.context().value_data(cid);
                Some(Value::from_parts(cid, self.module, data.ty))
            }
            BranchKind::Unconditional(_) => None,
        }
    }
    /// Iterator over successor block-ids.
    pub(super) fn successor_ids(self) -> Vec<ValueSlot> {
        match &*self.payload().kind.borrow() {
            BranchKind::Unconditional(t) => vec![*t],
            BranchKind::Conditional {
                then_bb, else_bb, ..
            } => vec![*then_bb, *else_bb],
        }
    }
    /// Successors as copyable block labels.
    pub fn successors(
        self,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        self.successor_ids()
            .into_iter()
            .map(move |id| BlockId::<Dyn, B>::from_raw(self.module.id(), id))
    }
}

/// `unreachable` terminator. Mirrors `UnreachableInst`
/// (`Instructions.h`).
#[derive(Branded)]
pub struct UnreachableInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(UnreachableInst);

// --------------------------------------------------------------------------
// Phi
// --------------------------------------------------------------------------

/// Shared body of the four phi handles' `remove_incoming`, mirroring
/// `PHINode::removeIncomingValue(Idx, /*DeletePHIIfEmpty=*/false)`
/// (`lib/IR/Instructions.cpp`).
///
/// Upstream backfills the vacated slot from the **end** of the incoming list
/// rather than shifting, so incoming order is not preserved; that behaviour is
/// mirrored exactly so a rewriter that follows LLVM's algorithm prints the same
/// text. The removed value's reverse use-list loses exactly one
/// `Instruction(phi)` edge — one edge per incoming was registered by
/// `add_incoming` / `phi_add_incoming_from_value`, so a value that is incoming
/// on several edges keeps the rest.
///
/// The `DeletePHIIfEmpty` half of upstream's contract is deliberately **not**
/// mirrored: llvmkit erases through
/// [`Instruction::erase_from_parent`](crate::Instruction::erase_from_parent),
/// which consumes the linear lifecycle handle so that use-after-erase is a
/// compile error, and a `Copy` opcode handle cannot express that consumption —
/// self-erasure here would hand the caller a live handle to an erased
/// instruction. The auto-erase behaviour already ships where it *can* be
/// sound, inside the `ReshapeCfg` pass surface (`FnReshape`'s edge edits RAUW
/// an emptied phi with poison and erase it, LLVM `removePredecessor`). Emptying
/// a phi here is legal but leaves a node with no printable textual form; the
/// caller owns finishing the job, and
/// [`Module::verify`](crate::Module::verify) flags the leftover through the
/// existing incoming-count-vs-predecessor rule.
fn phi_remove_incoming<'ctx, B: ModuleBrand + 'ctx>(
    phi_id: ValueSlot,
    module: ModuleRef<'ctx, B>,
    payload: &PhiData,
    index: u32,
) -> IrResult<Value<'ctx, B>> {
    let slot = usize::try_from(index).unwrap_or_else(|_| unreachable!("u32 fits in usize"));
    let removed = {
        let mut incoming = payload.incoming.borrow_mut();
        let count = u32::try_from(incoming.len())
            .unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"));
        if slot >= incoming.len() {
            return Err(crate::IrError::ArgumentIndexOutOfRange { index, count });
        }
        // `swap_remove` is exactly upstream's "swap with the end, nuke the
        // last" — the vacated slot is backfilled from the tail.
        incoming.swap_remove(slot).0.get()
    };
    // Deregister one phi-use of the removed value.
    {
        let core = module.module();
        let mut uses = core.context().value_data(removed).use_list.borrow_mut();
        if let Some(pos) = uses
            .iter()
            .position(|edge| *edge == ValueUse::Instruction(phi_id))
        {
            uses.remove(pos);
        }
    }
    let ty = module.module().context().value_data(removed).ty;
    Ok(Value::from_parts(removed, module, ty))
}

/// `phi` node. Mirrors `PHINode` (`Instructions.h`). Mutable
/// `add_incoming` mirrors `PHINode::addIncoming`; the factorial
/// example needs it because the loop-edge incoming value is defined
/// later in the same block.
///
/// The handle is a copyable read/edit view, minted from the storable
/// [`PhiInstId<W>`](crate::PhiInstId) the phi builders hand back. Authoring
/// (`add_incoming`) is crate-internal — block arguments
/// ([`IrBuilder::append_block_with_params`](crate::IrBuilder::append_block_with_params))
/// are the public phi-authoring surface — while
/// [`remove_incoming`](Self::remove_incoming) is public for CFG rewriters and
/// takes an `Unverified` module token as its mutation-capability witness.
#[derive(Branded)]
#[branded(Debug)]
pub struct PhiInst<'ctx, W: IntWidth, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _w: core::marker::PhantomData<fn() -> W>,
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> PhiInst<'ctx, W, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _w: core::marker::PhantomData,
        }
    }

    fn payload(&self) -> &'ctx PhiData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Phi(p) => p,
                _ => unreachable!("PhiInst invariant: kind is Phi"),
            },
            _ => unreachable!("PhiInst invariant: kind is Instruction"),
        }
    }

    /// Fast-math flags. Mirrors `FPMathOperator::getFastMathFlags`, which a
    /// `phi` answers when its result type is floating-point.
    pub fn fast_math_flags(&self) -> FastMathFlags {
        self.payload().fmf.get()
    }

    /// Set the fast-math flags. Mirrors `Instruction::setFastMathFlags`,
    /// which `LLParser::parseInstruction` calls after `parsePHI` returns.
    ///
    /// A `phi` is an `FPMathOperator` only when its result type is
    /// floating-point, so non-empty flags on any other result type are
    /// refused — upstream reports that as
    /// `fast-math-flags specified for phi without floating-point scalar or
    /// vector return type`.
    pub fn set_fast_math_flags(&self, fmf: FastMathFlags) -> IrResult<()> {
        if !fmf.is_empty()
            && !crate::operator::is_supported_floating_point_type(self.as_view().ty())
        {
            return Err(IrError::InvalidOperation {
                message: "fast-math flags require a floating-point phi result",
            });
        }
        self.payload().fmf.set(fmf);
        Ok(())
    }

    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// Bare arena slot of the underlying value (same slot as
    /// [`to_erased`](Self::to_erased)). Untagged: prefer [`id`](Self::id),
    /// which carries the owning module and resolves back through
    /// [`Module::view`](crate::Module::view).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.to_erased().id
    }

    /// Storable, module-tagged [`PhiInstId<W>`](crate::PhiInstId) for this phi
    /// — the id its builder handed back, re-mintable from a rediscovered
    /// handle.
    #[inline]
    pub fn id(&self) -> PhiInstId<W, B> {
        PhiInstId::from_raw(self.module.id(), self.id)
    }

    /// Result handle for the phi node, narrowed to the static width
    /// `W`.
    #[inline]
    pub fn as_int_value(&self) -> IntValue<'ctx, W, B> {
        let v = Value::from_parts(self.id, self.module, self.ty);
        IntValue::<W, B>::from_value_unchecked(v)
    }

    pub fn incoming_count(&self) -> u32 {
        let len = self.payload().incoming.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"))
    }

    /// Read the `(value, block label)` pair at `index`.
    pub fn incoming(&self, index: u32) -> IrResult<(Value<'ctx, B>, BlockId<Dyn, B>)> {
        let slot = usize::try_from(index).unwrap_or_else(|_| unreachable!("u32 fits in usize"));
        let module = self.module.module();
        let pair = self
            .payload()
            .incoming
            .borrow()
            .get(slot)
            .map(|(v, b)| (v.get(), *b))
            .ok_or(crate::IrError::ArgumentIndexOutOfRange {
                index,
                count: self.incoming_count(),
            })?;
        let (vid, bid) = pair;
        let v_data = module.context().value_data(vid);
        let value = Value::from_parts(vid, self.module, v_data.ty);
        let block = BlockId::<Dyn, B>::from_raw(self.module.id(), bid);
        Ok((value, block))
    }

    /// Iterate the `(value, block label)` incoming pairs in declaration
    /// order — the same pairs [`Self::incoming`] yields by index. Mirrors
    /// walking `PHINode::blocks()`/`incoming_values()`. Snapshots the
    /// incoming list up front (like [`SwitchInst::cases`]), so callers may
    /// mutate the phi while iterating.
    pub fn incomings(
        &self,
    ) -> impl ExactSizeIterator<Item = (Value<'ctx, B>, BlockId<Dyn, B>)>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, W, B> {
        let module = self.module.module();
        let module_ref = self.module;
        let entries: Vec<(ValueSlot, ValueSlot)> = self
            .payload()
            .incoming
            .borrow()
            .iter()
            .map(|(v, b)| (v.get(), *b))
            .collect();
        entries.into_iter().map(move |(vid, bid)| {
            let v_data = module.context().value_data(vid);
            let value = Value::from_parts(vid, module_ref, v_data.ty);
            let block = BlockId::<Dyn, B>::from_raw(module_ref.id(), bid);
            (value, block)
        })
    }

    /// Remove the incoming `(value, block)` pair at `index` and return the
    /// removed value. Mirrors `PHINode::removeIncomingValue`, which real CFG
    /// rewriters call when a predecessor edge disappears.
    ///
    /// Like upstream, the vacated slot is backfilled from the **end** of the
    /// incoming list, so **incoming order is not preserved**. Errors with
    /// [`IrError::ArgumentIndexOutOfRange`] when `index` is past the end.
    ///
    /// Requires an `Unverified` module token: like
    /// [`AtomicRmwInst::set_value_operand`], this mutates the IR and must not be
    /// reachable without proof of mutation capability.
    ///
    /// Unlike upstream's default `DeletePHIIfEmpty = true`, removing the last
    /// incoming leaves the (now unprintable) phi in place — see
    /// the module-level note on this file's `phi_remove_incoming` helper for why
    /// self-erasure cannot be sound on a `Copy` handle, and use the `ReshapeCfg`
    /// edge edits when the whole predecessor edge is going away.
    pub fn remove_incoming(
        &self,
        module_token: &'ctx Module<B, Unverified>,
        index: u32,
    ) -> IrResult<Value<'ctx, B>> {
        let _ = module_token;
        phi_remove_incoming(self.id, self.module, self.payload(), index)
    }
}

// The typed phi *authoring* surface is crate-internal since slice 7 (block
// arguments are the public phi-authoring surface). It has no production caller
// today — the parser and SSA builder add incomings through the erased
// `phi_add_incoming_from_value` path — so `dead_code` is allowed in non-test
// builds; the in-crate raw-phi tests exercise it.
#[cfg(test)]
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> PhiInst<'ctx, W, B> {
    /// Append `(value, block)` to the incoming list. Mirrors
    /// `PHINode::addIncoming`. Returns `Self` so calls chain.
    /// Errors if `value`'s type does not match the phi's result type.
    /// Rejects a second entry for the same block with a different value
    /// ([`IrError::AmbiguousPhiIncoming`](crate::IrError::AmbiguousPhiIncoming));
    /// same-value duplicates are legal (multi-edges from `switch`).
    /// The block's module provenance is carried by its branded handle in
    /// ordinary construction paths; CFG predecessor completeness is verified by
    /// [`Module::verify`](crate::Module::verify).
    pub(crate) fn add_incoming<V, R, Block>(self, value: V, block: Block) -> IrResult<Self>
    where
        V: IntoIntValue<'ctx, W, B>,
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let value = value.into_int_value(self.module)?;
        if value.as_erased().ty == self.ty {
            let value_id = value.slot();
            let block_id = block.into_basic_block_label(self.module)?.slot();
            if self
                .payload()
                .incoming
                .borrow()
                .iter()
                .any(|(v, b)| *b == block_id && v.get() != value_id)
            {
                return Err(crate::IrError::AmbiguousPhiIncoming {
                    block: module.context().block_diag_name(block_id),
                });
            }
            self.payload()
                .incoming
                .borrow_mut()
                .push((core::cell::Cell::new(value_id), block_id));
            // Register the phi as a user of the incoming value.
            module
                .context()
                .value_data(value_id)
                .add_use(ValueUse::Instruction(self.id));
            Ok(self)
        } else {
            Err(crate::IrError::TypeMismatch {
                expected: Type::<B>::new(self.ty, module).kind_label(),
                got: value.as_erased().ty().kind_label(),
            })
        }
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand> Clone for PhiInst<'ctx, W, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> Copy for PhiInst<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand> PartialEq for PhiInst<'ctx, W, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> Eq for PhiInst<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand> core::hash::Hash for PhiInst<'ctx, W, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

// --------------------------------------------------------------------------
// FpPhiInst<'ctx, K, P> -- floating-point phi handle
// --------------------------------------------------------------------------

/// `phi` node whose result type is `FloatType<'ctx, K>`. Mirrors
/// upstream `PHINode` in `Instructions.h`; we keep one handle per
/// element-kind family (int / float / pointer) to mirror the existing
/// per-opcode handle pattern in this crate (the unified-trait alternative
/// would force every read accessor through dyn dispatch).
#[derive(Branded)]
#[branded(Debug)]
pub struct FpPhiInst<'ctx, K: FloatKind, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _k: core::marker::PhantomData<fn() -> K>,
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> FpPhiInst<'ctx, K, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _k: core::marker::PhantomData,
        }
    }

    fn payload(&self) -> &'ctx PhiData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Phi(p) => p,
                _ => unreachable!("FpPhiInst invariant: kind is Phi"),
            },
            _ => unreachable!("FpPhiInst invariant: kind is Instruction"),
        }
    }

    /// Fast-math flags. Mirrors `FPMathOperator::getFastMathFlags`.
    pub fn fast_math_flags(&self) -> FastMathFlags {
        self.payload().fmf.get()
    }

    /// Set the fast-math flags. Mirrors `Instruction::setFastMathFlags`,
    /// which `LLParser::parseInstruction` calls after `parsePHI` returns.
    ///
    /// A `phi` is an `FPMathOperator` only when its result type is
    /// floating-point (scalar or vector), so non-empty flags on any other
    /// result type are refused; upstream reports that as `fast-math-flags
    /// specified for phi without floating-point scalar or vector return
    /// type`.
    pub fn set_fast_math_flags(&self, fmf: FastMathFlags) -> IrResult<()> {
        if !fmf.is_empty()
            && !crate::operator::is_supported_floating_point_type(self.as_view().ty())
        {
            return Err(IrError::InvalidOperation {
                message: "fast-math flags require a floating-point phi result",
            });
        }
        self.payload().fmf.set(fmf);
        Ok(())
    }

    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// Bare arena slot of the underlying value (same slot as
    /// [`to_erased`](Self::to_erased)). Untagged: prefer [`id`](Self::id).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.to_erased().id
    }

    /// Storable, module-tagged [`FpPhiInstId<K>`](crate::FpPhiInstId) for this
    /// phi.
    #[inline]
    pub fn id(&self) -> FpPhiInstId<K, B> {
        FpPhiInstId::from_raw(self.module.id(), self.id)
    }

    /// Result handle for the phi, narrowed to the static kind `K`.
    #[inline]
    pub fn as_float_value(&self) -> FloatValue<'ctx, K, B> {
        let v = Value::from_parts(self.id, self.module, self.ty);
        FloatValue::<K, B>::from_value_unchecked(v)
    }

    pub fn incoming_count(&self) -> u32 {
        let len = self.payload().incoming.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"))
    }

    /// Read the `(value, block label)` pair at `index`.
    pub fn incoming(&self, index: u32) -> IrResult<(Value<'ctx, B>, BlockId<Dyn, B>)> {
        let slot = usize::try_from(index).unwrap_or_else(|_| unreachable!("u32 fits in usize"));
        let module = self.module.module();
        let pair = self
            .payload()
            .incoming
            .borrow()
            .get(slot)
            .map(|(v, b)| (v.get(), *b))
            .ok_or(crate::IrError::ArgumentIndexOutOfRange {
                index,
                count: self.incoming_count(),
            })?;
        let (vid, bid) = pair;
        let v_data = module.context().value_data(vid);
        let value = Value::from_parts(vid, self.module, v_data.ty);
        let block = BlockId::<Dyn, B>::from_raw(self.module.id(), bid);
        Ok((value, block))
    }

    /// Iterate the `(value, block label)` incoming pairs in declaration
    /// order — the same pairs [`Self::incoming`] yields by index. Mirrors
    /// walking `PHINode::blocks()`/`incoming_values()`. Snapshots the
    /// incoming list up front (like [`SwitchInst::cases`]), so callers may
    /// mutate the phi while iterating.
    pub fn incomings(
        &self,
    ) -> impl ExactSizeIterator<Item = (Value<'ctx, B>, BlockId<Dyn, B>)>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, K, B> {
        let module = self.module.module();
        let module_ref = self.module;
        let entries: Vec<(ValueSlot, ValueSlot)> = self
            .payload()
            .incoming
            .borrow()
            .iter()
            .map(|(v, b)| (v.get(), *b))
            .collect();
        entries.into_iter().map(move |(vid, bid)| {
            let v_data = module.context().value_data(vid);
            let value = Value::from_parts(vid, module_ref, v_data.ty);
            let block = BlockId::<Dyn, B>::from_raw(module_ref.id(), bid);
            (value, block)
        })
    }

    /// Remove the incoming `(value, block)` pair at `index` and return the
    /// removed value — the float-phi twin of
    /// [`PhiInst::remove_incoming`], with the same upstream-mirroring
    /// swap-with-last semantics and the same non-deleting empty-phi contract.
    pub fn remove_incoming(
        &self,
        module_token: &'ctx Module<B, Unverified>,
        index: u32,
    ) -> IrResult<Value<'ctx, B>> {
        let _ = module_token;
        phi_remove_incoming(self.id, self.module, self.payload(), index)
    }
}

#[cfg(test)]
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> FpPhiInst<'ctx, K, B> {
    /// Append `(value, block)` to the incoming list. Mirrors
    /// `PHINode::addIncoming`. Errors if `value`'s type does not match
    /// the phi's result type. Rejects a second entry for the same block with
    /// a different value
    /// ([`IrError::AmbiguousPhiIncoming`](crate::IrError::AmbiguousPhiIncoming));
    /// same-value duplicates are legal (multi-edges from `switch`).
    /// The block's module provenance is carried by its
    /// branded handle in ordinary construction paths; CFG predecessor
    /// completeness is verified by [`Module::verify`](crate::Module::verify).
    pub(crate) fn add_incoming<V, R, Block>(self, value: V, block: Block) -> IrResult<Self>
    where
        V: IntoFloatValue<'ctx, K, B>,
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let value = value.into_float_value(self.module)?;
        if value.as_erased().ty == self.ty {
            let value_id = value.slot();
            let block_id = block.into_basic_block_label(self.module)?.slot();
            if self
                .payload()
                .incoming
                .borrow()
                .iter()
                .any(|(v, b)| *b == block_id && v.get() != value_id)
            {
                return Err(crate::IrError::AmbiguousPhiIncoming {
                    block: module.context().block_diag_name(block_id),
                });
            }
            self.payload()
                .incoming
                .borrow_mut()
                .push((core::cell::Cell::new(value_id), block_id));
            module
                .context()
                .value_data(value_id)
                .add_use(ValueUse::Instruction(self.id));
            Ok(self)
        } else {
            Err(crate::IrError::TypeMismatch {
                expected: Type::<B>::new(self.ty, module).kind_label(),
                got: value.as_erased().ty().kind_label(),
            })
        }
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand> Clone for FpPhiInst<'ctx, K, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> Copy for FpPhiInst<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand> PartialEq for FpPhiInst<'ctx, K, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> Eq for FpPhiInst<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand> core::hash::Hash for FpPhiInst<'ctx, K, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

// --------------------------------------------------------------------------
// PointerPhiInst<'ctx, P> -- pointer phi handle
// --------------------------------------------------------------------------

/// `phi` node whose result type is a pointer. Pointers carry no
/// element-kind type parameter (only addrspace, which is encoded in
/// the type id), so the handle carries no marker beyond the brand.
#[derive(Branded)]
#[branded(Debug, Clone, Copy)]
pub struct PointerPhiInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

impl<'ctx, B: ModuleBrand + 'ctx> PointerPhiInst<'ctx, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
        }
    }

    fn payload(&self) -> &'ctx PhiData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Phi(p) => p,
                _ => unreachable!("PointerPhiInst invariant: kind is Phi"),
            },
            _ => unreachable!("PointerPhiInst invariant: kind is Instruction"),
        }
    }

    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// Bare arena slot of the underlying value (same slot as
    /// [`to_erased`](Self::to_erased)). Untagged: prefer [`id`](Self::id).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.to_erased().id
    }

    /// Storable, module-tagged [`PointerPhiInstId`] for this phi.
    #[inline]
    pub fn id(&self) -> PointerPhiInstId<B> {
        PointerPhiInstId::from_raw(self.module.id(), self.id)
    }

    /// Result handle for the phi, narrowed to a [`PointerValue`].
    #[inline]
    pub fn as_pointer_value(&self) -> PointerValue<'ctx, B> {
        let v = Value::from_parts(self.id, self.module, self.ty);
        PointerValue::from_value_unchecked(v)
    }

    pub fn incoming_count(&self) -> u32 {
        let len = self.payload().incoming.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"))
    }

    /// Read the `(value, block label)` pair at `index`.
    pub fn incoming(&self, index: u32) -> IrResult<(Value<'ctx, B>, BlockId<Dyn, B>)> {
        let slot = usize::try_from(index).unwrap_or_else(|_| unreachable!("u32 fits in usize"));
        let module = self.module.module();
        let pair = self
            .payload()
            .incoming
            .borrow()
            .get(slot)
            .map(|(v, b)| (v.get(), *b))
            .ok_or(crate::IrError::ArgumentIndexOutOfRange {
                index,
                count: self.incoming_count(),
            })?;
        let (vid, bid) = pair;
        let v_data = module.context().value_data(vid);
        let value = Value::from_parts(vid, self.module, v_data.ty);
        let block = BlockId::<Dyn, B>::from_raw(self.module.id(), bid);
        Ok((value, block))
    }

    /// Iterate the `(value, block label)` incoming pairs in declaration
    /// order — the same pairs [`Self::incoming`] yields by index. Mirrors
    /// walking `PHINode::blocks()`/`incoming_values()`. Snapshots the
    /// incoming list up front (like [`SwitchInst::cases`]), so callers may
    /// mutate the phi while iterating.
    pub fn incomings(
        &self,
    ) -> impl ExactSizeIterator<Item = (Value<'ctx, B>, BlockId<Dyn, B>)>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, B> {
        let module = self.module.module();
        let module_ref = self.module;
        let entries: Vec<(ValueSlot, ValueSlot)> = self
            .payload()
            .incoming
            .borrow()
            .iter()
            .map(|(v, b)| (v.get(), *b))
            .collect();
        entries.into_iter().map(move |(vid, bid)| {
            let v_data = module.context().value_data(vid);
            let value = Value::from_parts(vid, module_ref, v_data.ty);
            let block = BlockId::<Dyn, B>::from_raw(module_ref.id(), bid);
            (value, block)
        })
    }

    /// Remove the incoming `(value, block)` pair at `index` and return the
    /// removed value — the pointer-phi twin of
    /// [`PhiInst::remove_incoming`], with the same upstream-mirroring
    /// swap-with-last semantics and the same non-deleting empty-phi contract.
    pub fn remove_incoming(
        &self,
        module_token: &'ctx Module<B, Unverified>,
        index: u32,
    ) -> IrResult<Value<'ctx, B>> {
        let _ = module_token;
        phi_remove_incoming(self.id, self.module, self.payload(), index)
    }
}

#[cfg(test)]
impl<'ctx, B: ModuleBrand + 'ctx> PointerPhiInst<'ctx, B> {
    /// Append `(value, block)` to the incoming list. Rejects a second entry
    /// for the same block with a different value
    /// ([`IrError::AmbiguousPhiIncoming`](crate::IrError::AmbiguousPhiIncoming));
    /// same-value duplicates are legal (multi-edges from `switch`).
    pub(crate) fn add_incoming<V, R, Block>(self, value: V, block: Block) -> IrResult<Self>
    where
        V: IntoPointerValue<'ctx, B>,
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let value = value.into_pointer_value(self.module)?;
        if value.as_erased().ty == self.ty {
            let value_id = value.slot();
            let block_id = block.into_basic_block_label(self.module)?.slot();
            if self
                .payload()
                .incoming
                .borrow()
                .iter()
                .any(|(v, b)| *b == block_id && v.get() != value_id)
            {
                return Err(crate::IrError::AmbiguousPhiIncoming {
                    block: module.context().block_diag_name(block_id),
                });
            }
            self.payload()
                .incoming
                .borrow_mut()
                .push((core::cell::Cell::new(value_id), block_id));
            module
                .context()
                .value_data(value_id)
                .add_use(ValueUse::Instruction(self.id));
            Ok(self)
        } else {
            Err(crate::IrError::TypeMismatch {
                expected: Type::<B>::new(self.ty, module).kind_label(),
                got: IsValue::as_erased(value).ty().kind_label(),
            })
        }
    }
}

impl<'ctx, B: ModuleBrand> PartialEq for PointerPhiInst<'ctx, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, B: ModuleBrand> Eq for PointerPhiInst<'ctx, B> {}
impl<'ctx, B: ModuleBrand> core::hash::Hash for PointerPhiInst<'ctx, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

// --------------------------------------------------------------------------
// OtherPhiInst<'ctx> -- vector/aggregate phi handle (fully erased)
// --------------------------------------------------------------------------

/// `phi` node whose result type is neither integer, float, nor pointer
/// (a vector, array, or struct). Rediscovery yields this handle so that
/// [`PhiKind::Other`](crate::PhiKind) exposes only the erased read surface
/// — there is no lying `as_int_value()` narrowing (the bug the split
/// [`PhiKind`](crate::PhiKind) exists to remove).
#[derive(Branded)]
#[branded(Debug, Clone, Copy)]
pub struct OtherPhiInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(OtherPhiInst);

impl<'ctx, B: ModuleBrand + 'ctx> OtherPhiInst<'ctx, B> {
    fn payload(&self) -> &'ctx PhiData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Phi(p) => p,
                _ => unreachable!("OtherPhiInst invariant: kind is Phi"),
            },
            _ => unreachable!("OtherPhiInst invariant: kind is Instruction"),
        }
    }

    /// Fast-math flags. Mirrors `FPMathOperator::getFastMathFlags`.
    pub fn fast_math_flags(&self) -> FastMathFlags {
        self.payload().fmf.get()
    }

    /// Set the fast-math flags. Mirrors `Instruction::setFastMathFlags`,
    /// which `LLParser::parseInstruction` calls after `parsePHI` returns.
    ///
    /// A `phi` is an `FPMathOperator` only when its result type is
    /// floating-point (scalar or vector), so non-empty flags on any other
    /// result type are refused; upstream reports that as `fast-math-flags
    /// specified for phi without floating-point scalar or vector return
    /// type`.
    pub fn set_fast_math_flags(&self, fmf: FastMathFlags) -> IrResult<()> {
        if !fmf.is_empty()
            && !crate::operator::is_supported_floating_point_type(self.as_view().ty())
        {
            return Err(IrError::InvalidOperation {
                message: "fast-math flags require a floating-point phi result",
            });
        }
        self.payload().fmf.set(fmf);
        Ok(())
    }

    /// Bare arena slot of the underlying value (same slot as
    /// [`to_erased`](Self::to_erased)). Untagged: prefer [`id`](Self::id).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.id
    }

    /// Storable, module-tagged [`OtherPhiInstId`] for this phi.
    #[inline]
    pub fn id(&self) -> OtherPhiInstId<B> {
        OtherPhiInstId::from_raw(self.module.id(), self.id)
    }

    /// Remove the incoming `(value, block)` pair at `index` and return the
    /// removed value — the erased-phi twin of [`PhiInst::remove_incoming`],
    /// with the same upstream-mirroring swap-with-last semantics and the same
    /// non-deleting empty-phi contract.
    pub fn remove_incoming(
        &self,
        module_token: &'ctx Module<B, Unverified>,
        index: u32,
    ) -> IrResult<Value<'ctx, B>> {
        let _ = module_token;
        phi_remove_incoming(self.id, self.module, self.payload(), index)
    }

    /// Number of incoming `(value, block)` edges.
    pub fn incoming_count(&self) -> u32 {
        let len = self.payload().incoming.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"))
    }

    /// Read the `(value, block label)` pair at `index`.
    pub fn incoming(&self, index: u32) -> IrResult<(Value<'ctx, B>, BlockId<Dyn, B>)> {
        let slot = usize::try_from(index).unwrap_or_else(|_| unreachable!("u32 fits in usize"));
        let module = self.module.module();
        let pair = self
            .payload()
            .incoming
            .borrow()
            .get(slot)
            .map(|(v, b)| (v.get(), *b))
            .ok_or(crate::IrError::ArgumentIndexOutOfRange {
                index,
                count: self.incoming_count(),
            })?;
        let (vid, bid) = pair;
        let v_data = module.context().value_data(vid);
        let value = Value::from_parts(vid, self.module, v_data.ty);
        let block = BlockId::<Dyn, B>::from_raw(self.module.id(), bid);
        Ok((value, block))
    }

    /// Iterate the `(value, block label)` incoming pairs in declaration
    /// order — the same pairs [`Self::incoming`] yields by index. Mirrors
    /// walking `PHINode::blocks()`/`incoming_values()`. Snapshots the
    /// incoming list up front (like [`SwitchInst::cases`]), so callers may
    /// mutate the phi while iterating.
    pub fn incomings(
        &self,
    ) -> impl ExactSizeIterator<Item = (Value<'ctx, B>, BlockId<Dyn, B>)>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, B> {
        let module = self.module.module();
        let module_ref = self.module;
        let entries: Vec<(ValueSlot, ValueSlot)> = self
            .payload()
            .incoming
            .borrow()
            .iter()
            .map(|(v, b)| (v.get(), *b))
            .collect();
        entries.into_iter().map(move |(vid, bid)| {
            let v_data = module.context().value_data(vid);
            let value = Value::from_parts(vid, module_ref, v_data.ty);
            let block = BlockId::<Dyn, B>::from_raw(module_ref.id(), bid);
            (value, block)
        })
    }
}

// --------------------------------------------------------------------------
// Unary ops: fneg / freeze / va_arg
// --------------------------------------------------------------------------

/// `fneg` floating-point negate. Mirrors `UnaryOperator::FNeg` in
/// `InstrTypes.h`. Carries [`crate::FastMathFlags`] like every
/// `FPMathOperator`-class instruction (`Operator.h`).
#[derive(Branded)]
pub struct FnegInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(FnegInst);

impl<'ctx, B: ModuleBrand + 'ctx> FnegInst<'ctx, B> {
    fn payload(self) -> &'ctx FnegInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Fneg(u) => u,
                _ => unreachable!("FnegInst invariant: kind is Fneg"),
            },
            _ => unreachable!("FnegInst invariant: kind is Instruction"),
        }
    }
    /// Source operand. Mirrors `UnaryOperator::getOperand(0)`.
    pub fn operand(self) -> Value<'ctx, B> {
        let id = self.payload().src.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// Fast-math flags. Mirrors `FPMathOperator::getFastMathFlags`.
    pub fn fast_math_flags(self) -> FastMathFlags {
        self.payload().fmf
    }
}

/// `freeze` poison/undef-removing operator. Mirrors `FreezeInst`
/// (`Instructions.h`). The result type matches the operand type.
#[derive(Branded)]
pub struct FreezeInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(FreezeInst);

impl<'ctx, B: ModuleBrand + 'ctx> FreezeInst<'ctx, B> {
    fn payload(self) -> &'ctx FreezeInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Freeze(u) => u,
                _ => unreachable!("FreezeInst invariant: kind is Freeze"),
            },
            _ => unreachable!("FreezeInst invariant: kind is Instruction"),
        }
    }
    /// Source operand. Mirrors `FreezeInst::getOperand(0)`.
    pub fn operand(self) -> Value<'ctx, B> {
        let id = self.payload().src.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
}

/// `va_arg` instruction. Mirrors `VaArgInst` (`Instructions.h`).
/// Loads the next argument from a `va_list` pointer; the destination
/// type lives on [`Self::result_type`].
#[derive(Branded)]
pub struct VaArgInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(VaArgInst);

impl<'ctx, B: ModuleBrand + 'ctx> VaArgInst<'ctx, B> {
    fn payload(self) -> &'ctx VaArgInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::VaArg(u) => u,
                _ => unreachable!("VaArgInst invariant: kind is VaArg"),
            },
            _ => unreachable!("VaArgInst invariant: kind is Instruction"),
        }
    }
    /// `va_list` pointer operand.
    /// Pointer operand (the `va_list`). Statically a pointer, so returned
    /// as [`PointerValue`] rather than the erased [`Value`].
    pub fn pointer(self) -> PointerValue<'ctx, B> {
        let id = self.payload().src.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        PointerValue::from_value_unchecked(Value::from_parts(id, self.module, data.ty))
    }
    /// Destination type (the second `, T` in `va_arg ptr %vl, T`).
    pub fn result_type(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}

// --------------------------------------------------------------------------
// Aggregate ops: extractvalue / insertvalue
// --------------------------------------------------------------------------

/// `extractvalue` reads a single sub-element of an aggregate by
/// constant indices. Mirrors `ExtractValueInst` (`Instructions.h`).
#[derive(Branded)]
pub struct ExtractValueInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(ExtractValueInst);

impl<'ctx, B: ModuleBrand + 'ctx> ExtractValueInst<'ctx, B> {
    fn payload(self) -> &'ctx ExtractValueInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::ExtractValue(d) => d,
                _ => unreachable!("ExtractValueInst invariant: kind is ExtractValue"),
            },
            _ => unreachable!("ExtractValueInst invariant: kind is Instruction"),
        }
    }
    /// Aggregate operand. Mirrors `getAggregateOperand`.
    pub fn aggregate(self) -> Value<'ctx, B> {
        let id = self.payload().aggregate.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// Compile-time index path. Mirrors `ExtractValueInst::indices`.
    pub fn indices(self) -> &'ctx [u32] {
        &self.payload().indices
    }
}

/// `insertvalue` writes a sub-element back into an aggregate by
/// constant indices. Mirrors `InsertValueInst` (`Instructions.h`).
#[derive(Branded)]
pub struct InsertValueInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(InsertValueInst);

impl<'ctx, B: ModuleBrand + 'ctx> InsertValueInst<'ctx, B> {
    fn payload(self) -> &'ctx InsertValueInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::InsertValue(d) => d,
                _ => unreachable!("InsertValueInst invariant: kind is InsertValue"),
            },
            _ => unreachable!("InsertValueInst invariant: kind is Instruction"),
        }
    }
    pub fn aggregate(self) -> Value<'ctx, B> {
        let id = self.payload().aggregate.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn inserted_value(self) -> Value<'ctx, B> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn indices(self) -> &'ctx [u32] {
        &self.payload().indices
    }
}

// --------------------------------------------------------------------------
// Vector ops: extractelement / insertelement / shufflevector
// --------------------------------------------------------------------------

/// `extractelement` reads a single element from a vector. Mirrors
/// `ExtractElementInst` (`Instructions.h`).
#[derive(Branded)]
pub struct ExtractElementInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(ExtractElementInst);

impl<'ctx, B: ModuleBrand + 'ctx> ExtractElementInst<'ctx, B> {
    fn payload(self) -> &'ctx ExtractElementInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::ExtractElement(d) => d,
                _ => unreachable!("ExtractElementInst invariant: kind is ExtractElement"),
            },
            _ => unreachable!("ExtractElementInst invariant: kind is Instruction"),
        }
    }
    pub fn vector(self) -> Value<'ctx, B> {
        let id = self.payload().vector.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn index(self) -> Value<'ctx, B> {
        let id = self.payload().index.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
}

/// `insertelement` writes a single element back into a vector.
/// Mirrors `InsertElementInst` (`Instructions.h`).
#[derive(Branded)]
pub struct InsertElementInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(InsertElementInst);

impl<'ctx, B: ModuleBrand + 'ctx> InsertElementInst<'ctx, B> {
    fn payload(self) -> &'ctx InsertElementInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::InsertElement(d) => d,
                _ => unreachable!("InsertElementInst invariant: kind is InsertElement"),
            },
            _ => unreachable!("InsertElementInst invariant: kind is Instruction"),
        }
    }
    pub fn vector(self) -> Value<'ctx, B> {
        let id = self.payload().vector.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn inserted_value(self) -> Value<'ctx, B> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn index(self) -> Value<'ctx, B> {
        let id = self.payload().index.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
}

/// `shufflevector` builds a new vector by selecting elements from two
/// input vectors per a constant integer mask. Mirrors
/// `ShuffleVectorInst` (`Instructions.h`).
#[derive(Branded)]
pub struct ShuffleVectorInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(ShuffleVectorInst);

impl<'ctx, B: ModuleBrand + 'ctx> ShuffleVectorInst<'ctx, B> {
    fn payload(self) -> &'ctx ShuffleVectorInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::ShuffleVector(d) => d,
                _ => unreachable!("ShuffleVectorInst invariant: kind is ShuffleVector"),
            },
            _ => unreachable!("ShuffleVectorInst invariant: kind is Instruction"),
        }
    }
    pub fn lhs(self) -> Value<'ctx, B> {
        let id = self.payload().lhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn rhs(self) -> Value<'ctx, B> {
        let id = self.payload().rhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// Shuffle mask. Mirrors `ShuffleVectorInst::getShuffleMask`.
    ///
    /// Upstream's `-1` poison entries are [`ShuffleMaskElem::Poison`] here, so
    /// a consumer names the case rather than testing a sign.
    pub fn mask(self) -> &'ctx [ShuffleMaskElem] {
        &self.payload().mask
    }

    /// Mirrors `ShuffleVectorInst::isValidOperands(const Value *V1, const
    /// Value *V2, ArrayRef<int> Mask)` (`Instructions.cpp`) — the
    /// **decoded-mask** overload. It is what the
    /// `ShuffleVectorInst(Value *, Value *, ArrayRef<int>, ...)` constructor
    /// asserts on, what `ConstantExpr::getShuffleVector` asserts on, and what
    /// `Verifier::visitShuffleVectorInst` calls.
    ///
    /// It is **not** the same predicate as
    /// [`Self::is_valid_operands_with_constant_mask`]: only that one has the
    /// `undef` / `zeroinitializer` early `return true` that precedes every
    /// scalable test.
    ///
    /// Two spellings change because Rust forces them; neither changes the
    /// logic.
    ///
    /// * `Elem != PoisonMaskElem` is [`ShuffleMaskElem::Lane`], and
    ///   `Mask[0] != 0 && Mask[0] != PoisonMaskElem` is the pair of variant
    ///   tests below. [`ShuffleMaskElem`] has exactly two cases, so the
    ///   translation is exact — there is no second negative sentinel in the IR
    ///   alphabet (`SM_SentinelZero` belongs to code generation, which is out
    ///   of scope; `docs/divergences.md` entry 69).
    /// * `Mask[0]` on an empty mask is `ArrayRef::operator[]`'s bounds
    ///   assertion. A crate that forbids runtime panics cannot abort, so the
    ///   empty scalable mask answers `false` — the verdict LLVM reaches one
    ///   step later anyway, since the constructor then calls
    ///   `VectorType::get(EltTy, 0, /*Scalable=*/true)`, whose own assertion
    ///   rejects a zero minimum element count.
    pub fn is_valid_operands(
        v1: Value<'ctx, B>,
        v2: Value<'ctx, B>,
        mask: &[ShuffleMaskElem],
    ) -> bool {
        // V1 and V2 must be vectors of the same type.
        //
        // The same read also yields upstream's
        // `cast<VectorType>(V1->getType())->getElementCount().getKnownMinValue()`,
        // which `TypeData::as_vector` already returns for both vector kinds.
        let Some((_, v1_size, v1_scalable)) = v1.ty().data().as_vector() else {
            return false;
        };
        if v1.ty() != v2.ty() {
            return false;
        }

        // Make sure the mask elements make sense.
        //
        // `V1Size * 2` is `int` arithmetic upstream; widening to `u64` keeps
        // the comparison exact for every `u32` lane count instead of wrapping.
        let bound = u64::from(v1_size) * 2;
        for element in mask {
            if let ShuffleMaskElem::Lane(lane) = *element
                && u64::from(lane) >= bound
            {
                return false;
            }
        }

        if v1_scalable {
            let Some(&first) = mask.first() else {
                return false;
            };
            if (first != ShuffleMaskElem::Lane(0) && first != ShuffleMaskElem::Poison)
                || !mask.iter().all(|element| *element == first)
            {
                return false;
            }
        }

        true
    }

    /// Mirrors `ShuffleVectorInst::isValidOperands(const Value *V1, const
    /// Value *V2, const Value *Mask)` (`Instructions.cpp`) — the
    /// **constant-mask** overload, the one `LLParser::parseShuffleVector`
    /// calls before the mask is decoded.
    ///
    /// The difference from [`Self::is_valid_operands`] is load-bearing: the
    /// `undef` / `zeroinitializer` early `return true` in the tail (the
    /// crate-internal `valid_shufflevector_mask_constant`)
    /// precedes every scalable test, which is precisely why a scalable
    /// `zeroinitializer` mask is accepted while
    /// `if (isa<ScalableVectorType>(MaskTy)) return false;` two lines below it
    /// refuses every other scalable mask.
    ///
    /// `mask` is a `Value`, not a `Constant`, because upstream's is: a
    /// non-constant mask reaches the routine's closing `return false` rather
    /// than being refused earlier.
    pub fn is_valid_operands_with_constant_mask(
        v1: Value<'ctx, B>,
        v2: Value<'ctx, B>,
        mask: Value<'ctx, B>,
    ) -> bool {
        // V1 and V2 must be vectors of the same type.
        let Some((_, v1_size, v1_scalable)) = v1.ty().data().as_vector() else {
            return false;
        };
        if v1.ty() != v2.ty() {
            return false;
        }

        // Mask must be vector of i32, and must be the same kind of vector as
        // the input vectors.
        let Some((mask_elem, _, mask_scalable)) = mask.ty().data().as_vector() else {
            return false;
        };
        if Type::new(mask_elem, mask.ty().module()).data().as_integer() != Some(32)
            || mask_scalable != v1_scalable
        {
            return false;
        }

        // Check to see if Mask is valid.
        //
        // No upstream counterpart: upstream's three operands are `Value *`s in
        // one `LLVMContext`, while a shared brand ([`DynBrand`], or a re-issued
        // named brand) lets a handle from another module reach here with a slot
        // that means something else in this arena. The two operand types are
        // already covered — `Type`'s equality compares the `ModuleId` as well as
        // the slot — but the mask's slot is read against V1's module below, so
        // it needs the tag test the crate spells `IrError::ForeignValueId`
        // elsewhere. A predicate has no error channel, and a mask belonging to a
        // different module is not a valid operand of this shuffle, so it joins
        // the routine's other rejections.
        if mask.module.id() != v1.module.id() {
            return false;
        }
        crate::constants::valid_shufflevector_mask_constant(
            v1.ty().module().core_ref(),
            mask.slot(),
            v1_size,
            v1_scalable,
        )
    }
}

// --------------------------------------------------------------------------
// Atomic ops: fence / cmpxchg / atomicrmw
// --------------------------------------------------------------------------

/// `fence` instruction. Mirrors `FenceInst` (`Instructions.h`).
/// No SSA operands; carries memory ordering and synchronization scope.
#[derive(Branded)]
pub struct FenceInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(FenceInst);

impl<'ctx, B: ModuleBrand + 'ctx> FenceInst<'ctx, B> {
    fn payload(self) -> &'ctx FenceInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Fence(d) => d,
                _ => unreachable!("FenceInst invariant: kind is Fence"),
            },
            _ => unreachable!("FenceInst invariant: kind is Instruction"),
        }
    }
    /// Memory ordering. Mirrors `FenceInst::getOrdering`.
    pub fn ordering(self) -> AtomicOrdering {
        self.payload().ordering
    }
    /// Synchronization scope. Mirrors `FenceInst::getSyncScopeID`.
    pub fn sync_scope(self) -> SyncScope {
        self.payload().sync_scope.clone()
    }
}

/// `cmpxchg` atomic compare-and-swap. Mirrors `AtomicCmpXchgInst`
/// (`Instructions.h`). Result type is the literal struct
/// `{ <pointee>, i1 }`.
#[derive(Branded)]
pub struct AtomicCmpXchgInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(AtomicCmpXchgInst);

impl<'ctx, B: ModuleBrand + 'ctx> AtomicCmpXchgInst<'ctx, B> {
    fn payload(self) -> &'ctx AtomicCmpXchgInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::AtomicCmpXchg(d) => d,
                _ => unreachable!("AtomicCmpXchgInst invariant: kind is AtomicCmpXchg"),
            },
            _ => unreachable!("AtomicCmpXchgInst invariant: kind is Instruction"),
        }
    }
    /// Pointer operand. Statically a pointer for this opcode, so returned
    /// as [`PointerValue`] rather than the erased [`Value`].
    pub fn pointer(self) -> PointerValue<'ctx, B> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        PointerValue::from_value_unchecked(Value::from_parts(id, self.module, data.ty))
    }
    pub fn compare_value(self) -> Value<'ctx, B> {
        let id = self.payload().cmp.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn new_value(self) -> Value<'ctx, B> {
        let id = self.payload().new_val.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn align(self) -> Option<Align> {
        self.payload().align.align()
    }
    pub fn success_ordering(self) -> AtomicOrdering {
        self.payload().success_ordering
    }
    pub fn failure_ordering(self) -> AtomicOrdering {
        self.payload().failure_ordering
    }
    pub fn sync_scope(self) -> SyncScope {
        self.payload().sync_scope.clone()
    }
    pub fn is_weak(self) -> bool {
        self.payload().weak
    }
    pub fn is_volatile(self) -> bool {
        self.payload().volatile
    }
}

/// `atomicrmw` read-modify-write. Mirrors `AtomicRMWInst`
/// (`Instructions.h`).
#[derive(Branded)]
pub struct AtomicRmwInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(AtomicRmwInst);

impl<'ctx, B: ModuleBrand + 'ctx> AtomicRmwInst<'ctx, B> {
    fn payload(self) -> &'ctx AtomicRmwInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::AtomicRmw(d) => d,
                _ => unreachable!("AtomicRmwInst invariant: kind is AtomicRmw"),
            },
            _ => unreachable!("AtomicRmwInst invariant: kind is Instruction"),
        }
    }
    pub fn operation(self) -> AtomicRmwBinOp {
        self.payload().op
    }
    /// Pointer operand. Statically a pointer for this opcode, so returned
    /// as [`PointerValue`] rather than the erased [`Value`].
    pub fn pointer(self) -> PointerValue<'ctx, B> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        PointerValue::from_value_unchecked(Value::from_parts(id, self.module, data.ty))
    }
    pub fn value_operand(self) -> Value<'ctx, B> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// Replace the value operand in place. Requires an `Unverified`
    /// module token: like [`crate::Instruction::replace_all_uses_with`], this
    /// mutates the IR and must not be reachable without proof of
    /// mutation capability. `module_token` is the capability witness; the
    /// interior-mutable slot is reached through the handle's own
    /// `ModuleRef`.
    pub fn set_value_operand(
        self,
        module_token: &'ctx Module<B, Unverified>,
        value: Value<'ctx, B>,
    ) -> IrResult<()> {
        let _ = module_token;
        let module = self.module.module();
        let expected = Type::new(self.ty, self.module);
        let got = value.ty();
        if got != expected {
            return Err(crate::IrError::TypeMismatch {
                expected: expected.kind_label(),
                got: got.kind_label(),
            });
        }
        let payload = self.payload();
        let old_id = payload.value.replace(value.id);
        if old_id == value.id {
            return Ok(());
        }
        {
            let mut old_uses = module.context().value_data(old_id).use_list.borrow_mut();
            if let Some(pos) = old_uses
                .iter()
                .position(|edge| *edge == ValueUse::Instruction(self.id))
            {
                old_uses.remove(pos);
            }
        }
        module
            .context()
            .value_data(value.id)
            .add_use(ValueUse::Instruction(self.id));
        Ok(())
    }
    pub fn align(self) -> Option<Align> {
        self.payload().align.align()
    }
    pub fn ordering(self) -> AtomicOrdering {
        self.payload().ordering
    }
    pub fn sync_scope(self) -> SyncScope {
        self.payload().sync_scope.clone()
    }
    pub fn is_volatile(self) -> bool {
        self.payload().volatile
    }
}

// --------------------------------------------------------------------------
// Variable-arity terminators: switch / indirectbr
// --------------------------------------------------------------------------

/// `switch` terminator. Mirrors `SwitchInst` (`Instructions.h`).
///
/// The `P: TermOpenState` parameter (default
/// [`Open`](TermOpen)) tracks whether this handle view can edit the case list.
/// `add_case` is gated to `P = Open`; `finish` moves the open handle and
/// returns a `Closed` view. Rediscovery through opcode discriminators is closed.
///
/// The `W: IntWidth` parameter (default [`IntDyn`]) threads the condition's
/// integer width. On a statically-typed switch (`W = i32`, …), every case
/// added through [`add_case`](SwitchInst::add_case) must have the SAME width
/// `W` — a wrong-width case is a *compile* error (there is no
/// `IntoIntValue<'ctx, W, B>` impl for the mismatched value). The erased
/// `W = IntDyn` flavour (produced by the parser / SSA builder via the
/// width-erased [`switch_dyn`](crate::IrBuilder::switch_dyn)) keeps the
/// runtime [`crate::IrError::TypeMismatch`] check instead. `W` is the LAST parameter
/// and defaults to `IntDyn`, so width-agnostic `SwitchInst<'ctx, P, B>`
/// annotations keep resolving to the erased flavour unchanged.
#[derive(Branded)]
#[branded(Debug)]
pub struct SwitchInst<'ctx, P: TermOpenState, B: ModuleBrand, W: IntWidth = IntDyn> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _p: core::marker::PhantomData<P>,
    _w: core::marker::PhantomData<W>,
}

impl<'ctx, P: TermOpenState, B: ModuleBrand, W: IntWidth> PartialEq for SwitchInst<'ctx, P, B, W> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: TermOpenState, B: ModuleBrand, W: IntWidth> Eq for SwitchInst<'ctx, P, B, W> {}
impl<'ctx, P: TermOpenState, B: ModuleBrand, W: IntWidth> core::hash::Hash
    for SwitchInst<'ctx, P, B, W>
{
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: TermOpenState, B: ModuleBrand + 'ctx, W: IntWidth> SwitchInst<'ctx, P, B, W> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _p: core::marker::PhantomData,
            _w: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(super) fn retag<P2: TermOpenState>(self) -> SwitchInst<'ctx, P2, B, W> {
        SwitchInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
            _w: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx SwitchInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Switch(d) => d,
                _ => unreachable!("SwitchInst invariant: kind is Switch"),
            },
            _ => unreachable!("SwitchInst invariant: kind is Instruction"),
        }
    }
    pub fn condition(&self) -> Value<'ctx, B> {
        let id = self.payload().cond.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn default_destination(&self) -> BlockId<Dyn, B> {
        BlockId::<Dyn, B>::from_raw(self.module.id(), self.payload().default_bb.get())
    }
    pub fn case_count(&self) -> u32 {
        let len = self.payload().cases.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("switch has more than u32::MAX cases"))
    }
    /// Iterate the `(case_value, target_block)` entries in declaration
    /// order. Mirrors walking `SwitchInst::cases()`.
    pub fn cases(
        &self,
    ) -> impl ExactSizeIterator<Item = (Value<'ctx, B>, BlockId<Dyn, B>)>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, P, B, W> {
        let module = self.module.module();
        let module_ref = self.module;
        let entries: Vec<(ValueSlot, ValueSlot)> = self
            .payload()
            .cases
            .borrow()
            .iter()
            .map(|(v, b)| (v.get(), *b))
            .collect();
        entries.into_iter().map(move |(vid, bid)| {
            let v_data = module.context().value_data(vid);
            let value = Value::from_parts(vid, module_ref, v_data.ty);
            let block = BlockId::<Dyn, B>::from_raw(module_ref.id(), bid);
            (value, block)
        })
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, W: IntWidth> SwitchInst<'ctx, TermOpen, B, W> {
    /// Consume the open switch and return its [`TermClosed`] view, preserving
    /// the condition width `W`. Mirrors the implicit "switch is finalised"
    /// convention upstream where the verifier subsequently runs
    /// `Verifier::visitSwitchInst`.
    #[inline]
    pub fn finish(self) -> SwitchInst<'ctx, TermClosed, B, W> {
        self.retag()
    }

    /// Shared case-append body for the *public* `add_case` flavours: validate,
    /// reject a parameterised target, then record. Both flavours funnel
    /// through here once their width discipline (compile-time for static `W`,
    /// runtime for [`IntDyn`]) has been discharged.
    ///
    /// A case edge added this way carries no block arguments, so its target
    /// must not be a **parameterised** block — the same guard the plain
    /// terminator builders apply, reported as
    /// [`crate::IrError::PhiArgArityMismatch`]. The argument-carrying route is
    /// [`IrBuilder::switch_with_args`](crate::IrBuilder::switch_with_args)
    /// (and its erased twin), which spells every case at the call and hands
    /// back an already-[`TermClosed`] switch — so a `switch` reaching a
    /// parameterised block either carries that block's arguments or does not
    /// build.
    fn push_case_checked<R, Target>(self, v: Value<'ctx, B>, target: Target) -> IrResult<Self>
    where
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = self.validate_case(v, target)?;
        require_no_block_parameters(self.module, target.slot())?;
        Ok(self.record_case(v, target))
    }

    /// Append a case whose target's block parameters this call's caller has
    /// **already seeded**, skipping the parameterised-target rejection
    /// `push_case_checked` applies. Crate-internal: only
    /// [`IrBuilder::switch_with_args`](crate::IrBuilder::switch_with_args)
    /// and its erased twin reach for it, after `add_block_args` has recorded
    /// each edge's incomings.
    pub(crate) fn push_case_seeded<R, Target>(
        self,
        v: Value<'ctx, B>,
        target: Target,
    ) -> IrResult<Self>
    where
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = self.validate_case(v, target)?;
        Ok(self.record_case(v, target))
    }

    /// Check a case value against the switch condition and resolve its target
    /// label. Runs before anything is recorded, so a rejected case leaves the
    /// case list untouched.
    ///
    /// Defence in depth: the case value's runtime type must still equal the
    /// condition's. For a typed switch this is guaranteed by the
    /// `IntoIntValue<'ctx, W, B>` bound; for the erased switch it is the
    /// primary check (mirrors `Verifier::visitSwitchInst`).
    fn validate_case<R, Target>(
        &self,
        v: Value<'ctx, B>,
        target: Target,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>>
    where
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let cond_ty = self.payload().cond.get();
        let cond_ty = module.context().value_data(cond_ty).ty;
        if v.ty != cond_ty {
            return Err(crate::IrError::TypeMismatch {
                expected: Type::<B>::new(cond_ty, module).kind_label(),
                got: v.ty().kind_label(),
            });
        }
        target.into_basic_block_label(self.module)
    }

    /// Push `(case_value_id, target)` onto the case list and register the
    /// switch as a user of the case value. Infallible: every check the case
    /// has to pass ran in `validate_case` / the caller's guard.
    fn record_case<R: ReturnMarker>(
        self,
        v: Value<'ctx, B>,
        target: BasicBlockLabel<'ctx, R, B>,
    ) -> Self {
        let v_id = v.id;
        self.payload()
            .cases
            .borrow_mut()
            .push((core::cell::Cell::new(v_id), target.slot()));
        let context = self.module.module().context();
        context
            .value_data(v_id)
            .add_use(ValueUse::Instruction(self.id));
        // `SwitchInst::addCase` grows the operand list by *two* — the case
        // value and its destination — so the block gets an edge as well.
        // Registered after the value, matching `[…, CaseVal, CaseDest]`.
        context
            .value_data(target.slot())
            .add_use(ValueUse::Instruction(self.id));
        self
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> SwitchInst<'ctx, TermOpen, B, IntDyn> {
    /// Append a `(case_value, target)` entry to a width-erased switch.
    /// Mirrors `SwitchInst::addCase`. Returns `Self` so calls chain.
    ///
    /// This is the erased flavour used by the parser and SSA builder: the
    /// case value is accepted as any [`IsValue`] and its width is checked
    /// against the condition at RUNTIME (the same
    /// [`crate::IrError::TypeMismatch`] LLVM's verifier would raise). The typed
    /// flavour on a `SwitchInst<'ctx, TermOpen, B, W>` for a static `W`
    /// makes a wrong-width case a compile error instead.
    pub fn add_case<V, R, Target>(self, case_value: V, target: Target) -> IrResult<Self>
    where
        V: IsValue<'ctx, B>,
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let v = case_value.as_erased();
        self.push_case_checked(v, target)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, W: StaticIntWidth> SwitchInst<'ctx, TermOpen, B, W> {
    /// Append a `(case_value, target)` entry to a statically-typed switch.
    /// Mirrors `SwitchInst::addCase`. Returns `Self` so calls chain.
    ///
    /// The case value must have the SAME width `W` as the condition:
    /// `V: IntoIntValue<'ctx, W, B>`. An `IntValue<'ctx, i64, B>` (or a
    /// bare `i64` literal) on a `W = i32` switch has no such impl, so the
    /// mismatch is a *compile* error rather than the runtime
    /// [`crate::IrError::TypeMismatch`] the erased flavour reports. A runtime
    /// type-equality backstop still runs as defence in depth.
    pub fn add_case<V, R, Target>(self, case_value: V, target: Target) -> IrResult<Self>
    where
        V: IntoIntValue<'ctx, W, B>,
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = self.module;
        let v = IsValue::as_erased(case_value.into_int_value(module_ref)?);
        self.push_case_checked(v, target)
    }
}

/// `indirectbr` terminator. Mirrors `IndirectBrInst`
/// (`Instructions.h`). The address operand selects one of the
/// declared destination blocks at runtime.
#[derive(Branded)]
#[branded(Debug)]
pub struct IndirectBrInst<'ctx, P: TermOpenState, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: TermOpenState, B: ModuleBrand> PartialEq for IndirectBrInst<'ctx, P, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: TermOpenState, B: ModuleBrand> Eq for IndirectBrInst<'ctx, P, B> {}
impl<'ctx, P: TermOpenState, B: ModuleBrand> core::hash::Hash for IndirectBrInst<'ctx, P, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: TermOpenState, B: ModuleBrand + 'ctx> IndirectBrInst<'ctx, P, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(super) fn retag<P2: TermOpenState>(self) -> IndirectBrInst<'ctx, P2, B> {
        IndirectBrInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx IndirectBrInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::IndirectBr(d) => d,
                _ => unreachable!("IndirectBrInst invariant: kind is IndirectBr"),
            },
            _ => unreachable!("IndirectBrInst invariant: kind is Instruction"),
        }
    }
    pub fn address(&self) -> Value<'ctx, B> {
        let id = self.payload().addr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn destination_count(&self) -> u32 {
        let len = self.payload().destinations.borrow().len();
        u32::try_from(len)
            .unwrap_or_else(|_| unreachable!("indirectbr has more than u32::MAX destinations"))
    }
    /// Iterate the destination blocks in declaration order. Mirrors
    /// walking `IndirectBrInst::successors()`.
    pub fn destinations(
        &self,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, P, B> {
        let module_ref = self.module;
        let ids: Vec<ValueSlot> = self.payload().destinations.borrow().clone();
        ids.into_iter()
            .map(move |bid| BlockId::<Dyn, B>::from_raw(module_ref.id(), bid))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> IndirectBrInst<'ctx, TermOpen, B> {
    /// Append a destination block. Mirrors `IndirectBrInst::addDestination`.
    ///
    /// An `indirectbr` edge carries no block arguments and has no
    /// argument-carrying form — the address picks the destination at run time,
    /// so there is nothing to attach a per-edge argument list to. A
    /// **parameterised** destination (one from
    /// [`IrBuilder::append_block_with_params`](crate::IrBuilder::append_block_with_params)
    /// or its siblings) is therefore rejected outright with
    /// [`crate::IrError::PhiArgArityMismatch`], the documented restriction the
    /// block-argument design called for. A block that merely *contains* phis is
    /// unaffected, so the classic phi-with-`indirectbr` shape still parses and
    /// round-trips from `.ll`.
    pub fn add_destination<R, Target>(self, target: Target) -> IrResult<Self>
    where
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = target.into_basic_block_label(self.module)?;
        require_no_block_parameters(self.module, target.slot())?;
        self.payload().destinations.borrow_mut().push(target.slot());
        // `IndirectBrInst::addDestination` appends a real operand.
        self.module
            .module()
            .context()
            .value_data(target.slot())
            .add_use(ValueUse::Instruction(self.id));
        Ok(self)
    }
    /// Consume the open `indirectbr` and return its [`TermClosed`] view.
    #[inline]
    pub fn finish(self) -> IndirectBrInst<'ctx, TermClosed, B> {
        self.retag()
    }
}

// --------------------------------------------------------------------------
// EH-call terminators: invoke / callbr
// --------------------------------------------------------------------------

/// `invoke` terminator. Mirrors `InvokeInst` (`Instructions.h`).
/// Like [`CallInst`] but transfers control to one of two label
/// successors (`normal` / `unwind`). The `R` parameter mirrors
/// [`CallInst`]'s typed-return marker.
#[derive(Branded)]
#[branded(Debug)]
pub struct InvokeInst<'ctx, R: ReturnMarker, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _r: core::marker::PhantomData<R>,
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> Clone for InvokeInst<'ctx, R, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Copy for InvokeInst<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> PartialEq for InvokeInst<'ctx, R, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Eq for InvokeInst<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> core::hash::Hash for InvokeInst<'ctx, R, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> InvokeInst<'ctx, R, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _r: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    /// Re-tag the return marker. Crate-internal: both
    /// [`crate::IrBuilder::invoke_dyn`] (caller-asserted `R2`) and
    /// the typed [`crate::IrBuilder::invoke`] (marker derived
    /// from the callee's `Ret::Marker`) flow through this.
    #[inline]
    pub(super) fn retag<R2: ReturnMarker>(self) -> InvokeInst<'ctx, R2, B> {
        InvokeInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: core::marker::PhantomData,
        }
    }
    /// Erase the return marker.
    #[inline]
    pub fn as_dyn(self) -> InvokeInst<'ctx, Dyn, B> {
        self.retag::<Dyn>()
    }
    fn payload(self) -> &'ctx InvokeInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Invoke(d) => d,
                _ => unreachable!("InvokeInst invariant: kind is Invoke"),
            },
            _ => unreachable!("InvokeInst invariant: kind is Instruction"),
        }
    }
    pub fn callee(self) -> Value<'ctx, B> {
        let id = self.payload().callee.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn function_type(self) -> FunctionType<'ctx, B> {
        FunctionType::new(self.payload().fn_ty, self.module)
    }
    pub fn args(
        self,
    ) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let module = self.module.module();
        let ids: Vec<ValueSlot> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
    pub fn calling_conv(self) -> CallingConv {
        self.payload().calling_conv
    }
    pub fn normal_destination(self) -> BlockId<Dyn, B> {
        BlockId::<Dyn, B>::from_raw(self.module.id(), self.payload().normal_dest.get())
    }
    pub fn unwind_destination(self) -> BlockId<Dyn, B> {
        BlockId::<Dyn, B>::from_raw(self.module.id(), self.payload().unwind_dest.get())
    }
}

/// `callbr` terminator. Mirrors `CallBrInst` (`Instructions.h`).
/// A call-like terminator with one fallthrough destination plus zero
/// or more indirect destination labels.
#[derive(Branded)]
pub struct CallBrInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(CallBrInst);

impl<'ctx, B: ModuleBrand + 'ctx> CallBrInst<'ctx, B> {
    fn payload(self) -> &'ctx CallBrInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::CallBr(d) => d,
                _ => unreachable!("CallBrInst invariant: kind is CallBr"),
            },
            _ => unreachable!("CallBrInst invariant: kind is Instruction"),
        }
    }
    pub fn callee(self) -> Value<'ctx, B> {
        let id = self.payload().callee.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn function_type(self) -> FunctionType<'ctx, B> {
        FunctionType::new(self.payload().fn_ty, self.module)
    }
    pub fn args(
        self,
    ) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let module = self.module.module();
        let ids: Vec<ValueSlot> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
    pub fn calling_conv(self) -> CallingConv {
        self.payload().calling_conv
    }
    pub fn default_destination(self) -> BlockId<Dyn, B> {
        BlockId::<Dyn, B>::from_raw(self.module.id(), self.payload().default_dest.get())
    }
    pub fn indirect_destinations(
        self,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let ids: Vec<ValueSlot> = self
            .payload()
            .indirect_dests
            .iter()
            .map(|c| c.get())
            .collect();
        ids.into_iter()
            .map(move |id| BlockId::<Dyn, B>::from_raw(self.module.id(), id))
    }
}

// --------------------------------------------------------------------------
// EH-data: landingpad / resume
// --------------------------------------------------------------------------

/// `landingpad` instruction. Mirrors `LandingPadInst` (`Instructions.h`).
///
/// The `P: TermOpenState` parameter (default
/// [`Open`](TermOpen)) tracks whether the clause list is still editable.
/// Open mutators are gated to `P = Open`; `finish` moves the open handle.
#[derive(Branded)]
#[branded(Debug)]
pub struct LandingPadInst<'ctx, P: TermOpenState, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: TermOpenState, B: ModuleBrand> PartialEq for LandingPadInst<'ctx, P, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: TermOpenState, B: ModuleBrand> Eq for LandingPadInst<'ctx, P, B> {}
impl<'ctx, P: TermOpenState, B: ModuleBrand> core::hash::Hash for LandingPadInst<'ctx, P, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: TermOpenState, B: ModuleBrand + 'ctx> LandingPadInst<'ctx, P, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(super) fn retag<P2: TermOpenState>(self) -> LandingPadInst<'ctx, P2, B> {
        LandingPadInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx LandingPadInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::LandingPad(d) => d,
                _ => unreachable!("LandingPadInst invariant: kind is LandingPad"),
            },
            _ => unreachable!("LandingPadInst invariant: kind is Instruction"),
        }
    }
    pub fn is_cleanup(&self) -> bool {
        self.payload().cleanup.get()
    }
    pub fn clause_count(&self) -> u32 {
        let len = self.payload().clauses.borrow().len();
        u32::try_from(len)
            .unwrap_or_else(|_| unreachable!("landingpad has more than u32::MAX clauses"))
    }
    /// Iterate the `(kind, type_info)` clauses in declaration order, where
    /// `kind` distinguishes `catch` from `filter`. Mirrors walking
    /// `LandingPadInst::clauses()` + `isCatch`/`isFilter`.
    pub fn clauses(
        &self,
    ) -> impl ExactSizeIterator<Item = (LandingPadClauseKind, Value<'ctx, B>)>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, P, B> {
        let module = self.module.module();
        let module_ref = self.module;
        let entries: Vec<(LandingPadClauseKind, ValueSlot)> = self
            .payload()
            .clauses
            .borrow()
            .iter()
            .map(|(k, v)| (*k, v.get()))
            .collect();
        entries.into_iter().map(move |(kind, vid)| {
            let v_data = module.context().value_data(vid);
            (kind, Value::from_parts(vid, module_ref, v_data.ty))
        })
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> LandingPadInst<'ctx, TermOpen, B> {
    /// Mark this landingpad as a cleanup. Mirrors `LandingPadInst::setCleanup(true)`.
    #[must_use]
    pub fn set_cleanup(self) -> Self {
        self.payload().cleanup.set(true);
        self
    }
    /// Append a `catch <ty> <val>` clause. Mirrors `LandingPadInst::addClause`
    /// for `Catch`.
    pub fn add_catch_clause<V: IsValue<'ctx, B>>(self, type_info: V) -> IrResult<Self> {
        let module = self.module.module();
        let v = type_info.as_erased();
        self.payload()
            .clauses
            .borrow_mut()
            .push((LandingPadClauseKind::Catch, core::cell::Cell::new(v.id)));
        module
            .context()
            .value_data(v.id)
            .add_use(ValueUse::Instruction(self.id));
        Ok(self)
    }
    /// Append a `filter <ty> <val>` clause.
    pub fn add_filter_clause<V: IsValue<'ctx, B>>(self, filter_array: V) -> IrResult<Self> {
        let module = self.module.module();
        let v = filter_array.as_erased();
        self.payload()
            .clauses
            .borrow_mut()
            .push((LandingPadClauseKind::Filter, core::cell::Cell::new(v.id)));
        module
            .context()
            .value_data(v.id)
            .add_use(ValueUse::Instruction(self.id));
        Ok(self)
    }
    /// Consume the open landingpad and return its [`TermClosed`] view.
    #[inline]
    pub fn finish(self) -> LandingPadInst<'ctx, TermClosed, B> {
        self.retag()
    }
}

/// `resume` terminator. Mirrors `ResumeInst` (`Instructions.h`).
/// Single value operand (typically a `landingpad` result).
#[derive(Branded)]
pub struct ResumeInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(ResumeInst);

impl<'ctx, B: ModuleBrand + 'ctx> ResumeInst<'ctx, B> {
    fn payload(self) -> &'ctx ResumeInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Resume(d) => d,
                _ => unreachable!("ResumeInst invariant: kind is Resume"),
            },
            _ => unreachable!("ResumeInst invariant: kind is Instruction"),
        }
    }
    pub fn value(self) -> Value<'ctx, B> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
}

// --------------------------------------------------------------------------
// Funclet ops: cleanuppad / cleanupret / catchpad / catchret / catchswitch
// --------------------------------------------------------------------------

/// `cleanuppad` instruction. Mirrors `CleanupPadInst` (`Instructions.h`).
/// Result is a `token`-typed value used as a funclet pad.
#[derive(Branded)]
pub struct CleanupPadInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(CleanupPadInst);

impl<'ctx, B: ModuleBrand + 'ctx> CleanupPadInst<'ctx, B> {
    fn payload(self) -> &'ctx CleanupPadInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::CleanupPad(d) => d,
                _ => unreachable!("CleanupPadInst invariant: kind is CleanupPad"),
            },
            _ => unreachable!("CleanupPadInst invariant: kind is Instruction"),
        }
    }
    /// `None` represents `within none`. Mirrors
    /// `FuncletPadInst::getParentPad`.
    pub fn parent_pad(self) -> Option<Value<'ctx, B>> {
        let id = self.payload().parent_pad.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, self.module, data.ty))
    }
    pub fn args(
        self,
    ) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let module = self.module.module();
        let ids: Vec<ValueSlot> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
}

/// `catchpad` instruction. Mirrors `CatchPadInst` (`Instructions.h`).
/// Result is a `token`-typed value used as a funclet pad. Parent must
/// be a `catchswitch` (verifier rule).
#[derive(Branded)]
pub struct CatchPadInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(CatchPadInst);

impl<'ctx, B: ModuleBrand + 'ctx> CatchPadInst<'ctx, B> {
    fn payload(self) -> &'ctx CatchPadInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::CatchPad(d) => d,
                _ => unreachable!("CatchPadInst invariant: kind is CatchPad"),
            },
            _ => unreachable!("CatchPadInst invariant: kind is Instruction"),
        }
    }
    pub fn parent_pad(self) -> Option<Value<'ctx, B>> {
        let id = self.payload().parent_pad.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, self.module, data.ty))
    }
    pub fn args(
        self,
    ) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let module = self.module.module();
        let ids: Vec<ValueSlot> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
}

/// `catchret` terminator. Mirrors `CatchReturnInst` (`Instructions.h`).
#[derive(Branded)]
pub struct CatchReturnInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(CatchReturnInst);

impl<'ctx, B: ModuleBrand + 'ctx> CatchReturnInst<'ctx, B> {
    fn payload(self) -> &'ctx CatchReturnInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::CatchReturn(d) => d,
                _ => unreachable!("CatchReturnInst invariant: kind is CatchReturn"),
            },
            _ => unreachable!("CatchReturnInst invariant: kind is Instruction"),
        }
    }
    pub fn catch_pad(self) -> Value<'ctx, B> {
        let id = self.payload().catch_pad.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    pub fn target(self) -> BlockId<Dyn, B> {
        BlockId::<Dyn, B>::from_raw(self.module.id(), self.payload().target_bb)
    }
}

/// `cleanupret` terminator. Mirrors `CleanupReturnInst` (`Instructions.h`).
#[derive(Branded)]
pub struct CleanupReturnInst<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

decl_handle_scaffold!(CleanupReturnInst);

impl<'ctx, B: ModuleBrand + 'ctx> CleanupReturnInst<'ctx, B> {
    fn payload(self) -> &'ctx CleanupReturnInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::CleanupReturn(d) => d,
                _ => unreachable!("CleanupReturnInst invariant: kind is CleanupReturn"),
            },
            _ => unreachable!("CleanupReturnInst invariant: kind is Instruction"),
        }
    }
    pub fn cleanup_pad(self) -> Value<'ctx, B> {
        let id = self.payload().cleanup_pad.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, self.module, data.ty)
    }
    /// `None` represents `unwind to caller`.
    pub fn unwind_dest(self) -> Option<BlockId<Dyn, B>> {
        let id = self.payload().unwind_dest?;
        Some(BlockId::<Dyn, B>::from_raw(self.module.id(), id))
    }
}

/// `catchswitch` terminator. Mirrors `CatchSwitchInst` (`Instructions.h`).
/// Variable-arity handler list with optional unwind destination.
#[derive(Branded)]
#[branded(Debug)]
pub struct CatchSwitchInst<'ctx, P: TermOpenState, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: TermOpenState, B: ModuleBrand> PartialEq for CatchSwitchInst<'ctx, P, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: TermOpenState, B: ModuleBrand> Eq for CatchSwitchInst<'ctx, P, B> {}
impl<'ctx, P: TermOpenState, B: ModuleBrand> core::hash::Hash for CatchSwitchInst<'ctx, P, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: TermOpenState, B: ModuleBrand + 'ctx> CatchSwitchInst<'ctx, P, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(super) fn retag<P2: TermOpenState>(self) -> CatchSwitchInst<'ctx, P2, B> {
        CatchSwitchInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx CatchSwitchInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::CatchSwitch(d) => d,
                _ => unreachable!("CatchSwitchInst invariant: kind is CatchSwitch"),
            },
            _ => unreachable!("CatchSwitchInst invariant: kind is Instruction"),
        }
    }
    pub fn parent_pad(&self) -> Option<Value<'ctx, B>> {
        let id = self.payload().parent_pad.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, self.module, data.ty))
    }
    /// `None` = `unwind to caller`.
    pub fn unwind_dest(&self) -> Option<BlockId<Dyn, B>> {
        let id = self.payload().unwind_dest.get()?;
        Some(BlockId::<Dyn, B>::from_raw(self.module.id(), id))
    }
    pub fn handler_count(&self) -> u32 {
        let len = self.payload().handlers.borrow().len();
        u32::try_from(len)
            .unwrap_or_else(|_| unreachable!("catchswitch has more than u32::MAX handlers"))
    }
    /// Iterate the handler blocks in declaration order. Mirrors walking
    /// `CatchSwitchInst::handlers()`.
    pub fn handlers(
        &self,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, P, B> {
        let module_ref = self.module;
        let ids: Vec<ValueSlot> = self.payload().handlers.borrow().clone();
        ids.into_iter()
            .map(move |bid| BlockId::<Dyn, B>::from_raw(module_ref.id(), bid))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> CatchSwitchInst<'ctx, TermOpen, B> {
    pub fn add_handler<R, Handler>(self, handler: Handler) -> IrResult<Self>
    where
        R: ReturnMarker,
        Handler: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let handler = handler.into_basic_block_label(self.module)?;
        self.payload().handlers.borrow_mut().push(handler.slot());
        // `CatchSwitchInst::addHandler` appends a real operand.
        self.module
            .module()
            .context()
            .value_data(handler.slot())
            .add_use(ValueUse::Instruction(self.id));
        Ok(self)
    }
    #[inline]
    pub fn finish(self) -> CatchSwitchInst<'ctx, TermClosed, B> {
        self.retag()
    }
}

/// Is `op` a legal cast from `src` to `dst`? Port of
/// `CastInst::castIsValid` (`llvm/lib/IR/Instructions.cpp`).
///
/// Pure and infallible — a predicate, not a builder check (design law 7).
/// The parser, the constant-expression path and the verifier all ask the
/// same question, so there is one table.
///
/// Element counts do the scalar/vector agreement work: a scalar counts as 0
/// elements, so comparing counts also rejects scalar-to-vector and
/// vector-to-scalar without a separate arm — upstream's own trick.
pub fn cast_is_valid<'ctx, B: ModuleBrand + 'ctx>(
    op: CastOpcode,
    src: Type<'ctx, B>,
    dst: Type<'ctx, B>,
) -> bool {
    if !src.is_first_class() || !dst.is_first_class() || src.is_aggregate() || dst.is_aggregate() {
        return false;
    }

    let src_is_vector = src.is_vector();
    let dst_is_vector = dst.is_vector();
    let src_scalar_bits = src.scalar_size_in_bits();
    let dst_scalar_bits = dst.scalar_size_in_bits();
    let src_elements = if src_is_vector {
        src.vector_element_count()
    } else {
        None
    };
    let dst_elements = if dst_is_vector {
        dst.vector_element_count()
    } else {
        None
    };
    let counts_agree = src_elements == dst_elements;

    match op {
        CastOpcode::Trunc => {
            src.is_int_or_int_vector()
                && dst.is_int_or_int_vector()
                && counts_agree
                && src_scalar_bits > dst_scalar_bits
        }
        CastOpcode::Zext | CastOpcode::Sext => {
            src.is_int_or_int_vector()
                && dst.is_int_or_int_vector()
                && counts_agree
                && src_scalar_bits < dst_scalar_bits
        }
        CastOpcode::FpTrunc => {
            src.is_float_or_float_vector()
                && dst.is_float_or_float_vector()
                && counts_agree
                && src_scalar_bits > dst_scalar_bits
        }
        CastOpcode::FpExt => {
            src.is_float_or_float_vector()
                && dst.is_float_or_float_vector()
                && counts_agree
                && src_scalar_bits < dst_scalar_bits
        }
        CastOpcode::UiToFp | CastOpcode::SiToFp => {
            src.is_int_or_int_vector() && dst.is_float_or_float_vector() && counts_agree
        }
        CastOpcode::FpToUi | CastOpcode::FpToSi => {
            src.is_float_or_float_vector() && dst.is_int_or_int_vector() && counts_agree
        }
        CastOpcode::PtrToAddr | CastOpcode::PtrToInt => {
            counts_agree && src.is_ptr_or_ptr_vector() && dst.is_int_or_int_vector()
        }
        CastOpcode::IntToPtr => {
            counts_agree && src.is_int_or_int_vector() && dst.is_ptr_or_ptr_vector()
        }
        CastOpcode::BitCast => {
            let src_ptr = src.scalar_type().pointer_address_space();
            let dst_ptr = dst.scalar_type().pointer_address_space();
            // A pointer may only bitcast to a pointer, and vice versa.
            if src_ptr.is_some() != dst_ptr.is_some() {
                return false;
            }
            let Some(src_space) = src_ptr else {
                // Non-pointer: a no-op cast of type only, so the bit widths
                // must match exactly.
                return src.primitive_size_in_bits() == dst.primitive_size_in_bits();
            };
            if Some(src_space) != dst_ptr {
                return false;
            }
            match (src_is_vector, dst_is_vector) {
                (true, true) => counts_agree,
                (true, false) => src_elements == Some(1),
                (false, true) => dst_elements == Some(1),
                (false, false) => true,
            }
        }
        CastOpcode::AddrSpaceCast => {
            let (Some(src_space), Some(dst_space)) = (
                src.scalar_type().pointer_address_space(),
                dst.scalar_type().pointer_address_space(),
            ) else {
                return false;
            };
            if src_space == dst_space {
                return false;
            }
            counts_agree
        }
    }
}

/// The type a `getelementptr` index list arrives at, or `None` when the list
/// does not index into `source_ty`. Port of
/// `GetElementPtrInst::getIndexedType` (`llvm/lib/IR/Instructions.cpp`).
///
/// `indices` is the index list *without* the base pointer, exactly as upstream
/// passes it. The first index steps the pointer and is never applied to
/// `source_ty`, so an empty list arrives at `source_ty` itself — which is why
/// upstream's null check doubles as "these indices are valid".
pub fn indexed_gep_type<'ctx, B: ModuleBrand + 'ctx>(
    source_ty: Type<'ctx, B>,
    indices: &[Value<'ctx, B>],
) -> Option<Type<'ctx, B>> {
    let module = source_ty.module;
    let slots: Vec<_> = indices.iter().map(|index| index.slot()).collect();
    crate::constants::gep_indexed_type(module.module(), source_ty.id(), &slots)
        .map(|indexed| Type::new(indexed, module))
}

/// The type an `extractvalue` / `insertvalue` index list arrives at, or
/// `None` when the list does not index into `agg_ty`. Port of
/// `ExtractValueInst::getIndexedType` (`llvm/lib/IR/Instructions.cpp`), which
/// **rejects** rather than clamps an index at or past the element count.
///
/// Unlike [`indexed_gep_type`] every index applies to the aggregate itself —
/// there is no leading pointer step — so an empty list arrives at `agg_ty`.
///
/// Two private near-copies of this walk exist, in `ir_builder.rs` and
/// `verifier.rs`; consolidating them onto this one is recorded in
/// `docs/future-work.md`.
pub fn indexed_aggregate_type<'ctx, B: ModuleBrand + 'ctx>(
    agg_ty: Type<'ctx, B>,
    indices: &[u32],
) -> Option<Type<'ctx, B>> {
    use crate::AnyTypeEnum;
    let mut current = agg_ty;
    for &index in indices {
        current = match AnyTypeEnum::from(current) {
            AnyTypeEnum::Array(array) => {
                if u64::from(index) >= array.len() {
                    return None;
                }
                array.element()
            }
            AnyTypeEnum::Struct(structure) => structure.field_type(usize::try_from(index).ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IrError, Linkage};

    /// Locks `TypedCallInst::result` as the `CallResult` GAT's narrowing
    /// path: wrapping a raw `CallInst<'ctx, i32, B>` and reading
    /// `result()` back must yield an `IntValue<'ctx, i32, B>` that names
    /// the exact same underlying value (same `ValueSlot`) as the call
    /// instruction itself -- i.e. `result()` narrows the derived
    /// `CallResult` GAT without losing or renaming the value.
    ///
    /// Field-literal construction stands in for the crate-internal
    /// `TypedCallInst::from_call` minting constructor here: `from_call`
    /// gets its typed-callee-builder caller in a later revision, per the
    /// same defer-until-first-caller discipline `OverflowFlags::from_parts`
    /// follows (`from_call` has no caller yet, and
    /// clippy's dead-code lint fires on a `pub(super)` item even when
    /// its only caller is `#[cfg(test)]`-gated, since the non-test
    /// `(lib)` artifact `-D warnings` gates never sees `#[cfg(test)]`
    /// code at all).
    #[test]
    fn typed_call_inst_result_narrows_to_callresult() -> Result<(), IrError> {
        let m = crate::module_new!("typed-call-inst-result")?;
        let callee = m
            .add_typed_function::<i32, (), _>("callee", Linkage::External)?
            .as_function();
        let caller_ty = m.function_type_no_parameters(m.i32_type());
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = m.view(caller).append_basic_block(&m, "entry");
        let b = crate::IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

        let call: CallInst<'_, i32, _> =
            b.view(b.call_dyn(callee, Vec::<Value<'_, _>>::new(), "call")?);
        let call_id = call.to_erased().slot();

        let typed = TypedCallInst::<i32, _> {
            inner: call,
            _ret: core::marker::PhantomData,
        };
        let result = typed.result();

        assert_eq!(result.slot(), call_id);
        Ok(())
    }
}
