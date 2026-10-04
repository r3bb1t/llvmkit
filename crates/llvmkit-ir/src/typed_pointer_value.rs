//! Rust-side static pointee overlay on opaque pointers.
//!
//! [`TypedPointerValue`] wraps a plain [`PointerValue`] and remembers a
//! pointee schema `T: IrField` at the type level. It is compile-time
//! bookkeeping only: the wrapped value's IR type is a plain opaque
//! pointer and printed IR is byte-identical to the erased path.
//! Unrelated to [`crate::TypedPointerType`], which is the IR-level
//! (GPU-only) typed-pointer *type* and prints differently.

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use super::value::into_pointer_value_sealed::Sealed;
use crate::capability::{Capability, CapabilityOf, Mutable};
use crate::error::IrResult;
use crate::module::{ModuleBrand, ModuleRef};
use crate::struct_schema::IrField;
use crate::value::{IntoPointerValue, PointerValue, Value};

/// Opaque `ptr` value plus a phantom pointee schema `T`.
pub struct TypedPointerValue<'ctx, T: IrField, B: ModuleBrand, C: Capability = Mutable> {
    ptr: PointerValue<'ctx, B, C>,
    _pointee: PhantomData<fn() -> T>,
}

impl<'ctx, T: IrField, B: ModuleBrand, C: Capability> Clone for TypedPointerValue<'ctx, T, B, C> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, T: IrField, B: ModuleBrand, C: Capability> Copy for TypedPointerValue<'ctx, T, B, C> {}

impl<T: IrField, B: ModuleBrand, C: Capability> CapabilityOf for TypedPointerValue<'_, T, B, C> {
    type Capability = C;
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx, C: Capability> fmt::Display
    for TypedPointerValue<'ctx, T, B, C>
{
    /// Print the operand form `ptr <ref>`. The pointee schema `T` is
    /// compile-time-only bookkeeping and does not appear in the output, so
    /// this is byte-identical to what the erased
    /// [`PointerValue`] / [`Value`] handles print.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.ptr, f)
    }
}

impl<'ctx, T: IrField, B: ModuleBrand, C: Capability> PartialEq
    for TypedPointerValue<'ctx, T, B, C>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl<'ctx, T: IrField, B: ModuleBrand, C: Capability> Eq for TypedPointerValue<'ctx, T, B, C> {}

impl<'ctx, T: IrField, B: ModuleBrand, C: Capability> Hash for TypedPointerValue<'ctx, T, B, C> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ptr.hash(state);
    }
}

impl<'ctx, T: IrField, B: ModuleBrand, C: Capability> fmt::Debug
    for TypedPointerValue<'ctx, T, B, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedPointerValue")
            .field("ptr", &self.ptr)
            .finish()
    }
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx, C: Capability> TypedPointerValue<'ctx, T, B, C> {
    #[inline]
    pub(crate) fn from_pointer(ptr: PointerValue<'ctx, B, C>) -> Self {
        Self {
            ptr,
            _pointee: PhantomData,
        }
    }

    /// Erase the pointee schema (D3 opt-out).
    #[inline]
    pub fn as_pointer_value(self) -> PointerValue<'ctx, B, C> {
        self.ptr
    }

    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        self.ptr.as_erased()
    }
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx, C: Capability> Sealed
    for TypedPointerValue<'ctx, T, B, C>
{
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx, C: Capability> IntoPointerValue<'ctx, B>
    for TypedPointerValue<'ctx, T, B, C>
{
    #[inline]
    fn into_pointer_value(self, module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        self.ptr.into_pointer_value(module)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> PointerValue<'ctx, B, C> {
    /// Attach a pointee schema. This is an *assertion*, not a checked
    /// conversion -- opaque pointers carry nothing to check against. A
    /// mis-assertion is exactly as unchecked as passing the wrong type
    /// to `load(ty, ptr, ..)` today: the emitted IR reads a
    /// different type than the slot was written with, which is *legal*
    /// IR under opaque pointers -- neither llvmkit's verifier nor
    /// upstream's can see through a `ptr`, so nothing downstream flags
    /// it. What the assertion can never do is make the Rust side
    /// memory-unsafe (D10): the cost of a wrong pointee is wrong IR
    /// semantics, observable in the printed module, not UB in your
    /// compiler.
    #[inline]
    pub fn with_pointee<T: IrField>(self) -> TypedPointerValue<'ctx, T, B, C> {
        TypedPointerValue::from_pointer(self)
    }
}
