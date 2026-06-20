//! Per-kind typed type handles + refinement enums. Mirrors
//! `llvm/include/llvm/IR/DerivedTypes.h`.
//!
//! Each handle (`IntType<'ctx>`, `FloatType<'ctx>`, ...) is a
//! `(TypeId, ModuleRef<'ctx>)` record. Both fields are themselves `Hash`
//! and `Eq`, so every handle derives the full
//! `Copy + Clone + PartialEq + Eq + Hash + Debug` surface without any
//! hand-written impls.
//!
//! Per-kind accessors call into the internal `TypeData::as_*` projection helpers
//! and `expect("<kind> invariant")` on the result. The by-construction
//! type-state invariant is named in exactly one place per accessor; no
//! `_ => unreachable!()` arms anywhere.
//!
//! Refinement enums mentioned in the IR foundation plan (Pivot 4):
//!
//! - [`SizedType`] - types you can `alloca` / `load` / `store`. Built via
//!   `TryFrom<Type>`; rejects function / label / metadata / token / void
//!   / opaque-struct / unsized-vector.
//! - [`BasicTypeEnum`] - first-class types that may hold SSA values
//!   (every `TypeID` except function, label, metadata, token, void, and
//!   opaque struct, per LangRef).
//! - [`AggregateType`] - array or struct (vectors deliberately
//!   excluded, matching `Type.h` + LangRef).
//! - [`BasicMetadataTypeEnum`] - basic + metadata, used for variadic
//!   intrinsic argument typing.

use core::fmt;

use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::module::{ModuleBrand, ModuleRef};
use crate::r#type::{Type, TypeData, TypeId, TypeKind};
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use crate::float_kind::{BFloat, FloatDyn, FloatKind, Fp128, Half, PpcFp128, X86Fp80};
use crate::int_width::{IntDyn, IntWidth};
use crate::r#type::{IrType, sealed};

// --------------------------------------------------------------------------
// Per-kind handles
// --------------------------------------------------------------------------

macro_rules! decl_type_handle {
    (
        $(#[$attr:meta])*
        $name:ident, $label:ident, predicate $pred:expr
    ) => {
        $(#[$attr])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name<'ctx, B: ModuleBrand = crate::module::Brand<'ctx>> {
            pub(crate) id: TypeId,
            pub(crate) module: ModuleRef<'ctx, B>,
        }

        impl<'ctx, B: ModuleBrand> $name<'ctx, B> {
            #[inline]
            pub(crate) fn new<M>(id: TypeId, module: M) -> Self
            where
                M: Into<ModuleRef<'ctx, B>>,
            {
                Self { id, module: module.into() }
            }

            /// Widen to the erased [`Type`] handle.
            #[inline]
            pub fn as_type(self) -> Type<'ctx, B> {
                Type { id: self.id, module: self.module }
            }
        }

        impl<'ctx, B: ModuleBrand> sealed::Sealed for $name<'ctx, B> {}
        impl<'ctx, B: ModuleBrand> IrType<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn as_type(self) -> Type<'ctx, B> { self.as_type() }
        }
        impl<'ctx, B: ModuleBrand> fmt::Display for $name<'ctx, B> {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                <Type<'ctx, B> as fmt::Display>::fmt(&self.as_type(), f)
            }
        }

        impl<'ctx, B: ModuleBrand> From<$name<'ctx, B>> for Type<'ctx, B> {
            #[inline]
            fn from(t: $name<'ctx, B>) -> Self { t.as_type() }
        }

        impl<'ctx, B: ModuleBrand> TryFrom<Type<'ctx, B>> for $name<'ctx, B> {
            type Error = IrError;
            #[inline]
            fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
                let pred: fn(&TypeData) -> bool = $pred;
                if pred(t.data()) {
                    Ok(Self { id: t.id(), module: t.module })
                } else {
                    Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::$label,
                        got: t.kind_label(),
                    })
                }
            }
        }
    };
}

decl_type_handle!(
    /// `void`. Mirrors `Type::getVoidTy`.
    VoidType, Void,
    predicate |d| matches!(d, TypeData::Void)
);
// `IntType<'ctx, W>` is hand-written below to carry the width marker;
// `FloatType<'ctx, K>` similarly carries the IEEE-kind marker.
decl_type_handle!(
    /// Opaque pointer (`ptr`, `ptr addrspace(N)`). Mirrors `PointerType`
    /// (`DerivedTypes.h`).
    PointerType, Pointer,
    predicate |d| matches!(d, TypeData::Pointer { .. })
);
decl_type_handle!(
    /// `[N x T]` array. Mirrors `ArrayType` (`DerivedTypes.h`).
    ArrayType, Array,
    predicate |d| matches!(d, TypeData::Array { .. })
);
// `StructType<'ctx, B = StructBodyDyn>` is hand-written below: the
// `B: StructBodyState` parameter expresses the body-set typestate
// (Doctrine D1 -- see `struct_body_state.rs`). Existing
// `StructType<'ctx>` references resolve to `StructType<'ctx,
// StructBodyDyn>` via the default.
/// Literal or identified struct. Mirrors `StructType` in
/// `llvm/include/llvm/IR/DerivedTypes.h`.
///
/// The `B: StructBodyState` parameter (default
/// [`crate::StructBodyDyn`]) tracks whether the struct's body has
/// been set. [`crate::Module::opaque_struct`] yields a
/// `StructType<'ctx, Opaque>`; [`crate::Module::set_struct_body`]
/// consumes the opaque handle and produces a `StructType<'ctx,
/// BodySet>`. The runtime-checked default keeps existing parsed-IR /
/// literal-struct call sites working without churn.
pub struct StructType<
    'ctx,
    Body: crate::struct_body_state::StructBodyState = crate::struct_body_state::StructBodyDyn,
    B: ModuleBrand = crate::module::Brand<'ctx>,
> {
    pub(crate) id: TypeId,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) _b: core::marker::PhantomData<Body>,
}

impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand> Clone
    for StructType<'ctx, Body, B>
{
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand> Copy
    for StructType<'ctx, Body, B>
{
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand> PartialEq
    for StructType<'ctx, Body, B>
{
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module
    }
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand> Eq
    for StructType<'ctx, Body, B>
{
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand> core::hash::Hash
    for StructType<'ctx, Body, B>
{
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
    }
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand> core::fmt::Debug
    for StructType<'ctx, Body, B>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StructType").field("id", &self.id).finish()
    }
}

impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand + 'ctx>
    StructType<'ctx, Body, B>
{
    #[inline]
    pub(crate) fn new<M>(id: TypeId, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            _b: core::marker::PhantomData,
        }
    }

    /// Re-tag the body-state marker. Crate-internal: only
    /// [`crate::Module::set_struct_body`] flips the public marker.
    #[inline]
    pub(crate) fn retag<Body2: crate::struct_body_state::StructBodyState>(
        self,
    ) -> StructType<'ctx, Body2, B> {
        StructType {
            id: self.id,
            module: self.module,
            _b: core::marker::PhantomData,
        }
    }

    /// Erase the body-state marker.
    #[inline]
    pub fn as_dyn(self) -> StructType<'ctx, crate::struct_body_state::StructBodyDyn, B> {
        self.retag::<crate::struct_body_state::StructBodyDyn>()
    }

    /// Widen to the erased [`Type`] handle.
    #[inline]
    pub fn as_type(self) -> Type<'ctx, B> {
        Type {
            id: self.id,
            module: self.module,
        }
    }
}

impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand> sealed::Sealed
    for StructType<'ctx, Body, B>
{
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand + 'ctx> IrType<'ctx, B>
    for StructType<'ctx, Body, B>
{
    #[inline]
    fn as_type(self) -> Type<'ctx, B> {
        self.as_type()
    }
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand + 'ctx> fmt::Display
    for StructType<'ctx, Body, B>
{
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Type<'ctx, B> as fmt::Display>::fmt(&self.as_type(), f)
    }
}
impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand + 'ctx>
    From<StructType<'ctx, Body, B>> for Type<'ctx, B>
{
    #[inline]
    fn from(t: StructType<'ctx, Body, B>) -> Self {
        t.as_type()
    }
}
impl<'ctx, B: ModuleBrand> TryFrom<Type<'ctx, B>>
    for StructType<'ctx, crate::struct_body_state::StructBodyDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
        if matches!(t.data(), TypeData::Struct(_)) {
            Ok(Self {
                id: t.id(),
                module: t.module,
                _b: core::marker::PhantomData,
            })
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: t.kind_label(),
            })
        }
    }
}
decl_type_handle!(
    /// Fixed or scalable vector. Mirrors `VectorType` (`DerivedTypes.h`).
    VectorType, FixedVector,
    predicate |d| matches!(d, TypeData::FixedVector { .. } | TypeData::ScalableVector { .. })
);
decl_type_handle!(
    /// Function signature. Mirrors `FunctionType` (`DerivedTypes.h`).
    FunctionType, Function,
    predicate |d| matches!(d, TypeData::Function { .. })
);
decl_type_handle!(
    /// `label`. Mirrors `Type::getLabelTy`.
    LabelType, Label,
    predicate |d| matches!(d, TypeData::Label)
);
decl_type_handle!(
    /// `metadata`. Mirrors `Type::getMetadataTy`.
    MetadataType, Metadata,
    predicate |d| matches!(d, TypeData::Metadata)
);
decl_type_handle!(
    /// `token`. Mirrors `Type::getTokenTy`.
    TokenType, Token,
    predicate |d| matches!(d, TypeData::Token)
);
decl_type_handle!(
    /// Target extension type. Mirrors `TargetExtType` (`DerivedTypes.h`).
    TargetExtType, TargetExt,
    predicate |d| matches!(d, TypeData::TargetExt(_))
);

// --------------------------------------------------------------------------
// IntType — bit-width accessors
// --------------------------------------------------------------------------
// --------------------------------------------------------------------------
// IntType<'ctx, W> "" width-typed integer handle
// --------------------------------------------------------------------------

/// `iN` integer type. Mirrors `IntegerType` (`DerivedTypes.h`).
///
/// The `W: IntWidth` marker encodes the bit-width at the type level:
/// `IntType<'ctx, i32>` is a different type from `IntType<'ctx, i64>`,
/// and the IRBuilder's binary-op methods use this distinction to
/// reject mismatched widths at compile time.
///
/// Use [`IntType<'ctx, IntDyn>`](IntDyn) when the width
/// is only known at runtime (parsed `.ll`).
pub struct IntType<'ctx, W: IntWidth, B: ModuleBrand = crate::module::Brand<'ctx>> {
    pub(crate) id: TypeId,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) _w: PhantomData<W>,
}

// Manual derives "" `derive` would require `W: Trait` on the impls; manual
// versions avoid leaking that bound to consumers (`PhantomData<W>` is
// trivially `Copy`/`Eq`/`Hash` regardless).
impl<'ctx, W: IntWidth, B: ModuleBrand> Clone for IntType<'ctx, W, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> Copy for IntType<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand> PartialEq for IntType<'ctx, W, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> Eq for IntType<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand> Hash for IntType<'ctx, W, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> fmt::Debug for IntType<'ctx, W, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntType")
            .field("id", &self.id)
            .field("width", &W::static_bits())
            .finish()
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand> IntType<'ctx, W, B> {
    #[inline]
    pub(crate) fn new<M>(id: TypeId, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            _w: PhantomData,
        }
    }

    /// Widen to the erased [`Type`] handle.
    #[inline]
    pub fn as_type(self) -> Type<'ctx, B> {
        Type {
            id: self.id,
            module: self.module,
        }
    }

    /// Bit width of this integer type. For static widths this is
    /// equivalent to `W::static_bits().unwrap()`; for [`IntDyn`] it
    /// reads from the type-arena payload.
    #[inline]
    pub fn bit_width(self) -> u32 {
        self.module
            .type_data(self.id)
            .as_integer()
            .expect("IntType invariant: wraps Integer")
    }

    /// Erase the width marker, producing an [`IntDyn`]-tagged handle that
    /// preserves the runtime width but loses the static guarantee.
    #[inline]
    pub fn as_dyn(self) -> IntType<'ctx, IntDyn, B> {
        IntType {
            id: self.id,
            module: self.module,
            _w: PhantomData,
        }
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand> sealed::Sealed for IntType<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand> IrType<'ctx, B> for IntType<'ctx, W, B> {
    #[inline]
    fn as_type(self) -> Type<'ctx, B> {
        self.as_type()
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> fmt::Display for IntType<'ctx, W, B> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Type<'ctx, B> as fmt::Display>::fmt(&self.as_type(), f)
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand> From<IntType<'ctx, W, B>> for Type<'ctx, B> {
    #[inline]
    fn from(t: IntType<'ctx, W, B>) -> Self {
        t.as_type()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Type<'ctx, B>> for IntType<'ctx, IntDyn, B> {
    type Error = IrError;
    #[inline]
    fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
        if matches!(t.data(), TypeData::Integer { .. }) {
            Ok(Self::new(t.id(), t.module()))
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: t.kind_label(),
            })
        }
    }
}

/// Static narrowing: succeeds only if the runtime width matches
/// `W::static_bits()`.
macro_rules! impl_int_type_static_try_from {
    ($marker:ident, $bits:expr) => {
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Type<'ctx, B>> for IntType<'ctx, $marker, B> {
            type Error = IrError;
            fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
                match t.data() {
                    TypeData::Integer { bits } if *bits == $bits => {
                        Ok(Self::new(t.id(), t.module()))
                    }
                    TypeData::Integer { bits } => Err(IrError::OperandWidthMismatch {
                        lhs: $bits,
                        rhs: *bits,
                    }),
                    _ => Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::Integer,
                        got: t.kind_label(),
                    }),
                }
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<IntType<'ctx, IntDyn, B>>
            for IntType<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(t: IntType<'ctx, IntDyn, B>) -> IrResult<Self> {
                let bits = t.bit_width();
                if bits == $bits {
                    Ok(Self::new(t.id, t.module))
                } else {
                    Err(IrError::OperandWidthMismatch {
                        lhs: $bits,
                        rhs: bits,
                    })
                }
            }
        }
    };
}
impl_int_type_static_try_from!(bool, 1);
impl_int_type_static_try_from!(i8, 8);
impl_int_type_static_try_from!(i16, 16);
impl_int_type_static_try_from!(i32, 32);
impl_int_type_static_try_from!(i64, 64);
impl_int_type_static_try_from!(i128, 128);

/// Static -> `Dyn` widening (always succeeds).
macro_rules! impl_int_type_static_to_dyn {
    ($marker:ident) => {
        impl<'ctx> From<IntType<'ctx, $marker>> for IntType<'ctx, IntDyn> {
            #[inline]
            fn from(t: IntType<'ctx, $marker>) -> Self {
                t.as_dyn()
            }
        }
    };
}
impl_int_type_static_to_dyn!(bool);
impl_int_type_static_to_dyn!(i8);
impl_int_type_static_to_dyn!(i16);
impl_int_type_static_to_dyn!(i32);
impl_int_type_static_to_dyn!(i64);
impl_int_type_static_to_dyn!(i128);

// --------------------------------------------------------------------------
// FloatType<'ctx, K> "" kind-typed floating-point handle
// --------------------------------------------------------------------------

/// IEEE / non-IEEE floating-point type. Mirrors the union of the
/// `Type::isFloatingPointTy` arms.
///
/// The `K: FloatKind` marker encodes which kind at the type level.
/// Use [`FloatDyn`] when the kind is only known
/// at runtime.
pub struct FloatType<'ctx, K: FloatKind, B: ModuleBrand = crate::module::Brand<'ctx>> {
    pub(crate) id: TypeId,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) _k: PhantomData<K>,
}

impl<'ctx, K: FloatKind, B: ModuleBrand> Clone for FloatType<'ctx, K, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> Copy for FloatType<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand> PartialEq for FloatType<'ctx, K, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> Eq for FloatType<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand> Hash for FloatType<'ctx, K, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> fmt::Debug for FloatType<'ctx, K, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FloatType")
            .field("id", &self.id)
            .field("kind", &K::ieee_label())
            .finish()
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand> FloatType<'ctx, K, B> {
    #[inline]
    pub(crate) fn new<M>(id: TypeId, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            _k: PhantomData,
        }
    }

    /// Widen to the erased [`Type`] handle.
    #[inline]
    pub fn as_type(self) -> Type<'ctx, B> {
        Type {
            id: self.id,
            module: self.module,
        }
    }

    /// Erase the kind marker, producing a [`FloatDyn`]-tagged handle.
    #[inline]
    pub fn as_dyn(self) -> FloatType<'ctx, FloatDyn, B> {
        FloatType {
            id: self.id,
            module: self.module,
            _k: PhantomData,
        }
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand> sealed::Sealed for FloatType<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand> IrType<'ctx, B> for FloatType<'ctx, K, B> {
    #[inline]
    fn as_type(self) -> Type<'ctx, B> {
        self.as_type()
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> fmt::Display for FloatType<'ctx, K, B> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Type<'ctx, B> as fmt::Display>::fmt(&self.as_type(), f)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> From<FloatType<'ctx, K, B>> for Type<'ctx, B> {
    #[inline]
    fn from(t: FloatType<'ctx, K, B>) -> Self {
        t.as_type()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Type<'ctx, B>> for FloatType<'ctx, FloatDyn, B> {
    type Error = IrError;
    fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
        if matches!(
            t.data(),
            TypeData::Half
                | TypeData::BFloat
                | TypeData::Float
                | TypeData::Double
                | TypeData::X86Fp80
                | TypeData::Fp128
                | TypeData::PpcFp128
        ) {
            Ok(Self::new(t.id(), t.module()))
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: t.kind_label(),
            })
        }
    }
}

/// Static narrowing for FloatType.
macro_rules! impl_float_type_static_try_from {
    ($marker:ident, $variant:ident, $label:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Type<'ctx, B>> for FloatType<'ctx, $marker, B> {
            type Error = IrError;
            fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
                match t.data() {
                    TypeData::$variant => Ok(Self::new(t.id(), t.module())),
                    _ => Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::$label,
                        got: t.kind_label(),
                    }),
                }
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<FloatType<'ctx, FloatDyn, B>>
            for FloatType<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(t: FloatType<'ctx, FloatDyn, B>) -> IrResult<Self> {
                <Self as TryFrom<Type<'ctx, B>>>::try_from(t.as_type())
            }
        }
    };
}
impl_float_type_static_try_from!(Half, Half, Half);
impl_float_type_static_try_from!(BFloat, BFloat, BFloat);
impl_float_type_static_try_from!(f32, Float, Float);
impl_float_type_static_try_from!(f64, Double, Double);
impl_float_type_static_try_from!(Fp128, Fp128, Fp128);
impl_float_type_static_try_from!(X86Fp80, X86Fp80, X86Fp80);
impl_float_type_static_try_from!(PpcFp128, PpcFp128, PpcFp128);

macro_rules! impl_float_type_static_to_dyn {
    ($marker:ident) => {
        impl<'ctx> From<FloatType<'ctx, $marker>> for FloatType<'ctx, FloatDyn> {
            #[inline]
            fn from(t: FloatType<'ctx, $marker>) -> Self {
                t.as_dyn()
            }
        }
    };
}
impl_float_type_static_to_dyn!(Half);
impl_float_type_static_to_dyn!(BFloat);
impl_float_type_static_to_dyn!(f32);
impl_float_type_static_to_dyn!(f64);
impl_float_type_static_to_dyn!(Fp128);
impl_float_type_static_to_dyn!(X86Fp80);
impl_float_type_static_to_dyn!(PpcFp128);

// --------------------------------------------------------------------------
// PointerType — address-space accessor
// --------------------------------------------------------------------------

impl<'ctx, B: ModuleBrand> PointerType<'ctx, B> {
    /// Address space; `0` is the default flat space.
    #[inline]
    pub fn address_space(self) -> u32 {
        self.module
            .type_data(self.id)
            .as_pointer()
            .expect("PointerType invariant: wraps Pointer")
    }
}

// --------------------------------------------------------------------------
// ArrayType — element + length accessors
// --------------------------------------------------------------------------

impl<'ctx, B: ModuleBrand + 'ctx> ArrayType<'ctx, B> {
    #[inline]
    pub fn element(self) -> Type<'ctx, B> {
        let (elem, _) = self
            .module
            .type_data(self.id)
            .as_array()
            .expect("ArrayType invariant: wraps Array");
        Type::new(elem, self.module)
    }
    #[inline]
    pub fn len(self) -> u64 {
        let (_, n) = self
            .module
            .type_data(self.id)
            .as_array()
            .expect("ArrayType invariant: wraps Array");
        n
    }
    /// `true` for `[0 x T]` (LLVM allows zero-element arrays).
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

// --------------------------------------------------------------------------
// VectorType — element + length / scalability accessors
// --------------------------------------------------------------------------

impl<'ctx, B: ModuleBrand + 'ctx> VectorType<'ctx, B> {
    #[inline]
    pub fn element(self) -> Type<'ctx, B> {
        let (elem, _, _) = self
            .module
            .type_data(self.id)
            .as_vector()
            .expect("VectorType invariant: wraps a Vector");
        Type::new(elem, self.module)
    }
    /// Length: for fixed vectors, the exact lane count. For scalable
    /// vectors, the *minimum* lane count (the runtime value is a positive
    /// multiple of this).
    #[inline]
    pub fn min_len(self) -> u32 {
        let (_, n, _) = self
            .module
            .type_data(self.id)
            .as_vector()
            .expect("VectorType invariant: wraps a Vector");
        n
    }
    #[inline]
    pub fn is_scalable(self) -> bool {
        let (_, _, scalable) = self
            .module
            .type_data(self.id)
            .as_vector()
            .expect("VectorType invariant: wraps a Vector");
        scalable
    }
}

// --------------------------------------------------------------------------
// FunctionType — return / params / varargs
// --------------------------------------------------------------------------

impl<'ctx, B: ModuleBrand + 'ctx> FunctionType<'ctx, B> {
    #[inline]
    pub fn return_type(self) -> Type<'ctx, B> {
        let (ret, _, _) = self
            .module
            .type_data(self.id)
            .as_function()
            .expect("FunctionType invariant: wraps Function");
        Type::new(ret, self.module)
    }
    /// Iterator over parameter types in declaration order.
    pub fn params(self) -> impl ExactSizeIterator<Item = Type<'ctx, B>> + 'ctx {
        let (_, params, _) = self
            .module
            .type_data(self.id)
            .as_function()
            .expect("FunctionType invariant: wraps Function");
        let module = self.module;
        params.iter().map(move |id| Type::new(*id, module))
    }
    #[inline]
    pub fn is_var_arg(self) -> bool {
        let (_, _, va) = self
            .module
            .type_data(self.id)
            .as_function()
            .expect("FunctionType invariant: wraps Function");
        va
    }
}

// --------------------------------------------------------------------------
// StructType — name / packed / opacity / fields
// --------------------------------------------------------------------------

impl<'ctx, Body: crate::struct_body_state::StructBodyState, B: ModuleBrand + 'ctx>
    StructType<'ctx, Body, B>
{
    /// Name of an identified (named) struct, or `None` for literal
    /// structs.
    pub fn name(self) -> Option<&'ctx str> {
        self.module
            .type_data(self.id)
            .as_struct()
            .expect("StructType invariant: wraps Struct")
            .name
            .as_deref()
    }
    /// `true` for an *opaque* identified struct (body unset). Always
    /// `false` for literal structs.
    pub fn is_opaque(self) -> bool {
        self.module
            .type_data(self.id)
            .as_struct()
            .expect("StructType invariant: wraps Struct")
            .body
            .borrow()
            .is_none()
    }
    /// `true` for `<{ ... }>` packed structs. Returns `false` for opaque
    /// named structs (no body to inspect).
    pub fn is_packed(self) -> bool {
        self.module
            .type_data(self.id)
            .as_struct()
            .expect("StructType invariant: wraps Struct")
            .body
            .borrow()
            .as_ref()
            .is_some_and(|b| b.packed)
    }
    /// Number of element fields, or `0` for opaque structs.
    pub fn field_count(self) -> usize {
        self.module
            .type_data(self.id)
            .as_struct()
            .expect("StructType invariant: wraps Struct")
            .body
            .borrow()
            .as_ref()
            .map(|b| b.elements.len())
            .unwrap_or(0)
    }
    /// Field type at `index`, or `None` if out of bounds (or opaque).
    pub fn field_type(self, index: usize) -> Option<Type<'ctx, B>> {
        let s = self
            .module
            .type_data(self.id)
            .as_struct()
            .expect("StructType invariant: wraps Struct");
        s.body
            .borrow()
            .as_ref()
            .and_then(|b| b.elements.get(index).copied())
            .map(|id| Type::new(id, self.module))
    }
}

// --------------------------------------------------------------------------
// TargetExtType — accessors
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetExtProperty {
    HasZeroInit,
    CanBeGlobal,
    CanBeLocal,
    CanBeVectorElement,
    IsTokenLike,
}

impl<'ctx, B: ModuleBrand + 'ctx> TargetExtType<'ctx, B> {
    pub fn name(self) -> &'ctx str {
        self.module
            .type_data(self.id)
            .as_target_ext()
            .expect("TargetExtType invariant: wraps TargetExt")
            .name
            .as_str()
    }
    pub fn type_params(self) -> impl ExactSizeIterator<Item = Type<'ctx, B>> + 'ctx {
        let t = self
            .module
            .type_data(self.id)
            .as_target_ext()
            .expect("TargetExtType invariant: wraps TargetExt");
        let module = self.module;
        t.type_params.iter().map(move |id| Type::new(*id, module))
    }
    pub fn int_params(self) -> impl ExactSizeIterator<Item = u32> + 'ctx {
        let t = self
            .module
            .type_data(self.id)
            .as_target_ext()
            .expect("TargetExtType invariant: wraps TargetExt");
        t.int_params.iter().copied()
    }
    pub fn has_property(self, property: TargetExtProperty) -> bool {
        let name = self.name();
        match name {
            "spirv.Image" | "spirv.SignedImage" | "spirv.Type" => {
                matches!(
                    property,
                    TargetExtProperty::CanBeGlobal | TargetExtProperty::CanBeLocal
                )
            }
            "spirv.IntegralConstant" | "spirv.Literal" => false,
            "spirv.Padding" => matches!(property, TargetExtProperty::CanBeGlobal),
            "aarch64.svcount" | "riscv.vector.tuple" => {
                matches!(
                    property,
                    TargetExtProperty::HasZeroInit | TargetExtProperty::CanBeLocal
                )
            }
            "dx.Padding" | "amdgcn.named.barrier" => {
                matches!(property, TargetExtProperty::CanBeGlobal)
            }
            "llvm.test.vectorelement" => {
                matches!(
                    property,
                    TargetExtProperty::CanBeLocal | TargetExtProperty::CanBeVectorElement
                )
            }
            _ if name.starts_with("spirv.") => {
                matches!(
                    property,
                    TargetExtProperty::HasZeroInit
                        | TargetExtProperty::CanBeGlobal
                        | TargetExtProperty::CanBeLocal
                )
            }
            _ if name.starts_with("dx.") => {
                matches!(
                    property,
                    TargetExtProperty::CanBeGlobal
                        | TargetExtProperty::CanBeLocal
                        | TargetExtProperty::IsTokenLike
                )
            }
            _ => false,
        }
    }
}

// --------------------------------------------------------------------------
// AnyTypeEnum — exhaustive widening
// --------------------------------------------------------------------------

/// Exhaustive enum over every type kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnyTypeEnum<'ctx> {
    Void(VoidType<'ctx>),
    Int(IntType<'ctx, IntDyn>),
    Float(FloatType<'ctx, FloatDyn>),
    Pointer(PointerType<'ctx>),
    Array(ArrayType<'ctx>),
    Struct(StructType<'ctx>),
    Vector(VectorType<'ctx>),
    Function(FunctionType<'ctx>),
    Label(LabelType<'ctx>),
    Metadata(MetadataType<'ctx>),
    Token(TokenType<'ctx>),
    TargetExt(TargetExtType<'ctx>),
}

impl<'ctx> AnyTypeEnum<'ctx> {
    pub fn as_type(self) -> Type<'ctx> {
        match self {
            Self::Void(t) => t.as_type(),
            Self::Int(t) => t.as_type(),
            Self::Float(t) => t.as_type(),
            Self::Pointer(t) => t.as_type(),
            Self::Array(t) => t.as_type(),
            Self::Struct(t) => t.as_type(),
            Self::Vector(t) => t.as_type(),
            Self::Function(t) => t.as_type(),
            Self::Label(t) => t.as_type(),
            Self::Metadata(t) => t.as_type(),
            Self::Token(t) => t.as_type(),
            Self::TargetExt(t) => t.as_type(),
        }
    }
}

impl<'ctx> From<Type<'ctx>> for AnyTypeEnum<'ctx> {
    fn from(t: Type<'ctx>) -> Self {
        let m = t.module();
        match t.kind() {
            TypeKind::Void => Self::Void(VoidType::new(t.id(), m)),
            TypeKind::Integer { .. } => Self::Int(IntType::new(t.id(), m)),
            TypeKind::Half
            | TypeKind::BFloat
            | TypeKind::Float
            | TypeKind::Double
            | TypeKind::X86Fp80
            | TypeKind::Fp128
            | TypeKind::PpcFp128 => Self::Float(FloatType::new(t.id(), m)),
            TypeKind::Pointer { .. } => Self::Pointer(PointerType::new(t.id(), m)),
            TypeKind::Array => Self::Array(ArrayType::new(t.id(), m)),
            TypeKind::Struct => Self::Struct(StructType::new(t.id(), m)),
            TypeKind::FixedVector | TypeKind::ScalableVector => {
                Self::Vector(VectorType::new(t.id(), m))
            }
            TypeKind::Function => Self::Function(FunctionType::new(t.id(), m)),
            TypeKind::Label => Self::Label(LabelType::new(t.id(), m)),
            TypeKind::Metadata => Self::Metadata(MetadataType::new(t.id(), m)),
            TypeKind::Token => Self::Token(TokenType::new(t.id(), m)),
            // X86_AMX has no dedicated handle in DerivedTypes.h; route
            // through the catch-all TargetExt slot for now. A future
            // revision may add `X86AmxType`.
            TypeKind::X86Amx | TypeKind::TargetExt => {
                Self::TargetExt(TargetExtType::new(t.id(), m))
            }
            TypeKind::TypedPointer => Self::Pointer(PointerType::new(t.id(), m)),
        }
    }
}

impl<'ctx> fmt::Display for AnyTypeEnum<'ctx> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_type().fmt(f)
    }
}

// --------------------------------------------------------------------------
// Refinements
// --------------------------------------------------------------------------

/// Types you can `alloca` / `load` / `store` / GEP through. Mirrors the
/// `Type::isSized` predicate, with the additional invariant - encoded
/// in the type system - that any `SizedType` you hold is provably
/// sized: methods that require sizedness can take it directly without
/// runtime checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SizedType<'ctx, B: ModuleBrand = crate::module::Brand<'ctx>>(pub(crate) Type<'ctx, B>);

impl<'ctx, B: ModuleBrand> SizedType<'ctx, B> {
    #[inline]
    pub fn as_type(self) -> Type<'ctx, B> {
        self.0
    }
}

impl<'ctx, B: ModuleBrand> From<SizedType<'ctx, B>> for Type<'ctx, B> {
    #[inline]
    fn from(s: SizedType<'ctx, B>) -> Self {
        s.0
    }
}

impl<'ctx, B: ModuleBrand> TryFrom<Type<'ctx, B>> for SizedType<'ctx, B> {
    type Error = IrError;
    fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
        if t.is_sized() {
            Ok(Self(t))
        } else {
            Err(IrError::UnsizedType {
                kind: t.kind_label(),
            })
        }
    }
}

impl<'ctx, B: ModuleBrand> fmt::Display for SizedType<'ctx, B> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// First-class types that may carry an SSA value: integer / float /
/// pointer / array / struct / vector. Mirrors LLVM's "basic" type group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasicTypeEnum<'ctx> {
    Int(IntType<'ctx, IntDyn>),
    Float(FloatType<'ctx, FloatDyn>),
    Pointer(PointerType<'ctx>),
    Array(ArrayType<'ctx>),
    Struct(StructType<'ctx>),
    Vector(VectorType<'ctx>),
}

impl<'ctx> BasicTypeEnum<'ctx> {
    pub fn as_type(self) -> Type<'ctx> {
        match self {
            Self::Int(t) => t.as_type(),
            Self::Float(t) => t.as_type(),
            Self::Pointer(t) => t.as_type(),
            Self::Array(t) => t.as_type(),
            Self::Struct(t) => t.as_type(),
            Self::Vector(t) => t.as_type(),
        }
    }
}

impl<'ctx> From<BasicTypeEnum<'ctx>> for Type<'ctx> {
    #[inline]
    fn from(b: BasicTypeEnum<'ctx>) -> Self {
        b.as_type()
    }
}

impl<'ctx> TryFrom<Type<'ctx>> for BasicTypeEnum<'ctx> {
    type Error = IrError;
    fn try_from(t: Type<'ctx>) -> IrResult<Self> {
        let m = t.module();
        Ok(match t.kind() {
            TypeKind::Integer { .. } => Self::Int(IntType::new(t.id(), m)),
            TypeKind::Half
            | TypeKind::BFloat
            | TypeKind::Float
            | TypeKind::Double
            | TypeKind::X86Fp80
            | TypeKind::Fp128
            | TypeKind::PpcFp128 => Self::Float(FloatType::new(t.id(), m)),
            TypeKind::Pointer { .. } => Self::Pointer(PointerType::new(t.id(), m)),
            TypeKind::Array => Self::Array(ArrayType::new(t.id(), m)),
            TypeKind::Struct => Self::Struct(StructType::new(t.id(), m)),
            TypeKind::FixedVector | TypeKind::ScalableVector => {
                Self::Vector(VectorType::new(t.id(), m))
            }
            _ => {
                return Err(IrError::TypeMismatch {
                    expected: TypeKindLabel::Integer,
                    got: t.kind_label(),
                });
            }
        })
    }
}

impl<'ctx> fmt::Display for BasicTypeEnum<'ctx> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_type().fmt(f)
    }
}

/// Basic + metadata. Used for the typing of variadic intrinsics whose
/// arguments may include `metadata` slots (e.g. `@llvm.dbg.value`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasicMetadataTypeEnum<'ctx> {
    Int(IntType<'ctx, IntDyn>),
    Float(FloatType<'ctx, FloatDyn>),
    Pointer(PointerType<'ctx>),
    Array(ArrayType<'ctx>),
    Struct(StructType<'ctx>),
    Vector(VectorType<'ctx>),
    Metadata(MetadataType<'ctx>),
}

impl<'ctx> BasicMetadataTypeEnum<'ctx> {
    pub fn as_type(self) -> Type<'ctx> {
        match self {
            Self::Int(t) => t.as_type(),
            Self::Float(t) => t.as_type(),
            Self::Pointer(t) => t.as_type(),
            Self::Array(t) => t.as_type(),
            Self::Struct(t) => t.as_type(),
            Self::Vector(t) => t.as_type(),
            Self::Metadata(t) => t.as_type(),
        }
    }
}

impl<'ctx> From<BasicTypeEnum<'ctx>> for BasicMetadataTypeEnum<'ctx> {
    fn from(b: BasicTypeEnum<'ctx>) -> Self {
        match b {
            BasicTypeEnum::Int(t) => Self::Int(t),
            BasicTypeEnum::Float(t) => Self::Float(t),
            BasicTypeEnum::Pointer(t) => Self::Pointer(t),
            BasicTypeEnum::Array(t) => Self::Array(t),
            BasicTypeEnum::Struct(t) => Self::Struct(t),
            BasicTypeEnum::Vector(t) => Self::Vector(t),
        }
    }
}

impl<'ctx> TryFrom<Type<'ctx>> for BasicMetadataTypeEnum<'ctx> {
    type Error = IrError;
    fn try_from(t: Type<'ctx>) -> IrResult<Self> {
        if t.is_metadata() {
            return Ok(Self::Metadata(MetadataType::new(t.id(), t.module())));
        }
        BasicTypeEnum::try_from(t).map(Self::from)
    }
}

impl<'ctx> From<BasicMetadataTypeEnum<'ctx>> for Type<'ctx> {
    #[inline]
    fn from(b: BasicMetadataTypeEnum<'ctx>) -> Self {
        b.as_type()
    }
}

impl<'ctx> fmt::Display for BasicMetadataTypeEnum<'ctx> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_type().fmt(f)
    }
}

/// Aggregate marker - array or struct. Vectors are deliberately excluded
/// so `extractvalue` / `insertvalue` cannot accept a vector source
/// (matches `Type.h` + LangRef).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateType<'ctx> {
    Array(ArrayType<'ctx>),
    Struct(StructType<'ctx>),
}

impl<'ctx> AggregateType<'ctx> {
    pub fn as_type(self) -> Type<'ctx> {
        match self {
            Self::Array(t) => t.as_type(),
            Self::Struct(t) => t.as_type(),
        }
    }
}

impl<'ctx> From<AggregateType<'ctx>> for Type<'ctx> {
    #[inline]
    fn from(a: AggregateType<'ctx>) -> Self {
        a.as_type()
    }
}

impl<'ctx> TryFrom<Type<'ctx>> for AggregateType<'ctx> {
    type Error = IrError;
    fn try_from(t: Type<'ctx>) -> IrResult<Self> {
        let m = t.module();
        match t.kind() {
            TypeKind::Array => Ok(Self::Array(ArrayType::new(t.id(), m))),
            TypeKind::Struct => Ok(Self::Struct(StructType::new(t.id(), m))),
            _ => Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Array,
                got: t.kind_label(),
            }),
        }
    }
}

impl<'ctx> fmt::Display for AggregateType<'ctx> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_type().fmt(f)
    }
}
