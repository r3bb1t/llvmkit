//! Per-opcode instruction handles. Mirrors a slice of
//! `llvm/include/llvm/IR/Instructions.h`.
//!
//! Each handle is a thin view onto an attached instruction in some basic
//! block. Internally it stores the `(ValueId, ModuleRef, TypeId)` triple ---
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
//! (D4 — `build_int_add::<W>` returns `IntValue<W>`), and the handles'
//! reachable constructors are rediscovery paths (`BlockCursor`,
//! `InstructionView`, `TryFrom`) which are inherently dyn-shaped — a
//! marker there would instantiate as `AddInst<IntDyn>` everywhere and
//! gate nothing.

use core::fmt;

use super::IrResult;
use super::align::Align;
use super::atomic_ordering::AtomicOrdering;
use super::atomicrmw_binop::AtomicRMWBinOp;
use super::basic_block::{BasicBlock, BasicBlockLabel, IntoBasicBlockLabel};
use super::block_state::Unterminated;
use super::calling_conv::CallingConv;
use super::cmp_predicate::{FloatPredicate, IntPredicate};
use super::derived_types::FunctionType;
use super::float_kind::{FloatKind, IntoFloatValue};
use super::fmf::FastMathFlags;
use super::function::FunctionValue;
use super::function_signature::{FunctionReturn, token::ValidatedCallResult};
use super::gep_no_wrap_flags::GepNoWrapFlags;
use super::instr_types::TailCallKind;
use super::instr_types::{
    BinaryOpData, BranchInstData, BranchKind, CastOpData, CastOpcode, CmpInstData,
    LandingPadClauseKind, PhiData, ReturnOpData,
};
use super::instruction::{InstructionKindData, InstructionView};
use super::int_width::{IntDyn, IntWidth, IntoIntValue};
use super::marker::{Dyn, Ptr, ReturnMarker};
use super::module::{Brand, Module, ModuleBrand, ModuleRef, Unverified};
use super::phi_state::{Closed, Open, PhiState};
use super::sync_scope::SyncScope;
use super::term_open_state::{Closed as TermClosed, Open as TermOpen, TermOpenState};
use super::r#type::{Type, TypeData, TypeId};
use super::value::{
    FloatValue, IntValue, IntoPointerValue, IsValue, PointerValue, Value, ValueId, ValueKindData,
    ValueUse,
};

macro_rules! decl_binop_handle {
    (
        $(#[$attr:meta])*
        $name:ident,
        $variant:ident
    ) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name<'ctx, B: ModuleBrand = Brand<'ctx>> {
            pub(super) id: ValueId,
            pub(super) module: ModuleRef<'ctx, B>,
            pub(super) ty: TypeId,
        }

        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            #[inline]
            pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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
            #[inline]
            pub fn as_value(&self) -> Value<'ctx, B> {
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
    UDivInst, UDiv
);
decl_binop_handle!(
    /// `sdiv` integer divide (signed).
    SDivInst, SDiv
);
decl_binop_handle!(
    /// `urem` integer remainder (unsigned).
    URemInst, URem
);
decl_binop_handle!(
    /// `srem` integer remainder (signed).
    SRemInst, SRem
);
decl_binop_handle!(
    /// `shl` logical left shift.
    ShlInst, Shl
);
decl_binop_handle!(
    /// `lshr` logical right shift.
    LShrInst, LShr
);
decl_binop_handle!(
    /// `ashr` arithmetic right shift.
    AShrInst, AShr
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
    FAddInst, FAdd
);
decl_binop_handle!(
    /// `fsub` floating-point subtract.
    FSubInst, FSub
);
decl_binop_handle!(
    /// `fmul` floating-point multiply.
    FMulInst, FMul
);
decl_binop_handle!(
    /// `fdiv` floating-point divide.
    FDivInst, FDiv
);
decl_binop_handle!(
    /// `frem` floating-point remainder.
    FRemInst, FRem
);

/// Common scaffolding used by every non-macro handle.
macro_rules! decl_handle_scaffold {
    ($name:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            #[inline]
            pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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
            #[inline]
            pub fn as_value(&self) -> Value<'ctx, B> {
                Value::from_parts(self.id, self.module, self.ty)
            }
        }
    };
}

/// `alloca` stack-slot allocation. Mirrors `AllocaInst`
/// (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllocaInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(AllocaInst);

impl<'ctx, B: ModuleBrand + 'ctx> AllocaInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::AllocaInstData {
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
}

/// `load` instruction. Mirrors `LoadInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoadInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(LoadInst);

impl<'ctx, B: ModuleBrand + 'ctx> LoadInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::LoadInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(StoreInst);

impl<'ctx, B: ModuleBrand + 'ctx> StoreInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::StoreInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GepInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(GepInst);

impl<'ctx, B: ModuleBrand + 'ctx> GepInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::GepInstData {
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
    /// Pointer operand. Statically a pointer for this opcode, so returned
    /// as [`PointerValue`] rather than the erased [`Value`].
    pub fn pointer(self) -> PointerValue<'ctx, B> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        PointerValue::from_value_unchecked(Value::from_parts(id, self.module, data.ty))
    }
    pub fn indices(self) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().indices.iter().map(|c| c.get()).collect();
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Callee<'ctx, B: ModuleBrand = Brand<'ctx>> {
    /// A direct call to a known function global.
    Direct(FunctionValue<'ctx, Dyn, B>),
    /// An indirect call through a function pointer.
    Indirect(PointerValue<'ctx, B>),
}

/// `call` instruction. Mirrors `CallInst` (`Instructions.h`).
///
/// The `R: ReturnMarker` parameter (default [`crate::Dyn`]) propagates
/// the callee's return shape, so a typed [`crate::IRBuilder::build_call_dyn`] for an `i32`
/// callee returns `CallInst<'ctx, i32>` and exposes a typed
/// `return_int_value()` accessor without a runtime
/// [`crate::IrError::TypeMismatch`].
#[derive(Debug)]
pub struct CallInst<'ctx, R: ReturnMarker = Dyn, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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
    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// Re-tag the return marker. Crate-internal: only [`build_call_dyn`]
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

    fn payload(self) -> &'ctx crate::instr_types::CallInstData {
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
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
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
call_inst_float_return!(
    f32,
    f64,
    crate::float_kind::Half,
    crate::float_kind::BFloat,
    crate::float_kind::Fp128,
    crate::float_kind::X86Fp80,
    crate::float_kind::PpcFp128,
    crate::float_kind::FloatDyn,
);

impl<'ctx, B: ModuleBrand + 'ctx> CallInst<'ctx, Ptr, B> {
    /// Typed result handle for a pointer-returning call.
    #[inline]
    pub fn return_pointer_value(self) -> PointerValue<'ctx, B> {
        crate::value::PointerValue::from_value_unchecked(Value::from_parts(
            self.id,
            self.module,
            self.ty,
        ))
    }
}

/// Call handle whose full return schema is carried at the type level.
/// The marker on the inner [`CallInst`] is `Ret::Marker` — derived from
/// the callee by [`crate::IRBuilder::build_call`], never caller-asserted.
pub struct TypedCallInst<'ctx, Ret, B: ModuleBrand = Brand<'ctx>>
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
    /// callee. Only the typed `build_call` family constructs this —
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

    /// Fully-erased handle (D3).
    #[inline]
    pub fn as_dyn(self) -> CallInst<'ctx, Dyn, B> {
        self.inner.as_dyn()
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        self.inner.as_value()
    }
}

/// `select` instruction. Mirrors `SelectInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(SelectInst);

impl<'ctx, B: ModuleBrand + 'ctx> SelectInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::SelectInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Select(s) => s,
                _ => unreachable!("SelectInst invariant: kind is Select"),
            },
            _ => unreachable!("SelectInst invariant: kind is Instruction"),
        }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RetInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name<'ctx, B: ModuleBrand = Brand<'ctx>> {
            pub(super) id: ValueId,
            pub(super) module: ModuleRef<'ctx, B>,
            pub(super) ty: TypeId,
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
    ZExtInst, ZExt
);
decl_cast_handle!(
    /// `sext .. to ..` — sign-extend an integer.
    SExtInst, SExt
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
    FpToUIInst, FpToUI
);
decl_cast_handle!(
    /// `fptosi .. to ..` — float to signed integer.
    FpToSIInst, FpToSI
);
decl_cast_handle!(
    /// `uitofp .. to ..` — unsigned integer to float.
    UIToFpInst, UIToFp
);
decl_cast_handle!(
    /// `sitofp .. to ..` — signed integer to float.
    SIToFpInst, SIToFp
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

/// `icmp` integer comparison. Mirrors `ICmpInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ICmpInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(ICmpInst);

impl<'ctx, B: ModuleBrand + 'ctx> ICmpInst<'ctx, B> {
    fn payload(self) -> &'ctx CmpInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::ICmp(c) => c,
                _ => unreachable!("ICmpInst invariant: kind is ICmp"),
            },
            _ => unreachable!("ICmpInst invariant: kind is Instruction"),
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

/// `fcmp` floating-point comparison. Mirrors `FCmpInst`
/// (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FCmpInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(FCmpInst);

impl<'ctx, B: ModuleBrand + 'ctx> FCmpInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::FCmpInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::FCmp(c) => c,
                _ => unreachable!("FCmpInst invariant: kind is FCmp"),
            },
            _ => unreachable!("FCmpInst invariant: kind is Instruction"),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BranchInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
        matches!(self.payload().kind, BranchKind::Conditional { .. })
    }
    pub fn condition(self) -> Option<Value<'ctx, B>> {
        match &self.payload().kind {
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
    pub(super) fn successor_ids(self) -> Vec<ValueId> {
        match &self.payload().kind {
            BranchKind::Unconditional(t) => vec![*t],
            BranchKind::Conditional {
                then_bb, else_bb, ..
            } => vec![*then_bb, *else_bb],
        }
    }
    /// Successors as copyable block labels.
    pub fn successors(self) -> impl ExactSizeIterator<Item = BasicBlockLabel<'ctx, Dyn, B>> + 'ctx {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        self.successor_ids().into_iter().map(move |id| {
            BasicBlock::<Dyn, Unterminated, B>::from_parts(id, self.module, label_ty).label()
        })
    }
}

/// `unreachable` terminator. Mirrors `UnreachableInst`
/// (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnreachableInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(UnreachableInst);

// --------------------------------------------------------------------------
// Phi
// --------------------------------------------------------------------------

/// `phi` node. Mirrors `PHINode` (`Instructions.h`). Mutable
/// `add_incoming` mirrors `PHINode::addIncoming`; the factorial
/// example needs it because the loop-edge incoming value is defined
/// later in the same block.
///
/// The `P: PhiState` parameter (default [`Open`]) tracks whether the
/// phi handle accepts `add_incoming` calls. Calling [`PhiInst::finish`] consumes
/// the open handle and returns a [`Closed`] handle; the closed handle exposes
/// only read accessors.
#[derive(Debug)]
pub struct PhiInst<'ctx, W: IntWidth, P: PhiState = Open, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    _w: core::marker::PhantomData<fn() -> W>,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, W: IntWidth, P: PhiState, B: ModuleBrand + 'ctx> PhiInst<'ctx, W, P, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _w: core::marker::PhantomData,
            _p: core::marker::PhantomData,
        }
    }

    /// Re-tag the phi-state marker. Crate-internal: only [`finish`]
    /// flips the public marker.
    #[inline]
    pub(super) fn retag<P2: PhiState>(self) -> PhiInst<'ctx, W, P2, B> {
        PhiInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _w: core::marker::PhantomData,
            _p: core::marker::PhantomData,
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

    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
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
    pub fn incoming(
        &self,
        index: u32,
    ) -> IrResult<(Value<'ctx, B>, BasicBlockLabel<'ctx, Dyn, B>)> {
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
        let label_ty = module.label_type().as_type().id();
        let block =
            BasicBlock::<Dyn, Unterminated, B>::from_parts(bid, self.module, label_ty).label();
        Ok((value, block))
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> PhiInst<'ctx, W, Open, B> {
    /// Append `(value, block)` to the incoming list. Mirrors
    /// `PHINode::addIncoming`. Returns `Self` so calls chain.
    /// Errors if `value`'s type does not match the phi's result type.
    /// The block's module provenance is carried by its branded handle in
    /// ordinary construction paths; CFG predecessor completeness is verified by
    /// [`Module::verify`](crate::Module::verify).
    pub fn add_incoming<V, R, Block>(self, value: V, block: Block) -> IrResult<Self>
    where
        V: IntoIntValue<'ctx, W, B>,
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let value = value.into_int_value(self.module)?;
        if value.as_value().ty == self.ty {
            let value_id = value.as_value().id;
            let block_id = block.into_basic_block_label().as_value().id;
            self.payload()
                .incoming
                .borrow_mut()
                .push((core::cell::Cell::new(value_id), block_id));
            // Register the phi as a user of the incoming value.
            module
                .context()
                .value_data(value_id)
                .use_list
                .borrow_mut()
                .push(ValueUse::Instruction(self.id));
            Ok(self)
        } else {
            Err(crate::IrError::TypeMismatch {
                expected: Type::new(self.ty, module).kind_label(),
                got: value.as_value().ty().kind_label(),
            })
        }
    }

    /// Consume the open phi and return its [`Closed`] view. After
    /// `finish`, `add_incoming` is no longer in scope at the type
    /// level; the closed handle exposes only read accessors. Mirrors
    /// the implicit "phi is finalised" convention upstream where the
    /// verifier subsequently runs `Verifier::visitPHINode`.
    #[inline]
    pub fn finish(self) -> PhiInst<'ctx, W, Closed, B> {
        self.retag::<Closed>()
    }
}

impl<'ctx, W: IntWidth, P: PhiState, B: ModuleBrand> PartialEq for PhiInst<'ctx, W, P, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth, P: PhiState, B: ModuleBrand> Eq for PhiInst<'ctx, W, P, B> {}
impl<'ctx, W: IntWidth, P: PhiState> core::hash::Hash for PhiInst<'ctx, W, P> {
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
#[derive(Debug)]
pub struct FpPhiInst<'ctx, K: FloatKind, P: PhiState = Open, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    _k: core::marker::PhantomData<fn() -> K>,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, K: FloatKind, P: PhiState, B: ModuleBrand + 'ctx> FpPhiInst<'ctx, K, P, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _k: core::marker::PhantomData,
            _p: core::marker::PhantomData,
        }
    }

    #[inline]
    pub(super) fn retag<P2: PhiState>(self) -> FpPhiInst<'ctx, K, P2, B> {
        FpPhiInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _k: core::marker::PhantomData,
            _p: core::marker::PhantomData,
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

    #[inline]
    pub fn as_view(&self) -> InstructionView<'ctx, B> {
        InstructionView::from_parts(self.id, self.module)
    }

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
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
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> FpPhiInst<'ctx, K, Open, B> {
    /// Append `(value, block)` to the incoming list. Mirrors
    /// `PHINode::addIncoming`. Errors if `value`'s type does not match
    /// the phi's result type. The block's module provenance is carried by its
    /// branded handle in ordinary construction paths; CFG predecessor
    /// completeness is verified by [`Module::verify`](crate::Module::verify).
    pub fn add_incoming<V, R, Block>(self, value: V, block: Block) -> IrResult<Self>
    where
        V: IntoFloatValue<'ctx, K, B>,
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let value = value.into_float_value(self.module)?;
        if value.as_value().ty == self.ty {
            let value_id = value.as_value().id;
            let block_id = block.into_basic_block_label().as_value().id;
            self.payload()
                .incoming
                .borrow_mut()
                .push((core::cell::Cell::new(value_id), block_id));
            module
                .context()
                .value_data(value_id)
                .use_list
                .borrow_mut()
                .push(ValueUse::Instruction(self.id));
            Ok(self)
        } else {
            Err(crate::IrError::TypeMismatch {
                expected: Type::new(self.ty, module).kind_label(),
                got: value.as_value().ty().kind_label(),
            })
        }
    }

    /// Consume the open phi and return its [`Closed`] view.
    #[inline]
    pub fn finish(self) -> FpPhiInst<'ctx, K, Closed, B> {
        self.retag::<Closed>()
    }
}

impl<'ctx, K: FloatKind, P: PhiState, B: ModuleBrand> PartialEq for FpPhiInst<'ctx, K, P, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind, P: PhiState, B: ModuleBrand> Eq for FpPhiInst<'ctx, K, P, B> {}
impl<'ctx, K: FloatKind, P: PhiState, B: ModuleBrand> core::hash::Hash
    for FpPhiInst<'ctx, K, P, B>
{
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
/// the type id), so the handle is parameterised only by `P: PhiState`.
#[derive(Debug)]
pub struct PointerPhiInst<'ctx, P: PhiState = Open, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: PhiState, B: ModuleBrand + 'ctx> PointerPhiInst<'ctx, P, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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
    pub(super) fn retag<P2: PhiState>(self) -> PointerPhiInst<'ctx, P2, B> {
        PointerPhiInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
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

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// Result handle for the phi, narrowed to a [`PointerValue`].
    #[inline]
    pub fn as_pointer_value(&self) -> PointerValue<'ctx, B> {
        let v = Value::from_parts(self.id, self.module, self.ty);
        crate::value::PointerValue::from_value_unchecked(v)
    }

    pub fn incoming_count(&self) -> u32 {
        let len = self.payload().incoming.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> PointerPhiInst<'ctx, Open, B> {
    /// Append `(value, block)` to the incoming list.
    pub fn add_incoming<V, R, Block>(self, value: V, block: Block) -> IrResult<Self>
    where
        V: IntoPointerValue<'ctx, B>,
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let value = value.into_pointer_value(self.module)?;
        if value.as_value().ty == self.ty {
            let value_id = value.as_value().id;
            let block_id = block.into_basic_block_label().as_value().id;
            self.payload()
                .incoming
                .borrow_mut()
                .push((core::cell::Cell::new(value_id), block_id));
            module
                .context()
                .value_data(value_id)
                .use_list
                .borrow_mut()
                .push(ValueUse::Instruction(self.id));
            Ok(self)
        } else {
            Err(crate::IrError::TypeMismatch {
                expected: Type::new(self.ty, module).kind_label(),
                got: IsValue::as_value(value).ty().kind_label(),
            })
        }
    }

    /// Consume the open phi and return its [`Closed`] view.
    #[inline]
    pub fn finish(self) -> PointerPhiInst<'ctx, Closed, B> {
        self.retag::<Closed>()
    }
}

impl<'ctx, P: PhiState, B: ModuleBrand> PartialEq for PointerPhiInst<'ctx, P, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: PhiState, B: ModuleBrand> Eq for PointerPhiInst<'ctx, P, B> {}
impl<'ctx, P: PhiState> core::hash::Hash for PointerPhiInst<'ctx, P> {
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
#[derive(Debug, Clone, Copy)]
pub struct OtherPhiInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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

    /// Number of incoming `(value, block)` edges.
    pub fn incoming_count(&self) -> u32 {
        let len = self.payload().incoming.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"))
    }

    /// Read the `(value, block label)` pair at `index`.
    pub fn incoming(
        &self,
        index: u32,
    ) -> IrResult<(Value<'ctx, B>, BasicBlockLabel<'ctx, Dyn, B>)> {
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
        let label_ty = module.label_type().as_type().id();
        let block =
            BasicBlock::<Dyn, Unterminated, B>::from_parts(bid, self.module, label_ty).label();
        Ok((value, block))
    }
}

// --------------------------------------------------------------------------
// Unary ops: fneg / freeze / va_arg
// --------------------------------------------------------------------------

/// `fneg` floating-point negate. Mirrors `UnaryOperator::FNeg` in
/// `InstrTypes.h`. Carries [`crate::FastMathFlags`] like every
/// `FPMathOperator`-class instruction (`Operator.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FNegInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(FNegInst);

impl<'ctx, B: ModuleBrand + 'ctx> FNegInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::FNegInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::FNeg(u) => u,
                _ => unreachable!("FNegInst invariant: kind is FNeg"),
            },
            _ => unreachable!("FNegInst invariant: kind is Instruction"),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FreezeInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(FreezeInst);

impl<'ctx, B: ModuleBrand + 'ctx> FreezeInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::FreezeInstData {
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

/// `va_arg` instruction. Mirrors `VAArgInst` (`Instructions.h`).
/// Loads the next argument from a `va_list` pointer; the destination
/// type lives on [`Self::result_type`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VAArgInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(VAArgInst);

impl<'ctx, B: ModuleBrand + 'ctx> VAArgInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::VAArgInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::VAArg(u) => u,
                _ => unreachable!("VAArgInst invariant: kind is VAArg"),
            },
            _ => unreachable!("VAArgInst invariant: kind is Instruction"),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExtractValueInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(ExtractValueInst);

impl<'ctx, B: ModuleBrand + 'ctx> ExtractValueInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::ExtractValueInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InsertValueInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(InsertValueInst);

impl<'ctx, B: ModuleBrand + 'ctx> InsertValueInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::InsertValueInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExtractElementInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(ExtractElementInst);

impl<'ctx, B: ModuleBrand + 'ctx> ExtractElementInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::ExtractElementInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InsertElementInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(InsertElementInst);

impl<'ctx, B: ModuleBrand + 'ctx> InsertElementInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::InsertElementInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShuffleVectorInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(ShuffleVectorInst);

impl<'ctx, B: ModuleBrand + 'ctx> ShuffleVectorInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::ShuffleVectorInstData {
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
    /// `-1` ([`crate::instr_types::POISON_MASK_ELEM`]) marks poison entries.
    pub fn mask(self) -> &'ctx [i32] {
        &self.payload().mask
    }
}

// --------------------------------------------------------------------------
// Atomic ops: fence / cmpxchg / atomicrmw
// --------------------------------------------------------------------------

/// `fence` instruction. Mirrors `FenceInst` (`Instructions.h`).
/// No SSA operands; carries memory ordering and synchronization scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FenceInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(FenceInst);

impl<'ctx, B: ModuleBrand + 'ctx> FenceInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::FenceInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtomicCmpXchgInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(AtomicCmpXchgInst);

impl<'ctx, B: ModuleBrand + 'ctx> AtomicCmpXchgInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::AtomicCmpXchgInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtomicRMWInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(AtomicRMWInst);

impl<'ctx, B: ModuleBrand + 'ctx> AtomicRMWInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::AtomicRMWInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::AtomicRMW(d) => d,
                _ => unreachable!("AtomicRMWInst invariant: kind is AtomicRMW"),
            },
            _ => unreachable!("AtomicRMWInst invariant: kind is Instruction"),
        }
    }
    pub fn operation(self) -> AtomicRMWBinOp {
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
    /// module token: like [`Instruction::replace_all_uses_with`], this
    /// mutates the IR and must not be reachable without proof of
    /// mutation capability. `module_token` is the capability witness; the
    /// interior-mutable slot is reached through the handle's own
    /// `ModuleRef`.
    pub fn set_value_operand(
        self,
        module_token: &Module<'ctx, B, Unverified>,
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
            .use_list
            .borrow_mut()
            .push(ValueUse::Instruction(self.id));
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
#[derive(Debug)]
pub struct SwitchInst<'ctx, P: TermOpenState = TermOpen, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: TermOpenState, B: ModuleBrand> PartialEq for SwitchInst<'ctx, P, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: TermOpenState, B: ModuleBrand> Eq for SwitchInst<'ctx, P, B> {}
impl<'ctx, P: TermOpenState, B: ModuleBrand> core::hash::Hash for SwitchInst<'ctx, P, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: TermOpenState, B: ModuleBrand + 'ctx> SwitchInst<'ctx, P, B> {
    #[inline]
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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
    pub(super) fn retag<P2: TermOpenState>(self) -> SwitchInst<'ctx, P2, B> {
        SwitchInst {
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

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx crate::instr_types::SwitchInstData {
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
    pub fn default_destination(&self) -> BasicBlockLabel<'ctx, Dyn, B> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        BasicBlock::<Dyn, Unterminated, B>::from_parts(
            self.payload().default_bb.get(),
            self.module,
            label_ty,
        )
        .label()
    }
    pub fn case_count(&self) -> u32 {
        let len = self.payload().cases.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("switch has more than u32::MAX cases"))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> SwitchInst<'ctx, TermOpen, B> {
    /// Append a `(case_value, target)` entry to the switch. Mirrors
    /// `SwitchInst::addCase`. Returns `Self` so calls chain.
    pub fn add_case<V, R, Target>(self, case_value: V, target: Target) -> IrResult<Self>
    where
        V: IsValue<'ctx, B>,
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module = self.module.module();
        let v = case_value.as_value();
        let cond_ty = self.payload().cond.get();
        let cond_ty = module.context().value_data(cond_ty).ty;
        if v.ty != cond_ty {
            return Err(crate::IrError::TypeMismatch {
                expected: Type::new(cond_ty, module).kind_label(),
                got: v.ty().kind_label(),
            });
        }
        let v_id = v.id;
        let bb_id = target.into_basic_block_label().as_value().id;
        self.payload()
            .cases
            .borrow_mut()
            .push((core::cell::Cell::new(v_id), bb_id));
        // Register the switch as a user of the case value.
        module
            .context()
            .value_data(v_id)
            .use_list
            .borrow_mut()
            .push(ValueUse::Instruction(self.id));
        Ok(self)
    }
    /// Consume the open switch and return its [`Closed`] view. Mirrors
    /// the implicit "switch is finalised" convention upstream where
    /// the verifier subsequently runs `Verifier::visitSwitchInst`.
    #[inline]
    pub fn finish(self) -> SwitchInst<'ctx, TermClosed, B> {
        self.retag()
    }
}

/// `indirectbr` terminator. Mirrors `IndirectBrInst`
/// (`Instructions.h`). The address operand selects one of the
/// declared destination blocks at runtime.
#[derive(Debug)]
pub struct IndirectBrInst<'ctx, P: TermOpenState = TermOpen, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx crate::instr_types::IndirectBrInstData {
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
}

impl<'ctx, B: ModuleBrand + 'ctx> IndirectBrInst<'ctx, TermOpen, B> {
    /// Append a destination block. Mirrors `IndirectBrInst::addDestination`.
    pub fn add_destination<R, Target>(self, target: Target) -> IrResult<Self>
    where
        R: ReturnMarker,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.payload()
            .destinations
            .borrow_mut()
            .push(target.into_basic_block_label().as_value().id);
        Ok(self)
    }
    /// Consume the open `indirectbr` and return its [`Closed`] view.
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
#[derive(Debug)]
pub struct InvokeInst<'ctx, R: ReturnMarker = Dyn, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    /// Re-tag the return marker. Crate-internal: both
    /// [`crate::IRBuilder::build_invoke_dyn`] (caller-asserted `R2`) and
    /// the typed [`crate::IRBuilder::build_invoke`] (marker derived
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
    fn payload(self) -> &'ctx crate::instr_types::InvokeInstData {
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
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
    pub fn calling_conv(self) -> CallingConv {
        self.payload().calling_conv
    }
    pub fn normal_destination(self) -> BasicBlockLabel<'ctx, Dyn, B> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        BasicBlock::<Dyn, Unterminated, B>::from_parts(
            self.payload().normal_dest.get(),
            self.module,
            label_ty,
        )
        .label()
    }
    pub fn unwind_destination(self) -> BasicBlockLabel<'ctx, Dyn, B> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        BasicBlock::<Dyn, Unterminated, B>::from_parts(
            self.payload().unwind_dest.get(),
            self.module,
            label_ty,
        )
        .label()
    }
}

/// `callbr` terminator. Mirrors `CallBrInst` (`Instructions.h`).
/// A call-like terminator with one fallthrough destination plus zero
/// or more indirect destination labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallBrInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(CallBrInst);

impl<'ctx, B: ModuleBrand + 'ctx> CallBrInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::CallBrInstData {
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
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
    pub fn calling_conv(self) -> CallingConv {
        self.payload().calling_conv
    }
    pub fn default_destination(self) -> BasicBlockLabel<'ctx, Dyn, B> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        BasicBlock::<Dyn, Unterminated, B>::from_parts(
            self.payload().default_dest.get(),
            self.module,
            label_ty,
        )
        .label()
    }
    pub fn indirect_destinations(
        self,
    ) -> impl ExactSizeIterator<Item = BasicBlockLabel<'ctx, Dyn, B>> + 'ctx {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        let ids: Vec<ValueId> = self
            .payload()
            .indirect_dests
            .iter()
            .map(|c| c.get())
            .collect();
        ids.into_iter().map(move |id| {
            BasicBlock::<Dyn, Unterminated, B>::from_parts(id, self.module, label_ty).label()
        })
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
#[derive(Debug)]
pub struct LandingPadInst<'ctx, P: TermOpenState = TermOpen, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx crate::instr_types::LandingPadInstData {
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
}

impl<'ctx, B: ModuleBrand + 'ctx> LandingPadInst<'ctx, TermOpen, B> {
    /// Mark this landingpad as a cleanup. Mirrors `LandingPadInst::setCleanup(true)`.
    pub fn set_cleanup(self) -> Self {
        self.payload().cleanup.set(true);
        self
    }
    /// Append a `catch <ty> <val>` clause. Mirrors `LandingPadInst::addClause`
    /// for `Catch`.
    pub fn add_catch_clause<V: IsValue<'ctx, B>>(self, type_info: V) -> IrResult<Self> {
        let module = self.module.module();
        let v = type_info.as_value();
        self.payload()
            .clauses
            .borrow_mut()
            .push((LandingPadClauseKind::Catch, core::cell::Cell::new(v.id)));
        module
            .context()
            .value_data(v.id)
            .use_list
            .borrow_mut()
            .push(ValueUse::Instruction(self.id));
        Ok(self)
    }
    /// Append a `filter <ty> <val>` clause.
    pub fn add_filter_clause<V: IsValue<'ctx, B>>(self, filter_array: V) -> IrResult<Self> {
        let module = self.module.module();
        let v = filter_array.as_value();
        self.payload()
            .clauses
            .borrow_mut()
            .push((LandingPadClauseKind::Filter, core::cell::Cell::new(v.id)));
        module
            .context()
            .value_data(v.id)
            .use_list
            .borrow_mut()
            .push(ValueUse::Instruction(self.id));
        Ok(self)
    }
    /// Consume the open landingpad and return its [`Closed`] view.
    #[inline]
    pub fn finish(self) -> LandingPadInst<'ctx, TermClosed, B> {
        self.retag()
    }
}

/// `resume` terminator. Mirrors `ResumeInst` (`Instructions.h`).
/// Single value operand (typically a `landingpad` result).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResumeInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(ResumeInst);

impl<'ctx, B: ModuleBrand + 'ctx> ResumeInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::ResumeInstData {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CleanupPadInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(CleanupPadInst);

impl<'ctx, B: ModuleBrand + 'ctx> CleanupPadInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::CleanupPadInstData {
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
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
}

/// `catchpad` instruction. Mirrors `CatchPadInst` (`Instructions.h`).
/// Result is a `token`-typed value used as a funclet pad. Parent must
/// be a `catchswitch` (verifier rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatchPadInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(CatchPadInst);

impl<'ctx, B: ModuleBrand + 'ctx> CatchPadInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::CatchPadInstData {
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
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, self.module, data.ty)
        })
    }
}

/// `catchret` terminator. Mirrors `CatchReturnInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatchReturnInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(CatchReturnInst);

impl<'ctx, B: ModuleBrand + 'ctx> CatchReturnInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::CatchReturnInstData {
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
    pub fn target(self) -> BasicBlockLabel<'ctx, Dyn, B> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        BasicBlock::<Dyn, Unterminated, B>::from_parts(
            self.payload().target_bb,
            self.module,
            label_ty,
        )
        .label()
    }
}

/// `cleanupret` terminator. Mirrors `CleanupReturnInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CleanupReturnInst<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

decl_handle_scaffold!(CleanupReturnInst);

impl<'ctx, B: ModuleBrand + 'ctx> CleanupReturnInst<'ctx, B> {
    fn payload(self) -> &'ctx crate::instr_types::CleanupReturnInstData {
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
    pub fn unwind_dest(self) -> Option<BasicBlockLabel<'ctx, Dyn, B>> {
        let id = self.payload().unwind_dest?;
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        Some(BasicBlock::<Dyn, Unterminated, B>::from_parts(id, self.module, label_ty).label())
    }
}

/// `catchswitch` terminator. Mirrors `CatchSwitchInst` (`Instructions.h`).
/// Variable-arity handler list with optional unwind destination.
#[derive(Debug)]
pub struct CatchSwitchInst<'ctx, P: TermOpenState = TermOpen, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
    pub(super) fn from_raw<M>(id: ValueId, module: M, ty: TypeId) -> Self
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

    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }
    fn payload(&self) -> &'ctx crate::instr_types::CatchSwitchInstData {
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
    pub fn unwind_dest(&self) -> Option<BasicBlockLabel<'ctx, Dyn, B>> {
        let id = self.payload().unwind_dest.get()?;
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        Some(BasicBlock::<Dyn, Unterminated, B>::from_parts(id, self.module, label_ty).label())
    }
    pub fn handler_count(&self) -> u32 {
        let len = self.payload().handlers.borrow().len();
        u32::try_from(len)
            .unwrap_or_else(|_| unreachable!("catchswitch has more than u32::MAX handlers"))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> CatchSwitchInst<'ctx, TermOpen, B> {
    pub fn add_handler<R, Handler>(self, handler: Handler) -> IrResult<Self>
    where
        R: ReturnMarker,
        Handler: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.payload()
            .handlers
            .borrow_mut()
            .push(handler.into_basic_block_label().as_value().id);
        Ok(self)
    }
    #[inline]
    pub fn finish(self) -> CatchSwitchInst<'ctx, TermClosed, B> {
        self.retag()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IrError, Linkage, Module};

    /// Locks `TypedCallInst::result` as the `CallResult` GAT's narrowing
    /// path: wrapping a raw `CallInst<'ctx, i32, B>` and reading
    /// `result()` back must yield an `IntValue<'ctx, i32, B>` that names
    /// the exact same underlying value (same `ValueId`) as the call
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
        Module::with_new("typed-call-inst-result", |m| {
            let fn_ty = m.fn_type(m.i32_type(), Vec::<Type>::new(), false);
            let callee = m.add_function::<i32, _>("callee", fn_ty, Linkage::External)?;
            let caller_ty = m.fn_type(m.i32_type(), Vec::<Type>::new(), false);
            let caller = m.add_function::<i32, _>("caller", caller_ty, Linkage::External)?;
            let entry = caller.append_basic_block(&m, "entry");
            let b = crate::IRBuilder::new_for::<i32>(&m).position_at_end(entry);

            let call: CallInst<'_, i32, _> =
                b.build_call_dyn(callee, Vec::<Value<'_, _>>::new(), "call")?;
            let call_id = call.as_value().id();

            let typed = TypedCallInst::<i32, _> {
                inner: call,
                _ret: core::marker::PhantomData,
            };
            let result = typed.result();

            assert_eq!(result.as_value().id(), call_id);
            Ok(())
        })
    }
}
