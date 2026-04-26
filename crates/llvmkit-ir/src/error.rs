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
}

/// Crate-wide `Result` alias.
pub type IrResult<T> = core::result::Result<T, IrError>;
