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
    BlockAddressPlaceholder, Constant, ConstantData, ConstantExprData, ConstantExprFlags,
    ConstantExprInRange, ConstantExprOpcode, IsConstant, OverflowingConstantExprFlags,
};
use crate::derived_types::{
    ArrayType, FloatType, IntType, PointerType, StructType, TargetExtProperty, VectorType,
};
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::function::FunctionValue;
use crate::marker::{Dyn, ReturnMarker};
use crate::module::{Module, ModuleBrand, ModuleCore, ModuleRef, Unverified};
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
        pub struct $name<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
            pub(crate) id: ValueId,
            pub(crate) module: ModuleRef<'ctx, B>,
            pub(crate) ty: TypeId,
        }

        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> $name<'ctx, B> {
            #[inline]
            pub(crate) fn from_parts(c: Constant<'ctx, B>) -> Self {
                Self { id: c.id, module: c.module, ty: c.ty }
            }

            /// Widen to the erased [`Constant`] handle.
            #[inline]
            pub fn as_constant(self) -> Constant<'ctx, B> {
                Constant { id: self.id, module: self.module, ty: self.ty }
            }

            /// Widen to the erased [`Value`] handle.
            #[inline]
            pub fn as_value(self) -> Value<'ctx, B> {
                Value { id: self.id, module: self.module, ty: self.ty }
            }
        }

        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> sealed::Sealed for $name<'ctx, B> {}
        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> IsValue<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn as_value(self) -> Value<'ctx, B> { Self::as_value(self) }
        }
        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> IsConstant<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn as_constant(self) -> Constant<'ctx, B> { Self::as_constant(self) }
        }
        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> Typed<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn ty(self) -> Type<'ctx, B> {
                Type::new(self.ty, self.module)
            }
        }
        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> HasName<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn name(self) -> Option<String> { self.as_value().name() }
            #[inline]
            fn set_name(self, module_token: &Module<'ctx, B, Unverified>, name: &str) { self.as_value().set_name(module_token, name); }
            #[inline]
            fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) { self.as_value().clear_name(module_token); }
        }
        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> HasDebugLoc for $name<'ctx, B> {
            #[inline]
            fn debug_loc(self) -> Option<DebugLoc> { self.as_value().debug_loc() }
        }

        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> From<$name<'ctx, B>> for Constant<'ctx, B> {
            #[inline]
            fn from(c: $name<'ctx, B>) -> Self { c.as_constant() }
        }
        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> From<$name<'ctx, B>> for Value<'ctx, B> {
            #[inline]
            fn from(c: $name<'ctx, B>) -> Self { c.as_value() }
        }

        impl<'ctx, B: crate::module::ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>> for $name<'ctx, B> {
            type Error = IrError;
            fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
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
pub struct ConstantIntValue<
    'ctx,
    W: IntWidth,
    B: crate::module::ModuleBrand = crate::module::Brand<'ctx>,
> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) ty: TypeId,
    pub(crate) _w: PhantomData<W>,
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Clone for ConstantIntValue<'ctx, W, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Copy for ConstantIntValue<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> PartialEq for ConstantIntValue<'ctx, W, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Eq for ConstantIntValue<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Hash for ConstantIntValue<'ctx, W, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> fmt::Debug for ConstantIntValue<'ctx, W, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConstantIntValue")
            .field("id", &self.id)
            .field("width", &W::static_bits())
            .finish()
    }
}

impl<'ctx, W: IntWidth, B: crate::module::ModuleBrand + 'ctx> ConstantIntValue<'ctx, W, B> {
    #[inline]
    pub(crate) fn from_parts_typed(c: Constant<'ctx, B>) -> Self {
        Self {
            id: c.id,
            module: c.module,
            ty: c.ty,
            _w: PhantomData,
        }
    }
    #[inline]
    pub fn as_constant(self) -> Constant<'ctx, B> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    /// Erase the width marker.
    #[inline]
    pub fn as_dyn(self) -> ConstantIntValue<'ctx, IntDyn, B> {
        ConstantIntValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _w: PhantomData,
        }
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> sealed::Sealed for ConstantIntValue<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for ConstantIntValue<'ctx, W, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Self::as_value(self)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IsConstant<'ctx, B>
    for ConstantIntValue<'ctx, W, B>
{
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B> {
        Self::as_constant(self)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Typed<'ctx, B> for ConstantIntValue<'ctx, W, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> HasName<'ctx, B> for ConstantIntValue<'ctx, W, B> {
    fn name(self) -> Option<String> {
        self.as_value().name()
    }
    fn set_name(self, module_token: &Module<'ctx, B, Unverified>, name: &str) {
        self.as_value().set_name(module_token, name);
    }
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> HasDebugLoc for ConstantIntValue<'ctx, W, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> From<ConstantIntValue<'ctx, W, B>>
    for Constant<'ctx, B>
{
    #[inline]
    fn from(c: ConstantIntValue<'ctx, W, B>) -> Self {
        c.as_constant()
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> From<ConstantIntValue<'ctx, W, B>>
    for Value<'ctx, B>
{
    #[inline]
    fn from(c: ConstantIntValue<'ctx, W, B>) -> Self {
        c.as_value()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>> for ConstantIntValue<'ctx, IntDyn, B> {
    type Error = IrError;
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
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
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
            for ConstantIntValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
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
        impl<'ctx, B: ModuleBrand + 'ctx> From<ConstantIntValue<'ctx, $marker, B>>
            for ConstantIntValue<'ctx, IntDyn, B>
        {
            #[inline]
            fn from(c: ConstantIntValue<'ctx, $marker, B>) -> Self {
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
pub struct ConstantFloatValue<
    'ctx,
    K: FloatKind,
    B: crate::module::ModuleBrand = crate::module::Brand<'ctx>,
> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) ty: TypeId,
    pub(crate) _k: PhantomData<K>,
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Clone for ConstantFloatValue<'ctx, K, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Copy for ConstantFloatValue<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> PartialEq for ConstantFloatValue<'ctx, K, B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Eq for ConstantFloatValue<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Hash for ConstantFloatValue<'ctx, K, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> fmt::Debug for ConstantFloatValue<'ctx, K, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConstantFloatValue")
            .field("id", &self.id)
            .field("kind", &K::ieee_label())
            .finish()
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> ConstantFloatValue<'ctx, K, B> {
    #[inline]
    pub(crate) fn from_parts_typed(c: Constant<'ctx, B>) -> Self {
        Self {
            id: c.id,
            module: c.module,
            ty: c.ty,
            _k: PhantomData,
        }
    }
    #[inline]
    pub fn as_constant(self) -> Constant<'ctx, B> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn as_dyn(self) -> ConstantFloatValue<'ctx, FloatDyn, B> {
        ConstantFloatValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _k: PhantomData,
        }
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> sealed::Sealed for ConstantFloatValue<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> IsValue<'ctx, B>
    for ConstantFloatValue<'ctx, K, B>
{
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Self::as_value(self)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> IsConstant<'ctx, B>
    for ConstantFloatValue<'ctx, K, B>
{
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B> {
        Self::as_constant(self)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Typed<'ctx, B> for ConstantFloatValue<'ctx, K, B> {
    fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> HasName<'ctx, B>
    for ConstantFloatValue<'ctx, K, B>
{
    fn name(self) -> Option<String> {
        self.as_value().name()
    }
    fn set_name(self, module_token: &Module<'ctx, B, Unverified>, name: &str) {
        self.as_value().set_name(module_token, name);
    }
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> HasDebugLoc for ConstantFloatValue<'ctx, K, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> From<ConstantFloatValue<'ctx, K, B>>
    for Constant<'ctx, B>
{
    fn from(c: ConstantFloatValue<'ctx, K, B>) -> Self {
        c.as_constant()
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> From<ConstantFloatValue<'ctx, K, B>>
    for Value<'ctx, B>
{
    fn from(c: ConstantFloatValue<'ctx, K, B>) -> Self {
        c.as_value()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
    for ConstantFloatValue<'ctx, FloatDyn, B>
{
    type Error = IrError;
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
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
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
            for ConstantFloatValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
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
        impl<'ctx, B: ModuleBrand + 'ctx> From<ConstantFloatValue<'ctx, $marker, B>>
            for ConstantFloatValue<'ctx, FloatDyn, B>
        {
            #[inline]
            fn from(c: ConstantFloatValue<'ctx, $marker, B>) -> Self {
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

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IntType<'ctx, W, B> {
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
    ) -> IrResult<ConstantIntValue<'ctx, W, B>> {
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
    pub fn const_int<V>(self, v: V) -> ConstantIntValue<'ctx, W, B>
    where
        V: IntoConstantInt<'ctx, W, B, Error = Infallible>,
    {
        match v.into_constant_int(self) {
            Ok(c) => c,
            Err(_e) => unreachable!("Infallible cannot be constructed"),
        }
    }

    /// Fallible variant for narrowing / dynamic-width targets.
    pub fn const_int_checked<V>(self, v: V) -> IrResult<ConstantIntValue<'ctx, W, B>>
    where
        V: IntoConstantInt<'ctx, W, B, Error = IrError>,
    {
        v.into_constant_int(self)
    }

    /// Construct an integer constant from a precomputed
    /// little-endian-words magnitude. Mirrors
    /// `ConstantInt::get(IntegerType*, ArrayRef<uint64_t>)`.
    pub fn const_int_arbitrary_precision(
        self,
        words: &[u64],
    ) -> IrResult<ConstantIntValue<'ctx, W, B>> {
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
    pub fn const_zero(self) -> ConstantIntValue<'ctx, W, B> {
        intern_int_constant(self, Box::<[u64]>::from([]))
    }

    /// `iN -1` (all-ones). Mirrors `Constant::getAllOnesValue`.
    pub fn const_all_ones(self) -> ConstantIntValue<'ctx, W, B> {
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

impl<'ctx, W: IntWidth, B: crate::module::ModuleBrand + 'ctx> ConstantIntValue<'ctx, W, B> {
    #[inline]
    pub fn ty(self) -> IntType<'ctx, W, B> {
        IntType::new(self.ty, self.module)
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

impl<'ctx, K: FloatKind, B: crate::module::ModuleBrand + 'ctx> ConstantFloatValue<'ctx, K, B> {
    #[inline]
    pub fn ty(self) -> FloatType<'ctx, K, B> {
        FloatType::new(self.ty, self.module)
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

#[derive(Debug, Clone, Default)]
pub struct ConstantExprOptions<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
    pub source_ty: Option<Type<'ctx, B>>,
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

impl<'ctx> ModuleCore {
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
        let mut data = ConstantExprData {
            opcode,
            result_ty: result_ty.id(),
            source_ty: source_ty_id,
            operands: ids.into_boxed_slice(),
            indices: indices.into_iter().collect::<Vec<_>>().into_boxed_slice(),
            mask: mask.into_iter().collect::<Vec<_>>().into_boxed_slice(),
            flags: canonical_constant_expr_flags(options.flags),
        };
        canonicalize_constant_expr_data(self, &mut data)?;
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
        let ty = self.ptr_type(function.address_space()).as_type().id();
        let id = self.context().intern_constant_block_address(
            ty,
            function.as_dyn().as_value().id,
            block.as_dyn().as_value().id,
        );
        Ok(constant_handle(id, self, ty))
    }

    /// Parser-only placeholder for a forward `blockaddress` reference.
    /// It must be RAUW'd to a real [`Self::block_address`] before the parsed
    /// module is observed.
    #[doc(hidden)]
    pub fn block_address_placeholder(
        &'ctx self,
        ty: Type<'ctx>,
    ) -> IrResult<BlockAddressPlaceholder<'ctx>> {
        if ty.module().id() != self.id() {
            return Err(IrError::InvalidOperation {
                message: "type does not belong to this module",
            });
        }
        if !ty.is_pointer() {
            return Err(IrError::InvalidOperation {
                message: "blockaddress placeholder must have pointer type",
            });
        }
        let id = self
            .context()
            .push_constant_block_address_placeholder(ty.id());
        Ok(BlockAddressPlaceholder::from_constant(constant_handle(
            id,
            self,
            ty.id(),
        )))
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
        pointer: impl IsConstant<'ctx>,
        key: impl IsConstant<'ctx>,
        discriminator: impl IsConstant<'ctx>,
        addr_discriminator: impl IsConstant<'ctx>,
        deactivation_symbol: impl IsConstant<'ctx>,
    ) -> IrResult<Constant<'ctx>> {
        let pointer = pointer.as_constant().as_value();
        let key = key.as_constant().as_value();
        let discriminator = discriminator.as_constant().as_value();
        let addr_discriminator = addr_discriminator.as_constant().as_value();
        let deactivation_symbol = deactivation_symbol.as_constant().as_value();
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
            return Err(IrError::InvalidOperation {
                message: "constant ptrauth base pointer must be a pointer",
            });
        }
        if !is_int_constant_with_type(self, key.id, self.i32_type().as_type().id()) {
            return Err(IrError::InvalidOperation {
                message: "constant ptrauth key must be i32 constant",
            });
        }
        if !is_int_constant_with_type(self, discriminator.id, self.i64_type().as_type().id()) {
            return Err(IrError::InvalidOperation {
                message: "constant ptrauth integer discriminator must be i64 constant",
            });
        }
        if !addr_discriminator.ty().is_pointer() {
            return Err(IrError::InvalidOperation {
                message: "constant ptrauth address discriminator must be a pointer",
            });
        }
        if !deactivation_symbol.ty().is_pointer() {
            return Err(IrError::InvalidOperation {
                message: "constant ptrauth deactivation symbol must be a pointer",
            });
        }
        if !is_global_value_or_null_constant(self, deactivation_symbol.id) {
            return Err(IrError::InvalidOperation {
                message: "constant ptrauth deactivation symbol must be a global value or null",
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

fn is_int_constant_with_type(module: &ModuleCore, id: ValueId, ty: TypeId) -> bool {
    module.context().value_data(id).ty == ty
        && matches!(
            &module.context().value_data(id).kind,
            ValueKindData::Constant(ConstantData::Int(_))
        )
}

fn is_global_value_or_null_constant(module: &ModuleCore, id: ValueId) -> bool {
    matches!(
        &module.context().value_data(id).kind,
        ValueKindData::Constant(ConstantData::GlobalValueRef { .. } | ConstantData::PointerNull)
    )
}
fn canonical_constant_expr_flags(flags: ConstantExprFlags) -> ConstantExprFlags {
    match flags {
        ConstantExprFlags::Overflowing(OverflowingConstantExprFlags {
            nuw: false,
            nsw: false,
        }) => ConstantExprFlags::None,
        ConstantExprFlags::Gep(flags) if flags.no_wrap.is_empty() && flags.in_range.is_none() => {
            ConstantExprFlags::None
        }
        ConstantExprFlags::Gep(mut flags) => {
            flags.no_wrap = crate::GepNoWrapFlags::from_bits_canonical(flags.no_wrap.bits());
            flags.in_range = flags.in_range.map(canonical_in_range);
            ConstantExprFlags::Gep(flags)
        }
        flags => flags,
    }
}

fn canonical_in_range(mut in_range: ConstantExprInRange) -> ConstantExprInRange {
    in_range.start = canonical_apint_words(in_range.start, in_range.bit_width);
    in_range.end = canonical_apint_words(in_range.end, in_range.bit_width);
    in_range
}

fn canonical_apint_words(words: Box<[u64]>, bit_width: u32) -> Box<[u64]> {
    let Ok(word_count) = usize::try_from(bit_width.div_ceil(64)) else {
        return words;
    };
    let mut canonical = vec![0; word_count];
    let copy_count = canonical.len().min(words.len());
    canonical[..copy_count].copy_from_slice(&words[..copy_count]);
    mask_apint_top_word(&mut canonical, bit_width);
    canonical.into_boxed_slice()
}

fn mask_apint_top_word(words: &mut [u64], bit_width: u32) {
    let top_bits = bit_width % 64;
    if top_bits == 0 {
        return;
    }
    if let Some(last) = words.last_mut() {
        *last &= (1u64 << top_bits) - 1;
    }
}

fn canonicalize_constant_expr_data(
    module: &ModuleCore,
    data: &mut ConstantExprData,
) -> IrResult<()> {
    if matches!(data.opcode, ConstantExprOpcode::GetElementPtr) {
        canonicalize_gep_operands(module, data)?;
    }
    Ok(())
}

fn canonicalize_gep_operands(module: &ModuleCore, data: &mut ConstantExprData) -> IrResult<()> {
    let Some(source_ty) = data.source_ty else {
        return Ok(());
    };
    let Some((lanes, scalable)) = vector_shape(module, data.result_ty) else {
        return Ok(());
    };
    let mut current = source_ty;
    let mut first = true;
    let mut operands = data.operands.to_vec();
    if operands.len() <= 1 {
        return Ok(());
    }
    for index in &mut operands[1..] {
        let is_struct_index =
            !first && matches!(module.context().type_data(current), TypeData::Struct(_));
        let index_ty = module.context().value_data(*index).ty;
        if let Some(index_shape) = vector_shape(module, index_ty)
            && index_shape != (lanes, scalable)
        {
            return Err(IrError::InvalidOperation {
                message: "invalid getelementptr constant expression",
            });
        }
        if is_struct_index {
            if vector_shape(module, index_ty).is_some()
                && let Some(splat) = vector_splat_value(module, *index)
            {
                *index = splat;
            }
        } else if vector_shape(module, index_ty).is_none() {
            *index = vector_splat_constant(module, *index, lanes, scalable)?;
        }

        if first {
            first = false;
            continue;
        }
        let Some(next) = advance_gep_index_type(module, current, *index) else {
            break;
        };
        current = next;
    }
    data.operands = operands.into_boxed_slice();
    Ok(())
}

fn vector_splat_constant(
    module: &ModuleCore,
    scalar: ValueId,
    lanes: u32,
    scalable: bool,
) -> IrResult<ValueId> {
    let lane_count = usize::try_from(lanes).map_err(|_| IrError::InvalidOperation {
        message: "invalid getelementptr constant expression",
    })?;
    let elem_ty = module.context().value_data(scalar).ty;
    let vector_ty = module
        .vector_type(Type::new(elem_ty, module), lanes, scalable)
        .as_type();
    Ok(intern_aggregate(vector_ty, vec![scalar; lane_count].into_boxed_slice()).id)
}
fn valid_shufflevector_mask_constant(
    module: &ModuleCore,
    mask: ValueId,
    lhs_lanes: u32,
    lhs_scalable: bool,
) -> bool {
    match &module.context().value_data(mask).kind {
        ValueKindData::Constant(ConstantData::Undef) => true,
        ValueKindData::Constant(ConstantData::Aggregate(_)) if lhs_scalable => false,
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            let Some(bound) = u64::from(lhs_lanes).checked_mul(2) else {
                return false;
            };
            elements.iter().all(
                |element| match &module.context().value_data(*element).kind {
                    ValueKindData::Constant(ConstantData::Undef) => true,
                    ValueKindData::Constant(ConstantData::Int(_)) => {
                        const_index_u64(module, *element).is_some_and(|value| value < bound)
                    }
                    _ => false,
                },
            )
        }
        _ => false,
    }
}

fn vector_splat_value(module: &ModuleCore, vector: ValueId) -> Option<ValueId> {
    let ValueKindData::Constant(ConstantData::Aggregate(elements)) =
        &module.context().value_data(vector).kind
    else {
        return None;
    };
    let (&first, rest) = elements.split_first()?;
    rest.iter()
        .all(|element| *element == first)
        .then_some(first)
}

pub(crate) fn replace_constant_uses_with<'ctx, B: crate::module::ModuleBrand + 'ctx>(
    from: Constant<'ctx, B>,
    replacement: Constant<'ctx, B>,
) -> IrResult<()> {
    if replacement.module.id() != from.module.id() {
        return Err(IrError::ForeignValue);
    }
    if replacement.ty != from.ty {
        return Err(IrError::TypeMismatch {
            expected: from.ty().kind_label(),
            got: replacement.ty().kind_label(),
        });
    }
    if replacement.id == from.id {
        return Ok(());
    }
    replace_value_uses_with_constant(from.module.module(), from.id, replacement.id)
}

fn replace_value_uses_with_constant(
    module: &ModuleCore,
    from_id: ValueId,
    replacement_id: ValueId,
) -> IrResult<()> {
    let user_ids = module
        .context()
        .value_data(from_id)
        .use_list
        .borrow()
        .clone();
    let mut direct_users = Vec::new();
    for user_id in user_ids.iter().copied() {
        match &module.context().value_data(user_id).kind {
            ValueKindData::Instruction(inst) => {
                crate::instruction::rewrite_operand_cells(&inst.kind, from_id, replacement_id);
                direct_users.push(user_id);
            }
            ValueKindData::Constant(_) => {
                if let Some(rewritten_id) =
                    constant_with_replaced_operand(module, user_id, from_id, replacement_id)?
                    && rewritten_id != user_id
                {
                    replace_value_uses_with_constant(module, user_id, rewritten_id)?;
                }
            }
            ValueKindData::Argument { .. }
            | ValueKindData::BasicBlock(_)
            | ValueKindData::Function(_)
            | ValueKindData::GlobalVariable(_)
            | ValueKindData::GlobalAlias(_)
            | ValueKindData::GlobalIFunc(_)
            | ValueKindData::InlineAsm(_)
            | ValueKindData::MetadataAsValue { .. } => {}
        }
    }
    module
        .context()
        .value_data(from_id)
        .use_list
        .borrow_mut()
        .clear();
    module
        .context()
        .value_data(replacement_id)
        .use_list
        .borrow_mut()
        .extend(direct_users);
    Ok(())
}

fn constant_with_replaced_operand(
    module: &ModuleCore,
    user_id: ValueId,
    from_id: ValueId,
    replacement_id: ValueId,
) -> IrResult<Option<ValueId>> {
    let user_data = module.context().value_data(user_id);
    let ty = user_data.ty;
    let ValueKindData::Constant(data) = &user_data.kind else {
        return Ok(None);
    };
    match data {
        ConstantData::Aggregate(elements) => {
            if !elements.contains(&from_id) {
                return Ok(None);
            }
            let elements = elements
                .iter()
                .map(|id| if *id == from_id { replacement_id } else { *id })
                .collect::<Vec<_>>()
                .into_boxed_slice();
            Ok(Some(
                module.context().intern_constant_aggregate(ty, elements),
            ))
        }
        ConstantData::Expr(expr) => {
            if !expr.operands.contains(&from_id) {
                return Ok(None);
            }
            let mut expr = expr.clone();
            for operand in expr.operands.iter_mut() {
                if *operand == from_id {
                    *operand = replacement_id;
                }
            }
            canonicalize_constant_expr_data(module, &mut expr)?;
            validate_constant_expr_data(module, &expr)?;
            Ok(Some(module.context().intern_constant_expr(expr)))
        }
        ConstantData::PtrAuth {
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        } => {
            let mut pointer = *pointer;
            let mut key = *key;
            let mut discriminator = *discriminator;
            let mut addr_discriminator = *addr_discriminator;
            let mut deactivation_symbol = *deactivation_symbol;
            let mut changed = false;
            for operand in [
                &mut pointer,
                &mut key,
                &mut discriminator,
                &mut addr_discriminator,
                &mut deactivation_symbol,
            ] {
                if *operand == from_id {
                    *operand = replacement_id;
                    changed = true;
                }
            }
            if !changed {
                return Ok(None);
            }
            let rebuilt = module.ptr_auth(
                constant_handle(pointer, module, module.context().value_data(pointer).ty),
                constant_handle(key, module, module.context().value_data(key).ty),
                constant_handle(
                    discriminator,
                    module,
                    module.context().value_data(discriminator).ty,
                ),
                constant_handle(
                    addr_discriminator,
                    module,
                    module.context().value_data(addr_discriminator).ty,
                ),
                constant_handle(
                    deactivation_symbol,
                    module,
                    module.context().value_data(deactivation_symbol).ty,
                ),
            )?;
            Ok(Some(rebuilt.id))
        }
        ConstantData::Int(_)
        | ConstantData::Float(_)
        | ConstantData::GlobalValueRef { .. }
        | ConstantData::PointerNull
        | ConstantData::BlockAddressPlaceholder
        | ConstantData::GepOffset { .. }
        | ConstantData::SymbolDelta { .. }
        | ConstantData::SymbolDeltaPlus { .. }
        | ConstantData::BlockAddress { .. }
        | ConstantData::DSOLocalEquivalent { .. }
        | ConstantData::NoCfi { .. }
        | ConstantData::TokenNone
        | ConstantData::TargetExtNone
        | ConstantData::Undef
        | ConstantData::Poison => Ok(None),
    }
}

fn advance_gep_index_type(module: &ModuleCore, current: TypeId, index: ValueId) -> Option<TypeId> {
    match module.context().type_data(current) {
        TypeData::Array { elem, .. }
        | TypeData::FixedVector { elem, .. }
        | TypeData::ScalableVector { elem, .. } => Some(*elem),
        TypeData::Struct(s) => {
            let field_index = usize::try_from(const_index_u64(module, index)?).ok()?;
            let body = s.body.borrow();
            body.as_ref()?.elements.get(field_index).copied()
        }
        _ => None,
    }
}

fn validate_constant_expr_flags(data: &ConstantExprData) -> IrResult<()> {
    match (&data.opcode, &data.flags) {
        (
            ConstantExprOpcode::Add | ConstantExprOpcode::Sub,
            ConstantExprFlags::None
            | ConstantExprFlags::Overflowing(OverflowingConstantExprFlags { .. }),
        )
        | (ConstantExprOpcode::Xor, ConstantExprFlags::None)
        | (
            ConstantExprOpcode::GetElementPtr,
            ConstantExprFlags::None | ConstantExprFlags::Gep(_),
        )
        | (
            ConstantExprOpcode::Trunc
            | ConstantExprOpcode::PtrToAddr
            | ConstantExprOpcode::PtrToInt
            | ConstantExprOpcode::IntToPtr
            | ConstantExprOpcode::BitCast
            | ConstantExprOpcode::AddrSpaceCast
            | ConstantExprOpcode::ExtractElement
            | ConstantExprOpcode::InsertElement
            | ConstantExprOpcode::ShuffleVector,
            ConstantExprFlags::None,
        ) => {}
        _ => {
            return Err(IrError::InvalidOperation {
                message: "invalid constant expression flags",
            });
        }
    }

    if let ConstantExprFlags::Gep(flags) = &data.flags
        && let Some(in_range) = &flags.in_range
        && !constant_range_is_non_empty(in_range)
    {
        return Err(IrError::InvalidOperation {
            message: "expected end to be larger than start",
        });
    }

    Ok(())
}

fn constant_range_is_non_empty(range: &ConstantExprInRange) -> bool {
    signed_apint_cmp(&range.start, &range.end, range.bit_width).is_lt()
}

fn signed_apint_cmp(lhs: &[u64], rhs: &[u64], bit_width: u32) -> core::cmp::Ordering {
    let lhs_negative = apint_sign_bit(lhs, bit_width);
    let rhs_negative = apint_sign_bit(rhs, bit_width);
    match (lhs_negative, rhs_negative) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => unsigned_apint_cmp(lhs, rhs, bit_width),
    }
}

fn apint_sign_bit(words: &[u64], bit_width: u32) -> bool {
    if bit_width == 0 {
        return false;
    }
    let bit_index = bit_width - 1;
    let word_index = usize::try_from(bit_index / 64).unwrap_or(usize::MAX);
    let bit_in_word = bit_index % 64;
    words
        .get(word_index)
        .is_some_and(|word| ((word >> bit_in_word) & 1) != 0)
}

fn unsigned_apint_cmp(lhs: &[u64], rhs: &[u64], bit_width: u32) -> core::cmp::Ordering {
    let word_count = usize::try_from(bit_width.div_ceil(64)).unwrap_or(0);
    for idx in (0..word_count).rev() {
        let lhs_word = apint_word(lhs, idx, bit_width);
        let rhs_word = apint_word(rhs, idx, bit_width);
        match lhs_word.cmp(&rhs_word) {
            core::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    core::cmp::Ordering::Equal
}

fn apint_word(words: &[u64], idx: usize, bit_width: u32) -> u64 {
    let mut word = words.get(idx).copied().unwrap_or(0);
    let word_count = usize::try_from(bit_width.div_ceil(64)).unwrap_or(0);
    if word_count != 0 && idx + 1 == word_count {
        let top_bits = bit_width % 64;
        if top_bits != 0 {
            word &= (1u64 << top_bits) - 1;
        }
    }
    word
}

// --------------------------------------------------------------------------
pub(crate) fn validate_constant_expr_data(
    module: &ModuleCore,
    data: &ConstantExprData,
) -> IrResult<()> {
    let result_ty = Type::new(data.result_ty, module);
    let operand_tys: Vec<Type<'_>> = data
        .operands
        .iter()
        .map(|id| Type::new(module.context().value_data(*id).ty, module))
        .collect();
    validate_constant_expr_flags(data)?;
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
        ConstantExprOpcode::GetElementPtr => {
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
            let mask_id = data.operands[2];
            if lhs_elem != rhs_elem
                || lhs_lanes != rhs_lanes
                || lhs_scalable != rhs_scalable
                || mask_elem != module.i32_type().as_type().id()
                || mask_scalable != lhs_scalable
                || !valid_shufflevector_mask_constant(module, mask_id, lhs_lanes, lhs_scalable)
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
        }
    }
    Ok(())
}

pub(crate) fn verify_constant_expr_data(
    module: &ModuleCore,
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
    module: &ModuleCore,
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
    if type_contains_scalable_vector(module, source_ty.id()) {
        return Err(IrError::InvalidOperation {
            message: "invalid base element for constant getelementptr",
        });
    }
    if !is_ptr_or_ptr_vector(module, base_ty.id())
        || !is_ptr_or_ptr_vector(module, result_ty.id())
        || (!index_tys.is_empty() && !source_ty.is_sized())
        || index_tys
            .iter()
            .any(|ty| !is_int_or_int_vector(module, ty.id()))
    {
        return Err(IrError::InvalidOperation {
            message: "invalid getelementptr constant expression",
        });
    }
    let Some(base_addr_space) = pointer_address_space(module, scalar_type_id(module, base_ty.id()))
    else {
        return Err(IrError::InvalidOperation {
            message: "invalid getelementptr constant expression",
        });
    };
    if pointer_address_space(module, scalar_type_id(module, result_ty.id()))
        != Some(base_addr_space)
    {
        return Err(IrError::InvalidOperation {
            message: "invalid getelementptr constant expression",
        });
    }
    if let ConstantExprFlags::Gep(flags) = &data.flags
        && let Some(in_range) = &flags.in_range
    {
        let index_bit_width = module.data_layout().index_size_in_bits(base_addr_space);
        if in_range.bit_width != index_bit_width {
            return Err(IrError::InvalidOperation {
                message: "invalid getelementptr inrange bit width",
            });
        }
    }
    let mut gep_width = vector_shape(module, base_ty.id());
    for index_ty in index_tys {
        if let Some(index_shape) = vector_shape(module, index_ty.id()) {
            match gep_width {
                Some(pointer_shape) if index_shape != pointer_shape => {
                    return Err(IrError::InvalidOperation {
                        message: "invalid getelementptr constant expression",
                    });
                }
                _ => gep_width = Some(index_shape),
            }
        }
    }
    if vector_shape(module, result_ty.id()) != gep_width {
        return Err(IrError::InvalidOperation {
            message: "invalid getelementptr constant expression",
        });
    }
    validate_gep_indices(module, source_ty.id(), &data.operands[1..])
}

fn scalar_int_bits(module: &ModuleCore, id: TypeId) -> Option<u32> {
    match module.context().type_data(scalar_type_id(module, id)) {
        TypeData::Integer { bits } => Some(*bits),
        _ => None,
    }
}

fn scalar_type_id(module: &ModuleCore, id: TypeId) -> TypeId {
    module
        .context()
        .type_data(id)
        .as_vector()
        .map_or(id, |(elem, _, _)| elem)
}

fn type_contains_scalable_vector(module: &ModuleCore, id: TypeId) -> bool {
    match module.context().type_data(id) {
        TypeData::ScalableVector { .. } => true,
        TypeData::Array { elem, .. } | TypeData::FixedVector { elem, .. } => {
            type_contains_scalable_vector(module, *elem)
        }
        TypeData::Struct(s) => s.body.borrow().as_ref().is_some_and(|body| {
            body.elements
                .iter()
                .any(|elem| type_contains_scalable_vector(module, *elem))
        }),
        _ => false,
    }
}

fn vector_shape(module: &ModuleCore, id: TypeId) -> Option<(u32, bool)> {
    module
        .context()
        .type_data(id)
        .as_vector()
        .map(|(_, lanes, scalable)| (lanes, scalable))
}

fn lane_shape_matches(module: &ModuleCore, lhs: TypeId, rhs: TypeId) -> bool {
    vector_shape(module, lhs) == vector_shape(module, rhs)
}

fn is_ptr_or_ptr_vector(module: &ModuleCore, id: TypeId) -> bool {
    matches!(
        module.context().type_data(scalar_type_id(module, id)),
        TypeData::Pointer { .. }
    )
}

fn pointer_address_space(module: &ModuleCore, id: TypeId) -> Option<u32> {
    match module.context().type_data(id) {
        TypeData::Pointer { addr_space } => Some(*addr_space),
        _ => None,
    }
}

fn valid_bitcast_constant(module: &ModuleCore, src: TypeId, dst: TypeId) -> bool {
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
    module: &ModuleCore,
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

fn const_index_u64(module: &ModuleCore, id: ValueId) -> Option<u64> {
    match &module.context().value_data(id).kind {
        ValueKindData::Constant(ConstantData::Int(words)) if words.len() <= 1 => {
            Some(words.first().copied().unwrap_or(0))
        }
        _ => None,
    }
}

fn type_bit_width(module: &ModuleCore, id: TypeId) -> Option<u32> {
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

fn is_int_or_int_vector(module: &ModuleCore, id: TypeId) -> bool {
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

fn intern_int_constant<'ctx, W: IntWidth, B: ModuleBrand + 'ctx>(
    ty: IntType<'ctx, W, B>,
    words: Box<[u64]>,
) -> ConstantIntValue<'ctx, W, B> {
    let module = ty.module;
    let id = module.module().context().intern_constant_int(ty.id, words);
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
    UndefValue::from_parts(constant_handle(id, module.core_ref(), ty.id()))
}

fn intern_poison<'ctx>(ty: Type<'ctx>) -> PoisonValue<'ctx> {
    let module = ty.module();
    let id = module.context().intern_constant_poison(ty.id());
    PoisonValue::from_parts(constant_handle(id, module.core_ref(), ty.id()))
}

fn intern_aggregate<'ctx>(ty: Type<'ctx>, ids: Box<[ValueId]>) -> ConstantAggregate<'ctx> {
    let module = ty.module();
    let id = module.context().intern_constant_aggregate(ty.id(), ids);
    ConstantAggregate::from_parts(constant_handle(id, module.core_ref(), ty.id()))
}

#[inline]
fn constant_handle<'ctx, B, M>(id: ValueId, module: M, ty: TypeId) -> Constant<'ctx, B>
where
    B: ModuleBrand + 'ctx,
    M: Into<ModuleRef<'ctx, B>>,
{
    Constant::from_parts(Value::from_parts(id, module, ty))
}
