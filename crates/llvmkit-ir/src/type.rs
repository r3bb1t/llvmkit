//! IR type system. Mirrors `llvm/include/llvm/IR/Type.h` and
//! `llvm/lib/IR/Type.cpp`.
//!
//! ## Representation
//!
//! Storage is index-based: every interned type lives in the owning
//! module's interning context, identified by a crate-internal `TypeSlot`
//! (a `NonZeroU32` newtype). Children are also `TypeSlot`s, so the
//! storage payload `TypeData` is lifetime-free, has trivial `Hash`/`Eq`
//! derivability at the storage layer, and never participates in
//! pointer comparisons.
//!
//! Per the IR foundation plan (Pivot 1, "dual-view"):
//!
//! - **Storage:** an internal `TypeData` enum, one variant
//!   per LLVM `TypeID`.
//! - **Public handle:** [`Type`] is `(TypeSlot, ModuleRef<'ctx>)`. Both
//!   fields are `Hash + Eq`, so the handle derives all of
//!   `Copy + Clone + PartialEq + Eq + Hash + Debug` with no hand-written
//!   impls.
//! - **Analysis enum:** [`TypeKind`] is the discriminator users
//!   pattern-match for read-only inspection.
//!
//! Bit-width constants [`MIN_INT_BITS`] / [`MAX_INT_BITS`] mirror
//! `IntegerType::MIN_INT_BITS` / `MAX_INT_BITS` (`DerivedTypes.h`).

use core::cell::RefCell;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::num::NonZeroU32;

use crate::TypeKindLabel;
use crate::error::{IrError, IrResult};
use crate::module::{ModuleBrand, ModuleCore, ModuleRef, ModuleView};

/// Minimum legal integer width. Mirrors `IntegerType::MIN_INT_BITS`
/// (`DerivedTypes.h`).
pub const MIN_INT_BITS: u32 = 1;

/// Maximum legal integer width. Mirrors `IntegerType::MAX_INT_BITS`
/// (`DerivedTypes.h`). Equals `1 << 23` (8 388 608).
pub const MAX_INT_BITS: u32 = 1 << 23;

// --------------------------------------------------------------------------
// Type id
// --------------------------------------------------------------------------

/// Stable index into the type arena. The numeric contents are opaque; callers
/// may store and pass the handle back to this crate, but cannot construct one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeSlot(NonZeroU32);

impl TypeSlot {
    /// Build from a 0-based arena index. Stored as `index + 1` so the
    /// underlying value is always non-zero.
    #[inline]
    pub(crate) fn from_index(index: usize) -> Self {
        let raw = u32::try_from(index + 1).expect("type arena overflow (>u32::MAX entries)");
        Self(NonZeroU32::new(raw).expect("idx + 1 > 0"))
    }

    /// Recover the 0-based arena index.
    #[inline]
    pub(crate) fn arena_index(self) -> usize {
        // Subtraction is sound: every TypeSlot was produced by `from_index`,
        // which guarantees the underlying value is in `1..=u32::MAX`.
        let nz = u32::from(self.0);
        usize::try_from(nz - 1).expect("u32 fits in usize on supported targets")
    }
}

// --------------------------------------------------------------------------
// Internal payload
// --------------------------------------------------------------------------

/// Internal payload for a single interned type.
///
/// One variant per `Type::TypeID` (`Type.h`). Children are stored as
/// [`TypeSlot`] indices into the same module's arena.
#[derive(Debug)]
pub(crate) enum TypeData {
    // ---- Primitive / sized-but-childless ----
    Void,
    Half,
    Bfloat,
    Float,
    Double,
    X86Fp80,
    Fp128,
    PpcFp128,
    X86Amx,
    WasmExnRef,
    Label,
    Metadata,
    Token,
    Integer {
        bits: u32,
    },
    /// Opaque pointer (LLVM 17+). Element types are no longer carried by
    /// `Pointer`; `getelementptr` / `load` / `store` carry them
    /// explicitly.
    Pointer {
        addr_space: u32,
    },

    // ---- Derived ----
    Function {
        ret: TypeSlot,
        params: Box<[TypeSlot]>,
        is_var_arg: bool,
    },
    Array {
        elem: TypeSlot,
        n: u64,
    },
    FixedVector {
        elem: TypeSlot,
        n: u32,
    },
    ScalableVector {
        elem: TypeSlot,
        min: u32,
    },
    Struct(StructTypeData),
    /// Typed pointer (legacy, only used by a few GPU targets in LLVM 22).
    /// Mirrors `TypedPointerType` (`TypedPointerType.h`).
    TypedPointer {
        pointee: TypeSlot,
        addr_space: u32,
    },
    TargetExt(TargetExtTypeData),
}

impl TypeData {
    // ---- Per-variant projection helpers ----
    //
    // Every typed handle (IntType, ArrayType, ...) wraps a `TypeSlot` whose
    // payload, by construction, is the matching variant. Accessors on
    // those handles call the corresponding `as_*` helper here and rely on
    // `expect("<Foo> invariant")` to make the by-construction promise
    // explicit. Centralising the per-variant projection means there is
    // exactly one place per kind where the invariant is named, instead of
    // a hidden `_ => unreachable!()` arm sprinkled across handle methods.

    #[inline]
    pub(crate) fn as_integer(&self) -> Option<u32> {
        if let Self::Integer { bits } = *self {
            Some(bits)
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn as_pointer(&self) -> Option<u32> {
        if let Self::Pointer { addr_space } = *self {
            Some(addr_space)
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn as_array(&self) -> Option<(TypeSlot, u64)> {
        if let Self::Array { elem, n } = *self {
            Some((elem, n))
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn as_vector(&self) -> Option<(TypeSlot, u32, bool)> {
        match *self {
            Self::FixedVector { elem, n } => Some((elem, n, false)),
            Self::ScalableVector { elem, min } => Some((elem, min, true)),
            _ => None,
        }
    }
    #[inline]
    pub(crate) fn as_function(&self) -> Option<(TypeSlot, &[TypeSlot], bool)> {
        if let Self::Function {
            ret,
            params,
            is_var_arg,
        } = self
        {
            Some((*ret, params, *is_var_arg))
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn as_struct(&self) -> Option<&StructTypeData> {
        if let Self::Struct(s) = self {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn as_typed_pointer(&self) -> Option<(TypeSlot, u32)> {
        if let Self::TypedPointer {
            pointee,
            addr_space,
        } = *self
        {
            Some((pointee, addr_space))
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn as_target_ext(&self) -> Option<&TargetExtTypeData> {
        if let Self::TargetExt(t) = self {
            Some(t)
        } else {
            None
        }
    }
}

/// Payload for any struct type — literal or identified.
///
/// [`StructIdentity`] is the discriminator, not the name: a literal struct's
/// `body` is set at creation and never changes, while an identified struct's
/// may be filled in later via `set_struct_body` and is `None` while opaque.
#[derive(Debug)]
pub(crate) struct StructTypeData {
    pub(crate) identity: StructIdentity,
    pub(crate) body: RefCell<Option<StructBody>>,
}

/// Which of LLVM's two struct-identity regimes a struct type belongs to.
///
/// Mirrors the `StructType::get` / `StructType::create` split. A *literal*
/// struct is structurally uniqued: `{i32}` written twice is one type. An
/// *identified* struct never unifies — `%a = type {i32}` and `%b = type {i32}`
/// are distinct, and so are `%0 = type {i32}` and `%1 = type {i32}`.
///
/// The name is not the discriminator. llvmkit used to spell literal-ness as
/// `name.is_none()`, which cannot represent an *anonymous identified* struct
/// — exactly what `%0 = type {i32}` is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum StructIdentity {
    Literal,
    Identified { name: Option<String> },
}

impl StructIdentity {
    /// The declared name, for an identified struct that has one.
    #[inline]
    pub(crate) fn name(&self) -> Option<&str> {
        match self {
            Self::Literal => None,
            Self::Identified { name } => name.as_deref(),
        }
    }

    #[inline]
    pub(crate) fn is_literal(&self) -> bool {
        matches!(self, Self::Literal)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StructBody {
    pub(crate) elements: Box<[TypeSlot]>,
    pub(crate) packed: bool,
}

#[derive(Debug)]
pub(crate) struct TargetExtTypeData {
    pub(crate) name: String,
    pub(crate) type_params: Box<[TypeSlot]>,
    pub(crate) int_params: Box<[u32]>,
}

// --------------------------------------------------------------------------
// Public handle
// --------------------------------------------------------------------------

/// Erased public handle for any IR type.
///
/// Two-field record: an arena index plus a brand-carrying module
/// reference. Equality and hashing compare the branded module reference by
/// [`ModuleId`](crate::ModuleId), so the handle remains cheap to copy and
/// store in maps.
pub struct Type<'ctx, B: ModuleBrand> {
    pub(crate) id: TypeSlot,
    pub(crate) module: ModuleRef<'ctx, B>,
}

impl<B: ModuleBrand> Clone for Type<'_, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: ModuleBrand> Copy for Type<'_, B> {}

impl<B: ModuleBrand> PartialEq for Type<'_, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module
    }
}

impl<B: ModuleBrand> Eq for Type<'_, B> {}

impl<B: ModuleBrand> Hash for Type<'_, B> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.module.hash(state);
    }
}

impl<B: ModuleBrand> fmt::Debug for Type<'_, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Type").field("id", &self.id).finish()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Type<'ctx, B> {
    /// Construct from raw parts. Crate-internal: a public Module method
    /// is the only path that hands out type handles.
    #[inline]
    pub(crate) fn new<M>(id: TypeSlot, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
        }
    }

    /// Borrow the underlying payload via the module's arena.
    #[inline]
    pub(crate) fn data(self) -> &'ctx TypeData {
        self.module.type_data(self.id)
    }

    /// Opaque arena id for structured side tables such as use-list order
    /// records.
    #[inline]
    pub fn id(self) -> TypeSlot {
        self.id
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// Require `got` to be exactly `self`, reporting the most precise
    /// error available when it is not.
    ///
    /// The crate's one answer to "this value claims a type it may not
    /// have". `self` is the *known-good* type — an operand's, a
    /// destination type's, a declared variable's — and `got` the runtime
    /// type of the value under suspicion.
    ///
    /// **Compares two runtime types, never a type against a marker.** That
    /// distinction is the whole point and callers must preserve it: a
    /// marker-keyed check (`W::narrow`) asks "is this *some* value of
    /// `W`", which for the erased [`IntDyn`](crate::int_width::IntDyn) /
    /// [`FloatDyn`](crate::float_kind::FloatDyn) markers means "any
    /// integer" / "any float" and would silently drop the width/kind
    /// check this performs. Reach for `narrow` only where the expected
    /// type is *statically* fixed (e.g. an icmp result is always `i1`),
    /// never where it comes from an operand or a declared variable.
    ///
    /// Two kinds get a dedicated error instead of
    /// [`IrError::TypeMismatch`], because [`TypeKindLabel`] collapses each
    /// of them into one variant that omits the only fact distinguishing
    /// the two sides:
    ///
    /// - **Integer width** drift reports
    ///   [`IrError::OperandWidthMismatch`]: `TypeKindLabel` has a
    ///   *single*, width-less `Integer` variant, so an i32-vs-i64 drift
    ///   would otherwise read "expected integer, got integer" — true, and
    ///   silent about the only fact that distinguishes them. This mirrors
    ///   the split `IntValue`'s `TryFrom<Value>` performs (`value.rs`),
    ///   for the same reason.
    /// - **Pointer address space** drift reports
    ///   [`IrError::AddressSpaceMismatch`], the identical argument applied
    ///   to the single, address-space-less `Pointer` variant. It needs its
    ///   own error rather than reusing `OperandWidthMismatch`: an address
    ///   space is not a width, and `lhs`/`rhs` bit counts would misname
    ///   what the numbers are.
    ///
    /// Both arms fire only when the two sides *share* the kind. A genuine
    /// cross-kind drift (`i32` vs `ptr`) falls through to the general arm,
    /// where the labels are already exactly the right amount of detail --
    /// never an `AddressSpaceMismatch` about an address space one side
    /// does not have. Float kinds need no special case either:
    /// `TypeKindLabel` has a distinct variant per kind
    /// (`Half`/`Float`/`Double`/…), so their `TypeMismatch` already names
    /// both sides precisely — they too fall through.
    ///
    /// No false rejections: `llvm_context.rs` memoizes `int_type(bits)` by
    /// width and `ptr_type(addr_space)` by address space, so `TypeSlot`
    /// equality is structural type equality — a correctly typed value
    /// always compares equal. Cost is one `TypeSlot` compare.
    pub(crate) fn require_match(self, got: Self) -> IrResult<()> {
        if self.id == got.id {
            return Ok(());
        }
        let (expected_data, got_data) = (self.data(), got.data());
        Err(match (expected_data.as_integer(), got_data.as_integer()) {
            (Some(lhs), Some(rhs)) => IrError::OperandWidthMismatch { lhs, rhs },
            _ => match (expected_data.as_pointer(), got_data.as_pointer()) {
                (Some(expected), Some(got)) => IrError::AddressSpaceMismatch { expected, got },
                _ => IrError::TypeMismatch {
                    expected: self.kind_label(),
                    got: got.kind_label(),
                },
            },
        })
    }

    /// Analysis-mode discriminator. Pattern-match here for read-only IR
    /// inspection.
    pub fn kind(self) -> TypeKind {
        match self.data() {
            TypeData::Void => TypeKind::Void,
            TypeData::Half => TypeKind::Half,
            TypeData::Bfloat => TypeKind::Bfloat,
            TypeData::Float => TypeKind::Float,
            TypeData::Double => TypeKind::Double,
            TypeData::X86Fp80 => TypeKind::X86Fp80,
            TypeData::Fp128 => TypeKind::Fp128,
            TypeData::PpcFp128 => TypeKind::PpcFp128,
            TypeData::X86Amx => TypeKind::X86Amx,
            TypeData::WasmExnRef => TypeKind::WasmExnRef,
            TypeData::Label => TypeKind::Label,
            TypeData::Metadata => TypeKind::Metadata,
            TypeData::Token => TypeKind::Token,
            TypeData::Integer { bits } => TypeKind::Integer { bits: *bits },
            TypeData::Pointer { addr_space } => TypeKind::Pointer {
                addr_space: *addr_space,
            },
            TypeData::Function { .. } => TypeKind::Function,
            TypeData::Array { .. } => TypeKind::Array,
            TypeData::FixedVector { .. } => TypeKind::FixedVector,
            TypeData::ScalableVector { .. } => TypeKind::ScalableVector,
            TypeData::Struct(_) => TypeKind::Struct,
            TypeData::TypedPointer { .. } => TypeKind::TypedPointer,
            TypeData::TargetExt(_) => TypeKind::TargetExt,
        }
    }

    /// `TypeKindLabel` for diagnostics.
    pub fn kind_label(self) -> TypeKindLabel {
        match self.data() {
            TypeData::Void => TypeKindLabel::Void,
            TypeData::Half => TypeKindLabel::Half,
            TypeData::Bfloat => TypeKindLabel::Bfloat,
            TypeData::Float => TypeKindLabel::Float,
            TypeData::Double => TypeKindLabel::Double,
            TypeData::X86Fp80 => TypeKindLabel::X86Fp80,
            TypeData::Fp128 => TypeKindLabel::Fp128,
            TypeData::PpcFp128 => TypeKindLabel::PpcFp128,
            TypeData::X86Amx => TypeKindLabel::X86Amx,
            TypeData::WasmExnRef => TypeKindLabel::WasmExnRef,
            TypeData::Label => TypeKindLabel::Label,
            TypeData::Metadata => TypeKindLabel::Metadata,
            TypeData::Token => TypeKindLabel::Token,
            TypeData::Integer { .. } => TypeKindLabel::Integer,
            TypeData::Pointer { .. } => TypeKindLabel::Pointer,
            TypeData::Function { .. } => TypeKindLabel::Function,
            TypeData::Array { .. } => TypeKindLabel::Array,
            TypeData::FixedVector { .. } => TypeKindLabel::FixedVector,
            TypeData::ScalableVector { .. } => TypeKindLabel::ScalableVector,
            TypeData::Struct(_) => TypeKindLabel::Struct,
            TypeData::TypedPointer { .. } => TypeKindLabel::TypedPointer,
            TypeData::TargetExt(_) => TypeKindLabel::TargetExt,
        }
    }

    // ---- LLVM-style predicates (`Type.h`) ----

    #[inline]
    pub fn is_void(self) -> bool {
        matches!(self.data(), TypeData::Void)
    }
    #[inline]
    pub fn is_integer(self) -> bool {
        matches!(self.data(), TypeData::Integer { .. })
    }
    #[inline]
    pub fn is_pointer(self) -> bool {
        matches!(self.data(), TypeData::Pointer { .. })
    }
    /// Legacy *typed* pointer (`i32*`), mirroring `TypedPointerType`. Distinct
    /// from [`Self::is_pointer`], which is the opaque `ptr` — a first-class data
    /// type just the same, so anywhere that accepts a pointer value (a `phi`
    /// result, say) must accept both.
    #[inline]
    pub fn is_typed_pointer(self) -> bool {
        matches!(self.data(), TypeData::TypedPointer { .. })
    }
    #[inline]
    pub fn is_function(self) -> bool {
        matches!(self.data(), TypeData::Function { .. })
    }
    #[inline]
    pub fn is_array(self) -> bool {
        matches!(self.data(), TypeData::Array { .. })
    }
    #[inline]
    pub fn is_struct(self) -> bool {
        matches!(self.data(), TypeData::Struct(_))
    }
    #[inline]
    pub fn is_label(self) -> bool {
        matches!(self.data(), TypeData::Label)
    }
    #[inline]
    pub fn is_metadata(self) -> bool {
        matches!(self.data(), TypeData::Metadata)
    }
    #[inline]
    pub fn is_token(self) -> bool {
        matches!(self.data(), TypeData::Token)
    }
    #[inline]
    pub fn is_target_ext(self) -> bool {
        matches!(self.data(), TypeData::TargetExt(_))
    }

    /// `true` for any of fixed / scalable vector. Mirrors `isVectorTy`.
    #[inline]
    pub fn is_vector(self) -> bool {
        matches!(
            self.data(),
            TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }
        )
    }

    /// Mirrors `isIEEELikeFPTy`.
    pub fn is_ieee_like_fp(self) -> bool {
        is_ieee_like_fp_data(self.data())
    }

    /// Mirrors `isFloatingPointTy`.
    pub fn is_floating_point(self) -> bool {
        is_floating_point_data(self.data())
    }

    /// Mirrors `isFPOrFPVectorTy` — a floating-point type, or a fixed or
    /// scalable vector whose element is one.
    ///
    /// This is **not** the predicate that decides whether a `call`, `select`
    /// or `phi` is an `FPMathOperator` and may therefore carry fast-math
    /// flags: `FPMathOperator::classof` asks
    /// [`is_supported_floating_point_type`](crate::operator::is_supported_floating_point_type),
    /// which also accepts a homogeneous floating-point aggregate. It *is* the
    /// predicate `parseCompare`'s `FCmp` arm and `parseAtomicRMW`'s
    /// floating-point-operand check ask.
    pub fn is_float_or_float_vector(self) -> bool {
        is_float_or_float_vector(self.module.module(), self.id)
    }

    /// Mirrors `isAggregateType`. Vectors are first-class but not
    /// aggregate per LangRef.
    pub fn is_aggregate(self) -> bool {
        matches!(self.data(), TypeData::Array { .. } | TypeData::Struct(_))
    }

    /// Mirrors `isSingleValueType`.
    pub fn is_single_value(self) -> bool {
        self.is_floating_point()
            || self.is_integer()
            || self.is_pointer()
            || self.is_vector()
            || matches!(self.data(), TypeData::X86Amx)
            || self.is_target_ext()
    }

    /// Mirrors `Type::isSized` together with `StructType::isSized`. Composite
    /// types recurse; opaque named structs remain unsized until their body is
    /// filled; a struct holding a scalable vector is unsized unless *every*
    /// element is that same scalable vector.
    ///
    /// Upstream's `Visited` set is an optional parameter that most callers
    /// leave null; `LLParser`'s `getelementptr` paths are among the few that
    /// supply one. llvmkit always threads it. On every constructible type the
    /// answer is identical — `StructType::checkBody` already rejects a body
    /// that reaches its own struct, so no cycle exists to find — but a
    /// predicate that recurses on its input should not rely on a guard in
    /// another file staying complete, and house law forbids the stack
    /// overflow that would follow if it ever did not.
    pub fn is_sized(self) -> bool {
        is_sized(self.module.module(), self.id, &mut Vec::new())
    }

    /// Mirrors `Type::isScalableTy`: whether this type *is* or *contains* a
    /// scalable vector, counting a target extension type whose layout type is
    /// one (`Type::isScalableTargetExtTy`).
    ///
    /// Distinct from `VectorType::is_scalable`, which asks only whether *that*
    /// vector is scalable. This walks array elements and struct bodies, so
    /// `[4 x { <vscale x 2 x i32> }]` answers `true`.
    pub fn is_scalable(self) -> bool {
        is_scalable(self.module.module(), self.id, &mut Vec::new())
    }

    /// Mirrors `Type::isFirstClassType`: every `TypeID` *except*
    /// `Function`, `Void`, and *opaque* identified structs.
    pub fn is_first_class(self) -> bool {
        match self.data() {
            TypeData::Function { .. } | TypeData::Void => false,
            TypeData::Struct(s) => s.body.borrow().is_some(),
            _ => true,
        }
    }

    // ---- Scalar / vector projection (`Type.h`) ----

    /// The element type of a vector, or `self` for anything else.
    /// Mirrors `Type::getScalarType`.
    pub fn scalar_type(self) -> Self {
        Type::new(scalar_type_slot(self.module.module(), self.id), self.module)
    }

    /// Element count of a vector — the *minimum* count for a scalable one, as
    /// `ElementCount` carries it. `None` for a non-vector.
    ///
    /// Comparing two of these also settles scalar-versus-vector agreement,
    /// which is how `CastInst::castIsValid` avoids a separate arm for it.
    pub fn vector_element_count(self) -> Option<u32> {
        match self.data() {
            TypeData::FixedVector { n, .. } => Some(*n),
            TypeData::ScalableVector { min, .. } => Some(*min),
            _ => None,
        }
    }

    /// Bit width of this type's *scalar* type, or 0 where LLVM has no answer.
    /// Mirrors `Type::getScalarSizeInBits`, whose zero return is the "not a
    /// sized primitive" signal the cast table relies on.
    pub fn scalar_size_in_bits(self) -> u32 {
        self.scalar_type().primitive_size_in_bits()
    }

    /// Mirrors `Type::getPrimitiveSizeInBits`: the bit width of a primitive
    /// or vector of primitives, and 0 for everything else. A scalable vector
    /// reports its minimum width, matching upstream's `TypeSize` in the
    /// contexts the parser uses this for.
    pub fn primitive_size_in_bits(self) -> u32 {
        match self.data() {
            TypeData::Half | TypeData::Bfloat => 16,
            TypeData::Float => 32,
            TypeData::Double => 64,
            TypeData::X86Fp80 => 80,
            TypeData::Fp128 | TypeData::PpcFp128 => 128,
            TypeData::X86Amx => 8192,
            TypeData::Integer { bits } => *bits,
            TypeData::FixedVector { elem, n } => Type::new(*elem, self.module)
                .primitive_size_in_bits()
                .saturating_mul(*n),
            TypeData::ScalableVector { elem, min } => Type::new(*elem, self.module)
                .primitive_size_in_bits()
                .saturating_mul(*min),
            _ => 0,
        }
    }

    /// Mirrors `Type::isIntOrIntVectorTy`.
    pub fn is_int_or_int_vector(self) -> bool {
        is_int_or_int_vector(self.module.module(), self.id)
    }

    /// Mirrors `Type::isIntegerTy(unsigned BitWidth)`.
    #[inline]
    pub fn is_integer_of_width(self, bit_width: u32) -> bool {
        matches!(self.data(), TypeData::Integer { bits } if *bits == bit_width)
    }

    /// Mirrors `Type::isIntOrIntVectorTy(unsigned BitWidth)` — the width-taking
    /// overload, `getScalarType()->isIntegerTy(BitWidth)`. `i1`/`<N x i1>` is
    /// the width every caller here asks for, and is what `m_LogicalOp`'s
    /// `LogicalOp_match` and `isImpliedCondition`'s entry assertion require.
    #[inline]
    pub fn is_int_or_int_vector_of_width(self, bit_width: u32) -> bool {
        self.scalar_type().is_integer_of_width(bit_width)
    }

    /// Mirrors `Type::isPtrOrPtrVectorTy`.
    pub fn is_ptr_or_ptr_vector(self) -> bool {
        is_ptr_or_ptr_vector(self.module.module(), self.id)
    }

    /// Address space of a pointer type, or `None` if this is not one.
    /// Mirrors `PointerType::getAddressSpace` with the null check folded in
    /// (design law 3: the `dyn_cast` plus null test becomes an `Option`).
    pub fn pointer_address_space(self) -> Option<u32> {
        match self.data() {
            TypeData::Pointer { addr_space } => Some(*addr_space),
            TypeData::TypedPointer { addr_space, .. } => Some(*addr_space),
            _ => None,
        }
    }

    // ---- Element / shape validity (`Type.cpp`) ----
    //
    // Deny-lists, not allow-lists, except for vectors — reproduced in that
    // shape so a type kind added later keeps upstream's default answer.

    /// Mirrors `StructType::isValidElementType`.
    pub fn is_valid_struct_element(self) -> bool {
        !matches!(
            self.data(),
            TypeData::Void
                | TypeData::Label
                | TypeData::Metadata
                | TypeData::Function { .. }
                | TypeData::Token
        )
    }

    /// Mirrors `ArrayType::isValidElementType`, which additionally denies
    /// `x86_amx` where the struct predicate does not.
    pub fn is_valid_array_element(self) -> bool {
        self.is_valid_struct_element() && !matches!(self.data(), TypeData::X86Amx)
    }

    /// Mirrors `VectorType::isValidElementType` — the one *allow*-list in the
    /// family: integers, floats, pointers, and target extension types that
    /// declare `CanBeVectorElement`.
    pub fn is_valid_vector_element(self) -> bool {
        match self.data() {
            TypeData::Integer { .. } | TypeData::Pointer { .. } | TypeData::TypedPointer { .. } => {
                true
            }
            TypeData::TargetExt(_) => crate::derived_types::TargetExtType::try_from(self)
                .is_ok_and(|t| {
                    t.has_property(crate::derived_types::TargetExtProperty::CanBeVectorElement)
                }),
            _ => self.is_floating_point(),
        }
    }

    /// Mirrors `PointerType::isValidElementType`. Only reachable through
    /// legacy typed-pointer syntax; opaque `ptr` has no element type.
    pub fn is_valid_pointer_element(self) -> bool {
        !matches!(
            self.data(),
            TypeData::Void
                | TypeData::Label
                | TypeData::Metadata
                | TypeData::Token
                | TypeData::X86Amx
        )
    }

    /// Mirrors `FunctionType::isValidReturnType`.
    pub fn is_valid_function_return(self) -> bool {
        !matches!(
            self.data(),
            TypeData::Function { .. } | TypeData::Label | TypeData::Metadata
        )
    }

    /// Mirrors `FunctionType::isValidArgumentType`.
    pub fn is_valid_function_argument(self) -> bool {
        self.is_first_class() && !self.is_label()
    }
}

/// Public discriminator for analysis-mode pattern matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TypeKind {
    Void,
    Half,
    Bfloat,
    Float,
    Double,
    X86Fp80,
    Fp128,
    PpcFp128,
    X86Amx,
    WasmExnRef,
    Label,
    Metadata,
    Token,
    Integer { bits: u32 },
    Pointer { addr_space: u32 },
    Function,
    Array,
    FixedVector,
    ScalableVector,
    Struct,
    TypedPointer,
    TargetExt,
}

// --------------------------------------------------------------------------
// Display
// --------------------------------------------------------------------------

impl<'ctx, B: ModuleBrand> fmt::Display for Type<'ctx, B> {
    /// IR-textual form. Placeholder until the full `AsmWriter.cpp` port
    /// lands; deterministic but not a faithful reproduction of every
    /// LLVM corner case (notably padding/alignment annotations).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.data() {
            TypeData::Void => f.write_str("void"),
            TypeData::Half => f.write_str("half"),
            TypeData::Bfloat => f.write_str("bfloat"),
            TypeData::Float => f.write_str("float"),
            TypeData::Double => f.write_str("double"),
            TypeData::X86Fp80 => f.write_str("x86_fp80"),
            TypeData::Fp128 => f.write_str("fp128"),
            TypeData::PpcFp128 => f.write_str("ppc_fp128"),
            TypeData::X86Amx => f.write_str("x86_amx"),
            TypeData::WasmExnRef => f.write_str("exnref"),
            TypeData::Label => f.write_str("label"),
            TypeData::Metadata => f.write_str("metadata"),
            TypeData::Token => f.write_str("token"),
            TypeData::Integer { bits } => write!(f, "i{bits}"),
            // `case Type::PointerTyID:` in `TypePrinting::print` — `OS <<
            // "ptr"` then `printAddressSpace(M, PTy->getAddressSpace(), OS)`
            // with the routine's default prefix `" "` and empty suffix. Shared
            // with the four other call sites so the unported
            // `PrintAddrspaceName` branch stays a one-place fix.
            TypeData::Pointer { addr_space } => {
                f.write_str("ptr")?;
                crate::asm_writer::print_address_space(f, *addr_space, " ", "", false)
            }
            TypeData::Function {
                ret,
                params,
                is_var_arg,
            } => {
                let m = self.module;
                write!(f, "{} (", Type::new(*ret, m))?;
                let mut first = true;
                for p in params.iter() {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    write!(f, "{}", Type::new(*p, m))?;
                }
                if *is_var_arg {
                    if !first {
                        f.write_str(", ")?;
                    }
                    f.write_str("...")?;
                }
                f.write_str(")")
            }
            TypeData::Array { elem, n } => {
                write!(f, "[{n} x {}]", Type::new(*elem, self.module))
            }
            TypeData::FixedVector { elem, n } => {
                write!(f, "<{n} x {}>", Type::new(*elem, self.module))
            }
            TypeData::ScalableVector { elem, min } => {
                write!(f, "<vscale x {min} x {}>", Type::new(*elem, self.module))
            }
            TypeData::Struct(s) => {
                // An identified struct prints as its *reference*, never as its
                // body: `%name`, or `%N` for an anonymous one, whose number is
                // its position among the module's anonymous identified structs
                // (`TypePrinting::NumberedTypes`). Only a literal struct spells
                // its body inline.
                match &s.identity {
                    StructIdentity::Identified { name: Some(name) } => {
                        return write!(f, "%{name}");
                    }
                    StructIdentity::Identified { name: None } => {
                        return match self
                            .module
                            .module()
                            .context()
                            .anonymous_identified_struct_number(self.id)
                        {
                            Some(number) => write!(f, "%{number}"),
                            None => f.write_str("%<unnumbered>"),
                        };
                    }
                    StructIdentity::Literal => {}
                }
                let body = s.body.borrow();
                let body = body.as_ref().expect("literal struct must have body");
                let m = self.module;
                if body.packed {
                    f.write_str("<{ ")?;
                } else {
                    f.write_str("{ ")?;
                }
                let mut first = true;
                for e in body.elements.iter() {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    write!(f, "{}", Type::new(*e, m))?;
                }
                if body.packed {
                    f.write_str(" }>")
                } else {
                    f.write_str(" }")
                }
            }
            TypeData::TypedPointer {
                pointee,
                addr_space: 0,
            } => write!(f, "{}*", Type::<B>::new(*pointee, self.module.module())),
            TypeData::TypedPointer {
                pointee,
                addr_space,
            } => write!(
                f,
                "{} addrspace({addr_space})*",
                Type::<B>::new(*pointee, self.module.module())
            ),
            TypeData::TargetExt(t) => {
                write!(f, "target(\"{}\"", t.name)?;
                let m = self.module.module();
                for tp in t.type_params.iter() {
                    write!(f, ", {}", Type::<B>::new(*tp, m))?;
                }
                for ip in t.int_params.iter() {
                    write!(f, ", {ip}")?;
                }
                f.write_str(")")
            }
        }
    }
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/// `Type::getScalarType` one layer below [`Type::scalar_type`].
///
/// The scalar/vector projection family needs a slot-level form because
/// `constants.rs`, `verifier.rs` and `value_tracking.rs` reach it from inside
/// routines that hold a `&ModuleCore` and a `TypeSlot` and cannot construct a
/// `Type` view. These four are the one implementation of each predicate; the
/// `Type` methods above are thin wrappers, not second copies.
pub(crate) fn scalar_type_slot(module: &ModuleCore, id: TypeSlot) -> TypeSlot {
    match module.context().type_data(id) {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => *elem,
        _ => id,
    }
}

/// `Type::isIntOrIntVectorTy`, at the slot layer.
pub(crate) fn is_int_or_int_vector(module: &ModuleCore, id: TypeSlot) -> bool {
    matches!(
        module.context().type_data(scalar_type_slot(module, id)),
        TypeData::Integer { .. }
    )
}

/// `Type::isPtrOrPtrVectorTy`, at the slot layer.
pub(crate) fn is_ptr_or_ptr_vector(module: &ModuleCore, id: TypeSlot) -> bool {
    matches!(
        module.context().type_data(scalar_type_slot(module, id)),
        TypeData::Pointer { .. }
    )
}

/// `Type::isFPOrFPVectorTy`, at the slot layer.
pub(crate) fn is_float_or_float_vector(module: &ModuleCore, id: TypeSlot) -> bool {
    is_floating_point_data(module.context().type_data(scalar_type_slot(module, id)))
}

/// `Type::isIEEELikeFPTy` against the payload, so [`Type::is_ieee_like_fp`] and
/// the slot-layer predicates share one list rather than restating it.
pub(crate) fn is_ieee_like_fp_data(data: &TypeData) -> bool {
    matches!(
        data,
        TypeData::Half | TypeData::Bfloat | TypeData::Float | TypeData::Double | TypeData::Fp128
    )
}

/// `Type::isFloatingPointTy` against the payload.
pub(crate) fn is_floating_point_data(data: &TypeData) -> bool {
    is_ieee_like_fp_data(data) || matches!(data, TypeData::X86Fp80 | TypeData::PpcFp128)
}

fn is_sized(module: &ModuleCore, id: TypeSlot, visited: &mut Vec<TypeSlot>) -> bool {
    let data = module.context().type_data(id);
    match data {
        TypeData::Void
        | TypeData::Label
        | TypeData::Metadata
        | TypeData::Token
        | TypeData::WasmExnRef
        | TypeData::Function { .. } => false,
        TypeData::Half
        | TypeData::Bfloat
        | TypeData::Float
        | TypeData::Double
        | TypeData::X86Fp80
        | TypeData::Fp128
        | TypeData::PpcFp128
        | TypeData::X86Amx
        | TypeData::Integer { .. }
        | TypeData::Pointer { .. }
        | TypeData::TypedPointer { .. } => true,
        TypeData::Array { elem, .. }
        | TypeData::FixedVector { elem, .. }
        | TypeData::ScalableVector { elem, .. } => is_sized(module, *elem, visited),
        // `Type::isSizedDerivedType` asks the *layout* type, so an extension
        // with no layout — upstream's `void` default, llvmkit's `None` — is
        // unsized rather than trivially sized.
        TypeData::TargetExt(_) => crate::data_layout::target_ext_layout_type(module, id)
            .is_some_and(|layout| is_sized(module, layout, visited)),
        TypeData::Struct(_) => struct_is_sized(module, id, visited),
    }
}

/// Mirrors `StructType::isSized`: an opaque body is unsized, a cycle answers
/// `false`, and an element that is or contains a scalable vector makes the
/// struct unsized — unless the body is homogeneously that scalable vector,
/// which upstream special-cases before the loop.
fn struct_is_sized(module: &ModuleCore, id: TypeSlot, visited: &mut Vec<TypeSlot>) -> bool {
    let TypeData::Struct(data) = module.context().type_data(id) else {
        return false;
    };
    let borrowed = data.body.borrow();
    let Some(body) = borrowed.as_ref() else {
        return false;
    };
    if visited.contains(&id) {
        return false;
    }
    visited.push(id);
    if contains_homogeneous_scalable_vector_types(module, &body.elements) {
        return true;
    }
    body.elements.iter().all(|elem| {
        // Upstream asks `isScalableTy()` with a *fresh* visited set here.
        !is_scalable(module, *elem, &mut Vec::new()) && is_sized(module, *elem, visited)
    })
}

/// Mirrors `StructType::containsHomogeneousTypes`:
/// `!ElementTys.empty() && all_equal(ElementTys)`. Types are uniqued, so
/// `all_equal` is slot equality.
pub(crate) fn contains_homogeneous_types(elements: &[TypeSlot]) -> bool {
    let Some(first) = elements.first() else {
        return false;
    };
    elements.iter().all(|elem| elem == first)
}

/// Mirrors `StructType::containsHomogeneousScalableVectorTypes`, which is a
/// first-element test followed by a `containsHomogeneousTypes()` call — so
/// this is that call, not a second copy of its body.
fn contains_homogeneous_scalable_vector_types(module: &ModuleCore, elements: &[TypeSlot]) -> bool {
    let Some(first) = elements.first() else {
        return false;
    };
    if !matches!(
        module.context().type_data(*first),
        TypeData::ScalableVector { .. }
    ) {
        return false;
    }
    contains_homogeneous_types(elements)
}

/// Mirrors `Type::isScalableTy` and `StructType::isScalableTy`, including
/// `Type::isScalableTargetExtTy`. Note that upstream does *not* recurse
/// through a fixed vector's element type here — a fixed vector of scalable
/// vectors is unrepresentable — so neither does this.
fn is_scalable(module: &ModuleCore, id: TypeSlot, visited: &mut Vec<TypeSlot>) -> bool {
    match module.context().type_data(id) {
        TypeData::Array { elem, .. } => is_scalable(module, *elem, visited),
        TypeData::Struct(data) => {
            if visited.contains(&id) {
                return false;
            }
            visited.push(id);
            let borrowed = data.body.borrow();
            borrowed.as_ref().is_some_and(|body| {
                body.elements
                    .iter()
                    .any(|elem| is_scalable(module, *elem, visited))
            })
        }
        TypeData::ScalableVector { .. } => true,
        TypeData::TargetExt(_) => crate::data_layout::target_ext_layout_type(module, id)
            .is_some_and(|layout| {
                matches!(
                    module.context().type_data(layout),
                    TypeData::ScalableVector { .. }
                )
            }),
        _ => false,
    }
}

// --------------------------------------------------------------------------
// Sealed marker trait
// --------------------------------------------------------------------------

/// Sealed crate-private supertrait. The empty trait pattern from Rust
/// API Guidelines `C-SEALED`: external crates cannot implement
/// [`IrType`] (or any other sealed trait in this crate) so the kind set
/// stays closed and we can add trait methods non-breakingly.
pub(crate) mod sealed {
    pub trait Sealed {}
}

/// Marker trait implemented by every typed type-handle (`IntType`,
/// `FloatType`, `PointerType`, ..., plus the erased [`Type`] itself).
///
/// Sealed: the closed set of LLVM type kinds is part of the IR spec,
/// not an extension point. Bound generic code with `T: IrType<'ctx>`
/// when a function should accept any type without enumerating every
/// concrete handle.
pub trait IrType<'ctx, B: ModuleBrand>: sealed::Sealed + Copy + Sized + core::fmt::Debug {
    /// Widen to the erased [`Type`] handle.
    fn as_type(self) -> Type<'ctx, B>;
}

impl<'ctx, B: ModuleBrand> sealed::Sealed for Type<'ctx, B> {}
impl<'ctx, B: ModuleBrand> IrType<'ctx, B> for Type<'ctx, B> {
    #[inline]
    fn as_type(self) -> Type<'ctx, B> {
        self
    }
}
