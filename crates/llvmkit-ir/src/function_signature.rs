//! Type-level function signature schemas and typed function facade.
//!
//! The traits in this module describe logical LLVM IR signatures using
//! lifetime-free Rust schema tokens. Concrete values produced from those
//! schemas remain branded by the originating module through `'ctx` and `B`.
//! Derived struct schemas can therefore map a Rust type such as
//! `WindowPlacement` to a branded `WindowPlacementValue<'ctx, B>` wrapper
//! without erasing module identity.

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use crate::argument::Argument;
use crate::basic_block::BasicBlock;
use crate::block_state::Unsealed;
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::float_kind::{BFloat, Fp128, Half, PpcFp128, X86Fp80};
use crate::function::FunctionValue;
use crate::int_width::Width;
use crate::ir_builder::{IRBuilder, Unpositioned, constant_folder::ConstantFolder};
use crate::marker::{Ptr, ReturnMarker};
use crate::module::{Brand, Module, ModuleBrand, Unverified};
use crate::r#type::{Type, TypeKind};
use crate::value::{FloatValue, IntValue, PointerValue};

#[doc(hidden)]
pub mod token {
    /// Capability proving that a raw function has already been validated against
    /// a typed parameter schema. The field is private and the lifetime is tied
    /// to one `TypedFunctionValue::params` call, so downstream crates can name
    /// this type in trait impls but cannot manufacture or retain the
    /// capability.
    #[derive(Debug)]
    pub struct ValidatedFunctionParams<'a> {
        _private: core::marker::PhantomData<&'a ()>,
    }

    impl<'a> ValidatedFunctionParams<'a> {
        pub(crate) fn new() -> Self {
            Self {
                _private: core::marker::PhantomData,
            }
        }
    }
}

use token::ValidatedFunctionParams;

/// Lifetime-free schema token for a function return type.
pub trait FunctionReturn: Sized + 'static {
    /// Return-marker typestate used by [`FunctionValue`] and [`IRBuilder`].
    type Marker: ReturnMarker;

    /// Construct this schema's LLVM IR return type in `module`.
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx;

    /// Check whether an existing raw return type matches this schema.
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx;

    /// Diagnostic kind label expected by this schema.
    fn expected_kind_label() -> TypeKindLabel;
}

/// Lifetime-free schema token for one function parameter.
pub trait FunctionParam: Sized + 'static {
    /// Branded IR value returned by [`TypedFunctionValue::params`].
    type Value<'ctx, B: ModuleBrand + 'ctx>;

    /// Construct this schema's LLVM IR parameter type in `module`.
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx;

    /// Check whether an existing raw parameter type matches this schema.
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx;

    /// Diagnostic kind label expected by this schema.
    fn expected_kind_label() -> TypeKindLabel;

    /// Validate that a raw argument can be represented by [`Self::Value`].
    fn validate_argument<'ctx, B>(arg: Argument<'ctx, B>) -> IrResult<()>
    where
        B: ModuleBrand + 'ctx;

    /// Convert a previously-validated raw argument into the branded value.
    /// The validation capability is only created by this crate after
    /// [`TypedFunctionValue::try_from_function`] succeeds, so safe downstream
    /// code cannot bypass the facade's type checks.
    fn value_from_argument<'ctx, B>(
        arg: Argument<'ctx, B>,
        validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx;
}

/// Lifetime-free tuple schema for a function's parameter list.
pub trait FunctionParamList: Sized + 'static {
    /// Number of parameters represented by this tuple schema.
    const ARITY: u32;

    /// Branded tuple returned by [`TypedFunctionValue::params`].
    type Values<'ctx, B: ModuleBrand + 'ctx>;

    /// Construct the LLVM IR parameter type list in tuple order.
    fn ir_types<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Vec<Type<'ctx, B>>>
    where
        B: ModuleBrand + 'ctx;

    /// Validate every raw argument in declaration order.
    fn validate<'ctx, R, B>(function: FunctionValue<'ctx, R, B>) -> IrResult<()>
    where
        R: ReturnMarker,
        B: ModuleBrand + 'ctx;

    /// Return typed parameter values in declaration order.
    /// The validation capability is only created by this crate after the raw
    /// function has passed arity and per-parameter checks.
    fn values<'ctx, R, B>(
        function: FunctionValue<'ctx, R, B>,
        validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Values<'ctx, B>
    where
        R: ReturnMarker,
        B: ModuleBrand + 'ctx;
}

/// Rust function-pointer schema facade.
pub trait FunctionSignature: Sized + 'static {
    type Ret: FunctionReturn;
    type Params: FunctionParamList;
}

/// Function handle whose return and parameter schema are both known at compile time.
pub struct TypedFunctionValue<'ctx, Ret, Params, B: ModuleBrand = Brand<'ctx>>
where
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    function: FunctionValue<'ctx, Ret::Marker, B>,
    _ret: PhantomData<Ret>,
    _params: PhantomData<Params>,
}

impl<'ctx, Ret, Params, B> Clone for TypedFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<'ctx, Ret, Params, B> Copy for TypedFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
}

impl<'ctx, Ret, Params, B> PartialEq for TypedFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.function == other.function
    }
}

impl<'ctx, Ret, Params, B> Eq for TypedFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
}

impl<'ctx, Ret, Params, B> Hash for TypedFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.function.hash(state);
    }
}

impl<'ctx, Ret, Params, B> fmt::Debug for TypedFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedFunctionValue")
            .field("function", &self.function)
            .finish()
    }
}

impl<'ctx, Ret, Params, B> TypedFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand + 'ctx,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    /// Wrap an existing raw function after validating the complete schema.
    pub fn try_from_function(function: FunctionValue<'ctx, Ret::Marker, B>) -> IrResult<Self> {
        let arg_count = function.arg_count();
        if arg_count != Params::ARITY {
            return Err(IrError::FunctionParameterCountMismatch {
                expected: Params::ARITY,
                got: arg_count,
            });
        }
        let return_type = function.return_type();
        if !Ret::matches_ir_type(return_type) {
            return Err(IrError::ReturnTypeMismatch {
                expected: Ret::expected_kind_label(),
                got: return_type.kind_label(),
            });
        }
        Params::validate(function)?;
        Ok(Self {
            function,
            _ret: PhantomData,
            _params: PhantomData,
        })
    }

    /// Return the underlying return-typed function handle.
    #[inline]
    pub fn as_function(self) -> FunctionValue<'ctx, Ret::Marker, B> {
        self.function
    }

    /// Return typed parameter values in declaration order.
    #[inline]
    pub fn params(self) -> Params::Values<'ctx, B> {
        let validated = ValidatedFunctionParams::new();
        Params::values(self.function, &validated)
    }

    /// Append a basic block to this function.
    #[inline]
    pub fn append_basic_block<Name>(
        self,
        module: &Module<'ctx, B, Unverified>,
        name: Name,
    ) -> BasicBlock<'ctx, Ret::Marker, Unsealed, B>
    where
        Name: Into<String>,
    {
        self.function.append_basic_block(module, name)
    }

    /// Construct a builder whose return typestate matches this function.
    #[inline]
    pub fn builder<'m>(
        self,
        module: &'m Module<'ctx, B, Unverified>,
    ) -> IRBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, Ret::Marker> {
        IRBuilder::new_for::<Ret::Marker>(module)
    }
}

impl FunctionReturn for () {
    type Marker = ();

    #[inline]
    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(module.void_type().as_type())
    }

    #[inline]
    fn matches_ir_type<'ctx, B>(ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        matches!(ty.kind(), TypeKind::Void)
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Void
    }
}

impl FunctionReturn for Ptr {
    type Marker = Ptr;

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
        matches!(ty.kind(), TypeKind::Pointer { .. })
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Pointer
    }
}

impl FunctionParam for Ptr {
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
        matches!(ty.kind(), TypeKind::Pointer { .. })
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Pointer
    }

    #[inline]
    fn validate_argument<'ctx, B>(arg: Argument<'ctx, B>) -> IrResult<()>
    where
        B: ModuleBrand + 'ctx,
    {
        PointerValue::try_from(arg).map(|_| ())
    }

    #[inline]
    fn value_from_argument<'ctx, B>(
        arg: Argument<'ctx, B>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        PointerValue::from_value_unchecked(arg.as_value())
    }
}

macro_rules! impl_int_signature_marker {
    ($marker:ty, $method:ident, $bits:literal) => {
        impl FunctionReturn for $marker {
            type Marker = $marker;

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
                matches!(ty.kind(), TypeKind::Integer { bits } if bits == $bits)
            }

            #[inline]
            fn expected_kind_label() -> TypeKindLabel {
                TypeKindLabel::Integer
            }
        }

        impl FunctionParam for $marker {
            type Value<'ctx, B: ModuleBrand + 'ctx> = IntValue<'ctx, $marker, B>;

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
                matches!(ty.kind(), TypeKind::Integer { bits } if bits == $bits)
            }

            #[inline]
            fn expected_kind_label() -> TypeKindLabel {
                TypeKindLabel::Integer
            }

            #[inline]
            fn validate_argument<'ctx, B>(arg: Argument<'ctx, B>) -> IrResult<()>
            where
                B: ModuleBrand + 'ctx,
            {
                IntValue::<$marker, B>::try_from(arg).map(|_| ())
            }

            #[inline]
            fn value_from_argument<'ctx, B>(
                arg: Argument<'ctx, B>,
                _validated: &ValidatedFunctionParams<'_>,
            ) -> Self::Value<'ctx, B>
            where
                B: ModuleBrand + 'ctx,
            {
                IntValue::<$marker, B>::from_value_unchecked(arg.as_value())
            }
        }
    };
}

impl_int_signature_marker!(bool, bool_type, 1);
impl_int_signature_marker!(i8, i8_type, 8);
impl_int_signature_marker!(i16, i16_type, 16);
impl_int_signature_marker!(i32, i32_type, 32);
impl_int_signature_marker!(i64, i64_type, 64);
impl_int_signature_marker!(i128, i128_type, 128);

impl<const N: u32> FunctionReturn for Width<N> {
    type Marker = Width<N>;

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
        matches!(ty.kind(), TypeKind::Integer { bits } if bits == N)
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Integer
    }
}

impl<const N: u32> FunctionParam for Width<N> {
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
        matches!(ty.kind(), TypeKind::Integer { bits } if bits == N)
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Integer
    }

    #[inline]
    fn validate_argument<'ctx, B>(arg: Argument<'ctx, B>) -> IrResult<()>
    where
        B: ModuleBrand + 'ctx,
    {
        IntValue::<Width<N>, B>::try_from(arg).map(|_| ())
    }

    #[inline]
    fn value_from_argument<'ctx, B>(
        arg: Argument<'ctx, B>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        IntValue::<Width<N>, B>::from_value_unchecked(arg.as_value())
    }
}

macro_rules! impl_float_signature_marker {
    ($marker:ty, $method:ident, $kind:pat, $label:ident) => {
        impl FunctionReturn for $marker {
            type Marker = $marker;

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
                TypeKindLabel::$label
            }
        }

        impl FunctionParam for $marker {
            type Value<'ctx, B: ModuleBrand + 'ctx> = FloatValue<'ctx, $marker, B>;

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
                TypeKindLabel::$label
            }

            #[inline]
            fn validate_argument<'ctx, B>(arg: Argument<'ctx, B>) -> IrResult<()>
            where
                B: ModuleBrand + 'ctx,
            {
                FloatValue::<$marker, B>::try_from(arg).map(|_| ())
            }

            #[inline]
            fn value_from_argument<'ctx, B>(
                arg: Argument<'ctx, B>,
                _validated: &ValidatedFunctionParams<'_>,
            ) -> Self::Value<'ctx, B>
            where
                B: ModuleBrand + 'ctx,
            {
                FloatValue::<$marker, B>::from_value_unchecked(arg.as_value())
            }
        }
    };
}

impl_float_signature_marker!(f32, f32_type, TypeKind::Float, Float);
impl_float_signature_marker!(f64, f64_type, TypeKind::Double, Double);
impl_float_signature_marker!(Half, half_type, TypeKind::Half, Half);
impl_float_signature_marker!(BFloat, bfloat_type, TypeKind::BFloat, BFloat);
impl_float_signature_marker!(Fp128, fp128_type, TypeKind::Fp128, Fp128);
impl_float_signature_marker!(X86Fp80, x86_fp80_type, TypeKind::X86Fp80, X86Fp80);
impl_float_signature_marker!(PpcFp128, ppc_fp128_type, TypeKind::PpcFp128, PpcFp128);

fn next_function_param<'ctx, B, I>(params: &mut I) -> Argument<'ctx, B>
where
    B: ModuleBrand + 'ctx,
    I: Iterator<Item = Argument<'ctx, B>>,
{
    match params.next() {
        Some(arg) => arg,
        None => unreachable!(
            "TypedFunctionValue invariant: parameter tuple arity matches function signature"
        ),
    }
}

impl FunctionParamList for () {
    const ARITY: u32 = 0;
    type Values<'ctx, B: ModuleBrand + 'ctx> = ();

    #[inline]
    fn ir_types<'ctx, B>(_module: &Module<'ctx, B, Unverified>) -> IrResult<Vec<Type<'ctx, B>>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(Vec::new())
    }

    #[inline]
    fn validate<'ctx, R, B>(_function: FunctionValue<'ctx, R, B>) -> IrResult<()>
    where
        R: ReturnMarker,
        B: ModuleBrand + 'ctx,
    {
        Ok(())
    }

    #[inline]
    fn values<'ctx, R, B>(
        _function: FunctionValue<'ctx, R, B>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Values<'ctx, B>
    where
        R: ReturnMarker,
        B: ModuleBrand + 'ctx,
    {
    }
}

macro_rules! impl_param_list_tuple {
    ($arity:literal; $($param:ident: $slot:literal),+) => {
        impl<$($param),+> FunctionParamList for ($($param,)+)
        where
            $($param: FunctionParam,)+
        {
            const ARITY: u32 = $arity;
            type Values<'ctx, B: ModuleBrand + 'ctx> = ($($param::Value<'ctx, B>,)+);

            #[inline]
            fn ir_types<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Vec<Type<'ctx, B>>>
            where
                B: ModuleBrand + 'ctx,
            {
                Ok(vec![$($param::ir_type(module)?),+])
            }

            #[inline]
            fn validate<'ctx, R, B>(function: FunctionValue<'ctx, R, B>) -> IrResult<()>
            where
                R: ReturnMarker,
                B: ModuleBrand + 'ctx,
            {
                $(
                    $param::validate_argument(function.param($slot)?)?;
                )+
                Ok(())
            }

            #[inline]
            fn values<'ctx, R, B>(
                function: FunctionValue<'ctx, R, B>,
                validated: &ValidatedFunctionParams<'_>,
            ) -> Self::Values<'ctx, B>
            where
                R: ReturnMarker,
                B: ModuleBrand + 'ctx,
            {
                let mut params = function.params();
                ($($param::value_from_argument(next_function_param(&mut params), validated),)+)
            }
        }
    };
}

impl_param_list_tuple!(1; A0: 0);
impl_param_list_tuple!(2; A0: 0, A1: 1);
impl_param_list_tuple!(3; A0: 0, A1: 1, A2: 2);
impl_param_list_tuple!(4; A0: 0, A1: 1, A2: 2, A3: 3);
impl_param_list_tuple!(5; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4);
impl_param_list_tuple!(6; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5);
impl_param_list_tuple!(7; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6);
impl_param_list_tuple!(8; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7);
impl_param_list_tuple!(9; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8);
impl_param_list_tuple!(10; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8, A9: 9);
impl_param_list_tuple!(11; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8, A9: 9, A10: 10);
impl_param_list_tuple!(12; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8, A9: 9, A10: 10, A11: 11);
impl_param_list_tuple!(13; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8, A9: 9, A10: 10, A11: 11, A12: 12);
impl_param_list_tuple!(14; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8, A9: 9, A10: 10, A11: 11, A12: 12, A13: 13);
impl_param_list_tuple!(15; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8, A9: 9, A10: 10, A11: 11, A12: 12, A13: 13, A14: 14);
impl_param_list_tuple!(16; A0: 0, A1: 1, A2: 2, A3: 3, A4: 4, A5: 5, A6: 6, A7: 7, A8: 8, A9: 9, A10: 10, A11: 11, A12: 12, A13: 13, A14: 14, A15: 15);

macro_rules! impl_function_signature {
    ($($param:ident),* $(,)?) => {
        impl<Ret, $($param,)*> FunctionSignature for fn($($param),*) -> Ret
        where
            Ret: FunctionReturn,
            ($($param,)*): FunctionParamList,
        {
            type Ret = Ret;
            type Params = ($($param,)*);
        }

        impl<Ret, $($param,)*> FunctionSignature for unsafe fn($($param),*) -> Ret
        where
            Ret: FunctionReturn,
            ($($param,)*): FunctionParamList,
        {
            type Ret = Ret;
            type Params = ($($param,)*);
        }

        impl<Ret, $($param,)*> FunctionSignature for extern "C" fn($($param),*) -> Ret
        where
            Ret: FunctionReturn,
            ($($param,)*): FunctionParamList,
        {
            type Ret = Ret;
            type Params = ($($param,)*);
        }

        impl<Ret, $($param,)*> FunctionSignature for unsafe extern "C" fn($($param),*) -> Ret
        where
            Ret: FunctionReturn,
            ($($param,)*): FunctionParamList,
        {
            type Ret = Ret;
            type Params = ($($param,)*);
        }

        impl<Ret, $($param,)*> FunctionSignature for extern "system" fn($($param),*) -> Ret
        where
            Ret: FunctionReturn,
            ($($param,)*): FunctionParamList,
        {
            type Ret = Ret;
            type Params = ($($param,)*);
        }

        impl<Ret, $($param,)*> FunctionSignature for unsafe extern "system" fn($($param),*) -> Ret
        where
            Ret: FunctionReturn,
            ($($param,)*): FunctionParamList,
        {
            type Ret = Ret;
            type Params = ($($param,)*);
        }
    };
}

impl_function_signature!();
impl_function_signature!(A0);
impl_function_signature!(A0, A1);
impl_function_signature!(A0, A1, A2);
impl_function_signature!(A0, A1, A2, A3);
impl_function_signature!(A0, A1, A2, A3, A4);
impl_function_signature!(A0, A1, A2, A3, A4, A5);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6, A7);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6, A7, A8);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12);
impl_function_signature!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, A13);
impl_function_signature!(
    A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, A13, A14
);
impl_function_signature!(
    A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, A13, A14, A15
);
