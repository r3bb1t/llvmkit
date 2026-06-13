//! Per-kind constant refinement handles plus their constructors.
//! Mirrors a slice of `llvm/include/llvm/IR/Constants.h`.
//!
//! Phase B continued: `ConstantInt`, `ConstantFP`,
//! `ConstantPointerNull`, `ConstantArray`/`Struct`/`Vector` (under one
//! [`ConstantAggregate`] handle), [`UndefValue`], [`PoisonValue`].
//!
//! Session 2 models the LLVM 22.1.4 parser-needed constant subset;
//! unsupported legacy `ConstantExpr` opcodes remain parser errors.
//!
//! Constructors live as **methods on the matching type-handle**, so
//! readers who already have an `IntType<'ctx>` write
//! `i32.const_int(42, false)?` instead of `Module::const_int(i32, 42,
//! ...)` — same shape inkwell uses.
//!
//! Every constant goes through the per-kind interning maps maintained
//! by the owning [`Module`]'s internal context, so structurally-equal
//! constants in the same module share a single value-id. That mirrors
//! LLVM's pointer-identity-after-uniquing semantics with no
//! pointer-based identity in our own code.

use crate::basic_block::BasicBlock;
use crate::block_state::BlockSealState;
use crate::constant::{
    Constant, ConstantData, ConstantExprData, ConstantExprFlags, ConstantExprOpcode, IsConstant,
};
use crate::derived_types::{
    ArrayType, FloatType, IntType, PointerType, StructType, TargetExtProperty, VectorType,
};
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::function::FunctionValue;
use crate::marker::{Dyn, ReturnMarker};
use crate::module::Module;
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

    /// Sign-extend the constant to a 128-bit signed integer. Mirrors
    /// `ConstantInt::getSExtValue` (`lib/IR/Constants.h`). Returns
    /// `None` if the constant's bit width does not fit in 128 bits;
    /// every llvmkit-shipped width (`bool`, `i8`..`i128`, and the
    /// `Width<const N>` form for `N <= 128`) round-trips losslessly.
    ///
    /// Differs from [`Self::value_zext_u128`] only when the high bit
    /// of the type's width is set: this method propagates it across
    /// the upper bits, [`Self::value_zext_u128`] zero-fills.
    pub fn value_sext_i128(self) -> Option<i128> {
        let w = self.words();
        let n = self.bit_width();
        if n > 128 {
            return None;
        }
        let raw = match w.len() {
            0 => 0u128,
            1 => u128::from(w[0]),
            2 => u128::from(w[0]) | (u128::from(w[1]) << 64),
            _ => return None,
        };
        let extended = if n == 128 {
            // Width matches; the bit pattern *is* the i128 value.
            raw
        } else {
            let sign_bit = 1u128 << (n - 1);
            let mask = (1u128 << n) - 1;
            let lo = raw & mask;
            if (lo & sign_bit) != 0 {
                // High bit set -> propagate ones across the upper bits.
                lo | !mask
            } else {
                lo
            }
        };
        // Reinterpret the u128 bit pattern as i128 without `as`.
        Some(i128::from_ne_bytes(extended.to_ne_bytes()))
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
// Parser-needed ConstantExpr and special constants
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct ConstantExprOptions<'ctx> {
    pub source_ty: Option<Type<'ctx>>,
    pub flags: ConstantExprFlags,
}

impl<'ctx> ConstantExprOptions<'ctx> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn source_ty(mut self, ty: Type<'ctx>) -> Self {
        self.source_ty = Some(ty);
        self
    }

    pub fn flags(mut self, flags: ConstantExprFlags) -> Self {
        self.flags = flags;
        self
    }
}

impl<'ctx> Module<'ctx> {
    /// Construct a parser-needed LLVM `ConstantExpr`.
    pub fn constant_expr(
        &'ctx self,
        result_ty: Type<'ctx>,
        opcode: ConstantExprOpcode,
        operands: impl IntoIterator<Item = Value<'ctx>>,
        indices: impl IntoIterator<Item = u32>,
        mask: impl IntoIterator<Item = i32>,
        flags: ConstantExprFlags,
    ) -> IrResult<Constant<'ctx>> {
        self.constant_expr_with_options(
            result_ty,
            opcode,
            operands,
            indices,
            mask,
            ConstantExprOptions {
                source_ty: None,
                flags,
            },
        )
    }

    /// Construct a parser-needed LLVM `ConstantExpr` with options such as an
    /// explicit `getelementptr` source element type.
    pub fn constant_expr_with_options(
        &'ctx self,
        result_ty: Type<'ctx>,
        opcode: ConstantExprOpcode,
        operands: impl IntoIterator<Item = Value<'ctx>>,
        indices: impl IntoIterator<Item = u32>,
        mask: impl IntoIterator<Item = i32>,
        options: ConstantExprOptions<'ctx>,
    ) -> IrResult<Constant<'ctx>> {
        if result_ty.module().id() != self.id() {
            return Err(IrError::InvalidOperation {
                message: "type does not belong to this module",
            });
        }
        let source_ty_id = match options.source_ty {
            Some(ty) => {
                if ty.module().id() != self.id() {
                    return Err(IrError::InvalidOperation {
                        message: "type does not belong to this module",
                    });
                }
                Some(ty.id())
            }
            None => None,
        };
        let mut ids = Vec::new();
        for operand in operands {
            if operand.module().id() != self.id() {
                return Err(IrError::ForeignValue);
            }
            ids.push(operand.id);
        }
        let data = ConstantExprData {
            opcode,
            result_ty: result_ty.id(),
            source_ty: source_ty_id,
            operands: ids.into_boxed_slice(),
            indices: indices.into_iter().collect::<Vec<_>>().into_boxed_slice(),
            mask: mask.into_iter().collect::<Vec<_>>().into_boxed_slice(),
            flags: options.flags,
        };
        validate_constant_expr_data(self, &data)?;
        let id = self.context().intern_constant_expr(data);
        Ok(constant_handle(id, self, result_ty.id()))
    }

    /// `blockaddress(@function, %block)`.
    pub fn block_address<R, S>(
        &'ctx self,
        function: FunctionValue<'ctx, R>,
        block: BasicBlock<'ctx, R, S>,
    ) -> IrResult<Constant<'ctx>>
    where
        R: ReturnMarker,
        S: BlockSealState,
    {
        if function.as_value().module().id() != self.id()
            || block.as_value().module().id() != self.id()
        {
            return Err(IrError::ForeignValue);
        }
        if block.parent_function().map(|f| f.as_value().id) != Some(function.as_dyn().as_value().id)
        {
            return Err(IrError::InvalidOperation {
                message: "blockaddress block must belong to function",
            });
        }
        let ty = self.ptr_type(0).as_type().id();
        let id = self.context().intern_constant_block_address(
            ty,
            function.as_dyn().as_value().id,
            block.as_dyn().as_value().id,
        );
        Ok(constant_handle(id, self, ty))
    }

    /// `dso_local_equivalent @function`.
    pub fn dso_local_equivalent(&'ctx self, function: FunctionValue<'ctx, Dyn>) -> Constant<'ctx> {
        let ty = self.ptr_type(0).as_type().id();
        let id = self
            .context()
            .intern_constant_dso_local_equivalent(ty, function.as_value().id);
        constant_handle(id, self, ty)
    }
    /// `dso_local_equivalent` over a function, alias-to-function, or ifunc.
    pub fn dso_local_equivalent_global(
        &'ctx self,
        global: Constant<'ctx>,
    ) -> IrResult<Constant<'ctx>> {
        if global.as_value().module().id() != self.id() {
            return Err(IrError::ForeignValue);
        }
        let value = match &self.context().value_data(global.as_value().id).kind {
            ValueKindData::Constant(ConstantData::GlobalValueRef { value }) => {
                Value::from_parts(*value, self, self.context().value_data(*value).ty)
            }
            _ => global.as_value(),
        };
        let is_function_like = match &self.context().value_data(value.id).kind {
            ValueKindData::Function(_) => true,
            ValueKindData::GlobalAlias(_) => crate::GlobalAlias::try_from(value)?
                .value_type()
                .is_function(),
            ValueKindData::GlobalIFunc(_) => crate::GlobalIFunc::try_from(value)?
                .value_type()
                .is_function(),
            _ => false,
        };
        if !is_function_like {
            return Err(IrError::InvalidOperation {
                message: "dso_local_equivalent expects a function, alias to function, or ifunc",
            });
        }
        let ty = self.ptr_type(0).as_type().id();
        let id = self
            .context()
            .intern_constant_dso_local_equivalent(ty, value.id);
        Ok(constant_handle(id, self, ty))
    }

    /// `no_cfi @function`.
    pub fn no_cfi(&'ctx self, function: FunctionValue<'ctx, Dyn>) -> Constant<'ctx> {
        let ty = self.ptr_type(0).as_type().id();
        let id = self
            .context()
            .intern_constant_no_cfi(ty, function.as_value().id);
        constant_handle(id, self, ty)
    }

    /// `no_cfi` over any global value reference.
    pub fn no_cfi_global(&'ctx self, global: Constant<'ctx>) -> IrResult<Constant<'ctx>> {
        if global.as_value().module().id() != self.id() {
            return Err(IrError::ForeignValue);
        }
        let value = match &self.context().value_data(global.as_value().id).kind {
            ValueKindData::Constant(ConstantData::GlobalValueRef { value }) => {
                Value::from_parts(*value, self, self.context().value_data(*value).ty)
            }
            _ => global.as_value(),
        };
        match &self.context().value_data(value.id).kind {
            ValueKindData::Function(_)
            | ValueKindData::GlobalVariable(_)
            | ValueKindData::GlobalAlias(_)
            | ValueKindData::GlobalIFunc(_) => {}
            _ => {
                return Err(IrError::InvalidOperation {
                    message: "no_cfi expects a global value",
                });
            }
        }
        let ty = self.ptr_type(0).as_type().id();
        let id = self.context().intern_constant_no_cfi(ty, value.id);
        Ok(constant_handle(id, self, ty))
    }

    /// `ptrauth (ptr <pointer>, i32 <key>, i64 <discriminator>, ptr <addr-discriminator>, ptr <deactivation-symbol>)`.
    pub fn ptr_auth(
        &'ctx self,
        pointer: impl IsValue<'ctx>,
        key: impl IsValue<'ctx>,
        discriminator: impl IsValue<'ctx>,
        addr_discriminator: impl IsValue<'ctx>,
        deactivation_symbol: impl IsValue<'ctx>,
    ) -> IrResult<Constant<'ctx>> {
        let pointer = pointer.as_value();
        let key = key.as_value();
        let discriminator = discriminator.as_value();
        let addr_discriminator = addr_discriminator.as_value();
        let deactivation_symbol = deactivation_symbol.as_value();
        for operand in [
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        ] {
            if operand.module().id() != self.id() {
                return Err(IrError::ForeignValue);
            }
        }
        if !pointer.ty().is_pointer() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: pointer.ty().kind_label(),
            });
        }
        if key.ty() != self.i32_type().as_type() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: key.ty().kind_label(),
            });
        }
        if discriminator.ty() != self.i64_type().as_type() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: discriminator.ty().kind_label(),
            });
        }
        if !addr_discriminator.ty().is_pointer() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: addr_discriminator.ty().kind_label(),
            });
        }
        if !deactivation_symbol.ty().is_pointer() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: deactivation_symbol.ty().kind_label(),
            });
        }
        let ty = pointer.ty().id();
        let id = self.context().intern_constant_ptrauth(
            ty,
            pointer.id,
            key.id,
            discriminator.id,
            addr_discriminator.id,
            deactivation_symbol.id,
        );
        Ok(constant_handle(id, self, ty))
    }

    /// `token none`.
    pub fn token_none(&'ctx self) -> Constant<'ctx> {
        let ty = self.token_type().as_type().id();
        let id = self.context().intern_constant_token_none(ty);
        constant_handle(id, self, ty)
    }

    /// `target(...) none`.
    pub fn target_ext_none(&'ctx self, ty: Type<'ctx>) -> IrResult<Constant<'ctx>> {
        if ty.module().id() != self.id() {
            return Err(IrError::InvalidOperation {
                message: "type does not belong to this module",
            });
        }
        let crate::derived_types::AnyTypeEnum::TargetExt(target_ty) =
            crate::derived_types::AnyTypeEnum::from(ty)
        else {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::TargetExt,
                got: ty.kind_label(),
            });
        };
        if !target_ty.has_property(TargetExtProperty::HasZeroInit) {
            return Err(IrError::InvalidOperation {
                message: "invalid type for null constant",
            });
        }
        let id = self.context().intern_constant_target_ext_none(ty.id());
        Ok(constant_handle(id, self, ty.id()))
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
pub(crate) fn validate_constant_expr_data(
    module: &Module<'_>,
    data: &ConstantExprData,
) -> IrResult<()> {
    let result_ty = Type::new(data.result_ty, module);
    let operand_tys: Vec<Type<'_>> = data
        .operands
        .iter()
        .map(|id| Type::new(module.context().value_data(*id).ty, module))
        .collect();
    match data.opcode {
        ConstantExprOpcode::Trunc => {
            let [src_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "trunc constant expression expects one operand",
                });
            };
            let Some(src_bits) = scalar_int_bits(module, src_ty.id()) else {
                return Err(IrError::InvalidOperation {
                    message: "invalid trunc constant expression",
                });
            };
            let Some(dst_bits) = scalar_int_bits(module, result_ty.id()) else {
                return Err(IrError::InvalidOperation {
                    message: "invalid trunc constant expression",
                });
            };
            if !lane_shape_matches(module, src_ty.id(), result_ty.id()) || dst_bits >= src_bits {
                return Err(IrError::InvalidOperation {
                    message: "invalid trunc constant expression",
                });
            }
        }
        ConstantExprOpcode::PtrToAddr | ConstantExprOpcode::PtrToInt => {
            let [src_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "ptrtoint constant expression expects one operand",
                });
            };
            if !is_ptr_or_ptr_vector(module, src_ty.id())
                || !is_int_or_int_vector(module, result_ty.id())
                || !lane_shape_matches(module, src_ty.id(), result_ty.id())
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid ptrtoint constant expression",
                });
            }
        }
        ConstantExprOpcode::IntToPtr => {
            let [src_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "inttoptr constant expression expects one operand",
                });
            };
            if !is_int_or_int_vector(module, src_ty.id())
                || !is_ptr_or_ptr_vector(module, result_ty.id())
                || !lane_shape_matches(module, src_ty.id(), result_ty.id())
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid inttoptr constant expression",
                });
            }
        }
        ConstantExprOpcode::BitCast => {
            let [src_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "bitcast constant expression expects one operand",
                });
            };
            if !valid_bitcast_constant(module, src_ty.id(), result_ty.id()) {
                return Err(IrError::InvalidOperation {
                    message: "invalid bitcast constant expression",
                });
            }
        }
        ConstantExprOpcode::AddrSpaceCast => {
            let [src_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "addrspacecast constant expression expects one operand",
                });
            };
            if !is_ptr_or_ptr_vector(module, src_ty.id())
                || !is_ptr_or_ptr_vector(module, result_ty.id())
                || !lane_shape_matches(module, src_ty.id(), result_ty.id())
                || pointer_address_space(module, scalar_type_id(module, src_ty.id()))
                    == pointer_address_space(module, scalar_type_id(module, result_ty.id()))
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid addrspacecast constant expression",
                });
            }
        }
        ConstantExprOpcode::GetElementPtr | ConstantExprOpcode::InBoundsGetElementPtr => {
            validate_gep_constant_expr(module, data, result_ty, &operand_tys)?;
        }
        ConstantExprOpcode::ExtractElement => {
            let [vector_ty, index_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "extractelement constant expression expects two operands",
                });
            };
            let Some((elem, _, _)) = module.context().type_data(vector_ty.id()).as_vector() else {
                return Err(IrError::InvalidOperation {
                    message: "invalid extractelement constant expression",
                });
            };
            if !index_ty.is_integer() || Type::new(elem, module) != result_ty {
                return Err(IrError::InvalidOperation {
                    message: "invalid extractelement constant expression",
                });
            }
        }
        ConstantExprOpcode::InsertElement => {
            let [vector_ty, value_ty, index_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "insertelement constant expression expects three operands",
                });
            };
            let Some((elem, _, _)) = module.context().type_data(vector_ty.id()).as_vector() else {
                return Err(IrError::InvalidOperation {
                    message: "invalid insertelement constant expression",
                });
            };
            if !index_ty.is_integer()
                || *vector_ty != result_ty
                || Type::new(elem, module) != *value_ty
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid insertelement constant expression",
                });
            }
        }
        ConstantExprOpcode::ShuffleVector => {
            let [lhs_ty, rhs_ty, mask_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "shufflevector constant expression expects three operands",
                });
            };
            let Some((lhs_elem, lhs_lanes, lhs_scalable)) =
                module.context().type_data(lhs_ty.id()).as_vector()
            else {
                return Err(IrError::InvalidOperation {
                    message: "invalid shufflevector constant expression",
                });
            };
            let Some((rhs_elem, rhs_lanes, rhs_scalable)) =
                module.context().type_data(rhs_ty.id()).as_vector()
            else {
                return Err(IrError::InvalidOperation {
                    message: "invalid shufflevector constant expression",
                });
            };
            let Some((mask_elem, mask_lanes, mask_scalable)) =
                module.context().type_data(mask_ty.id()).as_vector()
            else {
                return Err(IrError::InvalidOperation {
                    message: "invalid shufflevector constant expression",
                });
            };
            let Some((result_elem, result_lanes, result_scalable)) =
                module.context().type_data(result_ty.id()).as_vector()
            else {
                return Err(IrError::InvalidOperation {
                    message: "invalid shufflevector constant expression",
                });
            };
            if lhs_elem != rhs_elem
                || lhs_lanes != rhs_lanes
                || lhs_scalable != rhs_scalable
                || !matches!(
                    module.context().type_data(mask_elem),
                    TypeData::Integer { .. }
                )
                || result_elem != lhs_elem
                || result_lanes != mask_lanes
                || result_scalable != mask_scalable
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid shufflevector constant expression",
                });
            }
        }
        ConstantExprOpcode::Add | ConstantExprOpcode::Sub | ConstantExprOpcode::Xor => {
            let [lhs_ty, rhs_ty] = operand_tys.as_slice() else {
                return Err(IrError::InvalidOperation {
                    message: "binary constant expression expects two operands",
                });
            };
            if lhs_ty != rhs_ty
                || *lhs_ty != result_ty
                || !is_int_or_int_vector(module, lhs_ty.id())
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid binary constant expression",
                });
            }
            if matches!(data.opcode, ConstantExprOpcode::Xor) && (data.flags.nuw || data.flags.nsw)
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid binary constant expression",
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_constant_expr_data(
    module: &Module<'_>,
    data: &ConstantExprData,
) -> IrResult<()> {
    validate_constant_expr_data(module, data)?;
    if matches!(data.opcode, ConstantExprOpcode::PtrToAddr) {
        let result_ty = Type::new(data.result_ty, module);
        let [src] = data.operands.as_ref() else {
            return Err(IrError::InvalidOperation {
                message: "ptrtoaddr constant expression expects one operand",
            });
        };
        let src_ty = Type::new(module.context().value_data(*src).ty, module);
        let addr_bits = pointer_address_space(module, scalar_type_id(module, src_ty.id()))
            .map(|as_id| module.data_layout().pointer_size_in_bits(as_id));
        if addr_bits != scalar_int_bits(module, result_ty.id()) {
            return Err(IrError::InvalidOperation {
                message: "PtrToAddr result must be address width",
            });
        }
    }
    Ok(())
}

fn validate_gep_constant_expr(
    module: &Module<'_>,
    data: &ConstantExprData,
    result_ty: Type<'_>,
    operand_tys: &[Type<'_>],
) -> IrResult<()> {
    let Some(source_ty) = data.source_ty.map(|id| Type::new(id, module)) else {
        return Err(IrError::InvalidOperation {
            message: "getelementptr constant expression missing source type",
        });
    };
    let Some((base_ty, index_tys)) = operand_tys.split_first() else {
        return Err(IrError::InvalidOperation {
            message: "getelementptr constant expression expects a pointer base",
        });
    };
    if !is_ptr_or_ptr_vector(module, base_ty.id())
        || !is_ptr_or_ptr_vector(module, result_ty.id())
        || !lane_shape_matches(module, base_ty.id(), result_ty.id())
        || !source_ty.is_sized()
        || matches!(source_ty.data(), TypeData::ScalableVector { .. })
        || index_tys
            .iter()
            .any(|ty| !is_int_or_int_vector(module, ty.id()))
    {
        return Err(IrError::InvalidOperation {
            message: "invalid getelementptr constant expression",
        });
    }
    if let Some(pointer_shape) = vector_shape(module, base_ty.id()) {
        for index_ty in index_tys {
            if let Some(index_shape) = vector_shape(module, index_ty.id())
                && index_shape != pointer_shape
            {
                return Err(IrError::InvalidOperation {
                    message: "invalid getelementptr constant expression",
                });
            }
        }
    }
    validate_gep_indices(module, source_ty.id(), &data.operands[1..])
}

fn scalar_int_bits(module: &Module<'_>, id: TypeId) -> Option<u32> {
    match module.context().type_data(scalar_type_id(module, id)) {
        TypeData::Integer { bits } => Some(*bits),
        _ => None,
    }
}

fn scalar_type_id(module: &Module<'_>, id: TypeId) -> TypeId {
    module
        .context()
        .type_data(id)
        .as_vector()
        .map_or(id, |(elem, _, _)| elem)
}

fn vector_shape(module: &Module<'_>, id: TypeId) -> Option<(u32, bool)> {
    module
        .context()
        .type_data(id)
        .as_vector()
        .map(|(_, lanes, scalable)| (lanes, scalable))
}

fn lane_shape_matches(module: &Module<'_>, lhs: TypeId, rhs: TypeId) -> bool {
    vector_shape(module, lhs) == vector_shape(module, rhs)
}

fn is_ptr_or_ptr_vector(module: &Module<'_>, id: TypeId) -> bool {
    matches!(
        module.context().type_data(scalar_type_id(module, id)),
        TypeData::Pointer { .. }
    )
}

fn pointer_address_space(module: &Module<'_>, id: TypeId) -> Option<u32> {
    match module.context().type_data(id) {
        TypeData::Pointer { addr_space } => Some(*addr_space),
        _ => None,
    }
}

fn valid_bitcast_constant(module: &Module<'_>, src: TypeId, dst: TypeId) -> bool {
    if !lane_shape_matches(module, src, dst) {
        return false;
    }
    let src_scalar = scalar_type_id(module, src);
    let dst_scalar = scalar_type_id(module, dst);
    let src_ptr = pointer_address_space(module, src_scalar);
    let dst_ptr = pointer_address_space(module, dst_scalar);
    match (src_ptr, dst_ptr) {
        (Some(src_as), Some(dst_as)) => src_as == dst_as,
        (Some(_), None) | (None, Some(_)) => false,
        (None, None) => {
            Type::new(src_scalar, module).is_single_value()
                && Type::new(dst_scalar, module).is_single_value()
                && !Type::new(src_scalar, module).is_aggregate()
                && !Type::new(dst_scalar, module).is_aggregate()
                && type_bit_width(module, src) == type_bit_width(module, dst)
        }
    }
}

fn validate_gep_indices(
    module: &Module<'_>,
    source_ty: TypeId,
    indices: &[ValueId],
) -> IrResult<()> {
    let mut current = source_ty;
    let mut first = true;
    for index in indices {
        if first {
            first = false;
            continue;
        }
        match module.context().type_data(current) {
            TypeData::Array { elem, .. } => current = *elem,
            TypeData::FixedVector { elem, .. } => current = *elem,
            TypeData::Struct(s) => {
                let Some(field_index) = const_index_u64(module, *index) else {
                    return Err(IrError::InvalidOperation {
                        message: "invalid getelementptr indices",
                    });
                };
                let Ok(field_index) = usize::try_from(field_index) else {
                    return Err(IrError::InvalidOperation {
                        message: "invalid getelementptr indices",
                    });
                };
                let body = s.body.borrow();
                let Some(body) = body.as_ref() else {
                    return Err(IrError::InvalidOperation {
                        message: "invalid getelementptr indices",
                    });
                };
                let Some(field_ty) = body.elements.get(field_index).copied() else {
                    return Err(IrError::InvalidOperation {
                        message: "invalid getelementptr indices",
                    });
                };
                current = field_ty;
            }
            _ => {
                return Err(IrError::InvalidOperation {
                    message: "invalid getelementptr indices",
                });
            }
        }
    }
    Ok(())
}

fn const_index_u64(module: &Module<'_>, id: ValueId) -> Option<u64> {
    match &module.context().value_data(id).kind {
        ValueKindData::Constant(ConstantData::Int(words)) if words.len() <= 1 => {
            Some(words.first().copied().unwrap_or(0))
        }
        _ => None,
    }
}

fn type_bit_width(module: &Module<'_>, id: TypeId) -> Option<u32> {
    match module.context().type_data(id) {
        TypeData::Half | TypeData::BFloat => Some(16),
        TypeData::Float => Some(32),
        TypeData::Double => Some(64),
        TypeData::X86Fp80 => Some(80),
        TypeData::Fp128 | TypeData::PpcFp128 => Some(128),
        TypeData::Integer { bits } => Some(*bits),
        TypeData::Pointer { addr_space } => {
            Some(module.data_layout().pointer_size_in_bits(*addr_space))
        }
        TypeData::FixedVector { elem, n } => {
            type_bit_width(module, *elem).and_then(|bits| bits.checked_mul(*n))
        }
        TypeData::ScalableVector { .. } => None,
        _ => None,
    }
}

fn is_int_or_int_vector(module: &Module<'_>, id: TypeId) -> bool {
    match module.context().type_data(id) {
        TypeData::Integer { .. } => true,
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            matches!(module.context().type_data(*elem), TypeData::Integer { .. })
        }
        _ => false,
    }
}
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
