//! Shared payload structs for instructions. Mirrors
//! `llvm/include/llvm/IR/InstrTypes.h`.
//!
//! The upstream `InstrTypes.h` is a grab-bag of base classes
//! (`UnaryInstruction`, `BinaryOperator`, `CastInst`, `CallBase`, ...)
//! that lift shared layout out of [`Instructions.h`](crate::instructions).
//! Phase E of the foundation only ships the binary-operator shape and
//! the return-instruction shape; `CastInst`, `CallBase`, etc. are
//! follow-up.
//!
//! The structs here are **storage payloads**, not public handles. They
//! are stored inside the per-instruction storage record
//! in the value arena. Public per-opcode handles live in
//! [`crate::instructions`].

use core::cell::Cell;

use crate::value::ValueId;

/// Storage payload for the binary-operator opcodes (`add`, `sub`,
/// `mul`, ...). Mirrors the operand/flag layout of `BinaryOperator`
/// (`InstrTypes.h`).
///
/// Operand slots are wrapped in [`Cell<ValueId>`] so RAUW
/// (`Value::replaceAllUsesWith`) can rewrite the wiring without
/// requiring `&mut Module` borrows. Reads must use `.get()`; the
/// flags remain plain (RAUW does not touch them).
#[derive(Debug)]
pub(crate) struct BinaryOpData {
    pub(crate) lhs: Cell<ValueId>,
    pub(crate) rhs: Cell<ValueId>,
    /// `nuw` (no-unsigned-wrap) flag. Mirrors
    /// `OverflowingBinaryOperator::NoUnsignedWrap`. Applies to
    /// `add` / `sub` / `mul` / `shl`.
    pub(crate) no_unsigned_wrap: bool,
    /// `nsw` (no-signed-wrap) flag. Mirrors
    /// `OverflowingBinaryOperator::NoSignedWrap`. Applies to `add` /
    /// `sub` / `mul` / `shl`.
    pub(crate) no_signed_wrap: bool,
    /// `exact` flag. Mirrors `PossiblyExactOperator::IsExact`.
    /// Applies to `udiv` / `sdiv` / `lshr` / `ashr`.
    pub(crate) is_exact: bool,
    /// Per-instruction fast-math flags. Applies only to FP binops
    /// (`fadd` / `fsub` / `fmul` / `fdiv` / `frem`); empty for integer
    /// opcodes. Mirrors the `FastMathFlags` slot on `FPMathOperator`
    /// (`Operator.h`).
    pub(crate) fmf: crate::fmf::FastMathFlags,
}

impl BinaryOpData {
    pub(crate) fn new(lhs: ValueId, rhs: ValueId) -> Self {
        Self {
            lhs: Cell::new(lhs),
            rhs: Cell::new(rhs),
            no_unsigned_wrap: false,
            no_signed_wrap: false,
            is_exact: false,
            fmf: crate::fmf::FastMathFlags::empty(),
        }
    }
}

// `BinaryOpData` carries `Cell` fields and is therefore neither
// `Clone` nor structurally-comparable by default. Provide manual impls
// that compare the *current* values of the cells, which is the only
// thing storage-equality could mean here.
impl Clone for BinaryOpData {
    fn clone(&self) -> Self {
        Self {
            lhs: Cell::new(self.lhs.get()),
            rhs: Cell::new(self.rhs.get()),
            no_unsigned_wrap: self.no_unsigned_wrap,
            no_signed_wrap: self.no_signed_wrap,
            is_exact: self.is_exact,
            fmf: self.fmf,
        }
    }
}
impl PartialEq for BinaryOpData {
    fn eq(&self, other: &Self) -> bool {
        self.lhs.get() == other.lhs.get()
            && self.rhs.get() == other.rhs.get()
            && self.no_unsigned_wrap == other.no_unsigned_wrap
            && self.no_signed_wrap == other.no_signed_wrap
            && self.is_exact == other.is_exact
            && self.fmf == other.fmf
    }
}
impl Eq for BinaryOpData {}
impl core::hash::Hash for BinaryOpData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.lhs.get().hash(h);
        self.rhs.get().hash(h);
        self.no_unsigned_wrap.hash(h);
        self.no_signed_wrap.hash(h);
        self.is_exact.hash(h);
        self.fmf.bits().hash(h);
    }
}

/// Storage payload for the `ret` terminator. `value: None` is `ret
/// void`. Mirrors `ReturnInst`'s operand layout (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct ReturnOpData {
    pub(crate) value: Cell<Option<ValueId>>,
}

impl ReturnOpData {
    pub(crate) fn new(value: Option<ValueId>) -> Self {
        Self {
            value: Cell::new(value),
        }
    }
}

impl Clone for ReturnOpData {
    fn clone(&self) -> Self {
        Self {
            value: Cell::new(self.value.get()),
        }
    }
}
impl PartialEq for ReturnOpData {
    fn eq(&self, other: &Self) -> bool {
        self.value.get() == other.value.get()
    }
}
impl Eq for ReturnOpData {}
impl core::hash::Hash for ReturnOpData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.value.get().hash(h);
    }
}

/// Closed enum mirroring the cast opcodes in
/// `Instruction::CastOps` (`Instructions.h`). The set is fixed by the
/// IR spec; new entries are not expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CastOpcode {
    /// `trunc` — narrow an integer to a smaller width.
    Trunc,
    /// `zext` — widen an integer with zero-extension.
    ZExt,
    /// `sext` — widen an integer with sign-extension.
    SExt,
    /// `fptrunc` — narrow a float kind.
    FpTrunc,
    /// `fpext` — widen a float kind.
    FpExt,
    /// `fptoui` — float to unsigned integer.
    FpToUI,
    /// `fptosi` — float to signed integer.
    FpToSI,
    /// `uitofp` — unsigned integer to float.
    UIToFp,
    /// `sitofp` — signed integer to float.
    SIToFp,
    /// `ptrtoint` — pointer to integer.
    PtrToInt,
    /// `inttoptr` — integer to pointer.
    IntToPtr,
    /// `bitcast` — same-size bit reinterpretation.
    BitCast,
    /// `addrspacecast` — address-space change on a pointer.
    AddrSpaceCast,
}

impl CastOpcode {
    /// `.ll` keyword for this cast opcode.
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Trunc => "trunc",
            Self::ZExt => "zext",
            Self::SExt => "sext",
            Self::FpTrunc => "fptrunc",
            Self::FpExt => "fpext",
            Self::FpToUI => "fptoui",
            Self::FpToSI => "fptosi",
            Self::UIToFp => "uitofp",
            Self::SIToFp => "sitofp",
            Self::PtrToInt => "ptrtoint",
            Self::IntToPtr => "inttoptr",
            Self::BitCast => "bitcast",
            Self::AddrSpaceCast => "addrspacecast",
        }
    }
}

/// Storage payload for a cast instruction. The destination type is
/// carried in the host [`crate::value::ValueData::ty`] field; this
/// payload only needs the source operand and which opcode to emit.
/// Mirrors the operand layout of `CastInst` (`InstrTypes.h`).
#[derive(Debug)]
pub(crate) struct CastOpData {
    pub(crate) kind: CastOpcode,
    pub(crate) src: Cell<ValueId>,
}

impl CastOpData {
    pub(crate) fn new(kind: CastOpcode, src: ValueId) -> Self {
        Self {
            kind,
            src: Cell::new(src),
        }
    }
}
impl Clone for CastOpData {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            src: Cell::new(self.src.get()),
        }
    }
}
impl PartialEq for CastOpData {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.src.get() == other.src.get()
    }
}
impl Eq for CastOpData {}
impl core::hash::Hash for CastOpData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.kind.hash(h);
        self.src.get().hash(h);
    }
}

// --------------------------------------------------------------------------
// Unary ops: fneg / freeze / va_arg
// --------------------------------------------------------------------------

/// Storage payload for `fneg`. Mirrors `UnaryOperator` restricted to the
/// `Instruction::FNeg` opcode in `InstrTypes.h`. Carries fast-math flags
/// because every `FPMathOperator` instruction subclass may set them
/// (`Operator.h`, `FPMathOperator`).
#[derive(Debug)]
pub(crate) struct FNegInstData {
    pub(crate) src: Cell<ValueId>,
    pub(crate) fmf: crate::fmf::FastMathFlags,
}

impl FNegInstData {
    pub(crate) fn new(src: ValueId, fmf: crate::fmf::FastMathFlags) -> Self {
        Self {
            src: Cell::new(src),
            fmf,
        }
    }
}
impl Clone for FNegInstData {
    fn clone(&self) -> Self {
        Self {
            src: Cell::new(self.src.get()),
            fmf: self.fmf,
        }
    }
}
impl PartialEq for FNegInstData {
    fn eq(&self, other: &Self) -> bool {
        self.src.get() == other.src.get() && self.fmf == other.fmf
    }
}
impl Eq for FNegInstData {}
impl core::hash::Hash for FNegInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.src.get().hash(h);
        self.fmf.bits().hash(h);
    }
}

/// Storage payload for `freeze`. Mirrors `FreezeInst`
/// (`Instructions.h`). The result type matches the operand type and
/// is carried in the host [`crate::value::ValueData::ty`].
#[derive(Debug)]
pub(crate) struct FreezeInstData {
    pub(crate) src: Cell<ValueId>,
}

impl FreezeInstData {
    pub(crate) fn new(src: ValueId) -> Self {
        Self {
            src: Cell::new(src),
        }
    }
}
impl Clone for FreezeInstData {
    fn clone(&self) -> Self {
        Self {
            src: Cell::new(self.src.get()),
        }
    }
}
impl PartialEq for FreezeInstData {
    fn eq(&self, other: &Self) -> bool {
        self.src.get() == other.src.get()
    }
}
impl Eq for FreezeInstData {}
impl core::hash::Hash for FreezeInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.src.get().hash(h);
    }
}

/// Storage payload for `va_arg`. Mirrors `VAArgInst`
/// (`Instructions.h`). The destination type is carried in the host
/// `ValueData::ty`; the payload stores only the `va_list` pointer.
#[derive(Debug)]
pub(crate) struct VAArgInstData {
    pub(crate) src: Cell<ValueId>,
}

impl VAArgInstData {
    pub(crate) fn new(src: ValueId) -> Self {
        Self {
            src: Cell::new(src),
        }
    }
}
impl Clone for VAArgInstData {
    fn clone(&self) -> Self {
        Self {
            src: Cell::new(self.src.get()),
        }
    }
}
impl PartialEq for VAArgInstData {
    fn eq(&self, other: &Self) -> bool {
        self.src.get() == other.src.get()
    }
}
impl Eq for VAArgInstData {}
impl core::hash::Hash for VAArgInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.src.get().hash(h);
    }
}

// --------------------------------------------------------------------------
// Comparison instructions (icmp)
// --------------------------------------------------------------------------

/// Storage payload for `icmp`. Mirrors the operand layout of
/// `CmpInst` (`InstrTypes.h`) restricted to integer compares.
/// Float comparisons (`fcmp`) will land alongside the float-builder
/// session and either generalise this struct (with a separate
/// `FloatPredicate` field on a dedicated payload) or live in their
/// own `FCmpInstData`. Today the IR builder only emits integer
/// compares, so the storage carries an [`IntPredicate`] directly:
/// no enum envelope means no `match` arms going stale.
///
/// The host [`crate::value::ValueData::ty`] is `i1` (or `<N x i1>`
/// once vector compares ship).
#[derive(Debug)]
pub(crate) struct CmpInstData {
    pub(crate) predicate: crate::cmp_predicate::IntPredicate,
    pub(crate) lhs: Cell<ValueId>,
    pub(crate) rhs: Cell<ValueId>,
}

impl CmpInstData {
    pub(crate) fn new(
        predicate: crate::cmp_predicate::IntPredicate,
        lhs: ValueId,
        rhs: ValueId,
    ) -> Self {
        Self {
            predicate,
            lhs: Cell::new(lhs),
            rhs: Cell::new(rhs),
        }
    }
}
impl Clone for CmpInstData {
    fn clone(&self) -> Self {
        Self {
            predicate: self.predicate,
            lhs: Cell::new(self.lhs.get()),
            rhs: Cell::new(self.rhs.get()),
        }
    }
}
impl PartialEq for CmpInstData {
    fn eq(&self, other: &Self) -> bool {
        self.predicate == other.predicate
            && self.lhs.get() == other.lhs.get()
            && self.rhs.get() == other.rhs.get()
    }
}
impl Eq for CmpInstData {}
impl core::hash::Hash for CmpInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.predicate.hash(h);
        self.lhs.get().hash(h);
        self.rhs.get().hash(h);
    }
}

/// Storage payload for `fcmp`. Mirrors the float-side of `CmpInst`
/// (`InstrTypes.h`). Distinct struct from [`CmpInstData`] so the
/// predicate field's type pins `FloatPredicate` at the storage
/// layer.
#[derive(Debug)]
pub(crate) struct FCmpInstData {
    pub(crate) predicate: crate::cmp_predicate::FloatPredicate,
    pub(crate) lhs: Cell<ValueId>,
    pub(crate) rhs: Cell<ValueId>,
    /// Per-instruction fast-math flags. `fcmp` is an `FPMathOperator`
    /// upstream, so the same FMF slot applies. Empty by default.
    pub(crate) fmf: crate::fmf::FastMathFlags,
}

impl FCmpInstData {
    pub(crate) fn new(
        predicate: crate::cmp_predicate::FloatPredicate,
        lhs: ValueId,
        rhs: ValueId,
    ) -> Self {
        Self {
            predicate,
            lhs: Cell::new(lhs),
            rhs: Cell::new(rhs),
            fmf: crate::fmf::FastMathFlags::empty(),
        }
    }
}
impl Clone for FCmpInstData {
    fn clone(&self) -> Self {
        Self {
            predicate: self.predicate,
            lhs: Cell::new(self.lhs.get()),
            rhs: Cell::new(self.rhs.get()),
            fmf: self.fmf,
        }
    }
}
impl PartialEq for FCmpInstData {
    fn eq(&self, other: &Self) -> bool {
        self.predicate == other.predicate
            && self.lhs.get() == other.lhs.get()
            && self.rhs.get() == other.rhs.get()
            && self.fmf == other.fmf
    }
}
impl Eq for FCmpInstData {}
impl core::hash::Hash for FCmpInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.predicate.hash(h);
        self.lhs.get().hash(h);
        self.rhs.get().hash(h);
        self.fmf.bits().hash(h);
    }
}

// --------------------------------------------------------------------------
// Branch / Unreachable terminators
// --------------------------------------------------------------------------

/// Storage payload for `br`. Mirrors `BranchInst` (`Instructions.h`)
/// with the C++ swizzled operand order normalised to logical
/// (cond, then, else) for the conditional case.
#[derive(Debug, Clone)]
pub(crate) struct BranchInstData {
    pub(crate) kind: BranchKind,
}

impl PartialEq for BranchInstData {
    fn eq(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (BranchKind::Unconditional(a), BranchKind::Unconditional(b)) => a == b,
            (
                BranchKind::Conditional {
                    cond: c1,
                    then_bb: t1,
                    else_bb: e1,
                },
                BranchKind::Conditional {
                    cond: c2,
                    then_bb: t2,
                    else_bb: e2,
                },
            ) => c1.get() == c2.get() && t1 == t2 && e1 == e2,
            _ => false,
        }
    }
}
impl Eq for BranchInstData {}
impl core::hash::Hash for BranchInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        match &self.kind {
            BranchKind::Unconditional(t) => {
                0u8.hash(h);
                t.hash(h);
            }
            BranchKind::Conditional {
                cond,
                then_bb,
                else_bb,
            } => {
                1u8.hash(h);
                cond.get().hash(h);
                then_bb.hash(h);
                else_bb.hash(h);
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum BranchKind {
    /// `br label %target`. Carries the target block's value-id.
    Unconditional(ValueId),
    /// `br i1 %cond, label %then, label %else`. The block ids are
    /// stored alongside the SSA `cond` operand; only `cond` is an
    /// SSA operand, so `User::operands()` returns just `cond` for
    /// this variant.
    Conditional {
        cond: Cell<ValueId>,
        then_bb: ValueId,
        else_bb: ValueId,
    },
}

impl Clone for BranchKind {
    fn clone(&self) -> Self {
        match self {
            BranchKind::Unconditional(t) => BranchKind::Unconditional(*t),
            BranchKind::Conditional {
                cond,
                then_bb,
                else_bb,
            } => BranchKind::Conditional {
                cond: Cell::new(cond.get()),
                then_bb: *then_bb,
                else_bb: *else_bb,
            },
        }
    }
}

/// Storage payload for `unreachable`. Mirrors `UnreachableInst`
/// (`Instructions.h`) -- no operands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) struct UnreachableInstData;

// --------------------------------------------------------------------------
// PHI
// --------------------------------------------------------------------------

/// Storage payload for `phi`. Mirrors `PHINode` (`Instructions.h`).
///
/// `incoming` is the only mutable instruction payload in the crate
/// because LLVM's `PHINode::addIncoming` lets callers extend the
/// list after construction (the factorial example exercises this:
/// the loop-edge incoming value is defined later in the same
/// block). The `RefCell` is private; mutation flows through
/// [`crate::instructions::PhiInst::add_incoming`].
#[derive(Debug)]
pub(crate) struct PhiData {
    /// First slot in each pair is the SSA value (operand); second is
    /// the predecessor block. RAUW rewrites the value slot only; block
    /// edges are CFG-level data, not SSA operands.
    pub(crate) incoming: core::cell::RefCell<Vec<(Cell<ValueId>, ValueId)>>,
}

impl PhiData {
    pub(crate) fn new() -> Self {
        Self {
            incoming: core::cell::RefCell::new(Vec::new()),
        }
    }
}

// `PhiData` carries a `RefCell` and is therefore neither `Clone` nor
// `PartialEq`/`Eq`/`Hash` by default. The instruction kind enum has
// to derive those traits via a manual impl that compares only the
// invariant identity bits -- instruction storage is value-id keyed,
// so structural equality on the payload alone is unused outside the
// per-instruction fast paths.
impl PartialEq for PhiData {
    fn eq(&self, other: &Self) -> bool {
        let a = self.incoming.borrow();
        let b = other.incoming.borrow();
        a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|((va, ba), (vb, bbid))| va.get() == vb.get() && ba == bbid)
    }
}
impl Eq for PhiData {}
impl core::hash::Hash for PhiData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        for (v, b) in self.incoming.borrow().iter() {
            v.get().hash(h);
            b.hash(h);
        }
    }
}
impl Clone for PhiData {
    fn clone(&self) -> Self {
        Self {
            incoming: core::cell::RefCell::new(
                self.incoming
                    .borrow()
                    .iter()
                    .map(|(v, b)| (Cell::new(v.get()), *b))
                    .collect(),
            ),
        }
    }
}

// --------------------------------------------------------------------------
// Per-opcode flag types
// --------------------------------------------------------------------------
//
// Each integer binary opcode that accepts flags has its own flag type
// exposing only the flags LLVM permits for that opcode. Mirrors the
// per-class flag bits on `OverflowingBinaryOperator` (`add`/`sub`/`mul`/
// `shl`) and `PossiblyExactOperator` (`udiv`/`sdiv`/`lshr`/`ashr`) in
// `Operator.h`. Replaces a single shared `BinopFlags` + runtime
// `validate` call with type-state: `AddFlags` has no `.exact()` method,
// so passing `exact` to an `add` is a *compile* error rather than a
// runtime `IrError::InvalidOperation`.
//
// Opcodes without flags (`urem`/`srem`/`and`/`or`/`xor`) have no
// matching flag type - they only ship the flag-free `build_int_*`
// methods.

macro_rules! decl_overflowing_flags {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name {
            pub(crate) nuw: bool,
            pub(crate) nsw: bool,
        }

        impl $name {
            #[inline]
            pub const fn new() -> Self {
                Self { nuw: false, nsw: false }
            }

            /// Set the `nuw` (no-unsigned-wrap) flag.
            #[inline]
            #[must_use]
            pub const fn nuw(mut self) -> Self { self.nuw = true; self }

            /// Set the `nsw` (no-signed-wrap) flag.
            #[inline]
            #[must_use]
            pub const fn nsw(mut self) -> Self { self.nsw = true; self }
        }
    };
}

macro_rules! decl_exact_flags {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name {
            pub(crate) exact: bool,
        }

        impl $name {
            #[inline]
            pub const fn new() -> Self { Self { exact: false } }

            /// Set the `exact` flag.
            #[inline]
            #[must_use]
            pub const fn exact(mut self) -> Self { self.exact = true; self }
        }
    };
}

decl_overflowing_flags!(
    /// Flags for `add`. Mirrors `OverflowingBinaryOperator` in
    /// `Operator.h`.
    AddFlags
);
decl_overflowing_flags!(
    /// Flags for `sub`.
    SubFlags
);
decl_overflowing_flags!(
    /// Flags for `mul`.
    MulFlags
);
decl_overflowing_flags!(
    /// Flags for `shl`.
    ShlFlags
);

decl_exact_flags!(
    /// Flags for `udiv`. Mirrors `PossiblyExactOperator`.
    UDivFlags
);
decl_exact_flags!(
    /// Flags for `sdiv`.
    SDivFlags
);
decl_exact_flags!(
    /// Flags for `lshr`.
    LShrFlags
);
decl_exact_flags!(
    /// Flags for `ashr`.
    AShrFlags
);

/// Crate-internal: write a flag-set onto the underlying
/// [`BinaryOpData`] storage payload. Each per-opcode flag struct
/// implements this; the IR builder uses it to lift the flags into
/// the storage record without per-opcode duplication.
pub(crate) trait WriteBinopFlags {
    fn apply(self, payload: &mut BinaryOpData);
}

macro_rules! impl_overflowing_flags_writer {
    ($name:ident) => {
        impl WriteBinopFlags for $name {
            #[inline]
            fn apply(self, payload: &mut BinaryOpData) {
                payload.no_unsigned_wrap = self.nuw;
                payload.no_signed_wrap = self.nsw;
            }
        }
    };
}
macro_rules! impl_exact_flags_writer {
    ($name:ident) => {
        impl WriteBinopFlags for $name {
            #[inline]
            fn apply(self, payload: &mut BinaryOpData) {
                payload.is_exact = self.exact;
            }
        }
    };
}
impl_overflowing_flags_writer!(AddFlags);
impl_overflowing_flags_writer!(SubFlags);
impl_overflowing_flags_writer!(MulFlags);
impl_overflowing_flags_writer!(ShlFlags);
impl_exact_flags_writer!(UDivFlags);
impl_exact_flags_writer!(SDivFlags);
impl_exact_flags_writer!(LShrFlags);
impl_exact_flags_writer!(AShrFlags);

// --------------------------------------------------------------------------
// Memory ops: alloca / load / store
// --------------------------------------------------------------------------

/// Storage payload for `alloca`. Mirrors `AllocaInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct AllocaInstData {
    pub(crate) allocated_ty: crate::r#type::TypeId,
    pub(crate) num_elements: Cell<Option<ValueId>>,
    pub(crate) align: crate::align::MaybeAlign,
    pub(crate) addr_space: u32,
}

impl AllocaInstData {
    pub(crate) fn new(
        allocated_ty: crate::r#type::TypeId,
        num_elements: Option<ValueId>,
        align: crate::align::MaybeAlign,
        addr_space: u32,
    ) -> Self {
        Self {
            allocated_ty,
            num_elements: Cell::new(num_elements),
            align,
            addr_space,
        }
    }
}
impl Clone for AllocaInstData {
    fn clone(&self) -> Self {
        Self {
            allocated_ty: self.allocated_ty,
            num_elements: Cell::new(self.num_elements.get()),
            align: self.align,
            addr_space: self.addr_space,
        }
    }
}
impl PartialEq for AllocaInstData {
    fn eq(&self, other: &Self) -> bool {
        self.allocated_ty == other.allocated_ty
            && self.num_elements.get() == other.num_elements.get()
            && self.align == other.align
            && self.addr_space == other.addr_space
    }
}
impl Eq for AllocaInstData {}
impl core::hash::Hash for AllocaInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.allocated_ty.hash(h);
        self.num_elements.get().hash(h);
        self.align.hash(h);
        self.addr_space.hash(h);
    }
}

/// Storage payload for `load`. Mirrors `LoadInst`
/// (`Instructions.h`). Atomic ordering and sync-scope mirror the
/// `OrderingField` / `SSID` slots on the upstream class; the default
/// (`AtomicOrdering::NotAtomic`, `SyncScope::System`) reproduces the
/// non-atomic load.
#[derive(Debug)]
pub(crate) struct LoadInstData {
    pub(crate) pointee_ty: crate::r#type::TypeId,
    pub(crate) ptr: Cell<ValueId>,
    pub(crate) align: crate::align::MaybeAlign,
    pub(crate) volatile: bool,
    pub(crate) ordering: crate::atomic_ordering::AtomicOrdering,
    pub(crate) sync_scope: crate::sync_scope::SyncScope,
}

impl LoadInstData {
    pub(crate) fn new(
        pointee_ty: crate::r#type::TypeId,
        ptr: ValueId,
        align: crate::align::MaybeAlign,
        volatile: bool,
        ordering: crate::atomic_ordering::AtomicOrdering,
        sync_scope: crate::sync_scope::SyncScope,
    ) -> Self {
        Self {
            pointee_ty,
            ptr: Cell::new(ptr),
            align,
            volatile,
            ordering,
            sync_scope,
        }
    }

    /// `true` when the load carries a non-`NotAtomic` ordering. Mirrors
    /// `LoadInst::isAtomic` in `Instructions.h`.
    pub(crate) fn is_atomic(&self) -> bool {
        !matches!(
            self.ordering,
            crate::atomic_ordering::AtomicOrdering::NotAtomic,
        )
    }
}
impl Clone for LoadInstData {
    fn clone(&self) -> Self {
        Self {
            pointee_ty: self.pointee_ty,
            ptr: Cell::new(self.ptr.get()),
            align: self.align,
            volatile: self.volatile,
            ordering: self.ordering,
            sync_scope: self.sync_scope.clone(),
        }
    }
}
impl PartialEq for LoadInstData {
    fn eq(&self, other: &Self) -> bool {
        self.pointee_ty == other.pointee_ty
            && self.ptr.get() == other.ptr.get()
            && self.align == other.align
            && self.volatile == other.volatile
            && self.ordering == other.ordering
            && self.sync_scope == other.sync_scope
    }
}
impl Eq for LoadInstData {}
impl core::hash::Hash for LoadInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.pointee_ty.hash(h);
        self.ptr.get().hash(h);
        self.align.hash(h);
        self.volatile.hash(h);
        self.ordering.hash(h);
        self.sync_scope.hash(h);
    }
}

/// Storage payload for `store`. Mirrors `StoreInst`
/// (`Instructions.h`). Atomic ordering and sync-scope mirror the
/// `OrderingField` / `SSID` slots on the upstream class; the default
/// (`AtomicOrdering::NotAtomic`, `SyncScope::System`) reproduces the
/// non-atomic store.
#[derive(Debug)]
pub(crate) struct StoreInstData {
    pub(crate) value: Cell<ValueId>,
    pub(crate) ptr: Cell<ValueId>,
    pub(crate) align: crate::align::MaybeAlign,
    pub(crate) volatile: bool,
    pub(crate) ordering: crate::atomic_ordering::AtomicOrdering,
    pub(crate) sync_scope: crate::sync_scope::SyncScope,
}

impl StoreInstData {
    pub(crate) fn new(
        value: ValueId,
        ptr: ValueId,
        align: crate::align::MaybeAlign,
        volatile: bool,
        ordering: crate::atomic_ordering::AtomicOrdering,
        sync_scope: crate::sync_scope::SyncScope,
    ) -> Self {
        Self {
            value: Cell::new(value),
            ptr: Cell::new(ptr),
            align,
            volatile,
            ordering,
            sync_scope,
        }
    }

    /// `true` when the store carries a non-`NotAtomic` ordering. Mirrors
    /// `StoreInst::isAtomic` in `Instructions.h`.
    pub(crate) fn is_atomic(&self) -> bool {
        !matches!(
            self.ordering,
            crate::atomic_ordering::AtomicOrdering::NotAtomic,
        )
    }
}
impl Clone for StoreInstData {
    fn clone(&self) -> Self {
        Self {
            value: Cell::new(self.value.get()),
            ptr: Cell::new(self.ptr.get()),
            align: self.align,
            volatile: self.volatile,
            ordering: self.ordering,
            sync_scope: self.sync_scope.clone(),
        }
    }
}
impl PartialEq for StoreInstData {
    fn eq(&self, other: &Self) -> bool {
        self.value.get() == other.value.get()
            && self.ptr.get() == other.ptr.get()
            && self.align == other.align
            && self.volatile == other.volatile
            && self.ordering == other.ordering
            && self.sync_scope == other.sync_scope
    }
}
impl Eq for StoreInstData {}
impl core::hash::Hash for StoreInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.value.get().hash(h);
        self.ptr.get().hash(h);
        self.align.hash(h);
        self.volatile.hash(h);
        self.ordering.hash(h);
        self.sync_scope.hash(h);
    }
}

// --------------------------------------------------------------------------
// GEP
// --------------------------------------------------------------------------

/// Storage payload for `getelementptr`. Mirrors `GetElementPtrInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct GepInstData {
    pub(crate) source_ty: crate::r#type::TypeId,
    pub(crate) ptr: Cell<ValueId>,
    pub(crate) indices: Box<[Cell<ValueId>]>,
    pub(crate) flags: crate::gep_no_wrap_flags::GepNoWrapFlags,
}

impl GepInstData {
    pub(crate) fn new(
        source_ty: crate::r#type::TypeId,
        ptr: ValueId,
        indices: impl IntoIterator<Item = ValueId>,
        flags: crate::gep_no_wrap_flags::GepNoWrapFlags,
    ) -> Self {
        Self {
            source_ty,
            ptr: Cell::new(ptr),
            indices: indices.into_iter().map(Cell::new).collect(),
            flags,
        }
    }
}
impl Clone for GepInstData {
    fn clone(&self) -> Self {
        Self {
            source_ty: self.source_ty,
            ptr: Cell::new(self.ptr.get()),
            indices: self.indices.iter().map(|c| Cell::new(c.get())).collect(),
            flags: self.flags,
        }
    }
}
impl PartialEq for GepInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.source_ty != other.source_ty
            || self.flags != other.flags
            || self.ptr.get() != other.ptr.get()
            || self.indices.len() != other.indices.len()
        {
            return false;
        }
        self.indices
            .iter()
            .zip(other.indices.iter())
            .all(|(a, b)| a.get() == b.get())
    }
}
impl Eq for GepInstData {}
impl core::hash::Hash for GepInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.source_ty.hash(h);
        self.ptr.get().hash(h);
        for idx in self.indices.iter() {
            idx.get().hash(h);
        }
        self.flags.hash(h);
    }
}

// --------------------------------------------------------------------------
// Call
// --------------------------------------------------------------------------

/// Tail-call modifier on a call site. Mirrors
/// `CallInst::TailCallKind` (`Instructions.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TailCallKind {
    #[default]
    None,
    Tail,
    MustTail,
    NoTail,
}

impl TailCallKind {
    /// `.ll` keyword for this kind, or `None` for the default
    /// (no marker).
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Tail => Some("tail"),
            Self::MustTail => Some("musttail"),
            Self::NoTail => Some("notail"),
        }
    }
}

/// Storage payload for `call`. Mirrors `CallInst`
/// (`Instructions.h`). Per-arg / per-fn attributes deferred.
#[derive(Debug)]
pub(crate) struct CallInstData {
    pub(crate) callee: Cell<ValueId>,
    pub(crate) fn_ty: crate::r#type::TypeId,
    pub(crate) args: Box<[Cell<ValueId>]>,
    pub(crate) calling_conv: crate::CallingConv,
    pub(crate) tail_kind: TailCallKind,
}

impl CallInstData {
    pub(crate) fn new(
        callee: ValueId,
        fn_ty: crate::r#type::TypeId,
        args: impl IntoIterator<Item = ValueId>,
        calling_conv: crate::CallingConv,
        tail_kind: TailCallKind,
    ) -> Self {
        Self {
            callee: Cell::new(callee),
            fn_ty,
            args: args.into_iter().map(Cell::new).collect(),
            calling_conv,
            tail_kind,
        }
    }
}
impl Clone for CallInstData {
    fn clone(&self) -> Self {
        Self {
            callee: Cell::new(self.callee.get()),
            fn_ty: self.fn_ty,
            args: self.args.iter().map(|c| Cell::new(c.get())).collect(),
            calling_conv: self.calling_conv,
            tail_kind: self.tail_kind,
        }
    }
}
impl PartialEq for CallInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.callee.get() != other.callee.get()
            || self.fn_ty != other.fn_ty
            || self.calling_conv != other.calling_conv
            || self.tail_kind != other.tail_kind
            || self.args.len() != other.args.len()
        {
            return false;
        }
        self.args
            .iter()
            .zip(other.args.iter())
            .all(|(a, b)| a.get() == b.get())
    }
}
impl Eq for CallInstData {}
impl core::hash::Hash for CallInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.callee.get().hash(h);
        self.fn_ty.hash(h);
        for arg in self.args.iter() {
            arg.get().hash(h);
        }
        self.calling_conv.hash(h);
        self.tail_kind.hash(h);
    }
}

// --------------------------------------------------------------------------
// Select
// --------------------------------------------------------------------------

/// Storage payload for `select`. Mirrors `SelectInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct SelectInstData {
    pub(crate) cond: Cell<ValueId>,
    pub(crate) true_val: Cell<ValueId>,
    pub(crate) false_val: Cell<ValueId>,
}

impl SelectInstData {
    pub(crate) fn new(cond: ValueId, true_val: ValueId, false_val: ValueId) -> Self {
        Self {
            cond: Cell::new(cond),
            true_val: Cell::new(true_val),
            false_val: Cell::new(false_val),
        }
    }
}
impl Clone for SelectInstData {
    fn clone(&self) -> Self {
        Self {
            cond: Cell::new(self.cond.get()),
            true_val: Cell::new(self.true_val.get()),
            false_val: Cell::new(self.false_val.get()),
        }
    }
}
impl PartialEq for SelectInstData {
    fn eq(&self, other: &Self) -> bool {
        self.cond.get() == other.cond.get()
            && self.true_val.get() == other.true_val.get()
            && self.false_val.get() == other.false_val.get()
    }
}
impl Eq for SelectInstData {}
impl core::hash::Hash for SelectInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.cond.get().hash(h);
        self.true_val.get().hash(h);
        self.false_val.get().hash(h);
    }
}

// --------------------------------------------------------------------------
// Aggregate ops: extractvalue / insertvalue
// --------------------------------------------------------------------------

/// Storage payload for `extractvalue`. Mirrors `ExtractValueInst`
/// (`Instructions.h`). Indices are `u32` because LangRef restricts them
/// to `i32`-fitting compile-time constants (see
/// `Instructions.h::ExtractValueInst::Indices` -- `SmallVector<unsigned,4>`).
#[derive(Debug)]
pub(crate) struct ExtractValueInstData {
    pub(crate) aggregate: Cell<ValueId>,
    pub(crate) indices: Box<[u32]>,
}

impl ExtractValueInstData {
    pub(crate) fn new(aggregate: ValueId, indices: impl IntoIterator<Item = u32>) -> Self {
        Self {
            aggregate: Cell::new(aggregate),
            indices: indices.into_iter().collect(),
        }
    }
}
impl Clone for ExtractValueInstData {
    fn clone(&self) -> Self {
        Self {
            aggregate: Cell::new(self.aggregate.get()),
            indices: self.indices.clone(),
        }
    }
}
impl PartialEq for ExtractValueInstData {
    fn eq(&self, other: &Self) -> bool {
        self.aggregate.get() == other.aggregate.get() && self.indices == other.indices
    }
}
impl Eq for ExtractValueInstData {}
impl core::hash::Hash for ExtractValueInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.aggregate.get().hash(h);
        self.indices.hash(h);
    }
}

/// Storage payload for `insertvalue`. Mirrors `InsertValueInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct InsertValueInstData {
    pub(crate) aggregate: Cell<ValueId>,
    pub(crate) value: Cell<ValueId>,
    pub(crate) indices: Box<[u32]>,
}

impl InsertValueInstData {
    pub(crate) fn new(
        aggregate: ValueId,
        value: ValueId,
        indices: impl IntoIterator<Item = u32>,
    ) -> Self {
        Self {
            aggregate: Cell::new(aggregate),
            value: Cell::new(value),
            indices: indices.into_iter().collect(),
        }
    }
}
impl Clone for InsertValueInstData {
    fn clone(&self) -> Self {
        Self {
            aggregate: Cell::new(self.aggregate.get()),
            value: Cell::new(self.value.get()),
            indices: self.indices.clone(),
        }
    }
}
impl PartialEq for InsertValueInstData {
    fn eq(&self, other: &Self) -> bool {
        self.aggregate.get() == other.aggregate.get()
            && self.value.get() == other.value.get()
            && self.indices == other.indices
    }
}
impl Eq for InsertValueInstData {}
impl core::hash::Hash for InsertValueInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.aggregate.get().hash(h);
        self.value.get().hash(h);
        self.indices.hash(h);
    }
}

// --------------------------------------------------------------------------
// Vector ops: extractelement / insertelement / shufflevector
// --------------------------------------------------------------------------

/// Storage payload for `extractelement`. Mirrors `ExtractElementInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct ExtractElementInstData {
    pub(crate) vector: Cell<ValueId>,
    pub(crate) index: Cell<ValueId>,
}

impl ExtractElementInstData {
    pub(crate) fn new(vector: ValueId, index: ValueId) -> Self {
        Self {
            vector: Cell::new(vector),
            index: Cell::new(index),
        }
    }
}
impl Clone for ExtractElementInstData {
    fn clone(&self) -> Self {
        Self {
            vector: Cell::new(self.vector.get()),
            index: Cell::new(self.index.get()),
        }
    }
}
impl PartialEq for ExtractElementInstData {
    fn eq(&self, other: &Self) -> bool {
        self.vector.get() == other.vector.get() && self.index.get() == other.index.get()
    }
}
impl Eq for ExtractElementInstData {}
impl core::hash::Hash for ExtractElementInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.vector.get().hash(h);
        self.index.get().hash(h);
    }
}

/// Storage payload for `insertelement`. Mirrors `InsertElementInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct InsertElementInstData {
    pub(crate) vector: Cell<ValueId>,
    pub(crate) value: Cell<ValueId>,
    pub(crate) index: Cell<ValueId>,
}

impl InsertElementInstData {
    pub(crate) fn new(vector: ValueId, value: ValueId, index: ValueId) -> Self {
        Self {
            vector: Cell::new(vector),
            value: Cell::new(value),
            index: Cell::new(index),
        }
    }
}
impl Clone for InsertElementInstData {
    fn clone(&self) -> Self {
        Self {
            vector: Cell::new(self.vector.get()),
            value: Cell::new(self.value.get()),
            index: Cell::new(self.index.get()),
        }
    }
}
impl PartialEq for InsertElementInstData {
    fn eq(&self, other: &Self) -> bool {
        self.vector.get() == other.vector.get()
            && self.value.get() == other.value.get()
            && self.index.get() == other.index.get()
    }
}
impl Eq for InsertElementInstData {}
impl core::hash::Hash for InsertElementInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.vector.get().hash(h);
        self.value.get().hash(h);
        self.index.get().hash(h);
    }
}

/// Sentinel mask element representing `poison` (mirrors
/// `PoisonMaskElem` / `UndefMaskElem` in `Instructions.h`).
pub const POISON_MASK_ELEM: i32 = -1;

/// Storage payload for `shufflevector`. Mirrors `ShuffleVectorInst`
/// (`Instructions.h`). The mask is stored as a list of integers per
/// upstream's `SmallVector<int, 4> ShuffleMask` representation;
/// `POISON_MASK_ELEM` (-1) marks poison entries.
#[derive(Debug)]
pub(crate) struct ShuffleVectorInstData {
    pub(crate) lhs: Cell<ValueId>,
    pub(crate) rhs: Cell<ValueId>,
    pub(crate) mask: Box<[i32]>,
}

impl ShuffleVectorInstData {
    pub(crate) fn new(lhs: ValueId, rhs: ValueId, mask: impl IntoIterator<Item = i32>) -> Self {
        Self {
            lhs: Cell::new(lhs),
            rhs: Cell::new(rhs),
            mask: mask.into_iter().collect(),
        }
    }
}
impl Clone for ShuffleVectorInstData {
    fn clone(&self) -> Self {
        Self {
            lhs: Cell::new(self.lhs.get()),
            rhs: Cell::new(self.rhs.get()),
            mask: self.mask.clone(),
        }
    }
}
impl PartialEq for ShuffleVectorInstData {
    fn eq(&self, other: &Self) -> bool {
        self.lhs.get() == other.lhs.get()
            && self.rhs.get() == other.rhs.get()
            && self.mask == other.mask
    }
}
impl Eq for ShuffleVectorInstData {}
impl core::hash::Hash for ShuffleVectorInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.lhs.get().hash(h);
        self.rhs.get().hash(h);
        self.mask.hash(h);
    }
}

// --------------------------------------------------------------------------
// Atomic ops: fence / cmpxchg / atomicrmw
// --------------------------------------------------------------------------

/// Storage payload for `fence`. Mirrors `FenceInst` (`Instructions.h`).
/// No SSA operands; ordering and sync-scope only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FenceInstData {
    pub(crate) ordering: crate::atomic_ordering::AtomicOrdering,
    pub(crate) sync_scope: crate::sync_scope::SyncScope,
}

impl FenceInstData {
    pub(crate) fn new(
        ordering: crate::atomic_ordering::AtomicOrdering,
        sync_scope: crate::sync_scope::SyncScope,
    ) -> Self {
        Self {
            ordering,
            sync_scope,
        }
    }
}

/// Storage payload for `cmpxchg`. Mirrors `AtomicCmpXchgInst`
/// (`Instructions.h`). The result type is the literal struct
/// `{ <pointee>, i1 }` (carried in the host `ValueData::ty`).
#[derive(Debug)]
pub(crate) struct AtomicCmpXchgInstData {
    pub(crate) ptr: Cell<ValueId>,
    pub(crate) cmp: Cell<ValueId>,
    pub(crate) new_val: Cell<ValueId>,
    pub(crate) align: crate::align::MaybeAlign,
    pub(crate) success_ordering: crate::atomic_ordering::AtomicOrdering,
    pub(crate) failure_ordering: crate::atomic_ordering::AtomicOrdering,
    pub(crate) sync_scope: crate::sync_scope::SyncScope,
    pub(crate) weak: bool,
    pub(crate) volatile: bool,
}

impl AtomicCmpXchgInstData {
    pub(crate) fn new(
        ptr: ValueId,
        cmp: ValueId,
        new_val: ValueId,
        config: crate::instr_types::AtomicCmpXchgConfig,
    ) -> Self {
        Self {
            ptr: Cell::new(ptr),
            cmp: Cell::new(cmp),
            new_val: Cell::new(new_val),
            align: config.align,
            success_ordering: config.success_ordering,
            failure_ordering: config.failure_ordering,
            sync_scope: config.sync_scope,
            weak: config.flags.weak,
            volatile: config.flags.volatile,
        }
    }
}
impl Clone for AtomicCmpXchgInstData {
    fn clone(&self) -> Self {
        Self {
            ptr: Cell::new(self.ptr.get()),
            cmp: Cell::new(self.cmp.get()),
            new_val: Cell::new(self.new_val.get()),
            align: self.align,
            success_ordering: self.success_ordering,
            failure_ordering: self.failure_ordering,
            sync_scope: self.sync_scope.clone(),
            weak: self.weak,
            volatile: self.volatile,
        }
    }
}
impl PartialEq for AtomicCmpXchgInstData {
    fn eq(&self, other: &Self) -> bool {
        self.ptr.get() == other.ptr.get()
            && self.cmp.get() == other.cmp.get()
            && self.new_val.get() == other.new_val.get()
            && self.align == other.align
            && self.success_ordering == other.success_ordering
            && self.failure_ordering == other.failure_ordering
            && self.sync_scope == other.sync_scope
            && self.weak == other.weak
            && self.volatile == other.volatile
    }
}
impl Eq for AtomicCmpXchgInstData {}
impl core::hash::Hash for AtomicCmpXchgInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.ptr.get().hash(h);
        self.cmp.get().hash(h);
        self.new_val.get().hash(h);
        self.align.hash(h);
        self.success_ordering.hash(h);
        self.failure_ordering.hash(h);
        self.sync_scope.hash(h);
        self.weak.hash(h);
        self.volatile.hash(h);
    }
}

/// Storage payload for `atomicrmw`. Mirrors `AtomicRMWInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct AtomicRMWInstData {
    pub(crate) op: crate::atomicrmw_binop::AtomicRMWBinOp,
    pub(crate) ptr: Cell<ValueId>,
    pub(crate) value: Cell<ValueId>,
    pub(crate) align: crate::align::MaybeAlign,
    pub(crate) ordering: crate::atomic_ordering::AtomicOrdering,
    pub(crate) sync_scope: crate::sync_scope::SyncScope,
    pub(crate) volatile: bool,
}

impl AtomicRMWInstData {
    pub(crate) fn new(
        op: crate::atomicrmw_binop::AtomicRMWBinOp,
        ptr: ValueId,
        value: ValueId,
        config: crate::instr_types::AtomicRMWConfig,
    ) -> Self {
        Self {
            op,
            ptr: Cell::new(ptr),
            value: Cell::new(value),
            align: config.align,
            ordering: config.ordering,
            sync_scope: config.sync_scope,
            volatile: config.flags.volatile,
        }
    }
}
impl Clone for AtomicRMWInstData {
    fn clone(&self) -> Self {
        Self {
            op: self.op,
            ptr: Cell::new(self.ptr.get()),
            value: Cell::new(self.value.get()),
            align: self.align,
            ordering: self.ordering,
            sync_scope: self.sync_scope.clone(),
            volatile: self.volatile,
        }
    }
}
impl PartialEq for AtomicRMWInstData {
    fn eq(&self, other: &Self) -> bool {
        self.op == other.op
            && self.ptr.get() == other.ptr.get()
            && self.value.get() == other.value.get()
            && self.align == other.align
            && self.ordering == other.ordering
            && self.sync_scope == other.sync_scope
            && self.volatile == other.volatile
    }
}
impl Eq for AtomicRMWInstData {}
impl core::hash::Hash for AtomicRMWInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.op.hash(h);
        self.ptr.get().hash(h);
        self.value.get().hash(h);
        self.align.hash(h);
        self.ordering.hash(h);
        self.sync_scope.hash(h);
        self.volatile.hash(h);
    }
}

/// Flags for `cmpxchg`. Mirrors `AtomicCmpXchgInst::isWeak` /
/// `isVolatile`. Default is non-weak, non-volatile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CmpXchgFlags {
    pub(crate) weak: bool,
    pub(crate) volatile: bool,
}

impl CmpXchgFlags {
    /// Default flags: not weak, not volatile.
    #[inline]
    pub const fn new() -> Self {
        Self {
            weak: false,
            volatile: false,
        }
    }
    /// Set the `weak` flag (mirrors `cmpxchg weak ...`).
    #[inline]
    #[must_use]
    pub const fn weak(mut self) -> Self {
        self.weak = true;
        self
    }
    /// Set the `volatile` flag.
    #[inline]
    #[must_use]
    pub const fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }
}

/// Flags for `atomicrmw`. Mirrors `AtomicRMWInst::isVolatile`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct AtomicRMWFlags {
    pub(crate) volatile: bool,
}

impl AtomicRMWFlags {
    #[inline]
    pub const fn new() -> Self {
        Self { volatile: false }
    }
    #[inline]
    #[must_use]
    pub const fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }
}

// --------------------------------------------------------------------------
// Variable-arity terminators: switch / indirectbr
// --------------------------------------------------------------------------

/// Storage payload for `switch`. Mirrors `SwitchInst`
/// (`Instructions.h`). The case list is mutable through the
/// [`Open`](crate::term_open_state::Open)-typestate handle's
/// `add_case` method (Doctrine D1); storage uses [`RefCell`] so the
/// mutation goes through `&self`.
#[derive(Debug)]
pub(crate) struct SwitchInstData {
    pub(crate) cond: Cell<ValueId>,
    pub(crate) default_bb: Cell<ValueId>,
    /// Each entry is `(case_value_id, dest_bb_id)`. Mirrors the
    /// `(Constant, BB)` pairs in `SwitchInst::Case`.
    pub(crate) cases: core::cell::RefCell<Vec<(Cell<ValueId>, ValueId)>>,
}

impl SwitchInstData {
    pub(crate) fn new(cond: ValueId, default_bb: ValueId) -> Self {
        Self {
            cond: Cell::new(cond),
            default_bb: Cell::new(default_bb),
            cases: core::cell::RefCell::new(Vec::new()),
        }
    }
}
impl Clone for SwitchInstData {
    fn clone(&self) -> Self {
        Self {
            cond: Cell::new(self.cond.get()),
            default_bb: Cell::new(self.default_bb.get()),
            cases: core::cell::RefCell::new(
                self.cases
                    .borrow()
                    .iter()
                    .map(|(v, b)| (Cell::new(v.get()), *b))
                    .collect(),
            ),
        }
    }
}
impl PartialEq for SwitchInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.cond.get() != other.cond.get() || self.default_bb.get() != other.default_bb.get() {
            return false;
        }
        let a = self.cases.borrow();
        let b = other.cases.borrow();
        a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|((va, ba), (vb, bbid))| va.get() == vb.get() && ba == bbid)
    }
}
impl Eq for SwitchInstData {}
impl core::hash::Hash for SwitchInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.cond.get().hash(h);
        self.default_bb.get().hash(h);
        for (v, b) in self.cases.borrow().iter() {
            v.get().hash(h);
            b.hash(h);
        }
    }
}

/// Storage payload for `indirectbr`. Mirrors `IndirectBrInst`
/// (`Instructions.h`). Destinations are mutable through the
/// [`Open`](crate::term_open_state::Open)-typestate handle's
/// `add_destination` method.
#[derive(Debug)]
pub(crate) struct IndirectBrInstData {
    pub(crate) addr: Cell<ValueId>,
    pub(crate) destinations: core::cell::RefCell<Vec<ValueId>>,
}

impl IndirectBrInstData {
    pub(crate) fn new(addr: ValueId) -> Self {
        Self {
            addr: Cell::new(addr),
            destinations: core::cell::RefCell::new(Vec::new()),
        }
    }
}
impl Clone for IndirectBrInstData {
    fn clone(&self) -> Self {
        Self {
            addr: Cell::new(self.addr.get()),
            destinations: core::cell::RefCell::new(self.destinations.borrow().clone()),
        }
    }
}
impl PartialEq for IndirectBrInstData {
    fn eq(&self, other: &Self) -> bool {
        self.addr.get() == other.addr.get()
            && *self.destinations.borrow() == *other.destinations.borrow()
    }
}
impl Eq for IndirectBrInstData {}
impl core::hash::Hash for IndirectBrInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.addr.get().hash(h);
        for d in self.destinations.borrow().iter() {
            d.hash(h);
        }
    }
}

// --------------------------------------------------------------------------
// EH-call terminators: invoke / callbr
// --------------------------------------------------------------------------

/// Storage payload for `invoke`. Mirrors `InvokeInst`
/// (`Instructions.h`). Layout reuses the call-site shape from
/// [`CallInstData`] plus the normal/unwind destination block ids.
#[derive(Debug)]
pub(crate) struct InvokeInstData {
    pub(crate) callee: Cell<ValueId>,
    pub(crate) fn_ty: crate::r#type::TypeId,
    pub(crate) args: Box<[Cell<ValueId>]>,
    pub(crate) calling_conv: crate::CallingConv,
    pub(crate) normal_dest: Cell<ValueId>,
    pub(crate) unwind_dest: Cell<ValueId>,
}

impl InvokeInstData {
    pub(crate) fn new(
        callee: ValueId,
        fn_ty: crate::r#type::TypeId,
        args: impl IntoIterator<Item = ValueId>,
        calling_conv: crate::CallingConv,
        normal_dest: ValueId,
        unwind_dest: ValueId,
    ) -> Self {
        Self {
            callee: Cell::new(callee),
            fn_ty,
            args: args.into_iter().map(Cell::new).collect(),
            calling_conv,
            normal_dest: Cell::new(normal_dest),
            unwind_dest: Cell::new(unwind_dest),
        }
    }
}
impl Clone for InvokeInstData {
    fn clone(&self) -> Self {
        Self {
            callee: Cell::new(self.callee.get()),
            fn_ty: self.fn_ty,
            args: self.args.iter().map(|c| Cell::new(c.get())).collect(),
            calling_conv: self.calling_conv,
            normal_dest: Cell::new(self.normal_dest.get()),
            unwind_dest: Cell::new(self.unwind_dest.get()),
        }
    }
}
impl PartialEq for InvokeInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.callee.get() != other.callee.get()
            || self.fn_ty != other.fn_ty
            || self.calling_conv != other.calling_conv
            || self.normal_dest.get() != other.normal_dest.get()
            || self.unwind_dest.get() != other.unwind_dest.get()
            || self.args.len() != other.args.len()
        {
            return false;
        }
        self.args
            .iter()
            .zip(other.args.iter())
            .all(|(a, b)| a.get() == b.get())
    }
}
impl Eq for InvokeInstData {}
impl core::hash::Hash for InvokeInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.callee.get().hash(h);
        self.fn_ty.hash(h);
        for arg in self.args.iter() {
            arg.get().hash(h);
        }
        self.calling_conv.hash(h);
        self.normal_dest.get().hash(h);
        self.unwind_dest.get().hash(h);
    }
}

/// Storage payload for `callbr`. Mirrors `CallBrInst`
/// (`Instructions.h`). The default destination is the fallthrough
/// block; the indirect destinations are the asm-listed labels.
#[derive(Debug)]
pub(crate) struct CallBrInstData {
    pub(crate) callee: Cell<ValueId>,
    pub(crate) fn_ty: crate::r#type::TypeId,
    pub(crate) args: Box<[Cell<ValueId>]>,
    pub(crate) calling_conv: crate::CallingConv,
    pub(crate) default_dest: Cell<ValueId>,
    pub(crate) indirect_dests: Box<[Cell<ValueId>]>,
}

impl CallBrInstData {
    pub(crate) fn new(
        callee: ValueId,
        fn_ty: crate::r#type::TypeId,
        args: impl IntoIterator<Item = ValueId>,
        calling_conv: crate::CallingConv,
        default_dest: ValueId,
        indirect_dests: impl IntoIterator<Item = ValueId>,
    ) -> Self {
        Self {
            callee: Cell::new(callee),
            fn_ty,
            args: args.into_iter().map(Cell::new).collect(),
            calling_conv,
            default_dest: Cell::new(default_dest),
            indirect_dests: indirect_dests.into_iter().map(Cell::new).collect(),
        }
    }
}
impl Clone for CallBrInstData {
    fn clone(&self) -> Self {
        Self {
            callee: Cell::new(self.callee.get()),
            fn_ty: self.fn_ty,
            args: self.args.iter().map(|c| Cell::new(c.get())).collect(),
            calling_conv: self.calling_conv,
            default_dest: Cell::new(self.default_dest.get()),
            indirect_dests: self
                .indirect_dests
                .iter()
                .map(|c| Cell::new(c.get()))
                .collect(),
        }
    }
}
impl PartialEq for CallBrInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.callee.get() != other.callee.get()
            || self.fn_ty != other.fn_ty
            || self.calling_conv != other.calling_conv
            || self.default_dest.get() != other.default_dest.get()
            || self.args.len() != other.args.len()
            || self.indirect_dests.len() != other.indirect_dests.len()
        {
            return false;
        }
        self.args
            .iter()
            .zip(other.args.iter())
            .all(|(a, b)| a.get() == b.get())
            && self
                .indirect_dests
                .iter()
                .zip(other.indirect_dests.iter())
                .all(|(a, b)| a.get() == b.get())
    }
}
impl Eq for CallBrInstData {}
impl core::hash::Hash for CallBrInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.callee.get().hash(h);
        self.fn_ty.hash(h);
        for arg in self.args.iter() {
            arg.get().hash(h);
        }
        self.calling_conv.hash(h);
        self.default_dest.get().hash(h);
        for d in self.indirect_dests.iter() {
            d.get().hash(h);
        }
    }
}

// --------------------------------------------------------------------------
// EH-data: landingpad / resume
// --------------------------------------------------------------------------

/// One clause of a `landingpad`. Mirrors `LandingPadInst::ClauseType`
/// in `Instructions.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LandingPadClauseKind {
    /// `catch <ty> <val>` --- a typeinfo. Mirrors
    /// `LandingPadInst::ClauseType::Catch`.
    Catch,
    /// `filter <ty> <val>` --- an array constant of typeinfos.
    /// Mirrors `LandingPadInst::ClauseType::Filter`.
    Filter,
}

/// Storage payload for `landingpad`. Mirrors `LandingPadInst`
/// (`Instructions.h`).
///
/// The result type lives in the host `ValueData::ty`. The clause list
/// is mutable through the [`crate::term_open_state::Open`]-typestate
/// handle's `add_clause` method.
#[derive(Debug)]
pub(crate) struct LandingPadInstData {
    pub(crate) cleanup: core::cell::Cell<bool>,
    pub(crate) clauses: core::cell::RefCell<Vec<(LandingPadClauseKind, Cell<ValueId>)>>,
}

impl LandingPadInstData {
    pub(crate) fn new(cleanup: bool) -> Self {
        Self {
            cleanup: core::cell::Cell::new(cleanup),
            clauses: core::cell::RefCell::new(Vec::new()),
        }
    }
}
impl Clone for LandingPadInstData {
    fn clone(&self) -> Self {
        Self {
            cleanup: core::cell::Cell::new(self.cleanup.get()),
            clauses: core::cell::RefCell::new(
                self.clauses
                    .borrow()
                    .iter()
                    .map(|(k, c)| (*k, Cell::new(c.get())))
                    .collect(),
            ),
        }
    }
}
impl PartialEq for LandingPadInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.cleanup.get() != other.cleanup.get() {
            return false;
        }
        let a = self.clauses.borrow();
        let b = other.clauses.borrow();
        a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|((ka, ca), (kb, cb))| ka == kb && ca.get() == cb.get())
    }
}
impl Eq for LandingPadInstData {}
impl core::hash::Hash for LandingPadInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.cleanup.get().hash(h);
        for (k, c) in self.clauses.borrow().iter() {
            k.hash(h);
            c.get().hash(h);
        }
    }
}

/// Storage payload for `resume`. Mirrors `ResumeInst`
/// (`Instructions.h`). Single SSA operand; no successors; terminator.
#[derive(Debug)]
pub(crate) struct ResumeInstData {
    pub(crate) value: Cell<ValueId>,
}

impl ResumeInstData {
    pub(crate) fn new(value: ValueId) -> Self {
        Self {
            value: Cell::new(value),
        }
    }
}
impl Clone for ResumeInstData {
    fn clone(&self) -> Self {
        Self {
            value: Cell::new(self.value.get()),
        }
    }
}
impl PartialEq for ResumeInstData {
    fn eq(&self, other: &Self) -> bool {
        self.value.get() == other.value.get()
    }
}
impl Eq for ResumeInstData {}
impl core::hash::Hash for ResumeInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.value.get().hash(h);
    }
}

// --------------------------------------------------------------------------
// Funclet ops: cleanuppad / cleanupret / catchpad / catchret / catchswitch
// --------------------------------------------------------------------------

/// Storage payload for `cleanuppad`. Mirrors `CleanupPadInst`
/// (`Instructions.h`). The result is a `token`-typed value.
#[derive(Debug)]
pub(crate) struct CleanupPadInstData {
    pub(crate) parent_pad: Cell<Option<ValueId>>,
    pub(crate) args: Box<[Cell<ValueId>]>,
}

impl CleanupPadInstData {
    pub(crate) fn new(
        parent_pad: Option<ValueId>,
        args: impl IntoIterator<Item = ValueId>,
    ) -> Self {
        Self {
            parent_pad: Cell::new(parent_pad),
            args: args.into_iter().map(Cell::new).collect(),
        }
    }
}
impl Clone for CleanupPadInstData {
    fn clone(&self) -> Self {
        Self {
            parent_pad: Cell::new(self.parent_pad.get()),
            args: self.args.iter().map(|c| Cell::new(c.get())).collect(),
        }
    }
}
impl PartialEq for CleanupPadInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.parent_pad.get() != other.parent_pad.get() || self.args.len() != other.args.len() {
            return false;
        }
        self.args
            .iter()
            .zip(other.args.iter())
            .all(|(a, b)| a.get() == b.get())
    }
}
impl Eq for CleanupPadInstData {}
impl core::hash::Hash for CleanupPadInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.parent_pad.get().hash(h);
        for arg in self.args.iter() {
            arg.get().hash(h);
        }
    }
}

/// Storage payload for `catchpad`. Mirrors `CatchPadInst`
/// (`Instructions.h`). Same shape as [`CleanupPadInstData`] but the
/// parent must be a `catchswitch` (verifier rule).
#[derive(Debug)]
pub(crate) struct CatchPadInstData {
    pub(crate) parent_pad: Cell<Option<ValueId>>,
    pub(crate) args: Box<[Cell<ValueId>]>,
}

impl CatchPadInstData {
    pub(crate) fn new(
        parent_pad: Option<ValueId>,
        args: impl IntoIterator<Item = ValueId>,
    ) -> Self {
        Self {
            parent_pad: Cell::new(parent_pad),
            args: args.into_iter().map(Cell::new).collect(),
        }
    }
}
impl Clone for CatchPadInstData {
    fn clone(&self) -> Self {
        Self {
            parent_pad: Cell::new(self.parent_pad.get()),
            args: self.args.iter().map(|c| Cell::new(c.get())).collect(),
        }
    }
}
impl PartialEq for CatchPadInstData {
    fn eq(&self, other: &Self) -> bool {
        if self.parent_pad.get() != other.parent_pad.get() || self.args.len() != other.args.len() {
            return false;
        }
        self.args
            .iter()
            .zip(other.args.iter())
            .all(|(a, b)| a.get() == b.get())
    }
}
impl Eq for CatchPadInstData {}
impl core::hash::Hash for CatchPadInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.parent_pad.get().hash(h);
        for arg in self.args.iter() {
            arg.get().hash(h);
        }
    }
}

/// Storage payload for `catchret`. Mirrors `CatchReturnInst`
/// (`Instructions.h`).
#[derive(Debug, Clone)]
pub(crate) struct CatchReturnInstData {
    pub(crate) catch_pad: Cell<ValueId>,
    pub(crate) target_bb: ValueId,
}

impl CatchReturnInstData {
    pub(crate) fn new(catch_pad: ValueId, target_bb: ValueId) -> Self {
        Self {
            catch_pad: Cell::new(catch_pad),
            target_bb,
        }
    }
}
impl PartialEq for CatchReturnInstData {
    fn eq(&self, other: &Self) -> bool {
        self.catch_pad.get() == other.catch_pad.get() && self.target_bb == other.target_bb
    }
}
impl Eq for CatchReturnInstData {}
impl core::hash::Hash for CatchReturnInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.catch_pad.get().hash(h);
        self.target_bb.hash(h);
    }
}

/// Storage payload for `cleanupret`. Mirrors `CleanupReturnInst`
/// (`Instructions.h`). `unwind_dest = None` represents `unwind to
/// caller`; `Some(bb_id)` is `unwind label %bb`.
#[derive(Debug, Clone)]
pub(crate) struct CleanupReturnInstData {
    pub(crate) cleanup_pad: Cell<ValueId>,
    pub(crate) unwind_dest: Option<ValueId>,
}

impl CleanupReturnInstData {
    pub(crate) fn new(cleanup_pad: ValueId, unwind_dest: Option<ValueId>) -> Self {
        Self {
            cleanup_pad: Cell::new(cleanup_pad),
            unwind_dest,
        }
    }
}
impl PartialEq for CleanupReturnInstData {
    fn eq(&self, other: &Self) -> bool {
        self.cleanup_pad.get() == other.cleanup_pad.get() && self.unwind_dest == other.unwind_dest
    }
}
impl Eq for CleanupReturnInstData {}
impl core::hash::Hash for CleanupReturnInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.cleanup_pad.get().hash(h);
        self.unwind_dest.hash(h);
    }
}

/// Storage payload for `catchswitch`. Mirrors `CatchSwitchInst`
/// (`Instructions.h`). Variable-arity (handlers).
#[derive(Debug)]
pub(crate) struct CatchSwitchInstData {
    pub(crate) parent_pad: Cell<Option<ValueId>>,
    pub(crate) unwind_dest: Cell<Option<ValueId>>,
    pub(crate) handlers: core::cell::RefCell<Vec<ValueId>>,
}

impl CatchSwitchInstData {
    pub(crate) fn new(parent_pad: Option<ValueId>, unwind_dest: Option<ValueId>) -> Self {
        Self {
            parent_pad: Cell::new(parent_pad),
            unwind_dest: Cell::new(unwind_dest),
            handlers: core::cell::RefCell::new(Vec::new()),
        }
    }
}
impl Clone for CatchSwitchInstData {
    fn clone(&self) -> Self {
        Self {
            parent_pad: Cell::new(self.parent_pad.get()),
            unwind_dest: Cell::new(self.unwind_dest.get()),
            handlers: core::cell::RefCell::new(self.handlers.borrow().clone()),
        }
    }
}
impl PartialEq for CatchSwitchInstData {
    fn eq(&self, other: &Self) -> bool {
        self.parent_pad.get() == other.parent_pad.get()
            && self.unwind_dest.get() == other.unwind_dest.get()
            && *self.handlers.borrow() == *other.handlers.borrow()
    }
}
impl Eq for CatchSwitchInstData {}
impl core::hash::Hash for CatchSwitchInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.parent_pad.get().hash(h);
        self.unwind_dest.get().hash(h);
        for h_id in self.handlers.borrow().iter() {
            h_id.hash(h);
        }
    }
}

/// Bundled configuration for [`crate::IRBuilder::build_atomic_cmpxchg`].
/// Mirrors the per-instruction state stored on `AtomicCmpXchgInst`
/// (orderings + scope + flags + alignment).
#[derive(Debug, Clone)]
pub struct AtomicCmpXchgConfig {
    pub success_ordering: crate::atomic_ordering::AtomicOrdering,
    pub failure_ordering: crate::atomic_ordering::AtomicOrdering,
    pub sync_scope: crate::sync_scope::SyncScope,
    pub flags: CmpXchgFlags,
    pub align: crate::align::MaybeAlign,
}

/// Bundled configuration for [`crate::IRBuilder::build_atomicrmw`].
/// Mirrors the per-instruction state stored on `AtomicRMWInst`.
#[derive(Debug, Clone)]
pub struct AtomicRMWConfig {
    pub ordering: crate::atomic_ordering::AtomicOrdering,
    pub sync_scope: crate::sync_scope::SyncScope,
    pub flags: AtomicRMWFlags,
    pub align: crate::align::MaybeAlign,
}

/// Bundled configuration for atomic [`crate::IRBuilder::build_int_load_atomic`]
/// / `build_load_atomic` / `build_int_load_atomic_volatile`. Mirrors the
/// state passed to the 5-arg upstream constructor
/// `LoadInst::LoadInst(Type*, Value*, Twine&, bool isVolatile, Align,
/// AtomicOrdering, SyncScope::ID)` (`Instructions.h`).
#[derive(Debug, Clone)]
pub struct AtomicLoadConfig {
    pub ordering: crate::atomic_ordering::AtomicOrdering,
    pub sync_scope: crate::sync_scope::SyncScope,
    pub align: crate::align::Align,
    pub volatile: bool,
}

impl AtomicLoadConfig {
    /// Convenience constructor with `volatile = false`. The 4-arg shape
    /// matches the common-case upstream `LoadInst` constructor that omits
    /// the volatile slot.
    pub fn new(
        ordering: crate::atomic_ordering::AtomicOrdering,
        sync_scope: crate::sync_scope::SyncScope,
        align: crate::align::Align,
    ) -> Self {
        Self {
            ordering,
            sync_scope,
            align,
            volatile: false,
        }
    }

    /// Flip the volatile bit. Mirrors `LoadInst::setVolatile(true)`.
    pub fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }
}

/// Bundled configuration for atomic [`crate::IRBuilder::build_store_atomic`]
/// / `build_store_atomic_volatile`. Mirrors the state passed to the 6-arg
/// upstream constructor `StoreInst::StoreInst(Value*, Value*, bool isVolatile,
/// Align, AtomicOrdering, SyncScope::ID)`.
#[derive(Debug, Clone)]
pub struct AtomicStoreConfig {
    pub ordering: crate::atomic_ordering::AtomicOrdering,
    pub sync_scope: crate::sync_scope::SyncScope,
    pub align: crate::align::Align,
    pub volatile: bool,
}

impl AtomicStoreConfig {
    pub fn new(
        ordering: crate::atomic_ordering::AtomicOrdering,
        sync_scope: crate::sync_scope::SyncScope,
        align: crate::align::Align,
    ) -> Self {
        Self {
            ordering,
            sync_scope,
            align,
            volatile: false,
        }
    }

    /// Flip the volatile bit. Mirrors `StoreInst::setVolatile(true)`.
    pub fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }
}
