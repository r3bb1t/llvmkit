#![forbid(unsafe_code)]
//! LLVM IR data model in pure safe Rust.
//!
//! This crate mirrors `llvm/lib/IR/` and `llvm/include/llvm/IR/` from the
//! reference C++ tree (LLVM 22.1.4). One Rust file per C++ translation unit;
//! header-only C++ files map to a Rust file of the same stem.
//!
//! See `local://IR_FOUNDATION_PLAN.md` for the design rationale and
//! per-phase deliverables. The current shipped surface is **Phase A**
//! (types, predicates, flags, calling conventions), **Phase B
//! attributes** subset, the **value-layer foundation**, **minimum
//! function/argument/basic-block/instruction layer** (`add`/`sub`/`mul`/
//! `ret`), and a **minimum [`IRBuilder`]**
//! with type-state insertion-point invariants.

pub mod align;
pub mod argument;
pub mod asm_writer;
pub mod attribute_mask;
pub mod attributes;
pub mod basic_block;
pub mod calling_conv;
pub mod cmp_predicate;
pub mod constant;
pub mod constants;
pub mod debug_loc;
pub mod derived_types;
pub mod error;
pub mod float_kind;
pub mod fmf;
pub mod function;
pub mod gep_no_wrap_flags;
pub mod global_value;
pub mod instr_types;
pub mod instruction;
pub mod instructions;
pub mod int_width;
pub mod ir_builder;
pub mod iter;
pub(crate) mod llvm_context;
pub mod marker;
pub mod module;
pub mod operator;
pub mod r#type;
pub mod typed_pointer_type;
pub mod r#use;
pub mod user;
pub mod value;
pub mod value_symbol_table;
pub mod verifier;

pub mod unnamed_addr;
pub use argument::Argument;
pub use attribute_mask::AttributeMask;
pub use attributes::{AttrIndex, AttrKind, Attribute, AttributeList, AttributeSet};
pub use basic_block::BasicBlock;
pub use calling_conv::CallingConv;
pub use cmp_predicate::{FloatPredicate, IntPredicate};
pub use constant::{Constant, IsConstant};
pub use constants::{
    ConstantAggregate, ConstantFloatValue, ConstantIntValue, ConstantPointerNull, PoisonValue,
    UndefValue,
};
pub use debug_loc::DebugLoc;
pub use derived_types::{
    AggregateType, AnyTypeEnum, ArrayType, BasicMetadataTypeEnum, BasicTypeEnum, FloatType,
    FunctionType, IntType, LabelType, MetadataType, PointerType, SizedType, StructType,
    TargetExtType, TokenType, VectorType, VoidType,
};
pub use error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel, VerifierRule};
pub use fmf::FastMathFlags;
pub use function::{FunctionBuilder, FunctionValue};
pub use gep_no_wrap_flags::GepNoWrapFlags;
pub use global_value::Linkage;
pub use instr_types::{
    AShrFlags, AddFlags, LShrFlags, MulFlags, SDivFlags, ShlFlags, SubFlags, TailCallKind,
    UDivFlags,
};
pub use instruction::{Instruction, InstructionKind, TerminatorKind};
pub use instructions::{
    AShrInst, AddInst, AllocaInst, AndInst, BranchInst, CallInst, CastInst, FAddInst, FCmpInst,
    FDivInst, FMulInst, FRemInst, FSubInst, GepInst, ICmpInst, LShrInst, LoadInst, MulInst, OrInst,
    PhiInst, RetInst, SDivInst, SRemInst, SelectInst, ShlInst, StoreInst, SubInst, UDivInst,
    URemInst, UnreachableInst, XorInst,
};
pub use ir_builder::constant_folder::ConstantFolder;
pub use ir_builder::folder::IRBuilderFolder;
pub use ir_builder::no_folder::NoFolder;
pub use ir_builder::{CallBuilder, IRBuilder, Positioned, SelectArm, Unpositioned};
pub use marker::{Dyn, Ptr, ReturnMarker};
pub use module::{Module, ModuleId, ModuleRef, VerifiedModule};
pub use operator::OverflowingBinaryOperator;
pub use r#type::{IrType, MAX_INT_BITS, MIN_INT_BITS, Type, TypeKind};
pub use typed_pointer_type::TypedPointerType;
pub use unnamed_addr::UnnamedAddr;
pub use r#use::Use;
pub use user::User;
pub use value::{
    ArrayValue, FloatValue, FunctionTypedValue, HasDebugLoc, HasName, IntValue, IntoPointerValue,
    IsValue, PointerValue, StructValue, Typed, Value, ValueCategory, VectorValue,
};

pub use align::{Align, MaybeAlign};
pub use float_kind::{
    BFloat, FloatDyn, FloatKind, FloatWiderThan, Fp128, Half, IntoConstantFloat, IntoFloatValue,
    PpcFp128, StaticFloatKind, X86Fp80,
};
// `f32`/`f64` are std types — no re-export needed.

pub use int_width::{
    IntDyn, IntWidth, IntoConstantInt, IntoIntValue, StaticIntWidth, WiderThan, Width,
};
// `bool`/`i8`/`i16`/`i32`/`i64`/`i128` are std types — no re-export.
