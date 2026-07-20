//! Inline assembly as a `call` callee. Mirrors `class InlineAsm` in
//! `llvm/include/llvm/IR/InlineAsm.h`.
//!
//! ## Representation
//!
//! An inline-asm value is a **context-global** value, just like a
//! [`Function`](crate::function::FunctionValue) or a
//! [`Constant`](crate::constant::Constant): it has no function-local SSA
//! definition and is never assigned a `%N` slot. It lives in the same value
//! arena as globals, constants, and functions.
//!
//! LLVM types an inline-asm value as a **pointer** (the asm "address"),
//! while the *function type* it conceptually wraps is carried separately
//! so a `call` through it knows the argument / return shape. This module
//! follows that split: the [`InlineAsm`] handle's [`Value::ty`] is the
//! module's `ptr` type, and the wrapped [`FunctionType`]
//! id is stored in the payload for the [`IRBuilder`](crate::ir_builder::IRBuilder)
//! to consume when it emits the call.
//!
//! The textual form a `call` prints is, e.g.:
//!
//! ```text
//! %r = call i64 asm sideeffect "add $1, $0", "=r,r,r"(i64 %a, i64 %b)
//! ```

use core::marker::PhantomData;

use crate::derived_types::FunctionType;
use crate::module::{ModuleBrand, ModuleRef, ModuleView};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct InlineAsmOptions {
    has_side_effects: bool,
    is_align_stack: bool,
    dialect: AsmDialect,
    can_unwind: bool,
}

impl InlineAsmOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn side_effects(mut self, value: bool) -> Self {
        self.has_side_effects = value;
        self
    }

    pub fn align_stack(mut self, value: bool) -> Self {
        self.is_align_stack = value;
        self
    }

    pub fn with_dialect(mut self, value: AsmDialect) -> Self {
        self.dialect = value;
        self
    }

    pub fn with_can_unwind(mut self, value: bool) -> Self {
        self.can_unwind = value;
        self
    }

    pub const fn has_side_effects(&self) -> bool {
        self.has_side_effects
    }

    pub const fn is_align_stack(&self) -> bool {
        self.is_align_stack
    }

    pub const fn dialect(&self) -> AsmDialect {
        self.dialect
    }

    pub const fn can_unwind(&self) -> bool {
        self.can_unwind
    }
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
    /// `unwind` keyword: the asm may unwind. Mirrors
    /// `InlineAsm::canThrow()`.
    pub(crate) can_unwind: bool,
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
pub struct InlineAsm<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx, B>,
    /// Cached pointer type id (`ptr`). The value's value-arena type is
    /// this pointer type; the wrapped function type lives in the payload.
    pub(crate) ty: TypeId,
    pub(crate) _ctx: PhantomData<&'ctx ()>,
}

impl<'ctx, B: ModuleBrand + 'ctx> core::fmt::Display for InlineAsm<'ctx, B> {
    /// Print the operand form `ptr asm [sideeffect] "<body>",
    /// "<constraints>"` -- the leading `ptr` is the value's IR type,
    /// matching LLVM's pointer typing of inline asm. Identical to what the
    /// erased [`Value`] handle from [`InlineAsm::as_value`] prints.
    ///
    /// A `call` whose callee is inline asm prints the `asm ...` body
    /// directly in the callee position and so does not go through this.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&InlineAsm::as_value(*self), f)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> InlineAsm<'ctx, B> {
    /// Construct from raw parts. Crate-internal: only
    /// [`Module::inline_asm`](crate::module::Module::inline_asm) hands
    /// these out, after pushing the value into the arena.
    #[inline]
    pub(crate) fn from_parts<M>(id: ValueId, module: M, ty: TypeId) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _ctx: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle. The widened value's type is
    /// the `ptr` type, matching LLVM's pointer typing of inline asm.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// The conceptual function type wrapped by this asm — the signature a
    /// `call` through it must match. Mirrors `InlineAsm::getFunctionType()`.
    #[inline]
    pub fn function_type(self) -> FunctionType<'ctx, B> {
        let fn_ty = self.payload().fn_ty;
        crate::derived_types::FunctionType::new(fn_ty, self.module)
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
    /// Number of label constraints (`!`) in the constraint string.
    #[inline]
    pub fn label_constraint_count(&self) -> usize {
        self.constraint_summary().label_count
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

    /// `true` when the `unwind` keyword is set. Mirrors
    /// `InlineAsm::canThrow()`.
    #[inline]
    pub fn can_unwind(self) -> bool {
        self.payload().can_unwind
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

impl<'ctx, B: ModuleBrand + 'ctx> From<InlineAsm<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(v: InlineAsm<'ctx, B>) -> Self {
        v.as_value()
    }
}
