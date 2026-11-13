//! Typed pointers: an analysis-only type. Mirrors
//! `llvm/include/llvm/IR/TypedPointerType.h`.
//!
//! LLVM keeps typed pointers for the DXIL and SPIR-V backends, to track pointee
//! types in their analyses. Its header says "It is not legal to use this type,
//! or derived types containing this type, in LLVM IR", and
//! `Value::Value`'s `checkType` asserts "Cannot have values with typed pointer
//! types". llvmkit makes both rules compile-time facts: [`TypedPointerType`]
//! does not implement [`IrType`] and has no route to
//! [`Type`], so no value can have it and no IR type can contain
//! it. It is interned in its own table, so a `Type` cannot hold one either.
//!
//! Unrelated to [`crate::TypedPointerValue`], the Rust-side pointee overlay on
//! an opaque pointer *value*.

use core::fmt;

use crate::Branded;
use crate::addr_space::{AddrSpace, AddressSpace};
use crate::array_len::ArrayLen;
use crate::capability::{Capability, CapabilityOf, Mutable};
use crate::derived_types::{
    ArrayType, FloatType, FunctionType, IntType, PointerType, StructType, TargetExtType, VectorType,
};
use crate::element::VecElem;
use crate::float_kind::FloatKind;
use crate::int_width::IntWidth;
use crate::module::{ModuleBrand, ModuleRef};
use crate::struct_body_state::StructBodyState;
use crate::r#type::{IrType, Type, TypeSlotAccess, TypedPointeeSlot, TypedPointerSlot};
use crate::vec_len::VecLen;

/// A typed pointer, `typedptr(<pointee>, <addrspace>)`. Analysis-only: no IR
/// type contains one and no value has one (see the module docs).
#[derive(Branded)]
pub struct TypedPointerType<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    id: TypedPointerSlot,
    module: ModuleRef<'ctx, B, C>,
}

/// What a typed pointer points at: an IR type, or another typed pointer
/// (`TypedPointerType::isValidElementType` admits one).
#[derive(Branded)]
pub enum TypedPointee<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    /// An IR type.
    Ir(Type<'ctx, B, C>),
    /// A typed pointer.
    Typed(TypedPointerType<'ctx, B, C>),
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for TypedPointerType<'_, B, C> {
    type Capability = C;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TypedPointerType<'ctx, B, C> {
    #[inline]
    pub(crate) fn new<M>(id: TypedPointerSlot, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B, C>>,
    {
        Self {
            id,
            module: module.into(),
        }
    }

    /// Pointee type. Mirrors `TypedPointerType::getElementType`.
    pub fn pointee(self) -> TypedPointee<'ctx, B, C> {
        let (pointee, _) = self.module.context().typed_pointer_data(self.id);
        match pointee {
            TypedPointeeSlot::Ir(slot) => TypedPointee::Ir(Type::new(slot, self.module)),
            TypedPointeeSlot::Typed(slot) => {
                TypedPointee::Typed(TypedPointerType::new(slot, self.module))
            }
        }
    }

    /// Address space. Mirrors `TypedPointerType::getAddressSpace`.
    pub fn address_space(self) -> AddressSpace {
        let (_, addr_space) = self.module.context().typed_pointer_data(self.id);
        addr_space
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TypedPointerType<'ctx, B> {
    /// A typed pointer to `pointee` in `addr_space`, built in the pointee's own
    /// module. Cannot fail: [`TypedPointerElement`] admits only valid pointees.
    /// For a runtime [`Type`], use
    /// [`Module::typed_pointer_type`](crate::Module::typed_pointer_type), which
    /// checks it.
    ///
    /// ```
    /// # use llvmkit_ir::{AddressSpace, Module, TypedPointerType, address_space};
    /// let m = Module::dynamic("doc");
    /// let inner = TypedPointerType::of(m.f32_type(), AddressSpace::DEFAULT);
    /// assert_eq!(
    ///     TypedPointerType::of(inner, address_space::<1>()).to_string(),
    ///     "typedptr(typedptr(float, 0), 1)"
    /// );
    /// ```
    pub fn of<P: TypedPointerElement<'ctx, B>>(
        pointee: P,
        addr_space: AddressSpace,
    ) -> TypedPointerType<'ctx, B, P::Capability> {
        let (slot, module) = match pointee.as_pointee() {
            // Internal: interned in the pointee's own module, so the slot is
            // that module's.
            TypedPointee::Ir(ty) => (
                TypedPointeeSlot::Ir(ty.slot_trusting_same_module()),
                ty.module,
            ),
            TypedPointee::Typed(inner) => (TypedPointeeSlot::Typed(inner.id), inner.module),
        };
        TypedPointerType::new(
            module.context().typed_pointer_type(slot, addr_space),
            module,
        )
    }
}

/// Mirrors `TypePrinting::print`'s `TypedPointerTyID` arm
/// (`lib/IR/AsmWriter.cpp`): `typedptr(` + the pointee + `, ` + the address
/// space + `)`.
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> fmt::Display for TypedPointerType<'ctx, B, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "typedptr({}, {})", self.pointee(), self.address_space())
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> fmt::Display for TypedPointee<'ctx, B, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypedPointee::Ir(ty) => ty.fmt(f),
            TypedPointee::Typed(pointer) => pointer.fmt(f),
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A pointee that is always valid for a typed pointer: every IR type handle
/// `TypedPointerType::isValidElementType` admits, and a typed pointer itself.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a typed pointer's pointee",
    label = "void, label, metadata, token and x86_amx cannot be pointed at",
    note = "a runtime `Type` goes through `typed_pointer_type(ty, addr_space)?`, which checks it \
            (`TypedPointerType::isValidElementType`)"
)]
pub trait TypedPointerElement<'ctx, B: ModuleBrand + 'ctx>:
    sealed::Sealed + Copy + CapabilityOf
{
    /// This pointee, as the typed pointer stores it.
    fn as_pointee(self) -> TypedPointee<'ctx, B, Self::Capability>;
}

// The IR type handles `TypedPointerType::isValidElementType` always admits —
// none of `void`, `label`, `metadata`, `token` or `x86_amx` has one of these.
macro_rules! ir_pointees {
    ($($handle:ident<$($param:ident: $bound:path),*>),+ $(,)?) => {$(
        impl<'ctx, $($param: $bound,)* B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed
            for $handle<'ctx, $($param,)* B, C>
        {
        }
        impl<'ctx, $($param: $bound,)* B: ModuleBrand + 'ctx, C: Capability>
            TypedPointerElement<'ctx, B> for $handle<'ctx, $($param,)* B, C>
        {
            #[inline]
            fn as_pointee(self) -> TypedPointee<'ctx, B, Self::Capability> {
                TypedPointee::Ir(IrType::as_type(self))
            }
        }
    )+};
}

ir_pointees!(
    IntType<W: IntWidth>,
    FloatType<K: FloatKind>,
    PointerType<A: AddrSpace>,
    ArrayType<E: VecElem, L: ArrayLen>,
    StructType<Body: StructBodyState>,
    VectorType<E: VecElem, L: VecLen>,
    FunctionType<>,
    TargetExtType<>,
);

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed for TypedPointerType<'ctx, B, C> {}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TypedPointerElement<'ctx, B>
    for TypedPointerType<'ctx, B, C>
{
    #[inline]
    fn as_pointee(self) -> TypedPointee<'ctx, B, C> {
        TypedPointee::Typed(self)
    }
}
