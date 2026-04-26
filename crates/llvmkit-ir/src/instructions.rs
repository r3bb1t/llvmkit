//! Per-opcode instruction handles. Mirrors a slice of
//! `llvm/include/llvm/IR/Instructions.h`.
//!
//! Phase E minimum: `add`, `sub`, `mul`, `ret`. Each handle is a thin
//! wrapper around an [`Instruction`] that exposes opcode-specific
//! accessors (operand pickers, flag readers, etc.). Most consumers
//! reach for [`Instruction`] and pattern-match on
//! [`Instruction::kind`] /
//! [`Instruction::terminator_kind`](Instruction::terminator_kind).
//! The dedicated handles are useful when a function accepts only one
//! opcode (e.g. `wraps_overflow_op(inst: AddInst)`).

use crate::instr_types::{BinaryOpData, CastOpData, CastOpcode, ReturnOpData};
use crate::instruction::{Instruction, InstructionKindData};
use crate::value::{Value, ValueKindData};

macro_rules! decl_binop_handle {
    (
        $(#[$attr:meta])*
        $name:ident,
        $variant:ident
    ) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name<'ctx>(pub(crate) Instruction<'ctx>);

        impl<'ctx> $name<'ctx> {
            /// Wrap an instruction known to be of this opcode. Crate-
            /// internal: only the IR builder and `Instruction::kind`
            /// hand these out.
            #[inline]
            pub(crate) fn wrap(inst: Instruction<'ctx>) -> Self { Self(inst) }

            /// Borrow the binary-operator payload.
            fn payload(self) -> &'ctx BinaryOpData {
                match &self.0.as_value().data().kind {
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

            /// Underlying generic instruction handle.
            #[inline]
            pub fn as_instruction(self) -> Instruction<'ctx> { self.0 }

            /// Left-hand side operand. Mirrors `getOperand(0)`.
            pub fn lhs(self) -> Value<'ctx> {
                let id = self.payload().lhs;
                let module = self.0.module();
                let data = module.context().value_data(id);
                Value::from_parts(id, module, data.ty)
            }

            /// Right-hand side operand. Mirrors `getOperand(1)`.
            pub fn rhs(self) -> Value<'ctx> {
                let id = self.payload().rhs;
                let module = self.0.module();
                let data = module.context().value_data(id);
                Value::from_parts(id, module, data.ty)
            }

            /// `nuw` flag.
            #[inline]
            pub fn has_no_unsigned_wrap(self) -> bool { self.payload().no_unsigned_wrap }

            /// `nsw` flag.
            #[inline]
            pub fn has_no_signed_wrap(self) -> bool { self.payload().no_signed_wrap }
        }

        impl<'ctx> ::core::convert::From<$name<'ctx>> for Instruction<'ctx> {
            #[inline]
            fn from(h: $name<'ctx>) -> Self { h.0 }
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

/// `ret` terminator instruction. Mirrors `ReturnInst` in
/// `Instructions.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RetInst<'ctx>(pub(crate) Instruction<'ctx>);

impl<'ctx> RetInst<'ctx> {
    /// Crate-internal wrapping constructor.
    #[inline]
    pub(crate) fn wrap(inst: Instruction<'ctx>) -> Self {
        Self(inst)
    }

    fn payload(self) -> &'ctx ReturnOpData {
        match &self.0.as_value().data().kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Ret(r) => r,
                _ => unreachable!("RetInst invariant: kind is Ret"),
            },
            _ => unreachable!("RetInst invariant: kind is Instruction"),
        }
    }

    /// Underlying generic instruction handle.
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx> {
        self.0
    }

    /// Returned value. `None` for `ret void`.
    pub fn return_value(self) -> Option<Value<'ctx>> {
        let id = self.payload().value?;
        let module = self.0.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }
}

impl<'ctx> From<RetInst<'ctx>> for Instruction<'ctx> {
    #[inline]
    fn from(h: RetInst<'ctx>) -> Self {
        h.0
    }
}

/// Cast instruction (`trunc`, `zext`, `sext`, `bitcast`, ...).
/// Mirrors `CastInst` in `InstrTypes.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CastInst<'ctx>(pub(crate) Instruction<'ctx>);

impl<'ctx> CastInst<'ctx> {
    /// Crate-internal wrapping constructor.
    #[inline]
    pub(crate) fn wrap(inst: Instruction<'ctx>) -> Self {
        Self(inst)
    }

    fn payload(self) -> &'ctx CastOpData {
        match &self.0.as_value().data().kind {
            ValueKindData::Instruction(i) => match &i.kind {
                InstructionKindData::Cast(c) => c,
                _ => unreachable!("CastInst invariant: kind is Cast"),
            },
            _ => unreachable!("CastInst invariant: kind is Instruction"),
        }
    }

    /// Underlying generic instruction handle.
    #[inline]
    pub fn as_instruction(self) -> Instruction<'ctx> {
        self.0
    }

    /// Cast opcode (`Trunc`, `ZExt`, ...).
    #[inline]
    pub fn opcode(self) -> CastOpcode {
        self.payload().kind
    }

    /// Source operand of the cast.
    pub fn src(self) -> Value<'ctx> {
        let id = self.payload().src;
        let module = self.0.module();
        let data = module.context().value_data(id);
        Value::from_parts(id, module, data.ty)
    }
}

impl<'ctx> From<CastInst<'ctx>> for Instruction<'ctx> {
    #[inline]
    fn from(h: CastInst<'ctx>) -> Self {
        h.0
    }
}
