//! Crate-wide error type.
//!
//! Per AGENTS.md and the IR foundation plan, every fallible IR API returns
//! [`IrResult`] (an alias for `Result<T, IrError>`). Pure constructors
//! (e.g. `Module::i32_type`) stay infallible; validation constructors and
//! all builder methods funnel through this enum.
//!
//! Variants are added phase-by-phase as new failure modes appear. Where the
//! `B: ModuleBrand` type parameter catches a class of bugs at compile time
//! (e.g. mixing handles from two modules carrying *distinct* brand types),
//! the corresponding runtime variant is deliberately *not* present here.
//! The brand is a backstop, not a guarantee, for the rungs where it cannot
//! separate two modules — two modules sharing a brand type (`DynBrand`, or a
//! named brand reused after the first was dropped) fall back to the runtime
//! `ModuleId` tag, which is what [`IrError::ForeignValueId`] reports.

#![deny(missing_docs)]

use core::fmt;

/// Human-readable label for a [`Type`](crate::Type) kind, embedded in
/// diagnostics that don't want to carry a borrowed type handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TypeKindLabel {
    /// The `void` type.
    Void,
    /// The 16-bit IEEE half-precision float (`half`).
    Half,
    /// The 16-bit brain-float (`bfloat`).
    Bfloat,
    /// The 32-bit IEEE single-precision float (`float`).
    Float,
    /// The 64-bit IEEE double-precision float (`double`).
    Double,
    /// The 80-bit x87 extended-precision float (`x86_fp80`).
    X86Fp80,
    /// The 128-bit IEEE quad-precision float (`fp128`).
    Fp128,
    /// The 128-bit PowerPC double-double float (`ppc_fp128`).
    PpcFp128,
    /// A basic-block `label` type.
    Label,
    /// The `metadata` type.
    Metadata,
    /// The `token` type.
    Token,
    /// The x86 AMX tile type (`x86_amx`).
    X86Amx,
    /// The WebAssembly exception-reference type (`exnref`).
    WasmExnRef,
    /// An arbitrary-width integer type (`iN`).
    Integer,
    /// A function type.
    Function,
    /// A pointer type.
    Pointer,
    /// A struct type.
    Struct,
    /// An array type.
    Array,
    /// A fixed-length vector type.
    FixedVector,
    /// A scalable vector type (`<vscale x N x T>`).
    ScalableVector,
    /// A typed (non-opaque) pointer type.
    TypedPointer,
    /// A target extension type (`target("...")`).
    TargetExt,
}

impl fmt::Display for TypeKindLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Lowercase forms match LLVM's IR textual syntax where applicable.
        let s = match self {
            TypeKindLabel::Void => "void",
            TypeKindLabel::Half => "half",
            TypeKindLabel::Bfloat => "bfloat",
            TypeKindLabel::Float => "float",
            TypeKindLabel::Double => "double",
            TypeKindLabel::X86Fp80 => "x86_fp80",
            TypeKindLabel::Fp128 => "fp128",
            TypeKindLabel::PpcFp128 => "ppc_fp128",
            TypeKindLabel::Label => "label",
            TypeKindLabel::Metadata => "metadata",
            TypeKindLabel::Token => "token",
            TypeKindLabel::X86Amx => "x86_amx",
            TypeKindLabel::WasmExnRef => "exnref",
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
    /// A constant value.
    Constant,
    /// A function argument.
    Argument,
    /// A basic block referenced as a value.
    BasicBlock,
    /// A function value.
    Function,
    /// An instruction result.
    Instruction,
    /// A global variable.
    GlobalVariable,
    /// A global alias.
    GlobalAlias,
    /// A global indirect function (`ifunc`).
    GlobalIfunc,
    /// Metadata wrapped as a value.
    MetadataAsValue,
    /// An inline-assembly value.
    InlineAsm,
}

impl fmt::Display for ValueCategoryLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ValueCategoryLabel::Constant => "constant",
            ValueCategoryLabel::Argument => "argument",
            ValueCategoryLabel::BasicBlock => "basic-block",
            ValueCategoryLabel::Function => "function",
            ValueCategoryLabel::Instruction => "instruction",
            ValueCategoryLabel::GlobalVariable => "global-variable",
            ValueCategoryLabel::GlobalAlias => "global-alias",
            ValueCategoryLabel::GlobalIfunc => "global-ifunc",
            ValueCategoryLabel::MetadataAsValue => "metadata-as-value",
            ValueCategoryLabel::InlineAsm => "inline-asm",
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
    /// `phi` result type is not a valid first-class *data* type. Accepted: int,
    /// float, pointer (the opaque `ptr` **and** the legacy typed `i32*`), vector,
    /// array, and non-opaque struct — the exact set the `.ll` parser's
    /// `parse_phi` accepts, so parser and verifier cannot drift.
    ///
    /// **Stricter than upstream.** `Verifier::visitPHINode` spells out only "PHI
    /// nodes cannot have token type"; llvmkit additionally rejects `label`,
    /// `metadata`, `x86_amx`, target-extension types, opaque structs, `void`, and
    /// function types as phi results. All but `void`/function are
    /// `Type::is_first_class`, so that predicate alone is *not* a sufficient
    /// gate — hence the explicit enumeration. Defense in depth: the rule holds
    /// regardless of construction path, not just for parsed IR.
    PhiInvalidResultType,
    /// `phi` in a block reachable from entry carries **zero** incoming values.
    /// Such a phi prints as `%p = phi i32` with no `[ … ]` pairs — a form that
    /// has no legal textual round-trip.
    ///
    /// **Stricter than upstream.** `Verifier::visitPHINode` shares the same
    /// `0 == 0` gap: it only checks the incoming count against the predecessor
    /// count, so a zero-incoming phi in a zero-predecessor block passes. llvmkit
    /// rejects it because such a phi fails `LLParser::parsePHI` round-trip — a
    /// phi with no incomings cannot be re-parsed. Restricted to blocks
    /// **reachable from entry**: an unreachable block may legitimately have no
    /// predecessors, and llvmkit does not force its phis to carry incomings.
    /// Defense in depth: the rule holds however the zero-incoming phi arose, not
    /// just via the public mutation path (which now erases such phis).
    PhiEmptyInReachableBlock,
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
    /// `getelementptr` indices do not index into the source element type
    /// (out-of-range or non-i32 struct index, or walking past a
    /// non-aggregate). Mirrors `Verifier::visitGetElementPtrInst`
    /// ("Invalid indices for GEP pointer type!").
    GepInvalidIndices,
    /// `getelementptr` result type's scalar type is not a pointer. Mirrors
    /// the `PtrTy` half of `Verifier::visitGetElementPtrInst`'s
    /// `Check(PtrTy && GEP.getResultElementType() == ElTy, "GEP is not of
    /// right type for indices!")`; the `getResultElementType() == ElTy` half
    /// has no counterpart, since `GepInstData` stores no result element type
    /// (docs/divergences.md).
    GepNonPointerResult,
    /// A vector `getelementptr`'s result width disagrees with its vector base
    /// or with one of its vector indices. Mirrors
    /// `Verifier::visitGetElementPtrInst`'s "Vector GEP result width doesn't
    /// match operand's" and "Invalid GEP index vector width".
    GepVectorWidthMismatch,
    /// `alloca` allocated type is unsized (function/void/label/...).
    /// Mirrors `Verifier::visitAllocaInst`.
    AllocaUnsizedType,
    /// `alloca` num-elements operand is not an integer.
    /// Mirrors `Verifier::visitAllocaInst`.
    AllocaNonIntegerCount,
    /// `swifterror` alloca is not pointer-typed, or is an array allocation.
    /// Mirrors `Verifier::visitAllocaInst`.
    SwiftErrorAlloca,
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
    /// `fneg` operand or result is not floating-point, or result type
    /// does not match operand type. Mirrors `Verifier::visitFNeg`.
    FnegTypeMismatch,
    /// `freeze` result type differs from operand type. Mirrors
    /// `Verifier::visitFreeze` ("Freeze should produce its operand's
    /// type").
    FreezeTypeMismatch,
    /// `va_arg` source operand is not a pointer. Mirrors
    /// `Verifier::visitVAArgInst`.
    VaArgNonPointerOperand,
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
    /// `shufflevector` operands fail `ShuffleVectorInst::isValidOperands`, or
    /// — with no upstream counterpart — the recorded result type disagrees
    /// with the operands or the mask length. Mirrors
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
    AtomicRmwOperandTypeMismatch,
    /// `switch` condition is not integer-typed, or a case value type
    /// disagrees with the condition. Mirrors `Verifier::visitSwitchInst`.
    SwitchOperandTypeMismatch,
    /// `indirectbr` address operand is not a pointer. Mirrors
    /// `Verifier::visitIndirectBrInst`.
    IndirectBrNonPointerAddress,
    /// Global variable initializer type does not match the global's
    /// value type. Mirrors `Verifier::visitGlobalVariable`
    /// ("Global variable initializer type does not match global
    /// variable type!").
    GlobalInitializerTypeMismatch,
    /// Global variable initializer is unsized. Mirrors
    /// `Verifier::visitGlobalVariable` ("Global variable initializer
    /// must be sized").
    GlobalInitializerUnsized,
    /// `common`-linkage global has a non-zero initializer, is
    /// `constant`, or is in a comdat. Mirrors
    /// `Verifier::visitGlobalVariable` (`hasCommonLinkage` arm).
    CommonLinkageInvariantViolated,
    /// Global value type contains a scalable vector. Mirrors
    /// `Verifier::visitGlobalVariable` ("Globals cannot contain
    /// scalable types").
    GlobalScalableType,
    /// `ifunc` carries a linkage `GlobalIFunc::isValidLinkage` rejects.
    /// Mirrors `Verifier::visitGlobalIFunc`.
    ///
    /// This is a *verifier* rule because upstream's parser has none:
    /// `parseAliasOrIFunc` checks `isValidLinkage` for aliases only
    /// (`if (IsAlias && ...)`), so `@i = appending ifunc ...` parses and is
    /// caught later.
    IfuncInvalidLinkage,
    /// `!range` attached to an instruction kind other than load/call/invoke.
    /// Mirrors `Verifier::visitInstruction`.
    RangeMetadataInvalidAttachment,
    /// `!range` / `!absolute_symbol` operand list shape or integer bounds are invalid.
    /// Mirrors `Verifier::verifyRangeLikeMetadata`.
    RangeMetadataMalformed,
    /// Range bound integer types disagree with each other or with the value type.
    /// Mirrors `Verifier::verifyRangeLikeMetadata`.
    RangeMetadataTypeMismatch,
    /// Range intervals overlap. Mirrors `Verifier::verifyRangeLikeMetadata`.
    RangeMetadataOverlapping,
    /// Range intervals are not sorted. Mirrors `Verifier::verifyRangeLikeMetadata`.
    RangeMetadataOutOfOrder,
    /// Range intervals are adjacent and should be coalesced.
    /// Mirrors `Verifier::verifyRangeLikeMetadata`.
    RangeMetadataContiguous,

    /// A `llvm.module.flags` operand is not a three-operand metadata tuple.
    /// Mirrors `Verifier::visitModuleFlag` ("incorrect number of operands in
    /// module flag").
    ModuleFlagInvalidOperandCount,
    /// A module flag's behavior operand is not a constant integer, or is a
    /// constant outside the `ModFlagBehavior` range `1..=8`. Mirrors
    /// `Verifier::visitModuleFlag` (both "invalid behavior operand in module
    /// flag" messages, split by `Module::isValidModFlagBehavior`).
    ModuleFlagInvalidBehavior,
    /// A module flag's ID operand is not a metadata string. Mirrors
    /// `Verifier::visitModuleFlag` ("invalid ID operand in module flag
    /// (expected metadata string)").
    ModuleFlagInvalidId,
    /// A module flag's value operand does not satisfy its behavior's
    /// constraint: `min` needs a constant non-negative integer, `max` a
    /// constant integer, `require` a two-element metadata pair whose first
    /// operand is a string, `append`/`appendunique` a metadata node — plus
    /// the per-key constant-integer constraints on `wchar_size` and
    /// `SemanticInterposition`. Mirrors the behavior `switch` and per-key
    /// checks of `Verifier::visitModuleFlag`.
    ModuleFlagInvalidValue,
    /// Two non-`require` module flags share one ID. Mirrors
    /// `Verifier::visitModuleFlag` ("module flag identifiers must be unique
    /// (or of 'require' type)").
    ModuleFlagDuplicateId,
    /// A `require` module flag names a flag that is absent, or one whose
    /// value differs from the required value. Mirrors the requirement
    /// validation loop of `Verifier::visitModuleFlags`.
    ModuleFlagInvalidRequirement,
    /// Exactly one of the `aarch64-elf-pauthabi-platform` /
    /// `aarch64-elf-pauthabi-version` module flags is present. Mirrors
    /// `Verifier::visitModuleFlags`.
    ModuleFlagPauthAbiPairing,
    /// A `Linker Options` module flag without the `llvm.linker.options`
    /// named metadata the bitcode reader upgrades it to. Mirrors
    /// `Verifier::visitModuleFlag` ("'Linker Options' named metadata no
    /// longer supported").
    ModuleFlagLinkerOptionsUnsupported,
    /// A `CG Profile` entry is not a `(function, function, count)` triple:
    /// not a three-operand node, a non-function non-null callee/caller, or a
    /// non-integer count. Mirrors `Verifier::visitModuleFlagCGProfileEntry`.
    ModuleFlagCgProfileMalformed,

    /// In-block use-before-def: an operand whose defining instruction follows
    /// the use within the same basic block. Mirrors
    /// `Verifier::verifyDominatesUse`.
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
            Self::PhiInvalidResultType => {
                "PHI node result type is not a valid first-class data type"
            }
            Self::PhiEmptyInReachableBlock => {
                "PHI node in a block reachable from entry has no incoming values"
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
            Self::GepInvalidIndices => "getelementptr indices are invalid for the source type",
            Self::GepNonPointerResult => "getelementptr result type is not a pointer",
            Self::GepVectorWidthMismatch => {
                "vector getelementptr width does not match its operands"
            }
            Self::AllocaUnsizedType => "alloca allocated type is unsized",
            Self::AllocaNonIntegerCount => "alloca num-elements operand is not an integer",
            Self::SwiftErrorAlloca => "swifterror alloca must be a non-array pointer allocation",
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
            Self::FnegTypeMismatch => "fneg operand/result is not floating-point or types differ",
            Self::FreezeTypeMismatch => "freeze result type does not match operand type",
            Self::VaArgNonPointerOperand => "va_arg source operand is not a pointer",
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
            Self::AtomicRmwOperandTypeMismatch => "atomicrmw operand type does not match operation",
            Self::SwitchOperandTypeMismatch => "switch operand types disagree",
            Self::IndirectBrNonPointerAddress => "indirectbr address operand is not a pointer",
            Self::GlobalInitializerTypeMismatch => {
                "global variable initializer type does not match value type"
            }
            Self::GlobalInitializerUnsized => "global variable initializer must be sized",
            Self::CommonLinkageInvariantViolated => {
                "common-linkage global must have a zero initializer, must not be constant, and must not be in a comdat"
            }
            Self::GlobalScalableType => "globals cannot contain scalable types",
            Self::IfuncInvalidLinkage => {
                "IFunc should have private, internal, linkonce, weak, linkonce_odr, weak_odr, or external linkage!"
            }
            Self::RangeMetadataInvalidAttachment => {
                "range metadata is only valid on loads, calls, and invokes"
            }
            Self::RangeMetadataMalformed => "range-like metadata operand list is malformed",
            Self::RangeMetadataTypeMismatch => "range metadata bound types are invalid",
            Self::RangeMetadataOverlapping => "range intervals overlap",
            Self::RangeMetadataOutOfOrder => "range intervals are not in order",
            Self::RangeMetadataContiguous => "range intervals are contiguous",
            Self::ModuleFlagInvalidOperandCount => "incorrect number of operands in module flag",
            Self::ModuleFlagInvalidBehavior => "invalid behavior operand in module flag",
            Self::ModuleFlagInvalidId => {
                "invalid ID operand in module flag (expected metadata string)"
            }
            Self::ModuleFlagInvalidValue => {
                "module flag value does not satisfy its behavior's requirements"
            }
            Self::ModuleFlagDuplicateId => {
                "module flag identifiers must be unique (or of 'require' type)"
            }
            Self::ModuleFlagInvalidRequirement => "invalid requirement on module flag",
            Self::ModuleFlagPauthAbiPairing => {
                "either both or no 'aarch64-elf-pauthabi-platform' and 'aarch64-elf-pauthabi-version' module flags must be present"
            }
            Self::ModuleFlagLinkerOptionsUnsupported => {
                "'Linker Options' named metadata no longer supported"
            }
            Self::ModuleFlagCgProfileMalformed => "CG Profile module flag entry is malformed",
            Self::UseBeforeDef => "instruction does not dominate all uses",
        };
        f.write_str(s)
    }
}

/// Crate-wide error.
///
/// Variants are added incrementally as new subsystems land. Marked
/// `#[non_exhaustive]` so future additions are non-breaking.
///
/// `Hash` alongside `Eq` so an error can be de-duplicated: a verifier or a
/// pass driver that collects failures across a whole module wants a
/// `HashSet<IrError>`, not a `Vec` it has to scan. Every payload is a plain
/// `String`, `&'static str`, or integer, so the derive is total. The sibling
/// `llvmkit_asmparser::ParseError` already carried `Hash` for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
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
    InvalidIntegerWidth {
        /// The rejected integer bit width.
        bits: u32,
    },

    /// A type was passed where a different kind was expected.
    #[error("type mismatch: expected {expected}, got {got}")]
    TypeMismatch {
        /// The type kind the API required.
        expected: TypeKindLabel,
        /// The type kind actually supplied.
        got: TypeKindLabel,
    },

    /// Two integer or vector types that were required to agree have
    /// differing element widths or vector lengths.
    ///
    /// The field names are positional, not semantic — `lhs` / `rhs` are
    /// **not** reliably a left and a right operand. Most producers pass
    /// `lhs` = the width that was *expected* and `rhs` = the width actually
    /// *supplied*: `Type::require_match` (the crate's one type compare,
    /// reached from the fold and variable-def seams) and the per-marker
    /// `TryFrom<Value>` / constant narrowings all read expected/got. Only
    /// the genuinely symmetric peer-compares — `ConstantRange`'s
    /// lower/upper bounds and `KnownBits`' zero/one masks — pass two
    /// operands in left/right order. The names predate that split and are
    /// kept for API compatibility.
    #[error("operand widths differ: lhs={lhs} rhs={rhs}")]
    OperandWidthMismatch {
        /// First width: the *expected* element width (or vector length) at
        /// the expected/got producers; the left peer at the symmetric ones.
        lhs: u32,
        /// Second width: the element width (or vector length) actually
        /// *supplied* at the expected/got producers; the right peer at the
        /// symmetric ones.
        rhs: u32,
    },

    /// A [`ConstantRange`](crate::ConstantRange) was asked for with equal
    /// endpoints that name neither the empty set nor the full one.
    ///
    /// `[lower, upper)` encodes empty as `lower == upper == 0` and full as
    /// `lower == upper == max`. Any other equal pair is unrepresentable: it
    /// would describe a range containing nothing while claiming not to be
    /// empty. Upstream asserts on it in `ConstantRange::ConstantRange`; this
    /// crate has no runtime asserts in production paths, so the constructor
    /// rejects instead. Use
    /// [`ConstantRange::non_empty`](crate::ConstantRange::non_empty) to read
    /// an equal pair as the full set, which is what
    /// `ConstantRange::getNonEmpty` does.
    #[error(
        "constant range endpoints are equal at {value}, which is neither the minimum \
         (the empty set) nor the maximum (the full set) for {bit_width} bits"
    )]
    DegenerateConstantRange {
        /// The shared endpoint value, rendered unsigned.
        value: String,
        /// Width of the range's integer domain.
        bit_width: u32,
    },

    /// A statically-lengthed array handle (`ArrayValue<_, ArrLen<N>>`) was
    /// narrowed from an array of a different length. Distinct from
    /// [`OperandWidthMismatch`](Self::OperandWidthMismatch), whose `u32` fields
    /// fit integer/vector widths but not the `u64` length of an `ArrayType`.
    #[error("array length mismatch: expected [{expected} x _], got [{got} x _]")]
    ArrayLengthMismatch {
        /// The statically-required length `N`.
        expected: u64,
        /// The array's actual length.
        got: u64,
    },

    /// Two pointer types differ only in their address space.
    ///
    /// The pointer analogue of
    /// [`OperandWidthMismatch`](Self::OperandWidthMismatch), and it exists
    /// for the same reason: [`TypeKindLabel`] has a *single*, address-space-less
    /// `Pointer` variant, so an `addrspace(0)`-vs-`addrspace(1)` drift would
    /// otherwise report `TypeMismatch { expected: Pointer, got: Pointer }` —
    /// true, and silent about the only fact that distinguishes the two. An
    /// address space is not a width, so `OperandWidthMismatch` cannot stand in
    /// here; the drift needs fields that say what they mean. Mirrors the
    /// distinction `LLParser::parseType` draws when it spells the full
    /// `ptr addrspace(N)` in its diagnostics rather than the bare kind.
    #[error("pointer address space mismatch: expected addrspace({expected}), got addrspace({got})")]
    AddressSpaceMismatch {
        /// Address space the API required.
        expected: u32,
        /// Address space actually supplied.
        got: u32,
    },

    /// `set_struct_body_dyn` called twice on the same named struct.
    #[error("named struct {name:?} already has a body")]
    StructBodyAlreadySet {
        /// Name of the named struct that already has a body.
        name: String,
    },

    /// An identified struct's body reaches the struct being defined. Port of
    /// `StructType::checkBody` (`lib/IR/Type.cpp`), whose message this
    /// reproduces verbatim so `LLParser::parseStructDefinition` can hand it
    /// straight to `tokError`. An anonymous identified struct has no name, and
    /// upstream's `getName()` renders empty there too.
    #[error("identified structure type '{name}' is recursive")]
    RecursiveStructBody {
        /// Name of the identified struct being defined; empty if anonymous.
        name: String,
    },

    /// A struct schema found an existing named struct with a different body.
    #[error("named struct {name:?} has a different body")]
    StructBodyMismatch {
        /// Name of the named struct whose existing body differs.
        name: String,
    },

    /// An operation that requires a sized type was passed a type that has
    /// no statically-known size (e.g. `function`, `label`, opaque struct).
    #[error("cannot allocate value of unsized type {kind}")]
    UnsizedType {
        /// Kind of the unsized type that was rejected.
        kind: TypeKindLabel,
    },

    /// A value with the wrong category was passed where a specific kind was
    /// expected (e.g. an instruction handed to an API that needs a constant).
    #[error("value category mismatch: expected {expected}, got {got}")]
    ValueCategoryMismatch {
        /// The value category the API required.
        expected: ValueCategoryLabel,
        /// The value category actually supplied.
        got: ValueCategoryLabel,
    },

    /// A function operation referenced a parameter slot that does not exist.
    #[error("function argument index {index} out of range (have {count})")]
    ArgumentIndexOutOfRange {
        /// The out-of-range parameter slot that was requested.
        index: u32,
        /// The number of parameters the function actually has.
        count: u32,
    },

    /// `extractvalue` / `insertvalue` indexed past the end of an array or
    /// struct's element list. Mirrors `ExtractValueInst::getIndexedType`
    /// (`lib/IR/Instructions.cpp`), which rejects out-of-range indices
    /// rather than clamping them.
    #[error("aggregate index {index} out of range (have {count})")]
    AggregateIndexOutOfRange {
        /// The out-of-range aggregate index that was requested.
        index: u32,
        /// The number of elements in the indexed array or struct.
        count: u64,
    },

    /// A `getelementptr` index sequence does not index into the source
    /// element type: a struct index that is not a constant `i32` in range,
    /// or an index that walks past a non-aggregate. Mirrors
    /// `GetElementPtrInst::getIndexedType` returning null (`LLParser`
    /// "invalid getelementptr indices").
    #[error("invalid getelementptr indices for the source type")]
    GepInvalidIndices,

    /// A typed function facade was requested with a parameter tuple whose arity
    /// does not match the function signature.
    #[error("function parameter count mismatch: expected {expected}, got {got}")]
    FunctionParameterCountMismatch {
        /// The arity the function signature declares.
        expected: u32,
        /// The arity of the parameter tuple that was supplied.
        got: u32,
    },

    /// A call/invoke/callbr site passed a wrong number of arguments for its
    /// callee's signature. Mirrors the `CallInst::init` /
    /// `CallBrInst::init` `NDEBUG` assertion ("Calling a function with a
    /// bad signature!", `lib/IR/Instructions.cpp`) and
    /// `Verifier::visitCallBase`'s authoritative arity check: a
    /// non-vararg callee requires an exact match, a vararg callee
    /// requires at least as many arguments as declared parameters.
    #[error("call argument count mismatch: expected {expected}, got {got}")]
    CallArgumentCountMismatch {
        /// The parameter count the callee's signature declares.
        expected: u32,
        /// The number of arguments actually passed.
        got: u32,
    },

    /// A call/invoke/callbr site passed an argument whose type does not
    /// exactly match the callee's parameter type at that position.
    /// Mirrors the same `CallInst::init` assertion and
    /// `Verifier::visitCallBase`'s per-argument type check. Both sides
    /// carry the IR-textual rendering of the type (`Type`'s `Display`,
    /// e.g. `i32`), not just its kind, so same-kind mismatches such as
    /// i8-vs-i32 stay distinguishable — the way `LLParser::parseCall`'s
    /// "argument is not of expected type 'i32'" spells the full type.
    #[error("call argument #{index} type mismatch: expected {expected}, got {got}")]
    CallArgumentTypeMismatch {
        /// Zero-based position of the mismatched argument.
        index: u32,
        /// IR-textual rendering of the callee's parameter type at `index`.
        expected: String,
        /// IR-textual rendering of the argument type actually passed.
        got: String,
    },

    /// A typed function facade ([`crate::TypedFunctionValue`]) was requested
    /// to wrap a raw function whose signature is variadic. The fixed-arity
    /// facade cannot represent a `...` tail; use
    /// [`crate::function_signature::TypedVarArgsFunctionValue`] instead.
    #[error("typed function facade does not accept a variadic signature")]
    UnexpectedVarArgsSignature,

    /// A varargs typed function facade
    /// ([`crate::function_signature::TypedVarArgsFunctionValue`]) was
    /// requested to wrap a raw function whose signature is not variadic.
    #[error("varargs typed function facade requires a variadic signature")]
    MissingVarArgsSignature,

    /// A function-declaring constructor (`add_typed_function`,
    /// `add_function_dyn`, …) saw a name already bound at module scope.
    #[error("a function named {name:?} already exists in this module")]
    DuplicateFunctionName {
        /// The function name that is already bound at module scope.
        name: String,
    },

    /// Installing a global-scope symbol (global variable, alias, or
    /// ifunc) saw a name already bound at module scope. Global variables,
    /// aliases, and ifuncs share the module's global-symbol namespace, so
    /// this one variant covers all three.
    #[error("a global named {name:?} already exists in this module")]
    DuplicateGlobalName {
        /// The global-scope name that is already bound at module scope.
        name: String,
    },

    /// A reserved `llvm.*` name is absent from the generated LLVM intrinsic table.
    #[error("unknown intrinsic `{name}`")]
    UnknownIntrinsic {
        /// The `llvm.*` name absent from the generated intrinsic table.
        name: String,
    },

    /// Ordinary function construction attempted to use a generated intrinsic name.
    #[error("intrinsic name `{name}` is reserved; use Module::get_or_insert_intrinsic_declaration")]
    ReservedIntrinsicName {
        /// The reserved intrinsic name that ordinary construction attempted.
        name: String,
    },

    /// A known intrinsic's name, overload suffix, or function signature is invalid.
    #[error("intrinsic `{name}` signature mismatch")]
    IntrinsicSignatureMismatch {
        /// Name of the intrinsic whose signature could not be satisfied.
        name: String,
    },

    /// `IrBuilder::ret` was given a value whose type does not
    /// match the function's declared return type.
    #[error("return type mismatch: function returns {expected}, got {got}")]
    ReturnTypeMismatch {
        /// The type kind the function is declared to return.
        expected: TypeKindLabel,
        /// The type kind of the value passed to `ret`.
        got: TypeKindLabel,
    },

    /// An immediate value does not fit in the destination integer type.
    #[error("immediate {value} does not fit in {bits} bits")]
    ImmediateOverflow {
        /// The immediate value that does not fit.
        value: u128,
        /// Bit width of the destination integer type.
        bits: u32,
    },
    /// A builder method was called with arguments that violate
    /// LangRef invariants the type system can't catch (e.g. `exact`
    /// flag on `add`, non-power-of-two alignment).
    #[error("invalid operation: {message}")]
    InvalidOperation {
        /// Human-readable description of the violated LangRef invariant.
        message: &'static str,
    },
    /// `target datalayout = "..."` directive could not be parsed.
    /// Mirrors the `Error` returns of
    /// `lib/IR/DataLayout.cpp::DataLayout::parseLayoutString`.
    #[error("invalid datalayout: {reason}")]
    InvalidDataLayout {
        /// Why the `target datalayout` string could not be parsed.
        reason: String,
    },
    /// A `.ll` keyword did not name any variant of the enum it was parsed
    /// into — the error of the [`FromStr`](core::str::FromStr) family
    /// ([`Linkage`](crate::Linkage), [`Visibility`](crate::Visibility),
    /// [`AtomicOrdering`](crate::AtomicOrdering),
    /// [`CallingConv`](crate::CallingConv), …).
    ///
    /// `target` names the type that rejected it, so one message serves the
    /// whole family without each enum inventing its own error. Mirrors the
    /// shape of `LLParser`'s `error(Loc, "invalid ... keyword")` diagnostics —
    /// upstream's parser reaches the same dead end from its generated keyword
    /// tables.
    #[error("invalid {target} keyword '{keyword}'")]
    InvalidKeyword {
        /// The type that rejected the keyword, e.g. `"linkage"`.
        target: &'static str,
        /// The unrecognized keyword text.
        keyword: String,
    },
    /// A raw numeric discriminant did not name any variant of the enum it was
    /// converted into — the error of the [`TryFrom`] family that pairs with
    /// each enum's `from_raw` const constructor
    /// ([`IntPredicate`](crate::IntPredicate),
    /// [`FloatPredicate`](crate::FloatPredicate)). `from_raw` stays the
    /// `const fn` path and returns [`Option`]; `TryFrom` is the `?`-friendly
    /// one and says which value was rejected.
    #[error("invalid {target} discriminant {value}")]
    InvalidDiscriminant {
        /// The type that rejected the value, e.g. `"icmp predicate"`.
        target: &'static str,
        /// The rejected raw discriminant, widened to the largest raw width in
        /// the family so one variant serves them all.
        value: u64,
    },
    /// A textual optimization level did not match LLVM's built-in aliases.
    #[error("invalid optimization level '{level}'")]
    InvalidOptimizationLevel {
        /// The unrecognized optimization-level text.
        level: String,
    },
    /// A textual pass or pipeline name contains invalid syntax.
    #[error("invalid pass pipeline name '{name}'")]
    InvalidPassPipelineName {
        /// The pass or pipeline name containing invalid syntax.
        name: String,
    },
    /// A textual pass pipeline has invalid delimiter nesting.
    #[error("invalid pass pipeline '{pipeline}'")]
    InvalidPassPipeline {
        /// The pipeline string with invalid delimiter nesting.
        pipeline: String,
    },
    /// An analysis result was requested before its analysis pass was registered.
    #[error("analysis {name} is not registered")]
    AnalysisNotRegistered {
        /// Type name of the analysis that was never registered.
        name: &'static str,
    },
    /// An invalidator asked for a cached analysis result that is absent.
    #[error("analysis {name} is not cached")]
    AnalysisNotCached {
        /// Type name of the analysis whose cached result is absent.
        name: &'static str,
    },
    /// IR validation failure detected by [`Module::verify`](crate::Module::verify) /
    /// [`Module::verify_borrowed`](crate::Module::verify_borrowed). The
    /// `rule` discriminator names the LangRef invariant that was
    /// violated; `function` / `block` carry diagnostic context, and
    /// `message` is a human-readable description that mirrors the
    /// shape of `Verifier::CheckFailed` output in
    /// `llvm/lib/IR/Verifier.cpp`.
    #[error("verifier: {rule}: {message}")]
    VerifierFailure {
        /// The LangRef invariant that was violated.
        rule: VerifierRule,
        /// Name of the function under verification, if known.
        function: Option<String>,
        /// Name of the offending basic block, if known.
        block: Option<String>,
        /// Human-readable description mirroring `Verifier::CheckFailed`.
        message: String,
    },

    /// [`crate::SsaBuilder`] read a strict (non-poison) variable on a path
    /// that reaches function entry without a preceding write. Mirrors the
    /// "use of undefined value" outcome of Braun et al. 2013's on-the-fly
    /// SSA construction when the caller declared the variable without
    /// opting into poison-on-undef.
    #[error("use of undefined SSA variable {variable:?} in block {block:?}")]
    SsaUseOfUndefinedVariable {
        /// Name of the variable read on a path with no preceding write.
        variable: String,
        /// Name of the block where the undefined read occurred.
        block: String,
    },

    /// One of [`crate::SsaBuilder`]'s terminator methods (`br` / `cond_br`
    /// / `switch`) recorded an incoming edge against a destination block
    /// that was already sealed at the time the edge was added. Braun's
    /// algorithm requires every predecessor edge to be recorded before
    /// the block is sealed.
    #[error("branch to already-sealed SSA block {block:?}")]
    SsaBranchToSealedBlock {
        /// Name of the destination block that was already sealed.
        block: String,
    },

    /// [`crate::SsaBuilder::seal_block`] was called twice on the same
    /// block.
    #[error("SSA block {block:?} is already sealed")]
    SsaBlockAlreadySealed {
        /// Name of the block that was sealed a second time.
        block: String,
    },

    /// An [`crate::SsaBuilder`] operation required a block that has not
    /// yet received its terminator (still open for phi head-insertion or
    /// further construction) but found one whose insertion capability was
    /// already consumed by a terminator.
    #[error("SSA block {block:?} is already filled (terminated)")]
    SsaBlockAlreadyFilled {
        /// Name of the block whose insertion capability was already consumed.
        block: String,
    },

    /// [`crate::SsaBuilder`] required a block to be filled (terminated)
    /// before proceeding, but the block has no terminator yet.
    #[error("SSA block {block:?} is not yet filled (unterminated)")]
    SsaUnfilledBlock {
        /// Name of the block that still lacks a terminator.
        block: String,
    },

    /// An [`crate::ssa_builder::IntVariable`] / `FloatVariable` /
    /// `PointerVariable` handle was used against a different
    /// [`crate::SsaBuilder`] than the one that declared it.
    #[error("SSA variable belongs to a different SsaBuilder")]
    SsaForeignVariable,

    /// An [`crate::ssa_builder::SsaBlock`] handle was used against a
    /// different [`crate::SsaBuilder`] than the one that created it.
    #[error("SSA block belongs to a different SsaBuilder")]
    SsaForeignBlock,

    /// A storable value id (e.g. [`crate::IntValueId`]) was resolved against a
    /// different [`Module`](crate::Module) than the one that minted it. The
    /// id's module tag did not match the target module, so it cannot name a
    /// value there. Raised by the fallible operand conversions
    /// ([`IntoIntValue`](crate::IntoIntValue) /
    /// [`IntoFloatValue`](crate::IntoFloatValue) /
    /// [`IntoPointerValue`](crate::IntoPointerValue)) when handed a foreign id.
    #[error("value id belongs to a different Module")]
    ForeignValueId,

    /// A [`MetadataId`](crate::MetadataId) named nothing in the target
    /// [`Module`](crate::Module) — the id's tag matched, but its slot is past
    /// the end of the arena.
    ///
    /// Raised by [`Module::metadata_set`](crate::Module::metadata_set) and
    /// [`Module::metadata_as_value`](crate::Module::metadata_as_value). A
    /// *foreign* id is [`ForeignMetadataId`](Self::ForeignMetadataId) instead:
    /// the tag separates the two cases, so an in-range slot from another module
    /// is rejected rather than silently mis-resolved.
    #[error("metadata slot {index} names nothing in this Module (holds {len})")]
    UnknownMetadataSlot {
        /// The index that was out of range.
        index: usize,
        /// How many entries the target store actually holds.
        len: usize,
    },

    /// A [`MetadataId`](crate::MetadataId) was used against a different
    /// [`Module`](crate::Module) than the one that minted it. The id's module
    /// tag did not match the target module, so it cannot name a node there.
    ///
    /// The metadata twin of [`ForeignValueId`](Self::ForeignValueId), raised by
    /// every module-level metadata API that accepts an id
    /// ([`Module::metadata_tuple`](crate::Module::metadata_tuple),
    /// [`metadata_node`](crate::Module::metadata_node),
    /// [`metadata_set`](crate::Module::metadata_set),
    /// [`named_metadata_add_operand`](crate::Module::named_metadata_add_operand),
    /// …) and by the attachment setters
    /// ([`InstructionView::set_metadata`](crate::InstructionView::set_metadata),
    /// [`push_debug_record`](crate::InstructionView::push_debug_record), and
    /// the `FunctionValue` / global siblings).
    #[error("metadata id belongs to a different Module")]
    ForeignMetadataId,

    /// A [`NamedMetadataId`](crate::NamedMetadataId) was used against a
    /// different [`Module`](crate::Module) than the one that minted it. The
    /// id's module tag did not match the target module, so it cannot name a
    /// node there.
    ///
    /// The named-metadata twin of
    /// [`ForeignMetadataId`](Self::ForeignMetadataId), raised by
    /// [`Module::named_metadata_add_operand`](crate::Module::named_metadata_add_operand)
    /// when the *id* (rather than the operand) is foreign. The lookup,
    /// [`Module::named_metadata_get`](crate::Module::named_metadata_get),
    /// returns `None` for a foreign id instead.
    #[error("named metadata id belongs to a different Module")]
    ForeignNamedMetadataId,

    /// [`crate::SsaState::for_function`] was given a function that
    /// already has a body. The layer must observe every CFG edge from
    /// birth (Braun's algorithm needs to see every `br` as it is
    /// recorded), so grafting onto a partially-built function is
    /// rejected.
    #[error("SsaBuilder requires a function with no existing basic blocks")]
    SsaFunctionHasBlocks,

    /// A [`crate::SsaBuilder`] was minted over an [`crate::SsaState`]
    /// that was opened for a *different* function. The state carries
    /// Braun bookkeeping keyed to one function's blocks, so pairing it
    /// with another would append blocks nowhere near the recorded edges.
    #[error("SsaState was opened for a different function")]
    SsaForeignFunction,

    /// An [`crate::SsaBuilder`] operation that needs an insertion point
    /// was called while the cursor was empty — either before the first
    /// [`switch_to_block`](crate::SsaBuilder::switch_to_block) or after a
    /// terminator cleared it.
    ///
    /// This is the *runtime rendering of a static law*: before llvmkit
    /// 2.0 cycle D the SSA layer carried an `Unpositioned`/`Positioned`
    /// type-state, so this case was an `E0599`. It became a runtime error
    /// on the on-the-fly SSA layer only (the crate's `_dyn` convention),
    /// because that layer's whole purpose is authoring a CFG discovered
    /// at run time — see `ssa_builder.rs`'s module docs. The plain
    /// [`IrBuilder`](crate::IrBuilder) keeps its static positioning
    /// type-state untouched.
    #[error("SsaBuilder has no current block; call switch_to_block first")]
    SsaUnpositioned,

    /// A phi already has an entry for this predecessor block with a
    /// different value; a second, differing entry is meaningless in any
    /// CFG. Same-block same-value duplicates are legal (multi-edges from
    /// `switch`). Mirrors the class of upstream bug llvm/llvm-project#196954,
    /// enforced at the edge-add call site rather than deferred to
    /// [`Module::verify`](crate::Module::verify)'s `AmbiguousPhi` rule.
    #[error("phi already has an entry for block %{block} with a different value")]
    AmbiguousPhiIncoming {
        /// Printed name of the predecessor block.
        block: String,
    },

    /// A block-argument branch supplied a number of arguments that does not
    /// match the target block's parameter (leading-phi) count. The branch
    /// carries exactly one value per target parameter; a differing count is
    /// rejected at the branch builder rather than deferred to
    /// [`Module::verify`](crate::Module::verify).
    #[error("phi argument arity mismatch: target expects {expected}, got {got}")]
    PhiArgArityMismatch {
        /// Number of parameters (leading head-phis) the target block declares.
        expected: usize,
        /// Number of arguments the branch supplied.
        got: usize,
    },

    /// A phi inserted through
    /// [`FnReshape::insert_phi`](crate::FnReshape::insert_phi) names an incoming
    /// value that does not dominate the CFG edge it flows in on: the value is
    /// defined in `value_block`, which does not dominate the predecessor
    /// `pred_block` the incoming enters from. Because a `ReshapeCfg` pass sees a
    /// complete CFG, this SSA-dominance obligation is witnessed at the insertion
    /// call (against the repaired dominator tree) rather than deferred to
    /// [`Module::verify`](crate::Module::verify)'s `verifyDominatesUse` rule.
    #[error(
        "phi incoming value defined in %{value_block} does not dominate its edge from predecessor %{pred_block}"
    )]
    PhiIncomingNotDominating {
        /// Printed name of the block that defines the offending incoming value.
        value_block: String,
        /// Printed name of the predecessor block the incoming edge enters from.
        pred_block: String,
    },

    /// A phi failed the shared per-phi coherence check (`check_phi_incoming`)
    /// during pass-side insertion
    /// ([`FnReshape::insert_phi`](crate::FnReshape::insert_phi)):
    /// its incoming set is incomplete, names a non-predecessor, over-counts a
    /// predecessor, carries a differing-value duplicate, or has a
    /// type-mismatched incoming. The rendered `message` is produced by the same
    /// renderer the `.ll` parser uses for its coherence diagnostics, so the two
    /// cannot drift.
    #[error("{message}")]
    PhiCoherence {
        /// Rendered coherence-failure description.
        message: String,
    },

    /// [`Module::branded`](crate::Module::branded) /
    /// [`branded_once`](crate::Module::branded_once) was asked for a brand type
    /// that a **live** module already holds. At most one module may carry a
    /// given brand at a time, which is what lets the brand stand in for module
    /// identity at compile time. Drop the incumbent module to free the brand,
    /// pick a different brand type, or use
    /// [`Module::dynamic`](crate::Module::dynamic) when the module count is not
    /// statically known.
    ///
    /// Note that leaking a module (e.g. [`core::mem::forget`]) never releases
    /// its brand — see [`Module::branded`](crate::Module::branded).
    #[error("module brand `{brand}` is already held by a live module")]
    BrandInUse {
        /// Rendered name of the brand type, from [`core::any::type_name`].
        brand: &'static str,
    },

    /// A brand retired by [`Module::branded_once`](crate::Module::branded_once)
    /// was claimed again. Retirement is permanent by design: a brand whose
    /// module is gone must never name a *successor*, or handles minted from two
    /// different generations of storage would share one static type.
    #[error("module brand `{brand}` was permanently retired by a `branded_once` module")]
    BrandRetired {
        /// Rendered name of the brand type, from [`core::any::type_name`].
        brand: &'static str,
    },
}

/// Crate-wide `Result` alias.
pub type IrResult<T> = core::result::Result<T, IrError>;
