//! Compile-time schemas for named LLVM struct types.
//!
//! The traits in this module let ordinary Rust marker types describe LLVM
//! identified structs while all concrete values remain branded by the module
//! that produced them.

use crate::argument::Argument;
use crate::constant::Constant;
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::float_kind::{BFloat, Fp128, Half, IntoFloatValue, PpcFp128, X86Fp80};
use crate::function_signature::{FunctionParam, FunctionReturn, token::ValidatedFunctionParams};
use crate::instruction::{Instruction, state::Attached};
use crate::int_width::{IntDyn, IntoIntValue, Width};
use crate::marker::{Dyn, Ptr};
use crate::module::{Brand, Module, ModuleBrand, ModuleRef, Unverified};
use crate::r#type::{Type, TypeData};
use crate::value::{FloatValue, IntValue, IntoPointerValue, PointerValue, StructValue, Value};

#[doc(hidden)]
pub mod token {
    use core::marker::PhantomData;

    /// Capability proving that a raw struct-typed value has already been
    /// validated against a [`StructSchema`](crate::StructSchema).
    #[derive(Debug)]
    pub struct ValidatedStructValue<'a> {
        _private: PhantomData<&'a ()>,
    }

    impl<'a> ValidatedStructValue<'a> {
        #[inline]
        pub(crate) fn new() -> Self {
            Self {
                _private: PhantomData,
            }
        }
    }
}

pub use token::ValidatedStructValue;

/// Lifetime-free schema token for one LLVM struct field.
pub trait IrField: Sized + 'static {
    /// Branded IR value produced by typed field extraction.
    type Value<'ctx, B: ModuleBrand + 'ctx>;

    /// Construct this field's LLVM IR type in `module`.
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx;

    /// Check whether an existing raw LLVM type matches this field schema.
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx;

    /// Diagnostic kind label expected by this schema.
    fn expected_kind_label() -> TypeKindLabel;

    /// Convert a raw field value after [`matches_ir_type`](Self::matches_ir_type)
    /// has accepted its type.
    fn value_from_ir_value<'ctx, B>(value: Value<'ctx, B>) -> IrResult<Self::Value<'ctx, B>>
    where
        B: ModuleBrand + 'ctx;
}

/// Inputs that can be lifted into a field value of schema `F`.
pub trait IntoIrField<'ctx, F: IrField, B: ModuleBrand = Brand<'ctx>>: Sized {
    fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

/// Branded wrapper value generated for a [`StructSchema`].
pub trait StructSchemaValue<'ctx, S: StructSchema, B: ModuleBrand = Brand<'ctx>>:
    Sized + Copy
{
    fn as_struct_value(self) -> StructValue<'ctx, B>;

    fn from_struct_value(raw: StructValue<'ctx, B>, validated: &ValidatedStructValue<'_>) -> Self;
}

/// Lifetime-free schema token for an LLVM identified struct.
pub trait StructSchema: Sized + 'static {
    type Value<'ctx, B: ModuleBrand + 'ctx>: StructSchemaValue<'ctx, Self, B>;

    /// LLVM identified-struct name, without the leading `%`.
    const NAME: &'static str;

    /// Whether the struct body is packed.
    const PACKED: bool = false;

    /// Construct this schema's field types in source-layout order.
    fn field_types<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Vec<Type<'ctx, B>>>
    where
        B: ModuleBrand + 'ctx;

    /// Check a raw identified-struct body against this schema.
    fn matches_fields<'ctx, B>(fields: &[Type<'ctx, B>]) -> bool
    where
        B: ModuleBrand + 'ctx;

    /// Return the idempotent named LLVM struct type for this schema.
    #[inline]
    fn ir_type<'ctx, B>(
        module: &Module<'ctx, B, Unverified>,
    ) -> IrResult<crate::StructType<'ctx, crate::BodySet, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        module.get_or_set_named_struct_body::<Self>()
    }
}

impl<S> IrField for S
where
    S: StructSchema,
{
    type Value<'ctx, B: ModuleBrand + 'ctx> = S::Value<'ctx, B>;

    #[inline]
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(S::ir_type(module)?.as_type())
    }

    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        let TypeData::Struct(data) = ty.data() else {
            return false;
        };
        if data.name.as_deref() != Some(S::NAME) {
            return false;
        }
        let body = data.body.borrow();
        let Some(body) = body.as_ref() else {
            return false;
        };
        if body.packed != S::PACKED {
            return false;
        }
        let fields: Vec<Type<'ctx, B>> = body
            .elements
            .iter()
            .map(|id| Type::new(*id, ty.module))
            .collect();
        S::matches_fields(&fields)
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Struct
    }

    fn value_from_ir_value<'ctx, B>(value: Value<'ctx, B>) -> IrResult<Self::Value<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        let raw = StructValue::try_from(value)?;
        if !<S as IrField>::matches_ir_type(raw.ty().as_type()) {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: raw.ty().as_type().kind_label(),
            });
        }
        let validated = ValidatedStructValue::new();
        Ok(S::Value::from_struct_value(raw, &validated))
    }
}

macro_rules! impl_int_field {
    ($($w:ty => $method:ident, $bits:literal),+ $(,)?) => {$(
        impl IrField for $w {
            type Value<'ctx, B: ModuleBrand + 'ctx> = IntValue<'ctx, $w, B>;

            #[inline]
            fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
            where
                B: ModuleBrand + 'ctx,
            {
                Ok(module.$method().as_type())
            }

            #[inline]
            fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
            where
                B: ModuleBrand + 'ctx,
            {
                matches!(ty.kind(), crate::TypeKind::Integer { bits } if bits == $bits)
            }

            #[inline]
            fn expected_kind_label() -> TypeKindLabel {
                TypeKindLabel::Integer
            }

            #[inline]
            fn value_from_ir_value<'ctx, B>(value: Value<'ctx, B>) -> IrResult<Self::Value<'ctx, B>>
            where
                B: ModuleBrand + 'ctx,
            {
                IntValue::<'ctx, $w, B>::try_from(value)
            }
        }

        impl<'ctx, B, V> IntoIrField<'ctx, $w, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoIntValue<'ctx, $w, B>,
        {
            #[inline]
            fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_int_value(module)?.as_value())
            }
        }
    )+};
}

impl_int_field!(
    bool => bool_type, 1,
    i8 => i8_type, 8,
    i16 => i16_type, 16,
    i32 => i32_type, 32,
    i64 => i64_type, 64,
    i128 => i128_type, 128,
);

impl IrField for IntDyn {
    type Value<'ctx, B: ModuleBrand + 'ctx> = IntValue<'ctx, IntDyn, B>;

    #[inline]
    fn ir_type<'ctx, B>(_module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        Err(IrError::InvalidOperation {
            message: "IntDyn field schemas require an explicit static width",
        })
    }

    #[inline]
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        ty.is_integer()
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Integer
    }

    #[inline]
    fn value_from_ir_value<'ctx, B>(value: Value<'ctx, B>) -> IrResult<Self::Value<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        IntValue::<'ctx, IntDyn, B>::try_from(value)
    }
}

impl<'ctx, B, V> IntoIrField<'ctx, IntDyn, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoIntValue<'ctx, IntDyn, B>,
{
    #[inline]
    fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_int_value(module)?.as_value())
    }
}

impl<const N: u32> IrField for Width<N> {
    type Value<'ctx, B: ModuleBrand + 'ctx> = IntValue<'ctx, Width<N>, B>;

    #[inline]
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(module.int_type_n::<N>().as_type())
    }

    #[inline]
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        matches!(ty.kind(), crate::TypeKind::Integer { bits } if bits == N)
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Integer
    }

    #[inline]
    fn value_from_ir_value<'ctx, B>(value: Value<'ctx, B>) -> IrResult<Self::Value<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        IntValue::<'ctx, Width<N>, B>::try_from(value)
    }
}

impl<'ctx, B, V, const N: u32> IntoIrField<'ctx, Width<N>, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoIntValue<'ctx, Width<N>, B>,
{
    #[inline]
    fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_int_value(module)?.as_value())
    }
}

macro_rules! impl_float_field {
    ($($k:ty => $method:ident, $kind:pat),+ $(,)?) => {$(
        impl IrField for $k {
            type Value<'ctx, B: ModuleBrand + 'ctx> = FloatValue<'ctx, $k, B>;

            #[inline]
            fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
            where
                B: ModuleBrand + 'ctx,
            {
                Ok(module.$method().as_type())
            }

            #[inline]
            fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
            where
                B: ModuleBrand + 'ctx,
            {
                matches!(ty.kind(), $kind)
            }

            #[inline]
            fn expected_kind_label() -> TypeKindLabel {
                TypeKindLabel::Float
            }

            #[inline]
            fn value_from_ir_value<'ctx, B>(value: Value<'ctx, B>) -> IrResult<Self::Value<'ctx, B>>
            where
                B: ModuleBrand + 'ctx,
            {
                FloatValue::<'ctx, $k, B>::try_from(value)
            }
        }

        impl<'ctx, B, V> IntoIrField<'ctx, $k, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoFloatValue<'ctx, $k, B>,
        {
            #[inline]
            fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_float_value(module)?.as_value())
            }
        }
    )+};
}

impl_float_field!(
    f32 => f32_type, crate::TypeKind::Float,
    f64 => f64_type, crate::TypeKind::Double,
    Half => half_type, crate::TypeKind::Half,
    BFloat => bfloat_type, crate::TypeKind::BFloat,
    Fp128 => fp128_type, crate::TypeKind::Fp128,
    X86Fp80 => x86_fp80_type, crate::TypeKind::X86Fp80,
    PpcFp128 => ppc_fp128_type, crate::TypeKind::PpcFp128,
);

impl IrField for Ptr {
    type Value<'ctx, B: ModuleBrand + 'ctx> = PointerValue<'ctx, B>;

    #[inline]
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(module.ptr_type(0).as_type())
    }

    #[inline]
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        ty.is_pointer()
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Pointer
    }

    #[inline]
    fn value_from_ir_value<'ctx, B>(value: Value<'ctx, B>) -> IrResult<Self::Value<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        PointerValue::try_from(value)
    }
}

impl<'ctx, B, V> IntoIrField<'ctx, Ptr, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoPointerValue<'ctx, B>,
{
    #[inline]
    fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_pointer_value(module)?.as_value())
    }
}

macro_rules! impl_struct_into_field {
    ($source:ty) => {
        impl<'ctx, S, B> IntoIrField<'ctx, S, B> for $source
        where
            S: StructSchema,
            B: ModuleBrand + 'ctx,
        {
            fn into_ir_field(self, _module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                let raw = StructValue::try_from(self)?;
                if !<S as IrField>::matches_ir_type(raw.ty().as_type()) {
                    return Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::Struct,
                        got: raw.ty().as_type().kind_label(),
                    });
                }
                Ok(raw.as_value())
            }
        }
    };
}

impl_struct_into_field!(Value<'ctx, B>);
impl_struct_into_field!(Argument<'ctx, B>);
impl_struct_into_field!(Constant<'ctx, B>);
impl_struct_into_field!(Instruction<'ctx, Attached, B>);

impl<S> FunctionReturn for S
where
    S: StructSchema,
{
    type Marker = Dyn;

    #[inline]
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        <S as IrField>::ir_type(module)
    }

    #[inline]
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        <S as IrField>::matches_ir_type(ty)
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Struct
    }
}

impl<S> FunctionParam for S
where
    S: StructSchema,
{
    type Value<'ctx, B: ModuleBrand + 'ctx> = S::Value<'ctx, B>;

    #[inline]
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        <S as IrField>::ir_type(module)
    }

    #[inline]
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        <S as IrField>::matches_ir_type(ty)
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Struct
    }

    fn validate_argument<'ctx, B>(arg: Argument<'ctx, B>) -> IrResult<()>
    where
        B: ModuleBrand + 'ctx,
    {
        if <S as IrField>::matches_ir_type(arg.ty()) {
            Ok(())
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: arg.ty().kind_label(),
            })
        }
    }

    fn value_from_argument<'ctx, B>(
        arg: Argument<'ctx, B>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        let validated = ValidatedStructValue::new();
        S::Value::from_struct_value(
            StructValue::from_value_unchecked(arg.as_value()),
            &validated,
        )
    }
}
