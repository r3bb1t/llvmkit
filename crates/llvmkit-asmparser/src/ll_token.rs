//! Token type and supporting enums for the `.ll` lexer.
//!
//! The shape mirrors `llvm::lltok::Kind` from
//! `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/AsmParser/LLToken.h`,
//! split into a few smaller enums for readability:
//! * [`Token`] — the discriminator the parser drives on, with payload-bearing
//!   variants holding borrowed slices into the source buffer (or owned bytes
//!   when string-style escapes had to be decoded).
//! * [`Keyword`] — every "plain" `kw_*` (everything that isn't an instruction
//!   opcode, an integer type width, or a primitive type).
//! * [`Opcode`] — instruction-opcode keywords (the parser pairs these with
//!   `llvm::Instruction::Opcode`).
//! * [`PrimitiveTy`] — type-keyword tokens that resolve to a stateless
//!   primitive type (`void`, `i32`, `ptr`, …).

use std::borrow::Cow;
use std::num::NonZeroU32;

/// A single lexical token. `'src` ties non-`'static` payloads to the source
/// buffer they were borrowed from.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Token<'src> {
    // ── Markers ──
    /// End of input.
    Eof,

    // ── Punctuation ──
    /// `...`
    DotDotDot,
    /// `=`
    Equal,
    /// `,`
    Comma,
    /// `*`
    Star,
    /// `[`
    LSquare,
    /// `]`
    RSquare,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `<`
    Less,
    /// `>`
    Greater,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `!` standing alone (the `!foo` form is [`Token::MetadataVar`]).
    Exclaim,
    /// `|`
    Bar,
    /// `:`
    Colon,
    /// `#` standing alone (`#42` is [`Token::AttrGrpId`]).
    Hash,

    // ── Identifier-prefix tokens ──
    // Quoted forms support `\xx` escapes which can decode into non-ASCII bytes;
    // unquoted forms are pure ASCII. We use `Cow` so escape-free inputs stay
    // borrowed and only allocate when decoding actually shortens the slice.
    /// `foo:` or `"quoted":`
    LabelStr(Cow<'src, [u8]>),
    /// `@foo` or `@"foo"`
    GlobalVar(Cow<'src, [u8]>),
    /// `@42`
    GlobalId(u32),
    /// `%foo` or `%"foo"`
    LocalVar(Cow<'src, [u8]>),
    /// `%42`
    LocalVarId(u32),
    /// `$foo` or `$"foo"`
    ComdatVar(Cow<'src, [u8]>),
    /// `!foo` (also accepts `\xx` escapes).
    MetadataVar(Cow<'src, [u8]>),
    /// `"..."` string constant.
    StringConstant(Cow<'src, [u8]>),
    /// `#42`
    AttrGrpId(u32),
    /// `^42`
    SummaryId(u32),

    // ── DWARF / DI prefix tokens ──
    /// `DW_TAG_*` — payload is the full keyword (`DW_TAG_subprogram`).
    DwarfTag(&'src str),
    /// `DW_ATE_*`
    DwarfAttEncoding(&'src str),
    /// `DW_VIRTUALITY_*`
    DwarfVirtuality(&'src str),
    /// `DW_LANG_*`
    DwarfLang(&'src str),
    /// `DW_LNAME_*`
    DwarfSourceLangName(&'src str),
    /// `DW_CC_*`
    DwarfCC(&'src str),
    /// `DW_OP_*`
    DwarfOp(&'src str),
    /// `DW_MACINFO_*`
    DwarfMacinfo(&'src str),
    /// `DW_APPLE_ENUM_KIND_*`
    DwarfEnumKind(&'src str),
    /// `DIFlag*`
    DiFlag(&'src str),
    /// `DISPFlag*`
    DiSpFlag(&'src str),
    /// `CSK_*`
    ChecksumKind(&'src str),
    /// `NoDebug` / `FullDebug` / `LineTablesOnly` / `DebugDirectivesOnly`.
    EmissionKind(&'src str),
    /// `GNU` / `Apple` / `None` / `Default`.
    NameTableKind(&'src str),
    /// `Binary` / `Decimal` / `Rational`.
    FixedPointKind(&'src str),
    /// `dbg_*` — payload is the **suffix** (`value`, `declare`, …),
    /// matching `LLLexer::DBGRECORDTYPEKEYWORD` (LLLexer.cpp:998).
    DbgRecordType(&'src str),

    // ── Type tokens ──
    PrimitiveType(PrimitiveTy),

    // ── Numeric tokens ──
    IntegerLit(IntLit<'src>),
    FloatLit(FpLit<'src>),

    // ── Instruction & plain keywords ──
    Instruction(Opcode),
    Kw(Keyword),
}

/// Type-keyword tokens that resolve to a stateless primitive type.
///
/// Mirrors `TYPEKEYWORD(...)` entries in `LLLexer.cpp` plus the `i[0-9]+`
/// integer-type fast path. `ptr` is the default-address-space pointer; the
/// `addrspace(N)` form is parser-level (it composes `Token::Kw(Keyword::Ptr)`
/// — wait, actually `ptr` is the default. Let me re-read.) — see `LLParser.cpp`
/// for `addrspace(N)` handling.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PrimitiveTy {
    Void,
    Label,
    Metadata,
    Token,
    X86Amx,
    Half,
    BFloat,
    Float,
    Double,
    X86Fp80,
    Fp128,
    PpcFp128,
    Ptr,
    /// `iN` for `1 <= N <= 16777215`.
    Integer(NonZeroU32),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Sign {
    Pos,
    Neg,
}

/// Numeric base for [`IntLit`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NumBase {
    /// Plain decimal integer (`42`, `-1`).
    Dec,
    /// `s0x...` — explicitly signed APSInt (LLLexer.cpp:1047).
    HexSigned,
    /// `u0x...` — explicitly unsigned APSInt.
    HexUnsigned,
}

/// Borrowed integer literal lexeme.
///
/// `digits` is always pure ASCII (the lexer regex guarantees it), which is why
/// borrowing as `&str` is sound without any UTF-8 round-trip cost.
///
/// For `[us]0x...` forms `sign` is always [`Sign::Pos`] — those forms cannot
/// carry a leading `-` (LLLexer.cpp:1047-1064).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct IntLit<'src> {
    pub sign: Sign,
    pub base: NumBase,
    pub digits: &'src str,
}

/// Borrowed floating-point literal lexeme.
///
/// Each variant carries the *digits* — the lexer strips the syntactic prefix
/// but does not perform numeric conversion (deferred to the IR layer once
/// `APFloat` is ported).
///
/// * `Decimal` is the full lexeme including any leading sign, decimal point,
///   and exponent — it parses as `IEEEdouble` per LLLexer.cpp:1225/1262.
/// * The `Hex*` variants strip `0x` and (where present) the format marker
///   (`K`, `L`, `M`, `H`, `R`); the remaining slice is hex digits only.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum FpLit<'src> {
    /// `[-+]?[0-9]+\.[0-9]*([eE][-+]?[0-9]+)?` — IEEE double.
    Decimal(&'src str),
    /// `0x[0-9A-Fa-f]+` — IEEE double payload.
    HexDouble(&'src str),
    /// `0xK[0-9A-Fa-f]+` — x87 80-bit extended precision.
    HexX87(&'src str),
    /// `0xL[0-9A-Fa-f]+` — IEEE 128-bit quad.
    HexQuad(&'src str),
    /// `0xM[0-9A-Fa-f]+` — PPC double-double.
    HexPpc128(&'src str),
    /// `0xH[0-9A-Fa-f]+` — IEEE half (16-bit).
    HexHalf(&'src str),
    /// `0xR[0-9A-Fa-f]+` — bfloat16.
    HexBFloat(&'src str),
}

/// Which 16-bit hex floating-point lexeme overflowed. Used in
/// [`crate::ll_lexer::LexError::HexFpTooLarge`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum HexFpKind {
    Half,
    BFloat,
}

/// Which quoted-name token kind is currently being lexed. Used in
/// [`crate::ll_lexer::LexError::UnterminatedQuotedName`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum QuotedNameKind {
    Global,
    Local,
    Comdat,
    String,
    Metadata,
}

/// Mirrors `Instruction::Opcode` in `llvm/include/llvm/IR/Instruction.def` for
/// the set of opcodes that have a `.ll` keyword form.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Opcode {
    // Unary
    FNeg,
    // Binary integer
    Add,
    Sub,
    Mul,
    UDiv,
    SDiv,
    URem,
    SRem,
    Shl,
    LShr,
    AShr,
    And,
    Or,
    Xor,
    // Binary float
    FAdd,
    FSub,
    FMul,
    FDiv,
    FRem,
    // Compare
    ICmp,
    FCmp,
    // Casts
    Trunc,
    ZExt,
    SExt,
    FPTrunc,
    FPExt,
    UIToFP,
    SIToFP,
    FPToUI,
    FPToSI,
    IntToPtr,
    PtrToAddr,
    PtrToInt,
    BitCast,
    AddrSpaceCast,
    // Other
    Phi,
    Call,
    Select,
    VAArg,
    LandingPad,
    // Terminators
    Ret,
    Br,
    Switch,
    IndirectBr,
    Invoke,
    Resume,
    Unreachable,
    CleanupRet,
    CatchRet,
    CatchSwitch,
    CatchPad,
    CleanupPad,
    CallBr,
    // Memory
    Alloca,
    Load,
    Store,
    AtomicCmpXchg,
    AtomicRMW,
    Fence,
    GetElementPtr,
    // Vector / aggregate
    ExtractElement,
    InsertElement,
    ShuffleVector,
    ExtractValue,
    InsertValue,
    Freeze,
}

/// Plain (non-instruction, non-type, non-prefix) keywords.
///
/// Every entry mirrors a `KEYWORD(...)` line in `LLLexer.cpp` (lines 543-877)
/// or an `ATTRIBUTE_ENUM` entry from `Attributes.td` (LLVM 22.1.4 snapshot, see
/// `keywords.rs`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Keyword {
    // Booleans / declarations
    True,
    False,
    Declare,
    Define,
    Global,
    Constant,

    // DSO scope
    DsoLocal,
    DsoPreemptable,

    // Linkage
    Private,
    Internal,
    AvailableExternally,
    Linkonce,
    LinkonceOdr,
    Weak,
    WeakOdr,
    Appending,
    Dllimport,
    Dllexport,
    Common,
    Default,
    Hidden,
    Protected,
    UnnamedAddr,
    LocalUnnamedAddr,
    ExternallyInitialized,
    ExternWeak,
    External,
    ThreadLocal,
    Localdynamic,
    Initialexec,
    Localexec,

    // Initializers / constants
    Zeroinitializer,
    Undef,
    Null,
    None,
    Poison,

    // Misc keywords
    To,
    Caller,
    Within,
    From,
    Tail,
    Musttail,
    Notail,
    Target,
    Triple,
    SourceFilename,
    Unwind,
    Datalayout,
    Volatile,
    Atomic,
    Unordered,
    Monotonic,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
    Syncscope,

    // Fast-math & friends
    Nnan,
    Ninf,
    Nsz,
    Arcp,
    Contract,
    Reassoc,
    Afn,
    Fast,

    // Wrap / exact / bounds
    Nuw,
    Nsw,
    Nusw,
    Exact,
    Disjoint,
    Inbounds,
    Nneg,
    Samesign,
    Inrange,

    // Module-level
    Addrspace,
    Section,
    Partition,
    CodeModel,
    Alias,
    Ifunc,
    Module,
    Asm,
    Sideeffect,
    Inteldialect,
    Gc,
    Prefix,
    Prologue,

    // Sanitizer attributes (kept here to mirror the LLLexer block)
    NoSanitizeAddress,
    NoSanitizeHwaddress,
    SanitizeAddressDyninit,

    // Calling conventions
    Ccc,
    Fastcc,
    Coldcc,
    CfguardCheckcc,
    X86Stdcallcc,
    X86Fastcallcc,
    X86Thiscallcc,
    X86Vectorcallcc,
    ArmApcscc,
    ArmAapcscc,
    ArmAapcsVfpcc,
    Aarch64VectorPcs,
    Aarch64SveVectorPcs,
    Aarch64SmePreservemostFromX0,
    Aarch64SmePreservemostFromX1,
    Aarch64SmePreservemostFromX2,
    Msp430Intrcc,
    AvrIntrcc,
    AvrSignalcc,
    PtxKernel,
    PtxDevice,
    SpirKernel,
    SpirFunc,
    IntelOclBicc,
    X86_64Sysvcc,
    Win64cc,
    X86Regcallcc,
    Swiftcc,
    Swifttailcc,
    Anyregcc,
    PreserveMostcc,
    PreserveAllcc,
    PreserveNonecc,
    Ghccc,
    X86Intrcc,
    Hhvmcc,
    HhvmCcc,
    CxxFastTlscc,
    AmdgpuVs,
    AmdgpuLs,
    AmdgpuHs,
    AmdgpuEs,
    AmdgpuGs,
    AmdgpuPs,
    AmdgpuCs,
    AmdgpuCsChain,
    AmdgpuCsChainPreserve,
    AmdgpuKernel,
    AmdgpuGfx,
    AmdgpuGfxWholeWave,
    Tailcc,
    M68kRtdcc,
    Graalcc,
    RiscvVectorCc,
    RiscvVlsCc,
    CheriotCompartmentcallcc,
    CheriotCompartmentcalleecc,
    CheriotLibrarycallcc,
    Cc,
    C,

    // Attribute-list framing
    Attributes,
    Sync,
    Async,

    // ── Attributes from Attributes.td (LLVM 22.1.4 snapshot) ──
    // Kept in alphabetical order to make snapshot diffs obvious.
    Align,
    Alignstack,
    Allocalign,
    Allockind,
    Allocptr,
    Allocsize,
    Alwaysinline,
    Builtin,
    Byref,
    Byval,
    Captures,
    Cold,
    Convergent,
    CoroElideSafe,
    CoroOnlyDestroyWhenComplete,
    DeadOnReturn,
    DeadOnUnwind,
    Dereferenceable,
    DereferenceableOrNull,
    DisableSanitizerInstrumentation,
    Elementtype,
    FnRetThunkExtern,
    Hot,
    HybridPatchable,
    Immarg,
    Inalloca,
    Initializes,
    Inlinehint,
    Inreg,
    Jumptable,
    Memory,
    Minsize,
    Mustprogress,
    Naked,
    Nest,
    Noalias,
    Nobuiltin,
    Nocallback,
    NocfCheck,
    Nocreateundeforpoison,
    Nodivergencesource,
    Noduplicate,
    Noext,
    Nofpclass,
    Nofree,
    Noimplicitfloat,
    Noinline,
    Nomerge,
    Nonlazybind,
    Nonnull,
    Noprofile,
    Norecurse,
    Noredzone,
    Noreturn,
    NosanitizeBounds,
    NosanitizeCoverage,
    Nosync,
    Noundef,
    Nounwind,
    NullPointerIsValid,
    Optdebug,
    Optforfuzzing,
    Optnone,
    Optsize,
    Preallocated,
    Presplitcoroutine,
    Range,
    Readnone,
    Readonly,
    Returned,
    ReturnsTwice,
    Safestack,
    SanitizeAddress,
    SanitizeAllocToken,
    SanitizeHwaddress,
    SanitizeMemory,
    SanitizeMemtag,
    SanitizeNumericalStability,
    SanitizeRealtime,
    SanitizeRealtimeBlocking,
    SanitizeThread,
    SanitizeType,
    Shadowcallstack,
    Signext,
    Skipprofile,
    Speculatable,
    SpeculativeLoadHardening,
    Sret,
    Ssp,
    Sspreq,
    Sspstrong,
    Strictfp,
    Swiftasync,
    Swifterror,
    Swiftself,
    Uwtable,
    VscaleRange,
    Willreturn,
    Writable,
    Writeonly,
    Zeroext,

    // ── Memory effects / legacy attributes ──
    Read,
    Write,
    Readwrite,
    Argmem,
    TargetMem0,
    TargetMem1,
    Inaccessiblemem,
    Errnomem,

    Argmemonly,
    Inaccessiblememonly,
    InaccessiblememOrArgmemonly,
    Nocapture,

    // Captures attribute components
    Address,
    AddressIsNull,
    Provenance,
    ReadProvenance,

    // nofpclass components
    All,
    Nan,
    Snan,
    Qnan,
    Inf,
    // ninf is a fast-math flag and reuses Keyword::Ninf
    Pinf,
    Norm,
    Nnorm,
    Pnorm,
    // sub is the instruction; the legacy nofpclass `sub` keyword is parsed by
    // value comparison at the parser level.
    Nsub,
    Psub,
    Zero,
    Nzero,
    Pzero,

    // Type / opaque / comdat bookkeeping
    Type,
    Opaque,
    Comdat,
    Any,
    Exactmatch,
    Largest,
    Nodeduplicate,
    Samesize,

    // ── ICmp/FCmp predicate keywords ──
    Eq,
    Ne,
    Slt,
    Sgt,
    Sle,
    Sge,
    Ult,
    Ugt,
    Ule,
    Uge,
    Oeq,
    One,
    Olt,
    Ogt,
    Ole,
    Oge,
    Ord,
    Uno,
    Ueq,
    Une,

    // ── atomicrmw ops not also instruction keywords ──
    Xchg,
    Nand,
    Max,
    Min,
    Umax,
    Umin,
    Fmax,
    Fmin,
    Fmaximum,
    Fminimum,
    UincWrap,
    UdecWrap,
    UsubCond,
    UsubSat,

    // ── Constants and constant expression keywords ──
    Splat,
    Vscale,
    X,
    Blockaddress,
    DsoLocalEquivalent,
    NoCfi,
    Ptrauth,

    // ── Metadata / use-list ──
    Distinct,
    Uselistorder,
    UselistorderBb,

    // ── EH / personality ──
    Personality,
    Cleanup,
    Catch,
    Filter,

    // ── Summary index keywords (LLLexer.cpp:788-877) ──
    Path,
    Hash_, // `hash` — collides with Token::Hash punctuation; renamed enum-side.
    Gv,
    Guid,
    Name,
    Summaries,
    Flags,
    Blockcount,
    Linkage,
    Visibility,
    NotEligibleToImport,
    Live,
    DsoLocal_,
    CanAutoHide,
    ImportType,
    Definition,
    Declaration,
    Function,
    Insts,
    FuncFlags,
    ReadNone,
    ReadOnly,
    NoRecurse,
    ReturnDoesNotAlias,
    NoInline,
    AlwaysInline,
    NoUnwind,
    MayThrow,
    HasUnknownCall,
    MustBeUnreachable,
    Calls,
    Callee,
    Params,
    Param,
    Hotness,
    Unknown,
    Critical,
    Relbf,
    Variable,
    VTableFuncs,
    VirtFunc,
    Aliasee,
    Refs,
    TypeIdInfo,
    TypeTests,
    TypeTestAssumeVCalls,
    TypeCheckedLoadVCalls,
    TypeTestAssumeConstVCalls,
    TypeCheckedLoadConstVCalls,
    VFuncId,
    Offset,
    Args,
    Typeid,
    TypeidCompatibleVTable,
    Summary,
    TypeTestRes,
    Kind,
    Unsat,
    ByteArray,
    Inline,
    Single,
    AllOnes,
    SizeM1BitWidth,
    AlignLog2,
    SizeM1,
    BitMask,
    InlineBits,
    VcallVisibility,
    WpdResolutions,
    WpdRes,
    Indir,
    SingleImpl,
    BranchFunnel,
    SingleImplName,
    ResByArg,
    ByArg,
    UniformRetVal,
    UniqueRetVal,
    VirtualConstProp,
    Info,
    Byte,
    Bit,
    VarFlags,
    Callsites,
    Clones,
    StackIds,
    Allocs,
    Versions,
    MemProf,
    Notcold,
}
