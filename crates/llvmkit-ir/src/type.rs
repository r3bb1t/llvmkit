//! IR type system. Mirrors `llvm/include/llvm/IR/Type.h` and
//! `llvm/lib/IR/Type.cpp`.
//!
//! ## Representation
//!
//! Storage is index-based: every interned type lives in the owning
//! module's interning context, identified by a crate-internal `TypeId`
//! (a `NonZeroU32` newtype). Children are also `TypeId`s, so the
//! storage payload `TypeData` is lifetime-free, has trivial `Hash`/`Eq`
//! derivability at the storage layer, and never participates in
//! pointer comparisons.
//!
//! Per the IR foundation plan (Pivot 1, "dual-view"):
//!
//! - **Storage:** an internal `TypeData` enum, one variant
//!   per LLVM `TypeID`.
//! - **Public handle:** [`Type`] is `(TypeId, ModuleRef<'ctx>)`. Both
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
use core::num::NonZeroU32;

use crate::TypeKindLabel;
use crate::module::{Module, ModuleRef};

/// Minimum legal integer width. Mirrors `IntegerType::MIN_INT_BITS`
/// (`DerivedTypes.h`).
pub const MIN_INT_BITS: u32 = 1;

/// Maximum legal integer width. Mirrors `IntegerType::MAX_INT_BITS`
/// (`DerivedTypes.h`). Equals `1 << 23` (8 388 608).
pub const MAX_INT_BITS: u32 = 1 << 23;

// --------------------------------------------------------------------------
// Type id
// --------------------------------------------------------------------------

/// Crate-internal index into the type arena. `NonZeroU32` so
/// `Option<TypeId>` stays 4 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TypeId(NonZeroU32);

impl TypeId {
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
        // Subtraction is sound: every TypeId was produced by `from_index`,
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
/// [`TypeId`] indices into the same module's arena.
#[derive(Debug)]
pub(crate) enum TypeData {
    // ---- Primitive / sized-but-childless ----
    Void,
    Half,
    BFloat,
    Float,
    Double,
    X86Fp80,
    Fp128,
    PpcFp128,
    X86Amx,
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
        ret: TypeId,
        params: Box<[TypeId]>,
        is_var_arg: bool,
    },
    Array {
        elem: TypeId,
        n: u64,
    },
    FixedVector {
        elem: TypeId,
        n: u32,
    },
    ScalableVector {
        elem: TypeId,
        min: u32,
    },
    Struct(StructTypeData),
    /// Typed pointer (legacy, only used by a few GPU targets in LLVM 22).
    /// Mirrors `TypedPointerType` (`TypedPointerType.h`).
    TypedPointer {
        pointee: TypeId,
        addr_space: u32,
    },
    TargetExt(TargetExtTypeData),
}

impl TypeData {
    // ---- Per-variant projection helpers ----
    //
    // Every typed handle (IntType, ArrayType, ...) wraps a `TypeId` whose
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
    pub(crate) fn as_array(&self) -> Option<(TypeId, u64)> {
        if let Self::Array { elem, n } = *self {
            Some((elem, n))
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn as_vector(&self) -> Option<(TypeId, u32, bool)> {
        match *self {
            Self::FixedVector { elem, n } => Some((elem, n, false)),
            Self::ScalableVector { elem, min } => Some((elem, min, true)),
            _ => None,
        }
    }
    #[inline]
    pub(crate) fn as_function(&self) -> Option<(TypeId, &[TypeId], bool)> {
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
    pub(crate) fn as_typed_pointer(&self) -> Option<(TypeId, u32)> {
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

/// Payload for any struct type — both literal and named.
///
/// `name = None` distinguishes literal structs (whose `body` is set at
/// creation and never changes) from identified ones (whose body may be
/// filled in later via `set_struct_body`, and is `None` while opaque).
#[derive(Debug)]
pub(crate) struct StructTypeData {
    pub(crate) name: Option<String>,
    pub(crate) body: RefCell<Option<StructBody>>,
}

#[derive(Debug, Clone)]
pub(crate) struct StructBody {
    pub(crate) elements: Box<[TypeId]>,
    pub(crate) packed: bool,
}

#[derive(Debug)]
pub(crate) struct TargetExtTypeData {
    pub(crate) name: String,
    pub(crate) type_params: Box<[TypeId]>,
    pub(crate) int_params: Box<[u32]>,
}

// --------------------------------------------------------------------------
// Public handle
// --------------------------------------------------------------------------

/// Erased public handle for any IR type.
///
/// Two-field record: an arena index plus a brand-carrying module
/// reference. The `ModuleRef<'ctx>` helper makes the `Module` field
/// equality-and-hash-by-`ModuleId`, so the entire handle derives
/// `PartialEq + Eq + Hash + Debug + Copy + Clone` without any hand-rolled
/// impls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Type<'ctx> {
    pub(crate) id: TypeId,
    pub(crate) module: ModuleRef<'ctx>,
}

impl<'ctx> Type<'ctx> {
    /// Construct from raw parts. Crate-internal: a public Module method
    /// is the only path that hands out type handles.
    #[inline]
    pub(crate) fn new(id: TypeId, module: &'ctx Module<'ctx>) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
        }
    }

    /// Borrow the underlying payload via the module's arena.
    #[inline]
    pub(crate) fn data(self) -> &'ctx TypeData {
        self.module.type_data(self.id)
    }

    /// Crate-internal id accessor.
    #[inline]
    pub(crate) fn id(self) -> TypeId {
        self.id
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// Analysis-mode discriminator. Pattern-match here for read-only IR
    /// inspection.
    pub fn kind(self) -> TypeKind {
        match self.data() {
            TypeData::Void => TypeKind::Void,
            TypeData::Half => TypeKind::Half,
            TypeData::BFloat => TypeKind::BFloat,
            TypeData::Float => TypeKind::Float,
            TypeData::Double => TypeKind::Double,
            TypeData::X86Fp80 => TypeKind::X86Fp80,
            TypeData::Fp128 => TypeKind::Fp128,
            TypeData::PpcFp128 => TypeKind::PpcFp128,
            TypeData::X86Amx => TypeKind::X86Amx,
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
            TypeData::BFloat => TypeKindLabel::BFloat,
            TypeData::Float => TypeKindLabel::Float,
            TypeData::Double => TypeKindLabel::Double,
            TypeData::X86Fp80 => TypeKindLabel::X86Fp80,
            TypeData::Fp128 => TypeKindLabel::Fp128,
            TypeData::PpcFp128 => TypeKindLabel::PpcFp128,
            TypeData::X86Amx => TypeKindLabel::X86Amx,
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
        matches!(
            self.data(),
            TypeData::Half
                | TypeData::BFloat
                | TypeData::Float
                | TypeData::Double
                | TypeData::Fp128
        )
    }

    /// Mirrors `isFloatingPointTy`.
    pub fn is_floating_point(self) -> bool {
        self.is_ieee_like_fp() || matches!(self.data(), TypeData::X86Fp80 | TypeData::PpcFp128)
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

    /// Mirrors `isSized`. Composite types recurse; opaque named structs
    /// remain unsized until their body is filled.
    pub fn is_sized(self) -> bool {
        is_sized(self.module.module(), self.id)
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
}

/// Public discriminator for analysis-mode pattern matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TypeKind {
    Void,
    Half,
    BFloat,
    Float,
    Double,
    X86Fp80,
    Fp128,
    PpcFp128,
    X86Amx,
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

impl<'ctx> fmt::Display for Type<'ctx> {
    /// IR-textual form. Placeholder until the full `AsmWriter.cpp` port
    /// lands; deterministic but not a faithful reproduction of every
    /// LLVM corner case (notably padding/alignment annotations).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.data() {
            TypeData::Void => f.write_str("void"),
            TypeData::Half => f.write_str("half"),
            TypeData::BFloat => f.write_str("bfloat"),
            TypeData::Float => f.write_str("float"),
            TypeData::Double => f.write_str("double"),
            TypeData::X86Fp80 => f.write_str("x86_fp80"),
            TypeData::Fp128 => f.write_str("fp128"),
            TypeData::PpcFp128 => f.write_str("ppc_fp128"),
            TypeData::X86Amx => f.write_str("x86_amx"),
            TypeData::Label => f.write_str("label"),
            TypeData::Metadata => f.write_str("metadata"),
            TypeData::Token => f.write_str("token"),
            TypeData::Integer { bits } => write!(f, "i{bits}"),
            TypeData::Pointer { addr_space: 0 } => f.write_str("ptr"),
            TypeData::Pointer { addr_space } => write!(f, "ptr addrspace({addr_space})"),
            TypeData::Function {
                ret,
                params,
                is_var_arg,
            } => {
                let m = self.module.module();
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
                write!(f, "[{n} x {}]", Type::new(*elem, self.module.module()))
            }
            TypeData::FixedVector { elem, n } => {
                write!(f, "<{n} x {}>", Type::new(*elem, self.module.module()))
            }
            TypeData::ScalableVector { elem, min } => {
                write!(
                    f,
                    "<vscale x {min} x {}>",
                    Type::new(*elem, self.module.module())
                )
            }
            TypeData::Struct(s) => {
                if let Some(name) = &s.name {
                    return write!(f, "%{name}");
                }
                let body = s.body.borrow();
                let body = body.as_ref().expect("literal struct must have body");
                let m = self.module.module();
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
            } => write!(f, "{}*", Type::new(*pointee, self.module.module())),
            TypeData::TypedPointer {
                pointee,
                addr_space,
            } => write!(
                f,
                "{} addrspace({addr_space})*",
                Type::new(*pointee, self.module.module())
            ),
            TypeData::TargetExt(t) => {
                write!(f, "target(\"{}\"", t.name)?;
                let m = self.module.module();
                for tp in t.type_params.iter() {
                    write!(f, ", {}", Type::new(*tp, m))?;
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

fn is_sized(module: &Module<'_>, id: TypeId) -> bool {
    let data = module.context().type_data(id);
    match data {
        TypeData::Void
        | TypeData::Label
        | TypeData::Metadata
        | TypeData::Token
        | TypeData::Function { .. } => false,
        TypeData::Half
        | TypeData::BFloat
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
        | TypeData::ScalableVector { elem, .. } => is_sized(module, *elem),
        TypeData::Struct(s) => match s.body.borrow().as_ref() {
            None => false,
            Some(body) => body.elements.iter().all(|e| is_sized(module, *e)),
        },
        TypeData::TargetExt(_) => true,
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
pub trait IrType<'ctx>: sealed::Sealed + Copy + Sized + core::fmt::Debug {
    /// Widen to the erased [`Type`] handle.
    fn as_type(self) -> Type<'ctx>;
}

impl<'ctx> sealed::Sealed for Type<'ctx> {}
impl<'ctx> IrType<'ctx> for Type<'ctx> {
    #[inline]
    fn as_type(self) -> Type<'ctx> {
        self
    }
}
