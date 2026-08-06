//! Generic [`Constant`] handle plus the storage payload for every
//! constant kind. Mirrors `llvm/include/llvm/IR/Constant.h` and the
//! constant-data subset of `llvm/include/llvm/IR/Constants.h`.
//!
//! ## Storage shape
//!
//! Like the type-data layer (`crate::Type`'s storage), the constant
//! storage payload is lifetime-free: every cross-reference is a value-
//! arena index into the same
//! module's value arena. Per-kind refinement handles
//! ([`ConstantIntValue`], [`ConstantFloatValue`], ...) live in
//! [`crate::constants`] and follow the same `(ValueSlot, ModuleRef, ty:
//! TypeSlot)` layout as the value handles.
//!
//! ## What's shipped
//!
//! - `Int(magnitude_words)` — arbitrary-precision integer.
//! - `Float(bit_pattern)` — IEEE bit pattern.
//! - `PointerNull` — `ptr null` / `null` for typed pointers.
//! - `Aggregate(elements)` — `ConstantArray`, `ConstantStruct`,
//!   `ConstantVector` element list.
//! - `Undef` / `Poison` — kind-erased markers.
//!
//! The represented LLVM 22.1.4 constant subset includes parser-needed
//! `ConstantExpr` opcodes; unsupported legacy `ConstantExpr` opcodes remain
//! parser errors.
//!

//! [`ConstantIntValue`]: crate::constants::ConstantIntValue
//! [`ConstantFloatValue`]: crate::constants::ConstantFloatValue

use super::derived_types::{FloatType, IntType};
use super::error::ValueCategoryLabel;
use super::float_kind::IntoConstantFloat;
use super::int_width::IntoConstantInt;
use super::module::ModuleBrand;
use super::value::ValueKindData;
use crate::Branded;
use crate::ap_int::ApInt;
use crate::gep_no_wrap_flags::GepNoWrapFlags;
use crate::module::{Module, ModuleRef, Unverified};
use crate::r#type::{Type, TypeKind, TypeSlot};
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueSlot, sealed};
use crate::{DebugLoc, IrError, IrResult};

/// Opcode carried by a parser-needed LLVM `ConstantExpr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstantExprOpcode {
    Add,
    Sub,
    Xor,
    GetElementPtr,
    ShuffleVector,
    InsertElement,
    ExtractElement,
    Trunc,
    PtrToAddr,
    PtrToInt,
    IntToPtr,
    BitCast,
    AddrSpaceCast,
}

impl ConstantExprOpcode {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Xor => "xor",
            Self::GetElementPtr => "getelementptr",
            Self::ShuffleVector => "shufflevector",
            Self::InsertElement => "insertelement",
            Self::ExtractElement => "extractelement",
            Self::Trunc => "trunc",
            Self::PtrToAddr => "ptrtoaddr",
            Self::PtrToInt => "ptrtoint",
            Self::IntToPtr => "inttoptr",
            Self::BitCast => "bitcast",
            Self::AddrSpaceCast => "addrspacecast",
        }
    }

    pub(crate) fn is_cast(self) -> bool {
        matches!(
            self,
            Self::Trunc
                | Self::PtrToAddr
                | Self::PtrToInt
                | Self::IntToPtr
                | Self::BitCast
                | Self::AddrSpaceCast
        )
    }
}

/// No-wrap flags accepted by LLVM 22's `add`/`sub` constant-expression parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct OverflowingConstantExprFlags {
    nuw: bool,
    nsw: bool,
}

impl OverflowingConstantExprFlags {
    #[inline]
    pub const fn new(nuw: bool, nsw: bool) -> Self {
        Self { nuw, nsw }
    }

    #[inline]
    pub const fn none() -> Self {
        Self::new(false, false)
    }

    #[inline]
    pub const fn nuw(self) -> bool {
        self.nuw
    }

    #[inline]
    pub const fn nsw(self) -> bool {
        self.nsw
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        !self.nuw && !self.nsw
    }
}

/// APInt half-open range attached to a constant `getelementptr`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstantExprInRange {
    start: Box<[u64]>,
    end: Box<[u64]>,
    bit_width: u32,
}

impl ConstantExprInRange {
    #[inline]
    pub fn new<Start, End>(start: Start, end: End, bit_width: u32) -> Self
    where
        Start: Into<Box<[u64]>>,
        End: Into<Box<[u64]>>,
    {
        Self {
            start: start.into(),
            end: end.into(),
            bit_width,
        }
    }

    #[inline]
    pub fn start(&self) -> &[u64] {
        &self.start
    }

    #[inline]
    pub fn end(&self) -> &[u64] {
        &self.end
    }

    #[inline]
    pub const fn bit_width(&self) -> u32 {
        self.bit_width
    }

    #[inline]
    pub(crate) fn into_parts(self) -> (Box<[u64]>, Box<[u64]>, u32) {
        (self.start, self.end, self.bit_width)
    }
}

/// Flags accepted by LLVM 22's `getelementptr` constant-expression parser.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ConstantGepFlags {
    no_wrap: GepNoWrapFlags,
    in_range: Option<ConstantExprInRange>,
}

impl ConstantGepFlags {
    #[inline]
    pub fn new(no_wrap: GepNoWrapFlags) -> Self {
        Self {
            no_wrap: GepNoWrapFlags::from_bits_canonical(no_wrap.bits()),
            in_range: None,
        }
    }

    #[inline]
    pub fn with_in_range(no_wrap: GepNoWrapFlags, in_range: ConstantExprInRange) -> Self {
        Self {
            no_wrap: GepNoWrapFlags::from_bits_canonical(no_wrap.bits()),
            in_range: Some(in_range),
        }
    }

    #[inline]
    pub const fn no_wrap(&self) -> GepNoWrapFlags {
        self.no_wrap
    }

    #[inline]
    pub fn in_range(&self) -> Option<&ConstantExprInRange> {
        self.in_range.as_ref()
    }

    #[inline]
    pub(crate) fn into_parts(self) -> (GepNoWrapFlags, Option<ConstantExprInRange>) {
        (self.no_wrap, self.in_range)
    }
}

/// Optional optimization and predicate flags attached to a constant expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum ConstantExprFlags {
    #[default]
    None,
    Overflowing(OverflowingConstantExprFlags),
    Gep(ConstantGepFlags),
}

impl ConstantExprFlags {
    pub const fn none() -> Self {
        Self::None
    }
    pub const fn overflowing(nuw: bool, nsw: bool) -> Self {
        let flags = OverflowingConstantExprFlags::new(nuw, nsw);
        if flags.is_empty() {
            Self::None
        } else {
            Self::Overflowing(flags)
        }
    }

    pub fn gep(no_wrap: GepNoWrapFlags) -> Self {
        Self::gep_raw(no_wrap, None)
    }

    pub fn gep_with_in_range(no_wrap: GepNoWrapFlags, in_range: ConstantExprInRange) -> Self {
        Self::gep_raw(no_wrap, Some(in_range))
    }

    pub(crate) fn gep_raw(no_wrap: GepNoWrapFlags, in_range: Option<ConstantExprInRange>) -> Self {
        let no_wrap = GepNoWrapFlags::from_bits_canonical(no_wrap.bits());
        if no_wrap.is_empty() && in_range.is_none() {
            Self::None
        } else {
            Self::Gep(ConstantGepFlags { no_wrap, in_range })
        }
    }
}

/// Lifetime-free payload for a `ConstantExpr`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ConstantExprData {
    pub(crate) opcode: ConstantExprOpcode,
    pub(crate) result_ty: TypeSlot,
    pub(crate) source_ty: Option<TypeSlot>,
    pub(crate) operands: Box<[ValueSlot]>,
    pub(crate) indices: Box<[u32]>,
    pub(crate) mask: Box<[i32]>,
    pub(crate) flags: ConstantExprFlags,
}

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload stored in the value arena under
/// [`ValueKindData::Constant`](crate::value::ValueKindData::Constant).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ConstantData {
    /// Parser-needed LLVM `ConstantExpr` storage.
    Expr(ConstantExprData),
    /// Arbitrary-precision integer. Magnitude words are little-endian
    /// (`words[0]` is the least significant 64-bit limb), normalised so
    /// trailing zero limbs are stripped. The sign is encoded by the
    /// owning [`IntType`](crate::IntType): two's-complement
    /// representation in `bit_width` bits is materialised via
    /// `ConstantIntValue::value_zext_u128` / `value_sext_i128`.
    Int(Box<[u64]>),
    /// IEEE bit pattern. Width is determined by the value's
    /// `FloatType`. Stored as a `u128` so every IEEE width up to
    /// `fp128` fits without a discriminant tag.
    Float(u128),
    /// A pointer-typed constant reference to a function or global value.
    /// Mirrors `GlobalValue` being a `Constant` whose `getType()` is the
    /// pointer type while `getValueType()` stores the pointee/function type.
    GlobalValueRef { value: ValueSlot },
    /// `null` of a pointer or typed-pointer type.
    PointerNull,
    /// Temporary parser placeholder for a forward `blockaddress`.
    /// It is replaced before successful module parsing completes.
    BlockAddressPlaceholder,
    /// Aggregate constant — `ConstantArray`, `ConstantStruct`, or
    /// `ConstantVector`. Element categorisation is determined by the
    /// owning aggregate type.
    Aggregate(Box<[ValueSlot]>),
    /// A specialised byte-offset into a global, printed as the constant
    /// expression `getelementptr inbounds (i8, ptr @<base>, i64 <off>)`.
    /// `base_id` is the value-id of the host global/function; `off` is the byte
    /// offset. This compact form is kept for symbol-relative initializers that
    /// point into the *middle* of another global, such as a relocated pointer
    /// slot inside an embedded section. The owning value's type is `ptr`.
    GepOffset { base_id: ValueSlot, off: i64 },
    /// Specialised link-time difference of two symbol addresses, printed as the
    /// constant expression `sub (i64 ptrtoint (ptr @hi to i64), i64 ptrtoint
    /// (ptr @lo to i64))`. Both ids are globals/functions; the owning value's
    /// type is `i64`. The subtraction is resolved by the linker (a
    /// section-relative relocation), so neither operand's absolute address need
    /// be known at emit time. This form is kept for symbol-relative obfuscation,
    /// where a real address is reached as `anchor + (real - anchor)` and only
    /// the delta lives in data. The two ids must differ (a self-delta would be a
    /// constant zero; callers should use `Int(0)` for that).
    SymbolDelta { hi_id: ValueSlot, lo_id: ValueSlot },
    /// Link-time symbol difference plus a constant addend, printed as
    /// `add (i64 sub (i64 ptrtoint (ptr @hi to i64), i64 ptrtoint (ptr @lo to
    /// i64)), i64 <addend>)`. Like [`ConstantData::SymbolDelta`] but with a
    /// baked-in integer `addend` the linker folds into the same relocation
    /// (additive relocations compose). Used to bake an *encrypted* delta —
    /// `(real - anchor) + K` — so the recovered value is `enc - K` rather than
    /// the bare delta, giving the runtime decrypt a genuine (non-identity)
    /// computation the optimizer cannot fold away. The two symbol ids must
    /// differ; the owning value's type is `i64`.
    SymbolDeltaPlus {
        hi_id: ValueSlot,
        lo_id: ValueSlot,
        addend: i64,
    },
    /// `blockaddress(@function, %block)`.
    BlockAddress {
        function: ValueSlot,
        block: ValueSlot,
    },
    /// `dso_local_equivalent @function`.
    DSOLocalEquivalent { function: ValueSlot },
    /// `no_cfi @function`.
    NoCfi { function: ValueSlot },
    /// `token none`.
    TokenNone,
    /// `target(...) none`.
    TargetExtNone,
    /// `ptrauth (...)`.
    PtrAuth {
        pointer: ValueSlot,
        key: ValueSlot,
        discriminator: ValueSlot,
        addr_discriminator: ValueSlot,
        deactivation_symbol: ValueSlot,
    },
    /// `undef` of any first-class type.
    Undef,
    /// `poison` of any first-class type. Distinct from `undef` per
    /// LangRef.
    Poison,
}

impl ConstantData {
    pub(crate) fn for_each_operand<F>(&self, mut f: F)
    where
        F: FnMut(ValueSlot),
    {
        match self {
            Self::Expr(data) => {
                for operand in data.operands.iter().copied() {
                    f(operand);
                }
            }
            Self::Aggregate(elements) => {
                for element in elements.iter().copied() {
                    f(element);
                }
            }
            Self::PtrAuth {
                pointer,
                key,
                discriminator,
                addr_discriminator,
                deactivation_symbol,
            } => {
                f(*pointer);
                f(*key);
                f(*discriminator);
                f(*addr_discriminator);
                f(*deactivation_symbol);
            }
            Self::Int(_)
            | Self::Float(_)
            | Self::GlobalValueRef { .. }
            | Self::PointerNull
            | Self::BlockAddressPlaceholder
            | Self::GepOffset { .. }
            | Self::SymbolDelta { .. }
            | Self::SymbolDeltaPlus { .. }
            | Self::BlockAddress { .. }
            | Self::DSOLocalEquivalent { .. }
            | Self::NoCfi { .. }
            | Self::TokenNone
            | Self::TargetExtNone
            | Self::Undef
            | Self::Poison => {}
        }
    }
}

/// Linear parser placeholder for a forward `blockaddress`.
///
/// The erased [`Constant`] view may be embedded in parsed constants and
/// instructions, but only this parser-only handle can resolve the placeholder.
#[derive(Branded)]
#[branded(Debug)]
pub struct BlockAddressPlaceholder<'ctx, B: ModuleBrand> {
    constant: Constant<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> BlockAddressPlaceholder<'ctx, B> {
    #[inline]
    pub(crate) fn from_constant(constant: Constant<'ctx, B>) -> Self {
        Self { constant }
    }

    #[inline]
    pub fn as_constant(&self) -> Constant<'ctx, B> {
        self.constant
    }

    #[doc(hidden)]
    pub fn replace_all_uses_with<C: IsConstant<'ctx, B>>(self, replacement: C) -> IrResult<()> {
        crate::constants::replace_constant_uses_with(self.constant, replacement.as_constant())
    }
}

// --------------------------------------------------------------------------
// Public erased handle
// --------------------------------------------------------------------------

/// Type-erased constant handle. Mirrors the role of `Constant *` in
/// LLVM C++ — every concrete constant ([`ConstantIntValue`], ...)
/// widens to this handle for storage in operand lists or for analysis
/// passes.
///
/// [`ConstantIntValue`]: crate::constants::ConstantIntValue
#[derive(Branded)]
pub struct Constant<'ctx, B: ModuleBrand> {
    pub(crate) id: ValueSlot,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) ty: TypeSlot,
}

impl<'ctx, B: ModuleBrand + 'ctx> Constant<'ctx, B> {
    /// Construct from raw parts. Crate-internal: only the constant
    /// constructors hand these out.
    #[inline]
    pub(crate) fn from_parts(value: Value<'ctx, B>) -> Self {
        Self {
            id: value.id,
            module: value.module,
            ty: value.ty,
        }
    }

    /// The single value every lane of this vector constant holds, if there is
    /// one.
    ///
    /// Ports `Constant::getSplatValue` together with the
    /// `ConstantVector::getSplatValue` loop it delegates to. Upstream's two
    /// short-circuits come first: an all-poison vector splats to poison of the
    /// *element* type, and a zeroinitializer to that type's null value.
    ///
    /// `allow_poison` is upstream's flag, and it defaults to `false` there.
    /// Nearly every caller takes that default — including all of
    /// `ConstantFold.cpp` and both `VectorUtils.cpp` splat entry points — so a
    /// poison lane is a mismatch and the answer is `None`. The `true` callers
    /// are the poison-tolerant `PatternMatch.h` matchers
    /// (`m_APIntAllowPoison` and its neighbours), where a poison lane agrees
    /// with any other because it can be read as whatever the rest hold.
    /// Passing `true` where upstream takes the default would silently widen
    /// the match, so the parameter is deliberately explicit rather than
    /// defaulted.
    ///
    /// Two of upstream's arms are unreachable rather than missing: the
    /// vector-typed `ConstantInt` and `ConstantFP` splat forms have no llvmkit
    /// representation, because a vector constant is always an element list.
    /// The constant-*expression* splat form is handled, in a private helper
    /// below — though only a *scalable* vector ever reaches it, because the
    /// folder materialises a fixed one into an element list at construction.
    pub fn splat_value(self, allow_poison: bool) -> Option<Constant<'ctx, B>> {
        let (element_ty, _, _) = self.ty().data().as_vector()?;
        let element_ty = Type::new(element_ty, self.as_erased().module());

        match &self.as_erased().data().kind {
            ValueKindData::Constant(ConstantData::Poison) => {
                return Some(element_ty.poison().as_constant());
            }
            // `isa<ConstantAggregateZero>`: llvmkit spells a zeroinitializer
            // as an aggregate of zeros, so this arm catches only the
            // whole-vector null form.
            ValueKindData::Constant(ConstantData::Aggregate(_)) => {}
            // The constant-expression splat `ConstantVector::getSplat` builds.
            ValueKindData::Constant(ConstantData::Expr(_)) => {
                return self.constant_expression_splat_value();
            }
            _ => return None,
        }

        let ValueKindData::Constant(ConstantData::Aggregate(elements)) =
            &self.as_erased().data().kind
        else {
            return None;
        };
        let module = self.as_erased().module();
        let element_at = |slot: &ValueSlot| {
            let data = module.context().value_data(*slot);
            Constant::from_parts(Value::from_parts(*slot, module, data.ty))
        };

        let mut splat: Option<Constant<'ctx, B>> = None;
        for slot in elements.iter() {
            let element = element_at(slot);
            let element_is_poison = matches!(
                &element.as_erased().data().kind,
                ValueKindData::Constant(ConstantData::Poison)
            );
            match splat {
                Some(seen) if seen == element => {}
                // Strict mode: any mismatch ends it.
                Some(_) if !allow_poison => return None,
                // Allow-poison mode: a poison lane carries no disagreement.
                Some(_) if element_is_poison => {}
                // A defined lane replaces a poison one already seen.
                Some(seen)
                    if matches!(
                        &seen.as_erased().data().kind,
                        ValueKindData::Constant(ConstantData::Poison)
                    ) =>
                {
                    splat = Some(element);
                }
                Some(_) => return None,
                None => splat = Some(element),
            }
        }
        splat
    }

    /// The scalar behind the constant-expression splat form
    /// `ConstantVector::getSplat` builds — an `insertelement` at lane 0
    /// broadcast through an all-zero `shufflevector` mask.
    ///
    /// Ports the closing `dyn_cast<ConstantExpr>` block of
    /// `Constant::getSplatValue`:
    ///
    /// ```text
    /// shufflevector (insertelement (undef, Splat, 0), undef, zeroinitializer)
    /// ```
    ///
    /// **Which mask this reads is the subtle part.** `ConstantExprData` has a
    /// `mask` field *and* a third operand, and only the operand is live:
    /// `validate_constant_expr_data` rejects a `ShuffleVector` expression whose
    /// `mask` field is non-empty, and every construction site passes an empty
    /// one. So the mask is `operands[2]`, an ordinary constant vector, and it
    /// is read as one here.
    ///
    /// `all_of(Mask, I == 0)` is upstream's test, and it is strict: an `undef`
    /// mask lane reads back as `-1` through `getShuffleMask`, so it fails.
    /// Every lane must be a defined zero.
    fn constant_expression_splat_value(self) -> Option<Constant<'ctx, B>> {
        let module = self.as_erased().module();
        let at = |slot: ValueSlot| {
            Constant::from_parts(Value::from_parts(
                slot,
                module,
                module.context().value_data(slot).ty,
            ))
        };
        let is_undefined = |constant: Constant<'ctx, B>| {
            // `isa<UndefValue>`, which catches `poison` upstream.
            matches!(
                &constant.as_erased().data().kind,
                ValueKindData::Constant(ConstantData::Undef | ConstantData::Poison)
            )
        };

        let ValueKindData::Constant(ConstantData::Expr(shuffle)) = &self.as_erased().data().kind
        else {
            return None;
        };
        if shuffle.opcode != ConstantExprOpcode::ShuffleVector {
            return None;
        }
        let [shuffle_source, shuffle_other, shuffle_mask] = *shuffle.operands else {
            return None;
        };
        if !is_undefined(at(shuffle_other)) {
            return None;
        }

        let inserted = at(shuffle_source);
        let ValueKindData::Constant(ConstantData::Expr(insert)) = &inserted.as_erased().data().kind
        else {
            return None;
        };
        if insert.opcode != ConstantExprOpcode::InsertElement {
            return None;
        }
        let [insert_into, splat, index] = *insert.operands else {
            return None;
        };
        if !is_undefined(at(insert_into)) {
            return None;
        }

        // `Index && Index->getValue() == 0`.
        let ValueKindData::Constant(ConstantData::Int(words)) = &at(index).as_erased().data().kind
        else {
            return None;
        };
        if !words.iter().all(|word| *word == 0) {
            return None;
        }

        // `llvm::all_of(Mask, [](int I) { return I == 0; })`, which on
        // llvmkit's representation is exactly "the mask constant is null".
        // Both hold for a `zeroinitializer` and for a written-out vector of
        // zeros, and both fail for an `undef` lane — `getShuffleMask` reads
        // one back as `-1`, and `is_null_value` does not count `undef` as
        // null. Asking it this way also reaches a *scalable* `zeroinitializer`
        // mask, which has no element list to walk.
        at(shuffle_mask).is_null_value().then(|| at(splat))
    }

    /// Whether this constant is the all-zero value of its type.
    ///
    /// Ports `Constant::isNullValue` for the constant forms llvmkit stores.
    /// The forms it does not reach — constant expressions, `blockaddress`,
    /// global references and the symbol-relative payloads — are never null,
    /// which is the answer upstream gives them too.
    pub fn is_null_value(self) -> bool {
        let value = self.as_erased();
        match &value.data().kind {
            ValueKindData::Constant(ConstantData::Int(words)) => {
                words.iter().all(|word| *word == 0)
            }
            ValueKindData::Constant(ConstantData::Float(bits)) => *bits == 0,
            ValueKindData::Constant(ConstantData::PointerNull) => true,
            ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
                let module = value.module();
                elements.iter().all(|slot| {
                    Constant::from_parts(Value::from_parts(
                        *slot,
                        module,
                        module.context().value_data(*slot).ty,
                    ))
                    .is_null_value()
                })
            }
            _ => false,
        }
    }

    /// Whether every bit of this constant is set.
    ///
    /// Ports `Constant::isAllOnesValue`, whose three arms this follows: an
    /// integer equal to `-1`, a float whose bit pattern is all ones, and a
    /// vector that splats either. Note the asymmetry with
    /// [`Self::is_null_value`], which walks an aggregate element by element —
    /// upstream routes this one through `getSplatValue` instead, so a vector
    /// of all-ones lanes qualifies only where that recognises a splat.
    pub fn is_all_ones_value(self) -> bool {
        let value = self.as_erased();
        match (&value.data().kind, self.ty().kind()) {
            // Check for -1 integers.
            (ValueKindData::Constant(ConstantData::Int(words)), TypeKind::Integer { bits }) => {
                ApInt::from_words(bits, words).is_all_ones()
            }
            // Check for FP which are bitcasted from -1 integers. Upstream
            // spells this `bitcastToAPInt().isAllOnes()`, which is the stored
            // bit pattern read at the format's own width.
            (ValueKindData::Constant(ConstantData::Float(pattern)), kind) => {
                let Some(bits) = float_format_bit_width(kind) else {
                    return false;
                };
                ApInt::from_words(
                    bits,
                    &[
                        u64::try_from(*pattern & u128::from(u64::MAX)).unwrap_or(0),
                        u64::try_from(*pattern >> 64).unwrap_or(0),
                    ],
                )
                .is_all_ones()
            }
            // Check for constant splat vectors of 1 values.
            _ if self.ty().is_vector() => self
                .splat_value(false)
                .is_some_and(Constant::is_all_ones_value),
            _ => false,
        }
    }

    /// The constant sitting at `index` inside this aggregate or vector
    /// constant.
    ///
    /// Ports `Constant::getAggregateElement(unsigned Elt)`. `None` is
    /// upstream's `nullptr`: an out-of-range index, a shape that keeps no
    /// element list, or the scalable-vector bail-out upstream marks `FIXME`.
    ///
    /// Upstream reaches the answer through five `dyn_cast` arms; three of them
    /// collapse here. `ConstantAggregateZero` and `ConstantDataSequential`
    /// both become an ordinary element list at construction, so they arrive at
    /// the stored-element-list arm. The vector-typed `ConstantInt` and
    /// `ConstantFP` splat forms have no llvmkit representation at all — a
    /// vector constant is always an element list — so those arms are
    /// unreachable rather than missing.
    pub fn aggregate_element(self, index: u32) -> Option<Constant<'ctx, B>> {
        let value = self.as_erased();
        let ValueKindData::Constant(constant) = &value.data().kind else {
            return None;
        };
        match constant {
            // `dyn_cast<ConstantAggregate>`: the element is stored, so it is
            // handed back rather than rebuilt.
            ConstantData::Aggregate(elements) => {
                let slot = *elements.get(usize::try_from(index).ok()?)?;
                let module = value.module();
                Some(Constant::from_parts(Value::from_parts(
                    slot,
                    module,
                    module.context().value_data(slot).ty,
                )))
            }
            // `dyn_cast<PoisonValue>` and `dyn_cast<UndefValue>`, which answer
            // the same marker one type down. Upstream's
            // `isa<ScalableVectorType>` bail-out sits *above* these two, so a
            // scalable `undef` answers nothing — that ordering is what
            // [`Self::aggregate_element_type`] reproduces.
            ConstantData::Undef => Some(self.aggregate_element_type(index)?.undef().as_constant()),
            ConstantData::Poison => {
                Some(self.aggregate_element_type(index)?.poison().as_constant())
            }
            _ => None,
        }
    }

    /// The element type [`Self::aggregate_element`] hands an `undef` or
    /// `poison` marker back at, or `None` when `index` is past the end.
    ///
    /// Ports the `getNumElements` / `getElementValue` pair that
    /// `UndefValue` and `PoisonValue` share, plus the `isa<ScalableVectorType>`
    /// bail-out that guards them.
    fn aggregate_element_type(self, index: u32) -> Option<Type<'ctx, B>> {
        let module = self.as_erased().module();
        let data = self.ty().data();
        let slot = if let Some((element, lanes, scalable)) = data.as_vector() {
            if scalable || index >= lanes {
                return None;
            }
            element
        } else if let Some((element, lanes)) = data.as_array() {
            if u64::from(index) >= lanes {
                return None;
            }
            element
        } else {
            let body = data.as_struct()?.body.borrow();
            *body.as_ref()?.elements.get(usize::try_from(index).ok()?)?
        };
        Some(Type::new(slot, module))
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// IR type of the constant.
    #[inline]
    pub fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}

/// The width of a floating-point format's bit pattern, as
/// `APFloat::bitcastToAPInt` would produce it.
///
/// `x86_fp80` and `ppc_fp128` answer `None`: llvmkit stores a float constant's
/// pattern in a `u128`, and reading an 80-bit format's all-ones question off
/// that would depend on how the unused bits were stored rather than on the
/// value. Both are answered `false` by the caller, which is the conservative
/// direction for an "is this all ones" test.
fn float_format_bit_width(kind: TypeKind) -> Option<u32> {
    match kind {
        TypeKind::Half | TypeKind::BFloat => Some(16),
        TypeKind::Float => Some(32),
        TypeKind::Double => Some(64),
        TypeKind::Fp128 => Some(128),
        _ => None,
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> core::fmt::Display for Constant<'ctx, B> {
    /// Print the operand form `<type> <literal>`, identical to what the
    /// erased [`Value`] handle from [`Constant::as_erased`] prints.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&Constant::as_erased(*self), f)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> sealed::Sealed for Constant<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for Constant<'ctx, B> {
    #[inline]
    fn as_erased(self) -> Value<'ctx, B> {
        Constant::as_erased(self)
    }
}
crate::value::impl_into_erased_value_for_handle!(Constant);
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for Constant<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Constant::ty(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for Constant<'ctx, B> {
    #[inline]
    fn name(self) -> Option<String> {
        self.as_erased().name()
    }
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_erased().set_name(module_token, name);
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        self.as_erased().clear_name(module_token);
    }
}
impl<B: ModuleBrand + 'static> HasDebugLoc for Constant<'_, B> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_erased().debug_loc()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<Constant<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(c: Constant<'ctx, B>) -> Self {
        c.as_erased()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for Constant<'ctx, B> {
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        if let ValueKindData::Constant(_) = v.data().kind {
            Ok(Self::from_parts(v))
        } else {
            Err(IrError::ValueCategoryMismatch {
                expected: ValueCategoryLabel::Constant,
                got: v.category().into(),
            })
        }
    }
}

// --------------------------------------------------------------------------
// Sealed marker
// --------------------------------------------------------------------------

/// Sealed marker implemented by every per-kind constant refinement
/// (`ConstantIntValue`, `ConstantFloatValue`, ...) plus the erased
/// [`Constant`] itself. Bound generic code with this trait when a
/// function should accept any constant.
pub trait IsConstant<'ctx, B: ModuleBrand>: sealed::Sealed + IsValue<'ctx, B> {
    /// Widen to the erased [`Constant`] handle.
    fn as_constant(self) -> Constant<'ctx, B>;
}

impl<'ctx, B: ModuleBrand + 'ctx> IsConstant<'ctx, B> for Constant<'ctx, B> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B> {
        self
    }
}

// --------------------------------------------------------------------------
// IntoConstantValue: constant initializers from handles or Rust literals
// --------------------------------------------------------------------------

/// A value usable as a constant initializer or element: an existing
/// constant handle, or a Rust scalar literal materialized through the
/// module's context.
///
/// The blanket impl accepts any [`IsConstant`] handle unchanged (its
/// `module` argument is ignored). The scalar impls — one per exact Rust
/// width (`bool`, `i8`..=`i128`, `u8`..=`u128`, `f32`, `f64`) — build the
/// matching IR constant through the module and erase it to [`Constant`].
/// One literal maps to exactly one IR width, with no widening: `0i32` is
/// an `i32`, `0i64` an `i64`. Ints route through
/// [`IntoConstantInt`], floats through
/// [`IntoConstantFloat`].
pub trait IntoConstantValue<'ctx, B: ModuleBrand> {
    /// Materialize `self` as an erased [`Constant`] owned by `module`.
    fn into_constant(self, module: ModuleRef<'ctx, B>) -> Constant<'ctx, B>;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: IsConstant<'ctx, B>> IntoConstantValue<'ctx, B> for C {
    #[inline]
    fn into_constant(self, _module: ModuleRef<'ctx, B>) -> Constant<'ctx, B> {
        self.as_constant()
    }
}

macro_rules! impl_into_constant_value_int {
    ($rust_ty:ty, $marker:ty, $ty_method:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantValue<'ctx, B> for $rust_ty {
            #[inline]
            fn into_constant(self, module: ModuleRef<'ctx, B>) -> Constant<'ctx, B> {
                let ty = IntType::<$marker, B>::new(
                    module.module().$ty_method::<B>().as_type().id(),
                    module,
                );
                IntoConstantInt::into_constant_int(self, ty)
                    .unwrap_or_else(|_| {
                        unreachable!("exact-width scalar literal is an infallible IR constant")
                    })
                    .as_constant()
            }
        }
    };
}

// `bool` -> i1; each `iN`/`uN` maps to its exact IR width (no widening).
impl_into_constant_value_int!(bool, bool, bool_type);
impl_into_constant_value_int!(i8, i8, i8_type);
impl_into_constant_value_int!(i16, i16, i16_type);
impl_into_constant_value_int!(i32, i32, i32_type);
impl_into_constant_value_int!(i64, i64, i64_type);
impl_into_constant_value_int!(i128, i128, i128_type);
impl_into_constant_value_int!(u8, i8, i8_type);
impl_into_constant_value_int!(u16, i16, i16_type);
impl_into_constant_value_int!(u32, i32, i32_type);
impl_into_constant_value_int!(u64, i64, i64_type);
impl_into_constant_value_int!(u128, i128, i128_type);

macro_rules! impl_into_constant_value_float {
    ($rust_ty:ty, $marker:ty, $ty_method:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantValue<'ctx, B> for $rust_ty {
            #[inline]
            fn into_constant(self, module: ModuleRef<'ctx, B>) -> Constant<'ctx, B> {
                let ty = FloatType::<$marker, B>::new(
                    module.module().$ty_method::<B>().as_type().id(),
                    module,
                );
                IntoConstantFloat::into_constant_float(self, ty)
                    .unwrap_or_else(|_| {
                        unreachable!("exact-width scalar literal is an infallible IR constant")
                    })
                    .as_constant()
            }
        }
    };
}

// `f32`/`f64` map to their exact IR float kind (no widening).
impl_into_constant_value_float!(f32, f32, f32_type);
impl_into_constant_value_float!(f64, f64, f64_type);
