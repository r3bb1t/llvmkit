//! Integer/pointer known-bits queries over IR values.
//!
//! Mirrors the `computeKnownBits` slice of `llvm/lib/Analysis/ValueTracking.cpp`.

use crate::align::Align;
use crate::analysis::{
    AllAnalysesOnFunction, CFGAnalyses, FunctionAnalysis, FunctionAnalysisInvalidator,
    FunctionAnalysisManager, FunctionAnalysisResult, PrefetchableAnalysis, PreservedAnalyses,
};
use crate::attributes::{AttrIndex, AttrKind, AttributeStorage, AttributeStored};
use crate::cmp_predicate::IntPredicate;
use crate::constant::{ConstantData, ConstantExprData, ConstantExprOpcode};
use crate::constant_range::{ConstantRange, constant_ranges_from_metadata};
use crate::data_layout::DataLayout;
use crate::dominator_tree::{DominatorTree, DominatorTreeAnalysis};
use crate::instr_types::{
    AllocaInstData, BinaryOpData, BinaryOpcode, CastOpData, CastOpcode, CmpInstData,
    ExtractElementInstData, GepInstData, InsertElementInstData, POISON_MASK_ELEM,
    ShuffleVectorInstData,
};
use crate::instruction::{InstructionData, InstructionKindData, InstructionView};
use crate::intrinsics::{IntrinsicSemantic, semantic_for_callee};
use crate::metadata::MetadataAttachmentKind;
use crate::module::{Brand, ModuleBrand, ModuleCore, ModuleRef};
use crate::pass_context::FunctionView;
use crate::r#type::{Type, TypeData, TypeId, TypeKind};
use crate::value::{Value, ValueId, ValueKindData};
use crate::{ApInt, IrResult, KnownBits};
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::ops::Not;
use std::collections::{HashMap, HashSet};

/// Default recursion limit. Mirrors LLVM's `MaxAnalysisRecursionDepth`.
pub const MAX_ANALYSIS_RECURSION_DEPTH: u32 = 6;

/// Function analysis that serves [`compute_known_bits`] queries.
///
/// Mirrors LLVM's new-PM analysis pattern around
/// `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBits`: the pass-manager
/// result owns the module data layout snapshot and reuses the per-result cache
/// across queries for the same function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KnownBitsAnalysis;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct KnownBitsCacheKey {
    value: ValueId,
    context_instruction: Option<ValueId>,
    demanded_elements: Option<ApInt>,
    uses_instruction_info: bool,
}

impl KnownBitsCacheKey {
    #[inline]
    fn new<'a, 'ctx, B: ModuleBrand>(
        value: ValueId,
        query: &ValueTrackingQuery<'a, 'ctx, B>,
    ) -> Self {
        Self {
            value,
            context_instruction: query
                .context_instruction
                .map(|instruction| instruction.id()),
            demanded_elements: query.demanded_elements.cloned(),
            uses_instruction_info: query.uses_instruction_info(),
        }
    }
}

type KnownBitsCacheStore = RefCell<HashMap<KnownBitsCacheKey, KnownBits>>;

enum QueryCache<'a> {
    Owned(KnownBitsCacheStore),
    Borrowed(&'a KnownBitsCacheStore),
}

impl<'a> QueryCache<'a> {
    #[inline]
    fn owned() -> Self {
        Self::Owned(RefCell::new(HashMap::new()))
    }

    #[inline]
    fn borrowed(cache: &'a KnownBitsCacheStore) -> Self {
        Self::Borrowed(cache)
    }

    #[inline]
    fn store(&self) -> &KnownBitsCacheStore {
        match self {
            Self::Owned(cache) => cache,
            Self::Borrowed(cache) => cache,
        }
    }
}

/// Cached result for [`KnownBitsAnalysis`].
pub struct KnownBitsAnalysisResult {
    data_layout: DataLayout,
    max_depth: u32,
    dominator_tree: Option<DominatorTree>,
    cache: KnownBitsCacheStore,
}

impl KnownBitsAnalysisResult {
    #[inline]
    pub fn query<'ctx, B: ModuleBrand + 'ctx>(&self) -> ValueTrackingQuery<'_, 'ctx, B> {
        let query = ValueTrackingQuery::new(&self.data_layout)
            .with_max_depth(self.max_depth)
            .with_shared_cache(&self.cache);
        if let Some(dominator_tree) = &self.dominator_tree {
            query.with_dominator_tree(dominator_tree)
        } else {
            query
        }
    }

    #[inline]
    pub fn compute_known_bits<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        value: Value<'ctx, B>,
    ) -> IrResult<KnownBits> {
        compute_known_bits(value, &self.query())
    }

    #[inline]
    pub fn is_known_non_zero<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        value: Value<'ctx, B>,
    ) -> IrResult<bool> {
        is_known_non_zero(value, &self.query())
    }

    #[inline]
    pub fn is_known_zero<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        value: Value<'ctx, B>,
    ) -> IrResult<bool> {
        is_known_zero(value, &self.query())
    }

    #[inline]
    pub fn is_known_one<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        value: Value<'ctx, B>,
        bit: u32,
    ) -> IrResult<bool> {
        is_known_one(value, bit, &self.query())
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysis<'ctx, B> for KnownBitsAnalysis {
    type Result = KnownBitsAnalysisResult;

    fn run(
        &self,
        function: FunctionView<'ctx, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result> {
        let dominator_tree = am
            .get_cached_result_by_type::<DominatorTreeAnalysis, DominatorTree, _>(function)
            .cloned();
        Ok(KnownBitsAnalysisResult {
            data_layout: function.module().data_layout().clone(),
            max_depth: MAX_ANALYSIS_RECURSION_DEPTH,
            dominator_tree,
            cache: RefCell::new(HashMap::new()),
        })
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> PrefetchableAnalysis<'ctx, B> for KnownBitsAnalysis {
    #[inline]
    fn ensure_registered(fam: &mut FunctionAnalysisManager<'ctx, B>) {
        fam.ensure_registered_default::<Self>();
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisResult<'ctx, B> for KnownBitsAnalysisResult {
    fn invalidate(
        &mut self,
        _function: FunctionView<'ctx, B>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        let checker = pa.checker::<KnownBitsAnalysis>();
        if !(checker.preserved() || checker.preserved_set::<AllAnalysesOnFunction>()) {
            return Ok(true);
        }
        if self.dominator_tree.is_some() {
            let dom_checker = pa.checker::<DominatorTreeAnalysis>();
            return Ok(!(dom_checker.preserved()
                || dom_checker.preserved_set::<AllAnalysesOnFunction>()
                || dom_checker.preserved_set::<CFGAnalyses>()));
        }
        Ok(false)
    }
}

/// Per-query state for known-bits computations.
pub struct ValueTrackingQuery<'a, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    data_layout: &'a DataLayout,
    max_depth: u32,
    dominator_tree: Option<&'a DominatorTree>,
    context_instruction: Option<Value<'ctx, B>>,
    demanded_elements: Option<&'a ApInt>,
    use_instr_info: bool,
    cache: QueryCache<'a>,
    _brand: PhantomData<(&'ctx (), B)>,
}

impl<'a, 'ctx, B: ModuleBrand + 'ctx> ValueTrackingQuery<'a, 'ctx, B> {
    #[inline]
    pub fn new(data_layout: &'a DataLayout) -> Self {
        Self {
            data_layout,
            max_depth: MAX_ANALYSIS_RECURSION_DEPTH,
            dominator_tree: None,
            context_instruction: None,
            demanded_elements: None,
            use_instr_info: true,
            cache: QueryCache::owned(),
            _brand: PhantomData,
        }
    }

    #[inline]
    pub fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth;
        self
    }

    #[inline]
    pub fn with_dominator_tree(mut self, dominator_tree: &'a DominatorTree) -> Self {
        self.dominator_tree = Some(dominator_tree);
        self
    }

    #[inline]
    pub fn with_context_instruction(mut self, instruction: &InstructionView<'ctx, B>) -> Self {
        self.context_instruction = Some(instruction.as_value());
        self
    }

    #[inline]
    pub fn with_demanded_elements(mut self, demanded_elements: &'a ApInt) -> Self {
        self.demanded_elements = Some(demanded_elements);
        self
    }

    #[inline]
    pub fn without_instruction_info(mut self) -> Self {
        self.use_instr_info = false;
        self
    }

    #[inline]
    pub fn with_instruction_info(mut self) -> Self {
        self.use_instr_info = true;
        self
    }

    #[inline]
    fn with_shared_cache(mut self, cache: &'a KnownBitsCacheStore) -> Self {
        self.cache = QueryCache::borrowed(cache);
        self
    }

    #[inline]
    fn with_temporary_demanded_elements<'b>(
        &'b self,
        demanded_elements: &'b ApInt,
    ) -> ValueTrackingQuery<'b, 'ctx, B> {
        ValueTrackingQuery {
            data_layout: self.data_layout,
            max_depth: self.max_depth,
            dominator_tree: self.dominator_tree,
            context_instruction: self.context_instruction,
            demanded_elements: Some(demanded_elements),
            use_instr_info: self.use_instr_info,
            cache: QueryCache::borrowed(self.cache()),
            _brand: PhantomData,
        }
    }

    #[inline]
    pub fn data_layout(&self) -> &DataLayout {
        self.data_layout
    }

    #[inline]
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    #[inline]
    pub fn dominator_tree(&self) -> Option<&DominatorTree> {
        self.dominator_tree
    }

    #[inline]
    pub fn context_instruction(&self) -> Option<Value<'ctx, B>> {
        self.context_instruction
    }

    #[inline]
    pub fn demanded_elements(&self) -> Option<&ApInt> {
        self.demanded_elements
    }

    #[inline]
    pub fn uses_instruction_info(&self) -> bool {
        self.use_instr_info
    }

    #[inline]
    fn cache(&self) -> &KnownBitsCacheStore {
        self.cache.store()
    }
}

/// Determine which bits of `value` are known zero/one.
pub fn compute_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<KnownBits> {
    let mut stack = HashSet::new();
    compute_known_bits_inner(value, query, 0, &mut stack)
}

/// Return true when `value` is known non-zero.
pub fn is_known_non_zero<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    Ok(compute_known_bits(value, query)?.is_non_zero())
}

/// Return true when `value` is known zero.
pub fn is_known_zero<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    Ok(compute_known_bits(value, query)?.is_zero())
}

/// Return true when `bit` of `value` is known one.
pub fn is_known_one<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    bit: u32,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    Ok(compute_known_bits(value, query)?.is_known_one(bit))
}

/// Compute known bits for an instruction/operator value, or unknown for non-operators.
pub fn known_bits_from_operator<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<KnownBits> {
    let mut stack = HashSet::new();
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    match &value.data().kind {
        ValueKindData::Instruction(inst) => {
            compute_instruction_known_bits(value, inst, query, 0, &mut stack)
        }
        _ => Ok(KnownBits::unknown(width)),
    }
}

fn compute_known_bits_inner<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    if depth > query.max_depth() {
        return Ok(KnownBits::unknown(width));
    }
    if stack.contains(&value.id()) {
        return Ok(KnownBits::unknown(width));
    }
    let cache_key = KnownBitsCacheKey::new(value.id(), query);
    if let Some(cached) = query.cache().borrow().get(&cache_key).cloned() {
        return Ok(cached);
    }

    stack.insert(value.id());
    let known = match &value.data().kind {
        ValueKindData::Constant(c) => compute_constant_known_bits(value, c, query, depth, stack)?,
        ValueKindData::Instruction(inst) => {
            compute_instruction_known_bits(value, inst, query, depth, stack)?
        }
        ValueKindData::Argument { .. }
        | ValueKindData::BasicBlock(_)
        | ValueKindData::Function(_)
        | ValueKindData::GlobalAlias(_)
        | ValueKindData::GlobalIFunc(_)
        | ValueKindData::GlobalVariable(_)
        | ValueKindData::MetadataAsValue(_)
        | ValueKindData::InlineAsm(_) => KnownBits::unknown(width),
    };
    stack.remove(&value.id());
    query.cache().borrow_mut().insert(cache_key, known.clone());
    Ok(known)
}

fn compute_constant_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    constant: &ConstantData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    Ok(match constant {
        ConstantData::Int(words) => KnownBits::from_ap_int(ApInt::from_words(width, words)),
        ConstantData::PointerNull => KnownBits::from_ap_int(ApInt::zero(width)),
        ConstantData::Expr(expr) => {
            compute_constant_expr_known_bits(value, expr, query, depth, stack)?
        }
        ConstantData::Undef | ConstantData::Poison => KnownBits::unknown(width),
        ConstantData::Aggregate(elements) => {
            aggregate_constant_known_bits(value, elements, query, depth, stack)?
        }
        ConstantData::Float(_)
        | ConstantData::GlobalValueRef { .. }
        | ConstantData::BlockAddressPlaceholder
        | ConstantData::GepOffset { .. }
        | ConstantData::SymbolDelta { .. }
        | ConstantData::SymbolDeltaPlus { .. }
        | ConstantData::BlockAddress { .. }
        | ConstantData::DSOLocalEquivalent { .. }
        | ConstantData::NoCfi { .. }
        | ConstantData::PtrAuth { .. }
        | ConstantData::TokenNone
        | ConstantData::TargetExtNone => KnownBits::unknown(width),
    })
}

fn compute_constant_expr_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    expr: &ConstantExprData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(anchor, query.data_layout()).unwrap_or(0);
    let operand = |idx: usize| {
        expr.operands
            .get(idx)
            .copied()
            .map(|id| value_from_id(anchor, id))
    };
    let Some(lhs) = operand(0) else {
        return Ok(KnownBits::unknown(width));
    };
    let lhs_bits = compute_known_bits_inner(lhs, query, depth + 1, stack)?;
    Ok(match expr.opcode {
        ConstantExprOpcode::Add => {
            if let Some(rhs) = operand(1) {
                KnownBits::add(
                    &lhs_bits,
                    &compute_known_bits_inner(rhs, query, depth + 1, stack)?,
                )
            } else {
                KnownBits::unknown(width)
            }
        }
        ConstantExprOpcode::Sub => {
            if let Some(rhs) = operand(1) {
                KnownBits::sub(
                    &lhs_bits,
                    &compute_known_bits_inner(rhs, query, depth + 1, stack)?,
                )
            } else {
                KnownBits::unknown(width)
            }
        }
        ConstantExprOpcode::Xor => {
            if let Some(rhs) = operand(1) {
                KnownBits::bitxor(
                    &lhs_bits,
                    &compute_known_bits_inner(rhs, query, depth + 1, stack)?,
                )
            } else {
                KnownBits::unknown(width)
            }
        }
        ConstantExprOpcode::Trunc => lhs_bits.trunc(width),
        ConstantExprOpcode::BitCast
        | ConstantExprOpcode::PtrToAddr
        | ConstantExprOpcode::PtrToInt
        | ConstantExprOpcode::IntToPtr => lhs_bits.zext_or_trunc(width),
        ConstantExprOpcode::AddrSpaceCast => KnownBits::unknown(width),
        ConstantExprOpcode::GetElementPtr => {
            let Some(source_ty) = expr.source_ty else {
                return Ok(KnownBits::unknown(width));
            };
            let indices = expr
                .operands
                .iter()
                .skip(1)
                .copied()
                .map(|id| value_from_id(anchor, id));
            gep_known_bits_from_values(
                GepKnownBitsInput {
                    anchor,
                    width,
                    known: lhs_bits,
                    source_ty,
                    indices,
                },
                query,
                depth,
                stack,
            )?
        }
        ConstantExprOpcode::ShuffleVector
        | ConstantExprOpcode::InsertElement
        | ConstantExprOpcode::ExtractElement => KnownBits::unknown(width),
    })
}

fn compute_instruction_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    inst: &InstructionData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    let known = match &inst.kind {
        InstructionKindData::Add(data) => {
            let (lhs, rhs) = binary_operand_known_bits(value, data, query, depth, stack)?;
            Ok(KnownBits::add_with_flags(
                &lhs,
                &rhs,
                query.uses_instruction_info() && data.no_signed_wrap,
                query.uses_instruction_info() && data.no_unsigned_wrap,
            ))
        }
        InstructionKindData::Sub(data) => {
            let (lhs, rhs) = binary_operand_known_bits(value, data, query, depth, stack)?;
            Ok(KnownBits::sub_with_flags(
                &lhs,
                &rhs,
                query.uses_instruction_info() && data.no_signed_wrap,
                query.uses_instruction_info() && data.no_unsigned_wrap,
            ))
        }
        InstructionKindData::Mul(data) => mul_known(value, data, query, depth, stack),
        InstructionKindData::UDiv(data) => {
            let (lhs, rhs) = binary_operand_known_bits(value, data, query, depth, stack)?;
            Ok(KnownBits::udiv_with_exact(
                &lhs,
                &rhs,
                query.uses_instruction_info() && data.is_exact,
            ))
        }
        InstructionKindData::SDiv(data) => {
            let (lhs, rhs) = binary_operand_known_bits(value, data, query, depth, stack)?;
            Ok(KnownBits::sdiv_with_exact(
                &lhs,
                &rhs,
                query.uses_instruction_info() && data.is_exact,
            ))
        }
        InstructionKindData::URem(data) => {
            binary_known(value, data, query, depth, stack, KnownBits::urem)
        }
        InstructionKindData::SRem(data) => {
            binary_known(value, data, query, depth, stack, KnownBits::srem)
        }
        InstructionKindData::Shl(data) => {
            let (lhs, rhs) = binary_operand_known_bits(value, data, query, depth, stack)?;
            Ok(KnownBits::shl_with_flags(
                &lhs,
                &rhs,
                query.uses_instruction_info() && data.no_unsigned_wrap,
                query.uses_instruction_info() && data.no_signed_wrap,
                false,
            ))
        }
        InstructionKindData::LShr(data) => {
            let (lhs, rhs) = binary_operand_known_bits(value, data, query, depth, stack)?;
            Ok(KnownBits::lshr_with_flags(
                &lhs,
                &rhs,
                false,
                query.uses_instruction_info() && data.is_exact,
            ))
        }
        InstructionKindData::AShr(data) => {
            let (lhs, rhs) = binary_operand_known_bits(value, data, query, depth, stack)?;
            Ok(KnownBits::ashr_with_flags(
                &lhs,
                &rhs,
                false,
                query.uses_instruction_info() && data.is_exact,
            ))
        }
        InstructionKindData::And(data) => {
            bitwise_known(value, data, BinaryOpcode::And, query, depth, stack)
        }
        InstructionKindData::Or(data) => {
            bitwise_known(value, data, BinaryOpcode::Or, query, depth, stack)
        }
        InstructionKindData::Xor(data) => {
            bitwise_known(value, data, BinaryOpcode::Xor, query, depth, stack)
        }
        InstructionKindData::Cast(data) => cast_known(value, data, query, depth, stack),
        InstructionKindData::Select(data) => {
            let cond = value_from_id(value, data.cond.get());
            let true_val = value_from_id(value, data.true_val.get());
            let false_val = value_from_id(value, data.false_val.get());
            let cond_bits = compute_known_bits_inner(cond, query, depth + 1, stack)?;
            if cond_bits.constant().is_some_and(|c| c.is_one()) {
                compute_known_bits_inner(true_val, query, depth + 1, stack)
            } else if cond_bits.constant().is_some_and(|c| c.is_zero()) {
                compute_known_bits_inner(false_val, query, depth + 1, stack)
            } else {
                let true_bits = compute_known_bits_inner(true_val, query, depth + 1, stack)?;
                let false_bits = compute_known_bits_inner(false_val, query, depth + 1, stack)?;
                Ok(true_bits.intersect_with(&false_bits))
            }
        }
        InstructionKindData::Phi(data) => {
            let incoming = data.incoming.borrow();
            let mut iter = incoming.iter();
            let Some((first, _)) = iter.next() else {
                return Ok(KnownBits::unknown(width));
            };
            let mut known = compute_known_bits_inner(
                value_from_id(value, first.get()),
                query,
                depth + 1,
                stack,
            )?;
            for (incoming_value, _) in iter {
                let next = compute_known_bits_inner(
                    value_from_id(value, incoming_value.get()),
                    query,
                    depth + 1,
                    stack,
                )?;
                known = known.intersect_with(&next);
                if known.is_unknown() {
                    break;
                }
            }
            Ok(known)
        }
        InstructionKindData::Freeze(data) => {
            let src = value_from_id(value, data.src.get());
            if is_guaranteed_not_to_be_poison(src, query, depth + 1, stack)? {
                compute_known_bits_inner(src, query, depth + 1, stack)
            } else {
                Ok(KnownBits::unknown(width))
            }
        }
        InstructionKindData::ICmp(data) => icmp_known(value, data, query, depth, stack),
        InstructionKindData::Alloca(data) => Ok(alloca_known_bits(value, data, query)),
        InstructionKindData::Call(data) => call_known_bits(
            value,
            CallKnownBitsInputs {
                callee_id: data.callee.get(),
                args: &data.args,
                return_attrs: data.attrs.return_attrs(),
                arg_attrs: data.attrs.arg_attrs(),
            },
            query,
            depth,
            stack,
        ),
        InstructionKindData::Invoke(data) => call_known_bits(
            value,
            CallKnownBitsInputs {
                callee_id: data.callee.get(),
                args: &data.args,
                return_attrs: data.attrs.return_attrs(),
                arg_attrs: data.attrs.arg_attrs(),
            },
            query,
            depth,
            stack,
        ),
        InstructionKindData::FAdd(_)
        | InstructionKindData::FSub(_)
        | InstructionKindData::FMul(_)
        | InstructionKindData::FDiv(_)
        | InstructionKindData::FRem(_)
        | InstructionKindData::FCmp(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_) => Ok(KnownBits::unknown(width)),
        InstructionKindData::Gep(data) => gep_known_bits(value, data, query, depth, stack),
        InstructionKindData::ExtractElement(data) => {
            extract_element_known_bits(value, data, query, depth, stack)
        }
        InstructionKindData::InsertElement(data) => {
            insert_element_known_bits(value, data, query, depth, stack)
        }
        InstructionKindData::ShuffleVector(data) => {
            shuffle_vector_known_bits(value, data, query, depth, stack)
        }
        InstructionKindData::FNeg(_)
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
        | InstructionKindData::Ret(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::Unreachable(_) => Ok(KnownBits::unknown(width)),
    }?;
    Ok(
        if query.uses_instruction_info()
            && matches!(
                &inst.kind,
                InstructionKindData::Load(_)
                    | InstructionKindData::Call(_)
                    | InstructionKindData::Invoke(_)
            )
        {
            known.union_with(&range_metadata_known_bits(value, inst, width))
        } else {
            known
        },
    )
}

fn range_metadata_known_bits<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    inst: &InstructionData,
    bit_width: u32,
) -> KnownBits {
    let Some(range_id) = inst.metadata.borrow().get(&MetadataAttachmentKind::Range) else {
        return KnownBits::unknown(bit_width);
    };
    let module_view = value.module();
    let module = module_view.core_ref();
    let store = module_view.metadata_store();
    let expected_ty = scalar_type_id(module, value.ty().id);
    let Some(ranges) = constant_ranges_from_metadata(module, &store, range_id, expected_ty) else {
        return KnownBits::unknown(bit_width);
    };
    ranges_known_bits(ranges, bit_width)
}

fn range_attribute_known_bits<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    attrs: &AttributeStorage,
    bit_width: u32,
) -> KnownBits {
    let Some(stored) = attrs.get(AttrIndex::Return) else {
        return KnownBits::unknown(bit_width);
    };
    let module_view = value.module();
    let expected_ty = scalar_type_id(module_view.core_ref(), value.ty().id);
    let ranges = stored.iter().filter_map(|attr| match attr {
        AttributeStored::Range { ty, lower, upper } if *ty == expected_ty => {
            let range = ConstantRange::new(lower.clone(), upper.clone()).ok()?;
            (!range.is_empty_set() && !range.is_full_set()).then_some(range)
        }
        _ => None,
    });
    ranges_known_bits(ranges, bit_width)
}

fn ranges_known_bits(ranges: impl IntoIterator<Item = ConstantRange>, bit_width: u32) -> KnownBits {
    let mut seen = false;
    let mut known = KnownBits::unknown(bit_width);
    known.set_all_conflict();
    for range in ranges {
        seen = true;
        let unsigned_max = range.unsigned_max().zext_or_trunc(bit_width);
        let unsigned_min = range.unsigned_min().zext_or_trunc(bit_width);
        let common_prefix_bits = unsigned_max.bitxor(&unsigned_min).count_leading_zeros();
        let mask = ApInt::bits_set(
            bit_width,
            bit_width.saturating_sub(common_prefix_bits),
            bit_width,
        );
        let range_known = KnownBits::from_zero_one(
            unsigned_max.clone().not().bitand(&mask),
            unsigned_max.bitand(&mask),
        )
        .unwrap_or_else(|_| KnownBits::unknown(bit_width));
        known = known.intersect_with(&range_known);
    }
    if !seen || known.has_conflict() {
        KnownBits::unknown(bit_width)
    } else {
        known
    }
}

struct CallKnownBitsInputs<'a> {
    callee_id: ValueId,
    args: &'a [Cell<ValueId>],
    return_attrs: &'a AttributeStorage,
    arg_attrs: &'a [AttributeStorage],
}

fn call_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    inputs: CallKnownBitsInputs<'_>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(anchor, query.data_layout()).unwrap_or(0);
    let mut known = range_attribute_known_bits(anchor, inputs.return_attrs, width);
    if let Some(returned_arg) = returned_arg_operand(anchor, inputs.args, inputs.arg_attrs)
        && returned_arg.ty() == anchor.ty()
    {
        known = known.union_with(&compute_known_bits_inner(
            returned_arg,
            query,
            depth + 1,
            stack,
        )?);
    }
    if let Some(semantic) = intrinsic_semantic_for_callee(anchor, inputs.callee_id) {
        let intrinsic_known =
            intrinsic_known_bits(anchor, semantic, inputs.args, query, depth, stack)?;
        known = known.union_with(&intrinsic_known);
    }
    if known.has_conflict() {
        Ok(KnownBits::unknown(width))
    } else {
        Ok(known)
    }
}

fn returned_arg_operand<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    args: &[Cell<ValueId>],
    arg_attrs: &[AttributeStorage],
) -> Option<Value<'ctx, B>> {
    arg_attrs.iter().enumerate().find_map(|(idx, attrs)| {
        returned_attr(attrs, idx)
            .then(|| args.get(idx).map(|arg| value_from_id(anchor, arg.get())))?
    })
}

fn returned_attr(attrs: &AttributeStorage, idx: usize) -> bool {
    let direct_slot = attrs
        .get(AttrIndex::Param(0))
        .is_some_and(attribute_slice_has_returned);
    if direct_slot {
        return true;
    }
    let Some(idx) = u32::try_from(idx).ok() else {
        return false;
    };
    attrs
        .get(AttrIndex::Param(idx))
        .is_some_and(attribute_slice_has_returned)
}

fn attribute_slice_has_returned(attrs: &[AttributeStored]) -> bool {
    attrs
        .iter()
        .any(|attr| matches!(attr, AttributeStored::Enum(AttrKind::Returned)))
}

fn intrinsic_semantic_for_callee<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    callee_id: ValueId,
) -> Option<IntrinsicSemantic> {
    semantic_for_callee(value_from_id(anchor, callee_id))
}

fn intrinsic_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    semantic: IntrinsicSemantic,
    args: &[Cell<ValueId>],
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(anchor, query.data_layout()).unwrap_or(0);
    let arg = |idx: usize| args.get(idx).map(|cell| value_from_id(anchor, cell.get()));
    let arg_bits = |idx: usize, stack: &mut HashSet<ValueId>| -> IrResult<KnownBits> {
        let Some(value) = arg(idx) else {
            return Ok(KnownBits::unknown(width));
        };
        compute_known_bits_inner(value, query, depth + 1, stack)
    };
    match semantic {
        IntrinsicSemantic::Abs => {
            let input = arg_bits(0, stack)?;
            Ok(input.abs_with_int_min_poison(argument_is_const_one(arg(1))))
        }
        IntrinsicSemantic::BitReverse => Ok(arg_bits(0, stack)?.reverse_bits()),
        IntrinsicSemantic::BSwap => Ok(arg_bits(0, stack)?.byte_swap()),
        IntrinsicSemantic::Ctlz => {
            let input = arg_bits(0, stack)?;
            let mut possible = input.count_max_leading_zeros();
            if argument_is_const_one(arg(1)) {
                possible = possible.min(width.saturating_sub(1));
            }
            let mut known = KnownBits::unknown(width);
            known.set_known_zero_bits_from(bit_width_u32(possible));
            Ok(known)
        }
        IntrinsicSemantic::Cttz => {
            let input = arg_bits(0, stack)?;
            let mut possible = input.count_max_trailing_zeros();
            if argument_is_const_one(arg(1)) {
                possible = possible.min(width.saturating_sub(1));
            }
            let mut known = KnownBits::unknown(width);
            known.set_known_zero_bits_from(bit_width_u32(possible));
            Ok(known)
        }
        IntrinsicSemantic::Ctpop => {
            let input = arg_bits(0, stack)?;
            let mut known = KnownBits::unknown(width);
            known.set_known_zero_bits_from(bit_width_u32(input.count_max_population()));
            Ok(known)
        }
        IntrinsicSemantic::FShl | IntrinsicSemantic::FShr => {
            let Some(shift) = argument_constant(arg(2)) else {
                return Ok(KnownBits::unknown(width));
            };
            if width == 0 {
                return Ok(KnownBits::unknown(width));
            }
            let raw_shift = shift.try_zext_u64().unwrap_or(0);
            let shift = u32::try_from(raw_shift % u64::from(width)).unwrap_or(0);
            let left_shift = if semantic == IntrinsicSemantic::FShr {
                width - shift
            } else {
                shift
            };
            let right_shift = width - left_shift;
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            let left_shift_bits =
                KnownBits::make_constant(ApInt::from_words(width, &[u64::from(left_shift)]));
            let right_shift_bits =
                KnownBits::make_constant(ApInt::from_words(width, &[u64::from(right_shift)]));
            Ok(KnownBits::bitor(
                &KnownBits::shl(&lhs, &left_shift_bits),
                &KnownBits::lshr(&rhs, &right_shift_bits),
            ))
        }
        IntrinsicSemantic::UAddSat => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::uadd_sat(&lhs, &rhs))
        }
        IntrinsicSemantic::USubSat => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::usub_sat(&lhs, &rhs))
        }
        IntrinsicSemantic::SAddSat => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::sadd_sat(&lhs, &rhs))
        }
        IntrinsicSemantic::SSubSat => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::ssub_sat(&lhs, &rhs))
        }
        IntrinsicSemantic::UMin => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::umin(&lhs, &rhs))
        }
        IntrinsicSemantic::UMax => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::umax(&lhs, &rhs))
        }
        IntrinsicSemantic::SMin => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::smin(&lhs, &rhs))
        }
        IntrinsicSemantic::SMax => {
            let lhs = arg_bits(0, stack)?;
            let rhs = arg_bits(1, stack)?;
            Ok(KnownBits::smax(&lhs, &rhs))
        }
        IntrinsicSemantic::VectorReduceAdd => {
            let Some(vector) = arg(0) else {
                return Ok(KnownBits::unknown(width));
            };
            let TypeKind::FixedVector = vector.ty().kind() else {
                return Ok(KnownBits::unknown(width));
            };
            let Some((_, lanes, _)) = vector.ty().data().as_vector() else {
                return Ok(KnownBits::unknown(width));
            };
            Ok(compute_known_bits_inner(vector, query, depth + 1, stack)?.reduce_add(lanes))
        }
        IntrinsicSemantic::PtrMask => {
            let ptr = arg_bits(0, stack)?;
            let mask = arg_bits(1, stack)?.anyext_or_trunc(width);
            Ok(KnownBits::bitand(&ptr, &mask))
        }
        _ => Ok(KnownBits::unknown(width)),
    }
}

fn argument_constant<'ctx, B: ModuleBrand + 'ctx>(value: Option<Value<'ctx, B>>) -> Option<ApInt> {
    let value = value?;
    let width = value_bit_width(value, &value.module().data_layout()).unwrap_or(0);
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(words)) => Some(ApInt::from_words(width, words)),
        _ => None,
    }
}

fn argument_is_const_one<'ctx, B: ModuleBrand + 'ctx>(value: Option<Value<'ctx, B>>) -> bool {
    argument_constant(value).is_some_and(|constant| constant.is_one())
}

fn bit_width_u32(value: u32) -> u32 {
    if value == 0 {
        0
    } else {
        u32::BITS - value.leading_zeros()
    }
}

fn scalar_type_id(module: &ModuleCore, ty: TypeId) -> TypeId {
    match module.context().type_data(ty) {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => *elem,
        _ => ty,
    }
}

fn binary_operand_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<(KnownBits, KnownBits)> {
    let lhs = compute_known_bits_inner(
        value_from_id(anchor, data.lhs.get()),
        query,
        depth + 1,
        stack,
    )?;
    let rhs = compute_known_bits_inner(
        value_from_id(anchor, data.rhs.get()),
        query,
        depth + 1,
        stack,
    )?;
    Ok((lhs, rhs))
}

fn binary_known<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
    f: fn(&KnownBits, &KnownBits) -> KnownBits,
) -> IrResult<KnownBits> {
    let (lhs, rhs) = binary_operand_known_bits(anchor, data, query, depth, stack)?;
    Ok(f(&lhs, &rhs))
}

fn mul_known<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let lhs_value = value_from_id(anchor, data.lhs.get());
    let rhs_value = value_from_id(anchor, data.rhs.get());
    let lhs = compute_known_bits_inner(lhs_value, query, depth + 1, stack)?;
    let rhs = compute_known_bits_inner(rhs_value, query, depth + 1, stack)?;
    let mut known = KnownBits::mul(&lhs, &rhs);
    if query.uses_instruction_info() && data.no_signed_wrap {
        let mut is_known_non_negative = data.lhs.get() == data.rhs.get();
        let mut is_known_negative = false;
        if !is_known_non_negative {
            is_known_non_negative = (lhs.is_negative() && rhs.is_negative())
                || (lhs.is_non_negative() && rhs.is_non_negative());
            if !is_known_non_negative && data.no_unsigned_wrap {
                let one = KnownBits::make_constant(ApInt::from_words(lhs.bit_width(), &[1]));
                is_known_non_negative = KnownBits::sgt(&lhs, &one).unwrap_or(false)
                    || KnownBits::sgt(&rhs, &one).unwrap_or(false);
            }
            if !is_known_non_negative {
                is_known_negative =
                    (lhs.is_negative() && rhs.is_non_negative() && rhs.is_non_zero())
                        || (rhs.is_negative() && lhs.is_non_negative() && lhs.is_non_zero());
            }
        }
        if is_known_non_negative && !known.is_negative() {
            known.make_non_negative();
        } else if is_known_negative && !known.is_non_negative() {
            known.make_negative();
        }
    }
    Ok(known)
}

fn bitwise_known<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
    opcode: BinaryOpcode,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let (lhs, rhs) = binary_operand_known_bits(anchor, data, query, depth, stack)?;
    let mut known = match opcode {
        BinaryOpcode::And => KnownBits::bitand(&lhs, &rhs),
        BinaryOpcode::Or => KnownBits::bitor(&lhs, &rhs),
        BinaryOpcode::Xor => KnownBits::bitxor(&lhs, &rhs),
        _ => KnownBits::unknown(lhs.bit_width()),
    };
    if !known.is_known_zero(0)
        && !known.is_known_one(0)
        && let Some(odd) = bitwise_self_plus_odd_operand(anchor, data)
    {
        let odd_bits = compute_known_bits_inner(odd, query, depth + 1, stack)?;
        if odd_bits.count_min_trailing_ones() > 0 {
            if opcode == BinaryOpcode::And {
                known.set_known_zero_bit(0);
            } else {
                known.set_known_one_bit(0);
            }
        }
    }
    Ok(known)
}

fn bitwise_self_plus_odd_operand<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
) -> Option<Value<'ctx, B>> {
    let lhs_id = data.lhs.get();
    let rhs_id = data.rhs.get();
    self_plus_odd_operand(anchor, lhs_id, rhs_id)
        .or_else(|| self_plus_odd_operand(anchor, rhs_id, lhs_id))
}

fn self_plus_odd_operand<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    base_id: ValueId,
    expr_id: ValueId,
) -> Option<Value<'ctx, B>> {
    let expr = value_from_id(anchor, expr_id);
    let ValueKindData::Instruction(inst) = &expr.data().kind else {
        return None;
    };
    match &inst.kind {
        InstructionKindData::Add(data) => odd_operand_from_commutative(base_id, data, anchor),
        InstructionKindData::Sub(data) => {
            if data.lhs.get() == base_id {
                Some(value_from_id(anchor, data.rhs.get()))
            } else if data.rhs.get() == base_id {
                Some(value_from_id(anchor, data.lhs.get()))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn odd_operand_from_commutative<'ctx, B: ModuleBrand + 'ctx>(
    base_id: ValueId,
    data: &BinaryOpData,
    anchor: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    if data.lhs.get() == base_id {
        Some(value_from_id(anchor, data.rhs.get()))
    } else if data.rhs.get() == base_id {
        Some(value_from_id(anchor, data.lhs.get()))
    } else {
        None
    }
}

fn cast_known<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &CastOpData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(anchor, query.data_layout()).unwrap_or(0);
    let src = value_from_id(anchor, data.src.get());
    let src_bits = compute_known_bits_inner(src, query, depth + 1, stack)?;
    Ok(match data.kind {
        CastOpcode::Trunc => src_bits.trunc(width),
        CastOpcode::ZExt => src_bits.zext(width),
        CastOpcode::SExt => src_bits.sext(width),
        CastOpcode::BitCast
        | CastOpcode::PtrToAddr
        | CastOpcode::PtrToInt
        | CastOpcode::IntToPtr => src_bits.zext_or_trunc(width),
        CastOpcode::AddrSpaceCast => KnownBits::unknown(width),
        CastOpcode::FpTrunc
        | CastOpcode::FpExt
        | CastOpcode::FpToUI
        | CastOpcode::FpToSI
        | CastOpcode::UIToFp
        | CastOpcode::SIToFp => KnownBits::unknown(width),
    })
}

fn icmp_known<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &CmpInstData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let lhs = compute_known_bits_inner(
        value_from_id(anchor, data.lhs.get()),
        query,
        depth + 1,
        stack,
    )?;
    let rhs = compute_known_bits_inner(
        value_from_id(anchor, data.rhs.get()),
        query,
        depth + 1,
        stack,
    )?;
    let result = match (lhs.constant(), rhs.constant()) {
        (Some(left), Some(right)) => Some(evaluate_icmp(data.predicate, &left, &right)),
        _ => known_icmp_from_bits(data.predicate, &lhs, &rhs),
    };
    Ok(match result {
        Some(true) => KnownBits::from_ap_int(ApInt::from_words(1, &[1])),
        Some(false) => KnownBits::from_ap_int(ApInt::zero(1)),
        None => KnownBits::unknown(1),
    })
}

fn known_icmp_from_bits(predicate: IntPredicate, lhs: &KnownBits, rhs: &KnownBits) -> Option<bool> {
    match predicate {
        IntPredicate::Eq => {
            if lhs.one_mask().intersects(rhs.zero_mask())
                || rhs.one_mask().intersects(lhs.zero_mask())
            {
                Some(false)
            } else {
                None
            }
        }
        IntPredicate::Ne => {
            known_icmp_from_bits(IntPredicate::Eq, lhs, rhs).map(core::ops::Not::not)
        }
        IntPredicate::Ugt => {
            if lhs.max_value().ule(&rhs.min_value()) {
                Some(false)
            } else if lhs.min_value().ugt(&rhs.max_value()) {
                Some(true)
            } else {
                None
            }
        }
        IntPredicate::Uge => {
            known_icmp_from_bits(IntPredicate::Ugt, rhs, lhs).map(core::ops::Not::not)
        }
        IntPredicate::Ult => known_icmp_from_bits(IntPredicate::Ugt, rhs, lhs),
        IntPredicate::Ule => known_icmp_from_bits(IntPredicate::Uge, rhs, lhs),
        IntPredicate::Sgt => {
            if lhs.signed_max_value().sle(&rhs.signed_min_value()) {
                Some(false)
            } else if lhs.signed_min_value().sgt(&rhs.signed_max_value()) {
                Some(true)
            } else {
                None
            }
        }
        IntPredicate::Sge => {
            known_icmp_from_bits(IntPredicate::Sgt, rhs, lhs).map(core::ops::Not::not)
        }
        IntPredicate::Slt => known_icmp_from_bits(IntPredicate::Sgt, rhs, lhs),
        IntPredicate::Sle => known_icmp_from_bits(IntPredicate::Sge, rhs, lhs),
    }
}

fn evaluate_icmp(predicate: IntPredicate, lhs: &ApInt, rhs: &ApInt) -> bool {
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

fn alloca_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &AllocaInstData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> KnownBits {
    let width = query.data_layout().pointer_size_in_bits(data.addr_space);
    let module = module_ref(value);
    let allocated_ty = erase_type(Type::new(data.allocated_ty, module));
    let align = data
        .align
        .align()
        .unwrap_or_else(|| query.data_layout().abi_type_align(allocated_ty));
    known_low_zero_bits(width, align)
}

fn known_low_zero_bits(width: u32, align: Align) -> KnownBits {
    KnownBits::from_zero_one(
        ApInt::low_bits_set(width, u32::from(align.log2_value())),
        ApInt::zero(width),
    )
    .unwrap_or_else(|_| KnownBits::unknown(width))
}

fn gep_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &GepInstData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    let ptr = value_from_id(value, data.ptr.get());
    let ptr_known = compute_known_bits_inner(ptr, query, depth + 1, stack)?;
    let indices = data
        .indices
        .iter()
        .map(|index| value_from_id(value, index.get()));
    gep_known_bits_from_values(
        GepKnownBitsInput {
            anchor: value,
            width,
            known: ptr_known,
            source_ty: data.source_ty,
            indices,
        },
        query,
        depth,
        stack,
    )
}

struct GepKnownBitsInput<'ctx, B, I>
where
    B: ModuleBrand + 'ctx,
    I: IntoIterator<Item = Value<'ctx, B>>,
{
    anchor: Value<'ctx, B>,
    width: u32,
    known: KnownBits,
    source_ty: TypeId,
    indices: I,
}

fn gep_known_bits_from_values<'a, 'ctx, B, I>(
    input: GepKnownBitsInput<'ctx, B, I>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits>
where
    B: ModuleBrand + 'ctx,
    I: IntoIterator<Item = Value<'ctx, B>>,
{
    let GepKnownBitsInput {
        anchor,
        width,
        mut known,
        source_ty,
        indices,
    } = input;
    let addr_space = pointer_addr_space(anchor.ty()).unwrap_or(0);
    let index_width = query.data_layout().index_size_in_bits(addr_space);
    let mut offset = ApInt::zero(index_width);
    let mut indexed_ty = source_ty;
    let module = module_ref(anchor);

    for index_value in indices {
        if known.is_unknown() {
            break;
        }
        let ty = Type::new(indexed_ty, module);
        match ty.data() {
            TypeData::Struct(_) => {
                let Some(index_ap) = argument_constant(Some(index_value)) else {
                    return Ok(KnownBits::unknown(width));
                };
                let Some(raw_index) = index_ap.try_zext_u64() else {
                    return Ok(KnownBits::unknown(width));
                };
                let Ok(field_index) = usize::try_from(raw_index) else {
                    return Ok(KnownBits::unknown(width));
                };
                let layout = query.data_layout().struct_layout(erase_type(ty));
                offset = offset.wrapping_add(&ApInt::from_words(
                    index_width,
                    &[layout.element_offset(field_index)],
                ));
                if let Some(field_ty) = struct_field_type_id(ty, field_index) {
                    indexed_ty = field_ty;
                }
            }
            TypeData::Array { elem, .. } | TypeData::FixedVector { elem, .. } => {
                let stride = query
                    .data_layout()
                    .type_alloc_size(erase_type(Type::new(*elem, module)));
                add_gep_index(
                    &mut known,
                    &mut offset,
                    index_value,
                    GepIndexScale {
                        stride,
                        index_width,
                        pointer_width: width,
                    },
                    query,
                    depth,
                    stack,
                )?;
                indexed_ty = *elem;
            }
            TypeData::ScalableVector { elem, .. } => {
                let stride = query
                    .data_layout()
                    .type_alloc_size(erase_type(Type::new(*elem, module)));
                let mut scale = KnownBits::unknown(index_width);
                scale.set_known_zero_bits_from(stride.trailing_zeros());
                add_gep_index_bits(
                    &mut known,
                    &KnownBits::mul(
                        &compute_known_bits_inner(index_value, query, depth + 1, stack)?
                            .sext_or_trunc(index_width),
                        &scale,
                    ),
                    index_width,
                    width,
                );
                indexed_ty = *elem;
            }
            _ => {
                let stride = query.data_layout().type_alloc_size(erase_type(ty));
                add_gep_index(
                    &mut known,
                    &mut offset,
                    index_value,
                    GepIndexScale {
                        stride,
                        index_width,
                        pointer_width: width,
                    },
                    query,
                    depth,
                    stack,
                )?;
            }
        }
    }

    if !known.is_unknown() && !offset.is_zero() {
        add_gep_index_bits(
            &mut known,
            &KnownBits::make_constant(offset),
            index_width,
            width,
        );
    }
    Ok(known)
}

#[derive(Clone, Copy)]
struct GepIndexScale {
    stride: u64,
    index_width: u32,
    pointer_width: u32,
}

fn add_gep_index<'a, 'ctx, B: ModuleBrand + 'ctx>(
    known: &mut KnownBits,
    offset: &mut ApInt,
    index_value: Value<'ctx, B>,
    scale: GepIndexScale,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<()> {
    if let Some(index) = argument_constant(Some(index_value)) {
        let scaled = index
            .sext_or_trunc(scale.index_width)
            .wrapping_mul(&ApInt::from_words(scale.index_width, &[scale.stride]));
        *offset = offset.wrapping_add(&scaled);
        return Ok(());
    }
    let index_bits = compute_known_bits_inner(index_value, query, depth + 1, stack)?
        .sext_or_trunc(scale.index_width);
    let scaled = KnownBits::mul(
        &index_bits,
        &KnownBits::make_constant(ApInt::from_words(scale.index_width, &[scale.stride])),
    );
    add_gep_index_bits(known, &scaled, scale.index_width, scale.pointer_width);
    Ok(())
}

fn add_gep_index_bits(
    known: &mut KnownBits,
    index_bits: &KnownBits,
    index_width: u32,
    pointer_width: u32,
) {
    if index_width == pointer_width {
        *known = KnownBits::add(known, index_bits);
    } else if index_width < pointer_width {
        let low = KnownBits::add(&known.trunc(index_width), index_bits);
        known.insert_bits(&low, 0);
    } else {
        *known = KnownBits::unknown(pointer_width);
    }
}

fn pointer_addr_space<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<u32> {
    match ty.data() {
        TypeData::Pointer { addr_space } | TypeData::TypedPointer { addr_space, .. } => {
            Some(*addr_space)
        }
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            pointer_addr_space(Type::new(*elem, module_ref_from_type(ty)))
        }
        _ => None,
    }
}

fn struct_field_type_id<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    field_index: usize,
) -> Option<TypeId> {
    let TypeData::Struct(data) = ty.data() else {
        return None;
    };
    data.body
        .borrow()
        .as_ref()
        .and_then(|body| body.elements.get(field_index).copied())
}

fn extract_element_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &ExtractElementInstData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let vector = value_from_id(value, data.vector.get());
    let Some((lanes, false)) = vector_shape(vector) else {
        return Ok(KnownBits::unknown(
            value_bit_width(value, query.data_layout()).unwrap_or(0),
        ));
    };
    let index = value_from_id(value, data.index.get());
    let demanded = argument_constant(Some(index))
        .and_then(|idx| idx.try_zext_u64())
        .and_then(|idx| u32::try_from(idx).ok())
        .filter(|idx| *idx < lanes)
        .map_or_else(
            || ApInt::all_ones(lanes),
            |idx| ApInt::one_bit_set(lanes, idx),
        );
    compute_known_bits_for_demanded(vector, &demanded, query, depth + 1, stack)
}

fn insert_element_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &InsertElementInstData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let Some((lanes, false)) = vector_shape(value) else {
        return Ok(KnownBits::unknown(
            value_bit_width(value, query.data_layout()).unwrap_or(0),
        ));
    };
    let demanded = demanded_elements_for(value, query).unwrap_or_else(|| ApInt::all_ones(lanes));
    let mut demanded_vec = demanded.clone();
    let index = value_from_id(value, data.index.get());
    let mut needs_element = true;
    if let Some(idx) = argument_constant(Some(index))
        .and_then(|idx| idx.try_zext_u64())
        .and_then(|idx| u32::try_from(idx).ok())
        .filter(|idx| *idx < lanes)
    {
        demanded_vec.clear_bit(idx);
        needs_element = demanded.is_one_bit_set(idx);
    }
    let mut known = KnownBits::unknown(value_bit_width(value, query.data_layout()).unwrap_or(0));
    known.set_all_conflict();
    if needs_element {
        known = compute_known_bits_inner(
            value_from_id(value, data.value.get()),
            query,
            depth + 1,
            stack,
        )?;
        if known.is_unknown() {
            return Ok(known);
        }
    }
    if !demanded_vec.is_zero() {
        let vec_known = compute_known_bits_for_demanded(
            value_from_id(value, data.vector.get()),
            &demanded_vec,
            query,
            depth + 1,
            stack,
        )?;
        known = known.intersect_with(&vec_known);
    }
    if known.has_conflict() {
        Ok(KnownBits::unknown(
            value_bit_width(value, query.data_layout()).unwrap_or(0),
        ))
    } else {
        Ok(known)
    }
}

fn shuffle_vector_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &ShuffleVectorInstData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let Ok(result_lanes) = u32::try_from(data.mask.len()) else {
        return Ok(KnownBits::unknown(
            value_bit_width(value, query.data_layout()).unwrap_or(0),
        ));
    };
    let demanded =
        demanded_elements_for(value, query).unwrap_or_else(|| ApInt::all_ones(result_lanes));
    let lhs = value_from_id(value, data.lhs.get());
    let rhs = value_from_id(value, data.rhs.get());
    let Some((lhs_lanes, false)) = vector_shape(lhs) else {
        return Ok(KnownBits::unknown(
            value_bit_width(value, query.data_layout()).unwrap_or(0),
        ));
    };
    let Some((rhs_lanes, false)) = vector_shape(rhs) else {
        return Ok(KnownBits::unknown(
            value_bit_width(value, query.data_layout()).unwrap_or(0),
        ));
    };
    let mut lhs_demand = ApInt::zero(lhs_lanes);
    let mut rhs_demand = ApInt::zero(rhs_lanes);
    for (lane, mask) in data.mask.iter().enumerate() {
        let Ok(lane) = u32::try_from(lane) else {
            return Ok(KnownBits::unknown(
                value_bit_width(value, query.data_layout()).unwrap_or(0),
            ));
        };
        if !demanded.is_one_bit_set(lane) {
            continue;
        }
        if *mask == POISON_MASK_ELEM {
            return Ok(KnownBits::unknown(
                value_bit_width(value, query.data_layout()).unwrap_or(0),
            ));
        }
        let Ok(mask) = u32::try_from(*mask) else {
            return Ok(KnownBits::unknown(
                value_bit_width(value, query.data_layout()).unwrap_or(0),
            ));
        };
        if mask < lhs_lanes {
            lhs_demand.set_bit(mask);
        } else {
            let rhs_lane = mask.saturating_sub(lhs_lanes);
            if rhs_lane >= rhs_lanes {
                return Ok(KnownBits::unknown(
                    value_bit_width(value, query.data_layout()).unwrap_or(0),
                ));
            }
            rhs_demand.set_bit(rhs_lane);
        }
    }
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    let mut known = KnownBits::unknown(width);
    known.set_all_conflict();
    if !lhs_demand.is_zero() {
        known = compute_known_bits_for_demanded(lhs, &lhs_demand, query, depth + 1, stack)?;
        if known.is_unknown() {
            return Ok(known);
        }
    }
    if !rhs_demand.is_zero() {
        let rhs_known = compute_known_bits_for_demanded(rhs, &rhs_demand, query, depth + 1, stack)?;
        known = known.intersect_with(&rhs_known);
    }
    if known.has_conflict() {
        Ok(KnownBits::unknown(width))
    } else {
        Ok(known)
    }
}

fn aggregate_constant_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    elements: &[ValueId],
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    let Some((lanes, false)) = vector_shape(value) else {
        return Ok(KnownBits::unknown(width));
    };
    let demanded = demanded_elements_for(value, query).unwrap_or_else(|| ApInt::all_ones(lanes));
    let mut known = KnownBits::unknown(width);
    known.set_all_conflict();
    for (lane, element) in elements.iter().enumerate() {
        let Ok(lane) = u32::try_from(lane) else {
            return Ok(KnownBits::unknown(width));
        };
        if !demanded.is_one_bit_set(lane) {
            continue;
        }
        let element_known =
            compute_known_bits_inner(value_from_id(value, *element), query, depth + 1, stack)?;
        known = known.intersect_with(&element_known);
        if known.is_unknown() {
            break;
        }
    }
    if known.has_conflict() {
        Ok(KnownBits::unknown(width))
    } else {
        Ok(known)
    }
}

fn compute_known_bits_for_demanded<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    demanded: &ApInt,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<KnownBits> {
    let subquery = query.with_temporary_demanded_elements(demanded);
    compute_known_bits_inner(value, &subquery, depth, stack)
}

fn demanded_elements_for<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> Option<ApInt> {
    let (lanes, scalable) = vector_shape(value)?;
    if scalable {
        return None;
    }
    Some(
        query
            .demanded_elements()
            .filter(|demanded| demanded.bit_width() == lanes)
            .cloned()
            .unwrap_or_else(|| ApInt::all_ones(lanes)),
    )
}

fn vector_shape<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<(u32, bool)> {
    value
        .ty()
        .data()
        .as_vector()
        .map(|(_, lanes, scalable)| (lanes, scalable))
}

fn value_bit_width<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    dl: &DataLayout,
) -> Option<u32> {
    type_bit_width(value.ty(), dl)
}

fn is_guaranteed_not_to_be_poison<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueId>,
) -> IrResult<bool> {
    if depth > query.max_depth() {
        return Ok(false);
    }
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Poison) => Ok(false),
        ValueKindData::Constant(_) => Ok(true),
        ValueKindData::Instruction(inst) => match &inst.kind {
            InstructionKindData::Shl(data)
            | InstructionKindData::LShr(data)
            | InstructionKindData::AShr(data) => {
                if query.uses_instruction_info()
                    && (data.no_unsigned_wrap || data.no_signed_wrap || data.is_exact)
                {
                    return Ok(false);
                }
                let lhs = value_from_id(value, data.lhs.get());
                let rhs = value_from_id(value, data.rhs.get());
                if !is_guaranteed_not_to_be_poison(lhs, query, depth + 1, stack)?
                    || !is_guaranteed_not_to_be_poison(rhs, query, depth + 1, stack)?
                {
                    return Ok(false);
                }
                let Some(width) = value_bit_width(lhs, query.data_layout()) else {
                    return Ok(false);
                };
                let rhs_bits = compute_known_bits_inner(rhs, query, depth + 1, stack)?;
                Ok(rhs_bits.max_value().limited_value(u64::from(width)) < u64::from(width))
            }
            _ => Ok(false),
        },
        ValueKindData::Argument { .. }
        | ValueKindData::BasicBlock(_)
        | ValueKindData::Function(_)
        | ValueKindData::GlobalAlias(_)
        | ValueKindData::GlobalIFunc(_)
        | ValueKindData::GlobalVariable(_)
        | ValueKindData::MetadataAsValue(_)
        | ValueKindData::InlineAsm(_) => Ok(false),
    }
}

fn type_bit_width<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>, dl: &DataLayout) -> Option<u32> {
    match ty.kind() {
        TypeKind::Integer { bits } => Some(bits),
        TypeKind::Pointer { addr_space } => Some(dl.pointer_size_in_bits(addr_space)),
        TypeKind::FixedVector | TypeKind::ScalableVector => {
            let (elem, _, _) = ty.data().as_vector()?;
            type_bit_width(Type::new(elem, module_ref_from_type(ty)), dl)
        }
        TypeKind::TypedPointer => {
            let (_, addr_space) = ty.data().as_typed_pointer()?;
            Some(dl.pointer_size_in_bits(addr_space))
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
        | TypeKind::WasmExnRef
        | TypeKind::Label
        | TypeKind::Metadata
        | TypeKind::Token
        | TypeKind::Function
        | TypeKind::Array
        | TypeKind::Struct
        | TypeKind::TargetExt => None,
    }
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

fn erase_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Type<'ctx> {
    Type::new(ty.id(), ModuleRef::new(ty.module().core_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruction::build_instruction_value;
    use crate::module::Module;

    fn fabricate_instruction(
        m: &Module<'_>,
        bb_id: ValueId,
        result_ty: TypeId,
        kind: InstructionKindData,
    ) -> ValueId {
        let core = m.core_ref();
        let value = build_instruction_value(result_ty, bb_id, kind, None);
        let id = core.context().push_value(value);
        let ValueKindData::BasicBlock(bb_data) = &core.context().value_data(bb_id).kind else {
            panic!("fabricate_instruction: bb_id is not a basic block");
        };
        bb_data.instructions.borrow_mut().push(id);
        id
    }

    fn fabricated_value<'ctx>(m: &Module<'ctx>, id: ValueId, ty: TypeId) -> Value<'ctx> {
        Value::from_parts(id, ModuleRef::new(m.core_ref()), ty)
    }

    /// Mirrors `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
    /// `getelementptr` handling: LLVM queries `DataLayout::getIndexTypeSizeInBits`
    /// with the GEP result type, whose pointer-vector element address space
    /// selects the index width.
    #[test]
    fn vector_gep_uses_element_pointer_address_space_for_index_width() -> crate::IrResult<()> {
        Module::with_new("vt-vector-gep-as", |m| {
            m.set_data_layout("p1:64:64:64:32")?;
            let i8_ty = m.i8_type();
            let i32_ty = m.i32_type();
            let ptr1_ty = m.ptr_type(1);
            let ptr_vec_ty = m.vector_type(ptr1_ty.as_type(), 2, false);
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, crate::Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");

            let base = ptr_vec_ty.const_vector([ptr1_ty.const_null(); 2])?;
            let minus_one = i32_ty.const_int(-1_i32);
            let gep_ty = ptr_vec_ty.as_type();
            let gep_id = fabricate_instruction(
                &m,
                entry.as_value().id,
                gep_ty.id(),
                InstructionKindData::Gep(GepInstData::new(
                    i8_ty.as_type().id(),
                    base.as_value().id,
                    [minus_one.as_value().id],
                    crate::GepNoWrapFlags::empty(),
                )),
            );
            let gep = fabricated_value(&m, gep_id, gep_ty.id());
            let dl = m.data_layout();
            let query = ValueTrackingQuery::new(&dl);

            assert_eq!(
                compute_known_bits(gep, &query)?.to_string(),
                "0000000000000000000000000000000011111111111111111111111111111111"
            );
            Ok(())
        })
    }
}
