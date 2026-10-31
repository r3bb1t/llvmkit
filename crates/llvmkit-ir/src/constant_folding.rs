//! DataLayout-aware analysis constant folding.
//!
//! Mirrors the public routines in `llvm/include/llvm/Analysis/ConstantFolding.h`.
//! The target-independent subset lives in [`crate::constant_fold`]; this module
//! adds the folds that need `DataLayout`, denormal handling, or target-library
//! availability.

use core::num::NonZeroU32;

use super::ap_float::{ApFloat, ApFloatSemantics, ApFloatSign};
use super::ap_int::Signedness;
use super::capability::{Capability, Mutable};
use super::cmp_predicate::{CmpPredicate, IntPredicate};
use super::constant::{
    Constant, ConstantAggregateKind, ConstantData, ConstantDataKind, ConstantExprData,
    ConstantExprFlags, ConstantExprInRange, ConstantExprOpcode, ConstantKind,
    ConstantSequentialKind, is_poison, is_undef, is_undef_or_poison,
};
use super::constant_fold::{
    all_ones_constant_for_type, constant_aggregate_from_elements, constant_cast_expr,
    constant_fold_binary_instruction_trusting_same_module,
    constant_fold_cast_instruction_trusting_same_module,
    constant_fold_compare_instruction_trusting_same_module,
    constant_fold_extract_element_instruction_trusting_same_module,
    constant_fold_extract_value_instruction, constant_fold_get_element_ptr_trusting_same_module,
    constant_fold_insert_element_instruction_trusting_same_module,
    constant_fold_insert_value_instruction_trusting_same_module,
    constant_fold_select_instruction_trusting_same_module,
    constant_fold_shuffle_vector_instruction_trusting_same_module, constant_fold_unary_instruction,
    constant_from_id, null_constant_for_type, shufflevector_mask_from_constant,
    vector_type_with_scalability,
};
use super::constants::{
    ConstantExprOptions, ConstantFloatValue, ConstantIntValue, GepTypeCursor, global_value_type,
};
use super::data_layout::DataLayout;
use super::denormal_mode::{DenormalMode, DenormalModeKind, DenormalModeSide};
use super::derived_types::{FloatType, IntType, PointerType, StructType, VectorType};
use super::element::ElemDyn;
use super::float_kind::FloatDyn;
use super::fmf::FastMathFlags;
use super::gep_no_wrap_flags::GepNoWrapFlags;
use super::global_value::GlobalValue;
use super::global_variable::GlobalVariable;
use super::instr_types::{BinaryOpcode, CastOpcode, PhiData, UnaryOpcode};
use super::instruction::{InstructionKindData, InstructionView};
use super::instructions::{cast_is_valid, is_eliminable_cast_pair};
use super::int_width::IntDyn;
use super::intrinsics::BinaryIntrinsic;
use super::module::{ModuleBrand, ModuleRef};
use super::pointer_analysis::{
    ConstantOffsetStrip, GepOperator, MAX_LOOKUP_SEARCH_DEPTH, index_type_size_in_bits,
    strip_and_accumulate_constant_offsets, underlying_object,
};
use super::target_library_info::{LibFunc, TargetLibraryInfo};
use super::r#type::{MAX_INT_BITS, MIN_INT_BITS, Type, TypeData, TypeSlotAccess, erase_type};
use super::type_components::lanes;
use super::type_size::TypeSize;
use super::value::{IsValue, Value, ValueKindData, ValueSlot, ValueSlotAccess};
use super::vec_len::LenDyn;
use super::{ApInt, Dyn, FunctionValue, IrError, IrResult};
use crate::Branded;

/// Whether folds that depend on host/libm floating-point determinism are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FoldNonDeterminism {
    Allow,
    Deny,
}

/// Flags preserved while collapsing a pair of casts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PreservedCastFlags {
    non_negative: bool,
    no_unsigned_wrap: bool,
    no_signed_wrap: bool,
}

impl PreservedCastFlags {
    #[inline]
    pub const fn none() -> Self {
        Self {
            non_negative: false,
            no_unsigned_wrap: false,
            no_signed_wrap: false,
        }
    }

    #[inline]
    pub const fn non_negative(mut self) -> Self {
        self.non_negative = true;
        self
    }

    #[inline]
    pub const fn no_unsigned_wrap(mut self) -> Self {
        self.no_unsigned_wrap = true;
        self
    }

    #[inline]
    pub const fn no_signed_wrap(mut self) -> Self {
        self.no_signed_wrap = true;
        self
    }

    #[inline]
    pub const fn has_non_negative(self) -> bool {
        self.non_negative
    }

    #[inline]
    pub const fn has_no_unsigned_wrap(self) -> bool {
        self.no_unsigned_wrap
    }

    #[inline]
    pub const fn has_no_signed_wrap(self) -> bool {
        self.no_signed_wrap
    }
}

/// Constant pointer offset relative to one global object.
#[derive(Branded)]
#[branded(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConstantOffsetFromGlobal<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    /// At the capability of the constant the analysis was asked about: the
    /// global is reached through it (D1, D8).
    global: GlobalValue<'ctx, B, C>,
    offset: ApInt,
    dso_local_equivalent: Option<Constant<'ctx, B, C>>,
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> ConstantOffsetFromGlobal<'ctx, B, C> {
    /// The start of `global`, reached through `dso_local_equivalent` if that is
    /// `Some`. `Offset = APInt(DL.getIndexTypeSizeInBits(GV->getType()), 0)`.
    fn at_start_of(
        global: GlobalValue<'ctx, B, C>,
        dso_local_equivalent: Option<Constant<'ctx, B, C>>,
        dl: &DataLayout,
    ) -> Self {
        let index_bits = index_type_size_in_bits(global_value_type(global.as_erased()), dl);
        Self {
            global,
            offset: ApInt::zero(index_bits),
            dso_local_equivalent,
        }
    }

    /// Base global object, at the analysed constant's capability.
    #[inline]
    pub fn global(&self) -> GlobalValue<'ctx, B, C> {
        self.global
    }

    /// Byte offset from the start of [`Self::global`].
    #[inline]
    pub fn offset(&self) -> &ApInt {
        &self.offset
    }

    /// The `dso_local_equivalent` the walk reached [`Self::global`] through, if
    /// it did. Upstream's `DSOEquiv` out-parameter.
    #[inline]
    pub fn dso_local_equivalent(&self) -> Option<Constant<'ctx, B, C>> {
        self.dso_local_equivalent
    }
}

/// True if the target library permits folding `lib_func`.
#[inline]
pub fn can_constant_fold_call_to(lib_func: LibFunc, tli: &TargetLibraryInfo) -> bool {
    tli.has(lib_func)
}

/// Flush one floating-point constant according to `mode`.
pub fn flush_fp_constant<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    operand: Constant<'ctx, B, C>,
    mode: DenormalMode,
    side: DenormalModeSide,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    if let Ok(fp) = ConstantFloatValue::<FloatDyn, B, C>::try_from(operand) {
        let apf = fp.ap_float();
        if !apf.is_denormal() {
            return Ok(Some(operand));
        }
        return flush_denormal_float(fp.ty(), &apf, mode.for_side(side));
    }

    if is_undef(operand) || is_poison(operand) {
        return Ok(Some(operand));
    }

    // Mirrors the `ConstantVector`/`ConstantDataVector` arms of upstream
    // `FlushFPConstant` (`ConstantFolding.cpp`): flush every lane of a fixed
    // FP vector and rebuild the vector constant. A whole-vector `undef`
    // (upstream's `UndefValue` arm) is already handled above. llvmkit has no
    // distinct `ConstantAggregateZero` representation — zero vectors are
    // ordinary `Aggregate`s — so upstream's all-zero fast path falls out of
    // this loop for free (0.0 is never denormal), and likewise upstream's
    // splat fast path is just a performance shortcut this loop reaches the
    // same answer without. Scalable vectors have no compile-time lane list to
    // walk and fall through to the decline below, matching upstream's
    // decline for a non-splat scalable operand.
    if matches!(operand.ty().data(), TypeData::FixedVector { .. })
        && let ValueKindData::Constant(ConstantData::Aggregate(elements)) =
            &operand.as_erased().data().kind
    {
        let module = operand.module;
        let mut flushed = Vec::with_capacity(elements.len());
        for id in elements.iter().copied() {
            let Some(element) = constant_from_id(module, id) else {
                return Ok(None);
            };
            let Some(flushed_element) = flush_fp_constant(element, mode, side)? else {
                return Ok(None);
            };
            flushed.push(flushed_element);
        }
        let Ok(vector_ty) = VectorType::<ElemDyn, LenDyn, B, C>::try_from(operand.ty()) else {
            return Ok(None);
        };
        return Ok(Some(
            vector_ty
                .const_vector::<Constant<'ctx, B, C>, _>(flushed)?
                .as_constant(),
        ));
    }

    Ok(None)
}

/// Resolve a constant pointer to a global plus byte offset. Ports
/// `IsConstantOffsetFromGlobal`: "if this constant is a constant offset from a
/// global, return the global and the constant".
pub fn constant_offset_from_global<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    pointer: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> Option<ConstantOffsetFromGlobal<'ctx, B, C>> {
    // Trivial case, constant is the global.
    if let Some(global) = GlobalValue::of(pointer.as_erased()) {
        return Some(ConstantOffsetFromGlobal::at_start_of(global, None, dl));
    }

    if let ValueKindData::Constant(ConstantData::DsoLocalEquivalent { function }) =
        &pointer.as_erased().data().kind
    {
        let module = pointer.module;
        let function_ty = module.context().value_data(*function).ty;
        let global = GlobalValue::of(Value::from_parts(*function, module, function_ty))?;
        return Some(ConstantOffsetFromGlobal::at_start_of(
            global,
            Some(pointer),
            dl,
        ));
    }

    // Look through ptr->int and ptr->ptr casts.
    if let Some((opcode, operand)) = constant_cast_expr(pointer)
        && matches!(
            opcode,
            CastOpcode::PtrToInt | CastOpcode::PtrToAddr | CastOpcode::BitCast
        )
    {
        return constant_offset_from_global(operand, dl);
    }

    // i32* getelementptr ([5 x i32]* @a, i32 0, i32 5)
    let gep = GepOperator::of(pointer.as_erased())?;

    let index_bits = index_type_size_in_bits(pointer.ty(), dl);
    // If the base isn't a global+constant, we aren't either.
    let base = constant_from_id(
        pointer.module,
        gep.pointer_operand.slot_trusting_same_module(),
    )?;
    let resolved = constant_offset_from_global(base, dl)?;

    // Otherwise, add any offset that our operands provide. Upstream asserts the
    // two widths agree, and they do unless a compact form names another space.
    let gep_offset = gep.accumulate_constant_offset(index_bits, dl)?;
    Some(ConstantOffsetFromGlobal {
        offset: resolved
            .offset
            .sext_or_trunc(index_bits)
            .wrapping_add(&gep_offset),
        ..resolved
    })
}

/// Resolve a constant pointer to a global plus byte offset.
#[inline]
pub fn is_constant_offset_from_global<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    pointer: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> Option<ConstantOffsetFromGlobal<'ctx, B, C>> {
    constant_offset_from_global(pointer, dl)
}

/// Fold a load from a pointer to a constant global.
///
/// Errors with [`IrError::ForeignType`] if `ty` belongs to a module other than
/// `pointer`'s.
pub fn constant_fold_load_from_const_ptr<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    pointer: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    offset: ApInt,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's load type, admitted against `pointer`'s module.
    ty.slot_in(pointer.module.id())?;
    constant_fold_load_from_const_ptr_trusting_same_module(pointer, ty, offset, dl)
}

/// [`constant_fold_load_from_const_ptr`] for a pointer and type of one module.
pub(crate) fn constant_fold_load_from_const_ptr_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    pointer: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    offset: ApInt,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Mirrors `ConstantFoldLoadFromConstPtr` (`ConstantFolding.cpp`), step for
    // step. We can only fold loads from constant globals with a definitive
    // initializer. Check this upfront, to skip expensive offset calculations.
    let object = underlying_object(pointer.as_erased(), MAX_LOOKUP_SEARCH_DEPTH);
    let Ok(global) = GlobalVariable::try_from(object) else {
        return Ok(None);
    };
    if !global.is_constant() || !global.has_definitive_initializer() {
        return Ok(None);
    }
    let Some(initializer) = global.initializer() else {
        return Ok(None);
    };

    // Upstream asserts the offset is at the pointer's index width. A public
    // caller can hand over any width, and the pointer of a load is no vector.
    let Some(index_bits) = index_bits_for_pointer(pointer.ty(), dl) else {
        return Ok(None);
    };
    let (stripped, offset) = strip_and_accumulate_constant_offsets(
        pointer.as_erased(),
        offset.sext_or_trunc(index_bits),
        ConstantOffsetStrip {
            allow_non_inbounds: true,
            look_through_int_to_ptr: false,
        },
        dl,
    );

    if stripped == object
        && let Some(result) =
            constant_fold_load_from_const_trusting_same_module(initializer, ty, offset, dl)?
    {
        return Ok(Some(result));
    }

    // If this load comes from anywhere in a uniform constant global, the value
    // is always the same, regardless of the loaded offset.
    constant_fold_load_from_uniform_value_trusting_same_module(initializer, ty, dl)
}

/// Fold a load directly from a constant aggregate/scalar at byte `offset`.
///
/// Errors with [`IrError::ForeignType`] if `ty` belongs to a module other than
/// `constant`'s.
pub fn constant_fold_load_from_const<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    offset: ApInt,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's load type, admitted against `constant`'s module.
    ty.slot_in(constant.module.id())?;
    constant_fold_load_from_const_trusting_same_module(constant, ty, offset, dl)
}

/// [`constant_fold_load_from_const`] for a constant and type of one module.
pub(crate) fn constant_fold_load_from_const_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    constant: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    offset: ApInt,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Mirrors `ConstantFoldLoadFromConst` in
    // `llvm/lib/Analysis/ConstantFolding.cpp`, step for step.
    if let Some(at_offset) = constant_at_offset(constant, &offset, dl)
        && let Some(folded) =
            constant_fold_load_through_bitcast_trusting_same_module(at_offset, ty, dl)?
    {
        return Ok(Some(folded));
    }

    // Explicitly check for out-of-bounds access, so we return poison even if
    // the constant is a uniform value. A scalable size is not known here.
    if let Some(size) = dl.type_alloc_size(erase_type(constant.ty())).fixed_value()
        && offset_sge(&offset, size)
    {
        return Ok(Some(ty.poison().as_constant()));
    }

    // Try an offset-independent fold of a uniform value.
    if let Some(folded) =
        constant_fold_load_from_uniform_value_trusting_same_module(constant, ty, dl)?
    {
        return Ok(Some(folded));
    }

    // Try hard to fold loads from bitcasted strange and non-type-safe things.
    match offset.try_sext_i64() {
        Some(offset) => fold_reinterpret_load_from_const(constant, ty, offset, dl),
        None => Ok(None),
    }
}

/// `Offset.sge(Size.getFixedValue())`: `APInt::sge(int64_t)`, whose `uint64_t`
/// argument converts implicitly, and which answers by sign for an offset too
/// wide for 64 bits.
fn offset_sge(offset: &ApInt, size: u64) -> bool {
    match offset.try_sext_i64() {
        Some(offset) => offset >= size.cast_signed(),
        None => !offset.is_negative(),
    }
}

/// Mirrors `ReadDataFromGlobal` (`ConstantFolding.cpp`): copy the bytes of
/// `constant` from `byte_offset` on into `out`, which is upstream's `CurPtr`
/// and `BytesLeft` as one slice. `out` starts zeroed, and what is not written
/// stays that way. `false` is upstream's.
///
/// A scalable size here is upstream's fatal `TypeSize` conversion, which a
/// struct of fixed size with a scalable field reaches. It declines
/// (`docs/divergences.md` D24).
fn read_data_from_global<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    byte_offset: u64,
    out: &mut [u8],
    dl: &DataLayout,
) -> IrResult<bool> {
    // Reading type padding, return zero.
    let Some(store_size) = dl.type_store_size(erase_type(constant.ty())).fixed_value() else {
        return Ok(false);
    };
    if byte_offset >= store_size {
        return Ok(true);
    }

    match constant.kind() {
        // If this element is zero or undefined, we can just return since
        // *CurPtr is zero initialized.
        ConstantKind::Data(ConstantDataKind::AggregateZero | ConstantDataKind::Undef(_)) => {
            Ok(true)
        }
        ConstantKind::Data(ConstantDataKind::Int(value)) => {
            let bit_width = value.bit_width();
            if bit_width & 7 != 0 {
                return Ok(false);
            }
            let bytes = value.ap_int();
            let int_bytes = u64::from(bit_width / 8);
            for (byte, byte_offset) in out.iter_mut().zip(byte_offset..int_bytes) {
                let n = if dl.is_little_endian() {
                    byte_offset
                } else {
                    int_bytes - byte_offset - 1
                };
                let Ok(n) = u32::try_from(n) else {
                    return Ok(false);
                };
                [*byte, ..] = bytes
                    .extract_bits(8, n * 8)
                    .limited_value(u64::MAX)
                    .to_le_bytes();
            }
            Ok(true)
        }
        ConstantKind::Data(ConstantDataKind::Float(value)) => {
            let bits = match value.ty().semantics() {
                ApFloatSemantics::IeeeDouble => 64,
                ApFloatSemantics::IeeeSingle => 32,
                ApFloatSemantics::IeeeHalf => 16,
                ApFloatSemantics::Bfloat
                | ApFloatSemantics::IeeeQuad
                | ApFloatSemantics::X87DoubleExtended
                | ApFloatSemantics::PpcDoubleDouble => return Ok(false),
            };
            let int_ty = int_type(constant.module, bits)?.as_type();
            match fold_bitcast_with_layout(constant, int_ty, dl)? {
                Some(bits) => read_data_from_global(bits, byte_offset, out, dl),
                None => Ok(false),
            }
        }
        ConstantKind::Aggregate(ConstantAggregateKind::Struct) => {
            read_struct_data_from_global(constant, byte_offset, out, dl)
        }
        ConstantKind::Aggregate(ConstantAggregateKind::Array | ConstantAggregateKind::Vector)
        | ConstantKind::Data(ConstantDataKind::Sequential(_)) => {
            read_sequence_data_from_global(constant, byte_offset, out, dl)
        }
        ConstantKind::Expr(ConstantExprOpcode::IntToPtr) => {
            let Some(expr) = constant_expr_data(constant) else {
                return Ok(false);
            };
            let [operand] = expr.operands.as_ref() else {
                return Ok(false);
            };
            let Some(operand) = constant_from_id(constant.module, *operand) else {
                return Ok(false);
            };
            if operand.ty() != int_ptr_type(constant.ty(), dl)? {
                return Ok(false);
            }
            read_data_from_global(operand, byte_offset, out, dl)
        }
        // Otherwise, unknown initializer type.
        ConstantKind::Data(
            ConstantDataKind::TargetNone
            | ConstantDataKind::PointerNull
            | ConstantDataKind::TokenNone,
        )
        | ConstantKind::GlobalValue
        | ConstantKind::BlockAddress
        | ConstantKind::Expr(_)
        | ConstantKind::DsoLocalEquivalent
        | ConstantKind::NoCfi
        | ConstantKind::PtrAuth
        | ConstantKind::ForwardReference => Ok(false),
    }
}

/// The `ConstantStruct` arm of [`read_data_from_global`].
fn read_struct_data_from_global<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    byte_offset: u64,
    out: &mut [u8],
    dl: &DataLayout,
) -> IrResult<bool> {
    let Ok(struct_ty) = StructType::try_from(constant.ty()) else {
        return Ok(false);
    };
    let layout = dl.struct_layout(erase_type(constant.ty()));
    let Some(mut index) = layout.element_containing_offset(byte_offset) else {
        return Ok(false);
    };
    let Some(mut current_element_offset) = layout.element_offset(index).fixed_value() else {
        return Ok(false);
    };
    let mut byte_offset = byte_offset - current_element_offset;
    let mut out = out;

    loop {
        // If the element access is to the element itself and not to tail
        // padding, read the bytes from the element.
        let Some(element) = u32::try_from(index)
            .ok()
            .and_then(|index| constant.aggregate_element(index))
        else {
            return Ok(false);
        };
        let Some(element_size) = dl.type_alloc_size(erase_type(element.ty())).fixed_value() else {
            return Ok(false);
        };
        if byte_offset < element_size && !read_data_from_global(element, byte_offset, out, dl)? {
            return Ok(false);
        }

        index += 1;

        // Check to see if we read from the last struct element, if so we're
        // done.
        if index == struct_ty.field_count() {
            return Ok(true);
        }

        // If we read all of the bytes we needed from this element we're done.
        let Some(next_element_offset) = layout.element_offset(index).fixed_value() else {
            return Ok(false);
        };
        let Some(advance) = next_element_offset
            .checked_sub(current_element_offset)
            .and_then(|gap| gap.checked_sub(byte_offset))
            .and_then(|advance| usize::try_from(advance).ok())
        else {
            return Ok(false);
        };
        if out.len() <= advance {
            return Ok(true);
        }

        // Move to the next element of the struct.
        out = &mut out[advance..];
        byte_offset = 0;
        current_element_offset = next_element_offset;
    }
}

/// The `ConstantArray`, `ConstantVector` and `ConstantDataSequential` arm of
/// [`read_data_from_global`].
fn read_sequence_data_from_global<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    byte_offset: u64,
    out: &mut [u8],
    dl: &DataLayout,
) -> IrResult<bool> {
    let module = constant.module;
    let (num_elts, elt_size) = if let Some((element, length)) = constant.ty().data().as_array() {
        let Some(size) = dl
            .type_alloc_size(erase_type(Type::new(element, module)))
            .fixed_value()
        else {
            return Ok(false);
        };
        (length, size)
    } else {
        let Some((element, lanes, _)) = constant.ty().data().as_vector() else {
            return Ok(false);
        };
        let element = Type::new(element, module);
        // TODO: For non-byte-sized vectors, current implementation assumes
        // there is padding to the next byte boundary between elements.
        if !dl.type_size_equals_store_size(erase_type(element)) {
            return Ok(false);
        }
        let Some(size) = dl.type_store_size(erase_type(element)).fixed_value() else {
            return Ok(false);
        };
        (u64::from(lanes.get()), size)
    };
    // A zero element size is a zero store size, which the padding test answered.
    let Some(mut index) = byte_offset.checked_div(elt_size) else {
        return Ok(true);
    };
    let mut offset = byte_offset - index * elt_size;
    let mut out = out;

    while index != num_elts {
        let Some(element) = u32::try_from(index)
            .ok()
            .and_then(|index| constant.aggregate_element(index))
        else {
            return Ok(false);
        };
        if !read_data_from_global(element, offset, out, dl)? {
            return Ok(false);
        }

        let bytes_written = elt_size - offset;
        let Some(written) = usize::try_from(bytes_written)
            .ok()
            .filter(|written| *written < out.len())
        else {
            return Ok(true);
        };

        offset = 0;
        out = &mut out[written..];
        index += 1;
    }
    Ok(true)
}

/// Mirrors `FoldReinterpretLoadFromConst` (`ConstantFolding.cpp`), step for step.
fn fold_reinterpret_load_from_const<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    load_ty: Type<'ctx, B, C>,
    offset: i64,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Bail out early. Not expect to load from scalable global variable.
    if matches!(load_ty.data(), TypeData::ScalableVector { .. }) {
        return Ok(None);
    }

    // If this isn't an integer load we can't fold it directly.
    let Ok(int_ty) = IntType::<IntDyn, B, C>::try_from(load_ty) else {
        return fold_reinterpret_load_as_integer(constant, load_ty, offset, dl);
    };

    let bytes_loaded = int_ty.bit_width().div_ceil(8);
    if bytes_loaded > 32 || bytes_loaded == 0 {
        return Ok(None);
    }

    // If we're not accessing anything in this constant, the result is undefined.
    if offset <= -i64::from(bytes_loaded) {
        return Ok(Some(load_ty.poison().as_constant()));
    }

    // TODO: We should be able to support scalable types.
    let Some(initializer_size) = dl.type_alloc_size(erase_type(constant.ty())).fixed_value() else {
        return Ok(None);
    };

    // If we're not accessing anything in this constant, the result is undefined.
    if offset >= initializer_size.cast_signed() {
        return Ok(Some(load_ty.poison().as_constant()));
    }

    let (Ok(bytes_loaded), Ok(skipped)) = (
        usize::try_from(bytes_loaded),
        usize::try_from(offset.min(0).unsigned_abs()),
    ) else {
        return Ok(None);
    };
    let mut raw_bytes = [0_u8; 32];

    // If we're loading off the beginning of the global, some bytes may be valid.
    let read = read_data_from_global(
        constant,
        offset.max(0).cast_unsigned(),
        &mut raw_bytes[skipped..bytes_loaded],
        dl,
    )?;
    if !read {
        return Ok(None);
    }

    let bit_width = int_ty.bit_width();
    let byte_at = |index: usize| ApInt::from_words(bit_width, &[u64::from(raw_bytes[index])]);
    let mut result = if dl.is_little_endian() {
        byte_at(bytes_loaded - 1)
    } else {
        byte_at(0)
    };
    for i in 1..bytes_loaded {
        let next = if dl.is_little_endian() {
            byte_at(bytes_loaded - 1 - i)
        } else {
            byte_at(i)
        };
        result = result.shl(8).bitor(&next);
    }
    Ok(Some(int_ty.const_ap_int(&result)?.as_constant()))
}

/// The non-integer arm of [`fold_reinterpret_load_from_const`]: a load that can
/// be folded as an integer load and then bitcast, which is useful for union
/// cases. Address spaces don't matter here, since no new load results.
fn fold_reinterpret_load_as_integer<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    load_ty: Type<'ctx, B, C>,
    offset: i64,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    if !load_ty.is_floating_point() && !load_ty.is_pointer() && !load_ty.is_vector() {
        return Ok(None);
    }
    let is_pointer = load_ty.is_ptr_or_ptr_vector();

    // `getFixedValue`. The caller refused a scalable vector, and a float, a
    // pointer and a fixed vector each have a fixed size.
    let Some(bits) = dl.type_size_in_bits(erase_type(load_ty)).fixed_value() else {
        unreachable!("a float, a pointer or a fixed vector has a fixed size");
    };
    // `Type::getIntNTy` asserts on a width no `IntegerType` has. Past 32 bytes
    // the integer load answers null anyway, so that is what this answers.
    let Some(bits) = u32::try_from(bits)
        .ok()
        .filter(|bits| (MIN_INT_BITS..=MAX_INT_BITS).contains(bits))
    else {
        return Ok(None);
    };
    let map_ty = int_type(constant.module, bits)?.as_type();
    let Some(res) = fold_reinterpret_load_from_const(constant, map_ty, offset, dl)? else {
        return Ok(None);
    };
    if res.is_null_value() && !matches!(load_ty.data(), TypeData::X86Amx) {
        // Materializing a zero can be done trivially without a bitcast.
        return null_constant_for_type(load_ty);
    }
    let cast_ty = if is_pointer {
        int_ptr_type(load_ty, dl)?
    } else {
        load_ty
    };
    let Some(res) = fold_bitcast_with_layout(res, cast_ty, dl)? else {
        return Ok(None);
    };
    if !is_pointer {
        return Ok(Some(res));
    }
    // For vector of pointer, we needed to first convert to a vector of
    // integer, then do vector inttoptr.
    if res.is_null_value() && !matches!(load_ty.data(), TypeData::X86Amx) {
        return null_constant_for_type(load_ty);
    }
    // Be careful not to replace a load of an addrspace value with an inttoptr
    // here.
    if dl.is_non_integral_pointer_type(load_ty) {
        return Ok(None);
    }
    constant
        .module
        .constant_expr(
            load_ty,
            ConstantExprOpcode::IntToPtr,
            [res.as_erased()],
            [],
            [],
            ConstantExprFlags::none(),
        )
        .map(Some)
}

/// Mirrors `DataLayout::getIntPtrType(Type *)`: the integer of a pointer's
/// size, or the vector of them for a vector of pointers.
fn int_ptr_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Type<'ctx, B, C>> {
    integer_type_of_pointer_size(ty, DataLayout::pointer_size_in_bits, dl)
}

/// Mirrors `DataLayout::getIndexType(Type *)`, which is also `getAddressType`:
/// the integer of a pointer's index size, or the vector of them.
fn index_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Type<'ctx, B, C>> {
    integer_type_of_pointer_size(ty, DataLayout::index_size_in_bits, dl)
}

fn integer_type_of_pointer_size<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: Type<'ctx, B, C>,
    size_in_bits: fn(&DataLayout, crate::AddressSpace) -> u32,
    dl: &DataLayout,
) -> IrResult<Type<'ctx, B, C>> {
    let module = ty.module;
    let Some(addr_space) = pointer_address_space(ty.scalar_type()) else {
        return Err(IrError::InvalidOperation {
            message: "a pointer's integer type of a type that is no pointer",
        });
    };
    let int_ty = int_type(module, size_in_bits(dl, addr_space))?;
    Ok(match ty.data().as_vector() {
        Some((_, lanes, scalable)) => {
            vector_type_with_scalability(module, int_ty, lanes, scalable).as_type()
        }
        None => int_ty.as_type(),
    })
}

/// If `constant` has a uniform bit pattern, materialise that value as `ty`.
///
/// Errors with [`IrError::ForeignType`] if `ty` belongs to a module other than
/// `constant`'s.
pub fn constant_fold_load_from_uniform_value<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's load type, admitted against `constant`'s module.
    ty.slot_in(constant.module.id())?;
    constant_fold_load_from_uniform_value_trusting_same_module(constant, ty, dl)
}

/// [`constant_fold_load_from_uniform_value`] for a constant and type of one
/// module.
pub(crate) fn constant_fold_load_from_uniform_value_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    constant: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Mirrors `ConstantFoldLoadFromUniformValue`.
    if is_poison(constant) {
        return Ok(Some(ty.poison().as_constant()));
    }
    if is_undef(constant) {
        return Ok(Some(ty.undef().as_constant()));
    }
    // If padding is needed when storing C to memory, then it isn't considered
    // as uniform.
    if !dl.type_size_equals_store_size(erase_type(constant.ty())) {
        return Ok(None);
    }
    if constant.is_null_value() && !matches!(ty.data(), TypeData::X86Amx) {
        return null_constant_for_type(ty);
    }
    if constant.is_all_ones_value() && (ty.is_int_or_int_vector() || ty.is_float_or_float_vector())
    {
        return all_ones_constant_for_type(ty);
    }
    Ok(None)
}

/// Fold a cast using `DataLayout` where target-independent `ConstantFold.cpp`
/// cannot decide.
///
/// Errors with [`IrError::ForeignType`] if `dest_ty` belongs to a module other
/// than `operand`'s.
pub fn constant_fold_cast_operand<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's destination type, admitted against `operand`'s
    // module.
    dest_ty.slot_in(operand.module.id())?;
    constant_fold_cast_operand_trusting_same_module(opcode, operand, dest_ty, dl)
}

/// [`constant_fold_cast_operand`] for an operand and type of one module.
pub(crate) fn constant_fold_cast_operand_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Mirrors `ConstantFoldCastOperand`, arm for arm.
    if let Some((first_opcode, source)) = constant_cast_expr(operand)
        && let Some(new_opcode) = is_eliminable_cast_pair(
            first_opcode,
            opcode,
            source.ty(),
            operand.ty(),
            dest_ty,
            Some(dl),
        )
    {
        return constant_fold_cast_operand_trusting_same_module(new_opcode, source, dest_ty, dl);
    }

    match opcode {
        CastOpcode::PtrToAddr | CastOpcode::PtrToInt => {
            if let Some(folded_value) = fold_pointer_to_integer_operand(opcode, operand, dl)? {
                // Do a zext or trunc to get to the ptrtoint/ptrtoaddr dest size.
                return fold_integer_cast_constant(folded_value, dest_ty, Signedness::Unsigned, dl);
            }
        }
        CastOpcode::IntToPtr => {
            // If the input is a ptrtoint, turn the pair into a ptr to ptr bitcast
            // if the int size is >= the ptr size and the address spaces are the
            // same. This requires knowing the width of a pointer, so it can't be
            // done in ConstantExpr::getCast.
            if let Some((CastOpcode::PtrToInt, source)) = constant_cast_expr(operand)
                && let Some(source_space) = pointer_address_space(source.ty().scalar_type())
                && operand.ty().scalar_size_in_bits() >= dl.pointer_size_in_bits(source_space)
                && Some(source_space) == pointer_address_space(dest_ty.scalar_type())
            {
                return fold_bitcast_with_layout(source, dest_ty, dl);
            }
        }
        CastOpcode::Trunc
        | CastOpcode::Zext
        | CastOpcode::Sext
        | CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::UiToFp
        | CastOpcode::SiToFp
        | CastOpcode::FpToUi
        | CastOpcode::FpToSi
        | CastOpcode::AddrSpaceCast => {}
        CastOpcode::BitCast => return fold_bitcast_with_layout(operand, dest_ty, dl),
    }

    if opcode.is_desirable_constant_expr() {
        let Some(expr_opcode) = cast_constant_expr_opcode(opcode) else {
            return Ok(None);
        };
        return operand
            .module
            .constant_expr(
                dest_ty,
                expr_opcode,
                [operand.as_erased()],
                [],
                [],
                ConstantExprFlags::none(),
            )
            .map(Some);
    }
    constant_fold_cast_instruction_trusting_same_module(opcode, operand, dest_ty)
}

/// The `PtrToAddr` / `PtrToInt` case of `ConstantFoldCastOperand`, to its
/// `FoldedValue`, which is `None` where upstream's is null.
fn fold_pointer_to_integer_operand<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    opcode: CastOpcode,
    operand: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // If the input is an inttoptr, eliminate the pair. This requires knowing the
    // width of a pointer, so it can't be done in ConstantExpr::getCast.
    if let Some((CastOpcode::IntToPtr, source)) = constant_cast_expr(operand) {
        // zext/trunc the inttoptr to pointer/address size. Upstream's ternary reads
        // the other way round from `isEliminableCastPair`'s case 11, which has
        // run already and left only a pair whose source and destination are both
        // wider than that case's `MidSize`.
        let mid_ty = if opcode == CastOpcode::PtrToInt {
            index_type(operand.ty(), dl)?
        } else {
            int_ptr_type(operand.ty(), dl)?
        };
        return fold_integer_cast_constant(source, mid_ty, Signedness::Unsigned, dl);
    }

    let Some(gep) = GepOperator::of(operand.as_erased()) else {
        return Ok(None);
    };
    let module = operand.module;
    // If we have GEP, we can perform the following folds:
    // (ptrtoint/ptrtoaddr (gep null, x)) -> x
    // (ptrtoint/ptrtoaddr (gep (gep null, x), y) -> x + y, etc.
    let bit_width = index_type_size_in_bits(operand.ty(), dl);
    let (base, base_offset) = strip_and_accumulate_constant_offsets(
        operand.as_erased(),
        ApInt::zero(bit_width),
        ConstantOffsetStrip {
            allow_non_inbounds: true,
            look_through_int_to_ptr: false,
        },
        dl,
    );
    let Some(base) = constant_from_id(module, base.slot_trusting_same_module()) else {
        return Ok(None);
    };
    if base.is_null_value() {
        return Ok(Some(
            int_type(module, bit_width)?
                .const_ap_int(&base_offset)?
                .as_constant(),
        ));
    }

    // ptrtoint/ptrtoaddr (gep i8, Ptr, (sub 0, V))
    //   -> sub (ptrtoint/ptrtoaddr Ptr), V
    let Some(index) = gep.sole_index_of_byte_gep_expr() else {
        return Ok(None);
    };
    let Some(ptr) = constant_from_id(module, gep.pointer_operand.slot_trusting_same_module())
    else {
        return Ok(None);
    };
    let Some(sub) = constant_from_id(module, index) else {
        return Ok(None);
    };
    let int_index_ty = index_type(ptr.ty(), dl)?;
    let Some(sub_expr) = constant_expr_data(sub) else {
        return Ok(None);
    };
    let [minuend, subtrahend] = sub_expr.operands.as_ref() else {
        return Ok(None);
    };
    let (Some(minuend), Some(subtrahend)) = (
        constant_from_id(module, *minuend),
        constant_from_id(module, *subtrahend),
    ) else {
        return Ok(None);
    };
    if sub.ty() != int_index_ty
        || sub_expr.opcode != ConstantExprOpcode::Sub
        || !minuend.is_null_value()
    {
        return Ok(None);
    }
    let Some(cast_opcode) = cast_constant_expr_opcode(opcode) else {
        return Ok(None);
    };
    let cast = module.constant_expr(
        int_index_ty,
        cast_opcode,
        [ptr.as_erased()],
        [],
        [],
        ConstantExprFlags::none(),
    )?;
    module
        .constant_expr(
            int_index_ty,
            ConstantExprOpcode::Sub,
            [cast.as_erased(), subtrahend.as_erased()],
            [],
            [],
            ConstantExprFlags::none(),
        )
        .map(Some)
}

/// Constant fold a zext, sext or trunc according to source and destination width.
///
/// Errors with [`IrError::ForeignType`] if `dest_ty` belongs to a module other
/// than `constant`'s.
pub fn constant_fold_integer_cast<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    signedness: Signedness,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's destination type, admitted against `constant`'s
    // module.
    dest_ty.slot_in(constant.module.id())?;
    constant_fold_integer_cast_trusting_same_module(constant, dest_ty, signedness, dl)
}

/// [`constant_fold_integer_cast`] for a constant and type of one module.
pub(crate) fn constant_fold_integer_cast_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    signedness: Signedness,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    fold_integer_cast_constant(constant, dest_ty, signedness, dl)
}

/// Fold an instruction using DataLayout-aware analysis rules.
pub fn constant_fold_instruction<'ctx, B, C>(
    instruction: &InstructionView<'ctx, B, C>,
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Option<Constant<'ctx, B, C>>>
where
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    let value = instruction.as_erased();
    let ValueKindData::Instruction(data) = &value.data().kind else {
        return Ok(None);
    };

    if let InstructionKindData::Phi(phi) = &data.kind {
        return Ok(fold_phi(value.ty(), value.module, phi));
    }

    if let InstructionKindData::Call(call) = &data.kind {
        let Some(tli) = tli else {
            return Ok(None);
        };
        let callee_data = value.module.context().value_data(call.callee.get());
        let ValueKindData::Function(_) = &callee_data.kind else {
            return Ok(None);
        };
        let function =
            FunctionValue::<Dyn, B, C>::from_parts_unchecked(call.callee.get(), value.module);
        let Some(lib_func) = tli.lib_func_for_name(&function.name().unwrap_or_default()) else {
            return Ok(None);
        };
        let mut args = Vec::with_capacity(call.args.len());
        for arg in call.args.iter().map(|arg| arg.get()) {
            let Some(constant) = constant_from_id(value.module, arg) else {
                return Ok(None);
            };
            args.push(constant_fold_constant(constant, dl, Some(tli))?);
        }
        return constant_fold_call_trusting_same_module(
            lib_func,
            &args,
            value.ty(),
            tli,
            FoldNonDeterminism::Allow,
        );
    }

    let mut operands = Vec::new();
    for id in data.kind.operand_ids() {
        let Some(constant) = constant_from_id(value.module, id) else {
            return Ok(None);
        };
        operands.push(constant_fold_constant(constant, dl, tli)?);
    }

    constant_fold_inst_operands_trusting_same_module(
        instruction,
        &operands,
        dl,
        tli,
        FoldNonDeterminism::Allow,
    )
}

/// Fold a constant using DataLayout-aware analysis rules.
pub fn constant_fold_constant<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Constant<'ctx, B, C>> {
    let module = constant.module;
    match &constant.as_erased().data().kind {
        ValueKindData::Constant(ConstantData::Expr(expr)) => {
            let Some(operands) =
                folded_constants_from_ids(module, expr.operands.iter().copied(), dl, tli)?
            else {
                return Ok(constant);
            };
            if let Some(folded) =
                constant_fold_constant_expr_operands(constant, expr, &operands, dl, tli)?
            {
                Ok(folded)
            } else {
                Ok(constant)
            }
        }
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            let mut changed = false;
            let mut folded_ids = Vec::with_capacity(elements.len());
            for id in elements.iter().copied() {
                let Some(element) = constant_from_id(module, id) else {
                    return Ok(constant);
                };
                let folded = constant_fold_constant(element, dl, tli)?;
                // Internal: `folded` was folded from this aggregate's own
                // element, in its module.
                let folded_id = folded.slot_trusting_same_module();
                changed |= folded_id != id;
                folded_ids.push(folded_id);
            }
            if !changed {
                return Ok(constant);
            }
            let id = module.context().intern_constant_aggregate(
                constant.ty().slot_trusting_same_module(),
                folded_ids.into_boxed_slice(),
            );
            Ok(Constant::from_parts(Value::from_parts(
                id,
                module,
                constant.ty().slot_trusting_same_module(),
            )))
        }
        _ => Ok(constant),
    }
}

/// Fold an instruction with caller-provided constant operands.
///
/// Errors with [`IrError::ForeignValueId`] if an operand belongs to a module
/// other than `instruction`'s.
pub fn constant_fold_inst_operands<'ctx, B, C: Capability>(
    instruction: &InstructionView<'ctx, B, C>,
    operands: &[Constant<'ctx, B, C>],
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B, C>>>
where
    B: ModuleBrand + 'ctx,
{
    // Boundary: the caller's operands, admitted against `instruction`'s
    // module.
    let owner = instruction.as_erased().module.id();
    for operand in operands {
        operand.slot_in(owner)?;
    }
    constant_fold_inst_operands_trusting_same_module(
        instruction,
        operands,
        dl,
        tli,
        allow_non_deterministic,
    )
}

/// [`constant_fold_inst_operands`] for operands of the instruction's module.
pub(crate) fn constant_fold_inst_operands_trusting_same_module<'ctx, B, C: Capability>(
    instruction: &InstructionView<'ctx, B, C>,
    operands: &[Constant<'ctx, B, C>],
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B, C>>>
where
    B: ModuleBrand + 'ctx,
{
    let value = instruction.as_erased();
    let ValueKindData::Instruction(data) = &value.data().kind else {
        return Ok(None);
    };

    if let Some(opcode) = binary_opcode(&data.kind) {
        let [lhs, rhs] = operands else {
            return Ok(None);
        };
        return if matches!(
            opcode,
            BinaryOpcode::Fadd
                | BinaryOpcode::Fsub
                | BinaryOpcode::Fmul
                | BinaryOpcode::Fdiv
                | BinaryOpcode::Frem
        ) {
            constant_fold_fp_inst_operands_trusting_same_module(
                opcode,
                *lhs,
                *rhs,
                dl,
                denormal_mode_for_instruction(data.parent.get(), value.module, value.ty()),
                binary_op_fmf(&data.kind),
                allow_non_deterministic,
            )
        } else {
            constant_fold_binary_op_operands_trusting_same_module(opcode, *lhs, *rhs, dl)
        };
    }

    match &data.kind {
        InstructionKindData::Cast(cast) => {
            let [operand] = operands else {
                return Ok(None);
            };
            constant_fold_cast_operand_trusting_same_module(cast.kind, *operand, value.ty(), dl)
        }
        InstructionKindData::Icmp(cmp) => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            constant_fold_compare_inst_operands_trusting_same_module(
                CmpPredicate::Int(cmp.predicate),
                *lhs,
                *rhs,
                dl,
                None,
            )
        }
        InstructionKindData::Fcmp(cmp) => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            constant_fold_compare_inst_operands_trusting_same_module(
                CmpPredicate::Float(cmp.predicate),
                *lhs,
                *rhs,
                dl,
                Some(denormal_mode_for_instruction(
                    data.parent.get(),
                    value.module,
                    lhs.ty(),
                )),
            )
        }
        InstructionKindData::Fneg(_) => {
            let [operand] = operands else {
                return Ok(None);
            };
            constant_fold_unary_op_operand(UnaryOpcode::Fneg, *operand, dl)
        }
        InstructionKindData::Select(_) => {
            let [condition, true_value, false_value] = operands else {
                return Ok(None);
            };
            constant_fold_select_instruction_trusting_same_module(
                *condition,
                *true_value,
                *false_value,
            )
        }
        InstructionKindData::ExtractElement(_) => {
            let [vector, index] = operands else {
                return Ok(None);
            };
            constant_fold_extract_element_instruction_trusting_same_module(*vector, *index)
        }
        InstructionKindData::InsertElement(_) => {
            let [vector, value, index] = operands else {
                return Ok(None);
            };
            constant_fold_insert_element_instruction_trusting_same_module(*vector, *value, *index)
        }
        InstructionKindData::ShuffleVector(shuffle) => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            constant_fold_shuffle_vector_instruction_trusting_same_module(*lhs, *rhs, &shuffle.mask)
        }
        InstructionKindData::ExtractValue(extract) => {
            let [aggregate] = operands else {
                return Ok(None);
            };
            Ok(constant_fold_extract_value_instruction(
                *aggregate,
                &extract.indices,
            ))
        }
        InstructionKindData::InsertValue(insert) => {
            let [aggregate, value] = operands else {
                return Ok(None);
            };
            constant_fold_insert_value_instruction_trusting_same_module(
                *aggregate,
                *value,
                &insert.indices,
            )
        }
        InstructionKindData::Gep(gep) => {
            let Some((pointer, indices)) = operands.split_first() else {
                return Ok(None);
            };
            let source_ty = Type::new(gep.source_ty, value.module);
            // `ConstantExpr::isSupportedGetElementPtr`. The constant-expression
            // arm needs no such line: `constant_expr` refuses that source.
            if source_ty.is_scalable() {
                return Ok(None);
            }
            if let Some(folded) =
                symbolically_evaluate_gep(source_ty, *pointer, indices, gep.flags, None, dl)?
            {
                return Ok(Some(folded));
            }
            constant_fold_get_element_ptr_trusting_same_module(source_ty, *pointer, indices, None)
        }
        InstructionKindData::Load(load) => {
            if load.volatile {
                return Ok(None);
            }
            let [pointer] = operands else {
                return Ok(None);
            };
            let Some(index_bits) = index_bits_for_pointer(pointer.ty(), dl) else {
                return Ok(None);
            };
            constant_fold_load_from_const_ptr_trusting_same_module(
                *pointer,
                value.ty(),
                ApInt::zero(index_bits),
                dl,
            )
        }
        // Mirrors `ConstantFoldInstOperandsImpl`'s Freeze arm in
        // `llvm/lib/Analysis/ConstantFolding.cpp`: only forward operands proven
        // not to be undef or poison.
        InstructionKindData::Freeze(_) => {
            let [operand] = operands else {
                return Ok(None);
            };
            if is_guaranteed_not_to_be_undef_or_poison(*operand) {
                Ok(Some(*operand))
            } else {
                Ok(None)
            }
        }
        InstructionKindData::Call(call) => {
            let Some(tli) = tli else {
                return Ok(None);
            };
            let callee_data = value.module.context().value_data(call.callee.get());
            let ValueKindData::Function(_) = &callee_data.kind else {
                return Ok(None);
            };
            let function =
                FunctionValue::<Dyn, B, C>::from_parts_unchecked(call.callee.get(), value.module);
            let Some(lib_func) = tli.lib_func_for_name(&function.name().unwrap_or_default()) else {
                return Ok(None);
            };
            let args = if operands.len() == call.args.len() {
                operands
            } else if operands.len() == call.args.len().saturating_add(1) {
                &operands[1..]
            } else {
                return Ok(None);
            };
            constant_fold_call_trusting_same_module(
                lib_func,
                args,
                value.ty(),
                tli,
                allow_non_deterministic,
            )
        }
        InstructionKindData::Add(_)
        | InstructionKindData::Sub(_)
        | InstructionKindData::Mul(_)
        | InstructionKindData::Udiv(_)
        | InstructionKindData::Sdiv(_)
        | InstructionKindData::Urem(_)
        | InstructionKindData::Srem(_)
        | InstructionKindData::Shl(_)
        | InstructionKindData::Lshr(_)
        | InstructionKindData::Ashr(_)
        | InstructionKindData::And(_)
        | InstructionKindData::Or(_)
        | InstructionKindData::Xor(_)
        | InstructionKindData::Fadd(_)
        | InstructionKindData::Fsub(_)
        | InstructionKindData::Fmul(_)
        | InstructionKindData::Fdiv(_)
        | InstructionKindData::Frem(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Ret(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::VaArg(_)
        | InstructionKindData::Fence(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRmw(_)
        | InstructionKindData::Switch(_)
        | InstructionKindData::IndirectBr(_)
        | InstructionKindData::Invoke(_)
        | InstructionKindData::CallBr(_)
        | InstructionKindData::LandingPad(_)
        | InstructionKindData::Resume(_)
        | InstructionKindData::CleanupPad(_)
        | InstructionKindData::CatchPad(_)
        | InstructionKindData::CatchReturn(_)
        | InstructionKindData::CleanupReturn(_)
        | InstructionKindData::CatchSwitch(_)
        | InstructionKindData::Unreachable(_) => Ok(None),
    }
}

/// Fold an integer or floating-point compare with caller-provided operands.
///
/// Mirrors `llvm::ConstantFoldCompareInstOperands`
/// (`llvm/lib/Analysis/ConstantFolding.cpp`). Before falling back to the
/// target-independent compare, try (in upstream's exact order, with upstream's
/// exact "commit once the precondition holds, don't keep trying more folds"
/// control flow):
///
/// 1. `icmp pred (inttoptr x), null` -> `icmp pred x, 0`, and its
///    `icmp pred (ptrtoint x), 0` / `ptrtoaddr` sibling. The swapped
///    `null`/`(inttoptr x)` forms are reached via the trailing operand swap
///    (case 4 below).
/// 2. `icmp pred (inttoptr x), (inttoptr y)` -> `icmp pred x, y`, and the
///    `ptrtoint`/`ptrtoaddr` sibling.
/// 3. Base+offset stripping: `(base+off1) pred (base+off2)` ->
///    `off1 pred off2`, for two pointers that peel down to the same
///    underlying base.
/// 4. If only the right-hand operand is a constant expression, swap
///    operands and predicate and retry.
///
/// Errors with [`IrError::ForeignValueId`] if `rhs` belongs to a module other
/// than `lhs`'s.
pub fn constant_fold_compare_inst_operands<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    predicate: CmpPredicate,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
    denormal_mode: Option<DenormalMode>,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's operands, admitted against `lhs`'s module.
    rhs.slot_in(lhs.module.id())?;
    constant_fold_compare_inst_operands_trusting_same_module(predicate, lhs, rhs, dl, denormal_mode)
}

/// [`constant_fold_compare_inst_operands`] for operands of one module.
pub(crate) fn constant_fold_compare_inst_operands_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    predicate: CmpPredicate,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
    denormal_mode: Option<DenormalMode>,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    if is_constant_expr_like(lhs) {
        if let Some(result) = fold_ptr_int_cast_vs_null(predicate, lhs, rhs, dl, denormal_mode)? {
            return Ok(result);
        }
        if is_constant_expr_like(rhs)
            && let Some(result) = fold_matching_cast_pair(predicate, lhs, rhs, dl, denormal_mode)?
        {
            return Ok(result);
        }
        if let CmpPredicate::Int(int_pred) = predicate
            && matches!(lhs.ty().data(), TypeData::Pointer { .. })
            && !int_pred.is_signed()
            && let Some(result) = fold_pointer_base_offset(int_pred, lhs, rhs, dl)
        {
            return Ok(Some(result));
        }
    } else if is_constant_expr_like(rhs) {
        // If RHS is a constant expression, but the left side isn't, swap the
        // operands and try again.
        return constant_fold_compare_inst_operands_trusting_same_module(
            swap_cmp_predicate(predicate),
            rhs,
            lhs,
            dl,
            denormal_mode,
        );
    }

    let lhs = if lhs.ty().is_floating_point() {
        if let Some(mode) = denormal_mode {
            let Some(flushed) = flush_fp_constant(lhs, mode, DenormalModeSide::Input)? else {
                return Ok(None);
            };
            flushed
        } else {
            lhs
        }
    } else {
        lhs
    };
    let rhs = if rhs.ty().is_floating_point() {
        if let Some(mode) = denormal_mode {
            let Some(flushed) = flush_fp_constant(rhs, mode, DenormalModeSide::Input)? else {
                return Ok(None);
            };
            flushed
        } else {
            rhs
        }
    } else {
        rhs
    };
    constant_fold_compare_instruction_trusting_same_module(predicate, lhs, rhs)
}

/// `dyn_cast<ConstantExpr>(V) != nullptr`, extended to llvmkit's compact
/// `GepOffset` encoding of an inbounds `getelementptr` constant expression
/// (see that variant's doc comment) so the folds below see through both
/// representations exactly as upstream sees through the single
/// `ConstantExpr` representation it has.
fn is_constant_expr_like<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    matches!(
        &constant.as_erased().data().kind,
        ValueKindData::Constant(ConstantData::Expr(_) | ConstantData::GepOffset { .. })
    )
}

/// `dyn_cast<ConstantExpr>(V)`, general-expression form only. llvmkit's
/// compact `GepOffset` encoding never carries one of the opcodes the folds
/// below match against, so the narrower match is equivalent for their
/// purposes.
fn constant_expr_data<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> Option<&'ctx ConstantExprData> {
    match &constant.as_erased().data().kind {
        ValueKindData::Constant(ConstantData::Expr(expr)) => Some(expr),
        _ => None,
    }
}

/// fold: `icmp pred (inttoptr x), null` -> `icmp pred x, 0`
/// fold: `icmp pred (ptrtoint x), 0` (or `ptrtoaddr`) -> `icmp pred x, null`
///
/// Mirrors the `Ops1->isNullValue()` block of
/// `ConstantFoldCompareInstOperands` (`ConstantFolding.cpp` lines
/// 1213-1236). The outer `Option` reports whether upstream's opcode/width
/// precondition held and committed to a recursive fold; when it did, the
/// inner `Option` is that recursion's own (possibly declining) answer,
/// which the caller must return as-is rather than trying more folds,
/// mirroring upstream's unconditional `return` inside the `if`.
fn fold_ptr_int_cast_vs_null<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    predicate: CmpPredicate,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
    denormal_mode: Option<DenormalMode>,
) -> IrResult<Option<Option<Constant<'ctx, B, C>>>> {
    if !Constant::is_null_value(rhs) {
        return Ok(None);
    }
    let Some(expr) = constant_expr_data(lhs) else {
        return Ok(None);
    };
    let module = lhs.module;

    if expr.opcode == ConstantExprOpcode::IntToPtr
        && let [operand_id] = expr.operands.as_ref()
        && let Some(operand) = constant_from_id(module, *operand_id)
        && let Some(addr_space) = pointer_address_space(lhs.ty())
    {
        let int_ptr_ty = int_type(module, dl.pointer_size_in_bits(addr_space))?;
        if let Some(casted) =
            fold_integer_cast_constant(operand, int_ptr_ty.as_type(), Signedness::Unsigned, dl)?
        {
            let null = int_ptr_ty.const_zero().as_constant();
            return Ok(Some(
                constant_fold_compare_inst_operands_trusting_same_module(
                    predicate,
                    casted,
                    null,
                    dl,
                    denormal_mode,
                )?,
            ));
        }
    }

    // icmp only compares the address part of the pointer, so only do this
    // transform if the integer size matches the address size.
    if matches!(
        expr.opcode,
        ConstantExprOpcode::PtrToInt | ConstantExprOpcode::PtrToAddr
    ) && let [operand_id] = expr.operands.as_ref()
        && let Some(operand) = constant_from_id(module, *operand_id)
        && let Some(addr_bits) = index_bits_for_pointer(operand.ty(), dl)
        && let Ok(lhs_int_ty) = IntType::<IntDyn, B, C>::try_from(lhs.ty())
        && lhs_int_ty.bit_width() == addr_bits
        && let Ok(ptr_ty) = PointerType::<crate::SpaceDyn, B, C>::try_from(operand.ty())
    {
        let null = ptr_ty.const_null().as_constant();
        return Ok(Some(
            constant_fold_compare_inst_operands_trusting_same_module(
                predicate,
                operand,
                null,
                dl,
                denormal_mode,
            )?,
        ));
    }

    Ok(None)
}

/// fold: `icmp pred (inttoptr x), (inttoptr y)` -> `icmp pred x, y`
/// fold: `icmp pred (ptrtoint x), (ptrtoint y)` -> `icmp pred x, y` (or
/// `ptrtoaddr`)
///
/// Mirrors the `CE0->getOpcode() == CE1->getOpcode()` block of
/// `ConstantFoldCompareInstOperands` (`ConstantFolding.cpp` lines
/// 1239-1266). The caller has already confirmed both `lhs` and `rhs` are
/// constant expressions. Same "commit once the precondition holds" contract
/// as [`fold_ptr_int_cast_vs_null`].
fn fold_matching_cast_pair<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    predicate: CmpPredicate,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
    denormal_mode: Option<DenormalMode>,
) -> IrResult<Option<Option<Constant<'ctx, B, C>>>> {
    let Some(lhs_expr) = constant_expr_data(lhs) else {
        return Ok(None);
    };
    let Some(rhs_expr) = constant_expr_data(rhs) else {
        return Ok(None);
    };
    if lhs_expr.opcode != rhs_expr.opcode {
        return Ok(None);
    }
    let module = lhs.module;

    if lhs_expr.opcode == ConstantExprOpcode::IntToPtr
        && let [lhs_operand_id] = lhs_expr.operands.as_ref()
        && let [rhs_operand_id] = rhs_expr.operands.as_ref()
        && let Some(lhs_operand) = constant_from_id(module, *lhs_operand_id)
        && let Some(rhs_operand) = constant_from_id(module, *rhs_operand_id)
        && let Some(addr_space) = pointer_address_space(lhs.ty())
    {
        let int_ptr_ty = int_type(module, dl.pointer_size_in_bits(addr_space))?;
        // Convert both integer values to the right size to ensure we get
        // the proper extension or truncation.
        if let Some(c0) =
            fold_integer_cast_constant(lhs_operand, int_ptr_ty.as_type(), Signedness::Unsigned, dl)?
            && let Some(c1) = fold_integer_cast_constant(
                rhs_operand,
                int_ptr_ty.as_type(),
                Signedness::Unsigned,
                dl,
            )?
        {
            return Ok(Some(
                constant_fold_compare_inst_operands_trusting_same_module(
                    predicate,
                    c0,
                    c1,
                    dl,
                    denormal_mode,
                )?,
            ));
        }
    }

    // icmp only compares the address part of the pointer, so only do this
    // transform if the integer size matches the address size.
    if matches!(
        lhs_expr.opcode,
        ConstantExprOpcode::PtrToInt | ConstantExprOpcode::PtrToAddr
    ) && let [lhs_operand_id] = lhs_expr.operands.as_ref()
        && let [rhs_operand_id] = rhs_expr.operands.as_ref()
        && let Some(lhs_operand) = constant_from_id(module, *lhs_operand_id)
        && let Some(rhs_operand) = constant_from_id(module, *rhs_operand_id)
        && let Some(addr_bits) = index_bits_for_pointer(lhs_operand.ty(), dl)
        && let Ok(lhs_int_ty) = IntType::<IntDyn, B, C>::try_from(lhs.ty())
        && lhs_int_ty.bit_width() == addr_bits
        && lhs_operand.ty() == rhs_operand.ty()
    {
        return Ok(Some(
            constant_fold_compare_inst_operands_trusting_same_module(
                predicate,
                lhs_operand,
                rhs_operand,
                dl,
                denormal_mode,
            )?,
        ));
    }

    Ok(None)
}

/// Convert pointer comparison `(base+offset1) pred (base+offset2)` into
/// `offset1 pred offset2`, for two pointers that strip down to the same
/// underlying base. Mirrors the final `if` block of
/// `ConstantFoldCompareInstOperands` (`ConstantFolding.cpp` lines
/// 1268-1291). The caller has already confirmed `lhs` is a constant
/// expression, `lhs`'s type is a (scalar) pointer, and `predicate` is not a
/// signed comparison.
///
/// Bases are compared with `==`, exactly as upstream compares the stripped
/// `Constant*` pointers (`Stripped0 == Stripped1`). That is sound in both
/// trees for the same reason: LLVM uniques `Constant*`, and llvmkit uniques
/// its `GlobalValueRef` / `GepOffset` constants, so two independently-built
/// pointers into the same global strip down to the same arena id.
fn fold_pointer_base_offset<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    predicate: IntPredicate,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> Option<Constant<'ctx, B, C>> {
    let index_bits = index_bits_for_pointer(lhs.ty(), dl)?;
    // This only works for equality and unsigned comparison, as inbounds
    // permits crossing the sign boundary. However, the offset comparison
    // itself is signed.
    let is_eq_pred = matches!(predicate, IntPredicate::Eq | IntPredicate::Ne);
    let strip = ConstantOffsetStrip {
        allow_non_inbounds: is_eq_pred,
        look_through_int_to_ptr: is_eq_pred,
    };
    let (lhs_base, lhs_offset) =
        strip_and_accumulate_constant_offsets(lhs.as_erased(), ApInt::zero(index_bits), strip, dl);
    let (rhs_base, rhs_offset) =
        strip_and_accumulate_constant_offsets(rhs.as_erased(), ApInt::zero(index_bits), strip, dl);
    if lhs_base != rhs_base {
        return None;
    }
    let signed_predicate = predicate.flip_signedness();
    let result = compare_ap_int_with_predicate(signed_predicate, &lhs_offset, &rhs_offset);
    let module = lhs.as_erased().module;
    Some(module.bool_type().const_int(result).as_constant())
}

/// `IcmpInst::compare` (`llvm/include/llvm/IR/InstrTypes.h`): evaluate a
/// signed/unsigned/equality integer predicate directly against two
/// `APInt`s.
fn compare_ap_int_with_predicate(predicate: IntPredicate, lhs: &ApInt, rhs: &ApInt) -> bool {
    match predicate {
        IntPredicate::Eq => lhs.eq_ap_int(rhs),
        IntPredicate::Ne => !lhs.eq_ap_int(rhs),
        IntPredicate::Ugt => lhs.ugt(rhs),
        IntPredicate::Uge => lhs.uge(rhs),
        IntPredicate::Ult => lhs.ult(rhs),
        IntPredicate::Ule => lhs.ule(rhs),
        IntPredicate::Sgt => lhs.sgt(rhs),
        IntPredicate::Sge => lhs.sge(rhs),
        IntPredicate::Slt => lhs.slt(rhs),
        IntPredicate::Sle => lhs.sle(rhs),
    }
}

/// `CmpInst::getSwappedPredicate` (`llvm/lib/IR/Instructions.cpp`),
/// dispatched across the combined int/float predicate space.
fn swap_cmp_predicate(predicate: CmpPredicate) -> CmpPredicate {
    match predicate {
        CmpPredicate::Int(pred) => CmpPredicate::Int(pred.swapped()),
        CmpPredicate::Float(pred) => CmpPredicate::Float(pred.swapped()),
    }
}

/// Fold a unary operation with a caller-provided constant operand.
pub fn constant_fold_unary_op_operand<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    opcode: UnaryOpcode,
    operand: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    let _ = dl;
    constant_fold_unary_instruction(opcode, operand)
}

/// Fold a binary operation with caller-provided constant operands.
///
/// Errors with [`IrError::ForeignValueId`] if `rhs` belongs to a module other
/// than `lhs`'s.
pub fn constant_fold_binary_op_operands<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's operands, admitted against `lhs`'s module.
    rhs.slot_in(lhs.module.id())?;
    constant_fold_binary_op_operands_trusting_same_module(opcode, lhs, rhs, dl)
}

/// [`constant_fold_binary_op_operands`] for operands of one module.
pub(crate) fn constant_fold_binary_op_operands_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    if let Some(folded) = constant_fold_binary_instruction_trusting_same_module(opcode, lhs, rhs)? {
        return Ok(Some(folded));
    }
    let _ = dl;
    if lhs.ty() != rhs.ty() || !opcode.is_supported_constant_expr() {
        return Ok(None);
    }
    let Some(expr_opcode) = binary_constant_expr_opcode(opcode) else {
        return Ok(None);
    };
    lhs.module
        .constant_expr(
            lhs.ty(),
            expr_opcode,
            [lhs.as_erased(), rhs.as_erased()],
            [],
            [],
            ConstantExprFlags::none(),
        )
        .map(Some)
}

/// Fold a floating-point binary operation with denormal handling.
///
/// Errors with [`IrError::ForeignValueId`] if `rhs` belongs to a module other
/// than `lhs`'s.
pub fn constant_fold_fp_inst_operands<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
    denormal_mode: DenormalMode,
    fmf: FastMathFlags,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's operands, admitted against `lhs`'s module.
    rhs.slot_in(lhs.module.id())?;
    constant_fold_fp_inst_operands_trusting_same_module(
        opcode,
        lhs,
        rhs,
        dl,
        denormal_mode,
        fmf,
        allow_non_deterministic,
    )
}

/// [`constant_fold_fp_inst_operands`] for operands of one module.
pub(crate) fn constant_fold_fp_inst_operands_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    opcode: BinaryOpcode,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    dl: &DataLayout,
    denormal_mode: DenormalMode,
    fmf: FastMathFlags,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    let Some(lhs) = flush_fp_constant(lhs, denormal_mode, DenormalModeSide::Input)? else {
        return Ok(None);
    };
    let Some(rhs) = flush_fp_constant(rhs, denormal_mode, DenormalModeSide::Input)? else {
        return Ok(None);
    };
    // Mirrors `ConstantFoldFPInstOperands`'s non-determinism guard in
    // `llvm/lib/Analysis/ConstantFolding.cpp`: nsz/reassoc/contract/arcp let a
    // later optimization change the FP result, so deterministic folding must
    // decline rather than commit to one answer.
    if matches!(allow_non_deterministic, FoldNonDeterminism::Deny)
        && fmf.intersects(
            FastMathFlags::NO_SIGNED_ZEROS
                | FastMathFlags::ALLOW_REASSOC
                | FastMathFlags::ALLOW_CONTRACT
                | FastMathFlags::ALLOW_RECIPROCAL,
        )
    {
        return Ok(None);
    }
    let Some(folded) = constant_fold_binary_op_operands_trusting_same_module(opcode, lhs, rhs, dl)?
    else {
        return Ok(None);
    };
    let Some(folded) = flush_fp_constant(folded, denormal_mode, DenormalModeSide::Output)? else {
        return Ok(None);
    };
    if matches!(allow_non_deterministic, FoldNonDeterminism::Deny) && constant_is_nan(folded) {
        return Ok(None);
    }
    Ok(Some(folded))
}

/// Fold a binary intrinsic of constant operands. Folds nothing yet: upstream's
/// `ConstantFoldBinaryIntrinsic` (`lib/Analysis/ConstantFolding.cpp`) hands
/// its operands to `ConstantFoldIntrinsicCall2`, which llvmkit has not ported
/// (`docs/future-work.md`), so every input answers `Ok(None)` — no fold,
/// which is always correct and never as strong as upstream.
///
/// Errors with [`IrError::ForeignValueId`] if `rhs`, or
/// [`IrError::ForeignType`] if `ty`, belongs to a module other than `lhs`'s.
pub fn constant_fold_binary_intrinsic<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    intrinsic: BinaryIntrinsic,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's operand and type, admitted against `lhs`'s
    // module.
    let owner = lhs.module.id();
    rhs.slot_in(owner)?;
    ty.slot_in(owner)?;
    Ok(constant_fold_binary_intrinsic_trusting_same_module(
        intrinsic, lhs, rhs, ty, dl,
    ))
}

/// [`constant_fold_binary_intrinsic`] for operands and a type of one module.
pub(crate) fn constant_fold_binary_intrinsic_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    intrinsic: BinaryIntrinsic,
    lhs: Constant<'ctx, B, C>,
    rhs: Constant<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> Option<Constant<'ctx, B, C>> {
    // Unported: `ConstantFoldIntrinsicCall2` (see `constant_fold_binary_intrinsic`).
    let _ = (intrinsic, lhs, rhs, ty, dl);
    None
}

/// Reinterpret `constant` as though it were loaded through a bitcasted pointer.
///
/// Errors with [`IrError::ForeignType`] if `dest_ty` belongs to a module other
/// than `constant`'s.
pub fn constant_fold_load_through_bitcast<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's destination type, admitted against `constant`'s
    // module.
    dest_ty.slot_in(constant.module.id())?;
    constant_fold_load_through_bitcast_trusting_same_module(constant, dest_ty, dl)
}

/// [`constant_fold_load_through_bitcast`] for a constant and type of one
/// module.
pub(crate) fn constant_fold_load_through_bitcast_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    let mut current = Some(constant);
    while let Some(value) = current {
        if value.ty() == dest_ty {
            return Ok(Some(value));
        }
        let dest_size = dl.type_size_in_bits(erase_type(dest_ty));
        let src_size = dl.type_size_in_bits(erase_type(value.ty()));
        if !TypeSize::is_known_ge(src_size, dest_size) {
            return Ok(None);
        }
        if let Some(folded) =
            constant_fold_load_from_uniform_value_trusting_same_module(value, dest_ty, dl)?
        {
            return Ok(Some(folded));
        }
        // Mirrors `ConstantFoldLoadThroughBitcast`: the all-zero/uniform case
        // above may legally coerce non-integral pointers, but direct
        // pointer/int casts require both sides to agree on non-integrality.
        if src_size == dest_size
            && dl.is_non_integral_pointer_type(value.ty())
                == dl.is_non_integral_pointer_type(dest_ty)
        {
            let opcode = load_through_bitcast_opcode(value.ty(), dest_ty);
            if cast_is_valid(opcode, value.ty(), dest_ty) {
                return constant_fold_cast_operand_trusting_same_module(opcode, value, dest_ty, dl);
            }
        }
        current = aggregate_first_element(value, dl);
    }
    Ok(None)
}

/// Return an inverse cast if recasting it with `cast_op` losslessly recovers `constant`.
///
/// Errors with [`IrError::ForeignType`] if `inv_cast_to` belongs to a module
/// other than `constant`'s.
pub fn lossless_inv_cast<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    inv_cast_to: Type<'ctx, B, C>,
    cast_op: CastOpcode,
    dl: &DataLayout,
) -> IrResult<Option<(Constant<'ctx, B, C>, PreservedCastFlags)>> {
    // Boundary: the caller's type, admitted against `constant`'s module before
    // either is read. `lossless_unsigned_trunc` and `lossless_signed_trunc`
    // forward their caller's pair here unread, so this is their check too.
    inv_cast_to.slot_in(constant.module.id())?;
    match cast_op {
        CastOpcode::BitCast => {
            let Some(folded) = constant_fold_cast_operand_trusting_same_module(
                CastOpcode::BitCast,
                constant,
                inv_cast_to,
                dl,
            )?
            else {
                return Ok(None);
            };
            Ok(Some((folded, PreservedCastFlags::none())))
        }
        CastOpcode::Trunc => {
            let Some(zext) = constant_fold_cast_operand_trusting_same_module(
                CastOpcode::Zext,
                constant,
                inv_cast_to,
                dl,
            )?
            else {
                return Ok(None);
            };
            let mut flags = PreservedCastFlags::none().no_unsigned_wrap();
            if constant_fold_cast_operand_trusting_same_module(
                CastOpcode::Sext,
                constant,
                inv_cast_to,
                dl,
            )? == Some(zext)
            {
                flags = flags.no_signed_wrap();
            }
            Ok(Some((zext, flags)))
        }
        CastOpcode::Zext | CastOpcode::Sext => {
            let Some(inv) = constant_fold_cast_operand_trusting_same_module(
                CastOpcode::Trunc,
                constant,
                inv_cast_to,
                dl,
            )?
            else {
                return Ok(None);
            };
            let Some(recast) =
                constant_fold_cast_operand_trusting_same_module(cast_op, inv, constant.ty(), dl)?
            else {
                return Ok(None);
            };
            if recast != constant {
                return Ok(None);
            }
            let mut flags = PreservedCastFlags::none();
            if cast_op == CastOpcode::Zext
                && constant_fold_cast_operand_trusting_same_module(
                    CastOpcode::Sext,
                    inv,
                    constant.ty(),
                    dl,
                )? == Some(recast)
            {
                flags = flags.non_negative();
            }
            Ok(Some((inv, flags)))
        }
        CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::FpToUi
        | CastOpcode::FpToSi
        | CastOpcode::UiToFp
        | CastOpcode::SiToFp
        | CastOpcode::PtrToInt
        | CastOpcode::PtrToAddr
        | CastOpcode::IntToPtr
        | CastOpcode::AddrSpaceCast => Ok(None),
    }
}

/// Return a truncation that can be losslessly unsigned-extended back to `constant`.
pub fn lossless_unsigned_trunc<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<(Constant<'ctx, B, C>, PreservedCastFlags)>> {
    lossless_inv_cast(constant, dest_ty, CastOpcode::Zext, dl)
}

/// Return a truncation that can be losslessly signed-extended back to `constant`.
pub fn lossless_signed_trunc<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<(Constant<'ctx, B, C>, PreservedCastFlags)>> {
    lossless_inv_cast(constant, dest_ty, CastOpcode::Sext, dl)
}
/// Fold a known library call with constant operands.
///
/// Errors with [`IrError::ForeignValueId`] if an operand belongs to a module
/// other than `result_ty`'s.
pub fn constant_fold_call<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    lib_func: LibFunc,
    operands: &[Constant<'ctx, B, C>],
    result_ty: Type<'ctx, B, C>,
    tli: &TargetLibraryInfo,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Boundary: the caller's operands, admitted against `result_ty`'s module —
    // the one handle a call with no operands still carries.
    let owner = result_ty.module.id();
    for operand in operands {
        operand.slot_in(owner)?;
    }
    constant_fold_call_trusting_same_module(
        lib_func,
        operands,
        result_ty,
        tli,
        allow_non_deterministic,
    )
}

/// [`constant_fold_call`] for operands and a result type of one module.
pub(crate) fn constant_fold_call_trusting_same_module<
    'ctx,
    B: ModuleBrand + 'ctx,
    C: Capability,
>(
    lib_func: LibFunc,
    operands: &[Constant<'ctx, B, C>],
    result_ty: Type<'ctx, B, C>,
    tli: &TargetLibraryInfo,
    allow_non_deterministic: FoldNonDeterminism,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    if !can_constant_fold_call_to(lib_func, tli) {
        return Ok(None);
    }
    // Keep APFloat-native folds such as sqrt available when deterministic mode
    // denies host-libm-dependent folds; unported libm arms decline below.
    match lib_func {
        LibFunc::Sqrt | LibFunc::Sqrtf => fold_sqrt_call(operands, result_ty),
        _ if matches!(allow_non_deterministic, FoldNonDeterminism::Deny) => Ok(None),
        _ => Ok(None),
    }
}

fn fold_phi<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: Type<'ctx, B, C>,
    module: ModuleRef<'ctx, B, C>,
    phi: &PhiData,
) -> Option<Constant<'ctx, B, C>> {
    let mut common = None;
    for (incoming, _) in phi.incoming.borrow().iter() {
        let constant = constant_from_id(module, incoming.get())?;
        // Upstream skips `isa<UndefValue>`, and `PoisonValue` is-a
        // `UndefValue` there, so poison incomings are skipped too; treating
        // poison as a candidate would fold `phi [poison], [undef]` to poison
        // — an illegal undef-to-poison weakening.
        if is_undef(constant) || is_poison(constant) {
            continue;
        }
        if let Some(previous) = common {
            if previous != constant {
                return None;
            }
        } else {
            common = Some(constant);
        }
    }
    Some(match common {
        Some(value) => value,
        None => ty.undef().as_constant(),
    })
}

fn fold_sqrt_call<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    operands: &[Constant<'ctx, B, C>],
    result_ty: Type<'ctx, B, C>,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    let [operand] = operands else {
        return Ok(None);
    };
    let Ok(src) = ConstantFloatValue::<FloatDyn, B, C>::try_from(*operand) else {
        return Ok(None);
    };
    if src.ty().as_type() != result_ty {
        return Ok(None);
    }

    let ap = src.ap_float();
    if ap.is_negative() && ap.is_finite() {
        return Ok(None);
    }

    match src.ty().semantics() {
        ApFloatSemantics::IeeeSingle => {
            let Some(bits) = ap.to_bits().try_zext_u64() else {
                return Ok(None);
            };
            let Ok(bits) = u32::try_from(bits) else {
                return Ok(None);
            };
            let result = f32::from_bits(bits).sqrt();
            let apf = ApFloat::from_bits(
                ApFloatSemantics::IeeeSingle,
                &ApInt::from_words(32, &[u64::from(result.to_bits())]),
            )?;
            Ok(Some(src.ty().const_ap_float(&apf)?.as_constant()))
        }
        ApFloatSemantics::IeeeDouble => {
            let Some(bits) = ap.to_bits().try_zext_u64() else {
                return Ok(None);
            };
            let result = f64::from_bits(bits).sqrt();
            let apf = ApFloat::from_bits(
                ApFloatSemantics::IeeeDouble,
                &ApInt::from_words(64, &[result.to_bits()]),
            )?;
            Ok(Some(src.ty().const_ap_float(&apf)?.as_constant()))
        }
        ApFloatSemantics::IeeeHalf
        | ApFloatSemantics::Bfloat
        | ApFloatSemantics::IeeeQuad
        | ApFloatSemantics::X87DoubleExtended
        | ApFloatSemantics::PpcDoubleDouble => Ok(None),
    }
}

fn flush_denormal_float<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: FloatType<'ctx, FloatDyn, B, C>,
    value: &ApFloat,
    mode: DenormalModeKind,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    let folded = match mode {
        DenormalModeKind::Dynamic => return Ok(None),
        DenormalModeKind::Ieee => value.clone(),
        DenormalModeKind::PreserveSign => {
            let sign = if value.is_negative() {
                ApFloatSign::Negative
            } else {
                ApFloatSign::Positive
            };
            ApFloat::zero(ty.semantics(), sign)
        }
        DenormalModeKind::PositiveZero => ApFloat::zero(ty.semantics(), ApFloatSign::Positive),
    };
    Ok(Some(ty.const_ap_float(&folded)?.as_constant()))
}

/// Mirrors `GEPOperator::accumulateConstantOffset` (`Operator.cpp`) for a
/// source type and its indices: the byte offset they add, or `None` where
/// upstream answers `false`. llvmkit has no `ExternalAnalysis` to pass, so any
/// index that is no `ConstantInt` is that `false`, and the fast path for an `i8`
/// source answers what the walk answers, so it is not spelled.
pub(crate) fn accumulate_constant_offset<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    source_ty: Type<'ctx, B, C>,
    index_ids: &[ValueSlot],
    index_bits: u32,
    dl: &DataLayout,
) -> Option<ApInt> {
    let mut offset = ApInt::zero(index_bits);
    let mut cursor = Some(GepTypeCursor::begin(source_ty));
    for index_id in index_ids.iter().copied() {
        let step = cursor?;
        let indexed_ty = step.indexed_type(index_id)?;
        cursor = GepTypeCursor::after(indexed_ty);

        // Scalable vectors are multiplied by a runtime constant.
        let scalable_type = indexed_ty.is_scalable();

        let index = constant_gep_index(source_ty.module, index_id)?;
        if index.is_zero() {
            continue;
        }
        // If the type is scalable and the constant is not zero
        // (vscale * n * 0 = 0) bailout.
        if scalable_type {
            return None;
        }
        // Handle a struct index, which adds its field offset to the pointer.
        if let Some(struct_ty) = step.struct_type() {
            let field = step.struct_field(index_id)?;
            // A scalable offset here is upstream's fatal `TypeSize` conversion:
            // the field's own type is fixed, so the bailout above let it by.
            let field_offset = dl
                .struct_layout(erase_type(struct_ty))
                .element_offset(field)
                .fixed_value()?;
            offset = offset.wrapping_add(&ApInt::from_words(index_bits, &[field_offset]));
            continue;
        }
        let stride = step.sequential_element_stride(dl).fixed_value()?;
        offset = offset.wrapping_add(&scaled_offset(
            &index.sext_or_trunc(index_bits),
            stride,
            index_bits,
        ));
    }
    Some(offset)
}

/// The index `id` names, at its own width, when it is a `ConstantInt`.
fn constant_gep_index<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    id: ValueSlot,
) -> Option<ApInt> {
    let constant = constant_from_id(module, id)?;
    Some(
        ConstantIntValue::<IntDyn, B, C>::try_from(constant)
            .ok()?
            .ap_int(),
    )
}

fn scaled_offset(index: &ApInt, scale: u64, index_bits: u32) -> ApInt {
    index.wrapping_mul(&ApInt::from_words(index_bits, &[scale]))
}

/// Mirrors `SymbolicallyEvaluateGEP`
/// (`llvm/lib/Analysis/ConstantFolding.cpp`): canonicalise an
/// all-constant-integer-index scalar `getelementptr` to a single
/// `getelementptr i8, ptr <base>, i64 <total offset>`, merging through nested
/// constant-GEP layers (both the general `ConstantExprOpcode::GetElementPtr`
/// form and llvmkit's compact `ConstantData::GepOffset` form) and propagating
/// no-wrap flags the same way upstream's merge loop does. Tried by both GEP
/// dispatch sites *before* falling back to the existing
/// [`constant_fold_get_element_ptr_trusting_same_module`] empty/poison/undef/noop/in-range folds,
/// matching upstream's `ConstantFoldInstOperandsImpl` dispatch order
/// (`SymbolicallyEvaluateGEP` first, `ConstantExpr::getGetElementPtr` — whose
/// own constructor re-runs the target-independent fold — second).
///
/// Narrower than upstream in four precise, safe-by-construction ways (each
/// only forgoes a fold upstream would make; none mis-fold):
///
///  - **No `CastGEPIndices`** (the index-width normalisation for vector-shaped
///    indices/pointers that `SymbolicallyEvaluateGEP` calls first). This port
///    only handles a scalar pointer base (`index_bits_for_pointer` returns
///    `None` for a vector-of-pointers type, matching upstream's own
///    `!Ptr->getType()->isPointerTy()` bail) with scalar `ConstantInt`
///    indices. Upstream itself never symbolically evaluates an offset for a
///    genuinely vector-shaped index either — its `isa<ConstantInt>` operand
///    loop always fails for a vector constant — so the only thing skipped is
///    upstream's index-width-only recursive re-fold for that shape, never an
///    offset canonicalisation this port would otherwise produce.
///  - **No `in_range` support**: llvmkit's [`ConstantExprInRange`] has no
///    `sextOrTrunc`/`subtract`/`intersectWith`, which upstream needs both when
///    it first reads `GEP->getInRange()` and again inside the merge loop. Any
///    GEP — outer or merged-through — carrying `in_range` stops the fold
///    immediately rather than dropping or mis-adjusting the range.
///  - **No null/`inttoptr`-base fold**: this port stops short of upstream's
///    `inttoptr` result, which [`DataLayout::must_not_introduce_int_to_ptr`]
///    guards. Rather than mis-canonicalising a from-null (or from-`inttoptr`-of-a-
///    nonzero-constant) GEP into a `getelementptr i8, ptr null, ...` form
///    upstream would instead fold to `inttoptr`, this bails to `None`
///    whenever the fully-merged base is null or `inttoptr` of a nonzero
///    `ConstantInt`.
///  - **No "infer inbounds for GEPs of globals"**
///    (`Value::getPointerDereferenceableBytes`): llvmkit has no
///    dereferenceable-bytes/pointer-capacity query. Simply omitted — the
///    produced no-wrap flags can be weaker (missing an inferable `inbounds`)
///    than upstream's, never wrong.
fn symbolically_evaluate_gep<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    source_ty: Type<'ctx, B, C>,
    pointer: Constant<'ctx, B, C>,
    indices: &[Constant<'ctx, B, C>],
    nw: GepNoWrapFlags,
    in_range: Option<&ConstantExprInRange>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // `SrcElemTy->isSized() && !isa<ScalableVectorType>(SrcElemTy)`.
    if !source_ty.is_sized() || matches!(source_ty.data(), TypeData::ScalableVector { .. }) {
        return Ok(None);
    }
    // Deferred: in_range preservation (see doc comment above).
    if in_range.is_some() {
        return Ok(None);
    }
    // `!Ptr->getType()->isPointerTy()` — also excludes vector-of-pointers,
    // deferred alongside `CastGEPIndices` (see doc comment above).
    let TypeData::Pointer { .. } = pointer.ty().data() else {
        return Ok(None);
    };
    let Some(index_bits) = index_bits_for_pointer(pointer.ty(), dl) else {
        return Ok(None);
    };

    // Internal: the indices are operands of one fold, already of `pointer`'s
    // module.
    let index_ids: Vec<ValueSlot> = indices
        .iter()
        .map(|index| index.slot_trusting_same_module())
        .collect();
    // `Offset = APInt(BitWidth, DL.getIndexedOffsetInType(SrcElemTy, Ops[1..]), ...)`.
    // Bails (matching `for i in 1..: if (!isa<ConstantInt>(Ops[i])) return
    // nullptr;`) whenever an index isn't a plain scalar `ConstantInt`.
    let Some(mut offset) = accumulate_constant_offset(source_ty, &index_ids, index_bits, dl) else {
        return Ok(None);
    };

    // "If this is a GEP of a GEP, fold it all into a single GEP."
    let mut nw = nw;
    let mut overflow = false;
    let mut ptr = pointer;
    while let Some(level_nw) = gep_level_no_wrap_flags(ptr) {
        // Upstream intersects a level's no-wrap flags unconditionally
        // (`NW &= GEP->getNoWrapFlags();`, `ConstantFolding.cpp`) —
        // *before* it even checks whether that level's indices are all
        // `ConstantInt` (its `AllConstantInt` guard). So this must run even when the
        // peel below fails and the loop is about to stop.
        nw &= level_nw;
        let Some((base, level_offset)) = peel_one_gep_level(ptr, index_bits, dl) else {
            break;
        };
        let (combined, this_overflowed) = offset.sadd_ov(&level_offset);
        offset = combined;
        overflow |= this_overflowed;
        ptr = base;
    }

    // "Preserving nusw (without inbounds) also requires that the offset
    // additions did not overflow."
    if nw.contains(GepNoWrapFlags::NUSW) && !nw.contains(GepNoWrapFlags::IN_BOUNDS) && overflow {
        nw = nw.difference(GepNoWrapFlags::IN_BOUNDS | GepNoWrapFlags::NUSW);
    }

    // Deferred: null/`inttoptr`-base -> `inttoptr` fold (see doc comment).
    if is_null_or_inttoptr_nonzero_base(ptr) {
        return Ok(None);
    }

    // Deferred: "infer inbounds for GEPs of globals" (see doc comment) — `nw`
    // is used exactly as merged above, never upgraded to `inbounds` here.

    // "nusw + nneg -> nuw"
    if nw.contains(GepNoWrapFlags::NUSW) && offset.is_non_negative() {
        nw |= GepNoWrapFlags::NUW;
    }

    build_canonical_i8_gep(ptr, &offset, nw)
}

/// Return the no-wrap flags of `ptr` if it is GEP-shaped — either llvmkit's
/// compact [`ConstantData::GepOffset`] (always `inbounds`) or a general
/// [`ConstantExprOpcode::GetElementPtr`] constant expression (its stored
/// flags) — or `None` if `ptr` is not a GEP at all (upstream's merge-loop
/// condition, `while (auto *GEP = dyn_cast<GEPOperator>(Ptr))`,
/// `ConstantFolding.cpp`).
///
/// Deliberately split out of [`peel_one_gep_level`]: upstream intersects a
/// level's no-wrap flags into the running state *unconditionally* (`NW &=
/// GEP->getNoWrapFlags();`) — before it even checks whether that level's
/// indices are all `ConstantInt` (its `AllConstantInt` guard).
/// Callers must consult this function first and intersect its result before
/// attempting [`peel_one_gep_level`], so the intersection still happens on
/// the level where the peel is about to fail and the merge loop is about to
/// stop.
fn gep_level_no_wrap_flags<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ptr: Constant<'ctx, B, C>,
) -> Option<GepNoWrapFlags> {
    match &ptr.as_erased().data().kind {
        ValueKindData::Constant(ConstantData::GepOffset { .. }) => Some(GepNoWrapFlags::inbounds()),
        ValueKindData::Constant(ConstantData::Expr(expr))
            if expr.opcode == ConstantExprOpcode::GetElementPtr =>
        {
            Some(match &expr.flags {
                ConstantExprFlags::Gep(flags) => flags.no_wrap(),
                _ => GepNoWrapFlags::empty(),
            })
        }
        _ => None,
    }
}

/// Attempt to peel one level of a constant `getelementptr` — either
/// llvmkit's compact [`ConstantData::GepOffset`] or the general
/// [`ConstantExprOpcode::GetElementPtr`] constant expression — returning its
/// base pointer and its own contribution to the running byte offset. Callers
/// must already know `ptr` is GEP-shaped (via [`gep_level_no_wrap_flags`])
/// before calling this; a non-GEP `ptr` still falls through to `None` here
/// too, but no longer needs to for the caller's own no-wrap intersection.
///
/// `None` stops the caller's merge loop exactly where upstream's does past
/// the unconditional `NW &=` (which [`gep_level_no_wrap_flags`] now covers):
/// a nested GEP with a non-`ConstantInt` index (`AllConstantInt` in
/// `ConstantFolding.cpp`), or (this port's addition) a nested
/// GEP carrying `in_range` — see [`symbolically_evaluate_gep`]'s doc comment
/// for why that last one isn't ported.
fn peel_one_gep_level<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ptr: Constant<'ctx, B, C>,
    index_bits: u32,
    dl: &DataLayout,
) -> Option<(Constant<'ctx, B, C>, ApInt)> {
    let module = ptr.module;
    match &ptr.as_erased().data().kind {
        ValueKindData::Constant(ConstantData::GepOffset { base_id, off }) => {
            // `base_id` names the host global/function directly — its own
            // `ValueKindData` is `GlobalVariable`/`Function`, never
            // `Constant` (see `ConstantData::GepOffset`'s doc comment) — so
            // it cannot be resolved through `constant_from_id`. Re-wrap it
            // as a `GlobalValueRef` of the same pointer type as `ptr` itself
            // (a GEP never changes address space), the same wrapping
            // `GlobalVariable`/`FunctionValue::as_global_constant_ptr` build
            // for the same kind of id elsewhere.
            let ptr_ty = ptr.ty();
            let wrapped = module
                .context()
                .intern_constant_global_value_ref(ptr_ty.slot_trusting_same_module(), *base_id);
            let base = Constant::from_parts(Value::from_parts(
                wrapped,
                module,
                ptr_ty.slot_trusting_same_module(),
            ));
            Some((base, gep_offset_magnitude(*off, index_bits)))
        }
        ValueKindData::Constant(ConstantData::Expr(expr))
            if expr.opcode == ConstantExprOpcode::GetElementPtr =>
        {
            let in_range = match &expr.flags {
                ConstantExprFlags::Gep(flags) => flags.in_range(),
                _ => None,
            };
            if in_range.is_some() {
                return None;
            }
            let source_ty = Type::new(expr.source_ty?, module);
            let (base_id, index_ids) = expr.operands.split_first()?;
            let base = constant_from_id(module, *base_id)?;
            let level_offset = accumulate_constant_offset(source_ty, index_ids, index_bits, dl)?;
            Some((base, level_offset))
        }
        _ => None,
    }
}

/// Reinterpret a [`ConstantData::GepOffset`] byte offset as an
/// `index_bits`-wide signed [`ApInt`], for [`peel_one_gep_level`].
fn gep_offset_magnitude(off: i64, index_bits: u32) -> ApInt {
    let magnitude = ApInt::from_words(index_bits, &[off.unsigned_abs()]);
    if off < 0 {
        magnitude.negate()
    } else {
        magnitude
    }
}

/// `Ptr->isNullValue()`, or `Ptr` is `inttoptr` of a literal nonzero
/// `ConstantInt` — the precondition upstream uses, its
/// `(Ptr->isNullValue() || BaseIntVal != 0) && !DL.mustNotIntroduceIntToPtr(..)`
/// guard, to fold to `inttoptr` instead of a GEP. This port declines (see
/// [`symbolically_evaluate_gep`]'s doc comment); `inttoptr` of anything else
/// (a non-`ConstantInt` operand) does *not* match, mirroring upstream's
/// `BaseIntVal` staying zero — and thus this condition staying false — for
/// that shape, because its `BaseIntVal` is only written under
/// `dyn_cast<ConstantInt>(CE->getOperand(0))`.
fn is_null_or_inttoptr_nonzero_base<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ptr: Constant<'ctx, B, C>,
) -> bool {
    if Constant::is_null_value(ptr) {
        return true;
    }
    let ValueKindData::Constant(ConstantData::Expr(expr)) = &ptr.as_erased().data().kind else {
        return false;
    };
    if expr.opcode != ConstantExprOpcode::IntToPtr {
        return false;
    }
    let [operand_id] = expr.operands.as_ref() else {
        return false;
    };
    let module = ptr.module;
    let Some(operand) = constant_from_id(module, *operand_id) else {
        return false;
    };
    let Ok(operand_int) = ConstantIntValue::<IntDyn, B, C>::try_from(operand) else {
        return false;
    };
    !operand_int.ap_int().is_zero()
}

/// Build `getelementptr i8, ptr <ptr>, i64 <offset>` with no-wrap flags `nw`,
/// mirroring the tail of `SymbolicallyEvaluateGEP` — its "canonicalize this
/// to a single ptradd" `ConstantExpr::getGetElementPtr(Type::getInt8Ty(..), ..)`
/// — plus the
/// `IsNoOp`/poison/undef auto-collapse every `ConstantExpr::getGetElementPtr`
/// construction gets for free upstream (`ConstantFold.cpp`'s
/// `ConstantFoldGetElementPtr`, reached here through
/// [`super::constants::ModuleCore::constant_expr_with_options`]'s own
/// internal fold attempt).
///
/// Prefers llvmkit's compact [`ConstantData::GepOffset`] encoding — which can
/// only represent a byte offset from a global/function reference, always
/// under `inbounds` — falling back to the general constant-expression form
/// (verbatim `nw`, any base) whenever `nw` isn't `inbounds` at all. A bare
/// `nuw` alongside `inbounds` is silently dropped rather than forcing the
/// general form: `inbounds` alone already implies everything `nuw` would add
/// (no wrapping pointer arithmetic), so this only ever *under*-annotates the
/// result — safe by the same "dropping a no-wrap flag is always
/// conservative" principle no-wrap flags rely on everywhere else — never
/// mis-annotates it.
fn build_canonical_i8_gep<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ptr: Constant<'ctx, B, C>,
    offset: &ApInt,
    nw: GepNoWrapFlags,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // Matches the single-zero-index case of `ConstantFoldGetElementPtr`'s
    // `IsNoOp` (`in_range` is always `None` here — see the caller's doc
    // comment): the canonical form we're about to build would immediately
    // collapse back to `ptr` unchanged, so just return that directly.
    if offset.is_zero() {
        return Ok(Some(ptr));
    }

    let module = ptr.module;

    if nw.contains(GepNoWrapFlags::IN_BOUNDS)
        && let ValueKindData::Constant(ConstantData::GlobalValueRef { value }) =
            &ptr.as_erased().data().kind
        && let Some(off) = offset.try_sext_i64()
    {
        let ptr_ty = ptr.ty();
        let id = module.context().intern_constant_gep_offset(
            ptr_ty.slot_trusting_same_module(),
            *value,
            off,
        );
        return Ok(Some(Constant::from_parts(Value::from_parts(
            id,
            module,
            ptr_ty.slot_trusting_same_module(),
        ))));
    }

    let idx_ty = int_type(module, offset.bit_width())?;
    let i8_ty = int_type(module, 8)?;
    let offset_const = idx_ty.const_ap_int(offset)?.as_constant();
    module
        .constant_expr_with_options(
            ptr.ty(),
            ConstantExprOpcode::GetElementPtr,
            [ptr.as_erased(), offset_const.as_erased()],
            [],
            [],
            ConstantExprOptions::new()
                .source_ty(i8_ty.as_type())
                .flags(ConstantExprFlags::gep(nw)),
        )
        .map(Some)
}

/// Mirrors `getConstantAtOffset` (`ConstantFolding.cpp`): the element of `base`
/// that starts exactly `offset` bytes in.
fn constant_at_offset<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    base: Constant<'ctx, B, C>,
    offset: &ApInt,
    dl: &DataLayout,
) -> Option<Constant<'ctx, B, C>> {
    if offset.is_zero() {
        return Some(base);
    }
    if !matches!(
        base.kind(),
        ConstantKind::Aggregate(_) | ConstantKind::Data(ConstantDataKind::Sequential(_))
    ) {
        return None;
    }
    let mut remaining = offset.clone();
    let indices = gep_indices_for_offset(base.ty(), &mut remaining, dl);
    let (first, walk) = indices.split_first()?;
    if !remaining.is_zero() || !first.is_zero() {
        return None;
    }
    let mut constant = base;
    for index in walk {
        if index.is_negative() || index.active_bits() >= 32 {
            return None;
        }
        constant = constant.aggregate_element(u32::try_from(index.try_zext_u64()?).ok()?)?;
    }
    Some(constant)
}

/// Mirrors `DataLayout::getGEPIndicesForOffset`.
fn gep_indices_for_offset<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: Type<'ctx, B, C>,
    offset: &mut ApInt,
    dl: &DataLayout,
) -> Vec<ApInt> {
    let mut element_ty = ty;
    let mut indices = vec![element_index(
        dl.type_alloc_size(erase_type(element_ty)),
        offset,
    )];
    while !offset.is_zero() {
        let Some(index) = gep_index_for_offset(&mut element_ty, offset, dl) else {
            break;
        };
        indices.push(index);
    }
    indices
}

/// Mirrors `getElementIndex` (`DataLayout.cpp`).
fn element_index(element_size: TypeSize, offset: &mut ApInt) -> ApInt {
    let bit_width = offset.bit_width();
    // Skip over scalable or zero size elements. Also skip element sizes larger
    // than the positive index space, because the arithmetic below may not be
    // correct in that case.
    let division = element_size
        .fixed_value()
        .filter(|size| {
            *size != 0 && u64::BITS - size.leading_zeros() <= bit_width.saturating_sub(1)
        })
        .map(|size| ApInt::from_words(bit_width, &[size]))
        .and_then(|size| Some((offset.checked_sdiv(&size)?, size)));
    let Some((mut index, size)) = division else {
        return ApInt::zero(bit_width);
    };
    *offset = offset.wrapping_sub(&index.wrapping_mul(&size));
    if offset.is_negative() {
        // Prefer a positive remaining offset to allow struct indexing.
        index = index.wrapping_sub(&ApInt::from_words(bit_width, &[1]));
        *offset = offset.wrapping_add(&size);
    }
    index
}

/// Mirrors `DataLayout::getGEPIndexForOffset`. A struct's size and field
/// offset are the implicit `TypeSize` conversion upstream, which is fatal on a
/// scalable struct; here that is one more type no index can step into.
fn gep_index_for_offset<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    element_ty: &mut Type<'ctx, B, C>,
    offset: &mut ApInt,
    dl: &DataLayout,
) -> Option<ApInt> {
    let module = element_ty.module;
    match element_ty.data() {
        TypeData::Array { elem, .. } => {
            *element_ty = Type::new(*elem, module);
            Some(element_index(
                dl.type_alloc_size(erase_type(*element_ty)),
                offset,
            ))
        }
        TypeData::Struct(data) => {
            let layout = dl.struct_layout(erase_type(*element_ty));
            let int_offset = offset.try_zext_u64()?;
            if int_offset >= layout.size_in_bytes().fixed_value()? {
                return None;
            }
            let field = layout.element_containing_offset(int_offset)?;
            let field_offset = layout.element_offset(field).fixed_value()?;
            *offset = offset.wrapping_sub(&ApInt::from_words(offset.bit_width(), &[field_offset]));
            let element = *data.body.borrow().as_ref()?.elements.get(field)?;
            *element_ty = Type::new(element, module);
            Some(ApInt::from_words(32, &[u64::try_from(field).ok()?]))
        }
        // Vector GEPs are partially supported. We do not allow indexing into
        // them in this API. Anything else is not an aggregate.
        _ => None,
    }
}

fn folded_constants_from_ids<'ctx, B, C: Capability, I>(
    module: ModuleRef<'ctx, B, C>,
    ids: I,
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Option<Vec<Constant<'ctx, B, C>>>>
where
    B: ModuleBrand + 'ctx,
    I: IntoIterator<Item = ValueSlot>,
{
    let mut folded = Vec::new();
    for id in ids {
        let Some(constant) = constant_from_id(module, id) else {
            return Ok(None);
        };
        folded.push(constant_fold_constant(constant, dl, tli)?);
    }
    Ok(Some(folded))
}

fn constant_fold_constant_expr_operands<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    original: Constant<'ctx, B, C>,
    expr: &ConstantExprData,
    operands: &[Constant<'ctx, B, C>],
    dl: &DataLayout,
    tli: Option<&TargetLibraryInfo>,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    let _ = tli;
    match expr.opcode {
        ConstantExprOpcode::Add | ConstantExprOpcode::Sub | ConstantExprOpcode::Xor => {
            let [lhs, rhs] = operands else {
                return Ok(None);
            };
            let Some(opcode) = binary_opcode_from_constant_expr(expr.opcode) else {
                return Ok(None);
            };
            constant_fold_binary_op_operands_trusting_same_module(opcode, *lhs, *rhs, dl)
        }
        ConstantExprOpcode::GetElementPtr => {
            let Some(source_ty) = expr.source_ty else {
                return Ok(None);
            };
            let Some((pointer, indices)) = operands.split_first() else {
                return Ok(None);
            };
            let source_ty = Type::new(source_ty, original.module);
            let (nw, in_range) = match &expr.flags {
                ConstantExprFlags::Gep(flags) => (flags.no_wrap(), flags.in_range()),
                _ => (GepNoWrapFlags::empty(), None),
            };
            if let Some(folded) =
                symbolically_evaluate_gep(source_ty, *pointer, indices, nw, in_range, dl)?
            {
                return Ok(Some(folded));
            }
            constant_fold_get_element_ptr_trusting_same_module(
                source_ty, *pointer, indices, in_range,
            )
        }
        ConstantExprOpcode::ShuffleVector => {
            let [lhs, rhs, mask] = operands else {
                return Ok(None);
            };
            let Some(mask) = shufflevector_mask_from_constant(*mask) else {
                return Ok(None);
            };
            constant_fold_shuffle_vector_instruction_trusting_same_module(*lhs, *rhs, &mask)
        }
        ConstantExprOpcode::InsertElement => {
            let [vector, value, index] = operands else {
                return Ok(None);
            };
            constant_fold_insert_element_instruction_trusting_same_module(*vector, *value, *index)
        }
        ConstantExprOpcode::ExtractElement => {
            let [vector, index] = operands else {
                return Ok(None);
            };
            constant_fold_extract_element_instruction_trusting_same_module(*vector, *index)
        }
        ConstantExprOpcode::Trunc
        | ConstantExprOpcode::PtrToAddr
        | ConstantExprOpcode::PtrToInt
        | ConstantExprOpcode::IntToPtr
        | ConstantExprOpcode::BitCast
        | ConstantExprOpcode::AddrSpaceCast => {
            let [operand] = operands else {
                return Ok(None);
            };
            let Some(opcode) = cast_opcode_from_constant_expr(expr.opcode) else {
                return Ok(None);
            };
            constant_fold_cast_operand_trusting_same_module(
                opcode,
                *operand,
                Type::new(expr.result_ty, original.module),
                dl,
            )
        }
    }
}

fn binary_opcode(kind: &InstructionKindData) -> Option<BinaryOpcode> {
    let opcode = match kind {
        InstructionKindData::Add(_) => BinaryOpcode::Add,
        InstructionKindData::Sub(_) => BinaryOpcode::Sub,
        InstructionKindData::Mul(_) => BinaryOpcode::Mul,
        InstructionKindData::Udiv(_) => BinaryOpcode::Udiv,
        InstructionKindData::Sdiv(_) => BinaryOpcode::Sdiv,
        InstructionKindData::Urem(_) => BinaryOpcode::Urem,
        InstructionKindData::Srem(_) => BinaryOpcode::Srem,
        InstructionKindData::Shl(_) => BinaryOpcode::Shl,
        InstructionKindData::Lshr(_) => BinaryOpcode::Lshr,
        InstructionKindData::Ashr(_) => BinaryOpcode::Ashr,
        InstructionKindData::And(_) => BinaryOpcode::And,
        InstructionKindData::Or(_) => BinaryOpcode::Or,
        InstructionKindData::Xor(_) => BinaryOpcode::Xor,
        InstructionKindData::Fadd(_) => BinaryOpcode::Fadd,
        InstructionKindData::Fsub(_) => BinaryOpcode::Fsub,
        InstructionKindData::Fmul(_) => BinaryOpcode::Fmul,
        InstructionKindData::Fdiv(_) => BinaryOpcode::Fdiv,
        InstructionKindData::Frem(_) => BinaryOpcode::Frem,
        _ => return None,
    };
    Some(opcode)
}

/// Fast-math flags carried by an FP binary instruction. Mirrors
/// `FPMathOperator::getFastMathFlags`, reached in upstream
/// `ConstantFoldFPInstOperands` via `dyn_cast_or_null<FPMathOperator>(I)`.
/// Empty for every other opcode, matching `BinaryOpData::fmf` being unset for
/// integer opcodes.
fn binary_op_fmf(kind: &InstructionKindData) -> FastMathFlags {
    match kind {
        InstructionKindData::Fadd(b)
        | InstructionKindData::Fsub(b)
        | InstructionKindData::Fmul(b)
        | InstructionKindData::Fdiv(b)
        | InstructionKindData::Frem(b) => b.fmf,
        _ => FastMathFlags::empty(),
    }
}

fn binary_opcode_from_constant_expr(opcode: ConstantExprOpcode) -> Option<BinaryOpcode> {
    match opcode {
        ConstantExprOpcode::Add => Some(BinaryOpcode::Add),
        ConstantExprOpcode::Sub => Some(BinaryOpcode::Sub),
        ConstantExprOpcode::Xor => Some(BinaryOpcode::Xor),
        _ => None,
    }
}

fn binary_constant_expr_opcode(opcode: BinaryOpcode) -> Option<ConstantExprOpcode> {
    match opcode {
        BinaryOpcode::Add => Some(ConstantExprOpcode::Add),
        BinaryOpcode::Sub => Some(ConstantExprOpcode::Sub),
        BinaryOpcode::Xor => Some(ConstantExprOpcode::Xor),
        BinaryOpcode::Mul
        | BinaryOpcode::Udiv
        | BinaryOpcode::Sdiv
        | BinaryOpcode::Urem
        | BinaryOpcode::Srem
        | BinaryOpcode::Shl
        | BinaryOpcode::Lshr
        | BinaryOpcode::Ashr
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::Fadd
        | BinaryOpcode::Fsub
        | BinaryOpcode::Fmul
        | BinaryOpcode::Fdiv
        | BinaryOpcode::Frem => None,
    }
}

fn cast_opcode_from_constant_expr(opcode: ConstantExprOpcode) -> Option<CastOpcode> {
    match opcode {
        ConstantExprOpcode::Trunc => Some(CastOpcode::Trunc),
        ConstantExprOpcode::PtrToAddr => Some(CastOpcode::PtrToAddr),
        ConstantExprOpcode::PtrToInt => Some(CastOpcode::PtrToInt),
        ConstantExprOpcode::IntToPtr => Some(CastOpcode::IntToPtr),
        ConstantExprOpcode::BitCast => Some(CastOpcode::BitCast),
        ConstantExprOpcode::AddrSpaceCast => Some(CastOpcode::AddrSpaceCast),
        _ => None,
    }
}

fn load_through_bitcast_opcode<B: ModuleBrand, C: Capability>(
    src_ty: Type<'_, B, C>,
    dest_ty: Type<'_, B, C>,
) -> CastOpcode {
    if matches!(src_ty.data(), TypeData::Integer { .. }) && pointer_address_space(dest_ty).is_some()
    {
        CastOpcode::IntToPtr
    } else if pointer_address_space(src_ty).is_some()
        && matches!(dest_ty.data(), TypeData::Integer { .. })
    {
        CastOpcode::PtrToInt
    } else {
        CastOpcode::BitCast
    }
}

fn aggregate_first_element<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dl: &DataLayout,
) -> Option<Constant<'ctx, B, C>> {
    let ValueKindData::Constant(ConstantData::Aggregate(elements)) =
        &constant.as_erased().data().kind
    else {
        return None;
    };
    let module = constant.module;
    match constant.ty().data() {
        TypeData::Struct(_) => {
            for id in elements.iter().copied() {
                let element = constant_from_id(module, id)?;
                if dl
                    .type_size_in_bits(erase_type(element.ty()))
                    .known_min_value()
                    != 0
                {
                    return Some(element);
                }
            }
            None
        }
        TypeData::Array { .. } => constant_from_id(module, elements.first().copied()?),
        TypeData::FixedVector { elem, .. } => {
            let elem_ty = Type::new(*elem, module);
            if !dl.type_size_equals_store_size(erase_type(elem_ty)) {
                return None;
            }
            constant_from_id(module, elements.first().copied()?)
        }
        _ => None,
    }
}

fn constant_is_nan<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    ConstantFloatValue::<FloatDyn, B, C>::try_from(constant)
        .is_ok_and(|value| value.ap_float().is_nan())
}

/// Ports `ConstantFoldIntegerCast`. Upstream asserts on a pair `castIsValid`
/// refuses, and a public caller can hand one over, so that declines.
fn fold_integer_cast_constant<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    signedness: Signedness,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    let src_ty = constant.ty();
    if src_ty == dest_ty {
        return Ok(Some(constant));
    }
    let opcode = if src_ty.scalar_size_in_bits() > dest_ty.scalar_size_in_bits() {
        CastOpcode::Trunc
    } else {
        match signedness {
            Signedness::Signed => CastOpcode::Sext,
            Signedness::Unsigned => CastOpcode::Zext,
        }
    };
    if !cast_is_valid(opcode, src_ty, dest_ty) {
        return Ok(None);
    }
    constant_fold_cast_operand_trusting_same_module(opcode, constant, dest_ty, dl)
}

/// `ConstantExpr::getBitCast`: `constant` itself at its own type, otherwise
/// whatever `getFoldedCast` makes of it, which is a fold or a `bitcast`
/// constant expression.
fn constant_expr_bit_cast<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
) -> IrResult<Constant<'ctx, B, C>> {
    if constant.ty() == dest_ty {
        return Ok(constant);
    }
    constant.module.constant_expr(
        dest_ty,
        ConstantExprOpcode::BitCast,
        [constant.as_erased()],
        [],
        [],
        ConstantExprFlags::none(),
    )
}

/// `FixedVectorType::get(IntegerType::get(Context, bits), lanes)`.
fn integer_vector_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    bits: u32,
    lanes: NonZeroU32,
) -> IrResult<Type<'ctx, B, C>> {
    let int_ty = int_type(module, bits)?;
    Ok(vector_type_with_scalability(module, int_ty, lanes, false).as_type())
}

/// Mirrors `foldConstVectorToAPInt`: the integer `constant`'s lanes spell.
/// `None` is upstream's returning the `bitcast` expression, which the caller
/// builds, for a lane that is no `ConstantInt`.
fn fold_const_vector_to_ap_int<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    result_bits: u32,
    constant: Constant<'ctx, B, C>,
    src_elt_ty: Type<'ctx, B, C>,
    num_src_elts: u32,
    dl: &DataLayout,
) -> Option<ApInt> {
    // Now that we know that the input value is a vector of integers, just shift
    // and insert them into our result.
    let bit_shift =
        u32::try_from(dl.type_size_in_bits(erase_type(src_elt_ty)).fixed_value()?).ok()?;
    let mut result = ApInt::zero(result_bits);
    for i in 0..num_src_elts {
        let element = if dl.is_little_endian() {
            constant.aggregate_element(num_src_elts - i - 1)
        } else {
            constant.aggregate_element(i)
        };
        if element.is_some_and(is_undef_or_poison) {
            result = result.shl(bit_shift);
            continue;
        }
        let element = ConstantIntValue::<IntDyn, B, C>::try_from(element?).ok()?;
        result = result.shl(bit_shift);
        result = result.bitor(&element.ap_int().zext_or_trunc(result_bits));
    }
    Some(result)
}

/// The vector-to-scalar arm of [`fold_bitcast_with_layout`]: `dest_ty` is an
/// integer or a float.
fn fold_vector_to_scalar_bitcast<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    src_elt_ty: Type<'ctx, B, C>,
    num_src_elts: NonZeroU32,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    // If the vector is a vector of floating point, convert it to vector of int
    // to simplify things.
    let constant = if src_elt_ty.is_floating_point() {
        let src_iv_ty = integer_vector_type(
            constant.module,
            src_elt_ty.scalar_size_in_bits(),
            num_src_elts,
        )?;
        // Ask IR to do the conversion now that #elts line up.
        constant_expr_bit_cast(constant, src_iv_ty)?
    } else {
        constant
    };

    // A cast of equal sizes has a fixed one, because the scalar's is.
    let Some(result_bits) = dl
        .type_size_in_bits(erase_type(dest_ty))
        .fixed_value()
        .and_then(|bits| u32::try_from(bits).ok())
    else {
        return Ok(None);
    };
    let Some(result) =
        fold_const_vector_to_ap_int(result_bits, constant, src_elt_ty, num_src_elts.get(), dl)
    else {
        return constant_expr_bit_cast(constant, dest_ty).map(Some);
    };

    if let Ok(int_ty) = IntType::<IntDyn, B, C>::try_from(dest_ty) {
        return Ok(Some(int_ty.const_ap_int(&result)?.as_constant()));
    }
    let Ok(float_ty) = FloatType::<FloatDyn, B, C>::try_from(dest_ty) else {
        return Ok(None);
    };
    let value = ApFloat::from_bits(float_ty.semantics(), &result)?;
    Ok(Some(float_ty.const_ap_float(&value)?.as_constant()))
}

/// The `NumDstElt < NumSrcElt` arm of [`fold_bitcast_with_layout`]. `None` is
/// a lane that is no `ConstantInt`, which the caller answers with the `bitcast`
/// expression.
fn merge_vector_lanes<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dst_elt_ty: Type<'ctx, B, C>,
    src_bit_size: u32,
    num_dst_elt: u32,
    num_src_elt: u32,
    dl: &DataLayout,
) -> IrResult<Option<Vec<Constant<'ctx, B, C>>>> {
    // Handle: bitcast (<4 x i32> <i32 0, i32 1, i32 2, i32 3> to <2 x i64>)
    let is_little_endian = dl.is_little_endian();
    let (Some(zero), Ok(dst_int_ty)) = (
        null_constant_for_type(dst_elt_ty)?,
        IntType::<IntDyn, B, C>::try_from(dst_elt_ty),
    ) else {
        return Ok(None);
    };
    let Some((src_elt_ty, _, _)) = constant.ty().data().as_vector() else {
        return Ok(None);
    };
    let src_elt_ty = Type::new(src_elt_ty, constant.module);
    let ratio = num_src_elt / num_dst_elt;
    let mut src_elt = 0;
    let mut result = Vec::new();
    for _ in 0..num_dst_elt {
        // Build each element of the result.
        let mut elt = zero;
        let mut shift_amt = if is_little_endian {
            0
        } else {
            src_bit_size * (ratio - 1)
        };
        for _ in 0..ratio {
            let src = constant.aggregate_element(src_elt);
            src_elt += 1;
            let src = match src {
                Some(src) if is_undef_or_poison(src) => null_constant_for_type(src_elt_ty)?,
                Some(src) if matches!(src.kind(), ConstantKind::Data(ConstantDataKind::Int(_))) => {
                    Some(src)
                }
                _ => None,
            };
            // Reject constantexpr elements.
            let Some(src) = src else {
                return Ok(None);
            };

            // Zero extend the element to the right size.
            let Some(src) = constant_fold_cast_operand_trusting_same_module(
                CastOpcode::Zext,
                src,
                dst_elt_ty,
                dl,
            )?
            else {
                return Ok(None);
            };

            // Shift it to the right place, depending on endianness.
            let amount = ApInt::from_words(dst_int_ty.bit_width(), &[u64::from(shift_amt)]);
            let amount = dst_int_ty.const_ap_int(&amount)?.as_constant();
            let Some(src) = constant_fold_binary_op_operands_trusting_same_module(
                BinaryOpcode::Shl,
                src,
                amount,
                dl,
            )?
            else {
                return Ok(None);
            };

            shift_amt = if is_little_endian {
                shift_amt.wrapping_add(src_bit_size)
            } else {
                shift_amt.wrapping_sub(src_bit_size)
            };

            // Mix it in.
            let Some(mixed) = constant_fold_binary_op_operands_trusting_same_module(
                BinaryOpcode::Or,
                elt,
                src,
                dl,
            )?
            else {
                return Ok(None);
            };
            elt = mixed;
        }
        result.push(elt);
    }
    Ok(Some(result))
}

/// The `NumDstElt > NumSrcElt` arm of [`fold_bitcast_with_layout`]. `None` is
/// a lane that is no `ConstantInt`, which the caller answers with the `bitcast`
/// expression.
fn split_vector_lanes<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dst_elt_ty: Type<'ctx, B, C>,
    num_dst_elt: u32,
    num_src_elt: u32,
    dl: &DataLayout,
) -> IrResult<Option<Vec<Constant<'ctx, B, C>>>> {
    // Handle: bitcast (<2 x i64> <i64 0, i64 1> to <4 x i32>)
    let is_little_endian = dl.is_little_endian();
    let Ok(dst_int_ty) = IntType::<IntDyn, B, C>::try_from(dst_elt_ty) else {
        return Ok(None);
    };
    let ratio = num_dst_elt / num_src_elt;
    let dst_bit_size = dst_int_ty.bit_width();

    // Loop over each source value, expanding into multiple results.
    let mut result = Vec::new();
    for i in 0..num_src_elt {
        // Reject constantexpr elements.
        let Some(element) = constant.aggregate_element(i) else {
            return Ok(None);
        };

        if is_undef_or_poison(element) {
            // Correctly Propagate undef values.
            for _ in 0..ratio {
                result.push(dst_elt_ty.undef().as_constant());
            }
            continue;
        }

        let Ok(src) = ConstantIntValue::<IntDyn, B, C>::try_from(element) else {
            return Ok(None);
        };

        let mut shift_amt = if is_little_endian {
            0
        } else {
            dst_bit_size * (ratio - 1)
        };
        for _ in 0..ratio {
            // Shift the piece of the value into the right place, depending on
            // endianness.
            let elt = src.ap_int().lshr(shift_amt);
            shift_amt = if is_little_endian {
                shift_amt.wrapping_add(dst_bit_size)
            } else {
                shift_amt.wrapping_sub(dst_bit_size)
            };

            // Truncate and remember this piece.
            let piece = elt.zext_or_trunc(dst_bit_size);
            result.push(dst_int_ty.const_ap_int(&piece)?.as_constant());
        }
    }
    Ok(Some(result))
}

/// Mirrors `FoldBitCast` (`ConstantFolding.cpp`), step for step: constant fold a
/// bitcast, symbolically evaluating it with `DataLayout`. Upstream always
/// answers a constant, which may be a constant expression if unfoldable. `None`
/// is a pair upstream asserts on: the public entries can be handed one.
fn fold_bitcast_with_layout<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
    dest_ty: Type<'ctx, B, C>,
    dl: &DataLayout,
) -> IrResult<Option<Constant<'ctx, B, C>>> {
    if !cast_is_valid(CastOpcode::BitCast, constant.ty(), dest_ty) {
        return Ok(None);
    }

    // Catch the obvious splat cases.
    if let Some(folded) =
        constant_fold_load_from_uniform_value_trusting_same_module(constant, dest_ty, dl)?
    {
        return Ok(Some(folded));
    }

    let module = constant.module;
    // Handle a vector->scalar integer/fp cast.
    if let Some((src_elt_ty, num_src_elts, _)) = constant.ty().data().as_vector()
        && (dest_ty.is_integer() || dest_ty.is_floating_point())
    {
        let src_elt_ty = Type::new(src_elt_ty, module);
        return fold_vector_to_scalar_bitcast(constant, dest_ty, src_elt_ty, num_src_elts, dl);
    }

    // The code below only handles casts to vectors currently.
    let Some((dst_elt_ty, num_dst_elt, _)) = dest_ty.data().as_vector() else {
        return constant_expr_bit_cast(constant, dest_ty).map(Some);
    };

    // If this is a scalar -> vector cast, convert the input into a <1 x scalar>
    // vector so the code below can handle it uniformly.
    if !constant.ty().is_vector()
        && matches!(
            constant.kind(),
            ConstantKind::Data(ConstantDataKind::Int(_) | ConstantDataKind::Float(_))
        )
    {
        let vector_ty = vector_type_with_scalability(module, constant.ty(), lanes::<1>(), false);
        let vector = vector_ty.const_vector::<Constant<'ctx, B, C>, _>([constant])?;
        return fold_bitcast_with_layout(vector.as_constant(), dest_ty, dl);
    }

    // Some of what follows may extend to cover scalable vectors but the current
    // implementation is fixed length specific.
    let Some((src_elt_ty, num_src_elt, false)) = constant.ty().data().as_vector() else {
        return constant_expr_bit_cast(constant, dest_ty).map(Some);
    };

    // If this is a bitcast from constant vector -> vector, fold it.
    if !matches!(
        constant.kind(),
        ConstantKind::Data(
            ConstantDataKind::Sequential(ConstantSequentialKind::Vector)
                | ConstantDataKind::Int(_)
                | ConstantDataKind::Float(_)
        ) | ConstantKind::Aggregate(ConstantAggregateKind::Vector)
    ) {
        return constant_expr_bit_cast(constant, dest_ty).map(Some);
    }

    // If the element types match, IR can fold it.
    if num_dst_elt == num_src_elt {
        return constant_expr_bit_cast(constant, dest_ty).map(Some);
    }

    let src_elt_ty = Type::new(src_elt_ty, module);
    let dst_elt_ty = Type::new(dst_elt_ty, module);

    // Otherwise, we're changing the number of elements in a vector, which
    // requires endianness information to do the right thing. For example,
    //    bitcast (<2 x i64> <i64 0, i64 1> to <4 x i32>)
    // folds to (little endian):
    //    <4 x i32> <i32 0, i32 0, i32 1, i32 0>
    // and to (big endian):
    //    <4 x i32> <i32 0, i32 0, i32 0, i32 1>

    // First thing is first. We only want to think about integer here, so if we
    // have something in FP form, recast it as integer.
    if dst_elt_ty.is_floating_point() {
        // Fold to an vector of integers with same size as our FP type.
        let dest_iv_ty =
            integer_vector_type(module, dst_elt_ty.scalar_size_in_bits(), num_dst_elt)?;
        // Recursively handle this integer conversion, if possible.
        let Some(recast) = fold_bitcast_with_layout(constant, dest_iv_ty, dl)? else {
            return Ok(None);
        };

        // Finally, IR can handle this now that #elts line up.
        return constant_expr_bit_cast(recast, dest_ty).map(Some);
    }

    // Okay, we know the destination is integer, if the input is FP, convert it
    // to integer first.
    let constant = if src_elt_ty.is_floating_point() {
        let src_iv_ty = integer_vector_type(module, src_elt_ty.scalar_size_in_bits(), num_src_elt)?;
        // Ask IR to do the conversion now that #elts line up.
        constant_expr_bit_cast(constant, src_iv_ty)?
    } else {
        constant
    };

    // Now we know that the input and output vectors are both integer vectors
    // of the same size, and that their #elements is not the same. Do the
    // conversion here, which depends on whether the input or output has more
    // elements.
    let result = if num_dst_elt < num_src_elt {
        merge_vector_lanes(
            constant,
            dst_elt_ty,
            src_elt_ty.scalar_size_in_bits(),
            num_dst_elt.get(),
            num_src_elt.get(),
            dl,
        )?
    } else {
        split_vector_lanes(
            constant,
            dst_elt_ty,
            num_dst_elt.get(),
            num_src_elt.get(),
            dl,
        )?
    };
    match result {
        Some(lanes) => constant_aggregate_from_elements(dest_ty, lanes),
        None => constant_expr_bit_cast(constant, dest_ty).map(Some),
    }
}

fn cast_constant_expr_opcode(opcode: CastOpcode) -> Option<ConstantExprOpcode> {
    match opcode {
        CastOpcode::Trunc => Some(ConstantExprOpcode::Trunc),
        CastOpcode::PtrToAddr => Some(ConstantExprOpcode::PtrToAddr),
        CastOpcode::PtrToInt => Some(ConstantExprOpcode::PtrToInt),
        CastOpcode::IntToPtr => Some(ConstantExprOpcode::IntToPtr),
        CastOpcode::BitCast => Some(ConstantExprOpcode::BitCast),
        CastOpcode::AddrSpaceCast => Some(ConstantExprOpcode::AddrSpaceCast),
        CastOpcode::Zext
        | CastOpcode::Sext
        | CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::FpToUi
        | CastOpcode::FpToSi
        | CastOpcode::UiToFp
        | CastOpcode::SiToFp => None,
    }
}

fn int_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    bits: u32,
) -> IrResult<IntType<'ctx, IntDyn, B, C>> {
    if !(MIN_INT_BITS..=MAX_INT_BITS).contains(&bits) {
        return Err(IrError::InvalidIntegerWidth { bits });
    }
    Ok(IntType::new(module.context().int_type(bits), module))
}

fn pointer_address_space<B: ModuleBrand, C: Capability>(
    ty: Type<'_, B, C>,
) -> Option<crate::AddressSpace> {
    match ty.data() {
        TypeData::Pointer { addr_space } => Some(*addr_space),
        _ => None,
    }
}

fn index_bits_for_pointer<B: ModuleBrand, C: Capability>(
    ty: Type<'_, B, C>,
    dl: &DataLayout,
) -> Option<u32> {
    pointer_address_space(ty).map(|addr_space| dl.index_size_in_bits(addr_space))
}

fn denormal_mode_for_instruction<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    parent_id: Option<ValueSlot>,
    module: ModuleRef<'ctx, B, C>,
    ty: Type<'ctx, B, C>,
) -> DenormalMode {
    let Ok(float_ty) = FloatType::<FloatDyn, B, C>::try_from(ty) else {
        return DenormalMode::dynamic();
    };
    // In no block, so in no function whose `denormal-fp-math` attribute could
    // be read — the same answer this routine gives for a block with no parent
    // function.
    let Some(parent_id) = parent_id else {
        return DenormalMode::dynamic();
    };
    let parent = module.context().value_data(parent_id);
    let ValueKindData::BasicBlock(block) = &parent.kind else {
        return DenormalMode::dynamic();
    };
    let Some(function_id) = *block.parent.borrow() else {
        return DenormalMode::dynamic();
    };
    let ValueKindData::Function(_) = &module.context().value_data(function_id).kind else {
        return DenormalMode::dynamic();
    };
    let function = FunctionValue::<Dyn, B, C>::from_parts_unchecked(function_id, module);
    function.denormal_mode(float_ty.semantics())
}

fn is_guaranteed_not_to_be_undef_or_poison<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    constant: Constant<'ctx, B, C>,
) -> bool {
    let module = constant.module;
    match &constant.as_erased().data().kind {
        ValueKindData::Constant(ConstantData::Undef | ConstantData::Poison) => false,
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => elements
            .iter()
            .copied()
            .all(|id| constant_id_guaranteed_not_to_be_undef_or_poison(module, id)),
        ValueKindData::Constant(ConstantData::PtrAuth {
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        }) => [
            *pointer,
            *key,
            *discriminator,
            *addr_discriminator,
            *deactivation_symbol,
        ]
        .iter()
        .copied()
        .all(|id| constant_id_guaranteed_not_to_be_undef_or_poison(module, id)),
        ValueKindData::Constant(ConstantData::Expr(_) | ConstantData::ForwardRefPlaceholder) => {
            false
        }
        ValueKindData::Constant(
            ConstantData::Int(_)
            | ConstantData::Float(_)
            | ConstantData::GlobalValueRef { .. }
            | ConstantData::PointerNull
            | ConstantData::GepOffset { .. }
            | ConstantData::SymbolDelta { .. }
            | ConstantData::SymbolDeltaPlus { .. }
            | ConstantData::BlockAddress { .. }
            | ConstantData::DsoLocalEquivalent { .. }
            | ConstantData::NoCfi { .. }
            | ConstantData::TokenNone
            | ConstantData::TargetExtNone,
        ) => true,
        _ => false,
    }
}

fn constant_id_guaranteed_not_to_be_undef_or_poison<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    module: ModuleRef<'ctx, B, C>,
    id: ValueSlot,
) -> bool {
    let data = module.context().value_data(id);
    let ValueKindData::Constant(_) = &data.kind else {
        return false;
    };
    is_guaranteed_not_to_be_undef_or_poison(Constant::from_parts(Value::from_parts(
        id, module, data.ty,
    )))
}
