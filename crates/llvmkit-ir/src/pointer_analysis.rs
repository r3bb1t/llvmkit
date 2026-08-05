//! Underlying-object, pointer-base and constant-data analysis.
//!
//! Mirrors the `getUnderlyingObject` / `findAllocaForValue` /
//! `getConstantDataArrayInfo` slice of `llvm/lib/Analysis/ValueTracking.cpp`:
//! given a pointer, peel the address arithmetic off it, name the object it
//! refers to, and — when that object is a constant global — read what is in it.
//!
//! Split out of [`value_tracking`](crate::value_tracking) for the same reason
//! [`speculation`](crate::speculation) is: the question is different in kind.
//! Nothing here computes a bit; every function walks the def-use graph looking
//! for a base, or reads an initializer once it has one.
//!
//! # What is not modeled, and why
//!
//! - [`get_underlying_objects`] takes no `LoopInfo`. Upstream uses it for one
//!   refinement — refusing to look through a loop-header phi whose underlying
//!   object changes every iteration — and llvmkit has no loop analysis, so the
//!   phi is always looked through. That is also upstream's behaviour at its own
//!   default of `LI == nullptr`.
//! - `GlobalValue::isInterposable` also answers true when the module sets
//!   semantic interposition and the value is not `dso_local`. llvmkit models
//!   `dso_local` but not the module-level flag, so only the linkage half is
//!   consulted.
//! - [`find_inserted_value`] has no `InsertBefore`. Upstream's optional
//!   parameter lets it *synthesise* `insertvalue` instructions to rebuild a
//!   sub-aggregate; an analysis that edits the IR it is asked about has no
//!   place here, and `std::nullopt` is upstream's own default.
//! - [`get_constant_data_array_info`] does not port `ReadByteArrayFromGlobal`,
//!   the `ConstantFolding.cpp` helper that re-reads an arbitrary initializer as
//!   bytes. Only initializers that are already an array of the requested
//!   element width, or a zeroinitializer, are read. Declining to read narrows
//!   coverage; it never reads the wrong bytes.

use crate::ApInt;
use crate::attributes::{AttrIndex, AttrKind, AttributeStored};
use crate::constant::{Constant, ConstantData, ConstantExprOpcode};
use crate::data_layout::DataLayout;
use crate::gep_no_wrap_flags::GepNoWrapFlags;
use crate::global_value::Linkage;
use crate::instr_types::{GepInstData, Opcode};
use crate::instruction::{InstructionKindData, InstructionView};
use crate::intrinsics::descriptor_for_callee;
use crate::module::{ModuleBrand, ModuleRef};
use crate::r#type::{Type, TypeData, TypeKind, TypeSlot};
use crate::value::{Value, ValueKindData, ValueSlot};
use crate::value_tracking::value_from_slot;
use std::collections::HashSet;

/// How many layers [`get_underlying_object`] peels before giving up.
///
/// Ports `llvm::MaxLookupSearchDepth` (`ValueTracking.h`), the default of every
/// `MaxLookup` parameter in this family.
pub const MAX_LOOKUP_SEARCH_DEPTH: u32 = 10;

/// How many distinct objects [`get_underlying_object_aggressive`] visits before
/// falling back. Ports its local `MaxVisited`.
const MAX_VISITED_AGGRESSIVE: usize = 8;

// --------------------------------------------------------------------------
// The single-object walk
// --------------------------------------------------------------------------

/// Peel address arithmetic off `value` and return the object underneath.
///
/// Ports `llvm::getUnderlyingObject`. `max_lookup` bounds the walk;
/// [`MAX_LOOKUP_SEARCH_DEPTH`] is upstream's default and `0` means unbounded,
/// exactly as upstream spells it.
///
/// The result is not guaranteed to be an *identifiable* object — the walk stops
/// at whatever it cannot peel, which may be a `load`, an argument, or `value`
/// itself. [`get_underlying_objects_for_code_gen`] is the variant that insists
/// on identifiability.
pub fn get_underlying_object<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    max_lookup: u32,
) -> Value<'ctx, B> {
    let mut current = value;
    let mut count = 0u32;
    while max_lookup == 0 || count < max_lookup {
        count = count.saturating_add(1);
        match peel_one_layer(current) {
            Some(next) => current = next,
            None => return current,
        }
    }
    current
}

/// One step of [`get_underlying_object`]'s loop, or `None` when nothing peels.
fn peel_one_layer<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    // `dyn_cast<GEPOperator>`: a `getelementptr` instruction or the constant
    // expression of the same name. Only a scalar pointer base peels — a vector
    // of pointers is where the walk stops.
    if let Some(pointer) = gep_pointer_operand(value) {
        return is_pointer(pointer.ty()).then_some(pointer);
    }

    // `Operator::getOpcode(V) == BitCast || AddrSpaceCast`.
    if let Some(opcode) = operator_opcode(value)
        && matches!(opcode, Opcode::BitCast | Opcode::AddrSpaceCast)
        && let Some(source) = operator_operand(value, 0)
    {
        return is_pointer(source.ty()).then_some(source);
    }

    if let ValueKindData::GlobalAlias(alias) = &value.data().kind {
        // Only the linkage half of `isInterposable`; see the module docs.
        if is_interposable_linkage(alias.linkage.get()) {
            return None;
        }
        return Some(value_from_slot(value, alias.aliasee.get()));
    }

    match instruction_kind(value) {
        // A single-argument phi is the shape LCSSA leaves behind.
        Some(InstructionKindData::Phi(data)) => {
            let incoming = data.incoming.borrow();
            let [(operand, _)] = incoming.as_slice() else {
                return None;
            };
            Some(value_from_slot(value, operand.get()))
        }
        // `CaptureTracking` knows capture properties of some intrinsics —
        // `launder.invariant.group` and friends — that attributes cannot
        // express. Going through the shared helper is what keeps the two in
        // sync; disagreeing would let two aliasing pointers be assumed
        // `noalias`.
        Some(
            InstructionKindData::Call(_)
            | InstructionKindData::Invoke(_)
            | InstructionKindData::CallBr(_),
        ) => argument_aliasing_to_returned_pointer_impl(value, false),
        _ => None,
    }
}

/// Try harder than [`get_underlying_object`] to name a *single* object,
/// following `select` and `phi` when every path agrees.
///
/// Ports `llvm::getUnderlyingObjectAggressive`. When the paths disagree, or
/// more than eight distinct objects turn up, the answer falls back to
/// `get_underlying_object(value)`.
pub fn get_underlying_object_aggressive<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Value<'ctx, B> {
    let first_object = get_underlying_object(value, MAX_LOOKUP_SEARCH_DEPTH);
    let mut visited: HashSet<ValueSlot> = HashSet::new();
    let mut worklist = vec![value];
    let mut object: Option<Value<'ctx, B>> = None;
    let mut first = true;

    while let Some(candidate) = worklist.pop() {
        let candidate = if first {
            first = false;
            first_object
        } else {
            get_underlying_object(candidate, MAX_LOOKUP_SEARCH_DEPTH)
        };

        if !visited.insert(candidate.slot()) {
            continue;
        }
        if visited.len() == MAX_VISITED_AGGRESSIVE {
            return first_object;
        }

        match instruction_kind(candidate) {
            Some(InstructionKindData::Select(data)) => {
                worklist.push(value_from_slot(candidate, data.true_val.get()));
                worklist.push(value_from_slot(candidate, data.false_val.get()));
                continue;
            }
            Some(InstructionKindData::Phi(data)) => {
                let incoming = data.incoming.borrow();
                worklist.extend(
                    incoming
                        .iter()
                        .map(|(operand, _)| value_from_slot(candidate, operand.get())),
                );
                continue;
            }
            _ => {}
        }

        match object {
            None => object = Some(candidate),
            Some(known) if known.slot() != candidate.slot() => return first_object,
            Some(_) => {}
        }
    }

    object.unwrap_or(first_object)
}

/// Every object `value` may refer to, following `select` and `phi`.
///
/// Ports `llvm::getUnderlyingObjects`. Where
/// [`get_underlying_object_aggressive`] gives up on disagreement, this collects
/// each answer: given `select %c, ptr %a, ptr %b` it returns both.
pub fn get_underlying_objects<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    max_lookup: u32,
) -> Vec<Value<'ctx, B>> {
    let mut objects = Vec::new();
    let mut visited: HashSet<ValueSlot> = HashSet::new();
    let mut worklist = vec![value];

    while let Some(candidate) = worklist.pop() {
        let candidate = get_underlying_object(candidate, max_lookup);
        if !visited.insert(candidate.slot()) {
            continue;
        }
        match instruction_kind(candidate) {
            Some(InstructionKindData::Select(data)) => {
                worklist.push(value_from_slot(candidate, data.true_val.get()));
                worklist.push(value_from_slot(candidate, data.false_val.get()));
            }
            Some(InstructionKindData::Phi(data)) => {
                // Upstream declines this when `LI` says the phi heads a loop
                // whose object changes each iteration; see the module docs.
                let incoming = data.incoming.borrow();
                worklist.extend(
                    incoming
                        .iter()
                        .map(|(operand, _)| value_from_slot(candidate, operand.get())),
                );
            }
            _ => objects.push(candidate),
        }
    }
    objects
}

/// [`get_underlying_objects`] plus `inttoptr` chasing, insisting every result
/// is an identifiable object.
///
/// Ports `llvm::getUnderlyingObjectsForCodeGen`. Upstream returns a `bool` and
/// clears its out-parameter on failure; here failure is `None`, so a caller
/// cannot read a half-filled list.
pub fn get_underlying_objects_for_code_gen<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Vec<Value<'ctx, B>>> {
    let mut objects = Vec::new();
    let mut visited: HashSet<ValueSlot> = HashSet::new();
    let mut working = vec![value];

    while let Some(current) = working.pop() {
        for object in get_underlying_objects(current, MAX_LOOKUP_SEARCH_DEPTH) {
            if !visited.insert(object.slot()) {
                continue;
            }
            if operator_opcode(object) == Some(Opcode::IntToPtr)
                && let Some(operand) = operator_operand(object, 0)
            {
                let base = underlying_object_from_int(operand);
                if is_pointer(base.ty()) {
                    working.push(base);
                    continue;
                }
            }
            // If no identifiable object can be named, this fails too, for
            // safety.
            if !is_identified_object(object) {
                return None;
            }
            objects.push(object);
        }
    }
    Some(objects)
}

/// Express `pointer` as a base plus a constant byte offset.
///
/// Ports `llvm::GetPointerBaseWithConstantOffset`, itself a wrapper around
/// `Value::stripAndAccumulateConstantOffsets` that unpacks the `APInt` into an
/// `int64_t`. Upstream writes the offset through a reference; here it is the
/// second half of the pair, so a base and the offset it belongs to cannot drift
/// apart.
///
/// `allow_non_inbounds` is upstream's parameter: with it clear, only `inbounds`
/// `getelementptr`s peel.
pub fn pointer_base_with_constant_offset<'ctx, B: ModuleBrand + 'ctx>(
    pointer: Value<'ctx, B>,
    data_layout: &DataLayout,
    allow_non_inbounds: bool,
) -> (Value<'ctx, B>, i64) {
    let index_bits = index_type_size_in_bits(pointer.ty(), data_layout);
    let (base, offset) =
        strip_and_accumulate_offset(pointer, index_bits, allow_non_inbounds, data_layout);
    (base, offset.try_sext_i64().unwrap_or(0))
}

/// The base and accumulated offset [`pointer_base_with_constant_offset`]
/// reports, kept as an `ApInt` for the in-crate callers that need the full
/// width.
fn strip_and_accumulate_offset<'ctx, B: ModuleBrand + 'ctx>(
    pointer: Value<'ctx, B>,
    index_bits: u32,
    allow_non_inbounds: bool,
    data_layout: &DataLayout,
) -> (Value<'ctx, B>, ApInt) {
    let mut offset = ApInt::zero(index_bits);
    let mut current = pointer;
    // Bounded the same way the underlying-object walk is; upstream relies on
    // the operand graph being acyclic through GEP pointers instead.
    for _ in 0..MAX_LOOKUP_SEARCH_DEPTH {
        let Some((base, step)) =
            peel_one_constant_offset(current, index_bits, allow_non_inbounds, data_layout)
        else {
            break;
        };
        offset = offset.wrapping_add(&step);
        current = base;
    }
    (current, offset)
}

/// The `alloca` `value` ultimately refers to, when there is exactly one.
///
/// Ports `llvm::findAllocaForValue`. With `offset_zero` set, a `getelementptr`
/// only peels when every index is zero, so the answer names the start of the
/// object rather than somewhere inside it.
///
/// `None` covers both of upstream's failure spellings — no alloca found, and
/// more than one found — because neither hands the caller an alloca.
pub fn find_alloca_for_value<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    offset_zero: bool,
) -> Option<Value<'ctx, B>> {
    let mut result: Option<Value<'ctx, B>> = None;
    let mut visited: HashSet<ValueSlot> = HashSet::new();
    let mut worklist = Vec::new();
    visited.insert(value.slot());
    worklist.push(value);

    while let Some(current) = worklist.pop() {
        let mut pending: Vec<Value<'ctx, B>> = Vec::new();
        match instruction_kind(current)? {
            InstructionKindData::Alloca(_) => match result {
                Some(known) if known.slot() != current.slot() => return None,
                _ => result = Some(current),
            },
            InstructionKindData::Cast(data) => {
                pending.push(value_from_slot(current, data.src.get()));
            }
            InstructionKindData::Phi(data) => {
                let incoming = data.incoming.borrow();
                pending.extend(
                    incoming
                        .iter()
                        .map(|(operand, _)| value_from_slot(current, operand.get())),
                );
            }
            InstructionKindData::Select(data) => {
                pending.push(value_from_slot(current, data.true_val.get()));
                pending.push(value_from_slot(current, data.false_val.get()));
            }
            InstructionKindData::Gep(data) => {
                if offset_zero && !gep_has_all_zero_indices(current, data) {
                    return None;
                }
                pending.push(value_from_slot(current, data.ptr.get()));
            }
            InstructionKindData::Call(_)
            | InstructionKindData::Invoke(_)
            | InstructionKindData::CallBr(_) => {
                // A call only continues the walk through a `returned` argument;
                // anything else could have produced the pointer from nowhere.
                pending.push(returned_argument(current)?);
            }
            _ => return None,
        }
        for candidate in pending {
            if visited.insert(candidate.slot()) {
                worklist.push(candidate);
            }
        }
    }

    result
}

// --------------------------------------------------------------------------
// Use-side predicates
// --------------------------------------------------------------------------

/// Whether every user of `value` is a lifetime marker.
///
/// Ports `llvm::onlyUsedByLifetimeMarkers`.
pub fn only_used_by_lifetime_markers<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    only_used_by_markers(value, true, false)
}

/// Whether every user of `value` is a lifetime marker or a droppable
/// instruction.
///
/// Ports `llvm::onlyUsedByLifetimeMarkersOrDroppableInsts`. Droppable is
/// `User::isDroppable` (`llvm/lib/IR/User.cpp`): `@llvm.assume`,
/// `@llvm.pseudoprobe` and `@llvm.experimental.noalias.scope.decl`, the
/// intrinsics a transform may delete rather than update.
pub fn only_used_by_lifetime_markers_or_droppable_instructions<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> bool {
    only_used_by_markers(value, true, true)
}

/// Ports the static `onlyUsedByLifetimeMarkersOrDroppableInstsHelper`.
fn only_used_by_markers<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    allow_lifetime: bool,
    allow_droppable: bool,
) -> bool {
    value.users().all(|user| {
        let Some(name) = called_intrinsic_name(user.to_erased()) else {
            return false;
        };
        if allow_lifetime && matches!(name, "llvm.lifetime.start" | "llvm.lifetime.end") {
            return true;
        }
        allow_droppable
            && matches!(
                name,
                "llvm.assume" | "llvm.pseudoprobe" | "llvm.experimental.noalias.scope.decl"
            )
    })
}

// --------------------------------------------------------------------------
// Call-return aliasing
// --------------------------------------------------------------------------

/// The call argument aliasing rules treat as the same object as the result.
///
/// Ports `llvm::getArgumentAliasingToReturnedPointer`. As upstream's comment
/// warns, this is an *aliasing* property: it says two values name the same
/// object, not that one may be substituted for the other.
pub fn argument_aliasing_to_returned_pointer<'ctx, B: ModuleBrand + 'ctx>(
    call: &InstructionView<'ctx, B>,
    must_preserve_nullness: bool,
) -> Option<Value<'ctx, B>> {
    argument_aliasing_to_returned_pointer_impl(call.to_erased(), must_preserve_nullness)
}

fn argument_aliasing_to_returned_pointer_impl<'ctx, B: ModuleBrand + 'ctx>(
    call: Value<'ctx, B>,
    must_preserve_nullness: bool,
) -> Option<Value<'ctx, B>> {
    if let Some(returned) = returned_argument(call) {
        return Some(returned);
    }
    if !intrinsic_returns_aliasing_argument(call, must_preserve_nullness) {
        return None;
    }
    call_argument(call, 0)
}

/// Whether the intrinsic `call` invokes returns a pointer aliasing its first
/// argument, capturing it only by returning it.
///
/// Ports `llvm::isIntrinsicReturningPointerAliasingArgumentWithoutCapturing`.
/// These intrinsics are not marked `nocapture` (returning counts as capture)
/// and their arguments are not marked `returned` (that would make them
/// useless), so the property has to be spelled out.
pub fn is_intrinsic_returning_pointer_aliasing_argument_without_capturing<
    'ctx,
    B: ModuleBrand + 'ctx,
>(
    call: &InstructionView<'ctx, B>,
    must_preserve_nullness: bool,
) -> bool {
    intrinsic_returns_aliasing_argument(call.to_erased(), must_preserve_nullness)
}

fn intrinsic_returns_aliasing_argument<'ctx, B: ModuleBrand + 'ctx>(
    call: Value<'ctx, B>,
    must_preserve_nullness: bool,
) -> bool {
    let Some(name) = called_intrinsic_name(call) else {
        return false;
    };
    match name {
        "llvm.launder.invariant.group"
        | "llvm.strip.invariant.group"
        | "llvm.aarch64.irg"
        | "llvm.aarch64.tagp"
        // `amdgcn.make.buffer.rsrc` does not alter the address, so it preserves
        // null-ness for escape analysis. It does not necessarily map
        // `ptr addrspace(N) null` to the addrspace(8) null descriptor, which
        // upstream documents out of caution rather than acts on.
        | "llvm.amdgcn.make.buffer.rsrc" => true,
        "llvm.ptrmask" => !must_preserve_nullness,
        // Upstream answers `!isPresplitCoroutine()`: the underlying variable
        // changes with the thread id, and the thread id can change at a
        // coroutine suspend point. llvmkit models no presplit-coroutine flag,
        // so this declines — the conservative direction.
        "llvm.threadlocal.address" => false,
        _ => false,
    }
}

// --------------------------------------------------------------------------
// Reading constant data
// --------------------------------------------------------------------------

/// A window into a constant array: an offset and a length.
///
/// Ports `llvm::ConstantDataArraySlice`. Upstream's `Array` field is a
/// `ConstantDataArray *`, the compact representation LLVM uses for an array of
/// primitive constants; llvmkit has no such specialisation — an array constant
/// is one aggregate constant whichever way its elements were written — so
/// [`Self::array`] is the aggregate constant itself.
///
/// A `None` array is upstream's null pointer: a zeroinitializer, which is a
/// perfectly good initializer that simply does not fit the `ConstantDataArray`
/// interface. [`Self::element`] reads `0` from it, as upstream's `operator[]`
/// does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantDataArraySlice<'ctx, B: ModuleBrand> {
    array: Option<Value<'ctx, B>>,
    offset: u64,
    length: u64,
}

impl<'ctx, B: ModuleBrand + 'ctx> ConstantDataArraySlice<'ctx, B> {
    /// The backing array constant, or `None` for a zeroinitializer.
    pub fn array(&self) -> Option<Value<'ctx, B>> {
        self.array
    }

    /// Where the slice starts, in elements.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// How many elements the slice covers.
    pub fn len(&self) -> u64 {
        self.length
    }

    /// Whether the slice covers no elements.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Move the start forward by `delta` elements, shortening the slice.
    ///
    /// Ports `ConstantDataArraySlice::move`, whose `assert(Delta < Length)`
    /// guards a caller precondition; here that precondition is the `None`.
    #[must_use]
    pub fn moved(self, delta: u64) -> Option<Self> {
        if delta >= self.length {
            return None;
        }
        Some(Self {
            array: self.array,
            offset: self.offset + delta,
            length: self.length - delta,
        })
    }

    /// Element `index` of the slice, as an unsigned integer.
    ///
    /// Ports `ConstantDataArraySlice::operator[]`, which reads
    /// `Array->getElementAsInteger(I + Offset)` and answers `0` for a
    /// zeroinitializer. Upstream does not bounds-check; `None` here is an index
    /// past the end, which upstream would read out of range.
    pub fn element(&self, index: u64) -> Option<u64> {
        if index >= self.length {
            return None;
        }
        let Some(array) = self.array else {
            return Some(0);
        };
        let position = usize::try_from(index.checked_add(self.offset)?).ok()?;
        let ValueKindData::Constant(ConstantData::Aggregate(elements)) = &array.data().kind else {
            return None;
        };
        let element = value_from_slot(array, *elements.get(position)?);
        match &element.data().kind {
            ValueKindData::Constant(ConstantData::Int(words)) => {
                Some(words.first().copied().unwrap_or(0))
            }
            // A `zeroinitializer` element inside an aggregate.
            ValueKindData::Constant(ConstantData::PointerNull) => Some(0),
            _ => None,
        }
    }
}

/// Read the constant array `value` points into.
///
/// Ports `llvm::getConstantDataArrayInfo`. `element_size` is in *bits* and must
/// be a multiple of 8 — upstream asserts that; here a size that is not is
/// `None`, so the precondition cannot be violated silently.
///
/// `offset` is upstream's starting element offset, added to whatever the
/// pointer arithmetic contributes.
pub fn get_constant_data_array_info<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    element_size: u32,
    offset: u64,
    data_layout: &DataLayout,
) -> Option<ConstantDataArraySlice<'ctx, B>> {
    if element_size == 0 || !element_size.is_multiple_of(8) {
        return None;
    }
    let element_size_in_bytes = u64::from(element_size / 8);

    // Drill down through the pointer expression, ignoring intervening casts,
    // and identify the object it references.
    let global = get_underlying_object(value, MAX_LOOKUP_SEARCH_DEPTH);
    let ValueKindData::GlobalVariable(data) = &global.data().kind else {
        return None;
    };
    if !data.is_constant {
        return None;
    }
    // `hasDefinitiveInitializer`: an initializer the linker cannot replace.
    let initializer = data.initializer.get()?;
    if is_interposable_linkage(data.linkage.get()) || data.externally_initialized.get() {
        return None;
    }
    let initializer = value_from_slot(global, initializer);

    // The offset from the global has to be a constant, and the walk has to land
    // on the global itself.
    let index_bits = index_type_size_in_bits(value.ty(), data_layout);
    let (base, byte_offset) = strip_and_accumulate_offset(value, index_bits, true, data_layout);
    if base.slot() != global.slot() {
        return None;
    }
    let start_index = byte_offset.limited_value(u64::MAX);
    if start_index == u64::MAX {
        // An excessive constant offset.
        return None;
    }
    // The offset is in bytes; convert to elements or give up.
    if !start_index.is_multiple_of(element_size_in_bytes) {
        return None;
    }
    let offset = offset.checked_add(start_index / element_size_in_bytes)?;

    let value_type = Type::new(data.value_type, module_ref(global));

    if is_null_constant(initializer) {
        let size_in_bytes = data_layout.type_store_size(value_type);
        let length = size_in_bytes / element_size_in_bytes;
        // An empty slice for an undersized constant, so callers can still fold
        // an otherwise-undefined library call into a well-defined expression.
        return Some(ConstantDataArraySlice {
            array: None,
            offset: 0,
            length: length.saturating_sub(offset),
        });
    }

    // Upstream additionally re-reads an arbitrary initializer as bytes via
    // `ReadByteArrayFromGlobal` when `ElementSize == 8`; see the module docs.
    let (element_type, element_count) = value_type.data().as_array()?;
    let element_type = Type::new(element_type, module_ref(global));
    if element_type.kind() != (TypeKind::Integer { bits: element_size }) {
        return None;
    }
    if !matches!(
        &initializer.data().kind,
        ValueKindData::Constant(ConstantData::Aggregate(_))
    ) {
        return None;
    }
    if offset > element_count {
        return None;
    }
    Some(ConstantDataArraySlice {
        array: Some(initializer),
        offset,
        length: element_count - offset,
    })
}

/// Read the constant C string `value` points to.
///
/// Ports `llvm::getConstantStringInfo`. `trim_at_nul` drops the terminator and
/// everything after it, which is upstream's default.
///
/// Upstream writes a `StringRef` into the caller's buffer and returns a
/// `bool`; here the string is the `Some`. It is a `Vec<u8>` rather than a
/// `String` because the bytes are IR data and need not be UTF-8.
pub fn get_constant_string_info<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    trim_at_nul: bool,
    data_layout: &DataLayout,
) -> Option<Vec<u8>> {
    let slice = get_constant_data_array_info(value, 8, 0, data_layout)?;

    let Some(_) = slice.array() else {
        if trim_at_nul {
            // A nul-terminated empty string, even for an empty slice. Every
            // caller requires a string argument and the functions they fold are
            // undefined otherwise, so folding this way beats making the call.
            return Some(Vec::new());
        }
        if slice.len() == 1 {
            return Some(vec![0]);
        }
        // No run of zero bytes to hand back.
        return None;
    };

    let mut bytes = Vec::new();
    for index in 0..slice.len() {
        let byte = u8::try_from(slice.element(index)? & 0xff).ok()?;
        if trim_at_nul && byte == 0 {
            // Trim the terminator and anything after it. An unterminated array
            // yields the whole tail; the caller may know its length another way.
            return Some(bytes);
        }
        bytes.push(byte);
    }
    Some(bytes)
}

/// The length of the constant C string `value` points to, plus one.
///
/// Ports `llvm::GetStringLength`, including its `+1`: the answer counts the
/// terminator, so an empty string is `1`. Upstream returns `0` for "cannot
/// tell"; that is `None` here, so a caller cannot mistake it for a length.
///
/// `char_size` is in bits and defaults to 8 upstream.
pub fn get_string_length<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    char_size: u32,
    data_layout: &DataLayout,
) -> Option<u64> {
    if !is_pointer(value.ty()) {
        return None;
    }
    let mut phis: HashSet<ValueSlot> = HashSet::new();
    match string_length_recursive(value, &mut phis, char_size, data_layout) {
        // Upstream's `~0ULL` means an infinite phi cycle: dead code, reported
        // as the empty string.
        StringLength::Cyclic => Some(1),
        StringLength::Known(length) => Some(length),
        StringLength::Unknown => None,
    }
}

/// The three answers upstream's `GetStringLengthH` packs into a `uint64_t`:
/// `0` for unknown, `~0ULL` for "already visiting this phi", anything else for
/// a length.
enum StringLength {
    Unknown,
    Cyclic,
    Known(u64),
}

/// Ports the static `GetStringLengthH`.
fn string_length_recursive<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    phis: &mut HashSet<ValueSlot>,
    char_size: u32,
    data_layout: &DataLayout,
) -> StringLength {
    // Look through no-op pointer casts.
    let value = strip_pointer_casts(value);

    if let Some(InstructionKindData::Phi(data)) = instruction_kind(value) {
        if !phis.insert(value.slot()) {
            return StringLength::Cyclic;
        }
        // See whether every incoming string has the same length.
        let incoming: Vec<ValueSlot> = data
            .incoming
            .borrow()
            .iter()
            .map(|(operand, _)| operand.get())
            .collect();
        let mut so_far: Option<u64> = None;
        for operand in incoming {
            match string_length_recursive(
                value_from_slot(value, operand),
                phis,
                char_size,
                data_layout,
            ) {
                StringLength::Unknown => return StringLength::Unknown,
                StringLength::Cyclic => continue,
                StringLength::Known(length) => match so_far {
                    Some(known) if known != length => return StringLength::Unknown,
                    _ => so_far = Some(length),
                },
            }
        }
        return match so_far {
            Some(length) => StringLength::Known(length),
            None => StringLength::Cyclic,
        };
    }

    // `strlen(select(c, x, y))` is a length only when both arms agree.
    if let Some(InstructionKindData::Select(data)) = instruction_kind(value) {
        let true_value = value_from_slot(value, data.true_val.get());
        let false_value = value_from_slot(value, data.false_val.get());
        let first = string_length_recursive(true_value, phis, char_size, data_layout);
        if matches!(first, StringLength::Unknown) {
            return StringLength::Unknown;
        }
        let second = string_length_recursive(false_value, phis, char_size, data_layout);
        return match (first, second) {
            (_, StringLength::Unknown) => StringLength::Unknown,
            (StringLength::Cyclic, other) => other,
            (other, StringLength::Cyclic) => other,
            (StringLength::Known(a), StringLength::Known(b)) if a == b => StringLength::Known(a),
            _ => StringLength::Unknown,
        };
    }

    // Otherwise, try to read the string.
    let Some(slice) = get_constant_data_array_info(value, char_size, 0, data_layout) else {
        return StringLength::Unknown;
    };
    if slice.array().is_none() {
        // A zeroinitializer, including an empty one.
        return StringLength::Known(1);
    }
    // Find the first nul. A conservative answer even without one is safe: the
    // string function being folded is otherwise undefined, and folding beats
    // making the undefined call.
    let mut null_index = 0u64;
    while null_index < slice.len() {
        if slice.element(null_index) == Some(0) {
            break;
        }
        null_index += 1;
    }
    StringLength::Known(null_index + 1)
}

/// The `i8` value `value` is made of, when it can be built by repeating one
/// byte in memory.
///
/// Ports `llvm::isBytewiseValue`. True of every `i8` obviously, but also of
/// `i32 0`, `i32 -1`, `i16 0xF0F0` and `double 0.0`; `i16 0x1234` is `None`.
///
/// Upstream returns a `Value *` — either the original `i8`, or a freshly minted
/// `i8` constant, or the `i8 undef` that means "entirely undef and padding".
/// Building a constant is a module mutation, so this returns the *byte* instead
/// and spells undef as [`BytewiseValue::AnyByte`]. The one case upstream can
/// return a non-constant, an `i8`-typed value, is [`BytewiseValue::Value`].
pub fn is_bytewise_value<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data_layout: &DataLayout,
) -> Option<BytewiseValue<'ctx, B>> {
    // Undef does not care which byte.
    //
    // Ordered before the `i8` arm, where upstream orders it after. Upstream can
    // afford the other order because LLVM uniques constants: its first arm
    // hands back `V`, and for an `i8 undef` that `V` *is* the `UndefInt8`
    // sentinel every later `Merge` compares against by pointer. llvmkit has no
    // such identity, so the sentinel is named up front instead. The two agree
    // on every answer, including the printed one — `AnyByte` is what upstream's
    // `UndefValue::get(i8)` prints as.
    if matches!(
        &value.data().kind,
        ValueKindData::Constant(ConstantData::Undef)
    ) {
        return Some(BytewiseValue::AnyByte);
    }

    // All byte-wide stores are splatable, even of an arbitrary variable.
    if value.ty().kind() == (TypeKind::Integer { bits: 8 }) {
        return Some(BytewiseValue::Value(value));
    }

    // Poison for a zero-sized type.
    if data_layout.type_store_size(value.ty()) == 0 {
        return Some(BytewiseValue::AnyByte);
    }

    let ValueKindData::Constant(constant) = &value.data().kind else {
        // A non-constant could in principle be recognised —
        //   %a = zext i8 %X to i16 / %b = shl i16 %a, 8 / %c = or i16 %a, %b
        // — but upstream does not bother without a motivating case, and neither
        // does this.
        return None;
    };

    // `null`, `zeroinitializer` and friends.
    if is_null_constant(value) {
        return Some(BytewiseValue::Byte(0));
    }

    match constant {
        // A float is byteable exactly when its bit pattern is.
        ConstantData::Float(bits) => {
            let width = float_bit_width(value.ty())?;
            // Upstream declines the long-double formats, whose constraints are
            // strange; those are the widths `float_bit_width` returns `None`
            // for.
            splat_byte(&ApInt::from_words(width, &to_words(*bits))).map(BytewiseValue::Byte)
        }
        // Any integer whose width is a multiple of 8 and whose bytes agree.
        ConstantData::Int(words) => {
            let TypeKind::Integer { bits } = value.ty().kind() else {
                return None;
            };
            if !bits.is_multiple_of(8) {
                return None;
            }
            splat_byte(&ApInt::from_words(bits, words)).map(BytewiseValue::Byte)
        }
        // Every element has to agree, with undef matching anything.
        ConstantData::Aggregate(elements) => {
            let mut merged = BytewiseValue::AnyByte;
            for element in elements.iter() {
                let element = value_from_slot(value, *element);
                merged = merge_bytewise(merged, is_bytewise_value(element, data_layout)?)?;
            }
            Some(merged)
        }
        _ => None,
    }
}

/// What [`is_bytewise_value`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BytewiseValue<'ctx, B: ModuleBrand> {
    /// A concrete byte. Upstream mints `ConstantInt::get(i8, byte)`.
    Byte(u8),
    /// Every byte is undef or padding, so any byte will do. Upstream's
    /// `UndefValue::get(i8)`.
    AnyByte,
    /// An `i8`-typed value that is not a constant — upstream returns `V` itself
    /// from the first arm.
    Value(Value<'ctx, B>),
}

/// Ports the `Merge` lambda inside `isBytewiseValue`.
fn merge_bytewise<'ctx, B: ModuleBrand + 'ctx>(
    lhs: BytewiseValue<'ctx, B>,
    rhs: BytewiseValue<'ctx, B>,
) -> Option<BytewiseValue<'ctx, B>> {
    match (lhs, rhs) {
        (BytewiseValue::AnyByte, other) | (other, BytewiseValue::AnyByte) => Some(other),
        (BytewiseValue::Byte(a), BytewiseValue::Byte(b)) if a == b => Some(BytewiseValue::Byte(a)),
        (BytewiseValue::Value(a), BytewiseValue::Value(b)) if a.slot() == b.slot() => {
            Some(BytewiseValue::Value(a))
        }
        _ => None,
    }
}

/// The scalar already sitting in `value` at `indices`, if one is.
///
/// Ports `llvm::FindInsertedValue` at its default `InsertBefore = nullopt`:
/// given `insertvalue`/`extractvalue` chains and constant aggregates, follow
/// the indices to whatever register already holds that element. `None` when the
/// element is not separately available — a `load` of a struct, say, or a
/// function's aggregate return.
///
/// Upstream's `InsertBefore` variant additionally *builds* `insertvalue`
/// instructions to reassemble a sub-aggregate; see the module docs for why that
/// half is absent.
pub fn find_inserted_value<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    indices: &[u32],
) -> Option<Value<'ctx, B>> {
    // Nothing to index? The value itself — useful at the end of the recursion.
    let Some((&first, rest)) = indices.split_first() else {
        return Some(value);
    };

    if let ValueKindData::Constant(_) = &value.data().kind {
        // `C->getAggregateElement(idx)`.
        let element = Constant::from_parts(value).aggregate_element(first)?;
        return find_inserted_value(element.into_erased(), rest);
    }

    match instruction_kind(value)? {
        InstructionKindData::InsertValue(data) => {
            // Walk the insertvalue's indices alongside the requested ones.
            for (position, own) in data.indices.iter().enumerate() {
                let Some(requested) = indices.get(position) else {
                    // The request names part of a nested aggregate. Rebuilding
                    // it needs new `insertvalue`s, which this does not do.
                    return None;
                };
                if requested != own {
                    // This insertvalue writes somewhere else; look in the
                    // aggregate it wrote into.
                    return find_inserted_value(
                        value_from_slot(value, data.aggregate.get()),
                        indices,
                    );
                }
            }
            // The indices match, possibly only partially; continue into the
            // inserted value with whatever is left.
            find_inserted_value(
                value_from_slot(value, data.value.get()),
                indices.get(data.indices.len()..).unwrap_or(&[]),
            )
        }
        InstructionKindData::ExtractValue(data) => {
            // Extracting from something extracted: go to the original and chain
            // the index lists.
            let mut chained = data.indices.to_vec();
            chained.extend_from_slice(indices);
            find_inserted_value(value_from_slot(value, data.aggregate.get()), &chained)
        }
        // Otherwise unknown — extracting from a call result or a load.
        _ => None,
    }
}

// --------------------------------------------------------------------------
// Shared helpers
// --------------------------------------------------------------------------

/// Ports the static `getUnderlyingObjectFromInt`: walk back through integer
/// arithmetic to the `ptrtoint` that started it.
fn underlying_object_from_int<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Value<'ctx, B> {
    let mut current = value;
    loop {
        let Some(opcode) = operator_opcode(current) else {
            return current;
        };
        // A `ptrtoint` hands control back to the pointer walk.
        if opcode == Opcode::PtrToInt {
            return operator_operand(current, 0).unwrap_or(current);
        }
        // An `add` of a constant, a multiply or a phi is likely to lead to the
        // base object on its left. The multiply cannot itself be computing the
        // address, because callers only care when the result is identifiable.
        if opcode != Opcode::Add {
            return current;
        }
        let (Some(lhs), Some(rhs)) = (operator_operand(current, 0), operator_operand(current, 1))
        else {
            return current;
        };
        let rhs_leads_to_base = matches!(
            &rhs.data().kind,
            ValueKindData::Constant(ConstantData::Int(_))
        ) || operator_opcode(rhs) == Some(Opcode::Mul)
            || matches!(instruction_kind(rhs), Some(InstructionKindData::Phi(_)));
        if !rhs_leads_to_base {
            return current;
        }
        current = lhs;
    }
}

/// Ports `Value::stripPointerCasts` for the cases the string walk meets:
/// `bitcast` and `addrspacecast` of a pointer, plus zero-offset GEPs.
fn strip_pointer_casts<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Value<'ctx, B> {
    let mut current = value;
    for _ in 0..MAX_LOOKUP_SEARCH_DEPTH {
        let next = match operator_opcode(current) {
            Some(Opcode::BitCast | Opcode::AddrSpaceCast) => operator_operand(current, 0),
            Some(Opcode::GetElementPtr) => match instruction_kind(current) {
                Some(InstructionKindData::Gep(data)) if gep_has_all_zero_indices(current, data) => {
                    Some(value_from_slot(current, data.ptr.get()))
                }
                _ => None,
            },
            _ => None,
        };
        match next.filter(|next| is_pointer(next.ty())) {
            Some(next) => current = next,
            None => return current,
        }
    }
    current
}

/// Ports `Value::stripPointerCastsSameRepresentation` (`llvm/lib/IR/Value.cpp`),
/// the narrower sibling of [`strip_pointer_casts`] used by
/// `isGuaranteedNotToBeUndefOrPoison` before its allocated-object test.
///
/// The difference from [`strip_pointer_casts`] is `addrspacecast`, which this
/// does **not** peel. Upstream peels it only when the two address spaces have
/// the same representation — a `DataLayout::isNonIntegralAddressSpace` question
/// llvmkit does not model — so declining is the conservative reading. Not
/// peeling only forgoes an answer; peeling wrongly would claim a pointer is an
/// allocated object when the cast changed what it means.
///
/// Crate-visible rather than public: `Value.h` is not a surface the
/// ValueTracking parity ledger tracks.
pub(crate) fn strip_pointer_casts_same_representation<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Value<'ctx, B> {
    let mut current = value;
    for _ in 0..MAX_LOOKUP_SEARCH_DEPTH {
        let next = match operator_opcode(current) {
            Some(Opcode::BitCast) => operator_operand(current, 0),
            Some(Opcode::GetElementPtr) => match instruction_kind(current) {
                Some(InstructionKindData::Gep(data)) if gep_has_all_zero_indices(current, data) => {
                    Some(value_from_slot(current, data.ptr.get()))
                }
                _ => None,
            },
            _ => None,
        };
        match next.filter(|next| is_pointer(next.ty())) {
            Some(next) => current = next,
            None => return current,
        }
    }
    current
}

/// Ports `llvm::isIdentifiedObject` (`llvm/lib/Analysis/AliasAnalysis.cpp`).
///
/// Not public: it belongs to `AliasAnalysis.h`, a surface the ValueTracking
/// parity ledger does not track, and only
/// [`get_underlying_objects_for_code_gen`] reads it.
fn is_identified_object<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    if matches!(
        instruction_kind(value),
        Some(InstructionKindData::Alloca(_))
    ) {
        return true;
    }
    match &value.data().kind {
        // A `GlobalAlias` is deliberately excluded: it names another object.
        ValueKindData::GlobalVariable(_)
        | ValueKindData::Function(_)
        | ValueKindData::GlobalIFunc(_) => return true,
        // `isNoAliasOrByValArgument`.
        ValueKindData::Argument { parent_fn, slot } => {
            return argument_has_any_attribute(
                value,
                *parent_fn,
                *slot,
                &[AttrKind::NoAlias, AttrKind::ByVal],
            );
        }
        _ => {}
    }
    // `isNoAliasCall`: a call whose return carries `noalias`.
    call_return_attrs(value).is_some_and(|attrs| {
        attrs
            .iter()
            .any(|attr| matches!(attr, AttributeStored::Enum(AttrKind::NoAlias)))
    })
}

/// Whether parameter `slot` of `parent_fn` carries any of `wanted`.
fn argument_has_any_attribute<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    parent_fn: ValueSlot,
    slot: u32,
    wanted: &[AttrKind],
) -> bool {
    let function = value_from_slot(anchor, parent_fn);
    let ValueKindData::Function(data) = &function.data().kind else {
        return false;
    };
    let attributes = data.attributes.borrow();
    attributes
        .get(AttrIndex::Param(slot))
        .is_some_and(|stored| {
            stored
                .iter()
                .any(|attr| matches!(attr, AttributeStored::Enum(kind) if wanted.contains(kind)))
        })
}

/// Ports `GlobalValue::isInterposableLinkage`.
fn is_interposable_linkage(linkage: Linkage) -> bool {
    matches!(
        linkage,
        Linkage::WeakAny | Linkage::LinkOnceAny | Linkage::Common | Linkage::ExternalWeak
    )
}

/// Ports `Constant::isNullValue` for the constant forms llvmkit stores.
fn is_null_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    matches!(&value.data().kind, ValueKindData::Constant(_))
        && Constant::from_parts(value).is_null_value()
}

/// The one byte `bits` repeats, when every byte is the same.
/// Ports the `CI->getValue().isSplat(8)` test plus the `trunc(8)` that follows.
fn splat_byte(bits: &ApInt) -> Option<u8> {
    let width = bits.bit_width();
    if width == 0 || !width.is_multiple_of(8) {
        return None;
    }
    let first = u8::try_from(bits.extract_bits(8, 0).limited_value(u64::from(u8::MAX))).ok()?;
    let mut position = 8;
    while position < width {
        let byte = u8::try_from(
            bits.extract_bits(8, position)
                .limited_value(u64::from(u8::MAX)),
        )
        .ok()?;
        if byte != first {
            return None;
        }
        position += 8;
    }
    Some(first)
}

/// The integer width `isBytewiseValue` reinterprets a float at, or `None` for
/// the long-double formats upstream declines.
fn float_bit_width<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<u32> {
    match ty.kind() {
        TypeKind::Half | TypeKind::BFloat => Some(16),
        TypeKind::Float => Some(32),
        TypeKind::Double => Some(64),
        _ => None,
    }
}

fn to_words(bits: u128) -> [u64; 2] {
    [
        u64::try_from(bits & u128::from(u64::MAX)).unwrap_or(0),
        u64::try_from(bits >> 64).unwrap_or(0),
    ]
}

/// The pointer operand of a `getelementptr`, whether an instruction or a
/// constant expression. Ports `dyn_cast<GEPOperator>` plus `getPointerOperand`.
fn gep_pointer_operand<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    if let Some(InstructionKindData::Gep(data)) = instruction_kind(value) {
        return Some(value_from_slot(value, data.ptr.get()));
    }
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Expr(expr))
            if expr.opcode == ConstantExprOpcode::GetElementPtr =>
        {
            Some(value_from_slot(value, *expr.operands.first()?))
        }
        // llvmkit's compact byte-offset-into-a-global form, which is always an
        // `inbounds getelementptr` of `base_id`.
        ValueKindData::Constant(ConstantData::GepOffset { base_id, .. }) => {
            Some(value_from_slot(value, *base_id))
        }
        _ => None,
    }
}

/// Ports `Operator::getOpcode`: the opcode of an instruction *or* of a constant
/// expression, which upstream reaches through the same `Operator` base.
fn operator_opcode<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Opcode> {
    if let ValueKindData::Instruction(instruction) = &value.data().kind {
        return Some(instruction.kind.opcode());
    }
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &value.data().kind else {
        return None;
    };
    Some(match expr.opcode {
        ConstantExprOpcode::Add => Opcode::Add,
        ConstantExprOpcode::Sub => Opcode::Sub,
        ConstantExprOpcode::Xor => Opcode::Xor,
        ConstantExprOpcode::GetElementPtr => Opcode::GetElementPtr,
        ConstantExprOpcode::ShuffleVector => Opcode::ShuffleVector,
        ConstantExprOpcode::InsertElement => Opcode::InsertElement,
        ConstantExprOpcode::ExtractElement => Opcode::ExtractElement,
        ConstantExprOpcode::Trunc => Opcode::Trunc,
        ConstantExprOpcode::PtrToAddr => Opcode::PtrToAddr,
        ConstantExprOpcode::PtrToInt => Opcode::PtrToInt,
        ConstantExprOpcode::IntToPtr => Opcode::IntToPtr,
        ConstantExprOpcode::BitCast => Opcode::BitCast,
        ConstantExprOpcode::AddrSpaceCast => Opcode::AddrSpaceCast,
    })
}

/// Operand `index` of an instruction or constant expression, in the order
/// `User::operands` yields them.
fn operator_operand<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    index: usize,
) -> Option<Value<'ctx, B>> {
    match &value.data().kind {
        ValueKindData::Instruction(instruction) => {
            let slot = *instruction.kind.operand_ids().get(index)?;
            Some(value_from_slot(value, slot))
        }
        ValueKindData::Constant(ConstantData::Expr(expr)) => {
            Some(value_from_slot(value, *expr.operands.get(index)?))
        }
        _ => None,
    }
}

/// Argument `index` of a call/invoke/callbr.
fn call_argument<'ctx, B: ModuleBrand + 'ctx>(
    call: Value<'ctx, B>,
    index: usize,
) -> Option<Value<'ctx, B>> {
    let args = match instruction_kind(call)? {
        InstructionKindData::Call(data) => &data.args,
        InstructionKindData::Invoke(data) => &data.args,
        InstructionKindData::CallBr(data) => &data.args,
        _ => return None,
    };
    Some(value_from_slot(call, args.get(index)?.get()))
}

/// The argument a call marks `returned`.
///
/// Ports `CallBase::getReturnedArgOperand`, via
/// `CallBase::getArgOperandWithAttribute`: the call site's own parameter
/// attributes first, then — when it names a function directly — that
/// function's. The fallback is load-bearing, because `declare ptr @f(ptr
/// returned)` puts the attribute on the declaration and a call site that does
/// not repeat it still returns its argument.
fn returned_argument<'ctx, B: ModuleBrand + 'ctx>(call: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let (args, callee, arg_attrs) = match instruction_kind(call)? {
        InstructionKindData::Call(data) => (&data.args, data.callee.get(), data.attrs.arg_attrs()),
        InstructionKindData::Invoke(data) => {
            (&data.args, data.callee.get(), data.attrs.arg_attrs())
        }
        InstructionKindData::CallBr(data) => {
            (&data.args, data.callee.get(), data.attrs.arg_attrs())
        }
        _ => return None,
    };

    let index = returned_parameter_index(arg_attrs.len(), |index| {
        arg_attrs
            .get(index)
            .and_then(|attrs| attrs.get(AttrIndex::Param(u32::try_from(index).ok()?)))
            .is_some_and(has_returned)
    })
    .or_else(|| {
        let callee = value_from_slot(call, callee);
        let ValueKindData::Function(data) = &callee.data().kind else {
            return None;
        };
        let attributes = data.attributes.borrow();
        returned_parameter_index(args.len(), |index| {
            u32::try_from(index).ok().is_some_and(|slot| {
                attributes
                    .get(AttrIndex::Param(slot))
                    .is_some_and(has_returned)
            })
        })
    })?;

    Some(value_from_slot(call, args.get(index)?.get()))
}

/// The first parameter position below `count` for which `carries` holds.
fn returned_parameter_index<F: Fn(usize) -> bool>(count: usize, carries: F) -> Option<usize> {
    (0..count).find(|index| carries(*index))
}

fn has_returned(attrs: &[AttributeStored]) -> bool {
    attrs
        .iter()
        .any(|attr| matches!(attr, AttributeStored::Enum(AttrKind::Returned)))
}

/// The return-position attributes of a call/invoke/callbr.
fn call_return_attrs<'ctx, B: ModuleBrand + 'ctx>(
    call: Value<'ctx, B>,
) -> Option<&'ctx [AttributeStored]> {
    let attrs = match instruction_kind(call)? {
        InstructionKindData::Call(data) => &data.attrs,
        InstructionKindData::Invoke(data) => &data.attrs,
        InstructionKindData::CallBr(data) => &data.attrs,
        _ => return None,
    };
    attrs.return_attrs().get(AttrIndex::Return)
}

/// The base name of the intrinsic `call` invokes directly.
fn called_intrinsic_name<'ctx, B: ModuleBrand + 'ctx>(
    call: Value<'ctx, B>,
) -> Option<&'static str> {
    let callee = match instruction_kind(call)? {
        InstructionKindData::Call(data) => data.callee.get(),
        InstructionKindData::Invoke(data) => data.callee.get(),
        InstructionKindData::CallBr(data) => data.callee.get(),
        _ => return None,
    };
    let callee = value_from_slot(call, callee);
    Some(descriptor_for_callee(callee)?.id().base_name())
}

/// Ports `GetElementPtrInst::hasAllZeroIndices`.
fn gep_has_all_zero_indices<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &GepInstData,
) -> bool {
    data.indices.iter().all(|index| {
        let index = value_from_slot(anchor, index.get());
        matches!(
            &index.data().kind,
            ValueKindData::Constant(ConstantData::Int(words)) if words.iter().all(|word| *word == 0)
        )
    })
}

/// One step of [`strip_and_accumulate_offset`]: the peeled base and the byte
/// offset that step contributed.
///
/// Ports the instruction half of `Value::stripAndAccumulateConstantOffsets`
/// (`llvm/lib/IR/Value.cpp`). `addrspacecast` is deliberately not peeled, for
/// the reason upstream's own comment gives: crossing one can change the index
/// width mid-walk, and this keeps a single width throughout.
fn peel_one_constant_offset<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    index_bits: u32,
    allow_non_inbounds: bool,
    data_layout: &DataLayout,
) -> Option<(Value<'ctx, B>, ApInt)> {
    if let Some(InstructionKindData::Gep(data)) = instruction_kind(value) {
        if !allow_non_inbounds && !data.flags.contains(GepNoWrapFlags::IN_BOUNDS) {
            return None;
        }
        let base = value_from_slot(value, data.ptr.get());
        let offset = gep_constant_offset(value, data, index_bits, data_layout)?;
        return Some((base, offset));
    }
    if operator_opcode(value) == Some(Opcode::BitCast) {
        let source = operator_operand(value, 0)?;
        return is_pointer(source.ty()).then_some((source, ApInt::zero(index_bits)));
    }
    if let ValueKindData::Constant(ConstantData::GepOffset { base_id, off }) = &value.data().kind {
        // Always an `inbounds` GEP, so it peels regardless of the flag.
        return Some((
            value_from_slot(value, *base_id),
            signed_ap_int(*off, index_bits),
        ));
    }
    None
}

/// The constant byte offset a `getelementptr` adds, or `None` when any index is
/// not a constant or a type is not walkable.
fn gep_constant_offset<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &GepInstData,
    index_bits: u32,
    data_layout: &DataLayout,
) -> Option<ApInt> {
    let module = module_ref(anchor);
    let mut offset = ApInt::zero(index_bits);
    let mut current = Type::new(data.source_ty, module);

    for (position, index) in data.indices.iter().enumerate() {
        let index = value_from_slot(anchor, index.get());
        let ValueKindData::Constant(ConstantData::Int(words)) = &index.data().kind else {
            return None;
        };
        let index_value = ApInt::from_words(index_bits, words).try_sext_i64()?;

        if position == 0 {
            // The leading index strides whole `source_ty`s.
            let stride = i64::try_from(data_layout.type_alloc_size(current)).ok()?;
            offset =
                offset.wrapping_add(&signed_ap_int(index_value.checked_mul(stride)?, index_bits));
            continue;
        }

        match current.kind() {
            TypeKind::Struct => {
                let field = usize::try_from(index_value).ok()?;
                let byte = data_layout.struct_layout(current).element_offset(field);
                offset = offset.wrapping_add(&signed_ap_int(i64::try_from(byte).ok()?, index_bits));
                current = struct_field_type(current, field)?;
            }
            TypeKind::Array | TypeKind::FixedVector | TypeKind::ScalableVector => {
                let element = element_type(current)?;
                let stride = i64::try_from(data_layout.type_alloc_size(element)).ok()?;
                offset = offset
                    .wrapping_add(&signed_ap_int(index_value.checked_mul(stride)?, index_bits));
                current = element;
            }
            _ => return None,
        }
    }
    Some(offset)
}

fn struct_field_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    field: usize,
) -> Option<Type<'ctx, B>> {
    let TypeData::Struct(data) = ty.data() else {
        return None;
    };
    let slot: TypeSlot = data
        .body
        .borrow()
        .as_ref()
        .and_then(|body| body.elements.get(field).copied())?;
    Some(Type::new(slot, module_ref_from_type(ty)))
}

fn element_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<Type<'ctx, B>> {
    let module = module_ref_from_type(ty);
    if let Some((element, _)) = ty.data().as_array() {
        return Some(Type::new(element, module));
    }
    let (element, _, _) = ty.data().as_vector()?;
    Some(Type::new(element, module))
}

/// The index width `DataLayout::getIndexTypeSizeInBits` gives for `ty`.
fn index_type_size_in_bits<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    data_layout: &DataLayout,
) -> u32 {
    match ty.kind() {
        TypeKind::Pointer { addr_space } => data_layout.index_size_in_bits(addr_space),
        TypeKind::FixedVector | TypeKind::ScalableVector => match element_type(ty) {
            Some(element) => index_type_size_in_bits(element, data_layout),
            None => data_layout.index_size_in_bits(0),
        },
        _ => data_layout.index_size_in_bits(0),
    }
}

fn signed_ap_int(value: i64, bits: u32) -> ApInt {
    ApInt::from_words(64, &[value.cast_unsigned()]).sext_or_trunc(bits)
}

fn is_pointer<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    matches!(ty.kind(), TypeKind::Pointer { .. } | TypeKind::TypedPointer)
}

fn instruction_kind<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<&'ctx InstructionKindData> {
    match &value.data().kind {
        ValueKindData::Instruction(instruction) => Some(&instruction.kind),
        _ => None,
    }
}

fn module_ref<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> ModuleRef<'ctx, B> {
    ModuleRef::new(value.module().core_ref())
}

fn module_ref_from_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> ModuleRef<'ctx, B> {
    ModuleRef::new(ty.module().core_ref())
}
