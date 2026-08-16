//! Inline assembly as a `call` callee. Mirrors `class InlineAsm` in
//! `llvm/include/llvm/IR/InlineAsm.h`.
//!
//! ## Representation
//!
//! An inline-asm value is a **context-global** value, just like a
//! [`Function`](crate::function::FunctionValue) or a
//! [`Constant`](crate::constant::Constant): it has no function-local SSA
//! definition and is never assigned a `%N` slot. It lives in the same value
//! arena as globals, constants, and functions.
//!
//! LLVM types an inline-asm value as a **pointer** (the asm "address"),
//! while the *function type* it conceptually wraps is carried separately
//! so a `call` through it knows the argument / return shape. This module
//! follows that split: the [`InlineAsm`] handle's [`Value::ty`] is the
//! module's `ptr` type, and the wrapped [`FunctionType`]
//! id is stored in the payload for the [`IrBuilder`](crate::ir_builder::IrBuilder)
//! to consume when it emits the call.
//!
//! The textual form a `call` prints is, e.g.:
//!
//! ```text
//! %r = call i64 asm sideeffect "add $1, $0", "=r,r,r"(i64 %a, i64 %b)
//! ```

use core::marker::PhantomData;

use super::value::ValueKindData;
use crate::Branded;
use crate::derived_types::FunctionType;
use crate::module::{ModuleBrand, ModuleRef, ModuleView};
use crate::r#type::TypeSlot;
use crate::value::{Value, ValueSlot};

// --------------------------------------------------------------------------
// Assembly dialect
// --------------------------------------------------------------------------

/// Which assembler syntax the template uses. Mirrors
/// `InlineAsm::AsmDialect` in `llvm/include/llvm/IR/InlineAsm.h`.
///
/// In the textual IR, [`AsmDialect::Intel`] adds the `inteldialect`
/// keyword after the `asm` token; [`AsmDialect::Att`] (the default) adds
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AsmDialect {
    /// AT&T syntax (`$0`, `$1`, …; LLVM's default).
    #[default]
    Att,
    /// Intel syntax; prints the `inteldialect` keyword.
    Intel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct InlineAsmOptions {
    has_side_effects: bool,
    is_align_stack: bool,
    dialect: AsmDialect,
    can_unwind: bool,
}

impl InlineAsmOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the asm as having side effects. Default off.
    #[must_use]
    pub fn side_effects(mut self) -> Self {
        self.has_side_effects = true;
        self
    }

    /// Mark the asm as stack-aligning. Default off.
    #[must_use]
    pub fn align_stack(mut self) -> Self {
        self.is_align_stack = true;
        self
    }

    #[must_use]
    pub fn with_dialect(mut self, value: AsmDialect) -> Self {
        self.dialect = value;
        self
    }

    /// Mark the asm as able to unwind (the `.ll` `unwind` keyword).
    /// Default off. Accessor twin: [`Self::can_unwind`].
    #[must_use]
    pub fn unwind(mut self) -> Self {
        self.can_unwind = true;
        self
    }

    pub const fn has_side_effects(&self) -> bool {
        self.has_side_effects
    }

    pub const fn is_align_stack(&self) -> bool {
        self.is_align_stack
    }

    pub const fn dialect(&self) -> AsmDialect {
        self.dialect
    }

    pub const fn can_unwind(&self) -> bool {
        self.can_unwind
    }
}

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload for an inline-asm value. Stored in the value
/// arena under
/// [`ValueKindData::InlineAsm`](crate::value::ValueKindData::InlineAsm).
/// Mirrors the data portion of `class InlineAsm` in
/// `llvm/include/llvm/IR/InlineAsm.h`.
#[derive(Debug)]
pub(crate) struct InlineAsmData {
    /// The assembly template string (the `AsmString` field in LLVM).
    pub(crate) asm_string: String,
    /// The constraint string (the `Constraints` field in LLVM), e.g.
    /// `"=r,r,r"`.
    pub(crate) constraint_string: String,
    /// The conceptual function type of the asm: governs the call's
    /// return type and argument types. LLVM stores this as `FunctionType
    /// *FTy`.
    pub(crate) fn_ty: TypeSlot,
    /// `sideeffect` keyword: the asm has effects not captured by its
    /// outputs. Mirrors `InlineAsm::hasSideEffects()`.
    pub(crate) has_side_effects: bool,
    /// `alignstack` keyword: the asm needs the stack aligned. Mirrors
    /// `InlineAsm::isAlignStack()`.
    pub(crate) is_align_stack: bool,
    /// `unwind` keyword: the asm may unwind. Mirrors
    /// `InlineAsm::canThrow()`.
    pub(crate) can_unwind: bool,
    /// Source syntax of the template. Mirrors `InlineAsm::getDialect()`.
    pub(crate) dialect: AsmDialect,
}

// --------------------------------------------------------------------------
// Public handle
// --------------------------------------------------------------------------

/// Typed handle to an inline-asm value. Mirrors `InlineAsm *` in
/// upstream LLVM.
///
/// The handle's [`Value::ty`] is the module's `ptr` type (LLVM types
/// inline asm as a pointer); the *function type* the asm wraps — which a
/// `call` uses for its return / argument shape — is recovered via
/// [`Self::function_type`].
///
/// Shape mirrors [`GlobalVariable`](crate::global_variable::GlobalVariable)
/// / [`FunctionValue`](crate::function::FunctionValue): a `(ValueSlot,
/// ModuleRef, TypeSlot)` triple plus the cached pointer type.
#[derive(Branded)]
pub struct InlineAsm<'ctx, B: ModuleBrand> {
    pub(crate) id: ValueSlot,
    pub(crate) module: ModuleRef<'ctx, B>,
    /// Cached pointer type id (`ptr`). The value's value-arena type is
    /// this pointer type; the wrapped function type lives in the payload.
    pub(crate) ty: TypeSlot,
    pub(crate) _ctx: PhantomData<&'ctx ()>,
}

impl<'ctx, B: ModuleBrand + 'ctx> core::fmt::Display for InlineAsm<'ctx, B> {
    /// Print the operand form `ptr asm [sideeffect] "<body>",
    /// "<constraints>"` -- the leading `ptr` is the value's IR type,
    /// matching LLVM's pointer typing of inline asm. Identical to what the
    /// erased [`Value`] handle from [`InlineAsm::as_erased`] prints.
    ///
    /// A `call` whose callee is inline asm prints the `asm ...` body
    /// directly in the callee position and so does not go through this.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&InlineAsm::as_erased(*self), f)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> InlineAsm<'ctx, B> {
    /// Construct from raw parts. Crate-internal: only
    /// [`Module::inline_asm`](crate::module::Module::inline_asm) hands
    /// these out, after pushing the value into the arena.
    #[inline]
    pub(crate) fn from_parts<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _ctx: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle. The widened value's type is
    /// the `ptr` type, matching LLVM's pointer typing of inline asm.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// The conceptual function type wrapped by this asm — the signature a
    /// `call` through it must match. Mirrors `InlineAsm::getFunctionType()`.
    #[inline]
    pub fn function_type(self) -> FunctionType<'ctx, B> {
        let fn_ty = self.payload().fn_ty;
        FunctionType::new(fn_ty, self.module)
    }

    /// The assembly template string. Mirrors `InlineAsm::getAsmString()`.
    #[inline]
    pub fn asm_string(self) -> &'ctx str {
        &self.payload().asm_string
    }

    /// The constraint string. Mirrors `InlineAsm::getConstraintString()`.
    #[inline]
    pub fn constraint_string(&self) -> &'ctx str {
        &self.payload().constraint_string
    }
    /// The parsed constraint list. Mirrors
    /// `InlineAsm::ParseConstraints(getConstraintString())`; an unparseable
    /// string yields the empty list, exactly as upstream's own error signal
    /// does, so callers see what `InlineAsm::verify` would have rejected.
    pub fn constraint_info(&self) -> Vec<ConstraintInfo> {
        parse_constraints(self.constraint_string()).unwrap_or_default()
    }

    /// Number of label (`!`) constraints. Mirrors
    /// `InlineAsm::getNumLabels`, which counts parsed constraint records —
    /// not occurrences of `!`, which may also appear inside a `{...}`
    /// register name.
    #[inline]
    pub fn label_constraint_count(&self) -> usize {
        self.constraint_info()
            .iter()
            .filter(|constraint| constraint.kind == ConstraintKind::Label)
            .count()
    }

    /// Check this asm against the function type it wraps. Convenience twin of
    /// the static [`verify_inline_asm`], which is the shape upstream's
    /// `InlineAsm::verify(FunctionType *, StringRef)` has and the one the
    /// parser uses (it validates before constructing anything).
    pub fn verify(&self) -> Result<(), InlineAsmVerifyError> {
        verify_inline_asm(self.function_type(), self.constraint_string())
    }

    /// `true` when the `sideeffect` keyword is set. Mirrors
    /// `InlineAsm::hasSideEffects()`.
    #[inline]
    pub fn has_side_effects(self) -> bool {
        self.payload().has_side_effects
    }

    /// `true` when the `alignstack` keyword is set. Mirrors
    /// `InlineAsm::isAlignStack()`.
    #[inline]
    pub fn is_align_stack(self) -> bool {
        self.payload().is_align_stack
    }

    /// `true` when the `unwind` keyword is set. Mirrors
    /// `InlineAsm::canThrow()`.
    #[inline]
    pub fn can_unwind(self) -> bool {
        self.payload().can_unwind
    }

    /// The template's source dialect. Mirrors `InlineAsm::getDialect()`.
    #[inline]
    pub fn dialect(self) -> AsmDialect {
        self.payload().dialect
    }

    /// Borrow the underlying payload via the module's value arena.
    #[inline]
    fn payload(&self) -> &'ctx InlineAsmData {
        match &self.module.module().context().value_data(self.id).kind {
            ValueKindData::InlineAsm(d) => d,
            _ => unreachable!("InlineAsm handle invariant: kind is InlineAsm"),
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<InlineAsm<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(v: InlineAsm<'ctx, B>) -> Self {
        v.as_erased()
    }
}

// --------------------------------------------------------------------------
// Constraint strings — `InlineAsm::ConstraintInfo` and `ParseConstraints`
// --------------------------------------------------------------------------

/// What a single constraint record constrains. Mirrors
/// `InlineAsm::ConstraintPrefix` (`isInput` / `isOutput` / `isClobber` /
/// `isLabel`), renamed off upstream's predicate-style enumerators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstraintKind {
    /// No prefix: an input operand.
    Input,
    /// `=` prefix: an output operand.
    Output,
    /// `~` prefix: a clobbered register.
    Clobber,
    /// `!` prefix: a branch target (`callbr`).
    Label,
}

/// One alternative of a multiple-alternative constraint (the `|`-separated
/// form). Mirrors `InlineAsm::SubConstraintInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubConstraint {
    /// Index of the input constraint that must match this output, if any.
    /// Upstream spells the absent case `-1`.
    pub matching_input: Option<u32>,
    /// Constraint codes: register names in braces, or constraint letters.
    pub codes: Vec<String>,
}

/// One parsed constraint. Mirrors `InlineAsm::ConstraintInfo`.
///
/// Upstream's `isMultipleAlternative` bool is derived here — it is exactly
/// `!alternatives.is_empty()` — and `currentAlternativeIndex`, a cursor the
/// backends advance during selection, is not ported: nothing in llvmkit
/// selects instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintInfo {
    /// Which of the four prefixes this record carries.
    pub kind: ConstraintKind,
    /// `&` modifier: the output is written before the inputs are consumed.
    pub is_early_clobber: bool,
    /// `%` modifier: this operand and the next may be swapped.
    pub is_commutative: bool,
    /// `*` prefix: the operand is passed by address.
    pub is_indirect: bool,
    /// Index of the input constraint that must match this output, if any.
    pub matching_input: Option<u32>,
    /// Constraint codes, verbatim — including the braces around a register
    /// name. `InlineAsm::verify` reads only counts and kinds, so the codes
    /// are kept as written rather than decoded into a typed vocabulary.
    pub codes: Vec<String>,
    /// The `|`-separated alternatives, empty unless the constraint has any.
    /// When it does, `codes` stays empty and each alternative carries its own.
    pub alternatives: Vec<SubConstraint>,
}

impl ConstraintInfo {
    /// Mirrors `InlineAsm::ConstraintInfo::isMultipleAlternative`, which
    /// upstream stores as a bool set beside `multipleAlternatives`.
    #[must_use]
    pub fn has_alternatives(&self) -> bool {
        !self.alternatives.is_empty()
    }
}

/// Why a constraint string did not parse.
///
/// llvmkit-specific in its *spelling* only: upstream's
/// `ConstraintInfo::Parse` returns a bare `true` from each of these sites and
/// `ParseConstraints` turns that into an empty vector, so no text exists to
/// reproduce. The variants enumerate upstream's rejection sites one for one
/// rather than inventing a taxonomy; the user-visible message stays
/// `InlineAsm::verify`'s `failed to parse constraints`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum ConstraintParseError {
    /// An empty constraint — `",,"`, or a trailing comma.
    #[error("empty constraint")]
    Empty,
    /// A prefix or modifier with no constraint behind it, like `"="` or `"~"`.
    #[error("constraint is only a prefix")]
    PrefixOnly,
    /// `~` not immediately followed by `{`.
    #[error("clobber constraint must name a register")]
    ClobberWithoutRegister,
    /// `&` on something other than an output, or repeated.
    #[error("invalid early-clobber modifier")]
    InvalidEarlyClobber,
    /// `%` on a clobber, or repeated.
    #[error("invalid commutative modifier")]
    InvalidCommutative,
    /// `#` (comment) or `*` (register preferencing) in modifier position —
    /// upstream marks both "not supported".
    #[error("unsupported constraint modifier")]
    UnsupportedModifier,
    /// A `{` with no closing `}`.
    #[error("unterminated register name")]
    UnterminatedRegisterName,
    /// A matching constraint naming a slot that is not an earlier output, or
    /// one that already has a match.
    #[error("invalid matching constraint")]
    InvalidMatchingConstraint,
    /// A `^` or `@` multi-letter code that runs off the end of the string.
    /// Upstream reads past the end here — its two `assert`s are the only
    /// guard, and they compile out — so llvmkit reports instead, turning an
    /// out-of-bounds read into `failed to parse constraints`.
    #[error("malformed multi-letter constraint code")]
    MalformedMultiLetterCode,
}

/// Split a constraint string into records. Port of
/// `InlineAsm::ParseConstraints` (`llvm/lib/IR/InlineAsm.cpp`).
///
/// Upstream signals failure by returning an empty vector, which its one
/// caller then has to disambiguate from a legitimately empty constraint
/// string; here that split is the `Result` (design law 7). An empty input is
/// `Ok(vec![])`, matching upstream's `""` case.
pub fn parse_constraints(constraints: &str) -> Result<Vec<ConstraintInfo>, ConstraintParseError> {
    let mut result: Vec<ConstraintInfo> = Vec::new();
    let mut rest = constraints;
    while !rest.is_empty() {
        let (constraint, tail) = match rest.find(',') {
            Some(comma) => (&rest[..comma], Some(&rest[comma + 1..])),
            None => (rest, None),
        };
        let info = parse_one_constraint(constraint, &mut result)?;
        result.push(info);
        match tail {
            // Upstream's `if (I == E) { Result.clear(); break; }` — a
            // trailing comma is not an empty last constraint, it is a failure.
            Some("") => return Err(ConstraintParseError::Empty),
            Some(tail) => rest = tail,
            None => break,
        }
    }
    Ok(result)
}

/// Port of `InlineAsm::ConstraintInfo::Parse`. `so_far` is upstream's
/// `ConstraintsSoFar`, which a matching constraint reaches back into to
/// record itself on the output it matches.
fn parse_one_constraint(
    constraint: &str,
    so_far: &mut [ConstraintInfo],
) -> Result<ConstraintInfo, ConstraintParseError> {
    // Upstream's caller rejects the empty constraint before calling `Parse`,
    // which then reads `*I` unguarded.
    if constraint.is_empty() {
        return Err(ConstraintParseError::Empty);
    }
    let bytes = constraint.as_bytes();
    let alternative_count = constraint.matches('|').count() + 1;
    let mut alternatives = if alternative_count > 1 {
        vec![SubConstraint::default(); alternative_count]
    } else {
        Vec::new()
    };
    let mut alternative_index = 0usize;
    let mut codes: Vec<String> = Vec::new();

    let mut kind = ConstraintKind::Input;
    let mut is_early_clobber = false;
    let mut is_commutative = false;
    let mut is_indirect = false;
    let mut index = 0usize;

    // Prefixes.
    match bytes[index] {
        b'~' => {
            kind = ConstraintKind::Clobber;
            index += 1;
            // "'{' must immediately follow '~'".
            if index < bytes.len() && bytes[index] != b'{' {
                return Err(ConstraintParseError::ClobberWithoutRegister);
            }
        }
        b'=' => {
            index += 1;
            kind = ConstraintKind::Output;
        }
        b'!' => {
            index += 1;
            kind = ConstraintKind::Label;
        }
        _ => {}
    }
    if index < bytes.len() && bytes[index] == b'*' {
        is_indirect = true;
        index += 1;
    }
    if index == bytes.len() {
        return Err(ConstraintParseError::PrefixOnly);
    }

    // Modifiers.
    loop {
        match bytes[index] {
            b'&' => {
                if kind != ConstraintKind::Output || is_early_clobber {
                    return Err(ConstraintParseError::InvalidEarlyClobber);
                }
                is_early_clobber = true;
            }
            b'%' => {
                if kind == ConstraintKind::Clobber || is_commutative {
                    return Err(ConstraintParseError::InvalidCommutative);
                }
                is_commutative = true;
            }
            // Comment and register preferencing: "Not supported."
            b'#' | b'*' => return Err(ConstraintParseError::UnsupportedModifier),
            _ => break,
        }
        index += 1;
        if index == bytes.len() {
            return Err(ConstraintParseError::PrefixOnly);
        }
    }

    // Codes.
    while index < bytes.len() {
        let push_target: &mut Vec<String> = if alternatives.is_empty() {
            &mut codes
        } else {
            &mut alternatives
                .get_mut(alternative_index)
                .ok_or(ConstraintParseError::MalformedMultiLetterCode)?
                .codes
        };
        match bytes[index] {
            // Physical register reference: `{eax}`.
            b'{' => {
                let end = constraint[index + 1..]
                    .find('}')
                    .ok_or(ConstraintParseError::UnterminatedRegisterName)?
                    + index
                    + 1;
                push_target.push(constraint[index..=end].to_owned());
                index = end + 1;
            }
            // Matching constraint: maximal munch of digits.
            b'0'..=b'9' => {
                let start = index;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                let digits = &constraint[start..index];
                push_target.push(digits.to_owned());
                let matched: usize = digits
                    .parse()
                    .map_err(|_| ConstraintParseError::InvalidMatchingConstraint)?;
                record_matching_input(
                    so_far,
                    matched,
                    kind,
                    alternative_index,
                    alternative_count > 1,
                )?;
            }
            b'|' => {
                alternative_index += 1;
                index += 1;
            }
            // Multi-letter constraint. Upstream's own FIXME pins `^` at two
            // characters; `@` is length-prefixed by one digit.
            b'^' => {
                let start = index + 1;
                let end = start + 2;
                push_target.push(
                    constraint
                        .get(start..end)
                        .ok_or(ConstraintParseError::MalformedMultiLetterCode)?
                        .to_owned(),
                );
                index = end + 1;
            }
            b'@' => {
                let length = usize::from(
                    bytes
                        .get(index + 1)
                        .copied()
                        .filter(u8::is_ascii_digit)
                        .map(|digit| digit - b'0')
                        .filter(|digit| *digit > 0)
                        .ok_or(ConstraintParseError::MalformedMultiLetterCode)?,
                );
                let start = index + 2;
                let end = start
                    .checked_add(length)
                    .ok_or(ConstraintParseError::MalformedMultiLetterCode)?;
                push_target.push(
                    constraint
                        .get(start..end)
                        .ok_or(ConstraintParseError::MalformedMultiLetterCode)?
                        .to_owned(),
                );
                index = end;
            }
            _ => {
                // Single letter constraint. Advance a whole character so a
                // non-ASCII byte cannot split a code point.
                let end = constraint[index..]
                    .char_indices()
                    .nth(1)
                    .map_or(constraint.len(), |(offset, _)| index + offset);
                push_target.push(constraint[index..end].to_owned());
                index = end;
            }
        }
    }

    Ok(ConstraintInfo {
        kind,
        is_early_clobber,
        is_commutative,
        is_indirect,
        matching_input: None,
        codes,
        alternatives,
    })
}

/// The matching-constraint bookkeeping inside `ConstraintInfo::Parse`: a
/// digit code names an earlier *output*, and records this constraint's own
/// index on it. Upstream's `assert(... >= 0)` pair is unspellable here — the
/// index is a `usize` — and drops out.
fn record_matching_input(
    so_far: &mut [ConstraintInfo],
    matched: usize,
    kind: ConstraintKind,
    alternative_index: usize,
    has_alternatives: bool,
) -> Result<(), ConstraintParseError> {
    let this_index =
        u32::try_from(so_far.len()).map_err(|_| ConstraintParseError::InvalidMatchingConstraint)?;
    let Some(target) = so_far.get_mut(matched) else {
        return Err(ConstraintParseError::InvalidMatchingConstraint);
    };
    if target.kind != ConstraintKind::Output || kind != ConstraintKind::Input {
        return Err(ConstraintParseError::InvalidMatchingConstraint);
    }
    if has_alternatives {
        let Some(alternative) = target.alternatives.get_mut(alternative_index) else {
            return Err(ConstraintParseError::InvalidMatchingConstraint);
        };
        if alternative.matching_input.is_some() {
            return Err(ConstraintParseError::InvalidMatchingConstraint);
        }
        alternative.matching_input = Some(this_index);
    } else {
        // An output may already point at *this* constraint (the same digit
        // twice within one constraint); pointing at a different one is the
        // "output constrained to multiple inputs" rejection.
        if target
            .matching_input
            .is_some_and(|existing| existing != this_index)
        {
            return Err(ConstraintParseError::InvalidMatchingConstraint);
        }
        target.matching_input = Some(this_index);
    }
    Ok(())
}

// --------------------------------------------------------------------------
// `InlineAsm::verify`
// --------------------------------------------------------------------------

/// Why an inline-asm constraint string does not agree with its function type.
/// Every message is `InlineAsm::verify`'s own, byte for byte:
/// `LLParser::convertValIDToValue` prints them verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum InlineAsmVerifyError {
    /// The wrapped function type is variadic.
    #[error("inline asm cannot be variadic")]
    Variadic,
    /// `ParseConstraints` rejected a non-empty constraint string.
    #[error("failed to parse constraints")]
    FailedToParseConstraints,
    /// An output constraint appears after a non-indirect input, a clobber, or
    /// a label.
    #[error("output constraint occurs after input, clobber or label constraint")]
    OutputAfterInputClobberOrLabel,
    /// An input constraint appears after a clobber.
    #[error("input constraint occurs after clobber constraint")]
    InputAfterClobber,
    /// A label constraint appears after a clobber.
    #[error("label constraint occurs after clobber constraint")]
    LabelAfterClobber,
    /// No output constraints, but the asm returns something.
    #[error("inline asm without outputs must return void")]
    NoOutputsMustReturnVoid,
    /// One output constraint, but the asm returns a struct.
    #[error("inline asm with one output cannot return struct")]
    OneOutputCannotReturnStruct,
    /// Several output constraints, and the return type is not a struct with
    /// exactly that many fields.
    #[error("number of output constraints does not match number of return struct elements")]
    OutputCountMismatch,
    /// The input constraints and the parameter list disagree in count.
    #[error("number of input constraints does not match number of parameters")]
    InputCountMismatch,
}

/// Check a constraint string against the function type an inline asm wraps.
/// Port of the static `InlineAsm::verify(FunctionType *, StringRef)`.
///
/// `NumLabels` is counted but not checked, exactly as upstream notes: the
/// label count is compared against `callbr`'s indirect destinations by the
/// caller that has them.
pub fn verify_inline_asm<'ctx, B: ModuleBrand + 'ctx>(
    fn_ty: FunctionType<'ctx, B>,
    constraints: &str,
) -> Result<(), InlineAsmVerifyError> {
    if fn_ty.is_var_arg() {
        return Err(InlineAsmVerifyError::Variadic);
    }
    let parsed = parse_constraints(constraints)
        .map_err(|_| InlineAsmVerifyError::FailedToParseConstraints)?;
    if parsed.is_empty() && !constraints.is_empty() {
        return Err(InlineAsmVerifyError::FailedToParseConstraints);
    }

    let mut outputs = 0usize;
    let mut inputs = 0usize;
    let mut clobbers = 0usize;
    let mut indirect = 0usize;
    let mut labels = 0usize;
    for constraint in &parsed {
        match constraint.kind {
            ConstraintKind::Output => {
                if inputs - indirect != 0 || clobbers != 0 || labels != 0 {
                    return Err(InlineAsmVerifyError::OutputAfterInputClobberOrLabel);
                }
                if !constraint.is_indirect {
                    outputs += 1;
                    continue;
                }
                // Upstream falls through to the input arm for an indirect
                // output, after counting it as indirect.
                indirect += 1;
                if clobbers != 0 {
                    return Err(InlineAsmVerifyError::InputAfterClobber);
                }
                inputs += 1;
            }
            ConstraintKind::Input => {
                if clobbers != 0 {
                    return Err(InlineAsmVerifyError::InputAfterClobber);
                }
                inputs += 1;
            }
            ConstraintKind::Clobber => clobbers += 1,
            ConstraintKind::Label => {
                if clobbers != 0 {
                    return Err(InlineAsmVerifyError::LabelAfterClobber);
                }
                labels += 1;
            }
        }
    }

    let return_ty = fn_ty.return_type();
    match outputs {
        0 => {
            if !return_ty.is_void() {
                return Err(InlineAsmVerifyError::NoOutputsMustReturnVoid);
            }
        }
        1 => {
            if return_ty.is_struct() {
                return Err(InlineAsmVerifyError::OneOutputCannotReturnStruct);
            }
        }
        _ => {
            let fields = crate::derived_types::StructType::try_from(return_ty)
                .ok()
                .map(|struct_ty| struct_ty.field_count());
            if fields != Some(outputs) {
                return Err(InlineAsmVerifyError::OutputCountMismatch);
            }
        }
    }

    if fn_ty.params().len() != inputs {
        return Err(InlineAsmVerifyError::InputCountMismatch);
    }
    Ok(())
}
