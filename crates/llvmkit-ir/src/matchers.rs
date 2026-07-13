//! A `PatternMatch.h`-style combinator DSL for inspecting instructions.
//!
//! Mirrors `llvm/include/llvm/IR/PatternMatch.h`: small, composable
//! matchers that test an instruction's shape and *bind* the interesting
//! sub-values as a side effect of matching. Unlike the C++ version — which
//! binds through `Value *&` out-parameters — a llvmkit matcher **returns**
//! its bindings, so a failed match is `None` (never a half-filled slot) and
//! the bound tuple is composed and type-checked by the compiler.
//!
//! ```ignore
//! // add (sub X, Y), -1   -->   binds (x, y)
//! use llvmkit_ir::matchers::*;
//! if let Some((x, y)) =
//!     m_add(m_one_use(m_sub(m_value(), m_value())), m_all_ones()).match_view(&view)
//! {
//!     // x, y: Value
//! }
//! ```
//!
//! A matcher's `Bindings` is a flat tuple: `m_value()` contributes one
//! slot, matchers that only test (like `m_all_ones()`) contribute none, and
//! a composite concatenates its children's bindings via [`Combine`]. Flat
//! tuples are provided for up to four bound values.

use crate::ap_int::ApInt;
use crate::instr_types::BinaryOpcode;
use crate::instruction::{InstructionKind, InstructionView, PhiKind};
use crate::module::{Brand, ModuleBrand};
use crate::value::Value;

/// A composable instruction/value matcher. Implementors test `value` and,
/// on success, return the values they bind.
pub trait Matcher<'ctx, B: ModuleBrand + 'ctx = Brand<'ctx>> {
    /// The flat tuple of values this matcher binds on success.
    type Bindings;

    /// Test `value`; return the bindings if it matches.
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings>;

    /// Convenience: match against a rediscovered [`InstructionView`].
    #[inline]
    fn match_view(&self, view: &InstructionView<'ctx, B>) -> Option<Self::Bindings> {
        self.try_match(view.as_value())
    }
}

// --------------------------------------------------------------------------
// Combine: flat-tuple concatenation of two binding sets
// --------------------------------------------------------------------------

/// Concatenate two binding tuples into one flat tuple. Implemented for
/// every pair of arities whose sum is at most four, which is what the
/// composite matchers need to keep their `Bindings` a flat tuple.
pub trait Combine<Rhs> {
    /// The concatenated tuple type.
    type Out;
    /// Concatenate `self` (left bindings) with `rhs` (right bindings).
    fn combine(self, rhs: Rhs) -> Self::Out;
}

// 0 + n
impl Combine<()> for () {
    type Out = ();
    #[inline]
    fn combine(self, _rhs: ()) -> Self::Out {}
}
impl<R0> Combine<(R0,)> for () {
    type Out = (R0,);
    #[inline]
    fn combine(self, rhs: (R0,)) -> Self::Out {
        rhs
    }
}
impl<R0, R1> Combine<(R0, R1)> for () {
    type Out = (R0, R1);
    #[inline]
    fn combine(self, rhs: (R0, R1)) -> Self::Out {
        rhs
    }
}
impl<R0, R1, R2> Combine<(R0, R1, R2)> for () {
    type Out = (R0, R1, R2);
    #[inline]
    fn combine(self, rhs: (R0, R1, R2)) -> Self::Out {
        rhs
    }
}
impl<R0, R1, R2, R3> Combine<(R0, R1, R2, R3)> for () {
    type Out = (R0, R1, R2, R3);
    #[inline]
    fn combine(self, rhs: (R0, R1, R2, R3)) -> Self::Out {
        rhs
    }
}

// 1 + n
impl<L0> Combine<()> for (L0,) {
    type Out = (L0,);
    #[inline]
    fn combine(self, _rhs: ()) -> Self::Out {
        self
    }
}
impl<L0, R0> Combine<(R0,)> for (L0,) {
    type Out = (L0, R0);
    #[inline]
    fn combine(self, rhs: (R0,)) -> Self::Out {
        (self.0, rhs.0)
    }
}
impl<L0, R0, R1> Combine<(R0, R1)> for (L0,) {
    type Out = (L0, R0, R1);
    #[inline]
    fn combine(self, rhs: (R0, R1)) -> Self::Out {
        (self.0, rhs.0, rhs.1)
    }
}
impl<L0, R0, R1, R2> Combine<(R0, R1, R2)> for (L0,) {
    type Out = (L0, R0, R1, R2);
    #[inline]
    fn combine(self, rhs: (R0, R1, R2)) -> Self::Out {
        (self.0, rhs.0, rhs.1, rhs.2)
    }
}

// 2 + n
impl<L0, L1> Combine<()> for (L0, L1) {
    type Out = (L0, L1);
    #[inline]
    fn combine(self, _rhs: ()) -> Self::Out {
        self
    }
}
impl<L0, L1, R0> Combine<(R0,)> for (L0, L1) {
    type Out = (L0, L1, R0);
    #[inline]
    fn combine(self, rhs: (R0,)) -> Self::Out {
        (self.0, self.1, rhs.0)
    }
}
impl<L0, L1, R0, R1> Combine<(R0, R1)> for (L0, L1) {
    type Out = (L0, L1, R0, R1);
    #[inline]
    fn combine(self, rhs: (R0, R1)) -> Self::Out {
        (self.0, self.1, rhs.0, rhs.1)
    }
}

// 3 + n
impl<L0, L1, L2> Combine<()> for (L0, L1, L2) {
    type Out = (L0, L1, L2);
    #[inline]
    fn combine(self, _rhs: ()) -> Self::Out {
        self
    }
}
impl<L0, L1, L2, R0> Combine<(R0,)> for (L0, L1, L2) {
    type Out = (L0, L1, L2, R0);
    #[inline]
    fn combine(self, rhs: (R0,)) -> Self::Out {
        (self.0, self.1, self.2, rhs.0)
    }
}

// 4 + 0
impl<L0, L1, L2, L3> Combine<()> for (L0, L1, L2, L3) {
    type Out = (L0, L1, L2, L3);
    #[inline]
    fn combine(self, _rhs: ()) -> Self::Out {
        self
    }
}

// --------------------------------------------------------------------------
// m_value / m_specific / m_same_as — the leaf value matchers
// --------------------------------------------------------------------------

/// Matches any value and binds it. Mirrors `m_Value(V)`.
pub fn m_value<B: ModuleBrand>() -> MValue<B> {
    MValue(core::marker::PhantomData)
}

/// Matcher returned by [`m_value`].
pub struct MValue<B>(core::marker::PhantomData<fn() -> B>);

impl<'ctx, B: ModuleBrand + 'ctx> Matcher<'ctx, B> for MValue<B> {
    type Bindings = (Value<'ctx, B>,);
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        Some((value,))
    }
}

/// Matches only the given value; binds nothing. Mirrors `m_Specific(V)`.
pub fn m_specific<'ctx, B: ModuleBrand>(expected: Value<'ctx, B>) -> MSpecific<'ctx, B> {
    MSpecific(expected)
}

/// Matcher returned by [`m_specific`].
pub struct MSpecific<'ctx, B: ModuleBrand>(Value<'ctx, B>);

impl<'ctx, B: ModuleBrand + 'ctx> Matcher<'ctx, B> for MSpecific<'ctx, B> {
    type Bindings = ();
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        (value == self.0).then_some(())
    }
}

// --------------------------------------------------------------------------
// Constant-integer predicates
// --------------------------------------------------------------------------

/// A constant-integer predicate matcher; binds nothing. Backs `m_zero`,
/// `m_one`, `m_all_ones`, ... Scalar only (vector splats are not unwrapped).
pub struct MConstIntPred {
    pred: fn(&ApInt) -> bool,
}

impl<'ctx, B: ModuleBrand + 'ctx> Matcher<'ctx, B> for MConstIntPred {
    type Bindings = ();
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        let ap = value.as_const_int()?;
        (self.pred)(&ap).then_some(())
    }
}

/// Matches a constant zero. Mirrors `m_Zero`.
pub fn m_zero() -> MConstIntPred {
    MConstIntPred {
        pred: ApInt::is_zero,
    }
}
/// Matches a constant one. Mirrors `m_One`.
pub fn m_one() -> MConstIntPred {
    MConstIntPred {
        pred: ApInt::is_one,
    }
}
/// Matches an all-ones constant (`-1`). Mirrors `m_AllOnes`.
pub fn m_all_ones() -> MConstIntPred {
    MConstIntPred {
        pred: ApInt::is_all_ones,
    }
}
/// Matches a power-of-two constant. Mirrors `m_Power2`.
pub fn m_power2() -> MConstIntPred {
    MConstIntPred {
        pred: ApInt::is_power_of_2,
    }
}
/// Matches a negative constant. Mirrors `m_Negative`.
pub fn m_negative() -> MConstIntPred {
    MConstIntPred {
        pred: ApInt::is_negative,
    }
}
/// Matches a non-negative constant. Mirrors `m_NonNegative`.
pub fn m_non_negative() -> MConstIntPred {
    MConstIntPred {
        pred: ApInt::is_non_negative,
    }
}

/// Matches any constant integer and binds its [`ApInt`]. Mirrors `m_APInt`.
pub fn m_ap_int() -> MApInt {
    MApInt
}

/// Matcher returned by [`m_ap_int`].
pub struct MApInt;

impl<'ctx, B: ModuleBrand + 'ctx> Matcher<'ctx, B> for MApInt {
    type Bindings = (ApInt,);
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        value.as_const_int().map(|ap| (ap,))
    }
}

/// Matches a constant integer equal (as a signed value) to `n`. Mirrors
/// `m_SpecificInt`. Binds nothing.
pub fn m_specific_int(n: i128) -> MSpecificInt {
    MSpecificInt(n)
}

/// Matcher returned by [`m_specific_int`].
pub struct MSpecificInt(i128);

impl<'ctx, B: ModuleBrand + 'ctx> Matcher<'ctx, B> for MSpecificInt {
    type Bindings = ();
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        let ap = value.as_const_int()?;
        (ap.try_sext_i128() == Some(self.0)).then_some(())
    }
}

// --------------------------------------------------------------------------
// m_one_use — gate a sub-pattern on the value having exactly one use
// --------------------------------------------------------------------------

/// Matches `inner` only when the value has exactly one use. Mirrors
/// `m_OneUse` — the gate peephole rewrites use to avoid duplicating a
/// shared sub-expression.
pub fn m_one_use<M>(inner: M) -> MOneUse<M> {
    MOneUse(inner)
}

/// Matcher returned by [`m_one_use`].
pub struct MOneUse<M>(M);

impl<'ctx, B, M> Matcher<'ctx, B> for MOneUse<M>
where
    B: ModuleBrand + 'ctx,
    M: Matcher<'ctx, B>,
{
    type Bindings = M::Bindings;
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        if !value.has_one_use() {
            return None;
        }
        self.0.try_match(value)
    }
}

// --------------------------------------------------------------------------
// Binary-operator matchers
// --------------------------------------------------------------------------

/// A binary-operator matcher over a fixed opcode with sub-matchers for the
/// two operands. When `commutative` is set, the swapped operand order is
/// tried too (built by the `m_c_*` factories).
pub struct MBinOp<L, R> {
    opcode: BinaryOpcode,
    lhs: L,
    rhs: R,
    commutative: bool,
}

impl<'ctx, B, L, R> Matcher<'ctx, B> for MBinOp<L, R>
where
    B: ModuleBrand + 'ctx,
    L: Matcher<'ctx, B>,
    R: Matcher<'ctx, B>,
    L::Bindings: Combine<R::Bindings>,
{
    type Bindings = <L::Bindings as Combine<R::Bindings>>::Out;
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        let view = InstructionView::try_from(value).ok()?;
        let bop = view.kind()?.as_binary_op()?;
        if bop.opcode() != self.opcode {
            return None;
        }
        if let (Some(lb), Some(rb)) = (self.lhs.try_match(bop.lhs()), self.rhs.try_match(bop.rhs()))
        {
            return Some(lb.combine(rb));
        }
        if self.commutative
            && let (Some(lb), Some(rb)) =
                (self.lhs.try_match(bop.rhs()), self.rhs.try_match(bop.lhs()))
        {
            return Some(lb.combine(rb));
        }
        None
    }
}

macro_rules! binop_matcher {
    ($(#[$attr:meta])* $name:ident, $opcode:ident) => {
        $(#[$attr])*
        pub fn $name<L, R>(lhs: L, rhs: R) -> MBinOp<L, R> {
            MBinOp { opcode: BinaryOpcode::$opcode, lhs, rhs, commutative: false }
        }
    };
}

macro_rules! commutative_binop_matcher {
    ($(#[$attr:meta])* $name:ident, $opcode:ident) => {
        $(#[$attr])*
        pub fn $name<L, R>(lhs: L, rhs: R) -> MBinOp<L, R> {
            MBinOp { opcode: BinaryOpcode::$opcode, lhs, rhs, commutative: true }
        }
    };
}

binop_matcher!(
    /// `add`. Mirrors `m_Add`.
    m_add, Add
);
binop_matcher!(
    /// `sub`. Mirrors `m_Sub`.
    m_sub, Sub
);
binop_matcher!(
    /// `mul`. Mirrors `m_Mul`.
    m_mul, Mul
);
binop_matcher!(
    /// `udiv`. Mirrors `m_UDiv`.
    m_udiv, UDiv
);
binop_matcher!(
    /// `sdiv`. Mirrors `m_SDiv`.
    m_sdiv, SDiv
);
binop_matcher!(
    /// `urem`. Mirrors `m_URem`.
    m_urem, URem
);
binop_matcher!(
    /// `srem`. Mirrors `m_SRem`.
    m_srem, SRem
);
binop_matcher!(
    /// `shl`. Mirrors `m_Shl`.
    m_shl, Shl
);
binop_matcher!(
    /// `lshr`. Mirrors `m_LShr`.
    m_lshr, LShr
);
binop_matcher!(
    /// `ashr`. Mirrors `m_AShr`.
    m_ashr, AShr
);
binop_matcher!(
    /// `and`. Mirrors `m_And`.
    m_and, And
);
binop_matcher!(
    /// `or`. Mirrors `m_Or`.
    m_or, Or
);
binop_matcher!(
    /// `xor`. Mirrors `m_Xor`.
    m_xor, Xor
);
binop_matcher!(
    /// `fadd`. Mirrors `m_FAdd`.
    m_fadd, FAdd
);
binop_matcher!(
    /// `fsub`. Mirrors `m_FSub`.
    m_fsub, FSub
);
binop_matcher!(
    /// `fmul`. Mirrors `m_FMul`.
    m_fmul, FMul
);
binop_matcher!(
    /// `fdiv`. Mirrors `m_FDiv`.
    m_fdiv, FDiv
);
binop_matcher!(
    /// `frem`. Mirrors `m_FRem`.
    m_frem, FRem
);

commutative_binop_matcher!(
    /// Commutative `add`. Mirrors `m_c_Add`.
    m_c_add, Add
);
commutative_binop_matcher!(
    /// Commutative `mul`. Mirrors `m_c_Mul`.
    m_c_mul, Mul
);
commutative_binop_matcher!(
    /// Commutative `and`. Mirrors `m_c_And`.
    m_c_and, And
);
commutative_binop_matcher!(
    /// Commutative `or`. Mirrors `m_c_Or`.
    m_c_or, Or
);
commutative_binop_matcher!(
    /// Commutative `xor`. Mirrors `m_c_Xor`.
    m_c_xor, Xor
);
commutative_binop_matcher!(
    /// Commutative `fadd`. Mirrors `m_c_FAdd`.
    m_c_fadd, FAdd
);
commutative_binop_matcher!(
    /// Commutative `fmul`. Mirrors `m_c_FMul`.
    m_c_fmul, FMul
);

// --------------------------------------------------------------------------
// Sugar: m_not / m_neg
// --------------------------------------------------------------------------

/// Matches `xor %x, -1` (bitwise not) and forwards `inner`'s bindings.
/// Mirrors `m_Not`. Commutative on the `xor`.
pub fn m_not<M>(inner: M) -> MBinOp<MConstIntPred, M> {
    m_c_xor(m_all_ones(), inner)
}

/// Matches `sub 0, %x` (negation) and forwards `inner`'s bindings.
/// Mirrors `m_Neg`.
pub fn m_neg<M>(inner: M) -> MBinOp<MConstIntPred, M> {
    m_sub(m_zero(), inner)
}

// --------------------------------------------------------------------------
// m_combine_or / m_combine_and — pattern-level `||` / `&&`
// --------------------------------------------------------------------------

/// Matches if either sub-pattern matches (left first). Both must bind the
/// same tuple type. Mirrors `m_CombineOr`.
pub fn m_combine_or<A, C>(a: A, b: C) -> MCombineOr<A, C> {
    MCombineOr(a, b)
}

/// Matcher returned by [`m_combine_or`].
pub struct MCombineOr<A, C>(A, C);

impl<'ctx, B, A, C> Matcher<'ctx, B> for MCombineOr<A, C>
where
    B: ModuleBrand + 'ctx,
    A: Matcher<'ctx, B>,
    C: Matcher<'ctx, B, Bindings = A::Bindings>,
{
    type Bindings = A::Bindings;
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        self.0.try_match(value).or_else(|| self.1.try_match(value))
    }
}

/// Matches if both sub-patterns match; concatenates their bindings.
/// Mirrors `m_CombineAnd`.
pub fn m_combine_and<A, C>(a: A, b: C) -> MCombineAnd<A, C> {
    MCombineAnd(a, b)
}

/// Matcher returned by [`m_combine_and`].
pub struct MCombineAnd<A, C>(A, C);

impl<'ctx, B, A, C> Matcher<'ctx, B> for MCombineAnd<A, C>
where
    B: ModuleBrand + 'ctx,
    A: Matcher<'ctx, B>,
    C: Matcher<'ctx, B>,
    A::Bindings: Combine<C::Bindings>,
{
    type Bindings = <A::Bindings as Combine<C::Bindings>>::Out;
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        let a = self.0.try_match(value)?;
        let b = self.1.try_match(value)?;
        Some(a.combine(b))
    }
}

// --------------------------------------------------------------------------
// Memory matchers: m_load / m_gep (base)
// --------------------------------------------------------------------------

/// Matches a `load` and matches its pointer operand against `ptr`.
/// Mirrors `m_Load`.
pub fn m_load<P>(ptr: P) -> MLoad<P> {
    MLoad(ptr)
}

/// Matcher returned by [`m_load`].
pub struct MLoad<P>(P);

impl<'ctx, B, P> Matcher<'ctx, B> for MLoad<P>
where
    B: ModuleBrand + 'ctx,
    P: Matcher<'ctx, B>,
{
    type Bindings = P::Bindings;
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        let view = InstructionView::try_from(value).ok()?;
        match view.kind()? {
            InstructionKind::Load(load) => self.0.try_match(load.pointer().as_value()),
            _ => None,
        }
    }
}

/// Matches any `phi` and binds its result-typed [`PhiKind`] discriminator.
/// The binding narrows further by matching on the variant
/// (`PhiKind::Int`/`Fp`/`Ptr`/`Other`). No upstream analog binds the node
/// this precisely; `m_Phi` in `PatternMatch.h` (LLVM 22) binds operands only.
pub fn m_phi() -> MPhi {
    MPhi
}

/// Matcher returned by [`m_phi`].
pub struct MPhi;

impl<'ctx, B> Matcher<'ctx, B> for MPhi
where
    B: ModuleBrand + 'ctx,
{
    type Bindings = (PhiKind<'ctx, B>,);
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        let view = InstructionView::try_from(value).ok()?;
        match view.kind()? {
            InstructionKind::Phi(kind) => Some((kind,)),
            _ => None,
        }
    }
}

/// Matches a `getelementptr` and matches its base pointer against `ptr`.
/// Mirrors a base-only `m_GEP`.
pub fn m_gep<P>(ptr: P) -> MGep<P> {
    MGep(ptr)
}

/// Matcher returned by [`m_gep`].
pub struct MGep<P>(P);

impl<'ctx, B, P> Matcher<'ctx, B> for MGep<P>
where
    B: ModuleBrand + 'ctx,
    P: Matcher<'ctx, B>,
{
    type Bindings = P::Bindings;
    #[inline]
    fn try_match(&self, value: Value<'ctx, B>) -> Option<Self::Bindings> {
        let view = InstructionView::try_from(value).ok()?;
        match view.kind()? {
            InstructionKind::Gep(gep) => self.0.try_match(gep.pointer().as_value()),
            _ => None,
        }
    }
}
