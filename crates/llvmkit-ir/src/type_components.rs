//! What may go where in a derived type.
//!
//! LLVM's type constructors `assert` their component rules
//! (`FixedVectorType::get`, `ScalableVectorType::get`, `ArrayType::get`,
//! `FunctionType::FunctionType`, `PointerType::get`), and an `assert` is
//! compiled out of a release build. llvmkit states each rule twice: as a
//! sealed trait that a typed handle implements only when it can never break the
//! rule — so the typed constructors here cannot fail — and as
//! `require_component`, which an erased constructor runs and reports through
//! [`IrError::InvalidTypeComponent`].

use core::num::NonZeroU32;

use crate::capability::Capability;
use crate::derived_types::{FloatType, IntType, PointerType, VectorType};
use crate::element::{ElemDyn, VecElem};
use crate::error::{IrError, IrResult, TypePosition};
use crate::float_kind::{Bfloat, FloatDyn, Fp128, Half, PpcFp128, X86Fp80};
use crate::int_width::{IntDyn, Width};
use crate::module::ModuleBrand;
use crate::r#type::{IrType, Type, TypeSlotAccess};
use crate::vec_len::LenDyn;

mod sealed {
    pub trait Sealed {}
}

/// The lane count `N` as a [`NonZeroU32`], checked at compile time.
///
/// `FixedVectorType::get` and `ScalableVectorType::get` assert `NumElts > 0`;
/// here `lanes::<0>()` does not compile, so no run-time path can reach a
/// zero-lane vector.
///
/// ```
/// # use llvmkit_ir::{Module, VectorType, lanes};
/// let m = Module::dynamic("doc");
/// let v = VectorType::of(m.i32_type(), lanes::<4>());
/// assert_eq!(v.to_string(), "<4 x i32>");
/// ```
pub const fn lanes<const N: u32>() -> NonZeroU32 {
    const {
        match NonZeroU32::new(N) {
            Some(lanes) => lanes,
            None => panic!(
                "a vector needs at least one lane: `FixedVectorType::get` and \
                 `ScalableVectorType::get` assert `NumElts > 0`"
            ),
        }
    }
}

/// A type handle that is always a valid vector element: an integer, a float
/// or a pointer — the kinds `VectorType::isValidElementType` admits without
/// asking anything at run time.
///
/// Sealed. A target type is a vector element only when it declares
/// `CanBeVectorElement`, which is run-time data, so it takes the checked
/// route: [`Module::vector_type`](crate::Module::vector_type).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a vector element",
    label = "LLVM vectors hold integers, floats and pointers",
    note = "a target type that declares `CanBeVectorElement`, or any runtime `Type`, goes \
            through `vector_type(ty, lanes)?`, which checks it (`VectorType::isValidElementType`)"
)]
pub trait VectorElement<'ctx, B: ModuleBrand + 'ctx>: IrType<'ctx, B> + sealed::Sealed {
    /// The element marker the vector carries: the scalar marker when the
    /// handle has one (`i32`, `f64`, …), [`ElemDyn`] otherwise.
    type Elem: VecElem;
}

macro_rules! scalar_elements {
    ($handle:ident: $($marker:ty => $elem:ty),+ $(,)?) => {$(
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed for $handle<'ctx, $marker, B, C> {}
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> VectorElement<'ctx, B>
            for $handle<'ctx, $marker, B, C>
        {
            type Elem = $elem;
        }
    )+};
}

scalar_elements!(IntType: bool => bool, i8 => i8, i16 => i16, i32 => i32, i64 => i64, i128 => i128, IntDyn => ElemDyn);
scalar_elements!(FloatType: f32 => f32, f64 => f64, Half => Half, Bfloat => Bfloat, Fp128 => Fp128, X86Fp80 => X86Fp80, PpcFp128 => PpcFp128, FloatDyn => ElemDyn);

impl<'ctx, const N: u32, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed
    for IntType<'ctx, Width<N>, B, C>
{
}
impl<'ctx, const N: u32, B: ModuleBrand + 'ctx, C: Capability> VectorElement<'ctx, B>
    for IntType<'ctx, Width<N>, B, C>
{
    type Elem = ElemDyn;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed for PointerType<'ctx, B, C> {}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> VectorElement<'ctx, B>
    for PointerType<'ctx, B, C>
{
    type Elem = ElemDyn;
}

impl<'ctx, B: ModuleBrand + 'ctx> VectorType<'ctx, ElemDyn, LenDyn, B> {
    /// A fixed vector `<lanes x element>`, built in the element's own module.
    ///
    /// Cannot fail: [`VectorElement`] is implemented only for integers, floats
    /// and pointers (`VectorType::isValidElementType`), and `lanes` is a
    /// [`NonZeroU32`] (`FixedVectorType::get` asserts `NumElts > 0`). For a
    /// runtime [`Type`], use [`Module::vector_type`](crate::Module::vector_type),
    /// which checks both.
    ///
    /// ```
    /// # use llvmkit_ir::{Module, VectorType, lanes};
    /// let m = Module::dynamic("doc");
    /// assert_eq!(VectorType::of(m.f32_type(), lanes::<8>()).to_string(), "<8 x float>");
    /// ```
    pub fn of<T: VectorElement<'ctx, B>>(
        element: T,
        lanes: NonZeroU32,
    ) -> VectorType<'ctx, T::Elem, LenDyn, B, T::Capability> {
        let element = element.as_type();
        // Internal: interned in the element's own module, so the slot is that
        // module's.
        let slot = element.slot_trusting_same_module();
        let module = element.module;
        VectorType::new(module.context().fixed_vector_type(slot, lanes), module)
    }

    /// A scalable vector `<vscale x min_lanes x element>`, built in the
    /// element's own module. Cannot fail, for the reasons [`Self::of`] gives
    /// (`ScalableVectorType::get` asserts the same two rules).
    pub fn scalable_of<T: VectorElement<'ctx, B>>(
        element: T,
        min_lanes: NonZeroU32,
    ) -> VectorType<'ctx, T::Elem, LenDyn, B, T::Capability> {
        let element = element.as_type();
        // Internal: as in `of`.
        let slot = element.slot_trusting_same_module();
        let module = element.module;
        VectorType::new(
            module.context().scalable_vector_type(slot, min_lanes),
            module,
        )
    }
}

/// Refuses `component` in `position` when LLVM forbids it there.
///
/// The one runtime check every erased type constructor runs, after admitting
/// the component against its own module.
pub(crate) fn require_component<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    position: TypePosition,
    component: Type<'ctx, B, C>,
) -> IrResult<()> {
    let valid = match position {
        TypePosition::VectorElement => component.is_valid_vector_element(),
        TypePosition::ArrayElement => component.is_valid_array_element(),
        TypePosition::StructElement { .. } => component.is_valid_struct_element(),
        TypePosition::PointerElement => component.is_valid_pointer_element(),
        TypePosition::FunctionReturn => component.is_valid_function_return(),
        TypePosition::FunctionParameter { .. } => component.is_valid_function_argument(),
    };
    if valid {
        Ok(())
    } else {
        Err(IrError::InvalidTypeComponent {
            position,
            kind: component.kind_label(),
        })
    }
}
