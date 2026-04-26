//! IR builder. Mirrors `llvm/include/llvm/IR/IRBuilder.h` and
//! `llvm/lib/IR/IRBuilder.cpp`.
//!
//! ## Type-state
//!
//! The builder carries two state-marker generics:
//!
//! - `S` ([`Unpositioned`] / [`Positioned`]) — distinguishes "I have an
//!   insertion point" from "I do not". The `build_*` methods are only
//!   available on the [`Positioned`] state.
//! - `R: ReturnMarker` — the parent function's return shape. The
//!   typed `build_ret` methods are dispatched on `R` so calling
//!   `build_ret(int_value)` against a `void`-returning builder is a
//!   compile-time error rather than a runtime
//!   [`IrError::ReturnTypeMismatch`].
//!
//! Mirrors the inkwell `Builder<'ctx>` shape but with the additional
//! invariants that an unpositioned builder has no `build_*` API at
//! all and a `void`-returning builder cannot accidentally emit a
//! value-bearing return.
//!
//! ## What's shipped
//!
//! Phase G minimum + Phase A3 typing + Phase C `trunc`:
//! `build_int_add`, `build_int_sub`, `build_int_mul`, `build_trunc`,
//! `build_ret`, `build_ret_void`. Constant folding for the three
//! arithmetic opcodes routes through [`folder::IRBuilderFolder`] with
//! [`constant_folder::ConstantFolder`] as the default.
//!
//! Other `build_*` methods land as their consumers do; the trait /
//! method names are stable.

pub mod constant_folder;
pub mod folder;
pub mod no_folder;

use core::marker::PhantomData;

use crate::basic_block::BasicBlock;
use crate::derived_types::IntType;
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::float_kind::FloatKind;
use crate::function::FunctionValue;
use crate::instr_types::{BinaryOpData, CastOpData, ReturnOpData};
use crate::instruction::{Instruction, InstructionKindData, build_instruction_value};
use crate::int_width::IntWidth;
use crate::ir_builder::constant_folder::ConstantFolder;
use crate::ir_builder::folder::IRBuilderFolder;
use crate::module::Module;
use crate::return_marker::{RDyn, RFloat, RInt, RPtr, RVoid, ReturnMarker};
use crate::r#type::TypeId;
use crate::value::IntValue;
use crate::value::Value;

/// Type-state marker: the builder has no insertion point. None of the
/// `build_*` methods are reachable in this state.
#[derive(Debug, Clone, Copy)]
pub struct Unpositioned;

/// Type-state marker: the builder has an insertion point and
/// can produce instructions.
#[derive(Debug, Clone, Copy)]
pub struct Positioned;

/// Sealed marker for the type-state generic so external crates cannot
/// invent new states.
mod state_sealed {
    pub trait Sealed {}
    impl Sealed for super::Unpositioned {}
    impl Sealed for super::Positioned {}
}

/// Builder for a chain of [`Instruction`]s appended to a
/// [`BasicBlock`].
///
/// Type parameters:
/// - `F` — folder strategy (defaults to [`ConstantFolder`]).
/// - `S` — insertion-point type-state ([`Unpositioned`] / [`Positioned`]).
/// - `R` — parent function's [`ReturnMarker`].
pub struct IRBuilder<'ctx, F, S, R>
where
    F: IRBuilderFolder<'ctx>,
    S: state_sealed::Sealed,
    R: ReturnMarker,
{
    module: &'ctx Module<'ctx>,
    insert_block: Option<BasicBlock<'ctx, R>>,
    folder: F,
    _state: PhantomData<S>,
}

// --------------------------------------------------------------------------
// Constructors
// --------------------------------------------------------------------------

impl<'ctx> IRBuilder<'ctx, ConstantFolder, Unpositioned, RDyn> {
    /// Construct an unpositioned builder using the default
    /// [`ConstantFolder`]. The runtime-checked [`RDyn`] return marker
    /// matches the runtime-equality `build_ret` path; use
    /// [`IRBuilder::new_for`] when the caller already knows the return
    /// shape statically.
    pub fn new(module: &'ctx Module<'ctx>) -> Self {
        Self {
            module,
            insert_block: None,
            folder: ConstantFolder,
            _state: PhantomData,
        }
    }

    /// Construct an unpositioned, typed-return builder. Use this
    /// when the caller already knows the parent function's return
    /// shape; the resulting builder's `build_ret` is statically
    /// typed.
    ///
    /// ```ignore
    /// let b = IRBuilder::new_for::<RInt<B32>>(&module);
    /// ```
    pub fn new_for<R>(
        module: &'ctx Module<'ctx>,
    ) -> IRBuilder<'ctx, ConstantFolder, Unpositioned, R>
    where
        R: ReturnMarker,
    {
        IRBuilder {
            module,
            insert_block: None,
            folder: ConstantFolder,
            _state: PhantomData,
        }
    }
}

impl<'ctx, F, R> IRBuilder<'ctx, F, Unpositioned, R>
where
    F: IRBuilderFolder<'ctx>,
    R: ReturnMarker,
{
    /// Construct an unpositioned builder using a caller-supplied
    /// folder.
    pub fn with_folder(module: &'ctx Module<'ctx>, folder: F) -> Self {
        Self {
            module,
            insert_block: None,
            folder,
            _state: PhantomData,
        }
    }

    /// Position the builder at the end of `bb`. Mirrors
    /// `IRBuilder::SetInsertPoint(BasicBlock*)`. The block's
    /// [`ReturnMarker`] must match the builder's.
    pub fn position_at_end(self, bb: BasicBlock<'ctx, R>) -> IRBuilder<'ctx, F, Positioned, R> {
        IRBuilder {
            module: self.module,
            insert_block: Some(bb),
            folder: self.folder,
            _state: PhantomData,
        }
    }
}

impl<'ctx, F, R> IRBuilder<'ctx, F, Positioned, R>
where
    F: IRBuilderFolder<'ctx>,
    R: ReturnMarker,
{
    /// Re-position the builder at the end of `bb`.
    pub fn position_at_end(self, bb: BasicBlock<'ctx, R>) -> Self {
        Self {
            module: self.module,
            insert_block: Some(bb),
            folder: self.folder,
            _state: PhantomData,
        }
    }

    /// Drop the insertion point. Mirrors
    /// `IRBuilder::ClearInsertionPoint`.
    pub fn unposition(self) -> IRBuilder<'ctx, F, Unpositioned, R> {
        IRBuilder {
            module: self.module,
            insert_block: None,
            folder: self.folder,
            _state: PhantomData,
        }
    }

    /// Current insertion block. Always populated in the positioned
    /// state.
    #[inline]
    pub fn insert_block(&self) -> BasicBlock<'ctx, R> {
        match self.insert_block {
            Some(bb) => bb,
            None => unreachable!("Positioned builder always has an insertion point"),
        }
    }

    // ---- Integer arithmetic ----

    /// Produce `add lhs, rhs`. Mirrors `IRBuilder::CreateAdd`.
    ///
    /// Operands must be the same width `W` — enforced at compile time
    /// by the type system.
    pub fn build_int_add<W: IntWidth>(
        &self,
        lhs: IntValue<'ctx, W>,
        rhs: IntValue<'ctx, W>,
        name: &str,
    ) -> IrResult<IntValue<'ctx, W>> {
        self.require_same_module(lhs.as_value())?;
        self.require_same_module(rhs.as_value())?;
        if let Some(folded) = self.folder.fold_int_add(lhs.as_value(), rhs.as_value()) {
            // Folder returns an `iN` constant of the same width; the
            // try_from cannot fail by construction.
            return Ok(IntValue::<W>::from_value_unchecked(folded.as_value()));
        }
        let payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(
            lhs.ty().as_type().id(),
            InstructionKindData::Add(payload),
            name,
        );
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `sub lhs, rhs`. Mirrors `IRBuilder::CreateSub`.
    pub fn build_int_sub<W: IntWidth>(
        &self,
        lhs: IntValue<'ctx, W>,
        rhs: IntValue<'ctx, W>,
        name: &str,
    ) -> IrResult<IntValue<'ctx, W>> {
        self.require_same_module(lhs.as_value())?;
        self.require_same_module(rhs.as_value())?;
        if let Some(folded) = self.folder.fold_int_sub(lhs.as_value(), rhs.as_value()) {
            return Ok(IntValue::<W>::from_value_unchecked(folded.as_value()));
        }
        let payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(
            lhs.ty().as_type().id(),
            InstructionKindData::Sub(payload),
            name,
        );
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `mul lhs, rhs`. Mirrors `IRBuilder::CreateMul`.
    pub fn build_int_mul<W: IntWidth>(
        &self,
        lhs: IntValue<'ctx, W>,
        rhs: IntValue<'ctx, W>,
        name: &str,
    ) -> IrResult<IntValue<'ctx, W>> {
        self.require_same_module(lhs.as_value())?;
        self.require_same_module(rhs.as_value())?;
        if let Some(folded) = self.folder.fold_int_mul(lhs.as_value(), rhs.as_value()) {
            return Ok(IntValue::<W>::from_value_unchecked(folded.as_value()));
        }
        let payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(
            lhs.ty().as_type().id(),
            InstructionKindData::Mul(payload),
            name,
        );
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    // ---- Cast: trunc ----

    /// Produce `trunc <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateTrunc`.
    ///
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty`'s
    /// width is not strictly less than `WSrc`'s width.
    pub fn build_trunc<WSrc, WDst>(
        &self,
        value: IntValue<'ctx, WSrc>,
        dst_ty: IntType<'ctx, WDst>,
        name: &str,
    ) -> IrResult<IntValue<'ctx, WDst>>
    where
        WSrc: IntWidth,
        WDst: IntWidth,
    {
        self.require_same_module(value.as_value())?;
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w >= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        let payload = CastOpData {
            kind: crate::instr_types::CastOpcode::Trunc,
            src: value.as_value().id,
        };
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<WDst>::from_value_unchecked(inst.as_value()))
    }

    // ---- Internal helpers ----

    /// Crate-internal: append a freshly-built instruction to the
    /// insertion block. `name` populates the value-symbol-table when
    /// non-empty.
    fn append_instruction(
        &self,
        ty: TypeId,
        kind: InstructionKindData,
        name: &str,
    ) -> Instruction<'ctx> {
        let bb = self.insert_block();
        let bb_id = bb.as_value().id;
        let stored_name = (!name.is_empty()).then(|| name.to_owned());
        let value = build_instruction_value(ty, bb_id, kind, stored_name);
        let id = self.module.context().push_value(value);
        bb.append_instruction(id);
        if !name.is_empty() {
            if let Some(parent_fn_id) = bb.parent_id() {
                let parent_fn =
                    FunctionValue::<RDyn>::from_parts_unchecked(parent_fn_id, self.module);
                parent_fn.register_value_name(name, id);
            }
        }
        Instruction::from_parts(id, self.module)
    }

    /// Crate-internal: refuse values that belong to a different module.
    /// The lifetime brand catches most cross-module mixing at compile
    /// time, but constants pulled from `'static` callers (or a future
    /// shared `TypePool`) need an explicit runtime check.
    fn require_same_module(&self, v: Value<'ctx>) -> IrResult<()> {
        if v.module().id() != self.module.id() {
            return Err(IrError::ForeignValue);
        }
        Ok(())
    }

    /// Build the `ret` payload and append. Crate-internal: the typed
    /// `build_ret` methods funnel here after their per-marker
    /// validation. Cannot fail by construction (same-module check is
    /// enforced upstream).
    fn append_ret(&self, value: Option<Value<'ctx>>) -> Instruction<'ctx> {
        let payload = ReturnOpData {
            value: value.map(|v| v.id),
        };
        let void_ty = self.module.void_type().as_type().id();
        self.append_instruction(void_ty, InstructionKindData::Ret(payload), "")
    }
}

// --------------------------------------------------------------------------
// Per-marker `build_ret` dispatch
// --------------------------------------------------------------------------

impl<'ctx, F, W> IRBuilder<'ctx, F, Positioned, RInt<W>>
where
    F: IRBuilderFolder<'ctx>,
    W: IntWidth,
{
    /// Produce `ret <value>` for an `iN`-returning function. The
    /// operand width is enforced at compile time — `build_ret` on a
    /// `RInt<B32>` builder requires an `IntValue<B32>`.
    ///
    /// Cross-module mixing (rare, escapes the lifetime brand only via
    /// `'static` constants or a future shared pool) errors with
    /// [`IrError::ForeignValue`]; everything else is statically
    /// guaranteed.
    pub fn build_ret(&self, value: IntValue<'ctx, W>) -> IrResult<Instruction<'ctx>> {
        let v = value.as_value();
        self.require_same_module(v)?;
        Ok(self.append_ret(Some(v)))
    }
}

impl<'ctx, F, K> IRBuilder<'ctx, F, Positioned, RFloat<K>>
where
    F: IRBuilderFolder<'ctx>,
    K: FloatKind,
{
    /// Produce `ret <value>` for a float-returning function. The
    /// operand kind is enforced at compile time.
    pub fn build_ret(
        &self,
        value: crate::value::FloatValue<'ctx, K>,
    ) -> IrResult<Instruction<'ctx>> {
        let v = value.as_value();
        self.require_same_module(v)?;
        Ok(self.append_ret(Some(v)))
    }
}

impl<'ctx, F> IRBuilder<'ctx, F, Positioned, RPtr>
where
    F: IRBuilderFolder<'ctx>,
{
    /// Produce `ret <ptr>` for a pointer-returning function.
    pub fn build_ret(
        &self,
        value: crate::value::PointerValue<'ctx>,
    ) -> IrResult<Instruction<'ctx>> {
        let v = value.as_value();
        self.require_same_module(v)?;
        Ok(self.append_ret(Some(v)))
    }
}

impl<'ctx, F> IRBuilder<'ctx, F, Positioned, RVoid>
where
    F: IRBuilderFolder<'ctx>,
{
    /// Produce `ret void`. Mirrors `IRBuilder::CreateRetVoid`. The
    /// `RVoid` builder does not expose `build_ret(value)` at all, so
    /// `build_ret_void` is the only return option.
    pub fn build_ret_void(&self) -> Instruction<'ctx> {
        self.append_ret(None)
    }
}

impl<'ctx, F> IRBuilder<'ctx, F, Positioned, RDyn>
where
    F: IRBuilderFolder<'ctx>,
{
    /// Produce `ret <value>` against a runtime-checked
    /// [`RDyn`]-marked builder. Errors with
    /// [`IrError::ReturnTypeMismatch`] if the value's type does not
    /// match the parent function's declared return type.
    pub fn build_ret<V>(&self, value: V) -> IrResult<Instruction<'ctx>>
    where
        V: crate::value::IsValue<'ctx>,
    {
        let v = value.as_value();
        self.require_same_module(v)?;
        let parent_fn = self.parent_function_dyn();
        let expected = parent_fn.return_type();
        if v.ty().id() != expected.id() {
            return Err(IrError::ReturnTypeMismatch {
                expected: expected.kind_label(),
                got: v.ty().kind_label(),
            });
        }
        Ok(self.append_ret(Some(v)))
    }

    /// Produce `ret void`. Errors with
    /// [`IrError::ReturnTypeMismatch`] if the parent function does
    /// not actually return `void`.
    pub fn build_ret_void(&self) -> IrResult<Instruction<'ctx>> {
        let parent_fn = self.parent_function_dyn();
        let expected = parent_fn.return_type();
        if !expected.is_void() {
            return Err(IrError::ReturnTypeMismatch {
                expected: expected.kind_label(),
                got: TypeKindLabel::Void,
            });
        }
        Ok(self.append_ret(None))
    }

    /// Owning function of the current insertion block, in its
    /// runtime-checked form.
    fn parent_function_dyn(&self) -> FunctionValue<'ctx, RDyn> {
        let bb = self.insert_block();
        let parent_id = bb.parent_id().unwrap_or_else(|| {
            unreachable!("Positioned builder block always has a parent function")
        });
        FunctionValue::<RDyn>::from_parts_unchecked(parent_id, self.module)
    }
}

// `require_same_int_width` is no longer needed: the IRBuilder's binary-
// op methods are parametrised by `W: IntWidth`, so mismatched widths
// are a compile-time error rather than a runtime check.
