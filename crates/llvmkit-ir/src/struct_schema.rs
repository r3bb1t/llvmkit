//! Compile-time schemas for named LLVM struct types.
//!
//! A [`StructSchema`] names an identified struct and lists its fields. Each
//! field is an [`IrField`], and each field kind names a [`FieldLayout`]: one
//! of a sealed set of valid struct elements that llvmkit builds and matches
//! itself. So a schema's body is minted from its layouts alone, in the module
//! that asks; building and matching cannot disagree; and no field can be laid
//! out as an invalid element or as another module's type.
//!
//! A schema's value holds a [`SchemaValue`], which only llvmkit mints — after
//! checking a raw value against the schema, or as the result of its own typed
//! operations — so holding one proves the value has the schema's type. All
//! concrete values stay branded by the module that produced them.

use core::marker::PhantomData;

use crate::argument::Argument;
use crate::array_len::{ArrLen, static_len_u64};
use crate::capability::{Capability, Mutable, ReadOnly};
use crate::constant::Constant;
use crate::derived_types::StructType;
use crate::element::VecElem;
use crate::error::{IrError, IrResult, RenderedType, TypeKindLabel};
use crate::float_kind::{Bfloat, Fp128, Half, IntoFloatValue, PpcFp128, X86Fp80};
use crate::function::FunctionValue;
use crate::function_signature::{
    CallArgs, FunctionParam, FunctionParamList, FunctionReturn, IntoCallArg, LoweredCallArguments,
    token::{ValidatedCallResult, ValidatedFunctionParams},
};
use crate::instruction::{Instruction, state::Attached};
use crate::int_width::{IntoIntValue, Width};
use crate::marker::{Dyn, Ptr, ReturnMarker};
use crate::module::{ModuleBrand, ModuleRef, UniqueBrand};
use crate::struct_body_state::BodySet;
use crate::r#type::{Type, TypeData};
use crate::value::{
    ArrayValue, FloatValue, IntValue, IntoErasedValue, IntoPointerValue, PointerValue, StructValue,
    Value,
};
use crate::{Branded, CrateOnly};

/// Home of [`Minting`]. Private, so the type is nameable nowhere outside
/// llvmkit — the hidden [`FieldLayout::mint_type`] names it, and only llvmkit
/// can build one.
mod minting {
    /// The schemas one minting call is inside, by LLVM name. A nested schema
    /// found here is being minted further up the same call — it contains
    /// itself by value — so the minter hands back its still-opaque named type
    /// instead of recursing (`ModuleRef::mint_struct_of`).
    #[derive(Debug, Default)]
    pub struct Minting {
        pub(super) in_progress: Vec<&'static str>,
    }
}

pub(crate) use minting::Minting;

impl Minting {
    /// A minting call with no schema in progress.
    #[inline]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether the schema named `name` is being minted further up this call.
    #[inline]
    pub(crate) fn is_minting(&self, name: &'static str) -> bool {
        self.in_progress.contains(&name)
    }

    /// Enter the schema named `name`.
    #[inline]
    pub(crate) fn begin(&mut self, name: &'static str) {
        self.in_progress.push(name);
    }

    /// Leave the schema entered last.
    #[inline]
    pub(crate) fn end(&mut self) {
        self.in_progress.pop();
    }
}

mod layout_sealed {
    pub trait Sealed {}
}

mod field_list_sealed {
    pub trait Sealed {}
}

/// What a struct field is in LLVM: one of a closed set of valid struct
/// elements, whose type llvmkit builds and matches itself.
///
/// Implemented for `bool`, `i8`…`i128`, [`Width<N>`], the floating-point
/// kinds, [`Ptr<AS>`], an array `[L; N]` of a scalar layout
/// ([`ArrayElementLayout`]), and every [`StructSchema`]. Sealed: a layout
/// anyone could write could name `label`, or another module's type. Its
/// methods other than [`Value`](Self::Value) take llvmkit's call token, so
/// only llvmkit builds a layout's type or wraps a value as one.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a struct-field layout",
    label = "not a `FieldLayout`",
    note = "a field's layout is `bool`, `i8`…`i128`, `Width<N>`, a float kind, `Ptr<AS>`, \
            an array `[L; N]` of a scalar layout, or a `StructSchema`"
)]
pub trait FieldLayout: layout_sealed::Sealed + Sized + 'static {
    /// The value of a field of this layout: what a field read returns.
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability>: Copy;

    /// Build this layout's type in `module`, whose reference is [`Mutable`]
    /// because a schema's struct is created and its body set here (D8). A
    /// schema's struct type may be refused — [`IrError::StructBodyMismatch`],
    /// [`IrError::RecursiveStructBody`] — and a scalar's never is.
    #[doc(hidden)]
    fn mint_type<'ctx, B: ModuleBrand + 'ctx>(
        module: ModuleRef<'ctx, B>,
        minting: &mut Minting,
        token: CrateOnly,
    ) -> IrResult<Type<'ctx, B>>;

    /// This layout's type as a diagnostic names it, at any capability: a
    /// schema is spelled from its name, so naming the type a check wanted
    /// never creates it.
    #[doc(hidden)]
    fn rendered<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        module: ModuleRef<'ctx, B, C>,
        token: CrateOnly,
    ) -> RenderedType;

    /// Whether `ty` is this layout's type.
    #[doc(hidden)]
    fn matches_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        ty: Type<'ctx, B, C>,
        token: CrateOnly,
    ) -> bool;

    /// Wrap `raw`, whose type llvmkit has proven to be this layout's, as the
    /// layout's value. Unchecked: the proof is the caller's.
    #[doc(hidden)]
    fn value_of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: Value<'ctx, B, C>,
        token: CrateOnly,
    ) -> Self::Value<'ctx, B, C>;

    /// The layout's value, erased.
    #[doc(hidden)]
    fn erase<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
        token: CrateOnly,
    ) -> Value<'ctx, B, C>;
}

/// The value of a field of layout `L`.
pub type LayoutValue<'ctx, L, B, C> = <L as FieldLayout>::Value<'ctx, B, C>;

/// A layout an array field's element may have: the scalar integer and
/// floating-point layouts. `Width<N>`, `Ptr<AS>` and schemas stay out of
/// arrays for now — a typed array value names its element by a vector-element
/// marker, and they have none.
pub trait ArrayElementLayout: FieldLayout + VecElem {}

/// A struct field's kind: the layout it has in LLVM, and the value a field
/// read returns.
///
/// The built-in kinds are the layouts themselves (`bool`, `i8`…`i128`,
/// [`Width<N>`], the float kinds, [`Ptr<AS>`], every [`StructSchema`]), with
/// [`wrap`](Self::wrap) and [`unwrap`](Self::unwrap) the identity. `u8`…`u128`
/// have the signless integer layout of their width — LLVM integers carry no
/// sign — and that integer's value. `[T; N]` has the layout `[T::Layout; N]`
/// and an [`ArrayValue`] of it. A kind of the user's own names a layout and
/// converts its values; llvmkit derives the field's type, its match and a
/// schema's whole body from [`Layout`](Self::Layout) alone:
///
/// ```
/// use llvmkit_ir::{Capability, FloatValue, IrField, LayoutValue, ModuleBrand};
///
/// /// A distance, laid out as a `double`.
/// struct Meters;
///
/// impl IrField for Meters {
///     type Layout = f64;
///     type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = FloatValue<'ctx, f64, B, C>;
///
///     fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
///         raw: LayoutValue<'ctx, f64, B, C>,
///     ) -> Self::Value<'ctx, B, C> {
///         raw
///     }
///
///     fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
///         value: Self::Value<'ctx, B, C>,
///     ) -> LayoutValue<'ctx, f64, B, C> {
///         value
///     }
/// }
/// ```
pub trait IrField: Sized + 'static {
    /// What this field is in LLVM.
    type Layout: FieldLayout;

    /// The value a field read returns.
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability>;

    /// The field's value from its layout's.
    fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: LayoutValue<'ctx, Self::Layout, B, C>,
    ) -> Self::Value<'ctx, B, C>;

    /// The layout's value from the field's.
    fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
    ) -> LayoutValue<'ctx, Self::Layout, B, C>;
}

/// Inputs a field of kind `F` accepts: whatever its layout accepts
/// ([`IntoLayoutValue`]), so a kind of the user's own takes its layout's
/// values with no impl of its own.
pub trait IntoIrField<'ctx, F: IrField, B: ModuleBrand>: Sized {
    fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

impl<'ctx, F, B, V> IntoIrField<'ctx, F, B> for V
where
    F: IrField,
    B: ModuleBrand + 'ctx,
    V: IntoLayoutValue<'ctx, F::Layout, B>,
{
    #[inline]
    fn into_ir_field(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        self.into_layout_value(module)
    }
}

/// Inputs a field of layout `L` accepts, admitted at the receiving module:
/// the typed handles of a scalar layout and the values that lift into them,
/// a typed array of an array layout, and — for a schema — its value or a raw
/// struct handle checked against it. The [`IrStruct`](crate::IrStruct) derive
/// implements it for the value type it generates.
pub trait IntoLayoutValue<'ctx, L: FieldLayout, B: ModuleBrand>: Sized {
    fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

/// A handle that may be a struct value, at capability `C`: the conversion
/// keeps the capability (D8).
#[doc(hidden)]
pub trait TryIntoStructValue<'ctx, B: ModuleBrand, C: Capability>: Sized {
    fn try_into_struct_value(self) -> IrResult<StructValue<'ctx, B, C>>;
}

impl<'ctx, B, C> TryIntoStructValue<'ctx, B, C> for StructValue<'ctx, B, C>
where
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    #[inline]
    fn try_into_struct_value(self) -> IrResult<StructValue<'ctx, B, C>> {
        Ok(self)
    }
}

macro_rules! impl_try_into_struct_value {
    ($source:ident) => {
        impl<'ctx, B, C> TryIntoStructValue<'ctx, B, C> for $source<'ctx, B, C>
        where
            B: ModuleBrand + 'ctx,
            C: Capability,
        {
            #[inline]
            fn try_into_struct_value(self) -> IrResult<StructValue<'ctx, B, C>> {
                StructValue::try_from(self)
            }
        }
    };
}

impl_try_into_struct_value!(Value);
impl_try_into_struct_value!(Argument);
impl_try_into_struct_value!(Constant);

// The linear `Instruction` handle is `Mutable` by construction (R7).
impl<'ctx, B> TryIntoStructValue<'ctx, B, Mutable> for Instruction<'ctx, Attached, B>
where
    B: ModuleBrand + 'ctx,
{
    #[inline]
    fn try_into_struct_value(self) -> IrResult<StructValue<'ctx, B>> {
        StructValue::try_from(self)
    }
}

/// A struct value of schema `S`'s type, at capability `C` — the capability
/// of the struct value it holds (D8).
///
/// Only llvmkit mints one: [`try_new`](Self::try_new) checks a raw value
/// against `S`, and llvmkit's own typed operations (field reads, builds,
/// typed loads, typed parameters) mint the values they produce. So holding a
/// `SchemaValue<S>` proves the value has `S`'s type, and a schema's value
/// type, which holds one, needs no check of its own.
#[derive(Branded)]
pub struct SchemaValue<'ctx, S: StructSchema, B: ModuleBrand, C: Capability = Mutable> {
    raw: StructValue<'ctx, B, C>,
    _schema: PhantomData<fn() -> S>,
}

impl<'ctx, S, B, C> SchemaValue<'ctx, S, B, C>
where
    S: StructSchema,
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    /// Wrap `raw`, whose type llvmkit has proven to be `S`'s. Unchecked: the
    /// proof is the caller's.
    #[inline]
    pub(crate) fn new_unchecked(raw: StructValue<'ctx, B, C>) -> Self {
        Self {
            raw,
            _schema: PhantomData,
        }
    }

    /// Check `value` against schema `S`: it must be a struct of `S`'s type —
    /// named `S::NAME`, `S::PACKED`, its elements `S`'s field layouts.
    ///
    /// # Errors
    ///
    /// A value that is no struct fails its narrowing; a struct of another
    /// type is [`IrError::TypeIdentityMismatch`], both types spelled out.
    pub fn try_new<V>(value: V) -> IrResult<Self>
    where
        V: TryIntoStructValue<'ctx, B, C>,
    {
        let raw = value.try_into_struct_value()?;
        check_struct_identity::<S, B, C>(raw)?;
        Ok(Self::new_unchecked(raw))
    }

    /// The struct value.
    #[inline]
    pub fn as_struct_value(self) -> StructValue<'ctx, B, C> {
        self.raw
    }

    /// The struct value, erased.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        self.raw.as_erased()
    }
}

/// A schema's value type, at capability `C` — the capability of the struct
/// value it wraps (D8). It holds a [`SchemaValue`], so it is valid for `S` by
/// construction.
pub trait StructSchemaValue<'ctx, S: StructSchema, B: ModuleBrand, C: Capability>:
    Sized + Copy
{
    /// Wrap a checked value of `S`.
    fn from_schema_value(value: SchemaValue<'ctx, S, B, C>) -> Self;

    /// The checked value this wraps.
    fn schema_value(self) -> SchemaValue<'ctx, S, B, C>;
}

/// Schema `S`'s struct type in a module, with `S`'s body — what
/// `get_or_insert_struct_of::<S>()` answers once it has confirmed or set the
/// body. Holding one proves `%S` exists with that body, so a build from it
/// (the derive's `build` / `try_build`) does no lookup and cannot meet
/// [`IrError::StructBodyMismatch`].
#[derive(Branded)]
pub struct StructOf<'ctx, S: StructSchema, B: ModuleBrand, C: Capability = Mutable> {
    ty: StructType<'ctx, BodySet, B, C>,
    _schema: PhantomData<fn() -> S>,
}

impl<'ctx, S, B, C> StructOf<'ctx, S, B, C>
where
    S: StructSchema,
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    /// Wrap `ty`, which llvmkit has confirmed is `%S` with `S`'s body.
    #[inline]
    pub(crate) fn new_unchecked(ty: StructType<'ctx, BodySet, B, C>) -> Self {
        Self {
            ty,
            _schema: PhantomData,
        }
    }

    /// The struct type.
    #[inline]
    pub fn struct_type(self) -> StructType<'ctx, BodySet, B, C> {
        self.ty
    }

    /// The struct type, erased.
    #[inline]
    pub fn as_type(self) -> Type<'ctx, B, C> {
        self.ty.as_type()
    }
}

/// What a build without `?` takes for a field of layout `L`: a value of the
/// layout's own type — its view handle, at any capability — or, for a scalar
/// layout, a Rust literal of that scalar, which becomes a constant. A storable
/// id is no argument: an id outlives its module, so only the checked
/// `try_build` takes one.
///
/// Keyed by layout, so a field kind of your own takes its layout's arguments.
/// Open, so the derive can implement it for the value type it generates; an
/// implementation can answer only through [`LoweredField::of`], which takes a
/// value of the layout's own type, so no argument carries another type.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a build argument for a field laid out as `{L}`",
    label = "not a `FieldArg` for this field",
    note = "a build without `?` takes a view handle of the field's own type (for `Ptr`, an \
            address-space-0 `PointerValue`) or a Rust scalar literal; storable ids and \
            other values go through `try_build`, which checks"
)]
pub trait FieldArg<'ctx, L: FieldLayout, B: UniqueBrand>: Sized {
    /// This argument as a value of the layout's type in `module`.
    fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, L, B>;
}

/// A build argument lowered to a value of layout `L`'s type — only llvmkit
/// mints one, from a value of that type ([`of`](Self::of)) or a constant of it.
pub struct LoweredField<'ctx, L: FieldLayout, B: ModuleBrand> {
    value: Value<'ctx, B>,
    _layout: PhantomData<fn() -> L>,
}

impl<'ctx, L, B> LoweredField<'ctx, L, B>
where
    L: FieldLayout,
    B: ModuleBrand + 'ctx,
{
    /// A value of layout `L`'s own type, in `module`. No module check: the
    /// registry keeps one live module per [`UniqueBrand`] brand, and both
    /// `value` and `module` borrow a live module of brand `B`.
    #[inline]
    pub fn of<C: Capability>(value: LayoutValue<'ctx, L, B, C>, module: ModuleRef<'ctx, B>) -> Self
    where
        B: UniqueBrand,
    {
        Self::new_unchecked(L::erase(value, CrateOnly(())).reminted_trusting_same_module(module))
    }

    /// Wrap `value`, which llvmkit has proven to be of `L`'s type and of the
    /// builder's module.
    #[inline]
    pub(crate) fn new_unchecked(value: Value<'ctx, B>) -> Self {
        Self {
            value,
            _layout: PhantomData,
        }
    }

    /// The lowered value.
    #[inline]
    pub(crate) fn value(self) -> Value<'ctx, B> {
        self.value
    }
}

/// A Rust integer literal as a build argument for the integer layout of its
/// width: a constant minted at the layout's own type. Unsigned literals fill
/// the signless layout of their width, as `u8`…`u128` fields do.
macro_rules! impl_int_literal_field_arg {
    ($($layout:ty => $method:ident : $($lit:ty),+);+ $(;)?) => {$($(
        impl<'ctx, B: UniqueBrand + 'ctx> FieldArg<'ctx, $layout, B> for $lit {
            #[inline]
            fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, $layout, B> {
                LoweredField::new_unchecked(module.$method().const_int(self).as_erased())
            }
        }
    )+)+};
}

impl_int_literal_field_arg!(
    bool => bool_type: bool;
    i8 => i8_type: i8, u8;
    i16 => i16_type: i16, u16;
    i32 => i32_type: i32, u32;
    i64 => i64_type: i64, u64;
    i128 => i128_type: i128, u128;
);

impl<'ctx, B: UniqueBrand + 'ctx> FieldArg<'ctx, f32, B> for f32 {
    #[inline]
    fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, f32, B> {
        LoweredField::new_unchecked(module.f32_type().const_float(self).as_erased())
    }
}

impl<'ctx, B: UniqueBrand + 'ctx> FieldArg<'ctx, f64, B> for f64 {
    #[inline]
    fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, f64, B> {
        LoweredField::new_unchecked(module.f64_type().const_double(self).as_erased())
    }
}

/// The identity check behind [`SchemaValue::try_new`]: `raw`'s type must be
/// the struct schema `S` describes. It reads `raw` only through its own
/// module, so it runs on a handle of any capability — the operand lifts below
/// check before they admit.
fn check_struct_identity<'ctx, S, B, C>(raw: StructValue<'ctx, B, C>) -> IrResult<()>
where
    S: StructSchema,
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    let got = raw.ty().as_type();
    if !<S as FieldLayout>::matches_type(got, CrateOnly(())) {
        // The match compares the struct's *name*, not just its kind, so the
        // common failure is one identified struct given where another was
        // required — and `expected: TypeKindLabel::Struct` rendered that as
        // "expected struct, got struct".
        return Err(IrError::TypeIdentityMismatch {
            expected: <S as FieldLayout>::rendered(got.module, CrateOnly(())),
            got: got.rendered(),
        });
    }
    Ok(())
}

/// Lifetime-free schema token for an LLVM identified struct.
pub trait StructSchema: Sized + 'static {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability>: StructSchemaValue<'ctx, Self, B, C>;

    /// Tuple of the field kinds, in source-layout order.
    type Fields: FieldList;

    /// LLVM identified-struct name, without the leading `%`. Must not be
    /// empty — `StructType::create(Context, "")` makes an anonymous struct —
    /// which minting the schema's type checks at compile time.
    const NAME: &'static str;

    /// Whether the struct body is packed.
    const PACKED: bool = false;
}

impl<S: StructSchema> layout_sealed::Sealed for S {}

impl<S> FieldLayout for S
where
    S: StructSchema,
{
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = S::Value<'ctx, B, C>;

    #[inline]
    fn mint_type<'ctx, B: ModuleBrand + 'ctx>(
        module: ModuleRef<'ctx, B>,
        minting: &mut Minting,
        _token: CrateOnly,
    ) -> IrResult<Type<'ctx, B>> {
        let slot = module.mint_struct_of::<S>(minting)?;
        Ok(Type::new(slot, module))
    }

    #[inline]
    fn rendered<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        _module: ModuleRef<'ctx, B, C>,
        _token: CrateOnly,
    ) -> RenderedType {
        RenderedType::of_named_struct(S::NAME)
    }

    fn matches_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        ty: Type<'ctx, B, C>,
        token: CrateOnly,
    ) -> bool {
        let TypeData::Struct(data) = ty.data() else {
            return false;
        };
        if data.identity.name() != Some(S::NAME) {
            return false;
        }
        let body = data.body.borrow();
        let Some(body) = body.as_ref() else {
            return false;
        };
        if body.packed != S::PACKED {
            return false;
        }
        let fields: Vec<Type<'ctx, B, C>> = body
            .elements
            .iter()
            .map(|slot| Type::new(*slot, ty.module))
            .collect();
        <S::Fields as FieldList>::matches_types(&fields, token)
    }

    #[inline]
    fn value_of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Self::Value<'ctx, B, C> {
        S::Value::from_schema_value(SchemaValue::new_unchecked(
            StructValue::from_value_unchecked(raw),
        ))
    }

    #[inline]
    fn erase<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Value<'ctx, B, C> {
        value.schema_value().as_erased()
    }
}

impl<S> IrField for S
where
    S: StructSchema,
{
    type Layout = S;
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = S::Value<'ctx, B, C>;

    #[inline]
    fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: LayoutValue<'ctx, S, B, C>,
    ) -> Self::Value<'ctx, B, C> {
        raw
    }

    #[inline]
    fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
    ) -> LayoutValue<'ctx, S, B, C> {
        value
    }
}

/// The built-in kind of a layout that is its own field kind: `wrap` and
/// `unwrap` are the identity.
macro_rules! impl_layout_field {
    ($kind:ty => $value:ident < $marker:ty >) => {
        impl IrField for $kind {
            type Layout = Self;
            type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = $value<'ctx, $marker, B, C>;

            #[inline]
            fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                raw: LayoutValue<'ctx, Self, B, C>,
            ) -> Self::Value<'ctx, B, C> {
                raw
            }

            #[inline]
            fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                value: Self::Value<'ctx, B, C>,
            ) -> LayoutValue<'ctx, Self, B, C> {
                value
            }
        }
    };
}

macro_rules! impl_int_layout {
    ($($w:ty => $method:ident, $bits:literal),+ $(,)?) => {$(
        impl layout_sealed::Sealed for $w {}

        impl FieldLayout for $w {
            type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = IntValue<'ctx, $w, B, C>;

            #[inline]
            fn mint_type<'ctx, B: ModuleBrand + 'ctx>(
                module: ModuleRef<'ctx, B>,
                _minting: &mut Minting,
                _token: CrateOnly,
            ) -> IrResult<Type<'ctx, B>> {
                Ok(module.$method().as_type())
            }

            #[inline]
            fn rendered<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                module: ModuleRef<'ctx, B, C>,
                _token: CrateOnly,
            ) -> RenderedType {
                module.$method().as_type().rendered()
            }

            #[inline]
            fn matches_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                ty: Type<'ctx, B, C>,
                _token: CrateOnly,
            ) -> bool {
                matches!(ty.kind(), crate::TypeKind::Integer { bits } if bits == $bits)
            }

            #[inline]
            fn value_of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                raw: Value<'ctx, B, C>,
                _token: CrateOnly,
            ) -> Self::Value<'ctx, B, C> {
                IntValue::from_value_unchecked(raw)
            }

            #[inline]
            fn erase<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                value: Self::Value<'ctx, B, C>,
                _token: CrateOnly,
            ) -> Value<'ctx, B, C> {
                value.as_erased()
            }
        }

        impl ArrayElementLayout for $w {}

        impl<'ctx, B: UniqueBrand + 'ctx, C: Capability> FieldArg<'ctx, $w, B>
            for IntValue<'ctx, $w, B, C>
        {
            #[inline]
            fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, $w, B> {
                LoweredField::of(self, module)
            }
        }

        impl_layout_field!($w => IntValue<$w>);

        impl<'ctx, B, V> IntoLayoutValue<'ctx, $w, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoIntValue<'ctx, $w, B>,
        {
            #[inline]
            fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_int_value(module)?.as_erased())
            }
        }
    )+};
}

impl_int_layout!(
    bool => bool_type, 1,
    i8 => i8_type, 8,
    i16 => i16_type, 16,
    i32 => i32_type, 32,
    i64 => i64_type, 64,
    i128 => i128_type, 128,
);

/// `u8`…`u128` fields: the signless integer of their width, since LLVM
/// integers carry no sign, and that integer's value.
macro_rules! impl_unsigned_field {
    ($($u:ty => $signless:ty),+ $(,)?) => {$(
        impl IrField for $u {
            type Layout = $signless;
            type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = IntValue<'ctx, $signless, B, C>;

            #[inline]
            fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                raw: LayoutValue<'ctx, $signless, B, C>,
            ) -> Self::Value<'ctx, B, C> {
                raw
            }

            #[inline]
            fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                value: Self::Value<'ctx, B, C>,
            ) -> LayoutValue<'ctx, $signless, B, C> {
                value
            }
        }
    )+};
}

impl_unsigned_field!(u8 => i8, u16 => i16, u32 => i32, u64 => i64, u128 => i128);

impl<const N: u32> layout_sealed::Sealed for Width<N> {}

impl<const N: u32> FieldLayout for Width<N> {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = IntValue<'ctx, Width<N>, B, C>;

    #[inline]
    fn mint_type<'ctx, B: ModuleBrand + 'ctx>(
        module: ModuleRef<'ctx, B>,
        _minting: &mut Minting,
        _token: CrateOnly,
    ) -> IrResult<Type<'ctx, B>> {
        Ok(module.int_type_n::<N>().as_type())
    }

    #[inline]
    fn rendered<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        module: ModuleRef<'ctx, B, C>,
        _token: CrateOnly,
    ) -> RenderedType {
        module.int_type_n::<N>().as_type().rendered()
    }

    #[inline]
    fn matches_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        ty: Type<'ctx, B, C>,
        _token: CrateOnly,
    ) -> bool {
        matches!(ty.kind(), crate::TypeKind::Integer { bits } if bits == N)
    }

    #[inline]
    fn value_of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Self::Value<'ctx, B, C> {
        IntValue::from_value_unchecked(raw)
    }

    #[inline]
    fn erase<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Value<'ctx, B, C> {
        value.as_erased()
    }
}

impl<'ctx, B: UniqueBrand + 'ctx, C: Capability, const N: u32> FieldArg<'ctx, Width<N>, B>
    for IntValue<'ctx, Width<N>, B, C>
{
    #[inline]
    fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, Width<N>, B> {
        LoweredField::of(self, module)
    }
}

impl<const N: u32> IrField for Width<N> {
    type Layout = Self;
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = IntValue<'ctx, Width<N>, B, C>;

    #[inline]
    fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: LayoutValue<'ctx, Self, B, C>,
    ) -> Self::Value<'ctx, B, C> {
        raw
    }

    #[inline]
    fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
    ) -> LayoutValue<'ctx, Self, B, C> {
        value
    }
}

impl<'ctx, B, V, const N: u32> IntoLayoutValue<'ctx, Width<N>, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoIntValue<'ctx, Width<N>, B>,
{
    #[inline]
    fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_int_value(module)?.as_erased())
    }
}

macro_rules! impl_float_layout {
    ($($k:ty => $method:ident, $kind:pat),+ $(,)?) => {$(
        impl layout_sealed::Sealed for $k {}

        impl FieldLayout for $k {
            type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = FloatValue<'ctx, $k, B, C>;

            #[inline]
            fn mint_type<'ctx, B: ModuleBrand + 'ctx>(
                module: ModuleRef<'ctx, B>,
                _minting: &mut Minting,
                _token: CrateOnly,
            ) -> IrResult<Type<'ctx, B>> {
                Ok(module.$method().as_type())
            }

            #[inline]
            fn rendered<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                module: ModuleRef<'ctx, B, C>,
                _token: CrateOnly,
            ) -> RenderedType {
                module.$method().as_type().rendered()
            }

            #[inline]
            fn matches_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                ty: Type<'ctx, B, C>,
                _token: CrateOnly,
            ) -> bool {
                matches!(ty.kind(), $kind)
            }

            #[inline]
            fn value_of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                raw: Value<'ctx, B, C>,
                _token: CrateOnly,
            ) -> Self::Value<'ctx, B, C> {
                FloatValue::from_value_unchecked(raw)
            }

            #[inline]
            fn erase<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                value: Self::Value<'ctx, B, C>,
                _token: CrateOnly,
            ) -> Value<'ctx, B, C> {
                value.as_erased()
            }
        }

        impl ArrayElementLayout for $k {}

        impl<'ctx, B: UniqueBrand + 'ctx, C: Capability> FieldArg<'ctx, $k, B>
            for FloatValue<'ctx, $k, B, C>
        {
            #[inline]
            fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, $k, B> {
                LoweredField::of(self, module)
            }
        }

        impl_layout_field!($k => FloatValue<$k>);

        impl<'ctx, B, V> IntoLayoutValue<'ctx, $k, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoFloatValue<'ctx, $k, B>,
        {
            #[inline]
            fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_float_value(module)?.as_erased())
            }
        }
    )+};
}

impl_float_layout!(
    f32 => f32_type, crate::TypeKind::Float,
    f64 => f64_type, crate::TypeKind::Double,
    Half => half_type, crate::TypeKind::Half,
    Bfloat => bfloat_type, crate::TypeKind::Bfloat,
    Fp128 => fp128_type, crate::TypeKind::Fp128,
    X86Fp80 => x86_fp80_type, crate::TypeKind::X86Fp80,
    PpcFp128 => ppc_fp128_type, crate::TypeKind::PpcFp128,
);

impl<const AS: u32> layout_sealed::Sealed for Ptr<AS> {}

impl<const AS: u32> FieldLayout for Ptr<AS> {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> =
        PointerValue<'ctx, crate::Space<AS>, B, C>;

    #[inline]
    fn mint_type<'ctx, B: ModuleBrand + 'ctx>(
        module: ModuleRef<'ctx, B>,
        _minting: &mut Minting,
        _token: CrateOnly,
    ) -> IrResult<Type<'ctx, B>> {
        Ok(module.ptr_type_in::<AS>().as_type())
    }

    #[inline]
    fn rendered<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        module: ModuleRef<'ctx, B, C>,
        _token: CrateOnly,
    ) -> RenderedType {
        module.ptr_type_in::<AS>().as_type().rendered()
    }

    /// A pointer in address space `AS` exactly.
    #[inline]
    fn matches_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        ty: Type<'ctx, B, C>,
        _token: CrateOnly,
    ) -> bool {
        crate::PointerType::<crate::Space<AS>, B, C>::try_from(ty).is_ok()
    }

    #[inline]
    fn value_of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Self::Value<'ctx, B, C> {
        PointerValue::from_value_unchecked(raw)
    }

    #[inline]
    fn erase<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Value<'ctx, B, C> {
        value.as_erased()
    }
}

// A `Ptr<AS>` field's build argument is a pointer in address space `AS` by
// type — never a `SpaceDyn` one, whose space is known only at run time.
impl<'ctx, B: UniqueBrand + 'ctx, C: Capability, const AS: u32> FieldArg<'ctx, Ptr<AS>, B>
    for PointerValue<'ctx, crate::Space<AS>, B, C>
{
    #[inline]
    fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, Ptr<AS>, B> {
        LoweredField::of(self, module)
    }
}

impl<const AS: u32> IrField for Ptr<AS> {
    type Layout = Self;
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> =
        PointerValue<'ctx, crate::Space<AS>, B, C>;

    #[inline]
    fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: LayoutValue<'ctx, Self, B, C>,
    ) -> Self::Value<'ctx, B, C> {
        raw
    }

    #[inline]
    fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
    ) -> LayoutValue<'ctx, Self, B, C> {
        value
    }
}

// A `Ptr<AS>` field takes a pointer in address space `AS`, by type.
impl<'ctx, B, V, const AS: u32> IntoLayoutValue<'ctx, Ptr<AS>, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoPointerValue<'ctx, B, Space = crate::Space<AS>>,
{
    #[inline]
    fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_pointer_value(module)?.as_erased())
    }
}

impl<L: ArrayElementLayout, const N: usize> layout_sealed::Sealed for [L; N] {}

impl<L: ArrayElementLayout, const N: usize> FieldLayout for [L; N] {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = ArrayValue<'ctx, L, ArrLen<N>, B, C>;

    /// `[N x L]`. Mirrors `ArrayType::get`, whose `isValidElementType`
    /// assert holds for every scalar layout.
    #[inline]
    fn mint_type<'ctx, B: ModuleBrand + 'ctx>(
        module: ModuleRef<'ctx, B>,
        minting: &mut Minting,
        token: CrateOnly,
    ) -> IrResult<Type<'ctx, B>> {
        let element = L::mint_type(module, minting, token)?;
        Ok(module.array_type(element, static_len_u64(N))?.as_type())
    }

    /// `[N x L]`, as `TypePrinting::print`'s `ArrayTyID` arm spells it.
    #[inline]
    fn rendered<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        module: ModuleRef<'ctx, B, C>,
        token: CrateOnly,
    ) -> RenderedType {
        let element = L::rendered(module, token);
        RenderedType::new(
            TypeKindLabel::Array,
            format!("[{N} x {element}]").into_boxed_str(),
        )
    }

    #[inline]
    fn matches_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        ty: Type<'ctx, B, C>,
        token: CrateOnly,
    ) -> bool {
        match *ty.data() {
            TypeData::Array { elem, n } => {
                n == static_len_u64(N) && L::matches_type(Type::new(elem, ty.module), token)
            }
            _ => false,
        }
    }

    #[inline]
    fn value_of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Self::Value<'ctx, B, C> {
        ArrayValue::from_value_unchecked(raw)
    }

    #[inline]
    fn erase<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
        _token: CrateOnly,
    ) -> Value<'ctx, B, C> {
        value.as_erased()
    }
}

impl<'ctx, L, B, C, const N: usize> FieldArg<'ctx, [L; N], B>
    for ArrayValue<'ctx, L, ArrLen<N>, B, C>
where
    L: ArrayElementLayout,
    B: UniqueBrand + 'ctx,
    C: Capability,
{
    #[inline]
    fn lower(self, module: ModuleRef<'ctx, B>) -> LoweredField<'ctx, [L; N], B> {
        LoweredField::of(self, module)
    }
}

impl<T, const N: usize> IrField for [T; N]
where
    T: IrField,
    T::Layout: ArrayElementLayout,
{
    type Layout = [T::Layout; N];
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> =
        ArrayValue<'ctx, T::Layout, ArrLen<N>, B, C>;

    #[inline]
    fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: LayoutValue<'ctx, [T::Layout; N], B, C>,
    ) -> Self::Value<'ctx, B, C> {
        raw
    }

    #[inline]
    fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
    ) -> LayoutValue<'ctx, [T::Layout; N], B, C> {
        value
    }
}

// An array field takes a typed array of its element and length.
impl<'ctx, L, B, C, const N: usize> IntoLayoutValue<'ctx, [L; N], B>
    for ArrayValue<'ctx, L, ArrLen<N>, B, C>
where
    L: ArrayElementLayout,
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    #[inline]
    fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        self.into_erased_value(module)
    }
}

/// The struct-schema operand lift behind the [`IntoLayoutValue`] and
/// [`IntoCallArg`] impls below: narrow `value` to a struct, check it is the
/// struct `S` names, then admit it at `module`. The narrow and the check
/// read `value` only through its own module; admission comes last.
fn struct_operand<'ctx, S, B, C>(
    value: Value<'ctx, B, C>,
    module: ModuleRef<'ctx, B>,
) -> IrResult<Value<'ctx, B>>
where
    S: StructSchema,
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    let raw = StructValue::try_from(value)?;
    check_struct_identity::<S, B, C>(raw)?;
    // Boundary: refuse a value another module minted. A handle of any
    // capability is admitted and re-minted at `module`'s, so reading a value
    // as an operand is not mutating it.
    raw.as_erased().admitted_at(module)
}

macro_rules! impl_struct_operand {
    ($trait:ident :: $method:ident) => {
        impl<'ctx, S, B, C> $trait<'ctx, S, B> for Value<'ctx, B, C>
        where
            S: StructSchema,
            B: ModuleBrand + 'ctx,
            C: Capability,
        {
            fn $method(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                struct_operand::<S, B, C>(self, module)
            }
        }
        impl<'ctx, S, B, C> $trait<'ctx, S, B> for Argument<'ctx, B, C>
        where
            S: StructSchema,
            B: ModuleBrand + 'ctx,
            C: Capability,
        {
            fn $method(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                struct_operand::<S, B, C>(self.as_erased(), module)
            }
        }
        impl<'ctx, S, B, C> $trait<'ctx, S, B> for Constant<'ctx, B, C>
        where
            S: StructSchema,
            B: ModuleBrand + 'ctx,
            C: Capability,
        {
            fn $method(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                struct_operand::<S, B, C>(self.as_erased(), module)
            }
        }
        impl<'ctx, S, B> $trait<'ctx, S, B> for Instruction<'ctx, Attached, B>
        where
            S: StructSchema,
            B: ModuleBrand + 'ctx,
        {
            fn $method(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                struct_operand::<S, B, Mutable>(Instruction::to_erased(&self), module)
            }
        }
    };
}

impl_struct_operand!(IntoLayoutValue::into_layout_value);
impl_struct_operand!(IntoCallArg::into_call_arg);

/// A schema's field tuple: the field kinds in source-layout order, from none
/// to sixteen. Sealed; its hidden methods take llvmkit's call token, so only
/// llvmkit mints or matches a body from it.
pub trait FieldList: field_list_sealed::Sealed {
    /// Each field's type, minted in `module` from its layout.
    #[doc(hidden)]
    fn mint_types<'ctx, B: ModuleBrand + 'ctx>(
        module: ModuleRef<'ctx, B>,
        minting: &mut Minting,
        token: CrateOnly,
    ) -> IrResult<Vec<Type<'ctx, B>>>;

    /// Whether `types` are the fields' types: one per field, each its
    /// field's layout's.
    #[doc(hidden)]
    fn matches_types<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        types: &[Type<'ctx, B, C>],
        token: CrateOnly,
    ) -> bool;
}

impl field_list_sealed::Sealed for () {}

impl FieldList for () {
    #[inline]
    fn mint_types<'ctx, B: ModuleBrand + 'ctx>(
        _module: ModuleRef<'ctx, B>,
        _minting: &mut Minting,
        _token: CrateOnly,
    ) -> IrResult<Vec<Type<'ctx, B>>> {
        Ok(Vec::new())
    }

    #[inline]
    fn matches_types<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        types: &[Type<'ctx, B, C>],
        _token: CrateOnly,
    ) -> bool {
        types.is_empty()
    }
}

macro_rules! impl_field_list {
    ($( ($($f:ident $ty:ident),+) );+ $(;)?) => {$(
        impl<$($f: IrField),+> field_list_sealed::Sealed for ($($f,)+) {}

        impl<$($f: IrField),+> FieldList for ($($f,)+) {
            #[inline]
            fn mint_types<'ctx, B: ModuleBrand + 'ctx>(
                module: ModuleRef<'ctx, B>,
                minting: &mut Minting,
                token: CrateOnly,
            ) -> IrResult<Vec<Type<'ctx, B>>> {
                Ok(vec![$(<$f::Layout as FieldLayout>::mint_type(module, minting, token)?),+])
            }

            #[inline]
            fn matches_types<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
                types: &[Type<'ctx, B, C>],
                token: CrateOnly,
            ) -> bool {
                let [$($ty),+] = types else {
                    return false;
                };
                true $(&& <$f::Layout as FieldLayout>::matches_type(*$ty, token))+
            }
        }
    )+};
}

impl_field_list! {
    (F0 t0);
    (F0 t0, F1 t1);
    (F0 t0, F1 t1, F2 t2);
    (F0 t0, F1 t1, F2 t2, F3 t3);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8, F9 t9);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8, F9 t9, F10 t10);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8, F9 t9, F10 t10, F11 t11);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8, F9 t9, F10 t10, F11 t11, F12 t12);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8, F9 t9, F10 t10, F11 t11, F12 t12, F13 t13);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8, F9 t9, F10 t10, F11 t11, F12 t12, F13 t13, F14 t14);
    (F0 t0, F1 t1, F2 t2, F3 t3, F4 t4, F5 t5, F6 t6, F7 t7, F8 t8, F9 t9, F10 t10, F11 t11, F12 t12, F13 t13, F14 t14, F15 t15);
}

/// The `I`-th top-level field schema of a field tuple. Implemented for
/// tuple arities 1..=16, one impl per (arity, index) pair, so an
/// out-of-range index is "no impl" -- a compile error at the
/// `field_gep::<S, _, I, _>` call site.
pub trait StructFieldAt<const I: u32> {
    type Field: IrField;
}

/// Field schema of `S` at index `I`.
pub type FieldOf<S, const I: u32> = <<S as StructSchema>::Fields as StructFieldAt<I>>::Field;

// `$generics` / `$tuple` are matched as single opaque token trees (each a
// parenthesized group, not a `$(...)+` repetition), so both can be spliced
// verbatim any number of times inside the `$idx`/`$pick` repetition below
// without Rust's "two independently-lengthed repetitions at the same
// depth" restriction -- only one sequence (`$idx`/`$pick`) is ever
// repeated in this macro arm.
macro_rules! impl_struct_field_at_row {
    ($generics:tt, $tuple:tt => [ $( $idx:literal : $pick:ident ),+ $(,)? ]) => {
        $(
            impl_struct_field_at_one!($generics, $tuple, $idx, $pick);
        )+
    };
}

macro_rules! impl_struct_field_at_one {
    (($($generics:tt)*), $tuple:tt, $idx:literal, $pick:ident) => {
        impl<$($generics)*> StructFieldAt<$idx> for $tuple {
            type Field = $pick;
        }
    };
}

macro_rules! impl_struct_field_at {
    ($( ($($f:ident),+) => $picks:tt );+ $(;)?) => {
        $(
            impl_struct_field_at_row!(($($f: IrField),+), ($($f,)+) => $picks);
        )+
    };
}

impl_struct_field_at! {
    (F0) => [0: F0];
    (F0, F1) => [0: F0, 1: F1];
    (F0, F1, F2) => [0: F0, 1: F1, 2: F2];
    (F0, F1, F2, F3) => [0: F0, 1: F1, 2: F2, 3: F3];
    (F0, F1, F2, F3, F4) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4];
    (F0, F1, F2, F3, F4, F5) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5];
    (F0, F1, F2, F3, F4, F5, F6) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6];
    (F0, F1, F2, F3, F4, F5, F6, F7) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8, F9) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8, 9: F9];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8, 9: F9, 10: F10];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8, 9: F9, 10: F10, 11: F11];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8, 9: F9, 10: F10, 11: F11, 12: F12];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8, 9: F9, 10: F10, 11: F11, 12: F12, 13: F13];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8, 9: F9, 10: F10, 11: F11, 12: F12, 13: F13, 14: F14];
    (F0, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7, 8: F8, 9: F9, 10: F10, 11: F11, 12: F12, 13: F13, 14: F14, 15: F15];
}

/// Explicit function-parameter marker that expands a schema into its
/// top-level fields — for a schema whose field kinds are all function
/// parameters too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StructFields<S: StructSchema>(core::marker::PhantomData<S>);

impl<S> FunctionParamList for StructFields<S>
where
    S: StructSchema,
    S::Fields: FunctionParamList,
{
    const ARITY: u32 = <S::Fields as FunctionParamList>::ARITY;
    type Values<'ctx, B: ModuleBrand + 'ctx, C: Capability> =
        <S::Fields as FunctionParamList>::Values<'ctx, B, C>;

    #[inline]
    fn ir_types<'ctx, B>(module: ModuleRef<'ctx, B>) -> IrResult<Vec<Type<'ctx, B, ReadOnly>>>
    where
        B: ModuleBrand + 'ctx,
    {
        <S::Fields as FunctionParamList>::ir_types(module)
    }

    #[inline]
    fn validate<'ctx, R, B>(function: FunctionValue<'ctx, R, B, ReadOnly>) -> IrResult<()>
    where
        R: ReturnMarker,
        B: ModuleBrand + 'ctx,
    {
        <S::Fields as FunctionParamList>::validate(function)
    }

    #[inline]
    fn values<'ctx, R, B, C: Capability>(
        function: FunctionValue<'ctx, R, B, C>,
        validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Values<'ctx, B, C>
    where
        R: ReturnMarker,
        B: ModuleBrand + 'ctx,
    {
        <S::Fields as FunctionParamList>::values(function, validated)
    }

    #[inline]
    fn values_from_phi_values<'ctx, B, C: Capability>(
        phi_values: &[Value<'ctx, B, C>],
        validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Values<'ctx, B, C>
    where
        B: ModuleBrand + 'ctx,
    {
        <S::Fields as FunctionParamList>::values_from_phi_values(phi_values, validated)
    }
}

impl<'ctx, B, S, A> CallArgs<'ctx, StructFields<S>, B> for A
where
    B: ModuleBrand + 'ctx,
    S: StructSchema,
    S::Fields: FunctionParamList,
    A: CallArgs<'ctx, S::Fields, B>,
{
    fn lower(self, module: ModuleRef<'ctx, B>, token: CrateOnly) -> IrResult<LoweredCallArguments> {
        <A as CallArgs<'ctx, S::Fields, B>>::lower(self, module, token)
    }
}

impl<S> FunctionReturn for S
where
    S: StructSchema,
{
    type Marker = Dyn;

    #[inline]
    fn ir_type<'ctx, B>(module: ModuleRef<'ctx, B>) -> IrResult<Type<'ctx, B, ReadOnly>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(module.get_or_insert_struct_of::<S>()?.as_type().read_only())
    }

    #[inline]
    fn matches_ir_type<'ctx, B, C: Capability>(ty: Type<'ctx, B, C>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        <S as FieldLayout>::matches_type(ty, CrateOnly(()))
    }

    #[inline]
    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Struct
    }

    type CallResult<'ctx, B: ModuleBrand + 'ctx, C: Capability> = S::Value<'ctx, B, C>;

    fn call_result_from_value<'ctx, B, C: Capability>(
        value: Value<'ctx, B, C>,
        _validated: &ValidatedCallResult<'_>,
    ) -> Self::CallResult<'ctx, B, C>
    where
        B: ModuleBrand + 'ctx,
    {
        <S as FieldLayout>::value_of(value, CrateOnly(()))
    }
}

impl<S> FunctionParam for S
where
    S: StructSchema,
{
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = S::Value<'ctx, B, C>;

    #[inline]
    fn ir_type<'ctx, B>(module: ModuleRef<'ctx, B>) -> IrResult<Type<'ctx, B, ReadOnly>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(module.get_or_insert_struct_of::<S>()?.as_type().read_only())
    }

    #[inline]
    fn matches_ir_type<'ctx, B, C: Capability>(ty: Type<'ctx, B, C>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        <S as FieldLayout>::matches_type(ty, CrateOnly(()))
    }

    fn validate_argument<'ctx, B>(arg: Argument<'ctx, B, ReadOnly>) -> IrResult<()>
    where
        B: ModuleBrand + 'ctx,
    {
        if <S as FieldLayout>::matches_type(arg.ty(), CrateOnly(())) {
            Ok(())
        } else {
            // Same reasoning as `check_struct_identity` above: the guard is
            // finer than the kind, so both sides must carry their spelling.
            Err(IrError::TypeIdentityMismatch {
                expected: <S as FieldLayout>::rendered(arg.ty().module, CrateOnly(())),
                got: arg.ty().rendered(),
            })
        }
    }

    fn value_from_argument<'ctx, B, C: Capability>(
        arg: Argument<'ctx, B, C>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B, C>
    where
        B: ModuleBrand + 'ctx,
    {
        <S as FieldLayout>::value_of(arg.as_erased(), CrateOnly(()))
    }

    fn value_from_value<'ctx, B, C: Capability>(
        value: Value<'ctx, B, C>,
        _validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B, C>
    where
        B: ModuleBrand + 'ctx,
    {
        <S as FieldLayout>::value_of(value, CrateOnly(()))
    }
}

#[cfg(test)]
mod tests {
    use crate::{CallArgs, CrateOnly, IrBuilder, IrError, Linkage, NoFolder};

    #[derive(llvmkit_macros::IrStruct)]
    #[llvmkit(crate = crate)]
    struct Point {
        x: i32,
        y: i32,
    }

    /// llvmkit-specific derive-emitted `IntoCallArg` impl; closest upstream
    /// coverage is `unittests/IR/InstructionsTest.cpp` for `CallInst` operand
    /// construction, since a derived struct value must lower through the same
    /// `CallArgs` seam as any other typed call argument.
    ///
    /// In-crate since Task 24 fix round 2: `lower` takes the crate-only token,
    /// so it cannot be called from an integration test.
    #[test]
    fn derive_emits_into_call_arg_for_struct_schema() -> Result<(), IrError> {
        // The schema is a Rust struct too; read its fields once, as the
        // integration tests of the derive do.
        let rust_point = Point { x: 1, y: 2 };
        let _ = rust_point.x + rust_point.y;

        let m = crate::module_new!("derived_call_arg")?;
        let f = m.add_typed_function::<i32, (Point,), _>("consume_point", Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");
        let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let point = m
            .get_or_insert_struct_of::<Point>()?
            .build(&b, 1_i32, 2_i32, "point");

        let ids = <(_,) as CallArgs<'_, (Point,), _>>::lower((point,), (&m).into(), CrateOnly(()))?;

        assert_eq!(ids.len(), 1, "expected one lowered call-argument id");
        b.ret(0_i32)?;
        Ok(())
    }
}
