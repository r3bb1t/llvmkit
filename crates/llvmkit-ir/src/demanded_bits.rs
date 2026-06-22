//! Demanded-bits analysis for integer values.
//!
//! Mirrors `llvm/include/llvm/Analysis/DemandedBits.h` and
//! `llvm/lib/Analysis/DemandedBits.cpp` for the integer instruction subset
//! currently modelled by llvmkit.

use crate::analysis::{
    AllAnalysesOnFunction, CFGAnalyses, FunctionAnalysis, FunctionAnalysisInvalidator,
    FunctionAnalysisManager, FunctionAnalysisResult, PreservedAnalyses,
};
use crate::constant::ConstantData;
use crate::constants::ConstantIntValue;
use crate::data_layout::DataLayout;
use crate::instr_types::{BinaryOpData, CallInstData, CastOpData, CastOpcode, InvokeInstData};
use crate::instruction::{Instruction, InstructionData, InstructionKindData, state};
use crate::int_width::IntDyn;
use crate::intrinsics::IntrinsicId;
use crate::iter::BlockCursor;
use crate::module::{ModuleBrand, ModuleRef};
use crate::pass_context::{FunctionPassContext, FunctionView};
use crate::pass_manager::FunctionPass;
use crate::r#type::{Type, TypeKind};
use crate::value::{Value, ValueId, ValueKindData, ValueUse};
use crate::value_tracking::{ValueTrackingQuery, compute_known_bits};
use crate::{ApInt, IrError, IrResult, KnownBits};
use core::ops::Not;
use std::collections::{HashMap, HashSet, VecDeque};

/// Function analysis that computes demanded bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DemandedBitsAnalysis;

/// Function transform that replaces integer instructions whose demanded bits
/// are all known with constants, then erases the original instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SimplifyDemandedBitsPass;

/// Result of one demanded-bits simplification query.
pub struct SimplifyDemandedBitsResult<'ctx, B: ModuleBrand = crate::module::Brand<'ctx>> {
    known: KnownBits,
    demanded: ApInt,
    replacement: Option<ConstantIntValue<'ctx, IntDyn, B>>,
    demanded_bits_changed: bool,
}

impl<'ctx, B: ModuleBrand + 'ctx> SimplifyDemandedBitsResult<'ctx, B> {
    /// Known bits computed for the queried value.
    #[inline]
    pub fn known_bits(&self) -> &KnownBits {
        &self.known
    }

    /// Alive bits demanded by the value's users.
    #[inline]
    pub fn demanded_bits(&self) -> &ApInt {
        &self.demanded
    }

    /// Replacement constant when every demanded bit is known.
    #[inline]
    pub fn replacement(&self) -> Option<ConstantIntValue<'ctx, IntDyn, B>> {
        self.replacement
    }

    /// `true` when the demanded mask is narrower than the full scalar width.
    #[inline]
    pub fn demanded_bits_changed(&self) -> bool {
        self.demanded_bits_changed
    }
}

/// Compute whether `value` can be replaced by an integer constant for all bits
/// demanded by its current users.
pub fn simplify_demanded_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    demanded_bits: &DemandedBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<SimplifyDemandedBitsResult<'ctx, B>> {
    let Some(width) = int_scalar_bit_width(value.ty()) else {
        return Ok(SimplifyDemandedBitsResult {
            known: KnownBits::unknown(value_scalar_size_in_bits(value, query.data_layout())),
            demanded: ApInt::zero(0),
            replacement: None,
            demanded_bits_changed: false,
        });
    };
    let demanded = demanded_bits.get_demanded_bits(value);
    let full = ApInt::low_bits_set(width, u32::MAX);
    let known = compute_known_bits(value, query)?;
    let known_mask = known.zero_mask().bitor(known.one_mask());
    let unknown_demanded = demanded.bitand(&known_mask.not());
    let replacement = if unknown_demanded.is_zero() {
        let int_ty = crate::derived_types::IntType::<IntDyn, B>::try_from(value.ty())?;
        Some(int_ty.const_ap_int(known.one_mask())?)
    } else {
        None
    };
    Ok(SimplifyDemandedBitsResult {
        known,
        demanded_bits_changed: demanded != full,
        demanded,
        replacement,
    })
}

/// Cached demanded-bits result for one function.
pub struct DemandedBits {
    data_layout: DataLayout,
    alive_bits: HashMap<ValueId, ApInt>,
    operand_bits: HashMap<(ValueId, usize), ApInt>,
    dead_uses: HashSet<(ValueId, usize)>,
    visited_non_integer: HashSet<ValueId>,
    always_live: HashSet<ValueId>,
}

impl DemandedBits {
    fn new(data_layout: DataLayout) -> Self {
        Self {
            data_layout,
            alive_bits: HashMap::new(),
            operand_bits: HashMap::new(),
            dead_uses: HashSet::new(),
            visited_non_integer: HashSet::new(),
            always_live: HashSet::new(),
        }
    }

    /// Return the bits demanded from an instruction value.
    pub fn get_demanded_bits<'ctx, B: ModuleBrand + 'ctx>(&self, value: Value<'ctx, B>) -> ApInt {
        if let Some(bits) = self.alive_bits.get(&value.id()) {
            return bits.clone();
        }
        ApInt::low_bits_set(
            value_scalar_size_in_bits(value, &self.data_layout),
            u32::MAX,
        )
    }

    /// Return the bits demanded from operand `operand_index` of instruction `user`.
    pub fn get_operand_demanded_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        operand_index: usize,
    ) -> IrResult<ApInt> {
        let operands = instruction_operands(user)?;
        let Some(operand_id) = operands.get(operand_index).copied() else {
            return Err(IrError::InvalidOperation {
                message: "operand index out of range",
            });
        };
        if let Some(bits) = self.operand_bits.get(&(user.id(), operand_index)) {
            return Ok(bits.clone());
        }
        let operand = value_from_id(user, operand_id);
        let Some(width) = int_scalar_bit_width(operand.ty()) else {
            return Ok(ApInt::low_bits_set(
                value_scalar_size_in_bits(operand, &self.data_layout),
                u32::MAX,
            ));
        };
        if self.is_use_dead(user, operand_index)? {
            return Ok(ApInt::zero(width));
        }
        let a_out = self.get_demanded_bits(user);
        self.determine_live_operand_bits(user, operand, operand_index, &a_out)
    }

    /// Return true if `value` was unreachable from any live root during analysis.
    pub fn is_instruction_dead<'ctx, B: ModuleBrand + 'ctx>(&self, value: Value<'ctx, B>) -> bool {
        !self.visited_non_integer.contains(&value.id())
            && !self.alive_bits.contains_key(&value.id())
            && !self.always_live.contains(&value.id())
    }

    /// Return true if operand `operand_index` of instruction `user` has no demanded bits.
    pub fn is_use_dead<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        operand_index: usize,
    ) -> IrResult<bool> {
        let operands = instruction_operands(user)?;
        let Some(operand_id) = operands.get(operand_index).copied() else {
            return Err(IrError::InvalidOperation {
                message: "operand index out of range",
            });
        };
        let operand = value_from_id(user, operand_id);
        if int_scalar_bit_width(operand.ty()).is_none() {
            return Ok(false);
        }
        if self.always_live.contains(&user.id()) {
            return Ok(false);
        }
        if self.is_instruction_dead(user) {
            return Ok(true);
        }
        if self.dead_uses.contains(&(user.id(), operand_index)) {
            return Ok(true);
        }
        Ok(self.alive_bits.get(&user.id()).is_some_and(ApInt::is_zero))
    }

    /// Compute alive bits of one addition operand from alive output and known operands.
    pub fn determine_live_operand_bits_add(
        operand_index: usize,
        alive_out: &ApInt,
        lhs: &KnownBits,
        rhs: &KnownBits,
    ) -> ApInt {
        determine_live_operand_bits_add_carry(operand_index, alive_out, lhs, rhs, true, false)
    }

    /// Compute alive bits of one subtraction operand from alive output and known operands.
    pub fn determine_live_operand_bits_sub(
        operand_index: usize,
        alive_out: &ApInt,
        lhs: &KnownBits,
        rhs: &KnownBits,
    ) -> ApInt {
        let neg_rhs = KnownBits::from_zero_one(rhs.one_mask().clone(), rhs.zero_mask().clone())
            .unwrap_or_else(|_| KnownBits::unknown(rhs.bit_width()));
        determine_live_operand_bits_add_carry(operand_index, alive_out, lhs, &neg_rhs, false, true)
    }

    fn perform_analysis<'ctx, B: ModuleBrand + 'ctx>(
        &mut self,
        function: FunctionView<'ctx, B>,
    ) -> IrResult<()> {
        let mut worklist = VecDeque::new();
        let mut queued = HashSet::new();
        let anchor = function.as_function().as_value();

        for block in function.as_function().basic_blocks() {
            for inst in block.instructions() {
                let value = inst.as_value();
                let ValueKindData::Instruction(data) = &value.data().kind else {
                    continue;
                };
                if !is_always_live(data) {
                    continue;
                }
                self.always_live.insert(value.id());
                if let Some(width) = int_scalar_bit_width(value.ty()) {
                    self.alive_bits
                        .entry(value.id())
                        .or_insert_with(|| ApInt::zero(width));
                    enqueue(value.id(), &mut worklist, &mut queued);
                    continue;
                }
                for operand_id in data.kind.operand_ids() {
                    let operand = value_from_id(anchor, operand_id);
                    if is_instruction_value(operand) {
                        if let Some(width) = int_scalar_bit_width(operand.ty()) {
                            self.alive_bits
                                .insert(operand_id, ApInt::low_bits_set(width, u32::MAX));
                        } else {
                            self.visited_non_integer.insert(operand_id);
                        }
                        enqueue(operand_id, &mut worklist, &mut queued);
                    }
                }
            }
        }

        while let Some(user_id) = worklist.pop_back() {
            queued.remove(&user_id);
            let user = value_from_id(anchor, user_id);
            let ValueKindData::Instruction(user_data) = &user.data().kind else {
                continue;
            };
            let user_int_width = int_scalar_bit_width(user.ty());
            let input_known_dead = user_int_width.is_some()
                && self.alive_bits.get(&user_id).is_some_and(ApInt::is_zero)
                && !is_always_live(user_data);
            let a_out = user_int_width
                .and_then(|_| self.alive_bits.get(&user_id).cloned())
                .unwrap_or_else(|| ApInt::zero(0));

            for (operand_index, operand_id) in user_data.kind.operand_ids().into_iter().enumerate()
            {
                let operand = value_from_id(anchor, operand_id);
                if let Some(width) = int_scalar_bit_width(operand.ty()) {
                    let alive = if input_known_dead {
                        ApInt::zero(width)
                    } else {
                        self.determine_live_operand_bits(user, operand, operand_index, &a_out)?
                    };
                    self.operand_bits
                        .insert((user_id, operand_index), alive.clone());
                    if alive.is_zero() {
                        self.dead_uses.insert((user_id, operand_index));
                    } else {
                        self.dead_uses.remove(&(user_id, operand_index));
                    }
                    if is_instruction_value(operand) {
                        let previous = self.alive_bits.get(&operand_id).cloned();
                        let merged = previous
                            .as_ref()
                            .map_or_else(|| alive.clone(), |old| old.bitor(&alive));
                        let changed = previous.as_ref().is_none_or(|old| !merged.eq_ap_int(old));
                        if changed {
                            self.alive_bits.insert(operand_id, merged);
                            enqueue(operand_id, &mut worklist, &mut queued);
                        }
                    }
                } else if is_instruction_value(operand)
                    && self.visited_non_integer.insert(operand_id)
                {
                    enqueue(operand_id, &mut worklist, &mut queued);
                }
            }
        }
        Ok(())
    }

    fn determine_live_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        operand: Value<'ctx, B>,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<ApInt> {
        let width = int_scalar_bit_width(operand.ty()).unwrap_or(0);
        let all = ApInt::low_bits_set(width, u32::MAX);
        let ValueKindData::Instruction(data) = &user.data().kind else {
            return Ok(all);
        };
        Ok(match &data.kind {
            InstructionKindData::Add(bin) => {
                if alive_out.is_mask() {
                    alive_out.clone()
                } else {
                    let (lhs, rhs) = self.known_binary_operands(user, bin)?;
                    Self::determine_live_operand_bits_add(operand_index, alive_out, &lhs, &rhs)
                }
            }
            InstructionKindData::Sub(bin) => {
                if alive_out.is_mask() {
                    alive_out.clone()
                } else {
                    let (lhs, rhs) = self.known_binary_operands(user, bin)?;
                    Self::determine_live_operand_bits_sub(operand_index, alive_out, &lhs, &rhs)
                }
            }
            InstructionKindData::Mul(_) => ApInt::low_bits_set(width, alive_out.active_bits()),
            InstructionKindData::Shl(bin) => {
                self.shift_left_operand_bits(user, bin, operand_index, alive_out)?
            }
            InstructionKindData::LShr(bin) => {
                self.logical_shift_right_operand_bits(user, bin, operand_index, alive_out)?
            }
            InstructionKindData::AShr(bin) => {
                self.arithmetic_shift_right_operand_bits(user, bin, operand_index, alive_out)?
            }
            InstructionKindData::And(bin) => {
                self.and_operand_bits(user, bin, operand_index, alive_out)?
            }
            InstructionKindData::Or(bin) => {
                self.or_operand_bits(user, bin, operand_index, alive_out)?
            }
            InstructionKindData::Xor(_)
            | InstructionKindData::Phi(_)
            | InstructionKindData::Freeze(_) => alive_out.clone(),
            InstructionKindData::Cast(cast) => cast_operand_bits(cast, width, alive_out),
            InstructionKindData::Select(_) => {
                if operand_index == 0 {
                    all
                } else {
                    alive_out.clone()
                }
            }
            InstructionKindData::ExtractElement(_) => {
                if operand_index == 0 {
                    alive_out.clone()
                } else {
                    all
                }
            }
            InstructionKindData::InsertElement(_) | InstructionKindData::ShuffleVector(_) => {
                if operand_index == 0 || operand_index == 1 {
                    alive_out.clone()
                } else {
                    all
                }
            }
            InstructionKindData::Call(call) => self
                .intrinsic_call_operand_bits(user, call, operand_index, alive_out)?
                .unwrap_or(all),
            InstructionKindData::Invoke(invoke) => self
                .intrinsic_invoke_operand_bits(user, invoke, operand_index, alive_out)?
                .unwrap_or(all),
            InstructionKindData::UDiv(_)
            | InstructionKindData::SDiv(_)
            | InstructionKindData::URem(_)
            | InstructionKindData::SRem(_)
            | InstructionKindData::FAdd(_)
            | InstructionKindData::FSub(_)
            | InstructionKindData::FMul(_)
            | InstructionKindData::FDiv(_)
            | InstructionKindData::FRem(_)
            | InstructionKindData::ICmp(_)
            | InstructionKindData::FCmp(_)
            | InstructionKindData::Alloca(_)
            | InstructionKindData::Load(_)
            | InstructionKindData::Store(_)
            | InstructionKindData::Gep(_)
            | InstructionKindData::Ret(_)
            | InstructionKindData::Br(_)
            | InstructionKindData::FNeg(_)
            | InstructionKindData::VAArg(_)
            | InstructionKindData::ExtractValue(_)
            | InstructionKindData::InsertValue(_)
            | InstructionKindData::Fence(_)
            | InstructionKindData::AtomicCmpXchg(_)
            | InstructionKindData::AtomicRMW(_)
            | InstructionKindData::Switch(_)
            | InstructionKindData::IndirectBr(_)
            | InstructionKindData::CallBr(_)
            | InstructionKindData::LandingPad(_)
            | InstructionKindData::Resume(_)
            | InstructionKindData::CleanupPad(_)
            | InstructionKindData::CatchPad(_)
            | InstructionKindData::CatchReturn(_)
            | InstructionKindData::CleanupReturn(_)
            | InstructionKindData::CatchSwitch(_)
            | InstructionKindData::Unreachable(_) => all,
        })
    }

    fn known_binary_operands<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        bin: &BinaryOpData,
    ) -> IrResult<(KnownBits, KnownBits)> {
        let query = ValueTrackingQuery::new(&self.data_layout);
        let lhs = compute_known_bits(value_from_id(user, bin.lhs.get()), &query)?;
        let rhs = compute_known_bits(value_from_id(user, bin.rhs.get()), &query)?;
        Ok((lhs, rhs))
    }

    fn known_shift_range<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        bin: &BinaryOpData,
        width: u32,
    ) -> IrResult<(u32, u32)> {
        let rhs = value_from_id(user, bin.rhs.get());
        let query = ValueTrackingQuery::new(&self.data_layout);
        let known = compute_known_bits(rhs, &query)?;
        let limit = u64::from(width.saturating_sub(1));
        let min = u32::try_from(known.min_value().limited_value(limit)).unwrap_or(width);
        let max = u32::try_from(known.max_value().limited_value(limit)).unwrap_or(width);
        Ok((min, max))
    }

    fn shift_left_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        bin: &BinaryOpData,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<ApInt> {
        let width = alive_out.bit_width();
        if operand_index != 0 {
            return Ok(ApInt::low_bits_set(
                int_scalar_bit_width(value_from_id(user, bin.rhs.get()).ty()).unwrap_or(width),
                u32::MAX,
            ));
        }
        let shift =
            constant_ap_int(value_from_id(user, bin.rhs.get())).map(|v| limited_shift(&v, width));
        if let Some(amount) = shift {
            let mut bits = lshr_or_zero(alive_out, amount);
            if bin.no_signed_wrap {
                bits = bits.bitor(&ApInt::high_bits_set(width, amount.saturating_add(1)));
            } else if bin.no_unsigned_wrap {
                bits = bits.bitor(&ApInt::high_bits_set(width, amount));
            }
            Ok(bits)
        } else {
            let (min, max) = self.known_shift_range(user, bin, width)?;
            let mut bits = shifted_range_bits(alive_out, min, max, false);
            if bin.no_signed_wrap {
                bits = bits.bitor(&ApInt::high_bits_set(width, max.saturating_add(1)));
            } else if bin.no_unsigned_wrap {
                bits = bits.bitor(&ApInt::high_bits_set(width, max));
            }
            Ok(bits)
        }
    }

    fn logical_shift_right_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        bin: &BinaryOpData,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<ApInt> {
        let width = alive_out.bit_width();
        if operand_index != 0 {
            return Ok(ApInt::low_bits_set(
                int_scalar_bit_width(value_from_id(user, bin.rhs.get()).ty()).unwrap_or(width),
                u32::MAX,
            ));
        }
        let shift =
            constant_ap_int(value_from_id(user, bin.rhs.get())).map(|v| limited_shift(&v, width));
        if let Some(amount) = shift {
            let mut bits = shl_or_zero(alive_out, amount);
            if bin.is_exact {
                bits = bits.bitor(&ApInt::low_bits_set(width, amount));
            }
            Ok(bits)
        } else {
            let (min, max) = self.known_shift_range(user, bin, width)?;
            let mut bits = shifted_range_bits(alive_out, min, max, true);
            if bin.is_exact {
                bits = bits.bitor(&ApInt::low_bits_set(width, max));
            }
            Ok(bits)
        }
    }

    fn arithmetic_shift_right_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        bin: &BinaryOpData,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<ApInt> {
        let width = alive_out.bit_width();
        if operand_index != 0 {
            return Ok(ApInt::low_bits_set(
                int_scalar_bit_width(value_from_id(user, bin.rhs.get()).ty()).unwrap_or(width),
                u32::MAX,
            ));
        }
        let shift =
            constant_ap_int(value_from_id(user, bin.rhs.get())).map(|v| limited_shift(&v, width));
        if let Some(amount) = shift {
            let mut bits = shl_or_zero(alive_out, amount);
            if alive_out.intersects(&ApInt::high_bits_set(width, amount)) && width != 0 {
                bits = bits.bitor(&ApInt::sign_mask(width));
            }
            if bin.is_exact {
                bits = bits.bitor(&ApInt::low_bits_set(width, amount));
            }
            Ok(bits)
        } else {
            let (min, max) = self.known_shift_range(user, bin, width)?;
            let mut bits = shifted_range_bits(alive_out, min, max, true);
            if max != 0 && alive_out.intersects(&ApInt::high_bits_set(width, max)) {
                bits = bits.bitor(&ApInt::sign_mask(width));
            }
            if bin.is_exact {
                bits = bits.bitor(&ApInt::low_bits_set(width, max));
            }
            Ok(bits)
        }
    }

    fn intrinsic_call_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        call: &CallInstData,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<Option<ApInt>> {
        self.intrinsic_operand_bits(user, call.callee.get(), operand_index, alive_out)
    }

    fn intrinsic_invoke_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        invoke: &InvokeInstData,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<Option<ApInt>> {
        self.intrinsic_operand_bits(user, invoke.callee.get(), operand_index, alive_out)
    }

    fn intrinsic_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        callee_id: ValueId,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<Option<ApInt>> {
        let Some(arg_index) = operand_index.checked_sub(1) else {
            return Ok(None);
        };
        let Some(id) = intrinsic_id_for_callee(value_from_id(user, callee_id)) else {
            return Ok(None);
        };
        let width = alive_out.bit_width();
        Ok(match id {
            IntrinsicId::BSwap if arg_index == 0 => Some(alive_out.byte_swap()),
            IntrinsicId::BitReverse if arg_index == 0 => Some(alive_out.reverse_bits()),
            IntrinsicId::CTLZ if arg_index == 0 => {
                let known = compute_known_bits(
                    operand_value(user, operand_index)?,
                    &ValueTrackingQuery::new(&self.data_layout),
                )?;
                Some(ApInt::high_bits_set(
                    width,
                    known.count_max_leading_zeros().saturating_add(1).min(width),
                ))
            }
            IntrinsicId::CTTZ if arg_index == 0 => {
                let known = compute_known_bits(
                    operand_value(user, operand_index)?,
                    &ValueTrackingQuery::new(&self.data_layout),
                )?;
                Some(ApInt::low_bits_set(
                    width,
                    known
                        .count_max_trailing_zeros()
                        .saturating_add(1)
                        .min(width),
                ))
            }
            IntrinsicId::FShl | IntrinsicId::FShr => {
                self.funnel_shift_operand_bits(user, id, arg_index, alive_out)?
            }
            IntrinsicId::UMax | IntrinsicId::UMin | IntrinsicId::SMax | IntrinsicId::SMin
                if arg_index == 0 || arg_index == 1 =>
            {
                Some(ApInt::bits_set_from(
                    width,
                    alive_out.count_trailing_zeros(),
                ))
            }
            _ => None,
        })
    }

    fn funnel_shift_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        id: IntrinsicId,
        arg_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<Option<ApInt>> {
        let width = alive_out.bit_width();
        if arg_index == 2 {
            return Ok(is_power_of_two_u32(width)
                .then(|| ApInt::from_words(width, &[u64::from(width.saturating_sub(1))])));
        }
        let operands = instruction_operands(user)?;
        let Some(shift_value_id) = operands.get(3).copied() else {
            return Ok(None);
        };
        let Some(shift) = constant_ap_int(value_from_id(user, shift_value_id)) else {
            return Ok(None);
        };
        let mut shift = apint_unsigned_rem_u32(&shift, width);
        if id == IntrinsicId::FShr && shift != 0 {
            shift = width.saturating_sub(shift);
        }
        Ok(match arg_index {
            0 => Some(lshr_or_zero(alive_out, shift)),
            1 => Some(shl_or_zero(alive_out, width.saturating_sub(shift))),
            _ => None,
        })
    }

    fn and_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        bin: &BinaryOpData,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<ApInt> {
        let (lhs, rhs) = self.known_binary_operands(user, bin)?;
        let bits = if operand_index == 0 {
            alive_out.bitand(&rhs.zero_mask().not())
        } else {
            let lhs_only_zero = lhs.zero_mask().bitand(&rhs.zero_mask().not());
            alive_out.bitand(&lhs_only_zero.not())
        };
        Ok(bits)
    }

    fn or_operand_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        user: Value<'ctx, B>,
        bin: &BinaryOpData,
        operand_index: usize,
        alive_out: &ApInt,
    ) -> IrResult<ApInt> {
        let (lhs, rhs) = self.known_binary_operands(user, bin)?;
        let bits = if operand_index == 0 {
            alive_out.bitand(&rhs.one_mask().not())
        } else {
            let lhs_only_one = lhs.one_mask().bitand(&rhs.one_mask().not());
            alive_out.bitand(&lhs_only_one.not())
        };
        Ok(bits)
    }
}

impl<'ctx> FunctionPass<'ctx> for SimplifyDemandedBitsPass {
    fn run(&mut self, cx: &mut FunctionPassContext<'_, 'ctx>) -> IrResult<PreservedAnalyses> {
        let mut changed = false;
        loop {
            let iteration_changed = simplify_demanded_bits_iteration(cx)?;
            if !iteration_changed {
                break;
            }
            changed = true;
        }

        if changed {
            let mut preserved = PreservedAnalyses::none();
            preserved.preserve_set::<CFGAnalyses>();
            Ok(preserved)
        } else {
            Ok(PreservedAnalyses::all())
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysis<'ctx, B> for DemandedBitsAnalysis {
    type Result = DemandedBits;

    fn run(
        &self,
        function: FunctionView<'ctx, B>,
        _am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result> {
        let mut result = DemandedBits::new(function.module().data_layout().clone());
        result.perform_analysis(function)?;
        Ok(result)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisResult<'ctx, B> for DemandedBits {
    fn invalidate(
        &mut self,
        _function: FunctionView<'ctx, B>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        let checker = pa.checker::<DemandedBitsAnalysis>();
        Ok(!(checker.preserved() || checker.preserved_set::<AllAnalysesOnFunction>()))
    }
}

fn determine_live_operand_bits_add_carry(
    operand_index: usize,
    alive_out: &ApInt,
    lhs: &KnownBits,
    rhs: &KnownBits,
    carry_zero: bool,
    carry_one: bool,
) -> ApInt {
    let bound = lhs
        .zero_mask()
        .bitand(rhs.zero_mask())
        .bitor(&lhs.one_mask().bitand(rhs.one_mask()));
    let reverse_bound = reverse_bits(&bound);
    let reverse_alive_out = reverse_bits(alive_out);
    let reverse_bound_not = (&reverse_bound).not();
    let reverse_prop = reverse_alive_out.wrapping_add(&reverse_alive_out.bitor(&reverse_bound_not));
    let reverse_alive_carry = reverse_prop.bitxor(&reverse_bound_not);
    let alive_carry = reverse_bits(&reverse_alive_carry);

    let needed_zero = if operand_index == 0 {
        lhs.zero_mask().bitor(&rhs.zero_mask().not())
    } else {
        rhs.zero_mask().bitor(&lhs.zero_mask().not())
    };
    let needed_one = if operand_index == 0 {
        lhs.one_mask().bitor(&rhs.one_mask().not())
    } else {
        rhs.one_mask().bitor(&lhs.one_mask().not())
    };
    let carry_zero_bit =
        ApInt::from_words(alive_out.bit_width(), &[if carry_zero { 0 } else { 1 }]);
    let carry_one_bit = ApInt::from_words(alive_out.bit_width(), &[if carry_one { 1 } else { 0 }]);
    let possible_sum_zero = lhs
        .zero_mask()
        .not()
        .wrapping_add(&rhs.zero_mask().not())
        .wrapping_add(&carry_zero_bit);
    let possible_sum_one = lhs
        .one_mask()
        .wrapping_add(rhs.one_mask())
        .wrapping_add(&carry_one_bit);
    let needed_carry = possible_sum_zero
        .not()
        .bitor(&needed_zero)
        .bitand(&possible_sum_one.bitor(&needed_one));
    alive_out.bitor(&alive_carry.bitand(&needed_carry))
}

fn cast_operand_bits(cast: &CastOpData, src_width: u32, alive_out: &ApInt) -> ApInt {
    match cast.kind {
        CastOpcode::Trunc => alive_out.zext_or_trunc(src_width),
        CastOpcode::ZExt => alive_out
            .trunc(src_width)
            .unwrap_or_else(|| ApInt::low_bits_set(src_width, u32::MAX)),
        CastOpcode::SExt => {
            let mut bits = alive_out
                .trunc(src_width)
                .unwrap_or_else(|| ApInt::low_bits_set(src_width, u32::MAX));
            let extended_bits = alive_out.bit_width().saturating_sub(src_width);
            if alive_out.intersects(&ApInt::high_bits_set(alive_out.bit_width(), extended_bits))
                && src_width != 0
            {
                bits = bits.bitor(&ApInt::sign_mask(src_width));
            }
            bits
        }
        CastOpcode::PtrToAddr
        | CastOpcode::PtrToInt
        | CastOpcode::IntToPtr
        | CastOpcode::BitCast
        | CastOpcode::AddrSpaceCast => alive_out.zext_or_trunc(src_width),
        CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::FpToUI
        | CastOpcode::FpToSI
        | CastOpcode::UIToFp
        | CastOpcode::SIToFp => ApInt::low_bits_set(src_width, u32::MAX),
    }
}

fn reverse_bits(value: &ApInt) -> ApInt {
    let width = value.bit_width();
    let mut out = ApInt::zero(width);
    let mut bit = 0;
    while bit < width {
        if value.is_one_bit_set(bit) {
            out = out.bitor(&ApInt::one_bit_set(width, width - bit - 1));
        }
        bit += 1;
    }
    out
}

fn shl_or_zero(value: &ApInt, amount: u32) -> ApInt {
    value
        .checked_shl(amount)
        .unwrap_or_else(|| ApInt::zero(value.bit_width()))
}

fn lshr_or_zero(value: &ApInt, amount: u32) -> ApInt {
    value
        .checked_lshr(amount)
        .unwrap_or_else(|| ApInt::zero(value.bit_width()))
}

fn shifted_range_bits(alive_out: &ApInt, min: u32, max: u32, shift_left: bool) -> ApInt {
    let shift = |mask: &ApInt, amount| {
        if shift_left {
            shl_or_zero(mask, amount)
        } else {
            lshr_or_zero(mask, amount)
        }
    };
    let mut loop_range = max.saturating_sub(min);
    let mut mask = alive_out.clone();
    let mut shifted = alive_out.clone();
    let mut shift_amount = 1;
    while shift_amount <= loop_range {
        if loop_range & shift_amount != 0 {
            mask = mask.bitor(&shift(
                &shifted,
                loop_range.saturating_sub(shift_amount).saturating_add(1),
            ));
            loop_range = loop_range.saturating_sub(shift_amount);
        }
        shifted = shifted.bitor(&shift(&shifted, shift_amount));
        shift_amount = shift_amount.saturating_mul(2);
        if shift_amount == 0 {
            break;
        }
    }
    shift(&mask, min)
}

fn limited_shift(value: &ApInt, bit_width: u32) -> u32 {
    let limit = u64::from(bit_width.saturating_sub(1));
    u32::try_from(value.limited_value(limit)).unwrap_or(bit_width.saturating_sub(1))
}

fn apint_unsigned_rem_u32(value: &ApInt, divisor: u32) -> u32 {
    if divisor == 0 {
        return 0;
    }
    let divisor = ApInt::from_words(value.bit_width(), &[u64::from(divisor)]);
    value
        .checked_urem(&divisor)
        .and_then(|remainder| remainder.try_zext_u64())
        .and_then(|remainder| u32::try_from(remainder).ok())
        .unwrap_or(0)
}

fn operand_value<'ctx, B: ModuleBrand + 'ctx>(
    user: Value<'ctx, B>,
    operand_index: usize,
) -> IrResult<Value<'ctx, B>> {
    let operands = instruction_operands(user)?;
    let Some(id) = operands.get(operand_index).copied() else {
        return Err(IrError::InvalidOperation {
            message: "operand index out of range",
        });
    };
    Ok(value_from_id(user, id))
}

fn simplify_demanded_bits_iteration<'ctx>(
    cx: &mut FunctionPassContext<'_, 'ctx>,
) -> IrResult<bool> {
    let data_layout = cx.module().data_layout().clone();
    let mut demanded = DemandedBits::new(data_layout.clone());
    demanded.perform_analysis(cx.function())?;
    let query = ValueTrackingQuery::new(&data_layout);
    let module_token = cx.module_mut();
    let mut dead_to_erase = Vec::new();

    for block in cx.function_mut().basic_blocks() {
        let mut cursor = BlockCursor::at_start(block);
        while let Some((inst, next)) = cursor.next() {
            cursor = next;
            let value = inst.as_value();
            if !is_simplify_candidate(value) {
                continue;
            }
            if demanded.is_instruction_dead(value) {
                dead_to_erase.push(value.id());
                continue;
            }
            let simplified = simplify_demanded_bits(value, &demanded, &query)?;
            if let Some(replacement) = simplified.replacement() {
                let id = value.id();
                drop_zext_nneg_for_replaced_uses(value);
                inst.replace_all_uses_with(module_token, replacement)?;
                let erased =
                    Instruction::<state::Attached>::from_parts(id, module_token.module_ref());
                erased.erase_from_parent(module_token);
                return Ok(true);
            }
            if let Some(replacement) = demanded_value_replacement(value, &demanded, &query)? {
                let id = value.id();
                drop_zext_nneg_for_replaced_uses(value);
                inst.replace_all_uses_with(module_token, replacement)?;
                let erased =
                    Instruction::<state::Attached>::from_parts(id, module_token.module_ref());
                erased.erase_from_parent(module_token);
                return Ok(true);
            }
            if mark_non_negative_zext(value, &query)? {
                return Ok(true);
            }
            if simplify_demanded_operands(value, &demanded, &query)? {
                return Ok(true);
            }
        }
    }

    for id in dead_to_erase.into_iter().rev() {
        let erased = Instruction::<state::Attached>::from_parts(id, module_token.module_ref());
        if erased.as_value().has_uses() {
            continue;
        }
        erased.erase_from_parent(module_token);
        return Ok(true);
    }

    Ok(false)
}

fn simplify_demanded_operands<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    demanded_bits: &DemandedBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let demanded = demanded_bits.get_demanded_bits(value);
    let ValueKindData::Instruction(inst) = &value.data().kind else {
        return Ok(false);
    };
    match &inst.kind {
        InstructionKindData::And(bin) => {
            let lhs = value_from_id(value, bin.lhs.get());
            let lhs_known = compute_known_bits(lhs, query)?;
            let mask = demanded.bitand(&lhs_known.zero_mask().not());
            shrink_demanded_constant_operand(value, &bin.rhs, &mask)
        }
        InstructionKindData::Or(bin) => {
            shrink_demanded_constant_operand(value, &bin.rhs, &demanded)
        }
        InstructionKindData::Xor(bin) => simplify_xor_constant_operand(value, bin, &demanded),
        InstructionKindData::Add(bin)
        | InstructionKindData::Sub(bin)
        | InstructionKindData::Mul(bin) => {
            simplify_unused_high_bits_constant_operands(value, bin, &demanded)
        }
        _ => Ok(false),
    }
}

fn demanded_value_replacement<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    demanded_bits: &DemandedBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<Option<Value<'ctx, B>>> {
    let demanded = demanded_bits.get_demanded_bits(value);
    if !demanded.is_all_ones() && value.num_uses() != 1 {
        return Ok(None);
    }
    let ValueKindData::Instruction(inst) = &value.data().kind else {
        return Ok(None);
    };
    let replacement = match &inst.kind {
        InstructionKindData::And(bin) => {
            let lhs = value_from_id(value, bin.lhs.get());
            let rhs = value_from_id(value, bin.rhs.get());
            let lhs_known = compute_known_bits(lhs, query)?;
            let rhs_known = compute_known_bits(rhs, query)?;
            if is_subset_of(
                &demanded,
                &lhs_known.zero_mask().bitor(rhs_known.one_mask()),
            ) {
                Some(lhs)
            } else if is_subset_of(
                &demanded,
                &rhs_known.zero_mask().bitor(lhs_known.one_mask()),
            ) {
                Some(rhs)
            } else {
                None
            }
        }
        InstructionKindData::Or(bin) => {
            let lhs = value_from_id(value, bin.lhs.get());
            let rhs = value_from_id(value, bin.rhs.get());
            let lhs_known = compute_known_bits(lhs, query)?;
            let rhs_known = compute_known_bits(rhs, query)?;
            if is_subset_of(
                &demanded,
                &lhs_known.one_mask().bitor(rhs_known.zero_mask()),
            ) {
                Some(lhs)
            } else if is_subset_of(
                &demanded,
                &rhs_known.one_mask().bitor(lhs_known.zero_mask()),
            ) {
                Some(rhs)
            } else {
                None
            }
        }
        InstructionKindData::Xor(bin) => {
            let lhs = value_from_id(value, bin.lhs.get());
            let rhs = value_from_id(value, bin.rhs.get());
            let lhs_known = compute_known_bits(lhs, query)?;
            let rhs_known = compute_known_bits(rhs, query)?;
            if is_subset_of(&demanded, rhs_known.zero_mask()) {
                Some(lhs)
            } else if is_subset_of(&demanded, lhs_known.zero_mask()) {
                Some(rhs)
            } else {
                None
            }
        }
        _ => None,
    };
    Ok(replacement)
}

// Matches LLVM's `dropPoisonGeneratingFlags()` when demanded-bits
// simplification rewrites the value feeding a `zext nneg`, or mutates that
// value in place: the old operand may have been the proof that the source was
// non-negative.
fn drop_zext_nneg_for_replaced_uses<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) {
    let mut visited = HashSet::new();
    drop_zext_nneg_for_replaced_uses_recursive(value, &mut visited);
}

fn drop_zext_nneg_for_replaced_uses_recursive<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    visited: &mut HashSet<ValueId>,
) {
    if !visited.insert(value.id()) {
        return;
    }
    for user in value.users() {
        let user = user.as_value();
        drop_zext_nneg_for_replaced_operand(user, value.id());
        drop_zext_nneg_for_replaced_uses_recursive(user, visited);
    }
}

fn drop_zext_nneg_for_replaced_operand<'ctx, B: ModuleBrand + 'ctx>(
    user: Value<'ctx, B>,
    old_operand: ValueId,
) {
    let ValueKindData::Instruction(inst) = &user.data().kind else {
        return;
    };
    let InstructionKindData::Cast(cast) = &inst.kind else {
        return;
    };
    if cast.kind == CastOpcode::ZExt && cast.src.get() == old_operand && cast.nneg.get() {
        cast.nneg.set(false);
    }
}

fn mark_non_negative_zext<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let ValueKindData::Instruction(inst) = &value.data().kind else {
        return Ok(false);
    };
    let InstructionKindData::Cast(cast) = &inst.kind else {
        return Ok(false);
    };
    if cast.kind != CastOpcode::ZExt || cast.nneg.get() {
        return Ok(false);
    }
    let src = value_from_id(value, cast.src.get());
    let known = compute_known_bits(src, query)?;
    if !known.is_non_negative() {
        return Ok(false);
    }
    cast.nneg.set(true);
    Ok(true)
}

fn is_subset_of(bits: &ApInt, mask: &ApInt) -> bool {
    bits.bitand(&mask.not()).is_zero()
}

fn simplify_unused_high_bits_constant_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    bin: &BinaryOpData,
    demanded: &ApInt,
) -> IrResult<bool> {
    if bin.no_unsigned_wrap || bin.no_signed_wrap {
        return Ok(false);
    }
    let width = demanded.bit_width();
    let demanded_from_ops =
        ApInt::low_bits_set(width, width.saturating_sub(demanded.count_leading_zeros()));
    let lhs_changed = shrink_demanded_constant_operand(value, &bin.lhs, &demanded_from_ops)?;
    let rhs_changed = shrink_demanded_constant_operand(value, &bin.rhs, &demanded_from_ops)?;
    Ok(lhs_changed || rhs_changed)
}

fn simplify_xor_constant_operand<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    bin: &BinaryOpData,
    demanded: &ApInt,
) -> IrResult<bool> {
    let rhs = value_from_id(value, bin.rhs.get());
    let Some(rhs_bits) = constant_ap_int(rhs) else {
        return Ok(false);
    };
    if rhs_bits.is_all_ones() {
        return Ok(false);
    }
    if rhs_bits.bitor(&demanded.not()).is_all_ones() {
        let int_ty = crate::derived_types::IntType::<IntDyn, B>::try_from(rhs.ty())?;
        let all_ones = int_ty.const_ap_int(&ApInt::all_ones(demanded.bit_width()))?;
        return replace_instruction_operand(value, &bin.rhs, all_ones.as_value());
    }
    shrink_demanded_constant_operand(value, &bin.rhs, demanded)
}

fn shrink_demanded_constant_operand<'ctx, B: ModuleBrand + 'ctx>(
    user: Value<'ctx, B>,
    operand: &core::cell::Cell<ValueId>,
    demanded: &ApInt,
) -> IrResult<bool> {
    let current = value_from_id(user, operand.get());
    let Some(current_bits) = constant_ap_int(current) else {
        return Ok(false);
    };
    let demanded = demanded.zext_or_trunc(current_bits.bit_width());
    let shrunk = current_bits.bitand(&demanded);
    if shrunk.eq_ap_int(&current_bits) {
        return Ok(false);
    }
    let int_ty = crate::derived_types::IntType::<IntDyn, B>::try_from(current.ty())?;
    let replacement = int_ty.const_ap_int(&shrunk)?;
    replace_instruction_operand(user, operand, replacement.as_value())
}

fn replace_instruction_operand<'ctx, B: ModuleBrand + 'ctx>(
    user: Value<'ctx, B>,
    operand: &core::cell::Cell<ValueId>,
    replacement: Value<'ctx, B>,
) -> IrResult<bool> {
    if replacement.module().id() != user.module().id() {
        return Err(IrError::ForeignValue);
    }
    let old_id = operand.get();
    let new_id = replacement.id();
    if old_id == new_id {
        return Ok(false);
    }
    let old = value_from_id(user, old_id);
    if old.ty().id() != replacement.ty().id() {
        return Err(IrError::TypeMismatch {
            expected: old.ty().kind_label(),
            got: replacement.ty().kind_label(),
        });
    }
    drop_zext_nneg_for_replaced_operand(user, old_id);
    drop_zext_nneg_for_replaced_uses(user);
    operand.set(new_id);
    let module = user.module().core_ref();
    let edge = ValueUse::Instruction(user.id());
    let mut old_uses = module.context().value_data(old_id).use_list.borrow_mut();
    if let Some(pos) = old_uses.iter().position(|candidate| *candidate == edge) {
        old_uses.remove(pos);
    }
    drop(old_uses);
    module
        .context()
        .value_data(new_id)
        .use_list
        .borrow_mut()
        .push(edge);
    Ok(true)
}

fn intrinsic_id_for_callee<'ctx, B: ModuleBrand + 'ctx>(
    callee: Value<'ctx, B>,
) -> Option<IntrinsicId> {
    let ValueKindData::Function(function) = &callee.data().kind else {
        return None;
    };
    IntrinsicId::lookup(&function.name)
}

fn is_power_of_two_u32(value: u32) -> bool {
    value != 0 && (value & (value - 1)) == 0
}

fn constant_ap_int<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApInt> {
    let width = int_scalar_bit_width(value.ty())?;
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(words)) => Some(ApInt::from_words(width, words)),
        _ => None,
    }
}

fn instruction_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> IrResult<Vec<ValueId>> {
    match &value.data().kind {
        ValueKindData::Instruction(inst) => Ok(inst.kind.operand_ids()),
        other => Err(IrError::ValueCategoryMismatch {
            expected: crate::ValueCategoryLabel::Instruction,
            got: crate::value::category_label_for_kind(other),
        }),
    }
}

fn is_always_live(inst: &InstructionData) -> bool {
    inst.kind.is_terminator()
        || matches!(
            inst.kind,
            InstructionKindData::Store(_)
                | InstructionKindData::Fence(_)
                | InstructionKindData::AtomicCmpXchg(_)
                | InstructionKindData::AtomicRMW(_)
                | InstructionKindData::Call(_)
                | InstructionKindData::Invoke(_)
                | InstructionKindData::CallBr(_)
                | InstructionKindData::VAArg(_)
        )
}

fn is_simplify_candidate<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    if int_scalar_bit_width(value.ty()).is_none() {
        return false;
    }
    let ValueKindData::Instruction(inst) = &value.data().kind else {
        return false;
    };
    if is_always_live(inst) {
        return false;
    }
    matches!(
        inst.kind,
        InstructionKindData::Add(_)
            | InstructionKindData::Sub(_)
            | InstructionKindData::Mul(_)
            | InstructionKindData::Shl(_)
            | InstructionKindData::LShr(_)
            | InstructionKindData::AShr(_)
            | InstructionKindData::And(_)
            | InstructionKindData::Or(_)
            | InstructionKindData::Xor(_)
            | InstructionKindData::Cast(_)
            | InstructionKindData::Select(_)
            | InstructionKindData::Phi(_)
            | InstructionKindData::Freeze(_)
            | InstructionKindData::ICmp(_)
    )
}

fn enqueue(id: ValueId, worklist: &mut VecDeque<ValueId>, queued: &mut HashSet<ValueId>) {
    if queued.insert(id) {
        worklist.push_back(id);
    }
}

fn is_instruction_value<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    matches!(value.data().kind, ValueKindData::Instruction(_))
}

fn value_from_id<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    id: ValueId,
) -> Value<'ctx, B> {
    let module = module_ref(anchor);
    let data = module.value_data(id);
    Value::from_parts(id, module, data.ty)
}

fn module_ref<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> ModuleRef<'ctx, B> {
    ModuleRef::new(value.module().core_ref())
}

fn module_ref_from_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> ModuleRef<'ctx, B> {
    ModuleRef::new(ty.module().core_ref())
}

fn int_scalar_bit_width<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<u32> {
    match ty.kind() {
        TypeKind::Integer { bits } => Some(bits),
        TypeKind::FixedVector | TypeKind::ScalableVector => {
            let (elem, _, _) = ty.data().as_vector()?;
            int_scalar_bit_width(Type::new(elem, module_ref_from_type(ty)))
        }
        TypeKind::Void
        | TypeKind::Half
        | TypeKind::BFloat
        | TypeKind::Float
        | TypeKind::Double
        | TypeKind::X86Fp80
        | TypeKind::Fp128
        | TypeKind::PpcFp128
        | TypeKind::X86Amx
        | TypeKind::Label
        | TypeKind::Metadata
        | TypeKind::Token
        | TypeKind::Function
        | TypeKind::Array
        | TypeKind::Pointer { .. }
        | TypeKind::Struct
        | TypeKind::TypedPointer
        | TypeKind::TargetExt => None,
    }
}

fn value_scalar_size_in_bits<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    dl: &DataLayout,
) -> u32 {
    if let Some(width) = int_scalar_bit_width(value.ty()) {
        return width;
    }
    u32::try_from(dl.type_size_in_bits(erase_type(value.ty()))).unwrap_or(0)
}

fn erase_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Type<'ctx> {
    Type::new(ty.id(), ModuleRef::new(ty.module().core_ref()))
}
