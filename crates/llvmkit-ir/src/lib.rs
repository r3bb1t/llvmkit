#![forbid(unsafe_code)]
//! LLVM IR data model in pure safe Rust.
//!
//! `llvmkit-ir` mirrors the relevant `llvm/lib/IR/` and `llvm/include/llvm/IR/`
//! surfaces from LLVM 22.1.4: typed IR construction, AsmWriter printing,
//! structural verification, CFG and dominance queries, and a capability-graded,
//! new-pass-manager-inspired analysis and pass API. It does not link `libLLVM`.
//!
//! # Writing a pass
//!
//! A pass is one `impl` block. Declare a capability *rung* (`type Access`) — how
//! much of the IR it may touch — plus the analyses it needs; the driver derives
//! preservation and whether the output module stays [`Verified`]. Over-claiming
//! what a pass preserves is a compile error, not a stale-analysis miscompile.
//!
//! ```
//! use llvmkit_ir::{function_pass, DominatorTreeAnalysis, IrResult};
//!
//! struct EntryReachable;
//!
//! #[function_pass(name = "entry-reachable", access = Inspect, requires = [DominatorTreeAnalysis])]
//! impl EntryReachable {
//!     fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> {
//!         let dt = cx.analysis::<DominatorTreeAnalysis, _>();
//!         if let Some(entry) = cx.function().entry_block() {
//!             let _reachable = dt.is_reachable_from_entry(entry);
//!         }
//!         Ok(cx.done()) // `Inspect` has no `cx.mutate()`; the module stays Verified
//!     }
//! }
//! ```
//!
//! Run one pass with [`run_function_pass`] / [`run_module_pass`], compose several
//! at compile time with [`function_pipeline`] / [`module_pipeline`], or assemble
//! one at run time with [`DynFunctionPipeline`]. **The full pass guide — the
//! capability rungs, the three run modes, and mutating passes — is in the
//! [`pass_manager`] module docs**; the `#[function_pass]` / `#[module_pass]`
//! sugar is documented on [`macro@function_pass`]. Runnable end-to-end demos are
//! in the crate's `examples/` (`pass_manager_demo.rs`, `authored_pass.rs`).
//!
//! # Where to look
//!
//! - [`Module`] + [`IRBuilder`] — build and print IR (the crate README has a
//!   guided tour of the builder and typed-handle surface).
//! - [`pass_manager`] — author and run passes (start here for passes).
//! - [`pass_access`] — the capability rungs and the derived verified-state lattice.
//! - [`pass_context`] — the pass-author contexts ([`FnCx`]/[`ModCx`]) and mutators.
//! - [`analysis`] — built-in analyses and the bundled [`Analyses`] manager.
//!
//! The surface is intentionally incomplete: bitcode, a broad built-in transform
//! library, and PassBuilder-style pipeline builders are still ahead.

pub mod align;
pub mod analysis;
pub mod ap_float;
pub mod ap_int;
pub mod argument;
pub mod asm_writer;
pub mod atomic_ordering;
pub mod atomicrmw_binop;
pub mod attribute_mask;
pub mod attributes;
pub mod basic_block;
pub mod block_state;
pub mod calling_conv;
pub mod cfg;
pub mod cmp_predicate;
pub mod comdat;
pub mod constant;
pub mod constant_fold;
pub mod constant_folding;
pub mod constant_range;
pub mod constants;
pub mod data_layout;
pub mod dce;
pub mod debug_loc;
pub mod demanded_bits;
pub mod denormal_mode;
pub mod derived_types;
pub mod dominator_tree;
pub mod error;
pub mod float_kind;
pub mod fmf;
pub mod function;
pub mod function_signature;
pub mod gep_no_wrap_flags;
pub mod global_alias;
pub mod global_ifunc;
pub mod global_value;
pub mod global_variable;
pub mod inline_asm;
pub mod inst_simplify;
pub mod instr_types;
pub mod instruction;
pub mod instructions;
pub mod int_width;
pub mod intrinsic_inst;
pub mod intrinsics;
pub mod ir_builder;
pub mod iter;
pub mod known_bits;
pub(crate) mod llvm_context;
pub mod marker;
pub mod metadata;
pub mod module;
pub mod named_md_node;
pub mod operator;
pub mod optimization_level;
pub mod pass_access;
pub mod pass_context;
pub mod pass_instrumentation;
pub mod pass_manager;
pub mod pass_pipeline;
pub mod phi_state;
pub mod sized_element;
pub mod ssa_builder;
pub mod struct_body_state;
pub mod struct_schema;
pub mod sync_scope;
pub mod target_library_info;
pub mod term_open_state;
pub mod r#type;
pub mod typed_pointer_type;
pub mod typed_pointer_value;
pub mod r#use;
pub mod user;
pub mod value;
pub mod value_symbol_table;
pub mod value_tracking;
pub mod vector_element;
pub mod verifier;

pub mod unnamed_addr;
pub use analysis::{
    AllAnalysesOnFunction, AllAnalysesOnModule, Analyses, AnalysisKeyId, AnalysisSelector,
    AnalysisSetKeyId, CFGAnalyses, FunctionAnalysis, FunctionAnalysisInvalidator,
    FunctionAnalysisList, FunctionAnalysisManager, FunctionAnalysisManagerModuleProxy,
    FunctionAnalysisResult, Idx0, Idx1, Idx2, Idx3, Idx4, Idx5, Idx6, Idx7, ModuleAnalysis,
    ModuleAnalysisInvalidator, ModuleAnalysisList, ModuleAnalysisManager, ModuleAnalysisResult,
    ModuleAnalysisSelector, PreservedAnalyses, PreservedAnalysisChecker,
};
pub use ap_float::{
    ApFloat, ApFloatCategory, ApFloatCmpResult, ApFloatNextDirection, ApFloatSemantics,
    ApFloatSign, ApFloatStatus, Exactness, LosesInfo, NanPayload, RoundingMode,
};
pub use ap_int::{ApInt, ApIntDivRem, ApIntRounding, ApIntSignedness, ApIntTruncation};
pub use argument::Argument;
pub use atomic_ordering::AtomicOrdering;
pub use atomicrmw_binop::AtomicRMWBinOp;
pub use attribute_mask::AttributeMask;
pub use attributes::{
    AttrIndex, AttrKind, Attribute, AttributeList, AttributeSet, AttributeStorage, MemoryEffects,
    MemoryLocation, ModRefInfo,
};
pub use basic_block::{BasicBlock, BasicBlockLabel, IntoBasicBlockLabel};
pub use block_state::{BlockTerminationState, Terminated, Unterminated};
pub use calling_conv::CallingConv;
pub use cfg::{BasicBlockEdge, FunctionCfg};
pub use cmp_predicate::{CmpPredicate, FloatPredicate, IntPredicate};
pub use comdat::{ComdatRef, SelectionKind};
pub use constant::{
    BlockAddressPlaceholder, Constant, ConstantExprFlags, ConstantExprInRange, ConstantExprOpcode,
    ConstantGepFlags, IsConstant, OverflowingConstantExprFlags,
};
pub use constant_fold::{
    constant_fold_binary_instruction, constant_fold_cast_instruction,
    constant_fold_compare_instruction, constant_fold_extract_element_instruction,
    constant_fold_extract_value_instruction, constant_fold_get_element_ptr,
    constant_fold_insert_element_instruction, constant_fold_insert_value_instruction,
    constant_fold_instruction, constant_fold_select_instruction,
    constant_fold_shuffle_vector_instruction, constant_fold_unary_instruction,
    shufflevector_mask_from_constant,
};
pub use constant_folding::{
    ConstantOffsetFromGlobal, FoldNonDeterminism, PreservedCastFlags, can_constant_fold_call_to,
    constant_fold_binary_intrinsic, constant_fold_binary_op_operands, constant_fold_call,
    constant_fold_cast_operand, constant_fold_compare_inst_operands, constant_fold_constant,
    constant_fold_fp_inst_operands, constant_fold_inst_operands, constant_fold_integer_cast,
    constant_fold_load_from_const, constant_fold_load_from_const_ptr,
    constant_fold_load_from_uniform_value, constant_fold_load_through_bitcast,
    constant_fold_unary_op_operand, constant_offset_from_global, flush_fp_constant,
    is_constant_offset_from_global, lossless_inv_cast, lossless_signed_trunc,
    lossless_unsigned_trunc,
};
pub use constant_range::ConstantRange;
pub use constants::{
    ConstantAggregate, ConstantExprOptions, ConstantFloatValue, ConstantIntValue,
    ConstantPointerNull, PoisonValue, UndefValue,
};
pub use data_layout::{
    DataLayout, FunctionPtrAlignType, ManglingMode, PointerSpec, PrimitiveSpec, StructLayoutInfo,
};
pub use dce::DcePass;
pub use debug_loc::DebugLoc;
pub use demanded_bits::{
    DemandedBits, DemandedBitsAnalysis, SimplifyDemandedBitsPass, SimplifyDemandedBitsResult,
    simplify_demanded_bits,
};
pub use denormal_mode::{DenormalMode, DenormalModeKind, DenormalModeSide};
pub use derived_types::{
    AggregateType, AnyTypeEnum, ArrayType, BasicMetadataTypeEnum, BasicTypeEnum, FloatType,
    FunctionType, IntType, LabelType, MetadataType, PointerType, SizedType, StructType,
    TargetExtProperty, TargetExtType, TokenType, VectorType, VoidType,
};
pub use dominator_tree::{DominatorTree, DominatorTreeAnalysis, DominatorTreeBlock};
pub use error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel, VerifierRule};
pub use fmf::FastMathFlags;
pub use function::{FunctionBuilder, FunctionValue};
pub use function_signature::{
    CallArgs, FunctionParam, FunctionParamList, FunctionReturn, FunctionSignature, IntoCallArg,
    TypedFunctionValue, TypedVarArgsFunctionValue,
};
pub use gep_no_wrap_flags::GepNoWrapFlags;
pub use global_alias::{GlobalAlias, GlobalAliasBuilder};
pub use global_ifunc::{GlobalIFunc, GlobalIFuncBuilder};
pub use global_value::{DllStorageClass, DsoLocality, Linkage, ThreadLocalMode, Visibility};
pub use global_variable::{GlobalBuilder, GlobalVariable};
pub use inline_asm::{AsmDialect, InlineAsm, InlineAsmOptions};
pub use inst_simplify::InstSimplifyPass;
pub use instr_types::{
    AShrFlags, AddFlags, AllocaFlags, AtomicCmpXchgConfig, AtomicLoadConfig, AtomicRMWConfig,
    AtomicRMWFlags, AtomicStoreConfig, BinaryOpcode, CallAttributeData, CmpXchgFlags, ICmpFlags,
    LShrFlags, MulFlags, OperandBundleData, OperandBundleTag, OrFlags, OverflowFlags, SDivFlags,
    ShlFlags, SubFlags, TailCallKind, TruncFlags, UDivFlags, UIToFpFlags, UnaryOpcode, ZExtFlags,
};
pub use instruction::{Instruction, InstructionKind, InstructionView, TerminatorKind};
pub use instructions::{
    AShrInst, AddInst, AllocaInst, AndInst, AtomicCmpXchgInst, AtomicRMWInst, BranchInst,
    CallBrInst, CallInst, CastInst, CatchPadInst, CatchReturnInst, CatchSwitchInst, CleanupPadInst,
    CleanupReturnInst, ExtractElementInst, ExtractValueInst, FAddInst, FCmpInst, FDivInst,
    FMulInst, FNegInst, FRemInst, FSubInst, FenceInst, FpPhiInst, FreezeInst, GepInst, ICmpInst,
    IndirectBrInst, InsertElementInst, InsertValueInst, InvokeInst, LShrInst, LandingPadInst,
    LoadInst, MulInst, OrInst, PhiInst, PointerPhiInst, ResumeInst, RetInst, SDivInst, SRemInst,
    SelectInst, ShlInst, ShuffleVectorInst, StoreInst, SubInst, SwitchInst, TypedCallInst,
    UDivInst, URemInst, UnreachableInst, VAArgInst, XorInst,
};
pub use intrinsic_inst::{IntrinsicInst, LifetimeIntrinsic, MemIntrinsic};
pub use intrinsics::{
    BinaryIntrinsic, IntrinsicDescriptor, IntrinsicId, IntrinsicNameResolution,
    descriptor_for_callee, resolve_intrinsic_name,
};
pub use ir_builder::constant_folder::ConstantFolder;
pub use ir_builder::folder::IRBuilderFolder;
pub use ir_builder::no_folder::NoFolder;
pub use ir_builder::{
    BuilderPositionState, CallBuilder, CallSiteConfig, IRBuilder, InsertPoint, Positioned,
    SelectArm, SelectNarrow, Unpositioned,
};
pub use known_bits::KnownBits;
pub use marker::{Dyn, Ptr, ReturnMarker};
pub use metadata::{
    MetadataAttachmentKind, MetadataAttachmentSet, MetadataField, MetadataFieldValue, MetadataId,
    MetadataKind, MetadataRef, SpecializedMetadataKind, SpecializedMetadataNode,
};
pub use module::{
    Brand, ComdatView, GlobalAliasView, GlobalIFuncView, GlobalVariableView, Module, ModuleBrand,
    ModuleId, ModuleRef, ModuleView, Unverified, UseListOrderBBRecord, UseListOrderRecord,
    Verified,
};
pub use operator::OverflowingBinaryOperator;
pub use optimization_level::{
    OptLevelO0, OptLevelO1, OptLevelO2, OptLevelO3, OptLevelOs, OptLevelOz, OptimizationLevel,
    OptimizationLevelMarker, ThinOrFullLtoPhase,
};
pub use pass_access::{
    Downgrades, FnAccess, Inspect, ModAccess, MutatingFn, MutatingModule, PatchBody,
    PipelineVerdict, ReshapeCfg, RewriteModule, StaysVerified, VerdictFold,
};
pub use pass_context::{
    BasicBlockView, FnCx, FnPatch, FnReport, FnReshape, FunctionBody, FunctionView, ModCx,
    ModReport, ModRewrite, ModuleFunctionViews,
};
pub use pass_instrumentation::{PassInstrumentationAnalysis, PassInstrumentationCallbacks};
pub use pass_manager::{
    DynFunctionPipeline, DynModulePipeline, DynReadOnlyFunctionPipeline, DynReadOnlyModulePipeline,
    ForEachFunction, FunctionPass, FunctionPassList, FunctionPipeline, FunctionPipelineExecute,
    FunctionPipelineMember, ModulePass, ModulePassList, ModulePipeline, ModulePipelineExecute,
    ModulePipelineMember, PassExecution, ProvidesToken, ReadOnlyFn, ReadOnlyMod, VerdictCarry,
    for_each_function, function_pipeline, module_pipeline, run_function_pass, run_module_pass,
};
// Data-only textual-pipeline recipe names and AST (not yet executable — they
// mirror LLVM's pipeline strings but construct no passes). Kept importable from
// the crate root, but hidden from the crate-root doc listing so the runnable
// pass-authoring surface stays findable; browse them on the [`pass_pipeline`]
// module page instead.
#[doc(hidden)]
pub use pass_pipeline::{
    BDCE, CLEANUP_LIFT, CLEANUP_MIN, CLEANUP_O1_ISH, DCE, DEFAULT_O0, DEFAULT_O1, EARLY_CSE,
    FunctionPassScope, FunctionPipelineScope, FunctionPipelineStep, GVN_LITE, HasOptimizationLevel,
    INSTCOMBINE, INSTSIMPLIFY, ModulePassScope, ModulePipelineScope, ModulePipelineStep,
    NoOptimizationLevel, PassName, PassPipeline, PassPipelineElement, PassPipelineRecipe,
    PassPipelineTextName, PassScope, PipelineName, PipelineScope, RecipeLevelState, SCCP,
    SIMPLIFYCFG, cleanup_lift_pipeline, cleanup_min_pipeline, cleanup_o1_ish_pipeline,
    default_o0_pipeline, default_o1_pipeline, default_pipeline, parse_pass_pipeline_text,
};
pub use phi_state::{Closed, Open, PhiState};
pub use sized_element::{ArrayDyn, SizedElement};
pub use ssa_builder::{
    FloatVariable, IntVariable, IntoIrResult, PointerVariable, SsaBlock, SsaBuilder, SsaBuilderId,
};
pub use struct_body_state::{BodySet, Opaque, StructBodyDyn, StructBodyState};
pub use struct_schema::{
    FieldOf, IntoIrField, IrField, StructFieldAt, StructFields, StructSchema, StructSchemaValue,
    ValidatedStructValue,
};
pub use sync_scope::SyncScope;
pub use target_library_info::{LibFunc, TargetLibraryInfo};
pub use r#type::{IrType, MAX_INT_BITS, MIN_INT_BITS, Type, TypeId, TypeKind};
pub use typed_pointer_type::TypedPointerType;
pub use typed_pointer_value::TypedPointerValue;
pub use unnamed_addr::UnnamedAddr;
pub use r#use::Use;
pub use user::User;
pub use value::{
    ArrayValue, FloatValue, FunctionTypedValue, HasDebugLoc, HasName, IntValue, IntoPointerValue,
    IsValue, PointerValue, StructValue, Typed, Value, ValueCategory, ValueId, VectorValue,
};
pub use vector_element::{VectorDyn, VectorElement};

pub use align::{Align, MaybeAlign};
pub use float_kind::{
    BFloat, FloatDyn, FloatKind, FloatWiderThan, Fp128, Half, IntoConstantFloat, IntoFloatValue,
    PpcFp128, StaticFloatKind, X86Fp80,
};
// `f32`/`f64` are std types — no re-export needed.

pub use int_width::{
    IntDyn, IntWidth, IntoConstantInt, IntoIntValue, StaticIntWidth, WiderThan, Width,
};
pub use value_tracking::{
    KnownBitsAnalysis, KnownBitsAnalysisResult, MAX_ANALYSIS_RECURSION_DEPTH, ValueTrackingQuery,
    compute_known_bits, is_known_non_zero, is_known_one, is_known_zero, known_bits_from_operator,
};
// `bool`/`i8`/`i16`/`i32`/`i64`/`i128` are std types — no re-export.

#[cfg(feature = "macros")]
pub use llvmkit_macros::{IrStruct, function_pass, module_pass};
