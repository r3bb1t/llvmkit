//! Intrinsic call views. Mirrors `llvm/lib/IR/IntrinsicInst.cpp` wrappers.

use super::attributes::MemoryEffects;
use super::error::{IrError, IrResult};
use super::instructions::CallInst;
use super::intrinsics::{IntrinsicDescriptor, IntrinsicId, descriptor_for_callee};
use super::marker::{Dyn, ReturnMarker};
use super::module::{Brand, ModuleBrand};
use super::value::Value;

/// A call whose callee is a generated LLVM intrinsic declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IntrinsicInst<'ctx, R: ReturnMarker = Dyn, B: ModuleBrand = Brand<'ctx>> {
    call: CallInst<'ctx, R, B>,
    id: IntrinsicId,
}

/// Memory intrinsic call wrapper for `llvm.memcpy`, `llvm.memmove`, and `llvm.memset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemIntrinsic<'ctx, B: ModuleBrand = Brand<'ctx>, R: ReturnMarker = Dyn> {
    inner: IntrinsicInst<'ctx, R, B>,
}

/// Lifetime intrinsic call wrapper for `llvm.lifetime.start/end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LifetimeIntrinsic<'ctx, B: ModuleBrand = Brand<'ctx>, R: ReturnMarker = Dyn> {
    inner: IntrinsicInst<'ctx, R, B>,
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> IntrinsicInst<'ctx, R, B> {
    /// Return `Some` when `call` targets a generated intrinsic declaration.
    #[inline]
    pub fn from_call(call: CallInst<'ctx, R, B>) -> Option<Self> {
        let id = descriptor_for_callee(call.callee())?.id();
        Some(Self { call, id })
    }

    /// Convert a call to an intrinsic view, rejecting ordinary calls.
    #[inline]
    pub fn try_from_call(call: CallInst<'ctx, R, B>) -> IrResult<Self> {
        Self::from_call(call).ok_or(IrError::InvalidOperation {
            message: "call is not an intrinsic",
        })
    }

    /// Return the underlying call instruction.
    #[inline]
    pub fn call(self) -> CallInst<'ctx, R, B> {
        self.call
    }

    /// Generated intrinsic ID.
    #[inline]
    pub const fn intrinsic_id(self) -> IntrinsicId {
        self.id
    }

    /// Backward-compatible short ID accessor.
    #[inline]
    pub const fn id(self) -> IntrinsicId {
        self.intrinsic_id()
    }

    /// Generated descriptor matched from the callee declaration.
    #[inline]
    pub fn descriptor(self) -> IrResult<IntrinsicDescriptor<'ctx, B>> {
        let descriptor =
            descriptor_for_callee(self.call.callee()).ok_or(IrError::InvalidOperation {
                message: "call is not an intrinsic",
            })?;
        if descriptor.id() != self.id {
            return Err(IrError::IntrinsicSignatureMismatch {
                name: self.id.base_name().to_owned(),
            });
        }
        Ok(descriptor)
    }

    /// Whether the generated intrinsic record is marked commutative.
    #[inline]
    pub fn is_commutative(self) -> bool {
        self.intrinsic_id().is_commutative()
    }

    /// Whether the generated intrinsic may throw.
    #[inline]
    pub fn may_throw(self) -> bool {
        self.intrinsic_id().may_throw()
    }

    /// Generated memory effects for this intrinsic.
    #[inline]
    pub fn memory_effects(self) -> MemoryEffects {
        self.intrinsic_id().memory_effects()
    }

    /// Return value, or `None` for void-returning intrinsic calls.
    #[inline]
    pub fn return_value(self) -> Option<Value<'ctx, B>> {
        self.call.return_value()
    }
}

impl<'ctx, B, R> MemIntrinsic<'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: ReturnMarker,
{
    /// Narrow an intrinsic view to a memory intrinsic wrapper.
    #[inline]
    pub fn try_from_intrinsic(inner: IntrinsicInst<'ctx, R, B>) -> IrResult<Self> {
        match inner.intrinsic_id() {
            id if id == IntrinsicId::MEMCPY
                || id == IntrinsicId::MEMMOVE
                || id == IntrinsicId::MEMSET
                || id.base_name() == "llvm.memcpy.inline"
                || id.base_name() == "llvm.memset.inline" =>
            {
                Ok(Self { inner })
            }
            _ => Err(IrError::InvalidOperation {
                message: "intrinsic is not a memory intrinsic",
            }),
        }
    }

    /// Return the underlying intrinsic view.
    #[inline]
    pub fn inner(&self) -> &IntrinsicInst<'ctx, R, B> {
        &self.inner
    }

    /// Return the underlying call instruction.
    #[inline]
    pub fn call(&self) -> CallInst<'ctx, R, B> {
        self.inner.call()
    }
}

impl<'ctx, B, R> LifetimeIntrinsic<'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: ReturnMarker,
{
    /// Narrow an intrinsic view to a lifetime intrinsic wrapper.
    #[inline]
    pub fn try_from_intrinsic(inner: IntrinsicInst<'ctx, R, B>) -> IrResult<Self> {
        match inner.intrinsic_id() {
            id if id == IntrinsicId::LIFETIME_START || id == IntrinsicId::LIFETIME_END => {
                Ok(Self { inner })
            }
            _ => Err(IrError::InvalidOperation {
                message: "intrinsic is not a lifetime intrinsic",
            }),
        }
    }

    /// Return the underlying intrinsic view.
    #[inline]
    pub fn inner(&self) -> &IntrinsicInst<'ctx, R, B> {
        &self.inner
    }

    /// Return the underlying call instruction.
    #[inline]
    pub fn call(&self) -> CallInst<'ctx, R, B> {
        self.inner.call()
    }
}
