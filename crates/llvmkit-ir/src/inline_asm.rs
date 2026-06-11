//! Inline assembly as a `call` callee. Mirrors `class InlineAsm` in
//! `llvm/include/llvm/IR/InlineAsm.h`.
//!
//! ## Representation
//!
//! An inline-asm value is a **context-global** value, just like a
//! [`Function`](crate::function::FunctionValue) or a
//! [`Constant`](crate::constant::Constant): it has no function-local SSA
//! definition and is never assigned a `%N` slot. It lives in the value
//! arena under
//! [`ValueKindData::InlineAsm`](crate::value::ValueKindData::InlineAsm).
//!
//! LLVM types an inline-asm value as a **pointer** (the asm "address"),
//! while the *function type* it conceptually wraps is carried separately
//! so a `call` through it knows the argument / return shape. This module
//! follows that split: the [`InlineAsm`] handle's [`Value::ty`] is the
//! module's `ptr` type, and the wrapped [`FunctionType`] id is stored in
//! the payload for the [`IRBuilder`](crate::ir_builder::IRBuilder) to
//! consume when it emits the call.
//!
//! The textual form a `call` prints is, e.g.:
//!
//! ```text
//! %r = call i64 asm sideeffect "add $1, $0", "=r,r,r"(i64 %a, i64 %b)
//! ```

use core::marker::PhantomData;

use crate::module::{Module, ModuleRef};
use crate::r#type::TypeId;
use crate::value::{Value, ValueId};

// --------------------------------------------------------------------------
// Assembly dialect
// --------------------------------------------------------------------------

/// Which assembler syntax the template uses. Mirrors
/// `InlineAsm::AsmDialect` in `llvm/include/llvm/IR/InlineAsm.h`.
///
/// In the textual IR, [`AsmDialect::Intel`] adds the `inteldialect`
/// keyword after the `asm` token; [`AsmDialect::ATT`] (the default) adds
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AsmDialect {
    /// AT&T syntax (`$0`, `$1`, …; LLVM's default).
    #[default]
    ATT,
    /// Intel syntax; prints the `inteldialect` keyword.
    Intel,
}

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload for an inline-asm value. Stored in the value
/// arena under
/// [`ValueKindData::InlineAsm`](crate::value::ValueKindData::InlineAsm).
/// Mirrors the data portion of `class InlineAsm` in
/// `llvm/include/llvm/IR/InlineAsm.h`.
pub(crate) struct InlineAsmConstraintSummary {
    pub label_count: usize,
    pub arg_constraints: usize,
}

#[derive(Debug)]
pub(crate) struct InlineAsmData {
    /// The assembly template string (the `AsmString` field in LLVM).
    pub(crate) asm_string: String,
    /// The constraint string (the `Constraints` field in LLVM), e.g.
    /// `"=r,r,r"`.
    pub(crate) constraint_string: String,
    /// The conceptual function type of the asm: governs the call's
    /// return type and argument types. LLVM stores this as `FunctionType
    /// *FTy`.
    pub(crate) fn_ty: TypeId,
    /// `sideeffect` keyword: the asm has effects not captured by its
    /// outputs. Mirrors `InlineAsm::hasSideEffects()`.
    pub(crate) has_side_effects: bool,
    /// `alignstack` keyword: the asm needs the stack aligned. Mirrors
    /// `InlineAsm::isAlignStack()`.
    pub(crate) is_align_stack: bool,
    /// Source syntax of the template. Mirrors `InlineAsm::getDialect()`.
    pub(crate) dialect: AsmDialect,
}

// --------------------------------------------------------------------------
// Public handle
// --------------------------------------------------------------------------

/// Typed handle to an inline-asm value. Mirrors `InlineAsm *` in
/// upstream LLVM.
///
/// The handle's [`Value::ty`] is the module's `ptr` type (LLVM types
/// inline asm as a pointer); the *function type* the asm wraps — which a
/// `call` uses for its return / argument shape — is recovered via
/// [`Self::function_type`].
///
/// Shape mirrors [`GlobalVariable`](crate::global_variable::GlobalVariable)
/// / [`FunctionValue`](crate::function::FunctionValue): a `(ValueId,
/// ModuleRef, TypeId)` triple plus the cached pointer type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InlineAsm<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    /// Cached pointer type id (`ptr`). The value's value-arena type is
    /// this pointer type; the wrapped function type lives in the payload.
    pub(crate) ty: TypeId,
    pub(crate) _ctx: PhantomData<&'ctx ()>,
}

impl<'ctx> InlineAsm<'ctx> {
    /// Construct from raw parts. Crate-internal: only
    /// [`Module::inline_asm`](crate::module::Module::inline_asm) hands
    /// these out, after pushing the value into the arena.
    #[inline]
    pub(crate) fn from_parts(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _ctx: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle. The widened value's type is
    /// the `ptr` type, matching LLVM's pointer typing of inline asm.
    #[inline]
    pub fn as_value(self) -> Value<'ctx> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// The conceptual function type wrapped by this asm — the signature a
    /// `call` through it must match. Mirrors `InlineAsm::getFunctionType()`.
    #[inline]
    pub fn function_type(self) -> crate::derived_types::FunctionType<'ctx> {
        let fn_ty = self.payload().fn_ty;
        crate::derived_types::FunctionType::new(fn_ty, self.module.module())
    }

    /// The assembly template string. Mirrors `InlineAsm::getAsmString()`.
    #[inline]
    pub fn asm_string(self) -> &'ctx str {
        &self.payload().asm_string
    }

    /// The constraint string. Mirrors `InlineAsm::getConstraintString()`.
    #[inline]
    pub fn constraint_string(&self) -> &'ctx str {
        &self.payload().constraint_string
    }

    pub(crate) fn constraint_summary(&self) -> InlineAsmConstraintSummary {
        let label_count = self
            .constraint_string()
            .split(',')
            .filter(|constraint| constraint.contains('!'))
            .count();
        InlineAsmConstraintSummary {
            label_count,
            arg_constraints: 0,
        }
    }

    /// `true` when the `sideeffect` keyword is set. Mirrors
    /// `InlineAsm::hasSideEffects()`.
    #[inline]
    pub fn has_side_effects(self) -> bool {
        self.payload().has_side_effects
    }

    /// `true` when the `alignstack` keyword is set. Mirrors
    /// `InlineAsm::isAlignStack()`.
    #[inline]
    pub fn is_align_stack(self) -> bool {
        self.payload().is_align_stack
    }

    /// The template's source dialect. Mirrors `InlineAsm::getDialect()`.
    #[inline]
    pub fn dialect(self) -> AsmDialect {
        self.payload().dialect
    }

    /// Borrow the underlying payload via the module's value arena.
    #[inline]
    fn payload(&self) -> &'ctx InlineAsmData {
        match &self.module.module().context().value_data(self.id).kind {
            crate::value::ValueKindData::InlineAsm(d) => d,
            _ => unreachable!("InlineAsm handle invariant: kind is InlineAsm"),
        }
    }
}

impl<'ctx> From<InlineAsm<'ctx>> for Value<'ctx> {
    #[inline]
    fn from(v: InlineAsm<'ctx>) -> Self {
        v.as_value()
    }
}
