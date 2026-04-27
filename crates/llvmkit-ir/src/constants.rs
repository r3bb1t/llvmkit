//! Per-kind constant refinement handles plus their constructors.
//! Mirrors a slice of `llvm/include/llvm/IR/Constants.h`.
//!
//! Phase B continued: `ConstantInt`, `ConstantFP`,
//! `ConstantPointerNull`, `ConstantArray`/`Struct`/`Vector` (under one
//! [`ConstantAggregate`] handle), [`UndefValue`], [`PoisonValue`].
//!
//! The richer shapes (`ConstantExpr`, `BlockAddress`, `TokenNone`,
//! `PtrAuth`, `DSOLocalEquivalent`, `NoCFIValue`, target-extension
//! `none`) are scheduled per the foundation plan as their own
//! sessions.
//!
//! Constructors live as **methods on the matching type-handle**, so
//! readers who already have an `IntType<'ctx>` write
//! `i32.const_int(42, false)?` instead of `Module::const_int(i32, 42,
//! ...)` — same shape inkwell uses.
//!
//! Every constant goes through the per-kind interning maps maintained
//! by the owning [`Module`](crate::Module)'s internal context, so structurally-equal
//! constants in the same module share a single value-id. That mirrors
//! LLVM's pointer-identity-after-uniquing semantics with no
//! pointer-based identity in our own code.

use crate::constant::{Constant, ConstantData, IsConstant};
use crate::derived_types::{ArrayType, FloatType, IntType, PointerType, StructType, VectorType};
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::module::ModuleRef;
use crate::r#type::{Type, TypeData, TypeId};
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};
use crate::{DebugLoc, MAX_INT_BITS};
use core::convert::Infallible;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use crate::float_kind::{BFloat, FloatDyn, FloatKind, Fp128, Half, PpcFp128, X86Fp80};
use crate::int_width::IntoConstantInt;
use crate::int_width::{IntDyn, IntWidth};

// --------------------------------------------------------------------------
// Per-kind handles
// --------------------------------------------------------------------------

/// Internal helper: build a per-kind constant refinement handle.
macro_rules! decl_constant_handle {
    (
        $(#[$attr:meta])*
        $name:ident,
        $type_label:ident,
        type_predicate $pred:expr
    ) => {
        $(#[$attr])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name<'ctx> {
            pub(crate) id: ValueId,
            pub(crate) module: ModuleRef<'ctx>,
            pub(crate) ty: TypeId,
        }

        impl<'ctx> $name<'ctx> {
            #[inline]
            pub(crate) fn from_parts(c: Constant<'ctx>) -> Self {
                Self { id: c.id, module: c.module, ty: c.ty }
            }

            /// Widen to the erased [`Constant`] handle.
            #[inline]
            pub fn as_constant(self) -> Constant<'ctx> {
                Constant { id: self.id, module: self.module, ty: self.ty }
            }

            /// Widen to the erased [`Value`] handle.
            #[inline]
            pub fn as_value(self) -> Value<'ctx> {
                Value { id: self.id, module: self.module, ty: self.ty }
            }
        }

        impl<'ctx> sealed::Sealed for $name<'ctx> {}
        impl<'ctx> IsValue<'ctx> for $name<'ctx> {
            #[inline]
            fn as_value(self) -> Value<'ctx> { Self::as_value(self) }
        }
        impl<'ctx> IsConstant<'ctx> for $name<'ctx> {
            #[inline]
            fn as_constant(self) -> Constant<'ctx> { Self::as_constant(self) }
        }
        impl<'ctx> Typed<'ctx> for $name<'ctx> {
            #[inline]
            fn ty(self) -> Type<'ctx> {
                Type::new(self.ty, self.module.module())
            }
        }
        impl<'ctx> HasName<'ctx> for $name<'ctx> {
            #[inline]
            fn name(self) -> Option<String> { self.as_value().name() }
            #[inline]
            fn set_name(self, name: Option<&str>) { self.as_value().set_name(name); }
        }
        impl<'ctx> HasDebugLoc for $name<'ctx> {
            #[inline]
            fn debug_loc(self) -> Option<DebugLoc> { self.as_value().debug_loc() }
        }

        impl<'ctx> From<$name<'ctx>> for Constant<'ctx> {
            #[inline]
            fn from(c: $name<'ctx>) -> Self { c.as_constant() }
        }
        impl<'ctx> From<$name<'ctx>> for Value<'ctx> {
            #[inline]
            fn from(c: $name<'ctx>) -> Self { c.as_value() }
        }

        impl<'ctx> TryFrom<Constant<'ctx>> for $name<'ctx> {
            type Error = IrError;
            fn try_from(c: Constant<'ctx>) -> IrResult<Self> {
                let pred: fn(&TypeData) -> bool = $pred;
                let ty = c.ty();
                if pred(ty.data()) {
                    Ok(Self::from_parts(c))
                } else {
                    Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::$type_label,
                        got: ty.kind_label(),
                    })
                }
            }
        }
    };
}

// `ConstantIntValue<'ctx, W>` and `ConstantFloatValue<'ctx, K>` are
// hand-written below to carry their width/kind markers.
decl_constant_handle!(
    /// `null` constant for a pointer type.
    ConstantPointerNull, Pointer,
    type_predicate |d| matches!(d, TypeData::Pointer { .. })
);
decl_constant_handle!(
    /// `undef <type>` marker. Mirrors `UndefValue`.
    UndefValue, Integer,  // type-label is overridden in the constructor
    type_predicate |_| true
);
decl_constant_handle!(
    /// `poison <type>` marker. Mirrors `PoisonValue`.
    PoisonValue, Integer,  // type-label is overridden in the constructor
    type_predicate |_| true
);
decl_constant_handle!(
    /// Aggregate constant (`ConstantArray` / `ConstantStruct` /
    /// `ConstantVector`).
    ConstantAggregate, Array,
    type_predicate |d| matches!(
        d,
        TypeData::Array { .. } | TypeData::Struct(_)
            | TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }
    )
);

// --------------------------------------------------------------------------
// ConstantIntValue<'ctx, W> -- width-typed integer constant
// --------------------------------------------------------------------------

/// Integer constant of width `W`.
pub struct ConstantIntValue<'ctx, W: IntWidth> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    pub(crate) _w: PhantomData<W>,
}

impl<'ctx, W: IntWidth> Clone for ConstantIntValue<'ctx, W> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth> Copy for ConstantIntValue<'ctx, W> {}
impl<'ctx, W: IntWidth> PartialEq for ConstantIntValue<'ctx, W> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth> Eq for ConstantIntValue<'ctx, W> {}
impl<'ctx, W: IntWidth> Hash for ConstantIntValue<'ctx, W> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, W: IntWidth> fmt::Debug for ConstantIntValue<'ctx, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConstantIntValue")
            .field("id", &self.id)
            .field("width", &W::static_bits())
            .finish()
    }
}

impl<'ctx, W: IntWidth> ConstantIntValue<'ctx, W> {
    #[inline]
    pub(crate) fn from_parts_typed(c: Constant<'ctx>) -> Self {
        Self {
            id: c.id,
            module: c.module,
            ty: c.ty,
            _w: PhantomData,
        }
    }
    #[inline]
    pub fn as_constant(self) -> Constant<'ctx> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn as_value(self) -> Value<'ctx> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    /// Erase the width marker.
    #[inline]
    pub fn as_dyn(self) -> ConstantIntValue<'ctx, IntDyn> {
        ConstantIntValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _w: PhantomData,
        }
    }
}

impl<'ctx, W: IntWidth> sealed::Sealed for ConstantIntValue<'ctx, W> {}
impl<'ctx, W: IntWidth> IsValue<'ctx> for ConstantIntValue<'ctx, W> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        Self::as_value(self)
    }
}
impl<'ctx, W: IntWidth> IsConstant<'ctx> for ConstantIntValue<'ctx, W> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx> {
        Self::as_constant(self)
    }
}
impl<'ctx, W: IntWidth> Typed<'ctx> for ConstantIntValue<'ctx, W> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}
impl<'ctx, W: IntWidth> HasName<'ctx> for ConstantIntValue<'ctx, W> {
    fn name(self) -> Option<String> {
        self.as_value().name()
    }
    fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }
}
impl<'ctx, W: IntWidth> HasDebugLoc for ConstantIntValue<'ctx, W> {
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}
impl<'ctx, W: IntWidth> From<ConstantIntValue<'ctx, W>> for Constant<'ctx> {
    #[inline]
    fn from(c: ConstantIntValue<'ctx, W>) -> Self {
        c.as_constant()
    }
}
impl<'ctx, W: IntWidth> From<ConstantIntValue<'ctx, W>> for Value<'ctx> {
    #[inline]
    fn from(c: ConstantIntValue<'ctx, W>) -> Self {
        c.as_value()
    }
}
impl<'ctx> TryFrom<Constant<'ctx>> for ConstantIntValue<'ctx, IntDyn> {
    type Error = IrError;
    fn try_from(c: Constant<'ctx>) -> IrResult<Self> {
        let ty = c.ty();
        if matches!(ty.data(), TypeData::Integer { .. }) {
            Ok(Self::from_parts_typed(c))
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: ty.kind_label(),
            })
        }
    }
}

macro_rules! impl_constant_int_static_try_from {
    ($marker:ident, $bits:expr) => {
        impl<'ctx> TryFrom<Constant<'ctx>> for ConstantIntValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(c: Constant<'ctx>) -> IrResult<Self> {
                let ty = c.ty();
                match ty.data() {
                    TypeData::Integer { bits } if *bits == $bits => Ok(Self::from_parts_typed(c)),
                    TypeData::Integer { bits } => Err(IrError::OperandWidthMismatch {
                        lhs: $bits,
                        rhs: *bits,
                    }),
                    _ => Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::Integer,
                        got: ty.kind_label(),
                    }),
                }
            }
        }
        impl<'ctx> From<ConstantIntValue<'ctx, $marker>> for ConstantIntValue<'ctx, IntDyn> {
            #[inline]
            fn from(c: ConstantIntValue<'ctx, $marker>) -> Self {
                c.as_dyn()
            }
        }
    };
}
impl_constant_int_static_try_from!(bool, 1);
impl_constant_int_static_try_from!(i8, 8);
impl_constant_int_static_try_from!(i16, 16);
impl_constant_int_static_try_from!(i32, 32);
impl_constant_int_static_try_from!(i64, 64);
impl_constant_int_static_try_from!(i128, 128);

// --------------------------------------------------------------------------
// ConstantFloatValue<'ctx, K> -- kind-typed floating-point constant
// --------------------------------------------------------------------------

/// Floating-point constant of kind `K`.
pub struct ConstantFloatValue<'ctx, K: FloatKind> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    pub(crate) _k: PhantomData<K>,
}

impl<'ctx, K: FloatKind> Clone for ConstantFloatValue<'ctx, K> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind> Copy for ConstantFloatValue<'ctx, K> {}
impl<'ctx, K: FloatKind> PartialEq for ConstantFloatValue<'ctx, K> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind> Eq for ConstantFloatValue<'ctx, K> {}
impl<'ctx, K: FloatKind> Hash for ConstantFloatValue<'ctx, K> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, K: FloatKind> fmt::Debug for ConstantFloatValue<'ctx, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConstantFloatValue")
            .field("id", &self.id)
            .field("kind", &K::ieee_label())
            .finish()
    }
}

impl<'ctx, K: FloatKind> ConstantFloatValue<'ctx, K> {
    #[inline]
    pub(crate) fn from_parts_typed(c: Constant<'ctx>) -> Self {
        Self {
            id: c.id,
            module: c.module,
            ty: c.ty,
            _k: PhantomData,
        }
    }
    #[inline]
    pub fn as_constant(self) -> Constant<'ctx> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn as_value(self) -> Value<'ctx> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn as_dyn(self) -> ConstantFloatValue<'ctx, FloatDyn> {
        ConstantFloatValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _k: PhantomData,
        }
    }
}

impl<'ctx, K: FloatKind> sealed::Sealed for ConstantFloatValue<'ctx, K> {}
impl<'ctx, K: FloatKind> IsValue<'ctx> for ConstantFloatValue<'ctx, K> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        Self::as_value(self)
    }
}
impl<'ctx, K: FloatKind> IsConstant<'ctx> for ConstantFloatValue<'ctx, K> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx> {
        Self::as_constant(self)
    }
}
impl<'ctx, K: FloatKind> Typed<'ctx> for ConstantFloatValue<'ctx, K> {
    fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}
impl<'ctx, K: FloatKind> HasName<'ctx> for ConstantFloatValue<'ctx, K> {
    fn name(self) -> Option<String> {
        self.as_value().name()
    }
    fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }
}
impl<'ctx, K: FloatKind> HasDebugLoc for ConstantFloatValue<'ctx, K> {
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}
impl<'ctx, K: FloatKind> From<ConstantFloatValue<'ctx, K>> for Constant<'ctx> {
    fn from(c: ConstantFloatValue<'ctx, K>) -> Self {
        c.as_constant()
    }
}
impl<'ctx, K: FloatKind> From<ConstantFloatValue<'ctx, K>> for Value<'ctx> {
    fn from(c: ConstantFloatValue<'ctx, K>) -> Self {
        c.as_value()
    }
}
impl<'ctx> TryFrom<Constant<'ctx>> for ConstantFloatValue<'ctx, FloatDyn> {
    type Error = IrError;
    fn try_from(c: Constant<'ctx>) -> IrResult<Self> {
        let ty = c.ty();
        if matches!(
            ty.data(),
            TypeData::Half
                | TypeData::BFloat
                | TypeData::Float
                | TypeData::Double
                | TypeData::X86Fp80
                | TypeData::Fp128
                | TypeData::PpcFp128
        ) {
            Ok(Self::from_parts_typed(c))
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: ty.kind_label(),
            })
        }
    }
}

macro_rules! impl_constant_float_static_try_from {
    ($marker:ident, $variant:ident, $label:ident) => {
        impl<'ctx> TryFrom<Constant<'ctx>> for ConstantFloatValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(c: Constant<'ctx>) -> IrResult<Self> {
                let ty = c.ty();
                match ty.data() {
                    TypeData::$variant => Ok(Self::from_parts_typed(c)),
                    _ => Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::$label,
                        got: ty.kind_label(),
                    }),
                }
            }
        }
        impl<'ctx> From<ConstantFloatValue<'ctx, $marker>> for ConstantFloatValue<'ctx, FloatDyn> {
            #[inline]
            fn from(c: ConstantFloatValue<'ctx, $marker>) -> Self {
                c.as_dyn()
            }
        }
    };
}
impl_constant_float_static_try_from!(Half, Half, Half);
impl_constant_float_static_try_from!(BFloat, BFloat, BFloat);
impl_constant_float_static_try_from!(f32, Float, Float);
impl_constant_float_static_try_from!(f64, Double, Double);
impl_constant_float_static_try_from!(Fp128, Fp128, Fp128);
impl_constant_float_static_try_from!(X86Fp80, X86Fp80, X86Fp80);
impl_constant_float_static_try_from!(PpcFp128, PpcFp128, PpcFp128);

// --------------------------------------------------------------------------
// IntType: integer-constant constructors
// --------------------------------------------------------------------------

impl<'ctx, W: IntWidth> IntType<'ctx, W> {
    /// Construct an integer constant from raw 64-bit input. Mirrors
    /// `ConstantInt::get` with an explicit `sign_extend` flag.
    ///
    /// The numeric `value` is interpreted as either zero-extended
    /// (`sign_extend = false`) or sign-extended (`sign_extend = true`)
    /// to the integer's bit-width, then truncated. Returns
    /// `Err(IrError::ImmediateOverflow)` if `value` does not fit
    /// losslessly under the chosen extension scheme.
    ///
    /// Most callers should prefer the type-driven [`Self::const_int`]
    /// helper, which derives the extension mode from the Rust input
    /// type's signedness.
    pub fn const_int_raw(
        self,
        value: u64,
        sign_extend: bool,
    ) -> IrResult<ConstantIntValue<'ctx, W>> {
        let bits = self.bit_width();
        let storage = encode_int_value(value, bits, sign_extend)?;
        Ok(intern_int_constant(self, storage))
    }

    /// Construct an integer constant. The Rust input type drives the
    /// extension choice (signed Rust ints sign-extend, unsigned ints
    /// zero-extend, `bool` becomes `i1` true/false).
    ///
    /// For widths the input type fits losslessly into, the call is
    /// infallible. For narrowing inputs (e.g. `i64 -> i32`), use
    /// [`Self::const_int_checked`] or call this on a wider target.
    pub fn const_int<V>(self, v: V) -> ConstantIntValue<'ctx, W>
    where
        V: IntoConstantInt<'ctx, W, Error = Infallible>,
    {
        match v.into_constant_int(self) {
            Ok(c) => c,
            Err(_e) => unreachable!("Infallible cannot be constructed"),
        }
    }

    /// Fallible variant for narrowing / dynamic-width targets.
    pub fn const_int_checked<V>(self, v: V) -> IrResult<ConstantIntValue<'ctx, W>>
    where
        V: IntoConstantInt<'ctx, W, Error = IrError>,
    {
        v.into_constant_int(self)
    }

    /// Construct an integer constant from a precomputed
    /// little-endian-words magnitude. Mirrors
    /// `ConstantInt::get(IntegerType*, ArrayRef<uint64_t>)`.
    pub fn const_int_arbitrary_precision(
        self,
        words: &[u64],
    ) -> IrResult<ConstantIntValue<'ctx, W>> {
        let bits = self.bit_width();
        let bits_used = bits_used_in_words(words);
        if bits_used > bits {
            return Err(IrError::ImmediateOverflow {
                value: u128::MAX,
                bits,
            });
        }
        Ok(intern_int_constant(self, normalise_words(words)))
    }

    /// `iN 0`. Mirrors `Constant::getNullValue(IntegerType*)`.
    pub fn const_zero(self) -> ConstantIntValue<'ctx, W> {
        intern_int_constant(self, Box::<[u64]>::from([]))
    }

    /// `iN -1` (all-ones). Mirrors `Constant::getAllOnesValue`.
    pub fn const_all_ones(self) -> ConstantIntValue<'ctx, W> {
        let bits = self.bit_width();
        let bits_usize =
            usize::try_from(bits).unwrap_or_else(|_| unreachable!("u32 bit-width fits in usize"));
        let limbs = bits_usize.div_ceil(64);
        let mut words = vec![u64::MAX; limbs].into_boxed_slice();
        if let Some(top) = words.last_mut() {
            let unused = limbs * 64 - bits_usize;
            if unused > 0 {
                let unused_u32 = u32::try_from(unused)
                    .unwrap_or_else(|_| unreachable!("unused bits fit in u32"));
                *top &= u64::MAX.checked_shr(unused_u32).unwrap_or(0);
            }
        }
        intern_int_constant(self, normalise_words(&words))
    }
}

impl<'ctx, W: IntWidth> ConstantIntValue<'ctx, W> {
    #[inline]
    pub fn ty(self) -> IntType<'ctx, W> {
        IntType::new(self.ty, self.module.module())
    }

    #[inline]
    pub fn bit_width(self) -> u32 {
        self.ty().bit_width()
    }

    pub fn words(self) -> &'ctx [u64] {
        match &self.as_value().data().kind {
            ValueKindData::Constant(ConstantData::Int(words)) => words,
            _ => unreachable!("ConstantIntValue invariant: kind is Constant::Int"),
        }
    }

    pub fn value_zext_u128(self) -> Option<u128> {
        let w = self.words();
        match w.len() {
            0 => Some(0),
            1 => Some(u128::from(w[0])),
            2 => Some(u128::from(w[0]) | (u128::from(w[1]) << 64)),
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------
// FloatType: float-constant constructors
// --------------------------------------------------------------------------

impl<'ctx> FloatType<'ctx, f64> {
    /// Construct a `double` constant from an `f64`. Infallible.
    pub fn const_double(self, value: f64) -> ConstantFloatValue<'ctx, f64> {
        intern_float_constant(self, u128::from(value.to_bits()))
    }
}

impl<'ctx> FloatType<'ctx, f32> {
    /// Construct a `float` constant from an `f32`. Infallible.
    pub fn const_float(self, value: f32) -> ConstantFloatValue<'ctx, f32> {
        intern_float_constant(self, u128::from(value.to_bits()))
    }
}

impl<'ctx, K: FloatKind> FloatType<'ctx, K> {
    /// Construct a float constant directly from its bit pattern. Width
    /// of the pattern is implied by the kind.
    pub fn const_from_bits(self, bits: u128) -> ConstantFloatValue<'ctx, K> {
        intern_float_constant(self, bits)
    }
}

impl<'ctx, K: FloatKind> ConstantFloatValue<'ctx, K> {
    #[inline]
    pub fn ty(self) -> FloatType<'ctx, K> {
        FloatType::new(self.ty, self.module.module())
    }

    pub fn bit_pattern(self) -> u128 {
        match &self.as_value().data().kind {
            ValueKindData::Constant(ConstantData::Float(b)) => *b,
            _ => unreachable!("ConstantFloatValue invariant: kind is Constant::Float"),
        }
    }
}

// --------------------------------------------------------------------------
// PointerType: null
// --------------------------------------------------------------------------

impl<'ctx> PointerType<'ctx> {
    /// `ptr null`. Mirrors `ConstantPointerNull::get`.
    pub fn const_null(self) -> ConstantPointerNull<'ctx> {
        intern_pointer_null(self)
    }

    /// Same as [`Self::const_null`]; mirrors inkwell's `const_zero`.
    #[inline]
    pub fn const_zero(self) -> ConstantPointerNull<'ctx> {
        self.const_null()
    }
}

// --------------------------------------------------------------------------
// Aggregate constructors
// --------------------------------------------------------------------------

impl<'ctx> ArrayType<'ctx> {
    /// `[N x T] [...]`. Each element must have type `T` exactly.
    /// Mirrors `ConstantArray::get`.
    pub fn const_array<C, I>(self, elements: I) -> IrResult<ConstantAggregate<'ctx>>
    where
        I: IntoIterator<Item = C>,
        C: IsConstant<'ctx>,
    {
        let elem_ty = self.element().id();
        let expected_len = self.len();
        let mut ids = Vec::new();
        for c in elements {
            let value = c.as_value();
            if value.ty != elem_ty {
                return Err(IrError::TypeMismatch {
                    expected: self.element().kind_label(),
                    got: value.ty().kind_label(),
                });
            }
            ids.push(value.id);
        }
        if u64::try_from(ids.len()).unwrap_or_else(|_| unreachable!("element count fits in u64"))
            != expected_len
        {
            return Err(IrError::OperandWidthMismatch {
                lhs: u32::try_from(expected_len).unwrap_or(u32::MAX),
                rhs: u32::try_from(ids.len()).unwrap_or(u32::MAX),
            });
        }
        Ok(intern_aggregate(self.as_type(), ids.into_boxed_slice()))
    }
}

impl<'ctx, B: crate::struct_body_state::StructBodyState> StructType<'ctx, B> {
    /// `T { ... }`. Element types must match the struct's declared
    /// body. Mirrors `ConstantStruct::get`.
    pub fn const_struct<C, I>(self, elements: I) -> IrResult<ConstantAggregate<'ctx>>
    where
        I: IntoIterator<Item = C>,
        C: IsConstant<'ctx>,
    {
        // The struct must already have a body (literal structs always
        // do; identified structs need `set_struct_body` first).
        let count = self.field_count();
        let mut ids = Vec::new();
        for (i, c) in elements.into_iter().enumerate() {
            let value = c.as_value();
            let field = self.field_type(i).ok_or(IrError::OperandWidthMismatch {
                lhs: u32::try_from(count).unwrap_or(u32::MAX),
                rhs: u32::try_from(i + 1).unwrap_or(u32::MAX),
            })?;
            if value.ty != field.id() {
                return Err(IrError::TypeMismatch {
                    expected: field.kind_label(),
                    got: value.ty().kind_label(),
                });
            }
            ids.push(value.id);
        }
        if ids.len() != count {
            return Err(IrError::OperandWidthMismatch {
                lhs: u32::try_from(count).unwrap_or(u32::MAX),
                rhs: u32::try_from(ids.len()).unwrap_or(u32::MAX),
            });
        }
        Ok(intern_aggregate(self.as_type(), ids.into_boxed_slice()))
    }
}

impl<'ctx> VectorType<'ctx> {
    /// `<N x T> < ... >`. Mirrors `ConstantVector::get`.
    pub fn const_vector<C, I>(self, elements: I) -> IrResult<ConstantAggregate<'ctx>>
    where
        I: IntoIterator<Item = C>,
        C: IsConstant<'ctx>,
    {
        let elem_ty = self.element().id();
        let mut ids = Vec::new();
        for c in elements {
            let value = c.as_value();
            if value.ty != elem_ty {
                return Err(IrError::TypeMismatch {
                    expected: self.element().kind_label(),
                    got: value.ty().kind_label(),
                });
            }
            ids.push(value.id);
        }
        let n = ids.len();
        let expected = usize::try_from(self.min_len())
            .unwrap_or_else(|_| unreachable!("vector lane count fits in usize"));
        if !self.is_scalable() && n != expected {
            return Err(IrError::OperandWidthMismatch {
                lhs: u32::try_from(expected).unwrap_or(u32::MAX),
                rhs: u32::try_from(n).unwrap_or(u32::MAX),
            });
        }
        Ok(intern_aggregate(self.as_type(), ids.into_boxed_slice()))
    }
}

// --------------------------------------------------------------------------
// Undef / Poison
// --------------------------------------------------------------------------

impl<'ctx> Type<'ctx> {
    /// `undef <type>`. Mirrors `UndefValue::get`.
    pub fn get_undef(self) -> UndefValue<'ctx> {
        intern_undef(self)
    }

    /// `poison <type>`. Mirrors `PoisonValue::get`.
    pub fn get_poison(self) -> PoisonValue<'ctx> {
        intern_poison(self)
    }
}

// --------------------------------------------------------------------------
// Internal helpers
// --------------------------------------------------------------------------

fn encode_int_value(value: u64, bits: u32, sign_extend: bool) -> IrResult<Box<[u64]>> {
    debug_assert!((1..=MAX_INT_BITS).contains(&bits));
    if bits == 0 {
        return Err(IrError::InvalidIntegerWidth { bits });
    }
    let bit_count =
        usize::try_from(bits).unwrap_or_else(|_| unreachable!("u32 bit-width fits in usize"));

    if sign_extend && bits < 64 {
        // Sign-extend: treat `value` as a `bits`-wide signed integer.
        let bits_u32 = bits;
        let upper_mask: u64 = u64::MAX.checked_shl(bits_u32).unwrap_or(0);
        // Reject inputs whose top bits don't form a clean
        // sign-extension (i.e. top bits are neither all-zero for
        // non-negative nor all-one for negative).
        let masked = value & !upper_mask;
        let sign = (masked >> (bits_u32 - 1)) & 1 == 1;
        if sign {
            // Expected upper bits == upper_mask.
            if value & upper_mask != upper_mask {
                return Err(IrError::ImmediateOverflow {
                    value: u128::from(value),
                    bits,
                });
            }
        } else if value & upper_mask != 0 {
            return Err(IrError::ImmediateOverflow {
                value: u128::from(value),
                bits,
            });
        }
        // Truncate to `bits` and store the canonical zext form so
        // structurally-equal values share a value-id regardless of
        // sign-extension input shape.
        let truncated = masked;
        Ok(normalise_words(&[truncated]))
    } else if !sign_extend && bits < 64 {
        // Zero-extend: reject set bits above `bits`.
        let bits_u32 = bits;
        let lo_mask: u64 = (1u64 << bits_u32) - 1;
        if value & !lo_mask != 0 {
            return Err(IrError::ImmediateOverflow {
                value: u128::from(value),
                bits,
            });
        }
        Ok(normalise_words(&[value & lo_mask]))
    } else {
        // bits >= 64: every u64 fits.
        let _ = bit_count;
        Ok(normalise_words(&[value]))
    }
}

fn normalise_words(words: &[u64]) -> Box<[u64]> {
    let mut end = words.len();
    while end > 0 && words[end - 1] == 0 {
        end -= 1;
    }
    words[..end].to_vec().into_boxed_slice()
}

fn bits_used_in_words(words: &[u64]) -> u32 {
    let normalised = normalise_words(words);
    if normalised.is_empty() {
        return 0;
    }
    let top = normalised[normalised.len() - 1];
    let top_bits = 64 - top.leading_zeros();
    let limb_idx = u32::try_from(normalised.len() - 1)
        .unwrap_or_else(|_| unreachable!("limb count fits in u32 for any realistic constant"));
    limb_idx
        .checked_mul(64)
        .and_then(|p| p.checked_add(top_bits))
        .unwrap_or(u32::MAX)
}

fn intern_int_constant<'ctx, W: IntWidth>(
    ty: IntType<'ctx, W>,
    words: Box<[u64]>,
) -> ConstantIntValue<'ctx, W> {
    let module = ty.module.module();
    let id = module.context().intern_constant_int(ty.id, words);
    ConstantIntValue::from_parts_typed(constant_handle(id, module, ty.id))
}

fn intern_float_constant<'ctx, K: FloatKind>(
    ty: FloatType<'ctx, K>,
    bits: u128,
) -> ConstantFloatValue<'ctx, K> {
    let module = ty.module.module();
    let id = module.context().intern_constant_float(ty.id, bits);
    ConstantFloatValue::from_parts_typed(constant_handle(id, module, ty.id))
}

fn intern_pointer_null<'ctx>(ty: PointerType<'ctx>) -> ConstantPointerNull<'ctx> {
    let module = ty.module.module();
    let id = module.context().intern_constant_null(ty.id);
    ConstantPointerNull::from_parts(constant_handle(id, module, ty.id))
}

fn intern_undef<'ctx>(ty: Type<'ctx>) -> UndefValue<'ctx> {
    let module = ty.module();
    let id = module.context().intern_constant_undef(ty.id());
    UndefValue::from_parts(constant_handle(id, module, ty.id()))
}

fn intern_poison<'ctx>(ty: Type<'ctx>) -> PoisonValue<'ctx> {
    let module = ty.module();
    let id = module.context().intern_constant_poison(ty.id());
    PoisonValue::from_parts(constant_handle(id, module, ty.id()))
}

fn intern_aggregate<'ctx>(ty: Type<'ctx>, ids: Box<[ValueId]>) -> ConstantAggregate<'ctx> {
    let module = ty.module();
    let id = module.context().intern_constant_aggregate(ty.id(), ids);
    ConstantAggregate::from_parts(constant_handle(id, module, ty.id()))
}

#[inline]
fn constant_handle<'ctx>(
    id: ValueId,
    module: &'ctx crate::module::Module<'ctx>,
    ty: TypeId,
) -> Constant<'ctx> {
    Constant::from_parts(Value::from_parts(id, module, ty))
}
