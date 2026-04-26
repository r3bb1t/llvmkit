//! Keyword classification table.
//!
//! Maps a previously-extracted identifier byte slice to the corresponding
//! [`Token`]. Returns `None` when the slice is not a known keyword (the lexer
//! treats that case as "must be a label or an error", per `LexIdentifier`'s
//! tail in `LLLexer.cpp:1066-1074`).
//!
//! Special-case handlers that *cannot* live in this table because they need a
//! borrowed payload from the source buffer (or carry numeric data) are
//! resolved in `lexer/mod.rs::lex_identifier`:
//!
//! * `iN` integer types — parsed numerically, range-checked.
//! * `DW_<TYPE>_*`, `DIFlag*`, `DISPFlag*`, `CSK_*`, `dbg_*` —
//!   payload is the borrowed slice.
//! * `NoDebug` / `FullDebug` / `LineTablesOnly` / `DebugDirectivesOnly` —
//!   `EmissionKind`, payload is borrowed.
//! * `GNU` / `Apple` / `None` / `Default` — `NameTableKind`.
//! * `Binary` / `Decimal` / `Rational` — `FixedPointKind`.
//! * `[us]0x[0-9A-Fa-f]+` — APSInt-style hex literal.
//! * `cc<digits>` rewind — emit `kw_cc` and roll back the cursor.
//!
//! TODO(tablegen): the attribute keyword list is hand-mirrored from
//! `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/IR/Attributes.td`
//! (LLVM 22.1.4). Replace with a build-time tablegen port in a future session.

use super::super::ll_token::{Keyword, Opcode, PrimitiveTy, Token};

/// Classify a non-numeric, non-DWARF, non-`iN` identifier as either a fixed
/// keyword token or `None` (caller falls through to error or label handling).
///
/// Returns `Token<'static>` because every match is a unit-payload token; the
/// caller widens to `Token<'src>` via `'static: 'src` covariance.
pub(super) fn classify_word(word: &[u8]) -> Option<Token<'static>> {
    use Keyword::*;
    use Opcode as Op;
    use PrimitiveTy as Ty;

    let kw = |k: Keyword| Some(Token::Kw(k));
    let op = |o: Op| Some(Token::Instruction(o));
    let ty = |t: Ty| Some(Token::PrimitiveType(t));

    match word {
        // ── Type keywords (TYPEKEYWORD) ──
        b"void" => ty(Ty::Void),
        b"half" => ty(Ty::Half),
        b"bfloat" => ty(Ty::BFloat),
        b"float" => ty(Ty::Float),
        b"double" => ty(Ty::Double),
        b"x86_fp80" => ty(Ty::X86Fp80),
        b"fp128" => ty(Ty::Fp128),
        b"ppc_fp128" => ty(Ty::PpcFp128),
        b"label" => ty(Ty::Label),
        b"metadata" => ty(Ty::Metadata),
        b"x86_amx" => ty(Ty::X86Amx),
        b"token" => ty(Ty::Token),
        b"ptr" => ty(Ty::Ptr),

        // ── Instruction keywords (INSTKEYWORD) ──
        b"fneg" => op(Op::FNeg),
        b"add" => op(Op::Add),
        b"fadd" => op(Op::FAdd),
        b"sub" => op(Op::Sub),
        b"fsub" => op(Op::FSub),
        b"mul" => op(Op::Mul),
        b"fmul" => op(Op::FMul),
        b"udiv" => op(Op::UDiv),
        b"sdiv" => op(Op::SDiv),
        b"fdiv" => op(Op::FDiv),
        b"urem" => op(Op::URem),
        b"srem" => op(Op::SRem),
        b"frem" => op(Op::FRem),
        b"shl" => op(Op::Shl),
        b"lshr" => op(Op::LShr),
        b"ashr" => op(Op::AShr),
        b"and" => op(Op::And),
        b"or" => op(Op::Or),
        b"xor" => op(Op::Xor),
        b"icmp" => op(Op::ICmp),
        b"fcmp" => op(Op::FCmp),
        b"phi" => op(Op::Phi),
        b"call" => op(Op::Call),
        b"trunc" => op(Op::Trunc),
        b"zext" => op(Op::ZExt),
        b"sext" => op(Op::SExt),
        b"fptrunc" => op(Op::FPTrunc),
        b"fpext" => op(Op::FPExt),
        b"uitofp" => op(Op::UIToFP),
        b"sitofp" => op(Op::SIToFP),
        b"fptoui" => op(Op::FPToUI),
        b"fptosi" => op(Op::FPToSI),
        b"inttoptr" => op(Op::IntToPtr),
        b"ptrtoaddr" => op(Op::PtrToAddr),
        b"ptrtoint" => op(Op::PtrToInt),
        b"bitcast" => op(Op::BitCast),
        b"addrspacecast" => op(Op::AddrSpaceCast),
        b"select" => op(Op::Select),
        b"va_arg" => op(Op::VAArg),
        b"ret" => op(Op::Ret),
        b"br" => op(Op::Br),
        b"switch" => op(Op::Switch),
        b"indirectbr" => op(Op::IndirectBr),
        b"invoke" => op(Op::Invoke),
        b"resume" => op(Op::Resume),
        b"unreachable" => op(Op::Unreachable),
        b"callbr" => op(Op::CallBr),
        b"alloca" => op(Op::Alloca),
        b"load" => op(Op::Load),
        b"store" => op(Op::Store),
        b"cmpxchg" => op(Op::AtomicCmpXchg),
        b"atomicrmw" => op(Op::AtomicRMW),
        b"fence" => op(Op::Fence),
        b"getelementptr" => op(Op::GetElementPtr),
        b"extractelement" => op(Op::ExtractElement),
        b"insertelement" => op(Op::InsertElement),
        b"shufflevector" => op(Op::ShuffleVector),
        b"extractvalue" => op(Op::ExtractValue),
        b"insertvalue" => op(Op::InsertValue),
        b"landingpad" => op(Op::LandingPad),
        b"cleanupret" => op(Op::CleanupRet),
        b"catchret" => op(Op::CatchRet),
        b"catchswitch" => op(Op::CatchSwitch),
        b"catchpad" => op(Op::CatchPad),
        b"cleanuppad" => op(Op::CleanupPad),
        b"freeze" => op(Op::Freeze),

        // ── Booleans / declarations / linkage / visibility ──
        b"true" => kw(True),
        b"false" => kw(False),
        b"declare" => kw(Declare),
        b"define" => kw(Define),
        b"global" => kw(Global),
        b"constant" => kw(Constant),
        b"dso_local" => kw(DsoLocal),
        b"dso_preemptable" => kw(DsoPreemptable),
        b"private" => kw(Private),
        b"internal" => kw(Internal),
        b"available_externally" => kw(AvailableExternally),
        b"linkonce" => kw(Linkonce),
        b"linkonce_odr" => kw(LinkonceOdr),
        b"weak" => kw(Weak),
        b"weak_odr" => kw(WeakOdr),
        b"appending" => kw(Appending),
        b"dllimport" => kw(Dllimport),
        b"dllexport" => kw(Dllexport),
        b"common" => kw(Common),
        b"default" => kw(Default),
        b"hidden" => kw(Hidden),
        b"protected" => kw(Protected),
        b"unnamed_addr" => kw(UnnamedAddr),
        b"local_unnamed_addr" => kw(LocalUnnamedAddr),
        b"externally_initialized" => kw(ExternallyInitialized),
        b"extern_weak" => kw(ExternWeak),
        b"external" => kw(External),
        b"thread_local" => kw(ThreadLocal),
        b"localdynamic" => kw(Localdynamic),
        b"initialexec" => kw(Initialexec),
        b"localexec" => kw(Localexec),
        b"zeroinitializer" => kw(Zeroinitializer),
        b"undef" => kw(Undef),
        b"null" => kw(Null),
        b"none" => kw(None),
        b"poison" => kw(Poison),
        b"to" => kw(To),
        b"caller" => kw(Caller),
        b"within" => kw(Within),
        b"from" => kw(From),
        b"tail" => kw(Tail),
        b"musttail" => kw(Musttail),
        b"notail" => kw(Notail),
        b"target" => kw(Target),
        b"triple" => kw(Triple),
        b"source_filename" => kw(SourceFilename),
        b"unwind" => kw(Unwind),
        b"datalayout" => kw(Datalayout),
        b"volatile" => kw(Volatile),
        b"atomic" => kw(Atomic),
        b"unordered" => kw(Unordered),
        b"monotonic" => kw(Monotonic),
        b"acquire" => kw(Acquire),
        b"release" => kw(Release),
        b"acq_rel" => kw(AcqRel),
        b"seq_cst" => kw(SeqCst),
        b"syncscope" => kw(Syncscope),

        // ── Fast-math & friends ──
        b"nnan" => kw(Nnan),
        b"ninf" => kw(Ninf),
        b"nsz" => kw(Nsz),
        b"arcp" => kw(Arcp),
        b"contract" => kw(Contract),
        b"reassoc" => kw(Reassoc),
        b"afn" => kw(Afn),
        b"fast" => kw(Fast),

        // ── Wrap / exact / bounds ──
        b"nuw" => kw(Nuw),
        b"nsw" => kw(Nsw),
        b"nusw" => kw(Nusw),
        b"exact" => kw(Exact),
        b"disjoint" => kw(Disjoint),
        b"inbounds" => kw(Inbounds),
        b"nneg" => kw(Nneg),
        b"samesign" => kw(Samesign),
        b"inrange" => kw(Inrange),

        // ── Module-level ──
        b"addrspace" => kw(Addrspace),
        b"section" => kw(Section),
        b"partition" => kw(Partition),
        b"code_model" => kw(CodeModel),
        b"alias" => kw(Alias),
        b"ifunc" => kw(Ifunc),
        b"module" => kw(Module),
        b"asm" => kw(Asm),
        b"sideeffect" => kw(Sideeffect),
        b"inteldialect" => kw(Inteldialect),
        b"gc" => kw(Gc),
        b"prefix" => kw(Prefix),
        b"prologue" => kw(Prologue),

        // ── Sanitizer aux ──
        b"no_sanitize_address" => kw(NoSanitizeAddress),
        b"no_sanitize_hwaddress" => kw(NoSanitizeHwaddress),
        b"sanitize_address_dyninit" => kw(SanitizeAddressDyninit),

        // ── Calling conventions ──
        b"ccc" => kw(Ccc),
        b"fastcc" => kw(Fastcc),
        b"coldcc" => kw(Coldcc),
        b"cfguard_checkcc" => kw(CfguardCheckcc),
        b"x86_stdcallcc" => kw(X86Stdcallcc),
        b"x86_fastcallcc" => kw(X86Fastcallcc),
        b"x86_thiscallcc" => kw(X86Thiscallcc),
        b"x86_vectorcallcc" => kw(X86Vectorcallcc),
        b"arm_apcscc" => kw(ArmApcscc),
        b"arm_aapcscc" => kw(ArmAapcscc),
        b"arm_aapcs_vfpcc" => kw(ArmAapcsVfpcc),
        b"aarch64_vector_pcs" => kw(Aarch64VectorPcs),
        b"aarch64_sve_vector_pcs" => kw(Aarch64SveVectorPcs),
        b"aarch64_sme_preservemost_from_x0" => kw(Aarch64SmePreservemostFromX0),
        b"aarch64_sme_preservemost_from_x1" => kw(Aarch64SmePreservemostFromX1),
        b"aarch64_sme_preservemost_from_x2" => kw(Aarch64SmePreservemostFromX2),
        b"msp430_intrcc" => kw(Msp430Intrcc),
        b"avr_intrcc" => kw(AvrIntrcc),
        b"avr_signalcc" => kw(AvrSignalcc),
        b"ptx_kernel" => kw(PtxKernel),
        b"ptx_device" => kw(PtxDevice),
        b"spir_kernel" => kw(SpirKernel),
        b"spir_func" => kw(SpirFunc),
        b"intel_ocl_bicc" => kw(IntelOclBicc),
        b"x86_64_sysvcc" => kw(X86_64Sysvcc),
        b"win64cc" => kw(Win64cc),
        b"x86_regcallcc" => kw(X86Regcallcc),
        b"swiftcc" => kw(Swiftcc),
        b"swifttailcc" => kw(Swifttailcc),
        b"anyregcc" => kw(Anyregcc),
        b"preserve_mostcc" => kw(PreserveMostcc),
        b"preserve_allcc" => kw(PreserveAllcc),
        b"preserve_nonecc" => kw(PreserveNonecc),
        b"ghccc" => kw(Ghccc),
        b"x86_intrcc" => kw(X86Intrcc),
        b"hhvmcc" => kw(Hhvmcc),
        b"hhvm_ccc" => kw(HhvmCcc),
        b"cxx_fast_tlscc" => kw(CxxFastTlscc),
        b"amdgpu_vs" => kw(AmdgpuVs),
        b"amdgpu_ls" => kw(AmdgpuLs),
        b"amdgpu_hs" => kw(AmdgpuHs),
        b"amdgpu_es" => kw(AmdgpuEs),
        b"amdgpu_gs" => kw(AmdgpuGs),
        b"amdgpu_ps" => kw(AmdgpuPs),
        b"amdgpu_cs" => kw(AmdgpuCs),
        b"amdgpu_cs_chain" => kw(AmdgpuCsChain),
        b"amdgpu_cs_chain_preserve" => kw(AmdgpuCsChainPreserve),
        b"amdgpu_kernel" => kw(AmdgpuKernel),
        b"amdgpu_gfx" => kw(AmdgpuGfx),
        b"amdgpu_gfx_whole_wave" => kw(AmdgpuGfxWholeWave),
        b"tailcc" => kw(Tailcc),
        b"m68k_rtdcc" => kw(M68kRtdcc),
        b"graalcc" => kw(Graalcc),
        b"riscv_vector_cc" => kw(RiscvVectorCc),
        b"riscv_vls_cc" => kw(RiscvVlsCc),
        b"cheriot_compartmentcallcc" => kw(CheriotCompartmentcallcc),
        b"cheriot_compartmentcalleecc" => kw(CheriotCompartmentcalleecc),
        b"cheriot_librarycallcc" => kw(CheriotLibrarycallcc),
        b"cc" => kw(Cc),
        b"c" => kw(C),

        // ── Attribute-list framing ──
        b"attributes" => kw(Attributes),
        b"sync" => kw(Sync),
        b"async" => kw(Async),

        // ── Attributes from Attributes.td (LLVM 22.1.4 snapshot, 101 entries) ──
        b"align" => kw(Align),
        b"alignstack" => kw(Alignstack),
        b"allocalign" => kw(Allocalign),
        b"allockind" => kw(Allockind),
        b"allocptr" => kw(Allocptr),
        b"allocsize" => kw(Allocsize),
        b"alwaysinline" => kw(Alwaysinline),
        b"builtin" => kw(Builtin),
        b"byref" => kw(Byref),
        b"byval" => kw(Byval),
        b"captures" => kw(Captures),
        b"cold" => kw(Cold),
        b"convergent" => kw(Convergent),
        b"coro_elide_safe" => kw(CoroElideSafe),
        b"coro_only_destroy_when_complete" => kw(CoroOnlyDestroyWhenComplete),
        b"dead_on_return" => kw(DeadOnReturn),
        b"dead_on_unwind" => kw(DeadOnUnwind),
        b"dereferenceable" => kw(Dereferenceable),
        b"dereferenceable_or_null" => kw(DereferenceableOrNull),
        b"disable_sanitizer_instrumentation" => kw(DisableSanitizerInstrumentation),
        b"elementtype" => kw(Elementtype),
        b"fn_ret_thunk_extern" => kw(FnRetThunkExtern),
        b"hot" => kw(Hot),
        b"hybrid_patchable" => kw(HybridPatchable),
        b"immarg" => kw(Immarg),
        b"inalloca" => kw(Inalloca),
        b"initializes" => kw(Initializes),
        b"inlinehint" => kw(Inlinehint),
        b"inreg" => kw(Inreg),
        b"jumptable" => kw(Jumptable),
        b"memory" => kw(Memory),
        b"minsize" => kw(Minsize),
        b"mustprogress" => kw(Mustprogress),
        b"naked" => kw(Naked),
        b"nest" => kw(Nest),
        b"noalias" => kw(Noalias),
        b"nobuiltin" => kw(Nobuiltin),
        b"nocallback" => kw(Nocallback),
        b"nocf_check" => kw(NocfCheck),
        b"nocreateundeforpoison" => kw(Nocreateundeforpoison),
        b"nodivergencesource" => kw(Nodivergencesource),
        b"noduplicate" => kw(Noduplicate),
        b"noext" => kw(Noext),
        b"nofpclass" => kw(Nofpclass),
        b"nofree" => kw(Nofree),
        b"noimplicitfloat" => kw(Noimplicitfloat),
        b"noinline" => kw(Noinline),
        b"nomerge" => kw(Nomerge),
        b"nonlazybind" => kw(Nonlazybind),
        b"nonnull" => kw(Nonnull),
        b"noprofile" => kw(Noprofile),
        b"norecurse" => kw(Norecurse),
        b"noredzone" => kw(Noredzone),
        b"noreturn" => kw(Noreturn),
        b"nosanitize_bounds" => kw(NosanitizeBounds),
        b"nosanitize_coverage" => kw(NosanitizeCoverage),
        b"nosync" => kw(Nosync),
        b"noundef" => kw(Noundef),
        b"nounwind" => kw(Nounwind),
        b"null_pointer_is_valid" => kw(NullPointerIsValid),
        b"optdebug" => kw(Optdebug),
        b"optforfuzzing" => kw(Optforfuzzing),
        b"optnone" => kw(Optnone),
        b"optsize" => kw(Optsize),
        b"preallocated" => kw(Preallocated),
        b"presplitcoroutine" => kw(Presplitcoroutine),
        b"range" => kw(Range),
        b"readnone" => kw(Readnone),
        b"readonly" => kw(Readonly),
        b"returned" => kw(Returned),
        b"returns_twice" => kw(ReturnsTwice),
        b"safestack" => kw(Safestack),
        b"sanitize_address" => kw(SanitizeAddress),
        b"sanitize_alloc_token" => kw(SanitizeAllocToken),
        b"sanitize_hwaddress" => kw(SanitizeHwaddress),
        b"sanitize_memory" => kw(SanitizeMemory),
        b"sanitize_memtag" => kw(SanitizeMemtag),
        b"sanitize_numerical_stability" => kw(SanitizeNumericalStability),
        b"sanitize_realtime" => kw(SanitizeRealtime),
        b"sanitize_realtime_blocking" => kw(SanitizeRealtimeBlocking),
        b"sanitize_thread" => kw(SanitizeThread),
        b"sanitize_type" => kw(SanitizeType),
        b"shadowcallstack" => kw(Shadowcallstack),
        b"signext" => kw(Signext),
        b"skipprofile" => kw(Skipprofile),
        b"speculatable" => kw(Speculatable),
        b"speculative_load_hardening" => kw(SpeculativeLoadHardening),
        b"sret" => kw(Sret),
        b"ssp" => kw(Ssp),
        b"sspreq" => kw(Sspreq),
        b"sspstrong" => kw(Sspstrong),
        b"strictfp" => kw(Strictfp),
        b"swiftasync" => kw(Swiftasync),
        b"swifterror" => kw(Swifterror),
        b"swiftself" => kw(Swiftself),
        b"uwtable" => kw(Uwtable),
        b"vscale_range" => kw(VscaleRange),
        b"willreturn" => kw(Willreturn),
        b"writable" => kw(Writable),
        b"writeonly" => kw(Writeonly),
        b"zeroext" => kw(Zeroext),

        // ── Memory effects / legacy attributes ──
        b"read" => kw(Read),
        b"write" => kw(Write),
        b"readwrite" => kw(Readwrite),
        b"argmem" => kw(Argmem),
        b"target_mem0" => kw(TargetMem0),
        b"target_mem1" => kw(TargetMem1),
        b"inaccessiblemem" => kw(Inaccessiblemem),
        b"errnomem" => kw(Errnomem),
        b"argmemonly" => kw(Argmemonly),
        b"inaccessiblememonly" => kw(Inaccessiblememonly),
        b"inaccessiblemem_or_argmemonly" => kw(InaccessiblememOrArgmemonly),
        b"nocapture" => kw(Nocapture),

        // ── Captures attribute components ──
        b"address" => kw(Address),
        b"address_is_null" => kw(AddressIsNull),
        b"provenance" => kw(Provenance),
        b"read_provenance" => kw(ReadProvenance),

        // ── nofpclass components ──
        b"all" => kw(All),
        b"nan" => kw(Nan),
        b"snan" => kw(Snan),
        b"qnan" => kw(Qnan),
        b"inf" => kw(Inf),
        b"pinf" => kw(Pinf),
        b"norm" => kw(Norm),
        b"nnorm" => kw(Nnorm),
        b"pnorm" => kw(Pnorm),
        b"nsub" => kw(Nsub),
        b"psub" => kw(Psub),
        b"zero" => kw(Zero),
        b"nzero" => kw(Nzero),
        b"pzero" => kw(Pzero),

        // ── Type / opaque / comdat ──
        b"type" => kw(Type),
        b"opaque" => kw(Opaque),
        b"comdat" => kw(Comdat),
        b"any" => kw(Any),
        b"exactmatch" => kw(Exactmatch),
        b"largest" => kw(Largest),
        b"nodeduplicate" => kw(Nodeduplicate),
        b"samesize" => kw(Samesize),

        // ── ICmp/FCmp predicates ──
        b"eq" => kw(Eq),
        b"ne" => kw(Ne),
        b"slt" => kw(Slt),
        b"sgt" => kw(Sgt),
        b"sle" => kw(Sle),
        b"sge" => kw(Sge),
        b"ult" => kw(Ult),
        b"ugt" => kw(Ugt),
        b"ule" => kw(Ule),
        b"uge" => kw(Uge),
        b"oeq" => kw(Oeq),
        b"one" => kw(One),
        b"olt" => kw(Olt),
        b"ogt" => kw(Ogt),
        b"ole" => kw(Ole),
        b"oge" => kw(Oge),
        b"ord" => kw(Ord),
        b"uno" => kw(Uno),
        b"ueq" => kw(Ueq),
        b"une" => kw(Une),

        // ── atomicrmw ops not also instructions ──
        b"xchg" => kw(Xchg),
        b"nand" => kw(Nand),
        b"max" => kw(Max),
        b"min" => kw(Min),
        b"umax" => kw(Umax),
        b"umin" => kw(Umin),
        b"fmax" => kw(Fmax),
        b"fmin" => kw(Fmin),
        b"fmaximum" => kw(Fmaximum),
        b"fminimum" => kw(Fminimum),
        b"uinc_wrap" => kw(UincWrap),
        b"udec_wrap" => kw(UdecWrap),
        b"usub_cond" => kw(UsubCond),
        b"usub_sat" => kw(UsubSat),

        // ── Constant-expression / vector keywords ──
        b"splat" => kw(Splat),
        b"vscale" => kw(Vscale),
        b"x" => kw(X),
        b"blockaddress" => kw(Blockaddress),
        b"dso_local_equivalent" => kw(DsoLocalEquivalent),
        b"no_cfi" => kw(NoCfi),
        b"ptrauth" => kw(Ptrauth),

        // ── Metadata / use-list ──
        b"distinct" => kw(Distinct),
        b"uselistorder" => kw(Uselistorder),
        b"uselistorder_bb" => kw(UselistorderBb),

        // ── EH ──
        b"personality" => kw(Personality),
        b"cleanup" => kw(Cleanup),
        b"catch" => kw(Catch),
        b"filter" => kw(Filter),

        // ── Summary index keywords (LLLexer.cpp:788-877) ──
        b"path" => kw(Path),
        b"hash" => kw(Hash_),
        b"gv" => kw(Gv),
        b"guid" => kw(Guid),
        b"name" => kw(Name),
        b"summaries" => kw(Summaries),
        b"flags" => kw(Flags),
        b"blockcount" => kw(Blockcount),
        b"linkage" => kw(Linkage),
        b"visibility" => kw(Visibility),
        b"notEligibleToImport" => kw(NotEligibleToImport),
        b"live" => kw(Live),
        b"dsoLocal" => kw(DsoLocal_),
        b"canAutoHide" => kw(CanAutoHide),
        b"importType" => kw(ImportType),
        b"definition" => kw(Definition),
        b"declaration" => kw(Declaration),
        b"function" => kw(Function),
        b"insts" => kw(Insts),
        b"funcFlags" => kw(FuncFlags),
        b"readNone" => kw(ReadNone),
        b"readOnly" => kw(ReadOnly),
        b"noRecurse" => kw(NoRecurse),
        b"returnDoesNotAlias" => kw(ReturnDoesNotAlias),
        b"noInline" => kw(NoInline),
        b"alwaysInline" => kw(AlwaysInline),
        b"noUnwind" => kw(NoUnwind),
        b"mayThrow" => kw(MayThrow),
        b"hasUnknownCall" => kw(HasUnknownCall),
        b"mustBeUnreachable" => kw(MustBeUnreachable),
        b"calls" => kw(Calls),
        b"callee" => kw(Callee),
        b"params" => kw(Params),
        b"param" => kw(Param),
        b"hotness" => kw(Hotness),
        b"unknown" => kw(Unknown),
        b"critical" => kw(Critical),
        b"relbf" => kw(Relbf),
        b"variable" => kw(Variable),
        b"vTableFuncs" => kw(VTableFuncs),
        b"virtFunc" => kw(VirtFunc),
        b"aliasee" => kw(Aliasee),
        b"refs" => kw(Refs),
        b"typeIdInfo" => kw(TypeIdInfo),
        b"typeTests" => kw(TypeTests),
        b"typeTestAssumeVCalls" => kw(TypeTestAssumeVCalls),
        b"typeCheckedLoadVCalls" => kw(TypeCheckedLoadVCalls),
        b"typeTestAssumeConstVCalls" => kw(TypeTestAssumeConstVCalls),
        b"typeCheckedLoadConstVCalls" => kw(TypeCheckedLoadConstVCalls),
        b"vFuncId" => kw(VFuncId),
        b"offset" => kw(Offset),
        b"args" => kw(Args),
        b"typeid" => kw(Typeid),
        b"typeidCompatibleVTable" => kw(TypeidCompatibleVTable),
        b"summary" => kw(Summary),
        b"typeTestRes" => kw(TypeTestRes),
        b"kind" => kw(Kind),
        b"unsat" => kw(Unsat),
        b"byteArray" => kw(ByteArray),
        b"inline" => kw(Inline),
        b"single" => kw(Single),
        b"allOnes" => kw(AllOnes),
        b"sizeM1BitWidth" => kw(SizeM1BitWidth),
        b"alignLog2" => kw(AlignLog2),
        b"sizeM1" => kw(SizeM1),
        b"bitMask" => kw(BitMask),
        b"inlineBits" => kw(InlineBits),
        b"vcall_visibility" => kw(VcallVisibility),
        b"wpdResolutions" => kw(WpdResolutions),
        b"wpdRes" => kw(WpdRes),
        b"indir" => kw(Indir),
        b"singleImpl" => kw(SingleImpl),
        b"branchFunnel" => kw(BranchFunnel),
        b"singleImplName" => kw(SingleImplName),
        b"resByArg" => kw(ResByArg),
        b"byArg" => kw(ByArg),
        b"uniformRetVal" => kw(UniformRetVal),
        b"uniqueRetVal" => kw(UniqueRetVal),
        b"virtualConstProp" => kw(VirtualConstProp),
        b"info" => kw(Info),
        b"byte" => kw(Byte),
        b"bit" => kw(Bit),
        b"varFlags" => kw(VarFlags),
        b"callsites" => kw(Callsites),
        b"clones" => kw(Clones),
        b"stackIds" => kw(StackIds),
        b"allocs" => kw(Allocs),
        b"versions" => kw(Versions),
        b"memProf" => kw(MemProf),
        b"notcold" => kw(Notcold),

        _ => Option::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_classified() {
        assert!(matches!(
            classify_word(b"void"),
            Some(Token::PrimitiveType(PrimitiveTy::Void))
        ));
        assert!(matches!(
            classify_word(b"ptr"),
            Some(Token::PrimitiveType(PrimitiveTy::Ptr))
        ));
    }

    #[test]
    fn opcodes_classified() {
        assert!(matches!(
            classify_word(b"add"),
            Some(Token::Instruction(Opcode::Add))
        ));
        assert!(matches!(
            classify_word(b"getelementptr"),
            Some(Token::Instruction(Opcode::GetElementPtr))
        ));
    }

    #[test]
    fn plain_keywords_classified() {
        assert!(matches!(
            classify_word(b"true"),
            Some(Token::Kw(Keyword::True))
        ));
        assert!(matches!(
            classify_word(b"define"),
            Some(Token::Kw(Keyword::Define))
        ));
        assert!(matches!(
            classify_word(b"fastcc"),
            Some(Token::Kw(Keyword::Fastcc))
        ));
    }

    #[test]
    fn attributes_classified() {
        assert!(matches!(
            classify_word(b"noinline"),
            Some(Token::Kw(Keyword::Noinline))
        ));
        assert!(matches!(
            classify_word(b"nounwind"),
            Some(Token::Kw(Keyword::Nounwind))
        ));
        assert!(matches!(
            classify_word(b"sret"),
            Some(Token::Kw(Keyword::Sret))
        ));
    }

    #[test]
    fn summary_camelcase_distinct_from_snake() {
        // `dso_local` and `dsoLocal` are distinct keywords.
        assert!(matches!(
            classify_word(b"dso_local"),
            Some(Token::Kw(Keyword::DsoLocal))
        ));
        assert!(matches!(
            classify_word(b"dsoLocal"),
            Some(Token::Kw(Keyword::DsoLocal_))
        ));
        // `noinline` (attribute) vs `noInline` (summary index).
        assert!(matches!(
            classify_word(b"noinline"),
            Some(Token::Kw(Keyword::Noinline))
        ));
        assert!(matches!(
            classify_word(b"noInline"),
            Some(Token::Kw(Keyword::NoInline))
        ));
    }

    #[test]
    fn unknown_returns_none() {
        assert!(classify_word(b"completely_unknown").is_none());
        assert!(classify_word(b"").is_none());
    }
}
