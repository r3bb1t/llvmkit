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
}

impl BinaryOpData {
    pub(crate) fn new(lhs: ValueId, rhs: ValueId) -> Self {
        Self {
            lhs: Cell::new(lhs),
            rhs: Cell::new(rhs),
            no_unsigned_wrap: false,
            no_signed_wrap: false,
            is_exact: false,
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
        }
    }
}
impl Clone for FCmpInstData {
    fn clone(&self) -> Self {
        Self {
            predicate: self.predicate,
            lhs: Cell::new(self.lhs.get()),
            rhs: Cell::new(self.rhs.get()),
        }
    }
}
impl PartialEq for FCmpInstData {
    fn eq(&self, other: &Self) -> bool {
        self.predicate == other.predicate
            && self.lhs.get() == other.lhs.get()
            && self.rhs.get() == other.rhs.get()
    }
}
impl Eq for FCmpInstData {}
impl core::hash::Hash for FCmpInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.predicate.hash(h);
        self.lhs.get().hash(h);
        self.rhs.get().hash(h);
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
/// (`Instructions.h`). Atomic ordering / sync-scope are deferred to
/// the atomic-ops session.
#[derive(Debug)]
pub(crate) struct LoadInstData {
    pub(crate) pointee_ty: crate::r#type::TypeId,
    pub(crate) ptr: Cell<ValueId>,
    pub(crate) align: crate::align::MaybeAlign,
    pub(crate) volatile: bool,
}

impl LoadInstData {
    pub(crate) fn new(
        pointee_ty: crate::r#type::TypeId,
        ptr: ValueId,
        align: crate::align::MaybeAlign,
        volatile: bool,
    ) -> Self {
        Self {
            pointee_ty,
            ptr: Cell::new(ptr),
            align,
            volatile,
        }
    }
}
impl Clone for LoadInstData {
    fn clone(&self) -> Self {
        Self {
            pointee_ty: self.pointee_ty,
            ptr: Cell::new(self.ptr.get()),
            align: self.align,
            volatile: self.volatile,
        }
    }
}
impl PartialEq for LoadInstData {
    fn eq(&self, other: &Self) -> bool {
        self.pointee_ty == other.pointee_ty
            && self.ptr.get() == other.ptr.get()
            && self.align == other.align
            && self.volatile == other.volatile
    }
}
impl Eq for LoadInstData {}
impl core::hash::Hash for LoadInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.pointee_ty.hash(h);
        self.ptr.get().hash(h);
        self.align.hash(h);
        self.volatile.hash(h);
    }
}

/// Storage payload for `store`. Mirrors `StoreInst`
/// (`Instructions.h`).
#[derive(Debug)]
pub(crate) struct StoreInstData {
    pub(crate) value: Cell<ValueId>,
    pub(crate) ptr: Cell<ValueId>,
    pub(crate) align: crate::align::MaybeAlign,
    pub(crate) volatile: bool,
}

impl StoreInstData {
    pub(crate) fn new(
        value: ValueId,
        ptr: ValueId,
        align: crate::align::MaybeAlign,
        volatile: bool,
    ) -> Self {
        Self {
            value: Cell::new(value),
            ptr: Cell::new(ptr),
            align,
            volatile,
        }
    }
}
impl Clone for StoreInstData {
    fn clone(&self) -> Self {
        Self {
            value: Cell::new(self.value.get()),
            ptr: Cell::new(self.ptr.get()),
            align: self.align,
            volatile: self.volatile,
        }
    }
}
impl PartialEq for StoreInstData {
    fn eq(&self, other: &Self) -> bool {
        self.value.get() == other.value.get()
            && self.ptr.get() == other.ptr.get()
            && self.align == other.align
            && self.volatile == other.volatile
    }
}
impl Eq for StoreInstData {}
impl core::hash::Hash for StoreInstData {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.value.get().hash(h);
        self.ptr.get().hash(h);
        self.align.hash(h);
        self.volatile.hash(h);
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
