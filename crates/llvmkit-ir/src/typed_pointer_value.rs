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

use crate::error::IrResult;
use crate::module::{Brand, ModuleBrand, ModuleRef};
use crate::struct_schema::IrField;
use crate::value::{IntoPointerValue, PointerValue, Value};

/// Opaque `ptr` value plus a phantom pointee schema `T`.
pub struct TypedPointerValue<'ctx, T: IrField, B: ModuleBrand = Brand<'ctx>> {
    ptr: PointerValue<'ctx, B>,
    _pointee: PhantomData<fn() -> T>,
}

impl<'ctx, T: IrField, B: ModuleBrand> Clone for TypedPointerValue<'ctx, T, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, T: IrField, B: ModuleBrand> Copy for TypedPointerValue<'ctx, T, B> {}

impl<'ctx, T: IrField, B: ModuleBrand> PartialEq for TypedPointerValue<'ctx, T, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl<'ctx, T: IrField, B: ModuleBrand> Eq for TypedPointerValue<'ctx, T, B> {}

impl<'ctx, T: IrField, B: ModuleBrand> Hash for TypedPointerValue<'ctx, T, B> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ptr.hash(state);
    }
}

impl<'ctx, T: IrField, B: ModuleBrand> fmt::Debug for TypedPointerValue<'ctx, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedPointerValue")
            .field("ptr", &self.ptr)
            .finish()
    }
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx> TypedPointerValue<'ctx, T, B> {
    #[inline]
    pub(crate) fn from_pointer(ptr: PointerValue<'ctx, B>) -> Self {
        Self {
            ptr,
            _pointee: PhantomData,
        }
    }

    /// Erase the pointee schema (D3 opt-out).
    #[inline]
    pub fn as_pointer_value(self) -> PointerValue<'ctx, B> {
        self.ptr
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        self.ptr.as_value()
    }
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx> crate::value::into_pointer_value_sealed::Sealed
    for TypedPointerValue<'ctx, T, B>
{
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx> IntoPointerValue<'ctx, B>
    for TypedPointerValue<'ctx, T, B>
{
    #[inline]
    fn into_pointer_value(self, module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        self.ptr.into_pointer_value(module)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> PointerValue<'ctx, B> {
    /// Attach a pointee schema. This is an *assertion*, not a checked
    /// conversion -- opaque pointers carry nothing to check against. A
    /// mis-assertion is exactly as unchecked as passing the wrong type
    /// to `build_load(ty, ptr, ..)` today: the emitted IR reads a
    /// different type than the slot was written with, which is *legal*
    /// IR under opaque pointers -- neither llvmkit's verifier nor
    /// upstream's can see through a `ptr`, so nothing downstream flags
    /// it. What the assertion can never do is make the Rust side
    /// memory-unsafe (D10): the cost of a wrong pointee is wrong IR
    /// semantics, observable in the printed module, not UB in your
    /// compiler.
    #[inline]
    pub fn with_pointee<T: IrField>(self) -> TypedPointerValue<'ctx, T, B> {
        TypedPointerValue::from_pointer(self)
    }
}
