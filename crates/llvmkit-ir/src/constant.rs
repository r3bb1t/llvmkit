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

use super::capability::{CanMutate, Capability, CapabilityOf, Mutable, ReadOnly};
use super::constants::{ConstantFloatValue, ConstantIntValue};
use super::derived_types::{FloatType, IntType};
use super::error::ValueCategoryLabel;
use super::float_kind::{FloatDyn, IntoConstantFloat};
use super::int_width::{IntDyn, IntoConstantInt};
use super::module::ModuleBrand;
use super::value::ValueKindData;
use crate::Branded;
use crate::ap_int::ApInt;
use crate::gep_no_wrap_flags::GepNoWrapFlags;
use crate::module::{Module, ModuleRef, Unverified};
use crate::r#type::{Type, TypeData, TypeKind, TypeSlot, TypeSlotAccess};
use crate::value::{
    HasDebugLoc, HasName, IsValue, SetName, Typed, Value, ValueSlot, ValueSlotAccess, sealed,
};
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

/// The half-open `inrange(start, end)` range attached to a constant
/// `getelementptr`.
///
/// Mirrors the `InRangeStart` / `InRangeEnd` pair `LLParser::parseValID`'s
/// `getelementptr` arm carries as two `APSInt`s and hands to
/// `ConstantExpr::getGetElementPtr` as a `ConstantRange`, which holds two
/// `APInt`s. Both bounds are already at the index width — upstream applies
/// `extOrTrunc(IndexWidth)` before constructing the range, so the width is the
/// bounds' own and is not stored separately.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstantExprInRange {
    start: ApInt,
    end: ApInt,
}

impl ConstantExprInRange {
    /// The range `[start, end)`. Both bounds must already be at the index
    /// width; mismatched widths are widened to the larger, which is the only
    /// answer that keeps [`Self::bit_width`] meaningful.
    #[inline]
    #[must_use]
    pub fn new(start: ApInt, end: ApInt) -> Self {
        let width = start.bit_width().max(end.bit_width());
        Self {
            start: start.sext_or_trunc(width),
            end: end.sext_or_trunc(width),
        }
    }

    #[inline]
    pub fn start(&self) -> &ApInt {
        &self.start
    }

    #[inline]
    pub fn end(&self) -> &ApInt {
        &self.end
    }

    /// The shared width of the two bounds — upstream's
    /// `ConstantRange::getBitWidth`.
    #[inline]
    pub fn bit_width(&self) -> u32 {
        self.start.bit_width()
    }

    /// `InRangeStart.slt(InRangeEnd)` — the negation of
    /// `LLParser::parseValID`'s `if (InRangeStart.sge(InRangeEnd)) return
    /// error(…, "expected end to be larger than start")`.
    ///
    /// Signed, as upstream spells it, and the *only* emptiness test in the
    /// tree: the parser and `constants.rs` used to carry a hand-rolled
    /// `signed_apint_cmp` apiece.
    #[inline]
    #[must_use]
    pub fn is_non_empty(&self) -> bool {
        self.start.slt(&self.end)
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

/// `isa<UndefValue>(C)` **minus** poison.
///
/// Upstream's `PoisonValue` derives from `UndefValue`, so a bare
/// `isa<UndefValue>` there answers `true` for both. llvmkit splits the two so a
/// port can say which it means; a caller mirroring upstream's `isa<UndefValue>`
/// wants [`is_undef_or_poison`].
pub(crate) fn is_undef<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    matches!(
        &constant.as_erased().data().kind,
        ValueKindData::Constant(ConstantData::Undef)
    )
}

/// `isa<PoisonValue>(C)`.
pub(crate) fn is_poison<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    matches!(
        &constant.as_erased().data().kind,
        ValueKindData::Constant(ConstantData::Poison)
    )
}

/// Upstream's plain `isa<UndefValue>(C)`, which catches `poison` too.
pub(crate) fn is_undef_or_poison<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    is_undef(constant) || is_poison(constant)
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
    /// Temporary parser placeholder standing in for a value that has been
    /// referenced before it is defined. It is replaced before successful
    /// module parsing completes.
    ForwardRefPlaceholder,
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
    DsoLocalEquivalent { function: ValueSlot },
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
            // `BlockAddress`'s operands are `[Function, BasicBlock]`
            // (`BlockAddress::BlockAddress`), and both are real `Use`s — a
            // `blockaddress` is why a block can have a use from outside its
            // own function.
            Self::BlockAddress { function, block } => {
                f(*function);
                f(*block);
            }
            // The rest name no operand llvmkit can hand back. Note the ones
            // that *should*: `GlobalValueRef`, `DsoLocalEquivalent`, `NoCfi`,
            // `GepOffset` and the `SymbolDelta` pair all wrap a global, and
            // upstream counts each as a use of it. Yielding them here would
            // register the edge against llvmkit's *interned* wrapper rather
            // than the global, which loses count rather than fixing it —
            // see `docs/divergences.md` (D2/D3).
            Self::Int(_)
            | Self::Float(_)
            | Self::GlobalValueRef { .. }
            | Self::PointerNull
            | Self::ForwardRefPlaceholder
            | Self::GepOffset { .. }
            | Self::SymbolDelta { .. }
            | Self::SymbolDeltaPlus { .. }
            | Self::DsoLocalEquivalent { .. }
            | Self::NoCfi { .. }
            | Self::TokenNone
            | Self::TargetExtNone
            | Self::Undef
            | Self::Poison => {}
        }
    }
}

/// Linear parser placeholder standing in for a value referenced before it is
/// defined.
///
/// Mirrors the sentinel `LLParser` mints on first use — `new Argument(Ty)` in
/// `PerFunctionState::getVal`, `createGlobalFwdRef` for a `@`-reference, a
/// stand-in constant for a forward `blockaddress`. The erased [`Value`] view
/// may be embedded in parsed constants, instructions and metadata, but only
/// this parser-only handle can resolve the placeholder, and resolving
/// consumes it: upstream's sentinel is RAUW'd and deleted exactly once, so
/// the handle is deliberately not [`Copy`].
#[derive(Branded)]
#[branded(Debug)]
pub struct ForwardRefValue<'ctx, B: ModuleBrand> {
    constant: Constant<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ForwardRefValue<'ctx, B> {
    #[inline]
    pub(crate) fn from_constant(constant: Constant<'ctx, B>) -> Self {
        Self { constant }
    }

    /// The placeholder in constant-operand position.
    #[inline]
    pub fn as_constant(&self) -> Constant<'ctx, B> {
        self.constant
    }

    /// The placeholder in value-operand position.
    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        self.constant.as_erased()
    }

    /// The type the placeholder was minted at — the type its first use
    /// demanded.
    #[inline]
    pub fn ty(&self) -> Type<'ctx, B> {
        self.constant.ty()
    }

    /// Point every use of the placeholder at `replacement` and retire the
    /// placeholder. Mirrors the `Sentinel->replaceAllUsesWith(Inst)` step of
    /// `PerFunctionState::setInstName`.
    #[doc(hidden)]
    pub fn replace_all_uses_with(self, replacement: Value<'ctx, B>) -> IrResult<()> {
        crate::constants::replace_placeholder_uses_with(self.constant, replacement)
    }
}

// --------------------------------------------------------------------------
// Upstream's constant classes, as a sum type
// --------------------------------------------------------------------------

/// Which of upstream's `Constant` classes a constant is: the `HANDLE_CONSTANT`
/// list of `include/llvm/IR/Value.def` as a sum type, grouped the way
/// upstream groups it — `ConstantData`, then `ConstantAggregate`, then the
/// rest — so an abstract class's question (`isa<ConstantData>`) is a pattern
/// on the group. Read it with [`Constant::kind`]. Mirrors
/// `Value::getValueID` and each class's `classof`.
#[derive(Branded)]
#[branded(Clone, Copy, Debug)]
pub enum ConstantKind<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    /// `ConstantData`: a constant with no operands.
    Data(ConstantDataKind<'ctx, B, C>),
    /// `ConstantAggregate`: an element list that is none of the
    /// `ConstantData` forms upstream canonicalises such a list to.
    Aggregate(ConstantAggregateKind),
    /// `GlobalValue`: a function, global variable, alias or ifunc.
    GlobalValue,
    /// `BlockAddress`.
    BlockAddress,
    /// `ConstantExpr`, by opcode.
    Expr(ConstantExprOpcode),
    /// `DSOLocalEquivalent`.
    DsoLocalEquivalent,
    /// `NoCFIValue`.
    NoCfi,
    /// `ConstantPtrAuth`.
    PtrAuth,
    /// llvmkit's parser placeholder for a global, `blockaddress` or
    /// `dso_local_equivalent` referenced before it is defined. Upstream's
    /// placeholder in each position is a `GlobalVariable`
    /// (`createGlobalFwdRef`, and the `blockaddress` and
    /// `dso_local_equivalent` deferrals in `LLParser::parseValID`), so upstream
    /// answers `isa<GlobalValue>`; llvmkit's is a bare constant with no global
    /// behind it, so it is its own variant (`docs/divergences.md`, the
    /// forward-reference placeholder entry).
    ForwardReference,
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for ConstantKind<'_, B, C> {
    type Capability = C;
}

/// The `ConstantData` classes. See [`ConstantKind::Data`].
#[derive(Branded)]
#[branded(Clone, Copy, Debug)]
pub enum ConstantDataKind<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    /// `UndefValue`. `PoisonValue` derives from it upstream, so `Undef(_)`
    /// answers `isa<UndefValue>` and `Undef(Undefined::Poison)` answers
    /// `isa<PoisonValue>`.
    Undef(Undefined),
    /// `ConstantAggregateZero`.
    AggregateZero,
    /// `ConstantDataArray` or `ConstantDataVector`.
    Sequential(ConstantSequentialKind),
    /// `ConstantInt`.
    Int(ConstantIntValue<'ctx, IntDyn, B, C>),
    /// `ConstantFP`.
    Float(ConstantFloatValue<'ctx, FloatDyn, B, C>),
    /// `ConstantTargetNone`.
    TargetNone,
    /// `ConstantPointerNull`.
    PointerNull,
    /// `ConstantTokenNone`.
    TokenNone,
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for ConstantDataKind<'_, B, C> {
    type Capability = C;
}

/// `UndefValue` and its subclass `PoisonValue`. See [`ConstantDataKind::Undef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Undefined {
    /// `UndefValue` that is not a `PoisonValue`.
    Undef,
    /// `PoisonValue`.
    Poison,
}

/// The two `ConstantDataSequential` classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstantSequentialKind {
    /// `ConstantDataArray`.
    Array,
    /// `ConstantDataVector`.
    Vector,
}

/// The three `ConstantAggregate` classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstantAggregateKind {
    /// `ConstantArray`.
    Array,
    /// `ConstantStruct`.
    Struct,
    /// `ConstantVector`.
    Vector,
}

/// Mirrors `ConstantDataSequential::isElementTypeCompatible`: `half`,
/// `bfloat`, `float`, `double`, or an integer of 8, 16, 32 or 64 bits.
fn is_element_type_compatible(ty: &TypeData) -> bool {
    if matches!(
        ty,
        TypeData::Half | TypeData::Bfloat | TypeData::Float | TypeData::Double
    ) {
        return true;
    }
    if let TypeData::Integer { bits } = ty {
        return matches!(bits, 8 | 16 | 32 | 64);
    }
    false
}

/// Which `UndefValue` an element is, if it is one: `isa<UndefValue>` is
/// `.is_some()`, `isa<PoisonValue>` is `== Some(Undefined::Poison)`.
fn undefined<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> Option<Undefined> {
    match constant.kind() {
        ConstantKind::Data(ConstantDataKind::Undef(undefined)) => Some(undefined),
        _ => None,
    }
}

/// `V[i] == C` for a `C` that is the null value, `undef` or `poison` — the
/// only `C`s the canonicalisations below compare against. Upstream compares
/// uniqued pointers. Each of the three is unique for its type
/// (`Constant::getNullValue`, `UndefValue::get`, `PoisonValue::get`), and one
/// list has one element type, so `V[i]` is `C` exactly when it is the same
/// one of the three.
fn is_same_unique_constant<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    first: Constant<'ctx, B, C>,
    other: Constant<'ctx, B, C>,
) -> bool {
    if first.is_null_value() {
        return other.is_null_value();
    }
    match undefined(first) {
        Some(first) => undefined(other) == Some(first),
        None => false,
    }
}

/// Mirrors `getSequenceIfElementsMatch` (with `getIntSequenceIfElementsMatch`
/// and `getFPSequenceIfElementsMatch`): a `ConstantDataArray` /
/// `ConstantDataVector` when the first element is a `ConstantInt` of 8, 16, 32
/// or 64 bits and every element is a `ConstantInt`, or the first is a `half`,
/// `bfloat`, `float` or `double` `ConstantFP` and every element is a
/// `ConstantFP`. `None` is upstream's `nullptr`.
fn sequence_if_elements_match<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    first: Constant<'ctx, B, C>,
    elements: &[Constant<'ctx, B, C>],
    sequence: ConstantSequentialKind,
) -> Option<ConstantKind<'ctx, B, C>> {
    let matched = match first.kind() {
        ConstantKind::Data(ConstantDataKind::Int(_))
            if matches!(
                first.ty().data(),
                TypeData::Integer {
                    bits: 8 | 16 | 32 | 64
                }
            ) =>
        {
            elements.iter().all(|element| {
                matches!(element.kind(), ConstantKind::Data(ConstantDataKind::Int(_)))
            })
        }
        ConstantKind::Data(ConstantDataKind::Float(_))
            if matches!(
                first.ty().data(),
                TypeData::Half | TypeData::Bfloat | TypeData::Float | TypeData::Double
            ) =>
        {
            elements.iter().all(|element| {
                matches!(
                    element.kind(),
                    ConstantKind::Data(ConstantDataKind::Float(_))
                )
            })
        }
        _ => false,
    };
    matched.then_some(ConstantKind::Data(ConstantDataKind::Sequential(sequence)))
}

/// The classification half of `ConstantArray::getImpl`.
fn array_kind<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    elements: &[Constant<'ctx, B, C>],
) -> ConstantKind<'ctx, B, C> {
    // Empty arrays are canonicalized to ConstantAggregateZero.
    let Some(&first) = elements.first() else {
        return ConstantKind::Data(ConstantDataKind::AggregateZero);
    };
    // If this is an all-zero array, return a ConstantAggregateZero object. If
    // all undef, return an UndefValue, if "all simple", then return a
    // ConstantDataArray.
    let all_same_as_first = || {
        elements
            .iter()
            .all(|element| is_same_unique_constant(first, *element))
    };
    if undefined(first) == Some(Undefined::Poison) && all_same_as_first() {
        return ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison));
    }
    // An all-poison list returned above, so `first` is `undef` proper here.
    if undefined(first).is_some() && all_same_as_first() {
        return ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef));
    }
    if first.is_null_value() && all_same_as_first() {
        return ConstantKind::Data(ConstantDataKind::AggregateZero);
    }
    // Check to see if all of the elements are ConstantFP or ConstantInt and if
    // the element type is compatible with ConstantDataVector. If so, use it.
    if is_element_type_compatible(first.ty().data())
        && let Some(kind) =
            sequence_if_elements_match(first, elements, ConstantSequentialKind::Array)
    {
        return kind;
    }
    // Otherwise, we really do want to create a ConstantArray.
    ConstantKind::Aggregate(ConstantAggregateKind::Array)
}

/// The classification half of `ConstantStruct::get`.
fn struct_kind<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    elements: &[Constant<'ctx, B, C>],
) -> ConstantKind<'ctx, B, C> {
    // Create a ConstantAggregateZero value if all elements are zeros.
    let mut is_zero = true;
    let mut is_undef = false;
    let mut is_poison = false;
    if let Some(&first) = elements.first() {
        is_undef = undefined(first).is_some();
        is_poison = undefined(first) == Some(Undefined::Poison);
        is_zero = first.is_null_value();
        // PoisonValue inherits UndefValue, so its check is not necessary.
        if is_undef || is_zero {
            for &element in elements {
                if !element.is_null_value() {
                    is_zero = false;
                }
                if undefined(element) != Some(Undefined::Poison) {
                    is_poison = false;
                }
                if undefined(element) == Some(Undefined::Poison) || undefined(element).is_none() {
                    is_undef = false;
                }
            }
        }
    }
    if is_zero {
        return ConstantKind::Data(ConstantDataKind::AggregateZero);
    }
    if is_poison {
        return ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison));
    }
    if is_undef {
        return ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef));
    }
    ConstantKind::Aggregate(ConstantAggregateKind::Struct)
}

/// The classification half of `ConstantVector::getImpl`.
fn fixed_vector_kind<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    elements: &[Constant<'ctx, B, C>],
) -> ConstantKind<'ctx, B, C> {
    // `assert(!V.empty() && "Vectors can't be empty")`. llvmkit's builder
    // accepts a zero-lane vector type (only the parser refuses `<0 x T>`), so
    // an empty list answers as `ConstantArray::getImpl` answers an empty array.
    let Some(&first) = elements.first() else {
        return ConstantKind::Data(ConstantDataKind::AggregateZero);
    };
    // If this is an all-undef or all-zero vector, return a
    // ConstantAggregateZero or UndefValue.
    let mut is_zero = first.is_null_value();
    let mut is_undef = undefined(first).is_some();
    let mut is_poison = undefined(first) == Some(Undefined::Poison);
    // Upstream also carries `isSplatFP` / `isSplatInt` here, gated by the
    // hidden `-use-constant-fp-for-fixed-length-splat` and
    // `-use-constant-int-for-fixed-length-splat` options. Both default to
    // `false` and llvmkit has no option surface, so neither arm can run.
    if (is_zero || is_undef)
        && elements
            .iter()
            .skip(1)
            .any(|element| !is_same_unique_constant(first, *element))
    {
        is_zero = false;
        is_undef = false;
        is_poison = false;
    }
    if is_zero {
        return ConstantKind::Data(ConstantDataKind::AggregateZero);
    }
    if is_poison {
        return ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison));
    }
    if is_undef {
        return ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef));
    }
    // Check to see if all of the elements are ConstantFP or ConstantInt and if
    // the element type is compatible with ConstantDataVector. If so, use it.
    if is_element_type_compatible(first.ty().data())
        && let Some(kind) =
            sequence_if_elements_match(first, elements, ConstantSequentialKind::Vector)
    {
        return kind;
    }
    // Otherwise, the element type isn't compatible with ConstantDataVector, or
    // the operand list contains a ConstantExpr or something else strange.
    ConstantKind::Aggregate(ConstantAggregateKind::Vector)
}

/// The classification half of `ConstantVector::getSplat`'s scalable branch.
/// `VectorType::const_vector` demands a scalable list's lanes agree, so the
/// list is a splat of its first element.
fn scalable_vector_kind<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    elements: &[Constant<'ctx, B, C>],
) -> ConstantKind<'ctx, B, C> {
    // Upstream first tries the hidden `-use-constant-int-for-scalable-splat`
    // and `-use-constant-fp-for-scalable-splat` options; both default to
    // `false` and llvmkit has no option surface, so neither arm can run. An
    // empty list answers as `fixed_vector_kind` answers one.
    let Some(&splat) = elements.first() else {
        return ConstantKind::Data(ConstantDataKind::AggregateZero);
    };
    if splat.is_null_value() {
        return ConstantKind::Data(ConstantDataKind::AggregateZero);
    }
    match undefined(splat) {
        Some(undefined) => ConstantKind::Data(ConstantDataKind::Undef(undefined)),
        // Upstream builds the splat as
        // `shufflevector (insertelement poison, V, 0), poison, zeroinitializer`.
        None => ConstantKind::Expr(ConstantExprOpcode::ShuffleVector),
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
///
/// `C` is the [`Capability`] (D8). A constant reached from a `ReadOnly` value
/// or type is `ReadOnly`, and so is every constant read out of it.
#[derive(Branded)]
pub struct Constant<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    // Private to this module: the slot leaves a handle only through the two
    // doors of `ValueSlotAccess`, and the cached type only as a `Type` handle.
    id: ValueSlot,
    pub(crate) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for Constant<'_, B, C> {
    type Capability = C;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> Constant<'ctx, B, C> {
    /// Construct from raw parts. Crate-internal: only the constant
    /// constructors hand these out.
    #[inline]
    pub(crate) fn from_parts(value: Value<'ctx, B, C>) -> Self {
        // Internal: a re-wrap that keeps `value`'s own module.
        Self {
            id: value.slot_trusting_same_module(),
            module: value.module,
            ty: value.ty().slot_trusting_same_module(),
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
    pub fn splat_value(self, allow_poison: bool) -> Option<Constant<'ctx, B, C>> {
        let (element_ty, _, _) = self.ty().data().as_vector()?;
        let element_ty = Type::new(element_ty, self.module);

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
        let module = self.module;
        let element_at = |slot: &ValueSlot| {
            let data = module.value_data(*slot);
            Constant::from_parts(Value::from_parts(*slot, module, data.ty))
        };

        let mut splat: Option<Constant<'ctx, B, C>> = None;
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
    fn constant_expression_splat_value(self) -> Option<Constant<'ctx, B, C>> {
        let module = self.module;
        let at = |slot: ValueSlot| {
            Constant::from_parts(Value::from_parts(slot, module, module.value_data(slot).ty))
        };
        let is_undefined = |constant: Constant<'ctx, B, C>| {
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

    /// Which upstream `Constant` class this is. Mirrors `Value::getValueID`
    /// and each class's `classof`.
    ///
    /// llvmkit stores every element list — `zeroinitializer` included — in
    /// one form, where upstream canonicalises the list when it builds it. So
    /// an element list is classified by replaying that canonicalisation:
    /// `ConstantArray::getImpl` for an array, `ConstantStruct::get` for a
    /// struct, `ConstantVector::getImpl` for a fixed vector, and
    /// `ConstantVector::getSplat` for a scalable one — upstream's only
    /// constructor for a scalable vector constant, which
    /// `VectorType::const_vector` mirrors by demanding that a scalable list's
    /// lanes agree.
    pub fn kind(self) -> ConstantKind<'ctx, B, C> {
        // Matched exhaustively on purpose: a new constant kind has to be
        // classified against upstream's hierarchy, not defaulted.
        let data = match &self.as_erased().data().kind {
            ValueKindData::Constant(data) => data,
            // A global's own slot: `as_constant` on a global variable, alias
            // or ifunc wraps one, and so can an aliasee, a resolver, a
            // personality or an initializer. Upstream's `GlobalValue` is a
            // `Constant`, so this is its class.
            ValueKindData::Function(_)
            | ValueKindData::GlobalVariable(_)
            | ValueKindData::GlobalAlias(_)
            | ValueKindData::GlobalIfunc(_) => return ConstantKind::GlobalValue,
            ValueKindData::Argument { .. }
            | ValueKindData::BasicBlock(_)
            | ValueKindData::Instruction(_)
            | ValueKindData::MetadataAsValue(_)
            | ValueKindData::InlineAsm(_) => unreachable!(
                "a Constant view wraps a constant or a global value: every `Constant::from_parts` \
                 caller passes a `ValueKindData::Constant` slot, an element or operand of one, or \
                 a global's own slot"
            ),
        };
        match data {
            ConstantData::Undef => ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef)),
            ConstantData::Poison => ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison)),
            // The storage form proves the type: an `Int` payload is only ever
            // stored at an integer type, a `Float` payload at a float type.
            ConstantData::Int(_) => ConstantKind::Data(ConstantDataKind::Int(
                ConstantIntValue::from_parts_typed(self),
            )),
            ConstantData::Float(_) => ConstantKind::Data(ConstantDataKind::Float(
                ConstantFloatValue::from_parts_typed(self),
            )),
            ConstantData::PointerNull => ConstantKind::Data(ConstantDataKind::PointerNull),
            ConstantData::TokenNone => ConstantKind::Data(ConstantDataKind::TokenNone),
            ConstantData::TargetExtNone => ConstantKind::Data(ConstantDataKind::TargetNone),
            ConstantData::Aggregate(elements) => self.element_list_kind(elements),
            ConstantData::Expr(expr) => ConstantKind::Expr(expr.opcode),
            // llvmkit's compact forms, each printed as the constant expression
            // its variant's doc comment names.
            ConstantData::GepOffset { .. } => ConstantKind::Expr(ConstantExprOpcode::GetElementPtr),
            ConstantData::SymbolDelta { .. } => ConstantKind::Expr(ConstantExprOpcode::Sub),
            ConstantData::SymbolDeltaPlus { .. } => ConstantKind::Expr(ConstantExprOpcode::Add),
            ConstantData::GlobalValueRef { .. } => ConstantKind::GlobalValue,
            ConstantData::BlockAddress { .. } => ConstantKind::BlockAddress,
            ConstantData::DsoLocalEquivalent { .. } => ConstantKind::DsoLocalEquivalent,
            ConstantData::NoCfi { .. } => ConstantKind::NoCfi,
            ConstantData::PtrAuth { .. } => ConstantKind::PtrAuth,
            ConstantData::ForwardRefPlaceholder => ConstantKind::ForwardReference,
        }
    }

    /// The class upstream's constructor returns for this element list. See
    /// [`Self::kind`]. Each element is asked through its own `kind`, because
    /// a nested list is stored uncanonicalised too.
    fn element_list_kind(self, elements: &[ValueSlot]) -> ConstantKind<'ctx, B, C> {
        let module = self.module;
        let elements: Vec<Constant<'ctx, B, C>> = elements
            .iter()
            .map(|slot| {
                Constant::from_parts(Value::from_parts(
                    *slot,
                    module,
                    module.value_data(*slot).ty,
                ))
            })
            .collect();
        match self.ty().data() {
            TypeData::Array { .. } => array_kind(&elements),
            TypeData::Struct(_) => struct_kind(&elements),
            TypeData::FixedVector { .. } => fixed_vector_kind(&elements),
            TypeData::ScalableVector { .. } => scalable_vector_kind(&elements),
            _ => unreachable!(
                "an element list is interned only by `intern_aggregate`, whose callers pass an \
                 array, struct or vector type"
            ),
        }
    }

    /// Whether this constant belongs to upstream's `ConstantData` class —
    /// the operand-less, context-shared corner of the constant hierarchy that
    /// `isa<ConstantData>` selects and `Value::hasUseList` negates: whether
    /// [`Self::kind`] answers [`ConstantKind::Data`].
    ///
    /// Upstream's members are `ConstantInt`, `ConstantFP`,
    /// `ConstantPointerNull`, `ConstantTokenNone`, `ConstantTargetNone`,
    /// `UndefValue`, `PoisonValue`, `ConstantAggregateZero` and
    /// `ConstantDataSequential`. Everything else that is a `Constant` —
    /// constant expressions, `ConstantAggregate`, every `GlobalValue`,
    /// `BlockAddress`, `DSOLocalEquivalent`, `NoCFIValue`, `ConstantPtrAuth`
    /// — is not, and neither is llvmkit's forward-reference placeholder,
    /// whose upstream counterpart is a `GlobalVariable` that must keep a use
    /// list for RAUW to reach.
    pub fn is_constant_data(self) -> bool {
        matches!(self.kind(), ConstantKind::Data(_))
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
                let module = self.module;
                elements.iter().all(|slot| {
                    Constant::from_parts(Value::from_parts(
                        *slot,
                        module,
                        module.value_data(*slot).ty,
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
    pub fn aggregate_element(self, index: u32) -> Option<Constant<'ctx, B, C>> {
        let value = self.as_erased();
        let ValueKindData::Constant(constant) = &value.data().kind else {
            return None;
        };
        match constant {
            // `dyn_cast<ConstantAggregate>`: the element is stored, so it is
            // handed back rather than rebuilt.
            ConstantData::Aggregate(elements) => {
                let slot = *elements.get(usize::try_from(index).ok()?)?;
                let module = self.module;
                Some(Constant::from_parts(Value::from_parts(
                    slot,
                    module,
                    module.value_data(slot).ty,
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
    fn aggregate_element_type(self, index: u32) -> Option<Type<'ctx, B, C>> {
        let module = self.module;
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

    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// IR type of the constant, at the constant's capability.
    #[inline]
    pub fn ty(self) -> Type<'ctx, B, C> {
        Type::new(self.ty, self.module)
    }

    /// This constant at [`ReadOnly`]. Always sound — reading is a subset of
    /// mutating — and how a builder keeps a constant of any capability until
    /// it admits it against its own module.
    #[inline]
    pub fn read_only(self) -> Constant<'ctx, B, ReadOnly> {
        Constant {
            id: self.id,
            module: self.module.read_only(),
            ty: self.ty,
        }
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
        TypeKind::Half | TypeKind::Bfloat => Some(16),
        TypeKind::Float => Some(32),
        TypeKind::Double => Some(64),
        TypeKind::Fp128 => Some(128),
        _ => None,
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> core::fmt::Display for Constant<'ctx, B, C> {
    /// Print the operand form `<type> <literal>`, identical to what the
    /// erased [`Value`] handle from [`Constant::as_erased`] prints.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&Constant::as_erased(*self), f)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed for Constant<'ctx, B, C> {}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B> for Constant<'ctx, B, C> {
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        Constant::as_erased(self)
    }
}
crate::value::impl_into_erased_value_for_handle!(Constant);
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B> for Constant<'ctx, B, C> {
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        Constant::ty(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B> for Constant<'ctx, B, C> {
    #[inline]
    fn name(self) -> Option<String> {
        self.as_erased().name()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B> for Constant<'ctx, B, C> {
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        self.as_erased().set_name(module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        self.as_erased().clear_name(module_token);
    }
}
impl<B: ModuleBrand + 'static, C: Capability> HasDebugLoc for Constant<'_, B, C> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_erased().debug_loc()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<Constant<'ctx, B, C>> for Value<'ctx, B, C> {
    #[inline]
    fn from(c: Constant<'ctx, B, C>) -> Self {
        c.as_erased()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
    for Constant<'ctx, B, C>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
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
    /// Widen to the erased [`Constant`] handle, at the same capability.
    fn as_constant(self) -> Constant<'ctx, B, Self::Capability>;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IsConstant<'ctx, B> for Constant<'ctx, B, C> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B, C> {
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
/// The blanket impl accepts any [`IsConstant`] handle unchanged, refusing
/// one minted by a module other than `module` with
/// [`IrError::ForeignValueId`]. The scalar impls — one per exact Rust
/// width (`bool`, `i8`..=`i128`, `u8`..=`u128`, `f32`, `f64`) — build the
/// matching IR constant through the module and erase it to [`Constant`].
/// One literal maps to exactly one IR width, with no widening: `0i32` is
/// an `i32`, `0i64` an `i64`. Ints route through
/// [`IntoConstantInt`], floats through
/// [`IntoConstantFloat`].
pub trait IntoConstantValue<'ctx, B: ModuleBrand> {
    /// Materialize `self` as an erased [`Constant`] owned by `module`, at
    /// `module`'s capability.
    ///
    /// [`IrError::ForeignValueId`] when `self` is a constant handle another
    /// module minted.
    fn into_constant<C: Capability>(
        self,
        module: ModuleRef<'ctx, B, C>,
    ) -> IrResult<Constant<'ctx, B, C>>;
}

impl<'ctx, B: ModuleBrand + 'ctx, K: IsConstant<'ctx, B>> IntoConstantValue<'ctx, B> for K {
    #[inline]
    fn into_constant<C: Capability>(
        self,
        module: ModuleRef<'ctx, B, C>,
    ) -> IrResult<Constant<'ctx, B, C>> {
        // Boundary: refuse a handle minted by another module; one of any
        // capability is admitted and re-minted at `module`'s.
        Ok(Constant::from_parts(
            self.as_constant().as_erased().admitted_at(module)?,
        ))
    }
}

macro_rules! impl_into_constant_value_int {
    ($rust_ty:ty, $marker:ty, $ty_method:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantValue<'ctx, B> for $rust_ty {
            #[inline]
            fn into_constant<C: Capability>(
                self,
                module: ModuleRef<'ctx, B, C>,
            ) -> IrResult<Constant<'ctx, B, C>> {
                let ty: IntType<'ctx, $marker, B, C> = module.$ty_method();
                Ok(IntoConstantInt::into_constant_int(self, ty)
                    .unwrap_or_else(|_| {
                        unreachable!("exact-width scalar literal is an infallible IR constant")
                    })
                    .as_constant())
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
            fn into_constant<C: Capability>(
                self,
                module: ModuleRef<'ctx, B, C>,
            ) -> IrResult<Constant<'ctx, B, C>> {
                let ty: FloatType<'ctx, $marker, B, C> = module.$ty_method();
                Ok(IntoConstantFloat::into_constant_float(self, ty)
                    .unwrap_or_else(|_| {
                        unreachable!("exact-width scalar literal is an infallible IR constant")
                    })
                    .as_constant())
            }
        }
    };
}

// `f32`/`f64` map to their exact IR float kind (no widening).
impl_into_constant_value_float!(f32, f32, f32_type);
impl_into_constant_value_float!(f64, f64, f64_type);
