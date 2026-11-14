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
//! - `AggregateZero` — `ConstantAggregateZero`, which keeps no elements.
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
use crate::instr_types::{BinaryOpcode, CastOpcode};
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

    /// The operands an expression of this opcode takes, as
    /// [`crate::ConstantExprError::OperandCount`] states them.
    pub(crate) fn expected_operands(self) -> &'static str {
        match self {
            Self::Trunc
            | Self::PtrToAddr
            | Self::PtrToInt
            | Self::IntToPtr
            | Self::BitCast
            | Self::AddrSpaceCast => "one operand",
            Self::Add | Self::Sub | Self::Xor | Self::ExtractElement => "two operands",
            Self::InsertElement | Self::ShuffleVector => "three operands",
            Self::GetElementPtr => "a pointer base",
        }
    }

    pub(crate) fn is_cast(self) -> bool {
        self.cast_opcode().is_some()
    }

    /// The instruction binary opcode a binary expression of this opcode is, or
    /// `None` for an opcode that is no binary operator.
    pub(crate) fn binary_opcode(self) -> Option<BinaryOpcode> {
        match self {
            Self::Add => Some(BinaryOpcode::Add),
            Self::Sub => Some(BinaryOpcode::Sub),
            Self::Xor => Some(BinaryOpcode::Xor),
            Self::GetElementPtr
            | Self::ShuffleVector
            | Self::InsertElement
            | Self::ExtractElement
            | Self::Trunc
            | Self::PtrToAddr
            | Self::PtrToInt
            | Self::IntToPtr
            | Self::BitCast
            | Self::AddrSpaceCast => None,
        }
    }

    /// The expression opcode of a binary operator, or `None` for one LLVM 22
    /// keeps no constant expression of (`ConstantExpr::isDesirableBinOp`'s
    /// complement: `mul`, the divisions, shifts, `and` / `or`, the
    /// floating-point operators).
    pub(crate) const fn from_binary_opcode(opcode: BinaryOpcode) -> Option<Self> {
        match opcode {
            BinaryOpcode::Add => Some(Self::Add),
            BinaryOpcode::Sub => Some(Self::Sub),
            BinaryOpcode::Xor => Some(Self::Xor),
            BinaryOpcode::Mul
            | BinaryOpcode::Udiv
            | BinaryOpcode::Sdiv
            | BinaryOpcode::Urem
            | BinaryOpcode::Srem
            | BinaryOpcode::Shl
            | BinaryOpcode::Lshr
            | BinaryOpcode::Ashr
            | BinaryOpcode::And
            | BinaryOpcode::Or
            | BinaryOpcode::Fadd
            | BinaryOpcode::Fsub
            | BinaryOpcode::Fmul
            | BinaryOpcode::Fdiv
            | BinaryOpcode::Frem => None,
        }
    }

    /// The expression opcode of a cast, or `None` for one LLVM 22 keeps no
    /// constant expression of (`ConstantExpr::isDesirableCastOp`'s
    /// complement: the extensions and the floating-point casts).
    pub(crate) const fn from_cast_opcode(opcode: CastOpcode) -> Option<Self> {
        match opcode {
            CastOpcode::Trunc => Some(Self::Trunc),
            CastOpcode::PtrToAddr => Some(Self::PtrToAddr),
            CastOpcode::PtrToInt => Some(Self::PtrToInt),
            CastOpcode::IntToPtr => Some(Self::IntToPtr),
            CastOpcode::BitCast => Some(Self::BitCast),
            CastOpcode::AddrSpaceCast => Some(Self::AddrSpaceCast),
            CastOpcode::Zext
            | CastOpcode::Sext
            | CastOpcode::FpTrunc
            | CastOpcode::FpExt
            | CastOpcode::FpToUi
            | CastOpcode::FpToSi
            | CastOpcode::UiToFp
            | CastOpcode::SiToFp => None,
        }
    }

    /// The instruction cast opcode a cast expression of this opcode is
    /// (`ConstantExpr::getCast`'s `Opcode`), or `None` for an opcode that is no
    /// cast.
    pub(crate) fn cast_opcode(self) -> Option<CastOpcode> {
        match self {
            Self::Trunc => Some(CastOpcode::Trunc),
            Self::PtrToAddr => Some(CastOpcode::PtrToAddr),
            Self::PtrToInt => Some(CastOpcode::PtrToInt),
            Self::IntToPtr => Some(CastOpcode::IntToPtr),
            Self::BitCast => Some(CastOpcode::BitCast),
            Self::AddrSpaceCast => Some(CastOpcode::AddrSpaceCast),
            Self::Add
            | Self::Sub
            | Self::Xor
            | Self::GetElementPtr
            | Self::ShuffleVector
            | Self::InsertElement
            | Self::ExtractElement => None,
        }
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

/// `isa<UndefValue>(C)` **minus** poison, asked of the constant's class
/// ([`Constant::kind`]) rather than its storage: an element list of one
/// `undef` repeated is the `UndefValue` upstream canonicalises it to.
///
/// Upstream's `PoisonValue` derives from `UndefValue`, so a bare
/// `isa<UndefValue>` there answers `true` for both. llvmkit splits the two so a
/// port can say which it means; a caller mirroring upstream's `isa<UndefValue>`
/// wants [`is_undef_or_poison`].
pub(crate) fn is_undef<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    matches!(
        constant.kind(),
        ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef))
    )
}

/// `isa<PoisonValue>(C)`, asked of the constant's class ([`Constant::kind`]).
pub(crate) fn is_poison<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    matches!(
        constant.kind(),
        ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison))
    )
}

/// Upstream's plain `isa<UndefValue>(C)`, which catches `poison` too, asked
/// of the constant's class ([`Constant::kind`]).
pub(crate) fn is_undef_or_poison<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    matches!(
        constant.kind(),
        ConstantKind::Data(ConstantDataKind::Undef(_))
    )
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
    ///
    /// Never all null, all `undef` or all `poison` where upstream's
    /// constructor would not keep the list: `intern_aggregate` turns those
    /// into [`Self::AggregateZero`], [`Self::Undef`] and [`Self::Poison`], as
    /// `ConstantArray::getImpl`, `ConstantStruct::get` and
    /// `ConstantVector::getImpl` do. A vector list never has every lane equal
    /// either, nor is any list scalable: those are [`Self::Splat`].
    Aggregate(Box<[ValueSlot]>),
    /// `ConstantAggregateZero`: the null value of an array, struct or vector
    /// type. It keeps no elements: `ConstantAggregateZero::getElementValue`
    /// answers from the type, so its cost does not depend on the length.
    AggregateZero,
    /// A vector constant whose every lane is `element`, fixed or scalable, the
    /// element being no null, `undef` or `poison` (those are
    /// [`Self::AggregateZero`], [`Self::Undef`] and [`Self::Poison`]).
    ///
    /// Upstream has no such class. A fixed splat is a `ConstantDataVector` or
    /// a `ConstantVector` that stores every lane, and a scalable one is
    /// `ConstantVector::getSplat`'s `shufflevector (insertelement poison, V,
    /// 0), poison, zeroinitializer`. Storing one element is hardening: the
    /// cost no longer depends on the lane count, so `<4294967295 x i32> splat
    /// (i32 1)` costs one slot. [`Constant::kind`] still answers the class
    /// upstream's constructor leaves, and `intern_aggregate` turns an
    /// all-equal vector list into this, so a splat and its written-out list
    /// are one constant.
    ///
    /// `insert` is the `insertelement (poison, element, i64 0)` constant a
    /// scalable splat's `shufflevector` takes as its operand upstream, and
    /// `None` for a fixed splat. A scalable splat's one operand is that
    /// expression, which uses `element`, so the use lists are upstream's:
    /// `element` is used by the `insertelement`, and the `insertelement` by
    /// the splat.
    Splat {
        element: ValueSlot,
        insert: Option<ValueSlot>,
    },
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
    /// The element slots an every-lane or any-lane test over this element
    /// list or splat must visit: a list's every element, and a splat's one
    /// element, which each of its lanes is. `None` for any other storage.
    /// Not the lanes themselves: a reader that needs lane `i`, or the count,
    /// asks [`Constant::aggregate_element`] or the type.
    pub(crate) fn lane_slots(&self) -> Option<&[ValueSlot]> {
        match self {
            Self::Aggregate(elements) => Some(elements),
            Self::Splat { element, .. } => Some(core::slice::from_ref(element)),
            _ => None,
        }
    }

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
            // A scalable splat's operand is its `insertelement`, as upstream's
            // `shufflevector`'s is. A fixed splat uses its element once,
            // whatever the lane count: a `ConstantDataVector` keeps no use
            // list, and a `ConstantVector`'s one use per lane is
            // `docs/divergences.md` D39.
            Self::Splat { element, insert } => f(insert.unwrap_or(*element)),
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
            // `DSOLocalEquivalent` and `NoCFIValue` each hold their global as
            // `Op<0>` (`Constants.cpp`).
            Self::DsoLocalEquivalent { function } | Self::NoCfi { function } => f(*function),
            // llvmkit's compact spellings of a `getelementptr` on a global and
            // of `sub` / `add` over two `ptrtoint`s. Upstream's `getelementptr`
            // is a user of its global; in the `sub`, each global's user is the
            // `ptrtoint` naming it, which the compact form stands in for.
            Self::GepOffset { base_id, .. } => f(*base_id),
            Self::SymbolDelta { hi_id, lo_id } | Self::SymbolDeltaPlus { hi_id, lo_id, .. } => {
                f(*hi_id);
                f(*lo_id);
            }
            // `GlobalValueRef` *is* its global (`docs/divergences.md` D3): it
            // is no user of it, so it yields nothing. The rest have no
            // operands upstream.
            Self::Int(_)
            | Self::Float(_)
            | Self::GlobalValueRef { .. }
            | Self::PointerNull
            | Self::ForwardRefPlaceholder
            | Self::AggregateZero
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

/// The `C`s the canonicalisations below compare a list against — the null
/// value, `undef` and `poison` — and everything else.
///
/// Upstream's comparisons (`rangeOnlyContains(V, C)`, `V[i] != C`) test
/// uniqued pointers. Each of the three is unique for its type
/// (`Constant::getNullValue`, `UndefValue::get`, `PoisonValue::get`), and an
/// array or vector has one element type, so "`V[i]` is `C`" is "`V[i]` is the
/// same one of the three". A constant is at most one of them, so classifying
/// each element once keeps every list a single pass.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UniqueConstant {
    Null,
    Undefined(Undefined),
    Other,
}

impl UniqueConstant {
    fn of<'ctx, B: ModuleBrand + 'ctx, C: Capability>(constant: Constant<'ctx, B, C>) -> Self {
        if constant.is_null_value() {
            return Self::Null;
        }
        match undefined(constant) {
            Some(undefined) => Self::Undefined(undefined),
            None => Self::Other,
        }
    }
}

/// The operands of `shufflevector (insertelement (Into, Element, Index),
/// Other, Mask)`, the shape of the splat expression
/// `ConstantVector::getSplat` builds for a scalable vector. Only the shape:
/// `Constant::getSplatValue` (`constant_expression_splat_value`) and
/// `getSplat`'s own expression (`constants.rs::scalable_get_splat_element`)
/// each ask their own questions of the parts, so the one match serves both.
pub(crate) struct SplatExpression<'d> {
    /// The `insertelement` expression itself.
    pub(crate) insert: &'d ConstantExprData,
    pub(crate) into: ValueSlot,
    pub(crate) element: ValueSlot,
    pub(crate) index: ValueSlot,
    pub(crate) other: ValueSlot,
    pub(crate) mask: ValueSlot,
}

impl<'d> SplatExpression<'d> {
    pub(crate) fn of(
        context: &'d crate::llvm_context::Context,
        shuffle: &ConstantExprData,
    ) -> Option<Self> {
        if shuffle.opcode != ConstantExprOpcode::ShuffleVector {
            return None;
        }
        let [inserted, other, mask] = *shuffle.operands else {
            return None;
        };
        let ValueKindData::Constant(ConstantData::Expr(insert)) =
            &context.value_data(inserted).kind
        else {
            return None;
        };
        if insert.opcode != ConstantExprOpcode::InsertElement {
            return None;
        }
        let [into, element, index] = *insert.operands else {
            return None;
        };
        Some(Self {
            insert,
            into,
            element,
            index,
            other,
            mask,
        })
    }
}

/// The element at `slot` of an element list, minted as it is visited, so
/// classifying a list allocates nothing.
fn element_at<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    slot: ValueSlot,
) -> Constant<'ctx, B, C> {
    Constant::from_parts(Value::from_parts(slot, module, module.value_data(slot).ty))
}

/// Mirrors `getSequenceIfElementsMatch` (with `getIntSequenceIfElementsMatch`
/// and `getFPSequenceIfElementsMatch`): a `ConstantDataArray` /
/// `ConstantDataVector` when the first element is a `ConstantInt` of 8, 16, 32
/// or 64 bits and every element is a `ConstantInt`, or the first is a `half`,
/// `bfloat`, `float` or `double` `ConstantFP` and every element is a
/// `ConstantFP`. `None` is upstream's `nullptr`.
fn sequence_if_elements_match<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    first: Constant<'ctx, B, C>,
    slots: &[ValueSlot],
    sequence: ConstantSequentialKind,
) -> Option<ConstantKind<'ctx, B, C>> {
    let every = |class: fn(&ConstantKind<'ctx, B, C>) -> bool| {
        slots
            .iter()
            .all(|slot| class(&element_at(module, *slot).kind()))
    };
    let matched = match first.kind() {
        ConstantKind::Data(ConstantDataKind::Int(_))
            if matches!(
                first.ty().data(),
                TypeData::Integer {
                    bits: 8 | 16 | 32 | 64
                }
            ) =>
        {
            every(|kind| matches!(kind, ConstantKind::Data(ConstantDataKind::Int(_))))
        }
        ConstantKind::Data(ConstantDataKind::Float(_))
            if matches!(
                first.ty().data(),
                TypeData::Half | TypeData::Bfloat | TypeData::Float | TypeData::Double
            ) =>
        {
            every(|kind| matches!(kind, ConstantKind::Data(ConstantDataKind::Float(_))))
        }
        _ => false,
    };
    matched.then_some(ConstantKind::Data(ConstantDataKind::Sequential(sequence)))
}

/// The uniqued constant one of upstream's aggregate constructors returns in
/// place of an element list: a `ConstantAggregateZero`, or the `UndefValue` or
/// `PoisonValue` of the aggregate's type. [`canonical_aggregate`] answers
/// `None` for a list the constructor keeps, which upstream builds as a
/// `ConstantDataSequential` or a `ConstantAggregate` — one storage form here,
/// told apart by [`Constant::kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CanonicalAggregate {
    /// `ConstantAggregateZero::get(Ty)`.
    Zero,
    /// `UndefValue::get(Ty)` or `PoisonValue::get(Ty)`.
    Undefined(Undefined),
    /// A vector whose every lane is this element, which is none of the above:
    /// [`ConstantData::Splat`]. Upstream keeps the lanes; llvmkit stores the
    /// one element, as hardening.
    Splat(ValueSlot),
}

/// What `ConstantVector::getSplat` builds from `element`, after its zero,
/// `undef` and `poison` checks: the uniqued zero, the marker, or the splat.
/// Total, so a splat never reaches a list of its lanes.
///
/// `getSplat`'s fixed branch returns `ConstantDataVector::getSplat` or
/// `get(SmallVector<Constant *, 32>(N, V))`, and each canonicalises an
/// all-null list to a `ConstantAggregateZero` and an all-`undef` or
/// all-`poison` one to the marker before keeping it. Its scalable branch tests
/// `V->isNullValue()`, then `isa<PoisonValue>(V)`, then `isa<UndefValue>(V)`.
/// A constant is at most one of the three, so the order of the two is one
/// answer.
pub(crate) fn splat_canonical<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    element: ValueSlot,
) -> CanonicalAggregate {
    match UniqueConstant::of(element_at(module, element)) {
        UniqueConstant::Null => CanonicalAggregate::Zero,
        UniqueConstant::Undefined(undefined) => CanonicalAggregate::Undefined(undefined),
        UniqueConstant::Other => CanonicalAggregate::Splat(element),
    }
}

/// Which uniqued constant upstream's constructor for an aggregate of type
/// `ty` builds from the element list `slots`, if it builds one:
/// `ConstantArray::getImpl` for an array, `ConstantStruct::get` for a struct,
/// `ConstantVector::getImpl` for a fixed vector, and `ConstantVector::getSplat`
/// for a scalable one — upstream's only constructor for a scalable vector
/// constant, which `VectorType::const_vector` mirrors by demanding that a
/// scalable list's lanes agree, and which always answers here (a splat, a
/// zero or a marker), so no scalable list is ever stored.
///
/// `intern_aggregate` asks this before it stores a list, so a stored list is
/// never one of these, and [`Constant::kind`] reads the class off the storage
/// rather than replaying the canonicalisation. Every caller passes an array,
/// struct or vector type; any other is answered `None`.
pub(crate) fn canonical_aggregate<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    ty: &TypeData,
    slots: &[ValueSlot],
) -> Option<CanonicalAggregate> {
    match ty {
        TypeData::Array { .. } => array_get_impl(module, slots),
        TypeData::Struct(_) => struct_get(module, slots),
        TypeData::FixedVector { .. } => vector_get_impl(module, slots),
        TypeData::ScalableVector { .. } => Some(scalable_vector_get_splat(module, slots)),
        _ => None,
    }
}

/// An aggregate type's element count as upstream counts it, in `unsigned`:
/// `ConstantAggregateZero::getElementCount().getKnownMinValue()`, and
/// `UndefValue::getNumElements()`, which `PoisonValue` shares.
///
/// An array's length is the `uint64_t` `ArrayType::getNumElements` returns,
/// and both routines convert it to `unsigned` (`ElementCount` is
/// `FixedOrScalableQuantity<ElementCount, unsigned>`), which keeps its low 32
/// bits: `[4294967304 x i8]` counts 8 elements and `[1099511627776 x i8]` none.
/// A vector counts its (known minimum) lanes and a struct its fields, none for
/// an opaque one. `None` for a type that is no array, struct or vector.
pub(crate) fn aggregate_element_count(ty: &TypeData) -> Option<u32> {
    if let Some((_, length)) = ty.as_array() {
        let [b0, b1, b2, b3, ..] = length.to_le_bytes();
        return Some(u32::from_le_bytes([b0, b1, b2, b3]));
    }
    if let Some((_, lanes, _)) = ty.as_vector() {
        return Some(lanes.get());
    }
    let body = ty.as_struct()?.body.borrow();
    Some(body.as_ref().map_or(0, |body| {
        // `getStructNumElements` is `unsigned` too.
        let [b0, b1, b2, b3, ..] = body.elements.len().to_le_bytes();
        u32::from_le_bytes([b0, b1, b2, b3])
    }))
}

/// The type of element `index` of an aggregate type: the element type of an
/// array or a vector (`getSequentialElement`), the field type of a struct
/// (`getStructElementType(Idx)`). `None` past a struct's fields.
fn aggregate_element_type_slot(ty: &TypeData, index: u32) -> Option<TypeSlot> {
    if let Some((element, _)) = ty.as_array() {
        return Some(element);
    }
    if let Some((element, _, _)) = ty.as_vector() {
        return Some(element);
    }
    let body = ty.as_struct()?.body.borrow();
    body.as_ref()?
        .elements
        .get(usize::try_from(index).ok()?)
        .copied()
}

/// The canonicalisations of `ConstantArray::getImpl`, in its order. Its last
/// arm, `getSequenceIfElementsMatch<ConstantDataArray>`, answers a list —
/// a `ConstantDataArray` is one here too — so it is [`Constant::kind`]'s.
fn array_get_impl<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    slots: &[ValueSlot],
) -> Option<CanonicalAggregate> {
    // Empty arrays are canonicalized to ConstantAggregateZero.
    let Some(&first_slot) = slots.first() else {
        return Some(CanonicalAggregate::Zero);
    };
    // If this is an all-zero array, return a ConstantAggregateZero object. If
    // all undef, return an UndefValue, if "all simple", then return a
    // ConstantDataArray.
    let first = UniqueConstant::of(element_at(module, first_slot));
    // `rangeOnlyContains(V.begin(), V.end(), C)`, where `C` is `V[0]`.
    let range_only_contains_first = || {
        slots
            .iter()
            .all(|slot| UniqueConstant::of(element_at(module, *slot)) == first)
    };
    // `if (isa<PoisonValue>(C) && rangeOnlyContains(...)) return
    // PoisonValue::get(Ty);`
    if first == UniqueConstant::Undefined(Undefined::Poison) && range_only_contains_first() {
        return Some(CanonicalAggregate::Undefined(Undefined::Poison));
    }
    // `if (isa<UndefValue>(C) && rangeOnlyContains(...)) return
    // UndefValue::get(Ty);` — `isa<UndefValue>` holds for a poison too.
    if matches!(first, UniqueConstant::Undefined(_)) && range_only_contains_first() {
        return Some(CanonicalAggregate::Undefined(Undefined::Undef));
    }
    // `if (C->isNullValue() && rangeOnlyContains(...)) return
    // ConstantAggregateZero::get(Ty);`
    if first == UniqueConstant::Null && range_only_contains_first() {
        return Some(CanonicalAggregate::Zero);
    }
    None
}

/// The class of a list `ConstantArray::getImpl` or `ConstantVector::getImpl`
/// kept: their closing `getSequenceIfElementsMatch`, else the
/// `ConstantArray` or `ConstantVector` their callers build.
fn sequence_or_aggregate_kind<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    slots: &[ValueSlot],
    sequence: ConstantSequentialKind,
    aggregate: ConstantAggregateKind,
) -> ConstantKind<'ctx, B, C> {
    // Check to see if all of the elements are ConstantFP or ConstantInt and if
    // the element type is compatible with ConstantDataVector. If so, use it.
    if let Some(&first_slot) = slots.first() {
        let first = element_at(module, first_slot);
        if is_element_type_compatible(first.ty().data())
            && let Some(kind) = sequence_if_elements_match(module, first, slots, sequence)
        {
            return kind;
        }
    }
    // Otherwise, we really do want to create a ConstantArray.
    ConstantKind::Aggregate(aggregate)
}

/// The canonicalisations of `ConstantStruct::get`. What it does not
/// canonicalise it builds as a `ConstantStruct`.
fn struct_get<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    slots: &[ValueSlot],
) -> Option<CanonicalAggregate> {
    // Create a ConstantAggregateZero value if all elements are zeros.
    let mut is_zero = true;
    let mut is_undef = false;
    let mut is_poison = false;
    if let Some(&first_slot) = slots.first() {
        let first = UniqueConstant::of(element_at(module, first_slot));
        is_undef = matches!(first, UniqueConstant::Undefined(_));
        is_poison = first == UniqueConstant::Undefined(Undefined::Poison);
        is_zero = first == UniqueConstant::Null;
        // PoisonValue inherits UndefValue, so its check is not necessary.
        if is_undef || is_zero {
            for slot in slots {
                let element = UniqueConstant::of(element_at(module, *slot));
                if element != UniqueConstant::Null {
                    is_zero = false;
                }
                if element != UniqueConstant::Undefined(Undefined::Poison) {
                    is_poison = false;
                }
                if element == UniqueConstant::Undefined(Undefined::Poison)
                    || !matches!(element, UniqueConstant::Undefined(_))
                {
                    is_undef = false;
                }
            }
        }
    }
    if is_zero {
        return Some(CanonicalAggregate::Zero);
    }
    if is_poison {
        return Some(CanonicalAggregate::Undefined(Undefined::Poison));
    }
    if is_undef {
        return Some(CanonicalAggregate::Undefined(Undefined::Undef));
    }
    None
}

/// The canonicalisations of `ConstantVector::getImpl`, in its order. Its last
/// arm, `getSequenceIfElementsMatch<ConstantDataVector>`, answers a list, so it
/// is [`Constant::kind`]'s.
fn vector_get_impl<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    slots: &[ValueSlot],
) -> Option<CanonicalAggregate> {
    // `assert(!V.empty() && "Vectors can't be empty")`. A vector type's lane
    // count is a `NonZeroU32` and `VectorType::const_vector` takes one element
    // per lane, so no list is empty; one would answer as
    // `ConstantArray::getImpl` answers an empty array, rather than assert.
    let Some(&first_slot) = slots.first() else {
        return Some(CanonicalAggregate::Zero);
    };
    // If this is an all-undef or all-zero vector, return a
    // ConstantAggregateZero or UndefValue.
    let first_class = UniqueConstant::of(element_at(module, first_slot));
    let mut is_zero = first_class == UniqueConstant::Null;
    let mut is_undef = matches!(first_class, UniqueConstant::Undefined(_));
    let mut is_poison = first_class == UniqueConstant::Undefined(Undefined::Poison);
    // Upstream also carries `isSplatFP` / `isSplatInt` here, gated by the
    // hidden `-use-constant-fp-for-fixed-length-splat` and
    // `-use-constant-int-for-fixed-length-splat` options. Both default to
    // `false` and llvmkit has no option surface, so neither arm can run.
    //
    // `V[i] != C` compares uniqued pointers, which is comparing each element's
    // `UniqueConstant` with `C`'s.
    if (is_zero || is_undef)
        && slots
            .iter()
            .skip(1)
            .any(|slot| UniqueConstant::of(element_at(module, *slot)) != first_class)
    {
        is_zero = false;
        is_undef = false;
        is_poison = false;
    }
    if is_zero {
        return Some(CanonicalAggregate::Zero);
    }
    if is_poison {
        return Some(CanonicalAggregate::Undefined(Undefined::Poison));
    }
    if is_undef {
        return Some(CanonicalAggregate::Undefined(Undefined::Undef));
    }
    // Not upstream's: a list whose lanes are all one constant is stored as
    // that constant (`ConstantData::Splat`), where upstream's
    // `getSequenceIfElementsMatch` / `ConstantVector` keeps every lane.
    // Constants are uniqued, so "every lane is `V[0]`" is slot equality.
    if slots.iter().all(|slot| *slot == first_slot) {
        return Some(CanonicalAggregate::Splat(first_slot));
    }
    None
}

/// The canonicalisations of `ConstantVector::getSplat`'s scalable branch.
/// `VectorType::const_vector` demands a scalable list's lanes agree, so the
/// list is a splat of its first element, stored as [`ConstantData::Splat`]
/// unless it is a zero or a marker.
fn scalable_vector_get_splat<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    slots: &[ValueSlot],
) -> CanonicalAggregate {
    // Upstream first tries the hidden `-use-constant-int-for-scalable-splat`
    // and `-use-constant-fp-for-scalable-splat` options; both default to
    // `false` and llvmkit has no option surface, so neither arm can run. An
    // empty list answers as `vector_get_impl` answers one.
    let Some(&splat_slot) = slots.first() else {
        return CanonicalAggregate::Zero;
    };
    // `if (V->isNullValue())`, then `isa<PoisonValue>(V)`, then
    // `isa<UndefValue>(V)`. Upstream builds anything else as
    // `shufflevector (insertelement poison, V, 0), poison, zeroinitializer`;
    // llvmkit stores the splat, which `Constant::kind` classifies as that
    // expression.
    splat_canonical(module, splat_slot)
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
    /// representation. A splat, fixed or scalable, is stored as its one
    /// element and answers it directly, `getSplat`'s scalable `shufflevector`
    /// expression included. The constant-*expression* splat form is handled
    /// in a private helper below for the expressions that are no such splat:
    /// a scalable `shufflevector` of an `insertelement` written with an index
    /// type other than `i64`, which upstream also keeps as an expression (a
    /// fixed one folds to a splat).
    pub fn splat_value(self, allow_poison: bool) -> Option<Constant<'ctx, B, C>> {
        let (element_ty, _, _) = self.ty().data().as_vector()?;
        let element_ty = Type::new(element_ty, self.module);

        match &self.as_erased().data().kind {
            ValueKindData::Constant(ConstantData::Poison) => {
                return Some(element_ty.poison().as_constant());
            }
            // `if (isa<ConstantAggregateZero>(this)) return
            // getNullValue(cast<VectorType>(getType())->getElementType());`
            ValueKindData::Constant(ConstantData::AggregateZero) => {
                return element_ty.null_value();
            }
            // `dyn_cast<ConstantDataVector>` and `dyn_cast<ConstantVector>`,
            // one element list here; the loop below is their `getSplatValue`.
            ValueKindData::Constant(ConstantData::Aggregate(_)) => {}
            // Every lane is the element, which is no `poison`: both
            // `getSplatValue`s answer it whatever `AllowPoison` says, and so
            // does the constant-expression form a scalable splat is upstream.
            ValueKindData::Constant(ConstantData::Splat { element, .. }) => {
                return Some(element_at(self.module, *element));
            }
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
        let parts = SplatExpression::of(module.module().context(), shuffle)?;
        if !is_undefined(at(parts.other)) || !is_undefined(at(parts.into)) {
            return None;
        }
        let (index, shuffle_mask, splat) = (parts.index, parts.mask, parts.element);

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
    /// The zero, `undef` and `poison` an aggregate constructor canonicalises
    /// a list to are stored as such (`intern_aggregate` canonicalises before
    /// it stores a list), so they are read off the storage. What
    /// is left is one storage form for upstream's `ConstantDataSequential`
    /// and `ConstantAggregate` classes, told apart by the closing
    /// `getSequenceIfElementsMatch` of `ConstantArray::getImpl` and
    /// `ConstantVector::getImpl`. A splat (stored as its one element) is
    /// classified as its lanes would be, and a scalable one is the
    /// `shufflevector` expression `ConstantVector::getSplat` builds.
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
                 call passes a `ValueKindData::Constant` slot, an element or operand of one, or a \
                 global's own slot — audited per call by tests/constant_handle_mint_drift.rs"
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
            // A splat is classified as its lanes would be: every lane is
            // `element`, so `getSequenceIfElementsMatch` over the lanes is its
            // answer over the one element.
            ConstantData::Splat { element, .. } => {
                self.element_list_kind(core::slice::from_ref(element))
            }
            ConstantData::AggregateZero => ConstantKind::Data(ConstantDataKind::AggregateZero),
            ConstantData::Expr(expr) => ConstantKind::Expr(expr.opcode),
            // llvmkit's compact forms, each printed as the constant expression
            // its variant's doc comment names, and classified as upstream's
            // constructor leaves that expression. `ConstantFoldGetElementPtr`
            // returns the base pointer for a GEP whose indices are all null
            // (its `IsNoOp`), so a zero offset is the base global itself; and
            // `ConstantFoldBinaryInstruction` returns `X` for `add X, 0` (its
            // `getBinOpIdentity` fold), so a zero addend is the plain delta.
            ConstantData::GepOffset { off: 0, .. } => ConstantKind::GlobalValue,
            ConstantData::GepOffset { .. } => ConstantKind::Expr(ConstantExprOpcode::GetElementPtr),
            ConstantData::SymbolDelta { .. } => ConstantKind::Expr(ConstantExprOpcode::Sub),
            ConstantData::SymbolDeltaPlus { addend: 0, .. } => {
                ConstantKind::Expr(ConstantExprOpcode::Sub)
            }
            ConstantData::SymbolDeltaPlus { .. } => ConstantKind::Expr(ConstantExprOpcode::Add),
            ConstantData::GlobalValueRef { .. } => ConstantKind::GlobalValue,
            ConstantData::BlockAddress { .. } => ConstantKind::BlockAddress,
            ConstantData::DsoLocalEquivalent { .. } => ConstantKind::DsoLocalEquivalent,
            ConstantData::NoCfi { .. } => ConstantKind::NoCfi,
            ConstantData::PtrAuth { .. } => ConstantKind::PtrAuth,
            ConstantData::ForwardRefPlaceholder => ConstantKind::ForwardReference,
        }
    }

    /// The class of a stored element list. See [`Self::kind`]. A struct list
    /// is a `ConstantStruct`, even an all-null one: only
    /// `ConstantStruct::handleOperandChangeImpl`'s in-place update leaves such a
    /// list, and upstream leaves that `ConstantStruct` too.
    fn element_list_kind(self, slots: &[ValueSlot]) -> ConstantKind<'ctx, B, C> {
        let module = self.module;
        match self.ty().data() {
            TypeData::Array { .. } => sequence_or_aggregate_kind(
                module,
                slots,
                ConstantSequentialKind::Array,
                ConstantAggregateKind::Array,
            ),
            TypeData::Struct(_) => ConstantKind::Aggregate(ConstantAggregateKind::Struct),
            TypeData::FixedVector { .. } => sequence_or_aggregate_kind(
                module,
                slots,
                ConstantSequentialKind::Vector,
                ConstantAggregateKind::Vector,
            ),
            // `ConstantVector::getSplat` builds the splat of a value that is no
            // null, `undef` or `poison` as
            // `shufflevector (insertelement poison, V, 0), poison, zeroinitializer`.
            TypeData::ScalableVector { .. } => {
                ConstantKind::Expr(ConstantExprOpcode::ShuffleVector)
            }
            _ => unreachable!(
                "an element list is interned only by `intern_element_list` and the struct arm of \
                 `aggregate_with_replaced_operand`, each at an array, struct or vector type"
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

    /// Whether this constant is the all-zero value of its type: `0`, `+0.0`,
    /// `null`, `token none`, `target(...) none`, or a `ConstantAggregateZero`.
    /// Ports `Constant::isNullValue`; constant expressions, `blockaddress`,
    /// global values, the symbol-relative payloads and element lists are never
    /// null, as upstream answers too — a list of nulls is a
    /// `ConstantAggregateZero` from its constructor on, but for the
    /// `ConstantStruct` an in-place operand change can leave.
    pub fn is_null_value(self) -> bool {
        crate::constants::constant_id_is_null_value(self.module.module(), self.id)
    }

    /// Whether every bit of this constant is set.
    ///
    /// Ports `Constant::isAllOnesValue`, whose three arms this follows: an
    /// integer equal to `-1`, a float whose bit pattern is all ones, and a
    /// vector that splats either. Note the asymmetry with
    /// [`Self::is_null_value`], which asks the class: upstream routes this one
    /// through `getSplatValue`, so a vector of all-ones lanes qualifies only
    /// where that recognises a splat.
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
    /// Upstream reaches the answer through seven `dyn_cast` arms and a
    /// scalable-vector bail-out, taken here in its order.
    /// `ConstantDataSequential` is an ordinary element list here, so it
    /// arrives at the stored-element-list arm, which it may: a
    /// `ConstantDataSequential` is never scalable. A fixed
    /// splat (one stored element) is a `ConstantDataVector` or a `ConstantVector`
    /// and answers from the same arm; a *scalable* one stands for the
    /// `shufflevector` splat expression `ConstantVector::getSplat` builds,
    /// which no `dyn_cast` admits, so it reaches the bail-out. The
    /// vector-typed `ConstantInt` and `ConstantFP`
    /// splat forms have no llvmkit representation at all, so those two arms
    /// are unreachable rather than missing.
    ///
    /// The bounds are upstream's `unsigned` counts, so past 2^32 elements an
    /// array answers as its length's low 32 bits say: `[4294967304 x i8]` has
    /// elements 0 to 7, and `[1099511627776 x i8]` none.
    pub fn aggregate_element(self, index: u32) -> Option<Constant<'ctx, B, C>> {
        let value = self.as_erased();
        let ValueKindData::Constant(constant) = &value.data().kind else {
            return None;
        };
        let data = self.ty().data();
        let scalable = matches!(data, TypeData::ScalableVector { .. });
        match constant {
            // `if (const auto *CC = dyn_cast<ConstantAggregate>(this)) return
            // Elt < CC->getNumOperands() ? CC->getOperand(Elt) : nullptr;`
            // The element is stored, so it is handed back rather than rebuilt.
            ConstantData::Aggregate(elements) if !scalable => {
                let slot = *elements.get(usize::try_from(index).ok()?)?;
                let module = self.module;
                Some(Constant::from_parts(Value::from_parts(
                    slot,
                    module,
                    module.value_data(slot).ty,
                )))
            }
            // The same arm for a fixed splat, a `ConstantDataVector` or a
            // `ConstantVector` upstream: every lane is the element. A scalable
            // splat is upstream's `shufflevector` expression and reaches the
            // bail-out below.
            ConstantData::Splat { element, .. } if !scalable => {
                if index >= aggregate_element_count(data)? {
                    return None;
                }
                Some(element_at(self.module, *element))
            }
            // `if (const auto *CAZ = dyn_cast<ConstantAggregateZero>(this))
            // return Elt < CAZ->getElementCount().getKnownMinValue() ?
            // CAZ->getElementValue(Elt) : nullptr;` — above the bail-out, so a
            // scalable zero answers. `getElementValue` is the element type's
            // null value for an array or a vector (`getSequentialElement`) and
            // the field's for a struct (`getStructElement`). A type with no
            // null value answers `None`, where upstream's
            // `ConstantTargetNone::get` asserts.
            ConstantData::AggregateZero => {
                if index >= aggregate_element_count(data)? {
                    return None;
                }
                Type::new(aggregate_element_type_slot(data, index)?, self.module).null_value()
            }
            // `// FIXME: getNumElements() will fail for non-fixed vector types.
            // if (isa<ScalableVectorType>(getType())) return nullptr;`
            _ if scalable => None,
            // `if (const auto *PV = dyn_cast<PoisonValue>(this)) return Elt <
            // PV->getNumElements() ? PV->getElementValue(Elt) : nullptr;`
            ConstantData::Poison => {
                Some(self.undefined_element_type(index)?.poison().as_constant())
            }
            // The same for `dyn_cast<UndefValue>`.
            ConstantData::Undef => Some(self.undefined_element_type(index)?.undef().as_constant()),
            _ => None,
        }
    }

    /// How many lanes of a fixed vector of `lanes` lanes a walk that asks
    /// every lane one question (an every-lane or any-lane test) has to ask:
    /// one when [`Self::has_uniform_lanes`] says every lane is one constant, all of
    /// them otherwise. A lane walk over a constant goes through this, so a
    /// splat, a zero, `undef` or `poison` of 2^32 lanes costs one
    /// (`docs/divergences.md` D40).
    pub(crate) fn lanes_to_ask(self, lanes: u32) -> u32 {
        if self.has_uniform_lanes() { 1 } else { lanes }
    }

    /// Whether every lane of this fixed vector is one constant, read off its
    /// storage alone: a splat, a zero, `undef` or `poison`.
    /// [`Self::aggregate_element`] answers that constant for each lane, so a
    /// walk that asks every lane the same question (an every-lane or
    /// any-lane test) may ask it of lane 0 once and answer as the walk would:
    /// a fixed vector has at least one lane. `false` for an element list, a
    /// constant expression, a scalable vector and anything that is no vector.
    ///
    /// No upstream counterpart: upstream stores every lane of a fixed vector
    /// and walks them. llvmkit stores these four in one slot, and a walk over
    /// their lanes would cost the lane count (`docs/divergences.md` D40).
    pub(crate) fn has_uniform_lanes(self) -> bool {
        matches!(self.ty().data(), TypeData::FixedVector { .. })
            && matches!(
                &self.as_erased().data().kind,
                ValueKindData::Constant(
                    ConstantData::Splat { .. }
                        | ConstantData::AggregateZero
                        | ConstantData::Undef
                        | ConstantData::Poison
                )
            )
    }

    /// The element type [`Self::aggregate_element`] hands an `undef` or
    /// `poison` marker back at, or `None` when `index` is past the end:
    /// `Elt < UV->getNumElements()` (an `unsigned`, [`aggregate_element_count`])
    /// and the type `UndefValue::getElementValue(Elt)` builds its marker at.
    /// The caller has taken the scalable bail-out already, as upstream's
    /// `getNumElements` needs.
    fn undefined_element_type(self, index: u32) -> Option<Type<'ctx, B, C>> {
        let data = self.ty().data();
        if index >= aggregate_element_count(data)? {
            return None;
        }
        Some(Type::new(
            aggregate_element_type_slot(data, index)?,
            self.module,
        ))
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
