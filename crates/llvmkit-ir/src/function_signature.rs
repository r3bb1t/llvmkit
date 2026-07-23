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
use crate::block_state::Unterminated;
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::float_kind::{BFloat, Fp128, Half, IntoFloatValue, PpcFp128, X86Fp80};
use crate::function::FunctionValue;
use crate::int_width::{IntoIntValue, Width};
use crate::ir_builder::{IRBuilder, Unpositioned, constant_folder::ConstantFolder};
use crate::marker::{Ptr, ReturnMarker};
use crate::module::{Brand, Module, ModuleBrand, ModuleRef, Unverified};
use crate::r#type::{Type, TypeKind};
use crate::value::{FloatValue, IntValue, IntoPointerValue, PointerValue, Value, ValueId};

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

    /// Capability proving that a call result's type was validated when the
    /// typed callee facade was constructed. Only this crate mints it.
    #[derive(Debug)]
    pub struct ValidatedCallResult<'a> {
        _private: core::marker::PhantomData<&'a ()>,
    }

    impl<'a> ValidatedCallResult<'a> {
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

    /// Branded result handle of a typed call to a callee with this
    /// return schema: `()` for void, `IntValue<'ctx, i32, B>` for
    /// `i32`, `S::Value<'ctx, B>` for a struct schema, etc.
    type CallResult<'ctx, B: ModuleBrand + 'ctx>;

    /// Wrap a raw call result. The token is only minted by this crate
    /// after the callee schema was validated
    /// (`TypedFunctionValue::try_from_function`), so the unchecked
    /// wraps below cannot mistype.
    fn call_result_from_value<'ctx, B>(
        value: Value<'ctx, B>,
        validated: &token::ValidatedCallResult<'_>,
    ) -> Self::CallResult<'ctx, B>
    where
        B: ModuleBrand + 'ctx;
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

    /// Convert a previously-validated raw [`Value`] into the branded value —
    /// the [`Value`]-sourced analog of [`Self::value_from_argument`]. A
    /// block's parameters are its leading head-phi *results* (plain
    /// [`Value`]s), not [`Argument`]s, so the typed block constructor
    /// [`crate::IRBuilder::append_block_typed`] wraps each head-phi through
    /// this method. Reuses `from_value_unchecked` exactly as
    /// [`Self::value_from_argument`] does (that method is precisely this one
    /// applied to `arg.into_erased()`), and carries the same capability gate:
    /// the token is only minted by this crate after the phi types were built
    /// from this schema, so the unchecked wrap cannot mistype and safe
    /// downstream code cannot reach it.
    fn value_from_value<'ctx, B>(
        value: Value<'ctx, B>,
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

    /// Return typed parameter values sourced from a block's leading head-phi
    /// result [`Value`]s (declaration order) — the block-argument analog of
    /// [`Self::values`], which sources from a function's [`Argument`]s.
    /// `phi_values[i]` must be the head-phi built for parameter `i` from this
    /// schema's [`Self::ir_types`]; [`crate::IRBuilder::append_block_typed`]
    /// establishes that arity and ordering (one phi per `ir_types` entry, in
    /// order) before minting the capability token, so the per-position
    /// unchecked wraps cannot mistype.
    fn values_from_phi_values<'ctx, B>(
        phi_values: &[Value<'ctx, B>],
        validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Values<'ctx, B>
    where
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
        if function.signature().is_var_arg() {
            return Err(IrError::UnexpectedVarArgsSignature);
        }
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
    ) -> BasicBlock<'ctx, Ret::Marker, Unterminated, B>
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

/// Variadic twin of [`TypedFunctionValue`]: wraps a raw function whose
/// signature is `(Params..., ...)` — the fixed-prefix parameters are
/// statically typed via `Params`, and the `...` tail is accepted at
/// each call site through [`crate::IRBuilder::build_varargs_call`]'s
/// erased trailing argument list. Mirrors LLVM's variadic-function
/// convention (`FunctionType::isVarArg`); the fixed-arity
/// [`TypedFunctionValue`] and this facade are mutually exclusive —
/// each requires the opposite of [`crate::derived_types::FunctionType::is_var_arg`]
/// at construction time.
pub struct TypedVarArgsFunctionValue<'ctx, Ret, Params, B: ModuleBrand = Brand<'ctx>>
where
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    function: FunctionValue<'ctx, Ret::Marker, B>,
    _ret: PhantomData<Ret>,
    _params: PhantomData<Params>,
}

impl<'ctx, Ret, Params, B> Clone for TypedVarArgsFunctionValue<'ctx, Ret, Params, B>
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

impl<'ctx, Ret, Params, B> Copy for TypedVarArgsFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
}

impl<'ctx, Ret, Params, B> PartialEq for TypedVarArgsFunctionValue<'ctx, Ret, Params, B>
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

impl<'ctx, Ret, Params, B> Eq for TypedVarArgsFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
}

impl<'ctx, Ret, Params, B> Hash for TypedVarArgsFunctionValue<'ctx, Ret, Params, B>
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

impl<'ctx, Ret, Params, B> fmt::Debug for TypedVarArgsFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedVarArgsFunctionValue")
            .field("function", &self.function)
            .finish()
    }
}

impl<'ctx, Ret, Params, B> TypedVarArgsFunctionValue<'ctx, Ret, Params, B>
where
    B: ModuleBrand + 'ctx,
    Ret: FunctionReturn,
    Params: FunctionParamList,
{
    /// Wrap an existing raw function after validating the fixed-prefix
    /// schema and the variadic marker.
    pub fn try_from_function(function: FunctionValue<'ctx, Ret::Marker, B>) -> IrResult<Self> {
        if !function.signature().is_var_arg() {
            return Err(IrError::MissingVarArgsSignature);
        }
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

    /// Return typed fixed-prefix parameter values in declaration order.
    /// The `...` tail is not represented here — it is supplied
    /// per-call through [`crate::IRBuilder::build_varargs_call`].
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
    ) -> BasicBlock<'ctx, Ret::Marker, Unterminated, B>
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

    type CallResult<'ctx, B: ModuleBrand + 'ctx> = ();

    #[inline]
    fn call_result_from_value<'ctx, B>(
        value: Value<'ctx, B>,
        validated: &token::ValidatedCallResult<'_>,
    ) -> Self::CallResult<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        let _ = (value, validated);
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

    type CallResult<'ctx, B: ModuleBrand + 'ctx> = PointerValue<'ctx, B>;

    #[inline]
    fn call_result_from_value<'ctx, B>(
        value: Value<'ctx, B>,
        _validated: &token::ValidatedCallResult<'_>,
    ) -> Self::CallResult<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        PointerValue::from_value_unchecked(value)
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
        PointerValue::from_value_unchecked(arg.into_erased())
    }

    #[inline]
    fn value_from_value<'ctx, B>(
        value: Value<'ctx, B>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        PointerValue::from_value_unchecked(value)
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

            type CallResult<'ctx, B: ModuleBrand + 'ctx> = IntValue<'ctx, $marker, B>;

            #[inline]
            fn call_result_from_value<'ctx, B>(
                value: Value<'ctx, B>,
                _validated: &token::ValidatedCallResult<'_>,
            ) -> Self::CallResult<'ctx, B>
            where
                B: ModuleBrand + 'ctx,
            {
                IntValue::<$marker, B>::from_value_unchecked(value)
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
                IntValue::<$marker, B>::from_value_unchecked(arg.into_erased())
            }

            #[inline]
            fn value_from_value<'ctx, B>(
                value: Value<'ctx, B>,
                _validated: &ValidatedFunctionParams<'_>,
            ) -> Self::Value<'ctx, B>
            where
                B: ModuleBrand + 'ctx,
            {
                IntValue::<$marker, B>::from_value_unchecked(value)
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

    type CallResult<'ctx, B: ModuleBrand + 'ctx> = IntValue<'ctx, Width<N>, B>;

    #[inline]
    fn call_result_from_value<'ctx, B>(
        value: Value<'ctx, B>,
        _validated: &token::ValidatedCallResult<'_>,
    ) -> Self::CallResult<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        IntValue::<Width<N>, B>::from_value_unchecked(value)
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
        IntValue::<Width<N>, B>::from_value_unchecked(arg.into_erased())
    }

    #[inline]
    fn value_from_value<'ctx, B>(
        value: Value<'ctx, B>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        IntValue::<Width<N>, B>::from_value_unchecked(value)
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

            type CallResult<'ctx, B: ModuleBrand + 'ctx> = FloatValue<'ctx, $marker, B>;

            #[inline]
            fn call_result_from_value<'ctx, B>(
                value: Value<'ctx, B>,
                _validated: &token::ValidatedCallResult<'_>,
            ) -> Self::CallResult<'ctx, B>
            where
                B: ModuleBrand + 'ctx,
            {
                FloatValue::<$marker, B>::from_value_unchecked(value)
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
                FloatValue::<$marker, B>::from_value_unchecked(arg.into_erased())
            }

            #[inline]
            fn value_from_value<'ctx, B>(
                value: Value<'ctx, B>,
                _validated: &ValidatedFunctionParams<'_>,
            ) -> Self::Value<'ctx, B>
            where
                B: ModuleBrand + 'ctx,
            {
                FloatValue::<$marker, B>::from_value_unchecked(value)
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

    #[inline]
    fn values_from_phi_values<'ctx, B>(
        _phi_values: &[Value<'ctx, B>],
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Values<'ctx, B>
    where
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

            #[inline]
            fn values_from_phi_values<'ctx, B>(
                phi_values: &[Value<'ctx, B>],
                validated: &ValidatedFunctionParams<'_>,
            ) -> Self::Values<'ctx, B>
            where
                B: ModuleBrand + 'ctx,
            {
                // `append_block_typed` builds exactly one head-phi per
                // `ir_types` entry (arity `$arity`) in order, so `$slot` is
                // always in bounds and names the phi for position `$slot`.
                ($($param::value_from_value(phi_values[$slot], validated),)+)
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

/// Inputs that can fill the call-argument slot described by schema
/// token `P` in a typed call. Mirrors the multi-source posture of
/// [`IntoIrField`](crate::IntoIrField): typed handles, constants, Rust
/// literals, `Argument`, and erased `Value` all lift through the
/// underlying operand traits. Cross-module rejection lives inside
/// those traits' `into_*_value(module)` methods (D7), not at the call
/// site.
///
/// ## Diagnostic behavior
///
/// The `#[diagnostic::on_unimplemented]` message below only fires when a
/// call-argument type has **zero** candidate `IntoCallArg<P>` impls at
/// all -- the case for schema types with no matching family (e.g. a
/// struct-schema slot fed a type with no `IntoCallArg` impl in scope, or
/// an unrelated concrete type like `String`). For the int / float /
/// pointer slot families, `IntoCallArg` is implemented through a blanket
/// impl bounded by the underlying lift trait (`IntoIntValue<W>` /
/// `IntoFloatValue<K>` / `IntoPointerValue`); when the argument fails to
/// satisfy that lift-trait bound, rustc reports the *lift trait* as
/// unsatisfied, not `IntoCallArg` itself, so this message is not shown --
/// the caller instead sees the root `IntoIntValue<W>` (or float/pointer
/// equivalent) trait-bound error. Both shapes are locked by compile-fail
/// fixtures: `tests/compile_fail/typed_call_wrong_arg_type.rs` (no
/// candidate impl -- this message fires) and
/// `tests/compile_fail/typed_call_wrong_arg_type_lifted.rs` (candidate
/// impl exists, root lift-trait bound fails instead).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot fill a call-argument slot of schema `{P}`",
    label = "wrong argument type for this parameter position"
)]
pub trait IntoCallArg<'ctx, P: FunctionParam, B: ModuleBrand = Brand<'ctx>>: Sized {
    #[doc(hidden)]
    fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

macro_rules! impl_into_call_arg_int {
    ($($w:ty),+ $(,)?) => {$(
        impl<'ctx, B, V> IntoCallArg<'ctx, $w, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoIntValue<'ctx, $w, B>,
        {
            #[inline]
            fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_int_value(module)?.into_erased())
            }
        }
    )+};
}
impl_into_call_arg_int!(bool, i8, i16, i32, i64, i128);

impl<'ctx, B, V, const N: u32> IntoCallArg<'ctx, Width<N>, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoIntValue<'ctx, Width<N>, B>,
{
    #[inline]
    fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_int_value(module)?.into_erased())
    }
}

macro_rules! impl_into_call_arg_float {
    ($($k:ty),+ $(,)?) => {$(
        impl<'ctx, B, V> IntoCallArg<'ctx, $k, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoFloatValue<'ctx, $k, B>,
        {
            #[inline]
            fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_float_value(module)?.into_erased())
            }
        }
    )+};
}
impl_into_call_arg_float!(f32, f64, Half, BFloat, Fp128, X86Fp80, PpcFp128);

impl<'ctx, B, V> IntoCallArg<'ctx, Ptr, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoPointerValue<'ctx, B>,
{
    #[inline]
    fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_pointer_value(module)?.into_erased())
    }
}

mod call_args_sealed {
    pub trait Sealed {}
}

/// Argument tuple for a typed call site: arity must equal
/// `Params::ARITY` and position `i` must satisfy `IntoCallArg<P_i>`.
/// Wrong arity has no impl (compile error); a wrong-typed position
/// fails its `IntoCallArg` bound (compile error).
#[diagnostic::on_unimplemented(
    message = "argument tuple `{Self}` does not match the callee's parameter schema `{Params}`",
    note = "argument count and per-position types must match the callee's typed signature"
)]
pub trait CallArgs<'ctx, Params: FunctionParamList, B: ModuleBrand = Brand<'ctx>>:
    Sized + call_args_sealed::Sealed
{
    #[doc(hidden)]
    fn lower(self, module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>>;
}

impl call_args_sealed::Sealed for () {}
impl<'ctx, B: ModuleBrand + 'ctx> CallArgs<'ctx, (), B> for () {
    #[inline]
    fn lower(self, _module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>> {
        Ok(Box::new([]))
    }
}

macro_rules! impl_call_args_tuple {
    ($($p:ident / $v:ident / $x:ident),+) => {
        impl<$($v),+> call_args_sealed::Sealed for ($($v,)+) {}

        impl<'ctx, B, $($p,)+ $($v,)+> CallArgs<'ctx, ($($p,)+), B> for ($($v,)+)
        where
            B: ModuleBrand + 'ctx,
            $($p: FunctionParam,)+
            $($v: IntoCallArg<'ctx, $p, B>,)+
        {
            fn lower(self, module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>> {
                let ($($x,)+) = self;
                Ok(Box::new([$( $x.into_call_arg(module)?.id(), )+]))
            }
        }
    };
}
impl_call_args_tuple!(P0 / V0 / v0);
impl_call_args_tuple!(P0 / V0 / v0, P1 / V1 / v1);
impl_call_args_tuple!(P0 / V0 / v0, P1 / V1 / v1, P2 / V2 / v2);
impl_call_args_tuple!(P0 / V0 / v0, P1 / V1 / v1, P2 / V2 / v2, P3 / V3 / v3);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8,
    P9 / V9 / v9
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8,
    P9 / V9 / v9,
    P10 / V10 / v10
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8,
    P9 / V9 / v9,
    P10 / V10 / v10,
    P11 / V11 / v11
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8,
    P9 / V9 / v9,
    P10 / V10 / v10,
    P11 / V11 / v11,
    P12 / V12 / v12
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8,
    P9 / V9 / v9,
    P10 / V10 / v10,
    P11 / V11 / v11,
    P12 / V12 / v12,
    P13 / V13 / v13
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8,
    P9 / V9 / v9,
    P10 / V10 / v10,
    P11 / V11 / v11,
    P12 / V12 / v12,
    P13 / V13 / v13,
    P14 / V14 / v14
);
impl_call_args_tuple!(
    P0 / V0 / v0,
    P1 / V1 / v1,
    P2 / V2 / v2,
    P3 / V3 / v3,
    P4 / V4 / v4,
    P5 / V5 / v5,
    P6 / V6 / v6,
    P7 / V7 / v7,
    P8 / V8 / v8,
    P9 / V9 / v9,
    P10 / V10 / v10,
    P11 / V11 / v11,
    P12 / V12 / v12,
    P13 / V13 / v13,
    P14 / V14 / v14,
    P15 / V15 / v15
);
