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
use crate::function::FunctionValue;
use crate::instr_types::{BinaryOpData, CastOpData, ReturnOpData};
use crate::instruction::{Instruction, InstructionKindData, build_instruction_value};
use crate::int_width::IntWidth;
use crate::ir_builder::constant_folder::ConstantFolder;
use crate::ir_builder::folder::IRBuilderFolder;
use crate::marker::{Dyn, Ptr, ReturnMarker};
use crate::module::Module;
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

impl<'ctx> IRBuilder<'ctx, ConstantFolder, Unpositioned, Dyn> {
    /// Construct an unpositioned builder using the default
    /// [`ConstantFolder`]. The runtime-checked [`Dyn`] return marker
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
    /// let b = IRBuilder::new_for::<i32>(&module);
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
    /// Operands share width `W` -- enforced at compile time by the
    /// type system. Either side accepts any [`crate::IntoIntValue<'ctx, W>`]:
    /// already-typed [`IntValue`]s, [`crate::ConstantIntValue`]s, and
    /// Rust scalar literals (`5_i32`, `true`, ...) all work.
    pub fn build_int_add<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        let lhs = lhs.into_int_value(self.module)?;
        let rhs = rhs.into_int_value(self.module)?;
        self.require_same_module(lhs.as_value())?;
        self.require_same_module(rhs.as_value())?;
        if let Some(folded) = self.folder.fold_int_add(lhs.as_value(), rhs.as_value()) {
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
    pub fn build_int_sub<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        let lhs = lhs.into_int_value(self.module)?;
        let rhs = rhs.into_int_value(self.module)?;
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
    pub fn build_int_mul<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        let lhs = lhs.into_int_value(self.module)?;
        let rhs = rhs.into_int_value(self.module)?;
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

    /// Produce `udiv lhs, rhs`. Mirrors `IRBuilder::CreateUDiv`.
    pub fn build_int_udiv<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::UDiv)
    }

    /// Produce `sdiv lhs, rhs`. Mirrors `IRBuilder::CreateSDiv`.
    pub fn build_int_sdiv<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::SDiv)
    }

    /// Produce `urem lhs, rhs`. Mirrors `IRBuilder::CreateURem`.
    pub fn build_int_urem<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::URem)
    }

    /// Produce `srem lhs, rhs`. Mirrors `IRBuilder::CreateSRem`.
    pub fn build_int_srem<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::SRem)
    }

    /// Produce `shl lhs, rhs`. Mirrors `IRBuilder::CreateShl`.
    pub fn build_int_shl<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::Shl)
    }

    /// Produce `lshr lhs, rhs`. Mirrors `IRBuilder::CreateLShr`.
    pub fn build_int_lshr<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::LShr)
    }

    /// Produce `ashr lhs, rhs`. Mirrors `IRBuilder::CreateAShr`.
    pub fn build_int_ashr<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::AShr)
    }

    /// Produce `and lhs, rhs`. Mirrors `IRBuilder::CreateAnd`.
    pub fn build_int_and<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::And)
    }

    /// Produce `or lhs, rhs`. Mirrors `IRBuilder::CreateOr`.
    pub fn build_int_or<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::Or)
    }

    /// Produce `xor lhs, rhs`. Mirrors `IRBuilder::CreateXor`.
    pub fn build_int_xor<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop(lhs, rhs, name, InstructionKindData::Xor)
    }

    /// Produce `add lhs, rhs` with explicit [`crate::AddFlags`]. Mirrors
    /// `IRBuilder::CreateAdd` plus the `nuw`/`nsw` knobs. The flag
    /// set type only exposes flags LLVM accepts on `add`, so
    /// invalid combinations are a compile error.
    pub fn build_int_add_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::AddFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::Add)
    }

    /// Produce `sub lhs, rhs` with explicit [`crate::SubFlags`].
    pub fn build_int_sub_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::SubFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::Sub)
    }

    /// Produce `mul lhs, rhs` with explicit [`crate::MulFlags`].
    pub fn build_int_mul_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::MulFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::Mul)
    }

    /// Produce `shl lhs, rhs` with explicit [`crate::ShlFlags`].
    pub fn build_int_shl_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::ShlFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::Shl)
    }

    /// Produce `udiv lhs, rhs` with explicit [`crate::UDivFlags`].
    pub fn build_int_udiv_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::UDivFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::UDiv)
    }

    /// Produce `sdiv lhs, rhs` with explicit [`crate::SDivFlags`].
    pub fn build_int_sdiv_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::SDivFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::SDiv)
    }

    /// Produce `lshr lhs, rhs` with explicit [`crate::LShrFlags`].
    pub fn build_int_lshr_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::LShrFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::LShr)
    }

    /// Produce `ashr lhs, rhs` with explicit [`crate::AShrFlags`].
    pub fn build_int_ashr_with_flags<W, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::AShrFlags,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        self.build_int_binop_flagged(lhs, rhs, name, flags, InstructionKindData::AShr)
    }

    /// Crate-internal helper: emit a flagged binary op. The flag
    /// type's `WriteBinopFlags` impl writes its bits onto the
    /// payload; the kind constructor lifts the payload into the
    /// matching `InstructionKindData` variant.
    fn build_int_binop_flagged<W, Lhs, Rhs, Flags, Kind>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
        flags: Flags,
        kind_ctor: Kind,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
        Flags: crate::instr_types::WriteBinopFlags,
        Kind: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let lhs = lhs.into_int_value(self.module)?;
        let rhs = rhs.into_int_value(self.module)?;
        self.require_same_module(lhs.as_value())?;
        self.require_same_module(rhs.as_value())?;
        let mut payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        flags.apply(&mut payload);
        let inst = self.append_instruction(lhs.ty().as_type().id(), kind_ctor(payload), name);
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    /// Crate-internal helper: emit a binary op given a callback that
    /// wraps the payload into an [`InstructionKindData`] variant.
    /// Folder bypassed for opcodes the constant folder doesn't yet
    /// handle (everything except `Add`/`Sub`/`Mul`).
    fn build_int_binop<W, Lhs, Rhs, F2>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
        kind_ctor: F2,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let lhs = lhs.into_int_value(self.module)?;
        let rhs = rhs.into_int_value(self.module)?;
        self.require_same_module(lhs.as_value())?;
        self.require_same_module(rhs.as_value())?;
        let payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(lhs.ty().as_type().id(), kind_ctor(payload), name);
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    // ---- Floating-point arithmetic ----

    /// Produce `fadd lhs, rhs`. Mirrors `IRBuilder::CreateFAdd`.
    pub fn build_fp_add<K, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        K: crate::float_kind::FloatKind,
        Lhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        Rhs: crate::float_kind::IntoFloatValue<'ctx, K>,
    {
        self.build_fp_binop(lhs, rhs, name, InstructionKindData::FAdd)
    }

    /// Produce `fsub lhs, rhs`. Mirrors `IRBuilder::CreateFSub`.
    pub fn build_fp_sub<K, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        K: crate::float_kind::FloatKind,
        Lhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        Rhs: crate::float_kind::IntoFloatValue<'ctx, K>,
    {
        self.build_fp_binop(lhs, rhs, name, InstructionKindData::FSub)
    }

    /// Produce `fmul lhs, rhs`. Mirrors `IRBuilder::CreateFMul`.
    pub fn build_fp_mul<K, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        K: crate::float_kind::FloatKind,
        Lhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        Rhs: crate::float_kind::IntoFloatValue<'ctx, K>,
    {
        self.build_fp_binop(lhs, rhs, name, InstructionKindData::FMul)
    }

    /// Produce `fdiv lhs, rhs`. Mirrors `IRBuilder::CreateFDiv`.
    pub fn build_fp_div<K, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        K: crate::float_kind::FloatKind,
        Lhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        Rhs: crate::float_kind::IntoFloatValue<'ctx, K>,
    {
        self.build_fp_binop(lhs, rhs, name, InstructionKindData::FDiv)
    }

    /// Produce `frem lhs, rhs`. Mirrors `IRBuilder::CreateFRem`.
    pub fn build_fp_rem<K, Lhs, Rhs>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        K: crate::float_kind::FloatKind,
        Lhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        Rhs: crate::float_kind::IntoFloatValue<'ctx, K>,
    {
        self.build_fp_binop(lhs, rhs, name, InstructionKindData::FRem)
    }

    /// Crate-internal helper for float binops. Same shape as
    /// `build_int_binop` but parameterised by `K: FloatKind`.
    fn build_fp_binop<K, Lhs, Rhs, F2>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
        kind_ctor: F2,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        K: crate::float_kind::FloatKind,
        Lhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        Rhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let lhs = lhs.into_float_value(self.module)?;
        let rhs = rhs.into_float_value(self.module)?;
        self.require_same_module(crate::value::IsValue::as_value(lhs))?;
        self.require_same_module(crate::value::IsValue::as_value(rhs))?;
        let payload = BinaryOpData::new(
            crate::value::IsValue::as_value(lhs).id,
            crate::value::IsValue::as_value(rhs).id,
        );
        let inst =
            self.append_instruction(crate::value::Typed::ty(lhs).id(), kind_ctor(payload), name);
        Ok(crate::value::FloatValue::<K>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Produce `fcmp <pred> lhs, rhs`. Mirrors
    /// `IRBuilder::CreateFCmp`. Result is `i1`.
    pub fn build_fp_cmp<K, Lhs, Rhs>(
        &self,
        pred: crate::cmp_predicate::FloatPredicate,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, bool>>
    where
        K: crate::float_kind::FloatKind,
        Lhs: crate::float_kind::IntoFloatValue<'ctx, K>,
        Rhs: crate::float_kind::IntoFloatValue<'ctx, K>,
    {
        let lhs = lhs.into_float_value(self.module)?;
        let rhs = rhs.into_float_value(self.module)?;
        self.require_same_module(crate::value::IsValue::as_value(lhs))?;
        self.require_same_module(crate::value::IsValue::as_value(rhs))?;
        let payload = crate::instr_types::FCmpInstData::new(
            pred,
            crate::value::IsValue::as_value(lhs).id,
            crate::value::IsValue::as_value(rhs).id,
        );
        let i1_ty = self.module.bool_type().as_type().id();
        let inst = self.append_instruction(i1_ty, InstructionKindData::FCmp(payload), name);
        Ok(IntValue::<bool>::from_value_unchecked(inst.as_value()))
    }

    // ---- Casts: trunc / zext / sext ----

    /// Produce `trunc <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateTrunc`.
    ///
    /// The `Src: WiderThan<Dst>` bound enforces at compile time that
    /// the destination is strictly narrower than the source. Cross-
    /// width attempts (e.g. `build_trunc::<i32, i64>`) fail to
    /// compile rather than returning a runtime
    /// [`IrError::OperandWidthMismatch`]. Use
    /// [`Self::build_trunc_dyn`] when both widths are erased.
    ///
    /// The remaining `IrResult` failure mode is
    /// [`IrError::ForeignValue`] (cross-module operand).
    pub fn build_trunc<Src, Dst>(
        &self,
        value: IntValue<'ctx, Src>,
        dst_ty: IntType<'ctx, Dst>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, Dst>>
    where
        Src: crate::int_width::WiderThan<Dst>,
        Dst: IntWidth,
    {
        self.require_same_module(value.as_value())?;
        let payload = CastOpData::new(crate::instr_types::CastOpcode::Trunc, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `zext <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateZExt`.
    ///
    /// The `Dst: WiderThan<Src>` bound enforces at compile time that
    /// the destination is strictly wider than the source. Use
    /// [`Self::build_zext_dyn`] when both widths are erased.
    pub fn build_zext<Src, Dst>(
        &self,
        value: IntValue<'ctx, Src>,
        dst_ty: IntType<'ctx, Dst>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, Dst>>
    where
        Src: IntWidth,
        Dst: crate::int_width::WiderThan<Src>,
    {
        self.require_same_module(value.as_value())?;
        let payload = CastOpData::new(crate::instr_types::CastOpcode::ZExt, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `sext <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateSExt`.
    ///
    /// The `Dst: WiderThan<Src>` bound enforces at compile time that
    /// the destination is strictly wider than the source. Use
    /// [`Self::build_sext_dyn`] when both widths are erased.
    pub fn build_sext<Src, Dst>(
        &self,
        value: IntValue<'ctx, Src>,
        dst_ty: IntType<'ctx, Dst>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, Dst>>
    where
        Src: IntWidth,
        Dst: crate::int_width::WiderThan<Src>,
    {
        self.require_same_module(value.as_value())?;
        let payload = CastOpData::new(crate::instr_types::CastOpcode::SExt, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst>::from_value_unchecked(inst.as_value()))
    }

    // ---- Dyn fallbacks (runtime-checked) ----

    /// Runtime-checked `trunc` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly narrower than `value`'s runtime width.
    pub fn build_trunc_dyn(
        &self,
        value: IntValue<'ctx, crate::int_width::IntDyn>,
        dst_ty: IntType<'ctx, crate::int_width::IntDyn>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, crate::int_width::IntDyn>> {
        self.require_same_module(value.as_value())?;
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w >= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::Trunc, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<crate::int_width::IntDyn>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Runtime-checked `zext` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly wider than `value`'s runtime width.
    pub fn build_zext_dyn(
        &self,
        value: IntValue<'ctx, crate::int_width::IntDyn>,
        dst_ty: IntType<'ctx, crate::int_width::IntDyn>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, crate::int_width::IntDyn>> {
        self.build_int_extend_dyn(value, dst_ty, name, crate::instr_types::CastOpcode::ZExt)
    }

    /// Runtime-checked `sext` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly wider than `value`'s runtime width.
    pub fn build_sext_dyn(
        &self,
        value: IntValue<'ctx, crate::int_width::IntDyn>,
        dst_ty: IntType<'ctx, crate::int_width::IntDyn>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, crate::int_width::IntDyn>> {
        self.build_int_extend_dyn(value, dst_ty, name, crate::instr_types::CastOpcode::SExt)
    }

    /// Crate-internal helper for `build_zext_dyn` / `build_sext_dyn`.
    fn build_int_extend_dyn(
        &self,
        value: IntValue<'ctx, crate::int_width::IntDyn>,
        dst_ty: IntType<'ctx, crate::int_width::IntDyn>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<IntValue<'ctx, crate::int_width::IntDyn>> {
        self.require_same_module(value.as_value())?;
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w <= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        let payload = CastOpData::new(opcode, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<crate::int_width::IntDyn>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    // ---- Memory: alloca / load / store ----

    /// Produce `alloca <ty>`. Mirrors `IRBuilder::CreateAlloca`.
    /// The result is a `ptr` in the default address space.
    pub fn build_alloca<T>(
        &self,
        ty: T,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        T: crate::r#type::IrType<'ctx>,
    {
        self.build_alloca_inner(
            ty.as_type().id(),
            None,
            crate::align::MaybeAlign::NONE,
            0,
            name,
        )
    }

    /// Produce `alloca <ty>, <size-ty> <num_elements>`. Mirrors
    /// `IRBuilder::CreateAlloca` with an array-size operand.
    pub fn build_array_alloca<T, N>(
        &self,
        ty: T,
        num_elements: N,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        T: crate::r#type::IrType<'ctx>,
        N: crate::int_width::IntoIntValue<'ctx, crate::int_width::IntDyn>,
    {
        let n = num_elements.into_int_value(self.module)?;
        self.build_alloca_inner(
            ty.as_type().id(),
            Some(n.as_value().id),
            crate::align::MaybeAlign::NONE,
            0,
            name,
        )
    }

    /// Produce `alloca <ty>, align <N>`. Mirrors
    /// `IRBuilder::CreateAlignedAlloca`.
    pub fn build_alloca_with_align<T>(
        &self,
        ty: T,
        align: crate::align::Align,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        T: crate::r#type::IrType<'ctx>,
    {
        self.build_alloca_inner(
            ty.as_type().id(),
            None,
            crate::align::MaybeAlign::new(align),
            0,
            name,
        )
    }

    fn build_alloca_inner(
        &self,
        allocated_ty: TypeId,
        num_elements: Option<crate::value::ValueId>,
        align: crate::align::MaybeAlign,
        addr_space: u32,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>> {
        if let Some(id) = num_elements {
            let v = crate::value::Value::from_parts(id, self.module, {
                self.module.context().value_data(id).ty
            });
            self.require_same_module(v)?;
        }
        let payload =
            crate::instr_types::AllocaInstData::new(allocated_ty, num_elements, align, addr_space);
        let ptr_ty = self.module.ptr_type(addr_space).as_type().id();
        let inst = self.append_instruction(ptr_ty, InstructionKindData::Alloca(payload), name);
        Ok(crate::value::PointerValue::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Erased load: `load <ty>, ptr <ptr>`. Result type is whatever
    /// `ty` decodes to at runtime; returned as a [`Value`] handle the
    /// caller narrows via `try_into()`. Mirrors
    /// `IRBuilder::CreateLoad`.
    pub fn build_load<T, P>(&self, ty: T, ptr: P, name: impl AsRef<str>) -> IrResult<Value<'ctx>>
    where
        T: crate::r#type::IrType<'ctx>,
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(self.module)?;
        let inst = self.build_load_inner(ty_id, p, crate::align::MaybeAlign::NONE, false, name)?;
        Ok(inst.as_value())
    }

    /// Typed integer load: `load iN, ptr <ptr>`. Marker-only form:
    /// the result type comes from `W` via [`crate::StaticIntWidth`].
    /// Mirrors `IRBuilder::CreateLoad` with a fixed integer width.
    pub fn build_int_load<W, P>(&self, ptr: P, name: impl AsRef<str>) -> IrResult<IntValue<'ctx, W>>
    where
        W: crate::int_width::StaticIntWidth,
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let ty = W::ir_type(self.module);
        let p = ptr.into_pointer_value(self.module)?;
        let inst = self.build_load_inner(
            ty.as_type().id(),
            p,
            crate::align::MaybeAlign::NONE,
            false,
            name,
        )?;
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    /// Runtime-width integer load. Takes the type explicitly because
    /// the [`crate::IntDyn`] marker carries no static width.
    pub fn build_int_load_dyn<P>(
        &self,
        ty: IntType<'ctx, crate::int_width::IntDyn>,
        ptr: P,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, crate::int_width::IntDyn>>
    where
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let p = ptr.into_pointer_value(self.module)?;
        let inst = self.build_load_inner(
            ty.as_type().id(),
            p,
            crate::align::MaybeAlign::NONE,
            false,
            name,
        )?;
        Ok(IntValue::<crate::int_width::IntDyn>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Typed float load: `load <fpty>, ptr <ptr>`. Marker-only.
    pub fn build_fp_load<K, P>(
        &self,
        ptr: P,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        K: crate::float_kind::StaticFloatKind,
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let ty = K::ir_type(self.module);
        let p = ptr.into_pointer_value(self.module)?;
        let inst = self.build_load_inner(
            ty.as_type().id(),
            p,
            crate::align::MaybeAlign::NONE,
            false,
            name,
        )?;
        Ok(crate::value::FloatValue::<K>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Runtime-kind float load. Takes the type explicitly because
    /// [`crate::FloatDyn`] carries no static kind.
    pub fn build_fp_load_dyn<P>(
        &self,
        ty: crate::derived_types::FloatType<'ctx, crate::float_kind::FloatDyn>,
        ptr: P,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, crate::float_kind::FloatDyn>>
    where
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let p = ptr.into_pointer_value(self.module)?;
        let inst = self.build_load_inner(
            ty.as_type().id(),
            p,
            crate::align::MaybeAlign::NONE,
            false,
            name,
        )?;
        Ok(
            crate::value::FloatValue::<crate::float_kind::FloatDyn>::from_value_unchecked(
                inst.as_value(),
            ),
        )
    }

    /// Pointer-typed load: `load ptr, ptr <ptr>`. Pointer types are
    /// uniform (only address space varies); the loaded ptr is in the
    /// default address space. Use [`Self::build_load`] erased form for
    /// other address spaces.
    pub fn build_pointer_load<P>(
        &self,
        ptr: P,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let ty = self.module.ptr_type(0);
        let p = ptr.into_pointer_value(self.module)?;
        let inst = self.build_load_inner(
            ty.as_type().id(),
            p,
            crate::align::MaybeAlign::NONE,
            false,
            name,
        )?;
        Ok(crate::value::PointerValue::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Same as [`Self::build_int_load`] plus an explicit alignment.
    pub fn build_int_load_with_align<W, P>(
        &self,
        ptr: P,
        align: crate::align::Align,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: crate::int_width::StaticIntWidth,
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let ty = W::ir_type(self.module);
        let p = ptr.into_pointer_value(self.module)?;
        let inst = self.build_load_inner(
            ty.as_type().id(),
            p,
            crate::align::MaybeAlign::new(align),
            false,
            name,
        )?;
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    fn build_load_inner(
        &self,
        pointee_ty: TypeId,
        ptr: crate::value::PointerValue<'ctx>,
        align: crate::align::MaybeAlign,
        volatile: bool,
        name: impl AsRef<str>,
    ) -> IrResult<Instruction<'ctx>> {
        self.require_same_module(crate::value::IsValue::as_value(ptr))?;
        let payload = crate::instr_types::LoadInstData::new(
            pointee_ty,
            crate::value::IsValue::as_value(ptr).id,
            align,
            volatile,
        );
        Ok(self.append_instruction(pointee_ty, InstructionKindData::Load(payload), name))
    }

    /// Produce `store <value>, ptr <ptr>`. Mirrors
    /// `IRBuilder::CreateStore`.
    pub fn build_store<V, P>(
        &self,
        value: V,
        ptr: P,
    ) -> IrResult<crate::instructions::StoreInst<'ctx>>
    where
        V: crate::value::IsValue<'ctx>,
        P: crate::value::IntoPointerValue<'ctx>,
    {
        self.build_store_inner(value, ptr, crate::align::MaybeAlign::NONE, false)
    }

    /// Same as `build_store` plus an explicit alignment slot.
    pub fn build_store_with_align<V, P>(
        &self,
        value: V,
        ptr: P,
        align: crate::align::Align,
    ) -> IrResult<crate::instructions::StoreInst<'ctx>>
    where
        V: crate::value::IsValue<'ctx>,
        P: crate::value::IntoPointerValue<'ctx>,
    {
        self.build_store_inner(value, ptr, crate::align::MaybeAlign::new(align), false)
    }

    fn build_store_inner<V, P>(
        &self,
        value: V,
        ptr: P,
        align: crate::align::MaybeAlign,
        volatile: bool,
    ) -> IrResult<crate::instructions::StoreInst<'ctx>>
    where
        V: crate::value::IsValue<'ctx>,
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let v = value.as_value();
        let p = ptr.into_pointer_value(self.module)?;
        self.require_same_module(v)?;
        self.require_same_module(crate::value::IsValue::as_value(p))?;
        let payload = crate::instr_types::StoreInstData::new(
            v.id,
            crate::value::IsValue::as_value(p).id,
            align,
            volatile,
        );
        let void_ty = self.module.void_type().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Store(payload), "");
        Ok({
            let _i = inst;
            crate::instructions::StoreInst::from_raw(_i.as_value().id, _i.module(), _i.ty().id())
        })
    }

    // ---- Call ----

    /// Flat call form: pass a [`FunctionValue`] callee, an iterable of
    /// pre-widened arguments (each one already a [`Value<'ctx>`]), and
    /// a name. Mirrors the simple shape of `IRBuilder::CreateCall`.
    /// Use [`Self::call_builder`] for mixed-arg-type construction.
    pub fn build_call<R2, I, V>(
        &self,
        callee: FunctionValue<'ctx, R2>,
        args: I,
        name: impl AsRef<str>,
    ) -> IrResult<crate::instructions::CallInst<'ctx>>
    where
        R2: crate::marker::ReturnMarker,
        I: IntoIterator<Item = V>,
        V: crate::value::IsValue<'ctx>,
    {
        let mut builder = self.call_builder(callee).name(name);
        for arg in args {
            builder = builder.arg(arg);
        }
        builder.build()
    }

    /// Builder-pattern call construction. Returns a
    /// [`CallBuilder`] that accumulates per-arg / flag state via
    /// chainable methods, then emits the call on `.build()`. Each
    /// `.arg()` call is statically dispatched (no `dyn`); arg types
    /// can vary across calls.
    pub fn call_builder<R2: crate::marker::ReturnMarker>(
        &self,
        callee: FunctionValue<'ctx, R2>,
    ) -> CallBuilder<'_, 'ctx, F, R> {
        CallBuilder {
            parent: self,
            callee_id: callee.as_value().id,
            fn_ty: callee.signature().as_type().id(),
            return_ty: callee.return_type().id(),
            args: Vec::new(),
            calling_conv: callee.calling_conv(),
            tail_kind: crate::instr_types::TailCallKind::None,
            name: String::new(),
            _r: PhantomData,
        }
    }

    // ---- GEP ----

    /// Produce `getelementptr <source-ty>, ptr <ptr>, <indices>`.
    /// Mirrors `IRBuilder::CreateGEP`.
    pub fn build_gep<T, P, I, V>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        T: crate::r#type::IrType<'ctx>,
        P: crate::value::IntoPointerValue<'ctx>,
        I: IntoIterator<Item = V>,
        V: crate::int_width::IntoIntValue<'ctx, crate::int_width::IntDyn>,
    {
        self.build_gep_inner(
            source_ty,
            ptr,
            indices,
            crate::gep_no_wrap_flags::GepNoWrapFlags::empty(),
            name,
        )
    }

    /// Produce `getelementptr inbounds <source-ty>, ptr <ptr>,
    /// <indices>`. Mirrors `IRBuilder::CreateInBoundsGEP`.
    pub fn build_inbounds_gep<T, P, I, V>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        T: crate::r#type::IrType<'ctx>,
        P: crate::value::IntoPointerValue<'ctx>,
        I: IntoIterator<Item = V>,
        V: crate::int_width::IntoIntValue<'ctx, crate::int_width::IntDyn>,
    {
        self.build_gep_inner(
            source_ty,
            ptr,
            indices,
            crate::gep_no_wrap_flags::GepNoWrapFlags::inbounds(),
            name,
        )
    }

    /// Produce `getelementptr inbounds <struct-ty>, ptr <ptr>,
    /// i32 0, i32 <field-idx>`. Mirrors `IRBuilder::CreateStructGEP`.
    pub fn build_struct_gep<P>(
        &self,
        struct_ty: crate::derived_types::StructType<'ctx>,
        ptr: P,
        idx: u32,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        P: crate::value::IntoPointerValue<'ctx>,
    {
        let i32_ty = self.module.i32_type();
        let zero = i32_ty.const_zero().as_dyn();
        let idx_val = i32_ty
            .const_int(i32::try_from(idx).map_err(|_| IrError::InvalidOperation {
                message: "struct field index exceeds i32::MAX",
            })?)
            .as_dyn();
        self.build_gep_inner(
            struct_ty,
            ptr,
            [zero, idx_val],
            crate::gep_no_wrap_flags::GepNoWrapFlags::inbounds(),
            name,
        )
    }

    fn build_gep_inner<T, P, I, V>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        flags: crate::gep_no_wrap_flags::GepNoWrapFlags,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        T: crate::r#type::IrType<'ctx>,
        P: crate::value::IntoPointerValue<'ctx>,
        I: IntoIterator<Item = V>,
        V: crate::int_width::IntoIntValue<'ctx, crate::int_width::IntDyn>,
    {
        let source_ty_id = source_ty.as_type().id();
        let p = ptr.into_pointer_value(self.module)?;
        self.require_same_module(crate::value::IsValue::as_value(p))?;
        let mut idx_ids = Vec::new();
        for index in indices {
            let iv = index.into_int_value(self.module)?;
            self.require_same_module(iv.as_value())?;
            idx_ids.push(iv.as_value().id);
        }
        let payload = crate::instr_types::GepInstData::new(
            source_ty_id,
            crate::value::IsValue::as_value(p).id,
            idx_ids.into_boxed_slice(),
            flags,
        );
        let result_ty = self.module.ptr_type(0).as_type().id();
        let inst = self.append_instruction(result_ty, InstructionKindData::Gep(payload), name);
        Ok(crate::value::PointerValue::from_value_unchecked(
            inst.as_value(),
        ))
    }

    // ---- Floating-point casts ----

    /// Produce `fpext <value> to <dst>`. Compile-time check:
    /// `Dst: FloatWiderThan<Src>`. Mirrors `IRBuilder::CreateFPExt`.
    pub fn build_fp_ext<Src, Dst>(
        &self,
        value: crate::value::FloatValue<'ctx, Src>,
        dst_ty: crate::derived_types::FloatType<'ctx, Dst>,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, Dst>>
    where
        Src: crate::float_kind::FloatKind,
        Dst: crate::float_kind::FloatKind + crate::float_kind::FloatWiderThan<Src>,
    {
        self.build_fp_cast(value, dst_ty, name, crate::instr_types::CastOpcode::FpExt)
    }

    /// Produce `fptrunc <value> to <dst>`. Compile-time check:
    /// `Src: FloatWiderThan<Dst>`. Mirrors `IRBuilder::CreateFPTrunc`.
    pub fn build_fp_trunc<Src, Dst>(
        &self,
        value: crate::value::FloatValue<'ctx, Src>,
        dst_ty: crate::derived_types::FloatType<'ctx, Dst>,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, Dst>>
    where
        Src: crate::float_kind::FloatKind + crate::float_kind::FloatWiderThan<Dst>,
        Dst: crate::float_kind::FloatKind,
    {
        self.build_fp_cast(value, dst_ty, name, crate::instr_types::CastOpcode::FpTrunc)
    }

    /// Crate-internal helper for `build_fp_ext` / `build_fp_trunc`.
    fn build_fp_cast<Src, Dst>(
        &self,
        value: crate::value::FloatValue<'ctx, Src>,
        dst_ty: crate::derived_types::FloatType<'ctx, Dst>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<crate::value::FloatValue<'ctx, Dst>>
    where
        Src: crate::float_kind::FloatKind,
        Dst: crate::float_kind::FloatKind,
    {
        let v = crate::value::IsValue::as_value(value);
        self.require_same_module(v)?;
        let payload = CastOpData::new(opcode, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(crate::value::FloatValue::<Dst>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Produce `fptoui <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateFPToUI`.
    pub fn build_fp_to_ui<K, W>(
        &self,
        value: crate::value::FloatValue<'ctx, K>,
        dst_ty: IntType<'ctx, W>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        K: crate::float_kind::FloatKind,
        W: IntWidth,
    {
        self.build_fp_to_int(value, dst_ty, name, crate::instr_types::CastOpcode::FpToUI)
    }

    /// Produce `fptosi <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateFPToSI`.
    pub fn build_fp_to_si<K, W>(
        &self,
        value: crate::value::FloatValue<'ctx, K>,
        dst_ty: IntType<'ctx, W>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        K: crate::float_kind::FloatKind,
        W: IntWidth,
    {
        self.build_fp_to_int(value, dst_ty, name, crate::instr_types::CastOpcode::FpToSI)
    }

    fn build_fp_to_int<K, W>(
        &self,
        value: crate::value::FloatValue<'ctx, K>,
        dst_ty: IntType<'ctx, W>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        K: crate::float_kind::FloatKind,
        W: IntWidth,
    {
        let v = crate::value::IsValue::as_value(value);
        self.require_same_module(v)?;
        let payload = CastOpData::new(opcode, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `uitofp <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateUIToFP`.
    pub fn build_ui_to_fp<W, K>(
        &self,
        value: IntValue<'ctx, W>,
        dst_ty: crate::derived_types::FloatType<'ctx, K>,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        W: IntWidth,
        K: crate::float_kind::FloatKind,
    {
        self.build_int_to_fp(value, dst_ty, name, crate::instr_types::CastOpcode::UIToFp)
    }

    /// Produce `sitofp <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateSIToFP`.
    pub fn build_si_to_fp<W, K>(
        &self,
        value: IntValue<'ctx, W>,
        dst_ty: crate::derived_types::FloatType<'ctx, K>,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        W: IntWidth,
        K: crate::float_kind::FloatKind,
    {
        self.build_int_to_fp(value, dst_ty, name, crate::instr_types::CastOpcode::SIToFp)
    }

    fn build_int_to_fp<W, K>(
        &self,
        value: IntValue<'ctx, W>,
        dst_ty: crate::derived_types::FloatType<'ctx, K>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<crate::value::FloatValue<'ctx, K>>
    where
        W: IntWidth,
        K: crate::float_kind::FloatKind,
    {
        let v = value.as_value();
        self.require_same_module(v)?;
        let payload = CastOpData::new(opcode, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(crate::value::FloatValue::<K>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    // ---- Pointer casts ----

    /// Produce `ptrtoint <value> to <dst>`. Mirrors
    /// `IRBuilder::CreatePtrToInt`.
    pub fn build_ptr_to_int<W>(
        &self,
        value: crate::value::PointerValue<'ctx>,
        dst_ty: IntType<'ctx, W>,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, W>>
    where
        W: IntWidth,
    {
        let v = crate::value::IsValue::as_value(value);
        self.require_same_module(v)?;
        let payload = CastOpData::new(crate::instr_types::CastOpcode::PtrToInt, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<W>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `inttoptr <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateIntToPtr`.
    pub fn build_int_to_ptr<W>(
        &self,
        value: IntValue<'ctx, W>,
        dst_ty: crate::derived_types::PointerType<'ctx>,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>>
    where
        W: IntWidth,
    {
        let v = value.as_value();
        self.require_same_module(v)?;
        let payload = CastOpData::new(crate::instr_types::CastOpcode::IntToPtr, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(crate::value::PointerValue::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Produce `addrspacecast <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateAddrSpaceCast`.
    pub fn build_addrspace_cast(
        &self,
        value: crate::value::PointerValue<'ctx>,
        dst_ty: crate::derived_types::PointerType<'ctx>,
        name: impl AsRef<str>,
    ) -> IrResult<crate::value::PointerValue<'ctx>> {
        let v = crate::value::IsValue::as_value(value);
        self.require_same_module(v)?;
        let payload = CastOpData::new(crate::instr_types::CastOpcode::AddrSpaceCast, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(crate::value::PointerValue::from_value_unchecked(
            inst.as_value(),
        ))
    }

    // ---- Integer comparison ----

    /// Produce `icmp <pred> <ty> <lhs>, <rhs>`. Mirrors
    /// `IRBuilder::CreateICmp`.
    ///
    /// Both operands share width `W` at the type level. The result
    /// type is always `i1` (`IntValue<'ctx, bool>`).
    pub fn build_int_cmp<W, Lhs, Rhs>(
        &self,
        pred: crate::cmp_predicate::IntPredicate,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
    ) -> IrResult<IntValue<'ctx, bool>>
    where
        W: IntWidth,
        Lhs: crate::int_width::IntoIntValue<'ctx, W>,
        Rhs: crate::int_width::IntoIntValue<'ctx, W>,
    {
        let lhs = lhs.into_int_value(self.module)?;
        let rhs = rhs.into_int_value(self.module)?;
        self.require_same_module(lhs.as_value())?;
        self.require_same_module(rhs.as_value())?;
        let payload =
            crate::instr_types::CmpInstData::new(pred, lhs.as_value().id, rhs.as_value().id);
        let i1_ty = self.module.bool_type().as_type().id();
        let inst = self.append_instruction(i1_ty, InstructionKindData::ICmp(payload), name);
        Ok(IntValue::<bool>::from_value_unchecked(inst.as_value()))
    }

    // ---- Phi ----

    /// Produce `phi <ty>` with no initial incoming edges. Marker-only
    /// form: the result type comes from the `W` type parameter via
    /// [`crate::StaticIntWidth`], so callers spell it as
    /// `b.build_int_phi::<i32>("acc")?` without first binding
    /// `let i32_ty = m.i32_type();`. Mirrors `IRBuilder::CreatePHI`
    /// followed by zero `PHINode::addIncoming` calls. Subsequent
    /// edges are added through [`crate::PhiInst::add_incoming`],
    /// which returns `Self` so calls chain.
    pub fn build_int_phi<W>(
        &self,
        name: impl AsRef<str>,
    ) -> IrResult<crate::instructions::PhiInst<'ctx, W>>
    where
        W: crate::int_width::StaticIntWidth,
    {
        let ty = W::ir_type(self.module);
        let payload = crate::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok({
            let _i = inst;
            crate::instructions::PhiInst::<W>::from_raw(_i.as_value().id, _i.module(), _i.ty().id())
        })
    }

    /// Runtime-width phi for the [`crate::IntDyn`] case. Takes the
    /// type explicitly because the marker carries no static width.
    pub fn build_int_phi_dyn(
        &self,
        ty: IntType<'ctx, crate::int_width::IntDyn>,
        name: impl AsRef<str>,
    ) -> IrResult<crate::instructions::PhiInst<'ctx, crate::int_width::IntDyn>> {
        let payload = crate::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok({
            let _i = inst;
            crate::instructions::PhiInst::<crate::int_width::IntDyn>::from_raw(
                _i.as_value().id,
                _i.module(),
                _i.ty().id(),
            )
        })
    }

    // ---- Branch / Unreachable ----

    /// Produce `br label %target`. Mirrors `IRBuilder::CreateBr`.
    pub fn build_br(&self, target: BasicBlock<'ctx, R>) -> IrResult<Instruction<'ctx>> {
        if target.as_value().module().id() != self.module.id() {
            return Err(IrError::ForeignValue);
        }
        let payload = crate::instr_types::BranchInstData {
            kind: crate::instr_types::BranchKind::Unconditional(target.as_value().id),
        };
        let void_ty = self.module.void_type().as_type().id();
        Ok(self.append_instruction(void_ty, InstructionKindData::Br(payload), ""))
    }

    /// Produce `br i1 <cond>, label %then, label %else`. Mirrors
    /// `IRBuilder::CreateCondBr`.
    pub fn build_cond_br<C>(
        &self,
        cond: C,
        then_bb: BasicBlock<'ctx, R>,
        else_bb: BasicBlock<'ctx, R>,
    ) -> IrResult<Instruction<'ctx>>
    where
        C: crate::int_width::IntoIntValue<'ctx, bool>,
    {
        let cond = cond.into_int_value(self.module)?;
        self.require_same_module(cond.as_value())?;
        if then_bb.as_value().module().id() != self.module.id()
            || else_bb.as_value().module().id() != self.module.id()
        {
            return Err(IrError::ForeignValue);
        }
        let payload = crate::instr_types::BranchInstData {
            kind: crate::instr_types::BranchKind::Conditional {
                cond: core::cell::Cell::new(cond.as_value().id),
                then_bb: then_bb.as_value().id,
                else_bb: else_bb.as_value().id,
            },
        };
        let void_ty = self.module.void_type().as_type().id();
        Ok(self.append_instruction(void_ty, InstructionKindData::Br(payload), ""))
    }

    /// Produce `unreachable`. Mirrors `IRBuilder::CreateUnreachable`.
    /// Infallible: no operands, no module brand to validate.
    pub fn build_unreachable(&self) -> Instruction<'ctx> {
        let payload = crate::instr_types::UnreachableInstData;
        let void_ty = self.module.void_type().as_type().id();
        self.append_instruction(void_ty, InstructionKindData::Unreachable(payload), "")
    }

    // ---- Internal helpers ----

    /// Crate-internal: append a freshly-built instruction to the
    /// insertion block. `name` populates the value-symbol-table when
    /// non-empty.
    fn append_instruction<N: AsRef<str>>(
        &self,
        ty: TypeId,
        kind: InstructionKindData,
        name: N,
    ) -> Instruction<'ctx> {
        let name = name.as_ref();
        let bb = self.insert_block();
        let bb_id = bb.as_value().id;
        let stored_name = (!name.is_empty()).then(|| name.to_owned());
        let value = build_instruction_value(ty, bb_id, kind, stored_name);
        // Snapshot operand ids before the value is moved into the arena;
        // we need them to register the new instruction in each operand's
        // reverse use-list. Mirrors `User::setOperand` in
        // `llvm/lib/IR/User.cpp`, which threads each `Use` into its
        // operand's use-list at construction time.
        let operand_ids = match &value.kind {
            crate::value::ValueKindData::Instruction(i) => i.kind.operand_ids(),
            // append_instruction always builds an Instruction-kind value.
            _ => unreachable!("append_instruction built non-instruction value"),
        };
        let id = self.module.context().push_value(value);
        for op in operand_ids {
            self.module
                .context()
                .value_data(op)
                .use_list
                .borrow_mut()
                .push(id);
        }
        bb.append_instruction(id);
        if !name.is_empty() {
            if let Some(parent_fn_id) = bb.parent_id() {
                let parent_fn =
                    FunctionValue::<Dyn>::from_parts_unchecked(parent_fn_id, self.module);
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
        let payload = ReturnOpData::new(value.map(|v| v.id));
        let void_ty = self.module.void_type().as_type().id();
        self.append_instruction(void_ty, InstructionKindData::Ret(payload), "")
    }
}

// --------------------------------------------------------------------------
// `build_ret` dispatch via the [`IntoReturnValue`] trait
// --------------------------------------------------------------------------
//
// Rust's coherence checker rejects two blanket impls (`<W: IntWidth>` +
// `<K: FloatKind>`) on `IRBuilder<R>` even when no type implements both
// traits. We dispatch through a single sealed trait that pins the
// return-value lift per concrete marker. Each impl is concrete-typed so
// no overlap arises. Mirrors `IRBuilder::CreateRet` in `IRBuilder.h`.

/// Sealed: types that can be passed to [`IRBuilder::build_ret`] for a
/// function carrying [`ReturnMarker`] `R`. Concrete impls are provided
/// per `(value-shape, R)` pair so a typed builder accepts every Rust
/// scalar / typed handle that lifts to the correct IR type, while a
/// runtime-checked [`Dyn`] builder accepts anything that implements
/// [`crate::value::IsValue`].
pub trait IntoReturnValue<'ctx, R: ReturnMarker>: Sized {
    #[doc(hidden)]
    fn into_return_value(self, module: &'ctx Module<'ctx>) -> IrResult<Value<'ctx>>;
}

// Int-marker impls: every `IntoIntValue<'ctx, W>` is also a
// `IntoReturnValue<'ctx, W>`. Expanded per concrete `W` so coherence
// stays sane (a single blanket would conflict with the float side).
macro_rules! impl_into_return_value_int {
    ($($w:ty),+ $(,)?) => { $(
        impl<'ctx, V> IntoReturnValue<'ctx, $w> for V
        where
            V: crate::int_width::IntoIntValue<'ctx, $w>,
        {
            #[inline]
            fn into_return_value(
                self,
                module: &'ctx Module<'ctx>,
            ) -> IrResult<Value<'ctx>> {
                Ok(crate::value::IsValue::as_value(self.into_int_value(module)?))
            }
        }
    )+ };
}
impl_into_return_value_int!(bool, i8, i16, i32, i64, i128, crate::int_width::IntDyn);

// Float-marker impls. Phase 2 introduces `IntoFloatValue<'ctx, K>`; for
// now the typed `FloatValue<'ctx, K>` itself is the only direct
// `IntoReturnValue<'ctx, K>` source. Phase 2 will replace these with
// macro-expanded blanket-on-IntoFloatValue impls (matching the int
// side).
macro_rules! impl_into_return_value_float {
    ($($k:ty),+ $(,)?) => { $(
        impl<'ctx> IntoReturnValue<'ctx, $k> for crate::value::FloatValue<'ctx, $k> {
            #[inline]
            fn into_return_value(
                self,
                _module: &'ctx Module<'ctx>,
            ) -> IrResult<Value<'ctx>> {
                Ok(crate::value::IsValue::as_value(self))
            }
        }
    )+ };
}
impl_into_return_value_float!(
    f32,
    f64,
    crate::float_kind::Half,
    crate::float_kind::BFloat,
    crate::float_kind::Fp128,
    crate::float_kind::X86Fp80,
    crate::float_kind::PpcFp128,
    crate::float_kind::FloatDyn,
);

// Pointer-marker impl: `Ptr` accepts a `PointerValue`. Phase 4 will
// extend with `IntoPointerValue<'ctx>` lifts (constants / arguments).
impl<'ctx> IntoReturnValue<'ctx, Ptr> for crate::value::PointerValue<'ctx> {
    #[inline]
    fn into_return_value(self, _module: &'ctx Module<'ctx>) -> IrResult<Value<'ctx>> {
        Ok(crate::value::IsValue::as_value(self))
    }
}

// Top-level erased `Dyn` accepts anything implementing `IsValue`. Run
// the runtime check from inside `build_ret` itself for this marker
// (the type system pins R = Dyn so the typed paths above don't fire).
impl<'ctx, V> IntoReturnValue<'ctx, Dyn> for V
where
    V: crate::value::IsValue<'ctx>,
{
    #[inline]
    fn into_return_value(self, _module: &'ctx Module<'ctx>) -> IrResult<Value<'ctx>> {
        Ok(self.as_value())
    }
}

impl<'ctx, F, R> IRBuilder<'ctx, F, Positioned, R>
where
    F: IRBuilderFolder<'ctx>,
    R: ReturnMarker,
{
    /// Produce `ret <value>` against the function's declared return
    /// type. The accepted operand types are pinned by `R` through the
    /// [`IntoReturnValue`] trait - a builder for `i32`-returning
    /// function takes any `IntoIntValue<'ctx, i32>`, the float / ptr
    /// builders take their corresponding handles, and a [`Dyn`]
    /// builder accepts anything implementing
    /// [`crate::value::IsValue`] but runs an extra runtime
    /// type-equality check.
    ///
    /// Cross-module mixing errors with [`IrError::ForeignValue`].
    pub fn build_ret<V>(&self, value: V) -> IrResult<Instruction<'ctx>>
    where
        V: IntoReturnValue<'ctx, R>,
    {
        let v = value.into_return_value(self.module)?;
        self.require_same_module(v)?;
        // Runtime-check for the fully-erased `Dyn` marker.
        if R::expected_kind() == crate::marker::ExpectedRetKind::Dyn {
            let parent_fn = self.parent_function_dyn();
            let expected = parent_fn.return_type();
            if v.ty().id() != expected.id() {
                return Err(IrError::ReturnTypeMismatch {
                    expected: expected.kind_label(),
                    got: v.ty().kind_label(),
                });
            }
        }
        Ok(self.append_ret(Some(v)))
    }

    /// Owning function of the current insertion block, in its
    /// runtime-checked form. Used by the `Dyn`-marker fall-back inside
    /// [`Self::build_ret`].
    fn parent_function_dyn(&self) -> FunctionValue<'ctx, Dyn> {
        let bb = self.insert_block();
        let parent_id = bb.parent_id().unwrap_or_else(|| {
            unreachable!("Positioned builder block always has a parent function")
        });
        FunctionValue::<Dyn>::from_parts_unchecked(parent_id, self.module)
    }
}

impl<'ctx, F> IRBuilder<'ctx, F, Positioned, ()>
where
    F: IRBuilderFolder<'ctx>,
{
    /// Produce `ret void`. Mirrors `IRBuilder::CreateRetVoid`. The
    /// `()` builder does not expose `build_ret(value)` at all (no
    /// `IntoReturnValue<'ctx, ()>` impls exist), so `build_ret_void`
    /// is the only return option.
    pub fn build_ret_void(&self) -> Instruction<'ctx> {
        self.append_ret(None)
    }
}

impl<'ctx, F> IRBuilder<'ctx, F, Positioned, Dyn>
where
    F: IRBuilderFolder<'ctx>,
{
    /// Produce `ret void`. Errors with
    /// [`IrError::ReturnTypeMismatch`] if the parent function does
    /// not actually return `void`.
    pub fn build_ret_void(&self) -> IrResult<Instruction<'ctx>> {
        let bb = self.insert_block();
        let parent_id = bb.parent_id().unwrap_or_else(|| {
            unreachable!("Positioned builder block always has a parent function")
        });
        let parent_fn = FunctionValue::<Dyn>::from_parts_unchecked(parent_id, self.module);
        let expected = parent_fn.return_type();
        if !expected.is_void() {
            return Err(IrError::ReturnTypeMismatch {
                expected: expected.kind_label(),
                got: TypeKindLabel::Void,
            });
        }
        Ok(self.append_ret(None))
    }
}

// --------------------------------------------------------------------------
// CallBuilder
// --------------------------------------------------------------------------

/// Builder for [`crate::IRBuilder::call_builder`]. Accumulates
/// per-arg / flag state via chainable methods, then emits the call
/// instruction on `.build()`. Each `.arg(...)` call is statically
/// dispatched against `V: IsValue<'ctx>`; arg types can vary
/// across calls without trait objects.
pub struct CallBuilder<'a, 'ctx, F, R>
where
    F: IRBuilderFolder<'ctx>,
    R: ReturnMarker,
{
    parent: &'a IRBuilder<'ctx, F, Positioned, R>,
    callee_id: crate::value::ValueId,
    fn_ty: TypeId,
    return_ty: TypeId,
    args: Vec<crate::value::ValueId>,
    calling_conv: crate::CallingConv,
    tail_kind: crate::instr_types::TailCallKind,
    name: String,
    _r: PhantomData<R>,
}

impl<'a, 'ctx, F, R> CallBuilder<'a, 'ctx, F, R>
where
    F: IRBuilderFolder<'ctx>,
    R: ReturnMarker,
{
    /// Add an argument. Statically dispatched per `V: IsValue` so
    /// mixed-type argument lists work without homogeneity.
    pub fn arg<V: crate::value::IsValue<'ctx>>(mut self, value: V) -> Self {
        let v = value.as_value();
        // Same-module check is deferred to `.build()`; this lets
        // the caller chain without an early `IrResult` plumbing.
        self.args.push(v.id);
        self
    }

    pub fn tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::Tail;
        self
    }

    pub fn must_tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::MustTail;
        self
    }

    pub fn no_tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::NoTail;
        self
    }

    pub fn calling_conv(mut self, cc: crate::CallingConv) -> Self {
        self.calling_conv = cc;
        self
    }

    pub fn name(mut self, name: impl AsRef<str>) -> Self {
        self.name = name.as_ref().to_owned();
        self
    }

    /// Emit the call instruction.
    pub fn build(self) -> IrResult<crate::instructions::CallInst<'ctx>> {
        // Cross-module check on every operand id.
        let module = self.parent.module;
        for &id in &self.args {
            let data = module.context().value_data(id);
            let v = crate::value::Value::from_parts(id, module, data.ty);
            if v.module().id() != module.id() {
                return Err(IrError::ForeignValue);
            }
        }
        let payload = crate::instr_types::CallInstData::new(
            self.callee_id,
            self.fn_ty,
            self.args.into_boxed_slice(),
            self.calling_conv,
            self.tail_kind,
        );
        let inst = self.parent.append_instruction(
            self.return_ty,
            InstructionKindData::Call(payload),
            self.name,
        );
        Ok({
            let _i = inst;
            crate::instructions::CallInst::from_raw(_i.as_value().id, _i.module(), _i.ty().id())
        })
    }
}

// `require_same_int_width` is no longer needed: the IRBuilder's binary-

// --------------------------------------------------------------------------
// SelectArm + build_select
// --------------------------------------------------------------------------

/// Sealed: types that can appear as the true/false arms of a
/// `select`. The associated `Output` pins the result handle's
/// shape so `b.build_select(cond, a, b)` returns the same handle
/// type the user passed in. Mirrors LangRef's invariant that the
/// two arms must have identical IR types.
pub trait SelectArm<'ctx>: Sized + select_arm_sealed::Sealed {
    type Output;
    #[doc(hidden)]
    fn from_select_value(v: crate::value::Value<'ctx>) -> Self::Output;
    #[doc(hidden)]
    fn arm_value(self) -> crate::value::Value<'ctx>;
}

mod select_arm_sealed {
    pub trait Sealed {}
    impl<'ctx, W: crate::int_width::IntWidth> Sealed for crate::value::IntValue<'ctx, W> {}
    impl<'ctx, K: crate::float_kind::FloatKind> Sealed for crate::value::FloatValue<'ctx, K> {}
    impl<'ctx> Sealed for crate::value::PointerValue<'ctx> {}
}

impl<'ctx, W: crate::int_width::IntWidth> SelectArm<'ctx> for crate::value::IntValue<'ctx, W> {
    type Output = crate::value::IntValue<'ctx, W>;
    #[inline]
    fn from_select_value(v: crate::value::Value<'ctx>) -> Self::Output {
        crate::value::IntValue::<W>::from_value_unchecked(v)
    }
    #[inline]
    fn arm_value(self) -> crate::value::Value<'ctx> {
        crate::value::IsValue::as_value(self)
    }
}

impl<'ctx, K: crate::float_kind::FloatKind> SelectArm<'ctx> for crate::value::FloatValue<'ctx, K> {
    type Output = crate::value::FloatValue<'ctx, K>;
    #[inline]
    fn from_select_value(v: crate::value::Value<'ctx>) -> Self::Output {
        crate::value::FloatValue::<K>::from_value_unchecked(v)
    }
    #[inline]
    fn arm_value(self) -> crate::value::Value<'ctx> {
        crate::value::IsValue::as_value(self)
    }
}

impl<'ctx> SelectArm<'ctx> for crate::value::PointerValue<'ctx> {
    type Output = crate::value::PointerValue<'ctx>;
    #[inline]
    fn from_select_value(v: crate::value::Value<'ctx>) -> Self::Output {
        crate::value::PointerValue::from_value_unchecked(v)
    }
    #[inline]
    fn arm_value(self) -> crate::value::Value<'ctx> {
        crate::value::IsValue::as_value(self)
    }
}

impl<'ctx, F, R> IRBuilder<'ctx, F, Positioned, R>
where
    F: IRBuilderFolder<'ctx>,
    R: ReturnMarker,
{
    /// Produce `select i1 <cond>, <ty> <true>, <ty> <false>`.
    /// Mirrors `IRBuilder::CreateSelect`.
    ///
    /// Both arms must share the same Rust type `A`, which pins the
    /// IR-type invariant that LangRef requires. The returned handle
    /// is `A::Output`, statically tied to the arm category.
    pub fn build_select<C, A>(
        &self,
        cond: C,
        true_arm: A,
        false_arm: A,
        name: impl AsRef<str>,
    ) -> IrResult<A::Output>
    where
        C: crate::int_width::IntoIntValue<'ctx, bool>,
        A: SelectArm<'ctx> + Copy,
    {
        let c = cond.into_int_value(self.module)?;
        let true_v = true_arm.arm_value();
        let true_ty = true_arm.arm_value().ty().id();
        let false_v = false_arm.arm_value();
        let false_ty = false_arm.arm_value().ty().id();
        self.require_same_module(c.as_value())?;
        self.require_same_module(true_v)?;
        self.require_same_module(false_v)?;
        if true_ty != false_ty {
            return Err(IrError::TypeMismatch {
                expected: true_v.ty().kind_label(),
                got: false_v.ty().kind_label(),
            });
        }
        let payload =
            crate::instr_types::SelectInstData::new(c.as_value().id, true_v.id, false_v.id);
        let inst = self.append_instruction(true_ty, InstructionKindData::Select(payload), name);
        Ok(A::from_select_value(inst.as_value()))
    }
}
