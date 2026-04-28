//! Per-opcode instruction handles. Mirrors a slice of
//! `llvm/include/llvm/IR/Instructions.h`.
//!
//! Each handle is a thin `Copy` view onto an attached instruction in
//! some basic block. Internally it stores the `(ValueId, ModuleRef,
//! TypeId)` triple --- the same shape `Value` uses --- so it does not
//! depend on [`Instruction`]'s `!Copy` lifecycle handle. To get a
//! single-use lifecycle handle, call `as_instruction()` on the per-opcode handle;
//! the resulting [`Instruction<'ctx, state::Attached>`] is `!Copy` and
//! can drive `erase_from_parent` / `detach_from_parent` / RAUW.

use crate::instr_types::{
    BinaryOpData, BranchInstData, BranchKind, CastOpData, CastOpcode, CmpInstData, PhiData,
    ReturnOpData,
};
use crate::instruction::{Instruction, InstructionKindData, state};
use crate::module::{Module, ModuleRef};
use crate::phi_state::{Closed, Open, PhiState};
use crate::r#type::TypeId;
use crate::value::{Value, ValueId, ValueKindData};

macro_rules! decl_binop_handle {
    (
        $(#[$attr:meta])*
        $name:ident,
        $variant:ident
    ) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name<'ctx> {
            pub(crate) id: ValueId,
            pub(crate) module: ModuleRef<'ctx>,
            pub(crate) ty: TypeId,
        }

        impl<'ctx> $name<'ctx> {
            #[inline]
            pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
                Self { id, module: ModuleRef::new(module), ty }
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

            /// Materialise a single-use lifecycle handle for this
            /// instruction. The returned `Instruction<Attached>` is
            /// `!Copy`; the caller may call `erase_from_parent`,
            /// `detach_from_parent`, or `replace_all_uses_with` exactly
            /// once on the binding.
            #[inline]
            pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
                Instruction::from_parts(self.id, self.module.module())
            }

            /// Left-hand side operand. Mirrors `getOperand(0)`.
            pub fn lhs(self) -> Value<'ctx> {
                let id = self.payload().lhs.get();
                let module = self.module.module();
                let data = module.context().value_data(id);
                Value::from_parts(id, module, data.ty)
            }

            /// Right-hand side operand. Mirrors `getOperand(1)`.
            pub fn rhs(self) -> Value<'ctx> {
                let id = self.payload().rhs.get();
                let module = self.module.module();
                let data = module.context().value_data(id);
                Value::from_parts(id, module, data.ty)
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

        impl<'ctx> ::core::convert::From<$name<'ctx>> for Instruction<'ctx, state::Attached> {
            #[inline]
            fn from(h: $name<'ctx>) -> Self { h.as_instruction() }
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
        impl<'ctx> $name<'ctx> {
            #[inline]
            pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
                Self {
                    id,
                    module: ModuleRef::new(module),
                    ty,
                }
            }

            /// Materialise a single-use lifecycle handle. See
            /// [`AddInst::as_instruction`] for semantics.
            #[inline]
            pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
                Instruction::from_parts(self.id, self.module.module())
            }
        }

        impl<'ctx> ::core::convert::From<$name<'ctx>> for Instruction<'ctx, state::Attached> {
            #[inline]
            fn from(h: $name<'ctx>) -> Self {
                h.as_instruction()
            }
        }
    };
}

/// `alloca` stack-slot allocation. Mirrors `AllocaInst`
/// (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllocaInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(AllocaInst);

impl<'ctx> AllocaInst<'ctx> {
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
    pub fn allocated_type(self) -> crate::r#type::Type<'ctx> {
        crate::r#type::Type::new(self.payload().allocated_ty, self.module.module())
    }
    /// Optional element-count operand (`alloca i32, i32 %n`).
    pub fn array_size(self) -> Option<Value<'ctx>> {
        let id = self.payload().num_elements.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }
    /// Explicit alignment, if any.
    pub fn align(self) -> Option<crate::align::Align> {
        self.payload().align.align()
    }
    /// Address space of the result pointer.
    pub fn addr_space(self) -> u32 {
        self.payload().addr_space
    }
}

/// `load` instruction. Mirrors `LoadInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoadInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(LoadInst);

impl<'ctx> LoadInst<'ctx> {
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
    pub fn pointer(self) -> Value<'ctx> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn align(self) -> Option<crate::align::Align> {
        self.payload().align.align()
    }
    pub fn is_volatile(self) -> bool {
        self.payload().volatile
    }
}

/// `store` instruction. Mirrors `StoreInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(StoreInst);

impl<'ctx> StoreInst<'ctx> {
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
    pub fn value_operand(self) -> Value<'ctx> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn pointer(self) -> Value<'ctx> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn align(self) -> Option<crate::align::Align> {
        self.payload().align.align()
    }
    pub fn is_volatile(self) -> bool {
        self.payload().volatile
    }
}

/// `getelementptr` instruction. Mirrors `GetElementPtrInst`
/// (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GepInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(GepInst);

impl<'ctx> GepInst<'ctx> {
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
    pub fn source_element_type(self) -> crate::r#type::Type<'ctx> {
        crate::r#type::Type::new(self.payload().source_ty, self.module.module())
    }
    pub fn pointer(self) -> Value<'ctx> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn indices(self) -> impl ExactSizeIterator<Item = Value<'ctx>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().indices.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, module, data.ty)
        })
    }
    pub fn flags(self) -> crate::gep_no_wrap_flags::GepNoWrapFlags {
        self.payload().flags
    }
}

/// `call` instruction. Mirrors `CallInst` (`Instructions.h`).
///
/// The `R: ReturnMarker` parameter (default [`crate::Dyn`]) propagates
/// the callee's return shape, so a typed [`crate::IRBuilder::build_call`] for an `i32`
/// callee returns `CallInst<'ctx, i32>` and exposes a typed
/// `return_int_value()` accessor without a runtime
/// [`crate::IrError::TypeMismatch`].
#[derive(Debug)]
pub struct CallInst<'ctx, R: crate::marker::ReturnMarker = crate::marker::Dyn> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    _r: core::marker::PhantomData<R>,
}

impl<'ctx, R: crate::marker::ReturnMarker> Clone for CallInst<'ctx, R> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: crate::marker::ReturnMarker> Copy for CallInst<'ctx, R> {}
impl<'ctx, R: crate::marker::ReturnMarker> PartialEq for CallInst<'ctx, R> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: crate::marker::ReturnMarker> Eq for CallInst<'ctx, R> {}
impl<'ctx, R: crate::marker::ReturnMarker> core::hash::Hash for CallInst<'ctx, R> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, R: crate::marker::ReturnMarker> CallInst<'ctx, R> {
    #[inline]
    pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _r: core::marker::PhantomData,
        }
    }

    /// Materialise a single-use lifecycle handle.
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
        Instruction::from_parts(self.id, self.module.module())
    }

    /// Re-tag the return marker. Crate-internal: only [`build_call`]
    /// flows the typed marker; [`as_dyn`] erases it.
    #[inline]
    pub(crate) fn retag<R2: crate::marker::ReturnMarker>(self) -> CallInst<'ctx, R2> {
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
    pub fn as_dyn(self) -> CallInst<'ctx, crate::marker::Dyn> {
        self.retag::<crate::marker::Dyn>()
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
    /// Callee operand (typically a `FunctionValue`, but a function-
    /// pointer value also fits here).
    pub fn callee(self) -> Value<'ctx> {
        let id = self.payload().callee.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    /// Function-type of the call (`FunctionType<'ctx>`).
    pub fn function_type(self) -> crate::derived_types::FunctionType<'ctx> {
        crate::derived_types::FunctionType::new(self.payload().fn_ty, self.module.module())
    }
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, module, data.ty)
        })
    }
    pub fn calling_conv(self) -> crate::CallingConv {
        self.payload().calling_conv
    }
    pub fn tail_call_kind(self) -> crate::instr_types::TailCallKind {
        self.payload().tail_kind
    }
    /// Return value, or `None` for a void-returning callee. Available
    /// on every `R`; the typed `return_int_value` /
    /// `return_float_value` / `return_pointer_value` accessors below
    /// are gated to the corresponding marker so a typed callee skips
    /// the runtime narrowing.
    pub fn return_value(self) -> Option<Value<'ctx>> {
        let module = self.module.module();
        let ret_ty_data = module.context().type_data(self.ty);
        if matches!(ret_ty_data, crate::r#type::TypeData::Void) {
            None
        } else {
            Some(Value::from_parts(self.id, module, self.ty))
        }
    }
}

// Typed-return accessors. Each impl is gated on the concrete return
// marker so a `CallInst<'ctx, i32>` exposes `return_int_value` but not
// `return_float_value`, and a `CallInst<'ctx, ()>` exposes neither.
macro_rules! call_inst_int_return {
    ($($w:ty),+ $(,)?) => { $(
        impl<'ctx> CallInst<'ctx, $w> {
            /// Typed result handle for an integer-returning call.
            #[inline]
            pub fn return_int_value(self) -> crate::value::IntValue<'ctx, $w> {
                let v = Value::from_parts(self.id, self.module.module(), self.ty);
                crate::value::IntValue::<$w>::from_value_unchecked(v)
            }
        }
    )+ };
}
call_inst_int_return!(bool, i8, i16, i32, i64, i128, crate::int_width::IntDyn);

macro_rules! call_inst_float_return {
    ($($k:ty),+ $(,)?) => { $(
        impl<'ctx> CallInst<'ctx, $k> {
            /// Typed result handle for a float-returning call.
            #[inline]
            pub fn return_float_value(self) -> crate::value::FloatValue<'ctx, $k> {
                let v = Value::from_parts(self.id, self.module.module(), self.ty);
                crate::value::FloatValue::<$k>::from_value_unchecked(v)
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

impl<'ctx> CallInst<'ctx, crate::marker::Ptr> {
    /// Typed result handle for a pointer-returning call.
    #[inline]
    pub fn return_pointer_value(self) -> crate::value::PointerValue<'ctx> {
        crate::value::PointerValue::from_value_unchecked(Value::from_parts(
            self.id,
            self.module.module(),
            self.ty,
        ))
    }
}

impl<'ctx, R: crate::marker::ReturnMarker> From<CallInst<'ctx, R>>
    for Instruction<'ctx, state::Attached>
{
    #[inline]
    fn from(h: CallInst<'ctx, R>) -> Self {
        h.as_instruction()
    }
}

/// `select` instruction. Mirrors `SelectInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(SelectInst);

impl<'ctx> SelectInst<'ctx> {
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
    pub fn condition(self) -> Value<'ctx> {
        let id = self.payload().cond.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn true_value(self) -> Value<'ctx> {
        let id = self.payload().true_val.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn false_value(self) -> Value<'ctx> {
        let id = self.payload().false_val.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

/// `ret` terminator instruction. Mirrors `ReturnInst` in
/// `Instructions.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RetInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(RetInst);

impl<'ctx> RetInst<'ctx> {
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
    pub fn return_value(self) -> Option<Value<'ctx>> {
        let id = self.payload().value.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }
}

/// Cast instruction (`trunc`, `zext`, `sext`, `bitcast`, ...).
/// Mirrors `CastInst` in `InstrTypes.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CastInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(CastInst);

impl<'ctx> CastInst<'ctx> {
    fn payload(self) -> &'ctx CastOpData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Cast(c) => c,
                _ => unreachable!("CastInst invariant: kind is Cast"),
            },
            _ => unreachable!("CastInst invariant: kind is Instruction"),
        }
    }
    /// Cast opcode (`Trunc`, `ZExt`, ...).
    #[inline]
    pub fn opcode(self) -> CastOpcode {
        self.payload().kind
    }
    /// Source operand of the cast.
    pub fn src(self) -> Value<'ctx> {
        let id = self.payload().src.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

// --------------------------------------------------------------------------
// Comparison instructions
// --------------------------------------------------------------------------

/// `icmp` integer comparison. Mirrors `ICmpInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ICmpInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(ICmpInst);

impl<'ctx> ICmpInst<'ctx> {
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
    pub fn predicate(self) -> crate::cmp_predicate::IntPredicate {
        self.payload().predicate
    }
    pub fn lhs(self) -> Value<'ctx> {
        let id = self.payload().lhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn rhs(self) -> Value<'ctx> {
        let id = self.payload().rhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

/// `fcmp` floating-point comparison. Mirrors `FCmpInst`
/// (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FCmpInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(FCmpInst);

impl<'ctx> FCmpInst<'ctx> {
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
    pub fn predicate(self) -> crate::cmp_predicate::FloatPredicate {
        self.payload().predicate
    }
    pub fn lhs(self) -> Value<'ctx> {
        let id = self.payload().lhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn rhs(self) -> Value<'ctx> {
        let id = self.payload().rhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

// --------------------------------------------------------------------------
// Branch terminator
// --------------------------------------------------------------------------

/// `br` terminator. Mirrors `BranchInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BranchInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(BranchInst);

impl<'ctx> BranchInst<'ctx> {
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
    pub fn condition(self) -> Option<Value<'ctx>> {
        match &self.payload().kind {
            BranchKind::Conditional { cond, .. } => {
                let module = self.module.module();
                let cid = cond.get();
                let data = module.context().value_data(cid);
                Some(Value::from_parts(cid, module, data.ty))
            }
            BranchKind::Unconditional(_) => None,
        }
    }
    /// Iterator over successor block-ids.
    pub(crate) fn successor_ids(self) -> Vec<ValueId> {
        match &self.payload().kind {
            BranchKind::Unconditional(t) => vec![*t],
            BranchKind::Conditional {
                then_bb, else_bb, ..
            } => vec![*then_bb, *else_bb],
        }
    }
    /// Successors as runtime-checked basic-block handles.
    pub fn successors(
        self,
    ) -> impl ExactSizeIterator<Item = crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn>> + 'ctx
    {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        self.successor_ids()
            .into_iter()
            .map(move |id| crate::basic_block::BasicBlock::from_parts(id, module, label_ty))
    }
}

/// `unreachable` terminator. Mirrors `UnreachableInst`
/// (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnreachableInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
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
/// phi is still accepting `add_incoming` calls. Calling
/// [`PhiInst::finish`] consumes the open phi and returns a [`Closed`]
/// view; the closed view exposes only read accessors.
#[derive(Debug)]
pub struct PhiInst<'ctx, W: crate::int_width::IntWidth, P: PhiState = Open> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    _w: core::marker::PhantomData<fn() -> W>,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, W: crate::int_width::IntWidth, P: PhiState> Clone for PhiInst<'ctx, W, P> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: crate::int_width::IntWidth, P: PhiState> Copy for PhiInst<'ctx, W, P> {}

impl<'ctx, W: crate::int_width::IntWidth, P: PhiState> PhiInst<'ctx, W, P> {
    #[inline]
    pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _w: core::marker::PhantomData,
            _p: core::marker::PhantomData,
        }
    }

    /// Re-tag the phi-state marker. Crate-internal: only [`finish`]
    /// flips the public marker.
    #[inline]
    pub(crate) fn retag<P2: PhiState>(self) -> PhiInst<'ctx, W, P2> {
        PhiInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _w: core::marker::PhantomData,
            _p: core::marker::PhantomData,
        }
    }

    fn payload(self) -> &'ctx PhiData {
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
    pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
        Instruction::from_parts(self.id, self.module.module())
    }

    /// Result handle for the phi node, narrowed to the static width
    /// `W`.
    #[inline]
    pub fn as_int_value(self) -> crate::value::IntValue<'ctx, W> {
        let v = Value::from_parts(self.id, self.module.module(), self.ty);
        crate::value::IntValue::<W>::from_value_unchecked(v)
    }

    pub fn incoming_count(self) -> u32 {
        let len = self.payload().incoming.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("phi has more than u32::MAX incoming"))
    }

    /// Read the `(value, block)` pair at `index`.
    pub fn incoming(
        self,
        index: u32,
    ) -> crate::IrResult<(
        Value<'ctx>,
        crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn>,
    )> {
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
        let value = Value::from_parts(vid, module, v_data.ty);
        let label_ty = module.label_type().as_type().id();
        let block = crate::basic_block::BasicBlock::from_parts(bid, module, label_ty);
        Ok((value, block))
    }
}

impl<'ctx, W: crate::int_width::IntWidth> PhiInst<'ctx, W, Open> {
    /// Append `(value, block)` to the incoming list. Mirrors
    /// `PHINode::addIncoming`. Returns `Self` so calls chain.
    /// Errors if `value`'s type does not match the phi's result type
    /// or `block` belongs to a different module.
    pub fn add_incoming<V, R, S>(
        self,
        value: V,
        block: crate::basic_block::BasicBlock<'ctx, R, S>,
    ) -> crate::IrResult<Self>
    where
        V: crate::int_width::IntoIntValue<'ctx, W>,
        R: crate::marker::ReturnMarker,
        S: crate::block_state::BlockSealState,
    {
        let module = self.module.module();
        let value = value.into_int_value(module)?;
        if value.as_value().module().id() != module.id()
            || block.as_value().module().id() != module.id()
        {
            return Err(crate::IrError::ForeignValue);
        }
        if value.as_value().ty == self.ty {
            let value_id = value.as_value().id;
            let block_id = block.as_value().id;
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
                .push(self.id);
            Ok(self)
        } else {
            Err(crate::IrError::TypeMismatch {
                expected: crate::r#type::Type::new(self.ty, module).kind_label(),
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
    pub fn finish(self) -> PhiInst<'ctx, W, Closed> {
        self.retag::<Closed>()
    }
}

impl<'ctx, W: crate::int_width::IntWidth, P: PhiState> PartialEq for PhiInst<'ctx, W, P> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: crate::int_width::IntWidth, P: PhiState> Eq for PhiInst<'ctx, W, P> {}
impl<'ctx, W: crate::int_width::IntWidth, P: PhiState> core::hash::Hash for PhiInst<'ctx, W, P> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, W: crate::int_width::IntWidth, P: PhiState> From<PhiInst<'ctx, W, P>>
    for Instruction<'ctx, state::Attached>
{
    #[inline]
    fn from(h: PhiInst<'ctx, W, P>) -> Self {
        h.as_instruction()
    }
}

// --------------------------------------------------------------------------
// Unary ops: fneg / freeze / va_arg
// --------------------------------------------------------------------------

/// `fneg` floating-point negate. Mirrors `UnaryOperator::FNeg` in
/// `InstrTypes.h`. Carries [`crate::FastMathFlags`] like every
/// `FPMathOperator`-class instruction (`Operator.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FNegInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(FNegInst);

impl<'ctx> FNegInst<'ctx> {
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
    pub fn operand(self) -> Value<'ctx> {
        let id = self.payload().src.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    /// Fast-math flags. Mirrors `FPMathOperator::getFastMathFlags`.
    pub fn fast_math_flags(self) -> crate::fmf::FastMathFlags {
        self.payload().fmf
    }
}

/// `freeze` poison/undef-removing operator. Mirrors `FreezeInst`
/// (`Instructions.h`). The result type matches the operand type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FreezeInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(FreezeInst);

impl<'ctx> FreezeInst<'ctx> {
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
    pub fn operand(self) -> Value<'ctx> {
        let id = self.payload().src.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

/// `va_arg` instruction. Mirrors `VAArgInst` (`Instructions.h`).
/// Loads the next argument from a `va_list` pointer; the destination
/// type lives on [`Self::result_type`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VAArgInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(VAArgInst);

impl<'ctx> VAArgInst<'ctx> {
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
    pub fn pointer(self) -> Value<'ctx> {
        let id = self.payload().src.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    /// Destination type (the second `, T` in `va_arg ptr %vl, T`).
    pub fn result_type(self) -> crate::r#type::Type<'ctx> {
        crate::r#type::Type::new(self.ty, self.module.module())
    }
}

// --------------------------------------------------------------------------
// Aggregate ops: extractvalue / insertvalue
// --------------------------------------------------------------------------

/// `extractvalue` reads a single sub-element of an aggregate by
/// constant indices. Mirrors `ExtractValueInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExtractValueInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(ExtractValueInst);

impl<'ctx> ExtractValueInst<'ctx> {
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
    pub fn aggregate(self) -> Value<'ctx> {
        let id = self.payload().aggregate.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    /// Compile-time index path. Mirrors `ExtractValueInst::indices`.
    pub fn indices(self) -> &'ctx [u32] {
        &self.payload().indices
    }
}

/// `insertvalue` writes a sub-element back into an aggregate by
/// constant indices. Mirrors `InsertValueInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InsertValueInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(InsertValueInst);

impl<'ctx> InsertValueInst<'ctx> {
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
    pub fn aggregate(self) -> Value<'ctx> {
        let id = self.payload().aggregate.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn inserted_value(self) -> Value<'ctx> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
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
pub struct ExtractElementInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(ExtractElementInst);

impl<'ctx> ExtractElementInst<'ctx> {
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
    pub fn vector(self) -> Value<'ctx> {
        let id = self.payload().vector.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn index(self) -> Value<'ctx> {
        let id = self.payload().index.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

/// `insertelement` writes a single element back into a vector.
/// Mirrors `InsertElementInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InsertElementInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(InsertElementInst);

impl<'ctx> InsertElementInst<'ctx> {
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
    pub fn vector(self) -> Value<'ctx> {
        let id = self.payload().vector.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn inserted_value(self) -> Value<'ctx> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn index(self) -> Value<'ctx> {
        let id = self.payload().index.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

/// `shufflevector` builds a new vector by selecting elements from two
/// input vectors per a constant integer mask. Mirrors
/// `ShuffleVectorInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShuffleVectorInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(ShuffleVectorInst);

impl<'ctx> ShuffleVectorInst<'ctx> {
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
    pub fn lhs(self) -> Value<'ctx> {
        let id = self.payload().lhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn rhs(self) -> Value<'ctx> {
        let id = self.payload().rhs.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
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
pub struct FenceInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(FenceInst);

impl<'ctx> FenceInst<'ctx> {
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
    pub fn ordering(self) -> crate::atomic_ordering::AtomicOrdering {
        self.payload().ordering
    }
    /// Synchronization scope. Mirrors `FenceInst::getSyncScopeID`.
    pub fn sync_scope(self) -> crate::sync_scope::SyncScope {
        self.payload().sync_scope.clone()
    }
}

/// `cmpxchg` atomic compare-and-swap. Mirrors `AtomicCmpXchgInst`
/// (`Instructions.h`). Result type is the literal struct
/// `{ <pointee>, i1 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtomicCmpXchgInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(AtomicCmpXchgInst);

impl<'ctx> AtomicCmpXchgInst<'ctx> {
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
    pub fn pointer(self) -> Value<'ctx> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn compare_value(self) -> Value<'ctx> {
        let id = self.payload().cmp.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn new_value(self) -> Value<'ctx> {
        let id = self.payload().new_val.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn align(self) -> Option<crate::align::Align> {
        self.payload().align.align()
    }
    pub fn success_ordering(self) -> crate::atomic_ordering::AtomicOrdering {
        self.payload().success_ordering
    }
    pub fn failure_ordering(self) -> crate::atomic_ordering::AtomicOrdering {
        self.payload().failure_ordering
    }
    pub fn sync_scope(self) -> crate::sync_scope::SyncScope {
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
pub struct AtomicRMWInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(AtomicRMWInst);

impl<'ctx> AtomicRMWInst<'ctx> {
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
    pub fn operation(self) -> crate::atomicrmw_binop::AtomicRMWBinOp {
        self.payload().op
    }
    pub fn pointer(self) -> Value<'ctx> {
        let id = self.payload().ptr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn value_operand(self) -> Value<'ctx> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn align(self) -> Option<crate::align::Align> {
        self.payload().align.align()
    }
    pub fn ordering(self) -> crate::atomic_ordering::AtomicOrdering {
        self.payload().ordering
    }
    pub fn sync_scope(self) -> crate::sync_scope::SyncScope {
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
/// [`Open`](crate::term_open_state::Open)) tracks whether the case
/// list is still editable. `add_case` is gated to `P = Open`;
/// `finish` consumes the open handle and returns `Closed`.
#[derive(Debug)]
pub struct SwitchInst<'ctx, P: crate::term_open_state::TermOpenState = crate::term_open_state::Open>
{
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: crate::term_open_state::TermOpenState> Clone for SwitchInst<'ctx, P> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Copy for SwitchInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> PartialEq for SwitchInst<'ctx, P> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Eq for SwitchInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> core::hash::Hash for SwitchInst<'ctx, P> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> SwitchInst<'ctx, P> {
    #[inline]
    pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(crate) fn retag<P2: crate::term_open_state::TermOpenState>(self) -> SwitchInst<'ctx, P2> {
        SwitchInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
        Instruction::from_parts(self.id, self.module.module())
    }
    fn payload(self) -> &'ctx crate::instr_types::SwitchInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Switch(d) => d,
                _ => unreachable!("SwitchInst invariant: kind is Switch"),
            },
            _ => unreachable!("SwitchInst invariant: kind is Instruction"),
        }
    }
    pub fn condition(self) -> Value<'ctx> {
        let id = self.payload().cond.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn default_destination(self) -> crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        crate::basic_block::BasicBlock::from_parts(
            self.payload().default_bb.get(),
            module,
            label_ty,
        )
    }
    pub fn case_count(self) -> u32 {
        let len = self.payload().cases.borrow().len();
        u32::try_from(len).unwrap_or_else(|_| unreachable!("switch has more than u32::MAX cases"))
    }
}

impl<'ctx> SwitchInst<'ctx, crate::term_open_state::Open> {
    /// Append a `(case_value, target)` entry to the switch. Mirrors
    /// `SwitchInst::addCase`. Returns `Self` so calls chain.
    pub fn add_case<V, R, S>(
        self,
        case_value: V,
        target: crate::basic_block::BasicBlock<'ctx, R, S>,
    ) -> crate::IrResult<Self>
    where
        V: crate::value::IsValue<'ctx>,
        R: crate::marker::ReturnMarker,
        S: crate::block_state::BlockSealState,
    {
        let module = self.module.module();
        let v = case_value.as_value();
        if v.module().id() != module.id() || target.as_value().module().id() != module.id() {
            return Err(crate::IrError::ForeignValue);
        }
        let cond_ty = self.payload().cond.get();
        let cond_ty = module.context().value_data(cond_ty).ty;
        if v.ty != cond_ty {
            return Err(crate::IrError::TypeMismatch {
                expected: crate::r#type::Type::new(cond_ty, module).kind_label(),
                got: v.ty().kind_label(),
            });
        }
        let v_id = v.id;
        let bb_id = target.as_value().id;
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
            .push(self.id);
        Ok(self)
    }
    /// Consume the open switch and return its [`Closed`] view. Mirrors
    /// the implicit "switch is finalised" convention upstream where
    /// the verifier subsequently runs `Verifier::visitSwitchInst`.
    #[inline]
    pub fn finish(self) -> SwitchInst<'ctx, crate::term_open_state::Closed> {
        self.retag()
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> From<SwitchInst<'ctx, P>>
    for Instruction<'ctx, state::Attached>
{
    #[inline]
    fn from(h: SwitchInst<'ctx, P>) -> Self {
        h.as_instruction()
    }
}

/// `indirectbr` terminator. Mirrors `IndirectBrInst`
/// (`Instructions.h`). The address operand selects one of the
/// declared destination blocks at runtime.
#[derive(Debug)]
pub struct IndirectBrInst<
    'ctx,
    P: crate::term_open_state::TermOpenState = crate::term_open_state::Open,
> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: crate::term_open_state::TermOpenState> Clone for IndirectBrInst<'ctx, P> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Copy for IndirectBrInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> PartialEq for IndirectBrInst<'ctx, P> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Eq for IndirectBrInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> core::hash::Hash for IndirectBrInst<'ctx, P> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> IndirectBrInst<'ctx, P> {
    #[inline]
    pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(crate) fn retag<P2: crate::term_open_state::TermOpenState>(
        self,
    ) -> IndirectBrInst<'ctx, P2> {
        IndirectBrInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
        Instruction::from_parts(self.id, self.module.module())
    }
    fn payload(self) -> &'ctx crate::instr_types::IndirectBrInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::IndirectBr(d) => d,
                _ => unreachable!("IndirectBrInst invariant: kind is IndirectBr"),
            },
            _ => unreachable!("IndirectBrInst invariant: kind is Instruction"),
        }
    }
    pub fn address(self) -> Value<'ctx> {
        let id = self.payload().addr.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn destination_count(self) -> u32 {
        let len = self.payload().destinations.borrow().len();
        u32::try_from(len)
            .unwrap_or_else(|_| unreachable!("indirectbr has more than u32::MAX destinations"))
    }
}

impl<'ctx> IndirectBrInst<'ctx, crate::term_open_state::Open> {
    /// Append a destination block. Mirrors `IndirectBrInst::addDestination`.
    pub fn add_destination<R, S>(
        self,
        target: crate::basic_block::BasicBlock<'ctx, R, S>,
    ) -> crate::IrResult<Self>
    where
        R: crate::marker::ReturnMarker,
        S: crate::block_state::BlockSealState,
    {
        let module = self.module.module();
        if target.as_value().module().id() != module.id() {
            return Err(crate::IrError::ForeignValue);
        }
        self.payload()
            .destinations
            .borrow_mut()
            .push(target.as_value().id);
        Ok(self)
    }
    /// Consume the open `indirectbr` and return its [`Closed`] view.
    #[inline]
    pub fn finish(self) -> IndirectBrInst<'ctx, crate::term_open_state::Closed> {
        self.retag()
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> From<IndirectBrInst<'ctx, P>>
    for Instruction<'ctx, state::Attached>
{
    #[inline]
    fn from(h: IndirectBrInst<'ctx, P>) -> Self {
        h.as_instruction()
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
pub struct InvokeInst<'ctx, R: crate::marker::ReturnMarker = crate::marker::Dyn> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    _r: core::marker::PhantomData<R>,
}

impl<'ctx, R: crate::marker::ReturnMarker> Clone for InvokeInst<'ctx, R> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: crate::marker::ReturnMarker> Copy for InvokeInst<'ctx, R> {}
impl<'ctx, R: crate::marker::ReturnMarker> PartialEq for InvokeInst<'ctx, R> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: crate::marker::ReturnMarker> Eq for InvokeInst<'ctx, R> {}
impl<'ctx, R: crate::marker::ReturnMarker> core::hash::Hash for InvokeInst<'ctx, R> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, R: crate::marker::ReturnMarker> InvokeInst<'ctx, R> {
    #[inline]
    pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _r: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
        Instruction::from_parts(self.id, self.module.module())
    }
    /// Re-tag the return marker. Crate-internal: only [`build_invoke`]
    /// flows the typed marker.
    #[inline]
    pub(crate) fn retag<R2: crate::marker::ReturnMarker>(self) -> InvokeInst<'ctx, R2> {
        InvokeInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: core::marker::PhantomData,
        }
    }
    /// Erase the return marker.
    #[inline]
    pub fn as_dyn(self) -> InvokeInst<'ctx, crate::marker::Dyn> {
        self.retag::<crate::marker::Dyn>()
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
    pub fn callee(self) -> Value<'ctx> {
        let id = self.payload().callee.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn function_type(self) -> crate::derived_types::FunctionType<'ctx> {
        crate::derived_types::FunctionType::new(self.payload().fn_ty, self.module.module())
    }
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, module, data.ty)
        })
    }
    pub fn calling_conv(self) -> crate::CallingConv {
        self.payload().calling_conv
    }
    pub fn normal_destination(self) -> crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        crate::basic_block::BasicBlock::from_parts(
            self.payload().normal_dest.get(),
            module,
            label_ty,
        )
    }
    pub fn unwind_destination(self) -> crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        crate::basic_block::BasicBlock::from_parts(
            self.payload().unwind_dest.get(),
            module,
            label_ty,
        )
    }
}

impl<'ctx, R: crate::marker::ReturnMarker> From<InvokeInst<'ctx, R>>
    for Instruction<'ctx, state::Attached>
{
    #[inline]
    fn from(h: InvokeInst<'ctx, R>) -> Self {
        h.as_instruction()
    }
}

/// `callbr` terminator. Mirrors `CallBrInst` (`Instructions.h`).
/// A call-like terminator with one fallthrough destination plus zero
/// or more indirect destination labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallBrInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(CallBrInst);

impl<'ctx> CallBrInst<'ctx> {
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
    pub fn callee(self) -> Value<'ctx> {
        let id = self.payload().callee.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn function_type(self) -> crate::derived_types::FunctionType<'ctx> {
        crate::derived_types::FunctionType::new(self.payload().fn_ty, self.module.module())
    }
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, module, data.ty)
        })
    }
    pub fn calling_conv(self) -> crate::CallingConv {
        self.payload().calling_conv
    }
    pub fn default_destination(self) -> crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        crate::basic_block::BasicBlock::from_parts(
            self.payload().default_dest.get(),
            module,
            label_ty,
        )
    }
    pub fn indirect_destinations(
        self,
    ) -> impl ExactSizeIterator<Item = crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn>> + 'ctx
    {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        let ids: Vec<ValueId> = self
            .payload()
            .indirect_dests
            .iter()
            .map(|c| c.get())
            .collect();
        ids.into_iter()
            .map(move |id| crate::basic_block::BasicBlock::from_parts(id, module, label_ty))
    }
}

// --------------------------------------------------------------------------
// EH-data: landingpad / resume
// --------------------------------------------------------------------------

/// `landingpad` instruction. Mirrors `LandingPadInst` (`Instructions.h`).
///
/// The `P: TermOpenState` parameter (default
/// [`Open`](crate::term_open_state::Open)) tracks whether the clause
/// list is still editable. `add_clause` is gated to `P = Open`.
#[derive(Debug)]
pub struct LandingPadInst<
    'ctx,
    P: crate::term_open_state::TermOpenState = crate::term_open_state::Open,
> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: crate::term_open_state::TermOpenState> Clone for LandingPadInst<'ctx, P> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Copy for LandingPadInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> PartialEq for LandingPadInst<'ctx, P> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Eq for LandingPadInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> core::hash::Hash for LandingPadInst<'ctx, P> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> LandingPadInst<'ctx, P> {
    #[inline]
    pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(crate) fn retag<P2: crate::term_open_state::TermOpenState>(
        self,
    ) -> LandingPadInst<'ctx, P2> {
        LandingPadInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
        Instruction::from_parts(self.id, self.module.module())
    }
    fn payload(self) -> &'ctx crate::instr_types::LandingPadInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::LandingPad(d) => d,
                _ => unreachable!("LandingPadInst invariant: kind is LandingPad"),
            },
            _ => unreachable!("LandingPadInst invariant: kind is Instruction"),
        }
    }
    pub fn is_cleanup(self) -> bool {
        self.payload().cleanup.get()
    }
    pub fn clause_count(self) -> u32 {
        let len = self.payload().clauses.borrow().len();
        u32::try_from(len)
            .unwrap_or_else(|_| unreachable!("landingpad has more than u32::MAX clauses"))
    }
}

impl<'ctx> LandingPadInst<'ctx, crate::term_open_state::Open> {
    /// Mark this landingpad as a cleanup. Mirrors `LandingPadInst::setCleanup(true)`.
    pub fn set_cleanup(self) -> Self {
        self.payload().cleanup.set(true);
        self
    }
    /// Append a `catch <ty> <val>` clause. Mirrors `LandingPadInst::addClause`
    /// for `Catch`.
    pub fn add_catch_clause<V: crate::value::IsValue<'ctx>>(
        self,
        type_info: V,
    ) -> crate::IrResult<Self> {
        let module = self.module.module();
        let v = type_info.as_value();
        if v.module().id() != module.id() {
            return Err(crate::IrError::ForeignValue);
        }
        self.payload().clauses.borrow_mut().push((
            crate::instr_types::LandingPadClauseKind::Catch,
            core::cell::Cell::new(v.id),
        ));
        module
            .context()
            .value_data(v.id)
            .use_list
            .borrow_mut()
            .push(self.id);
        Ok(self)
    }
    /// Append a `filter <ty> <val>` clause.
    pub fn add_filter_clause<V: crate::value::IsValue<'ctx>>(
        self,
        filter_array: V,
    ) -> crate::IrResult<Self> {
        let module = self.module.module();
        let v = filter_array.as_value();
        if v.module().id() != module.id() {
            return Err(crate::IrError::ForeignValue);
        }
        self.payload().clauses.borrow_mut().push((
            crate::instr_types::LandingPadClauseKind::Filter,
            core::cell::Cell::new(v.id),
        ));
        module
            .context()
            .value_data(v.id)
            .use_list
            .borrow_mut()
            .push(self.id);
        Ok(self)
    }
    /// Consume the open landingpad and return its [`Closed`] view.
    #[inline]
    pub fn finish(self) -> LandingPadInst<'ctx, crate::term_open_state::Closed> {
        self.retag()
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> From<LandingPadInst<'ctx, P>>
    for Instruction<'ctx, state::Attached>
{
    #[inline]
    fn from(h: LandingPadInst<'ctx, P>) -> Self {
        h.as_instruction()
    }
}

/// `resume` terminator. Mirrors `ResumeInst` (`Instructions.h`).
/// Single value operand (typically a `landingpad` result).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResumeInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(ResumeInst);

impl<'ctx> ResumeInst<'ctx> {
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
    pub fn value(self) -> Value<'ctx> {
        let id = self.payload().value.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

// --------------------------------------------------------------------------
// Funclet ops: cleanuppad / cleanupret / catchpad / catchret / catchswitch
// --------------------------------------------------------------------------

/// `cleanuppad` instruction. Mirrors `CleanupPadInst` (`Instructions.h`).
/// Result is a `token`-typed value used as a funclet pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CleanupPadInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(CleanupPadInst);

impl<'ctx> CleanupPadInst<'ctx> {
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
    pub fn parent_pad(self) -> Option<Value<'ctx>> {
        let id = self.payload().parent_pad.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, module, data.ty)
        })
    }
}

/// `catchpad` instruction. Mirrors `CatchPadInst` (`Instructions.h`).
/// Result is a `token`-typed value used as a funclet pad. Parent must
/// be a `catchswitch` (verifier rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatchPadInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(CatchPadInst);

impl<'ctx> CatchPadInst<'ctx> {
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
    pub fn parent_pad(self) -> Option<Value<'ctx>> {
        let id = self.payload().parent_pad.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }
    pub fn args(self) -> impl ExactSizeIterator<Item = Value<'ctx>> + 'ctx {
        let module = self.module.module();
        let ids: Vec<ValueId> = self.payload().args.iter().map(|c| c.get()).collect();
        ids.into_iter().map(move |id| {
            let data = module.context().value_data(id);
            Value::from_parts(id, module, data.ty)
        })
    }
}

/// `catchret` terminator. Mirrors `CatchReturnInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatchReturnInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(CatchReturnInst);

impl<'ctx> CatchReturnInst<'ctx> {
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
    pub fn catch_pad(self) -> Value<'ctx> {
        let id = self.payload().catch_pad.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    pub fn target(self) -> crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn> {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        crate::basic_block::BasicBlock::from_parts(self.payload().target_bb, module, label_ty)
    }
}

/// `cleanupret` terminator. Mirrors `CleanupReturnInst` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CleanupReturnInst<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

decl_handle_scaffold!(CleanupReturnInst);

impl<'ctx> CleanupReturnInst<'ctx> {
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
    pub fn cleanup_pad(self) -> Value<'ctx> {
        let id = self.payload().cleanup_pad.get();
        let module = self.module.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
    /// `None` represents `unwind to caller`.
    pub fn unwind_dest(self) -> Option<crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn>> {
        let id = self.payload().unwind_dest?;
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        Some(crate::basic_block::BasicBlock::from_parts(
            id, module, label_ty,
        ))
    }
}

/// `catchswitch` terminator. Mirrors `CatchSwitchInst` (`Instructions.h`).
/// Variable-arity handler list with optional unwind destination.
#[derive(Debug)]
pub struct CatchSwitchInst<
    'ctx,
    P: crate::term_open_state::TermOpenState = crate::term_open_state::Open,
> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    _p: core::marker::PhantomData<P>,
}

impl<'ctx, P: crate::term_open_state::TermOpenState> Clone for CatchSwitchInst<'ctx, P> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Copy for CatchSwitchInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> PartialEq for CatchSwitchInst<'ctx, P> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, P: crate::term_open_state::TermOpenState> Eq for CatchSwitchInst<'ctx, P> {}
impl<'ctx, P: crate::term_open_state::TermOpenState> core::hash::Hash for CatchSwitchInst<'ctx, P> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> CatchSwitchInst<'ctx, P> {
    #[inline]
    pub(crate) fn from_raw(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub(crate) fn retag<P2: crate::term_open_state::TermOpenState>(
        self,
    ) -> CatchSwitchInst<'ctx, P2> {
        CatchSwitchInst {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _p: core::marker::PhantomData,
        }
    }
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx, state::Attached> {
        Instruction::from_parts(self.id, self.module.module())
    }
    fn payload(self) -> &'ctx crate::instr_types::CatchSwitchInstData {
        let module = self.module.module();
        match &module.context().value_data(self.id).kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::CatchSwitch(d) => d,
                _ => unreachable!("CatchSwitchInst invariant: kind is CatchSwitch"),
            },
            _ => unreachable!("CatchSwitchInst invariant: kind is Instruction"),
        }
    }
    pub fn parent_pad(self) -> Option<Value<'ctx>> {
        let id = self.payload().parent_pad.get()?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }
    /// `None` = `unwind to caller`.
    pub fn unwind_dest(self) -> Option<crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn>> {
        let id = self.payload().unwind_dest.get()?;
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        Some(crate::basic_block::BasicBlock::from_parts(
            id, module, label_ty,
        ))
    }
    pub fn handler_count(self) -> u32 {
        let len = self.payload().handlers.borrow().len();
        u32::try_from(len)
            .unwrap_or_else(|_| unreachable!("catchswitch has more than u32::MAX handlers"))
    }
}

impl<'ctx> CatchSwitchInst<'ctx, crate::term_open_state::Open> {
    pub fn add_handler<R, S>(
        self,
        handler: crate::basic_block::BasicBlock<'ctx, R, S>,
    ) -> crate::IrResult<Self>
    where
        R: crate::marker::ReturnMarker,
        S: crate::block_state::BlockSealState,
    {
        let module = self.module.module();
        if handler.as_value().module().id() != module.id() {
            return Err(crate::IrError::ForeignValue);
        }
        self.payload()
            .handlers
            .borrow_mut()
            .push(handler.as_value().id);
        Ok(self)
    }
    #[inline]
    pub fn finish(self) -> CatchSwitchInst<'ctx, crate::term_open_state::Closed> {
        self.retag()
    }
}

impl<'ctx, P: crate::term_open_state::TermOpenState> From<CatchSwitchInst<'ctx, P>>
    for Instruction<'ctx, state::Attached>
{
    #[inline]
    fn from(h: CatchSwitchInst<'ctx, P>) -> Self {
        h.as_instruction()
    }
}
