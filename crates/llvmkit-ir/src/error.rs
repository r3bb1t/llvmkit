//! Crate-wide error type.
//!
//! Per AGENTS.md and the IR foundation plan, every fallible IR API returns
//! [`IrResult`] (an alias for `Result<T, IrError>`). Pure constructors
//! (e.g. `Module::i32_type`) stay infallible; validation constructors and
//! all builder methods funnel through this enum.
//!
//! Variants are added phase-by-phase as new failure modes appear. Where
//! `'ctx` lifetime branding catches a class of bugs at compile time
//! (e.g. cross-Module mixing), the corresponding runtime variant is
//! deliberately *not* present here — see the IR foundation plan, Pivot 4.

use core::fmt;

/// Human-readable label for a [`Type`](crate::Type) kind, embedded in
/// diagnostics that don't want to carry a borrowed type handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TypeKindLabel {
    Void,
    Half,
    BFloat,
    Float,
    Double,
    X86Fp80,
    Fp128,
    PpcFp128,
    Label,
    Metadata,
    Token,
    X86Amx,
    Integer,
    Function,
    Pointer,
    Struct,
    Array,
    FixedVector,
    ScalableVector,
    TypedPointer,
    TargetExt,
}

impl fmt::Display for TypeKindLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Lowercase forms match LLVM's IR textual syntax where applicable.
        let s = match self {
            TypeKindLabel::Void => "void",
            TypeKindLabel::Half => "half",
            TypeKindLabel::BFloat => "bfloat",
            TypeKindLabel::Float => "float",
            TypeKindLabel::Double => "double",
            TypeKindLabel::X86Fp80 => "x86_fp80",
            TypeKindLabel::Fp128 => "fp128",
            TypeKindLabel::PpcFp128 => "ppc_fp128",
            TypeKindLabel::Label => "label",
            TypeKindLabel::Metadata => "metadata",
            TypeKindLabel::Token => "token",
            TypeKindLabel::X86Amx => "x86_amx",
            TypeKindLabel::Integer => "integer",
            TypeKindLabel::Function => "function",
            TypeKindLabel::Pointer => "pointer",
            TypeKindLabel::Struct => "struct",
            TypeKindLabel::Array => "array",
            TypeKindLabel::FixedVector => "fixed-vector",
            TypeKindLabel::ScalableVector => "scalable-vector",
            TypeKindLabel::TypedPointer => "typed-pointer",
            TypeKindLabel::TargetExt => "target-ext",
        };
        f.write_str(s)
    }
}

/// Human-readable label for a [`Value`](crate::Value)'s category, embedded
/// in diagnostics that don't want to carry a borrowed value handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ValueCategoryLabel {
    Constant,
    Argument,
    BasicBlock,
    Function,
    Instruction,
}

impl fmt::Display for ValueCategoryLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ValueCategoryLabel::Constant => "constant",
            ValueCategoryLabel::Argument => "argument",
            ValueCategoryLabel::BasicBlock => "basic-block",
            ValueCategoryLabel::Function => "function",
            ValueCategoryLabel::Instruction => "instruction",
        };
        f.write_str(s)
    }
}

/// Categorical discriminator over the verifier-rule set.
///
/// One variant per rule the verifier can enforce. Tests pattern-match
/// on this enum to assert which invariant fired without coupling to the
/// human-readable diagnostic message. New rules are added
/// non-breakingly via `#[non_exhaustive]`.
///
/// Each variant cites its `Verifier::visit*` C++ method in
/// `llvm/lib/IR/Verifier.cpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VerifierRule {
    /// Binary operator: LHS and RHS operand types differ.
    /// Mirrors `Verifier::visitBinaryOperator`.
    BinaryOperandsTypeMismatch,
    /// Binary operator's result type does not match its operand type.
    /// Mirrors `Verifier::visitBinaryOperator`.
    BinaryResultTypeMismatch,
    /// Integer arithmetic / shift / logical opcode given a non-integer
    /// operand. Mirrors `Verifier::visitBinaryOperator`.
    IntegerOpNonIntegerOperand,
    /// Floating-point arithmetic opcode given a non-float operand.
    /// Mirrors `Verifier::visitBinaryOperator`.
    FloatOpNonFloatOperand,
    /// `icmp` operands have different types or are not integer/pointer.
    /// Mirrors `Verifier::visitICmpInst`.
    IcmpOperandTypeMismatch,
    /// `fcmp` operands have different types or are not floating-point.
    /// Mirrors `Verifier::visitFCmpInst`.
    FcmpOperandTypeMismatch,
    /// `ret` operand type does not match the function's declared
    /// return type. Mirrors `Verifier::visitReturnInst`.
    ReturnTypeMismatch,
    /// Conditional `br` was given a non-`i1` condition operand.
    /// Mirrors `Verifier::visitBranchInst`.
    BranchConditionNotI1,
    /// A basic block has no terminator at all.
    /// Mirrors `Verifier::visitBasicBlock`.
    MissingTerminator,
    /// A basic block has more than one terminator, or a terminator
    /// that is not the last instruction.
    /// Mirrors `Verifier::visitInstruction` ("It is not the terminator
    /// of its parent").
    MisplacedTerminator,
    /// `phi` appears after a non-phi instruction within the same
    /// block. Mirrors `Verifier::visitPHINode` ("PHI nodes not grouped
    /// at top of block").
    PhiNotAtTop,
    /// `phi` references a predecessor block that is not actually a
    /// CFG predecessor of the phi's block, or omits a real predecessor.
    /// Mirrors `Verifier::visitPHINode`.
    PhiPredecessorMismatch,
    /// `phi` incoming-value type differs from the phi's result type.
    /// Mirrors `Verifier::visitPHINode`.
    PhiIncomingTypeMismatch,
    /// `phi` has duplicate entries from the same predecessor with
    /// differing values. Mirrors `Verifier::visitPHINode`
    /// ("PHI node has multiple entries for the same basic block with
    /// different incoming values").
    AmbiguousPhi,
    /// `call` callee is not a function-typed value.
    /// Mirrors `Verifier::visitCallBase`.
    CallNonFunction,
    /// `call` argument count differs from the callee signature's
    /// parameter count (and the callee is not vararg).
    /// Mirrors `Verifier::visitCallBase`.
    CallArgCountMismatch,
    /// `call` argument type differs from the callee signature's
    /// parameter type at the same slot.
    /// Mirrors `Verifier::visitCallBase`.
    CallArgTypeMismatch,
    /// `select` condition operand is not `i1`.
    /// Mirrors `Verifier::visitSelectInst`.
    SelectConditionNotI1,
    /// `select` true-arm and false-arm types differ, or differ from
    /// the result type. Mirrors `Verifier::visitSelectInst`.
    SelectArmTypeMismatch,
    /// `getelementptr` base operand is not a pointer (or vector of
    /// pointers). Mirrors `Verifier::visitGetElementPtrInst`.
    GepNonPointerBase,
    /// `getelementptr` source element type is unsized.
    /// Mirrors `Verifier::visitGetElementPtrInst`.
    GepUnsizedSourceType,
    /// `getelementptr` index operand is non-integer.
    /// Mirrors `Verifier::visitGetElementPtrInst`.
    GepNonIntegerIndex,
    /// `alloca` allocated type is unsized (function/void/label/...).
    /// Mirrors `Verifier::visitAllocaInst`.
    AllocaUnsizedType,
    /// `alloca` num-elements operand is not an integer.
    /// Mirrors `Verifier::visitAllocaInst`.
    AllocaNonIntegerCount,
    /// `load` pointer operand is not a pointer.
    /// Mirrors `Verifier::visitLoadInst`.
    LoadNonPointer,
    /// `load` pointee type is unsized.
    /// Mirrors `Verifier::visitLoadInst`.
    LoadUnsizedType,
    /// `store` pointer operand is not a pointer.
    /// Mirrors `Verifier::visitStoreInst`.
    StoreNonPointer,
    /// `store` value-operand type is unsized.
    /// Mirrors `Verifier::visitStoreInst`.
    StoreUnsizedType,
    /// Atomic `load` carries `Release` or `AcquireRelease` ordering.
    /// Mirrors `Verifier::visitLoadInst` ("Load cannot have Release ordering").
    AtomicLoadInvalidOrdering,
    /// Atomic `store` carries `Acquire` or `AcquireRelease` ordering.
    /// Mirrors `Verifier::visitStoreInst` ("Store cannot have Acquire ordering").
    AtomicStoreInvalidOrdering,
    /// Atomic load/store operand type is not integer / pointer / floating-point.
    /// Mirrors `Verifier::visitLoadInst` / `visitStoreInst` ("atomic load/store
    /// operand must have integer, pointer, floating point, or vector type!").
    AtomicLoadStoreInvalidType,
    /// Atomic memory access' bit size is not a power-of-two between 8
    /// and 128. Mirrors `Verifier::checkAtomicMemAccessSize`.
    AtomicLoadStoreInvalidSize,
    /// Non-atomic load/store carries a non-default `syncscope`. Mirrors
    /// `Verifier::visitLoadInst` / `visitStoreInst` ("Non-atomic load/store
    /// cannot have SynchronizationScope specified").
    NonAtomicWithSyncScope,
    /// `bitcast` source and destination bit widths differ.
    /// Mirrors `Verifier::visitBitCastInst`.
    BitCastSizeMismatch,
    /// Cast opcode source/destination kind constraint failed
    /// (e.g. `zext` from a non-integer, `fptrunc` to an integer).
    /// Mirrors `Verifier::visit{Trunc,ZExt,SExt,FpTrunc,FpExt,FpToUI,
    /// FpToSI,UIToFp,SIToFp,PtrToInt,IntToPtr}Inst`.
    CastTypeMismatch,
    /// Cast width relationship is wrong (e.g. `trunc` to a wider
    /// integer; `fpext` to a narrower float).
    /// Mirrors the same `visit*Inst` family.
    CastWidthMismatch,
    /// A non-`phi` instruction references its own result as an operand.
    /// Mirrors `Verifier::visitInstruction` ("Only PHI nodes may
    /// reference their own value").
    SelfReference,
    /// In-block use-before-def: an operand whose defining instruction
    /// follows the use in the same basic block.
    /// Mirrors `Verifier::verifyDominatesUse`.
    /// `fneg` operand or result is not floating-point, or result type
    /// does not match operand type. Mirrors `Verifier::visitFNeg`.
    FNegTypeMismatch,
    /// `freeze` result type differs from operand type. Mirrors
    /// `Verifier::visitFreeze` ("Freeze should produce its operand's
    /// type").
    FreezeTypeMismatch,
    /// `va_arg` source operand is not a pointer. Mirrors
    /// `Verifier::visitVAArgInst`.
    VAArgNonPointerOperand,
    /// `extractvalue` / `insertvalue` aggregate operand is not
    /// struct- or array-typed. Mirrors `Verifier::visitExtractValueInst`
    /// / `Verifier::visitInsertValueInst`.
    AggregateOpNonAggregate,
    /// `extractvalue` / `insertvalue` index walks past the leaves of
    /// the aggregate. Mirrors the same C++ visitors.
    AggregateIndexOutOfRange,
    /// `insertvalue` inserted-value type does not match the aggregate's
    /// leaf type at the index path.
    InsertValueLeafTypeMismatch,
    /// `extractelement` / `insertelement` operand is not vector-typed,
    /// or `extractelement` result type does not match the vector's
    /// element type. Mirrors `Verifier::visitExtractElementInst` /
    /// `Verifier::visitInsertElementInst`.
    VectorElementOpTypeMismatch,
    /// `shufflevector` operands disagree in element type, or the result
    /// type does not match the mask length. Mirrors
    /// `Verifier::visitShuffleVectorInst`.
    ShuffleVectorTypeMismatch,
    /// Atomic op (`fence`, `cmpxchg`, `atomicrmw`, `load atomic`, `store
    /// atomic`) given an invalid memory ordering. Mirrors
    /// `Verifier::visitFenceInst` / `visitAtomicCmpXchgInst` /
    /// `visitAtomicRMWInst`.
    AtomicInvalidOrdering,
    /// `cmpxchg` / `atomicrmw` pointer operand is not a pointer.
    AtomicNonPointerOperand,
    /// `atomicrmw` operand value type does not match the operation's
    /// expected element type, or the FP-only ops were given a non-FP
    /// operand.
    AtomicRMWOperandTypeMismatch,
    /// `switch` condition is not integer-typed, or a case value type
    /// disagrees with the condition. Mirrors `Verifier::visitSwitchInst`.
    SwitchOperandTypeMismatch,
    /// `indirectbr` address operand is not a pointer. Mirrors
    /// `Verifier::visitIndirectBrInst`.
    IndirectBrNonPointerAddress,
    UseBeforeDef,
}

impl fmt::Display for VerifierRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::BinaryOperandsTypeMismatch => "binary operands have differing types",
            Self::BinaryResultTypeMismatch => "binary result type differs from operand type",
            Self::IntegerOpNonIntegerOperand => "integer opcode with non-integer operand",
            Self::FloatOpNonFloatOperand => "float opcode with non-floating-point operand",
            Self::IcmpOperandTypeMismatch => {
                "icmp operand types do not match or are not integer/pointer"
            }
            Self::FcmpOperandTypeMismatch => {
                "fcmp operand types do not match or are not floating-point"
            }
            Self::ReturnTypeMismatch => "return value does not match function result type",
            Self::BranchConditionNotI1 => "conditional branch condition is not i1",
            Self::MissingTerminator => "basic block has no terminator",
            Self::MisplacedTerminator => "terminator is not the last instruction in its block",
            Self::PhiNotAtTop => "PHI nodes not grouped at top of block",
            Self::PhiPredecessorMismatch => {
                "PHI predecessor list disagrees with control-flow graph"
            }
            Self::PhiIncomingTypeMismatch => {
                "PHI incoming value type does not match the PHI result type"
            }
            Self::AmbiguousPhi => {
                "PHI node has multiple entries for the same basic block with different incoming values"
            }
            Self::CallNonFunction => "call callee is not a function value",
            Self::CallArgCountMismatch => "call argument count does not match callee signature",
            Self::CallArgTypeMismatch => "call argument type does not match callee parameter type",
            Self::SelectConditionNotI1 => "select condition is not i1",
            Self::SelectArmTypeMismatch => {
                "select arm types differ from each other or from the result"
            }
            Self::GepNonPointerBase => "getelementptr base is not a pointer",
            Self::GepUnsizedSourceType => "getelementptr source element type is unsized",
            Self::GepNonIntegerIndex => "getelementptr index operand is not an integer",
            Self::AllocaUnsizedType => "alloca allocated type is unsized",
            Self::AllocaNonIntegerCount => "alloca num-elements operand is not an integer",
            Self::LoadNonPointer => "load pointer operand is not a pointer",
            Self::LoadUnsizedType => "loading unsized types is not allowed",
            Self::StoreNonPointer => "store pointer operand is not a pointer",
            Self::StoreUnsizedType => "storing unsized types is not allowed",
            Self::AtomicLoadInvalidOrdering => "atomic load cannot have Release ordering",
            Self::AtomicStoreInvalidOrdering => "atomic store cannot have Acquire ordering",
            Self::AtomicLoadStoreInvalidType => {
                "atomic load/store operand must have integer, pointer, floating point, or vector type"
            }
            Self::AtomicLoadStoreInvalidSize => {
                "atomic memory access' size must be byte-sized and a power of two"
            }
            Self::NonAtomicWithSyncScope => {
                "non-atomic load/store cannot have a non-default syncscope"
            }
            Self::BitCastSizeMismatch => "bitcast source and destination have differing bit widths",
            Self::CastTypeMismatch => "cast source/destination kind constraint failed",
            Self::CastWidthMismatch => "cast width relationship is invalid",
            Self::SelfReference => "only PHI nodes may reference their own value",
            Self::FNegTypeMismatch => "fneg operand/result is not floating-point or types differ",
            Self::FreezeTypeMismatch => "freeze result type does not match operand type",
            Self::VAArgNonPointerOperand => "va_arg source operand is not a pointer",
            Self::AggregateOpNonAggregate => {
                "extractvalue/insertvalue aggregate is not struct- or array-typed"
            }
            Self::AggregateIndexOutOfRange => {
                "extractvalue/insertvalue index walks past the leaves"
            }
            Self::InsertValueLeafTypeMismatch => {
                "insertvalue leaf type does not match inserted value"
            }
            Self::VectorElementOpTypeMismatch => {
                "extractelement/insertelement operand types are inconsistent with the vector"
            }
            Self::ShuffleVectorTypeMismatch => {
                "shufflevector operand or result type does not match mask"
            }
            Self::AtomicInvalidOrdering => "atomic op given an invalid memory ordering",
            Self::AtomicNonPointerOperand => "atomic op pointer operand is not a pointer",
            Self::AtomicRMWOperandTypeMismatch => "atomicrmw operand type does not match operation",
            Self::SwitchOperandTypeMismatch => "switch operand types disagree",
            Self::IndirectBrNonPointerAddress => "indirectbr address operand is not a pointer",
            Self::UseBeforeDef => "instruction does not dominate all uses",
        };
        f.write_str(s)
    }
}

/// Crate-wide error.
///
/// Variants are added incrementally as new subsystems land. Marked
/// `#[non_exhaustive]` so future additions are non-breaking.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IrError {
    /// Integer width outside `[`[`MIN_INT_BITS`]`, `[`MAX_INT_BITS`]`]`.
    ///
    /// Mirrors LLVM's `IntegerType::MIN_INT_BITS` / `MAX_INT_BITS`
    /// (`DerivedTypes.h`).
    ///
    /// [`MIN_INT_BITS`]: crate::MIN_INT_BITS
    /// [`MAX_INT_BITS`]: crate::MAX_INT_BITS
    #[error("integer width {bits} out of range [1, 1<<23]")]
    InvalidIntegerWidth { bits: u32 },

    /// A type was passed where a different kind was expected.
    #[error("type mismatch: expected {expected}, got {got}")]
    TypeMismatch {
        expected: TypeKindLabel,
        got: TypeKindLabel,
    },

    /// Two integer or vector operands have differing element widths or
    /// vector lengths.
    #[error("operand widths differ: lhs={lhs} rhs={rhs}")]
    OperandWidthMismatch { lhs: u32, rhs: u32 },

    /// `set_struct_body` called twice on the same named struct.
    #[error("named struct {name:?} already has a body")]
    StructBodyAlreadySet { name: String },

    /// An operation that requires a sized type was passed a type that has
    /// no statically-known size (e.g. `function`, `label`, opaque struct).
    #[error("cannot allocate value of unsized type {kind}")]
    UnsizedType { kind: TypeKindLabel },

    /// A value with the wrong category was passed where a specific kind was
    /// expected (e.g. an instruction handed to an API that needs a constant).
    #[error("value category mismatch: expected {expected}, got {got}")]
    ValueCategoryMismatch {
        expected: ValueCategoryLabel,
        got: ValueCategoryLabel,
    },

    /// A function operation referenced a parameter slot that does not exist.
    #[error("function argument index {index} out of range (have {count})")]
    ArgumentIndexOutOfRange { index: u32, count: u32 },

    /// `Module::add_function` saw a name already bound at module scope.
    #[error("a function named {name:?} already exists in this module")]
    DuplicateFunctionName { name: String },

    /// `IRBuilder::build_*` was asked to use a value that does not belong
    /// to the builder's module. The lifetime brand catches this for short-
    /// lived borrows; this variant covers the rare cases where a runtime
    /// check is needed (e.g. mixing `'static` constants).
    #[error("value does not belong to this module")]
    ForeignValue,

    /// `IRBuilder::build_ret` was given a value whose type does not
    /// match the function's declared return type.
    #[error("return type mismatch: function returns {expected}, got {got}")]
    ReturnTypeMismatch {
        expected: TypeKindLabel,
        got: TypeKindLabel,
    },

    /// An immediate value does not fit in the destination integer type.
    #[error("immediate {value} does not fit in {bits} bits")]
    ImmediateOverflow { value: u128, bits: u32 },
    /// A builder method was called with arguments that violate
    /// LangRef invariants the type system can't catch (e.g. `exact`
    /// flag on `add`, non-power-of-two alignment).
    #[error("invalid operation: {message}")]
    InvalidOperation { message: &'static str },
    /// IR validation failure detected by [`Module::verify`](crate::Module::verify) /
    /// [`Module::verify_borrowed`](crate::Module::verify_borrowed). The
    /// `rule` discriminator names the LangRef invariant that was
    /// violated; `function` / `block` carry diagnostic context, and
    /// `message` is a human-readable description that mirrors the
    /// shape of `Verifier::CheckFailed` output in
    /// `llvm/lib/IR/Verifier.cpp`.
    #[error("verifier: {rule}: {message}")]
    VerifierFailure {
        rule: VerifierRule,
        function: Option<String>,
        block: Option<String>,
        message: String,
    },
}

/// Crate-wide `Result` alias.
pub type IrResult<T> = core::result::Result<T, IrError>;
