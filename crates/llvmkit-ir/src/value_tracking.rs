//! Integer/pointer known-bits queries over IR values.
//!
//! Mirrors the `computeKnownBits` slice of `llvm/lib/Analysis/ValueTracking.cpp`.

use crate::align::Align;
use crate::analysis::{
    AllAnalysesOnFunction, CFGAnalyses, FunctionAnalysis, FunctionAnalysisInvalidator,
    FunctionAnalysisManager, FunctionAnalysisResult, PrefetchableAnalysis, PreservedAnalyses,
};
use crate::assumptions::{
    AssumptionCache, AssumptionSource, DomConditionCache, find_values_affected_by_condition,
    is_valid_assume_for_context,
};
use crate::attributes::{AttrIndex, AttrKind, AttributeStorage, AttributeStored};
use crate::cmp_predicate::IntPredicate;
use crate::constant::{ConstantData, ConstantExprData, ConstantExprOpcode};
use crate::constant_range::{
    ConstantRange, OverflowResult, PreferredRangeType, constant_ranges_from_metadata,
};
use crate::data_layout::DataLayout;
use crate::dominator_tree::{DominatorTree, DominatorTreeAnalysis};
use crate::instr_types::{
    AllocaInstData, BinaryOpData, BinaryOpcode, BranchKind, CastOpData, CastOpcode, CmpInstData,
    ExtractElementInstData, GepInstData, InsertElementInstData, POISON_MASK_ELEM, PhiData,
    ShuffleVectorInstData,
};
use crate::instruction::{InstructionData, InstructionKindData, InstructionView};
use crate::intrinsics::{IntrinsicSemantic, semantic_for_callee};
use crate::metadata::MetadataAttachmentKind;
use crate::module::{DynBrand, ModuleBrand, ModuleCore, ModuleRef};
use crate::pass_context::FunctionView;
use crate::pointer_analysis::strip_pointer_casts_same_representation;
use crate::speculation::program_undefined_for_value;
use crate::r#type::{Type, TypeData, TypeKind, TypeSlot};
use crate::value::{Value, ValueKindData, ValueSlot};
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
    value: ValueSlot,
    context_instruction: Option<ValueSlot>,
    demanded_elements: Option<ApInt>,
    uses_instruction_info: bool,
}

impl KnownBitsCacheKey {
    #[inline]
    fn new<'a, 'ctx, B: ModuleBrand>(
        value: ValueSlot,
        query: &ValueTrackingQuery<'a, 'ctx, B>,
    ) -> Self {
        Self {
            value,
            context_instruction: query
                .context_instruction
                .map(|instruction| instruction.slot()),
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

    fn run<'v>(
        &self,
        function: FunctionView<'v, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>
    where
        'ctx: 'v,
    {
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
    fn invalidate<'v>(
        &mut self,
        _function: FunctionView<'v, B>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
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
pub struct ValueTrackingQuery<'a, 'ctx, B: ModuleBrand> {
    data_layout: &'a DataLayout,
    max_depth: u32,
    dominator_tree: Option<&'a DominatorTree>,
    context_instruction: Option<Value<'ctx, B>>,
    demanded_elements: Option<&'a ApInt>,
    use_instr_info: bool,
    assumptions: Option<&'a AssumptionCache>,
    dominating_conditions: Option<&'a DomConditionCache>,
    condition_context: Option<&'a CondContext<'ctx, B>>,
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
            assumptions: None,
            dominating_conditions: None,
            condition_context: None,
            cache: QueryCache::owned(),
            _brand: PhantomData,
        }
    }

    /// Let `@llvm.assume` calls refine the answers. Ports `SimplifyQuery::AC`.
    ///
    /// Only consulted together with a context instruction — an assumption is a
    /// fact at a *place*, and without one there is nowhere to check validity.
    #[inline]
    pub fn with_assumptions(mut self, assumptions: &'a AssumptionCache) -> Self {
        self.assumptions = Some(assumptions);
        self
    }

    /// Let dominating branch conditions refine the answers. Ports
    /// `SimplifyQuery::DC`.
    ///
    /// Only consulted together with a context instruction and a dominator tree.
    #[inline]
    pub fn with_dominating_conditions(mut self, conditions: &'a DomConditionCache) -> Self {
        self.dominating_conditions = Some(conditions);
        self
    }

    /// Assume `context`'s condition holds for the duration of the query. Ports
    /// `SimplifyQuery::CC`.
    #[inline]
    pub fn with_condition_context(mut self, context: &'a CondContext<'ctx, B>) -> Self {
        self.condition_context = Some(context);
        self
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
        self.context_instruction = Some(instruction.to_erased());
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
            assumptions: self.assumptions,
            dominating_conditions: self.dominating_conditions,
            condition_context: self.condition_context,
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

    /// The assumption cache, if one was attached.
    #[inline]
    pub fn assumptions(&self) -> Option<&AssumptionCache> {
        self.assumptions
    }

    /// The dominating-condition cache, if one was attached.
    #[inline]
    pub fn dominating_conditions(&self) -> Option<&DomConditionCache> {
        self.dominating_conditions
    }

    /// The injected condition, if one was attached.
    #[inline]
    pub fn condition_context(&self) -> Option<&CondContext<'ctx, B>> {
        self.condition_context
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
///
/// Upstream's `llvm::isKnownNonZero` is a dedicated walk —
/// `isKnownNonZeroFromOperator`, a dominating-condition check, then the
/// `stripNullTest` tail. llvmkit answers the first two through
/// [`compute_known_bits`], which proves less on the shapes that walk special-
/// cases but never proves something false; only the last is spelled out here,
/// because no amount of known-bits reasoning reaches it.
pub fn is_known_non_zero<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    if compute_known_bits(value, query)?.is_non_zero() {
        return Ok(true);
    }
    // `f(X)` is zero exactly when `X` is, so the question transfers whole.
    // Upstream recurses at the same depth, which terminates because
    // `stripNullTest` only ever answers with an operand of an operand.
    match strip_null_test(value) {
        Some(stripped) => is_known_non_zero(stripped, query),
        None => Ok(false),
    }
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
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    if depth > query.max_depth() {
        return Ok(KnownBits::unknown(width));
    }
    if stack.contains(&value.slot()) {
        return Ok(KnownBits::unknown(width));
    }
    let cache_key = KnownBitsCacheKey::new(value.slot(), query);
    if let Some(cached) = query.cache().borrow().get(&cache_key).cloned() {
        return Ok(cached);
    }

    stack.insert(value.slot());
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
    stack.remove(&value.slot());

    // `computeKnownBitsFromContext` strictly refines what the operator walk
    // found, so upstream runs it after; the same order is kept here.
    let known = known_bits_from_context(value, known, query, depth);

    query.cache().borrow_mut().insert(cache_key, known.clone());
    Ok(known)
}

fn compute_constant_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    constant: &ConstantData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
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
        InstructionKindData::Phi(data) => phi_known_bits(value, data, query, depth, stack),
        InstructionKindData::Freeze(data) => {
            let src = value_from_id(value, data.src.get());
            if is_guaranteed_not_to_be_undef_or_poison(
                src,
                query,
                depth + 1,
                stack,
                UndefPoisonKind::PoisonOnly,
            )? {
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
    let Some(ranges) = constant_ranges_from_metadata(module, &store, range_id.slot(), expected_ty)
    else {
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

fn ranges_known_bits<I>(ranges: I, bit_width: u32) -> KnownBits
where
    I: IntoIterator<Item = ConstantRange>,
{
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
    callee_id: ValueSlot,
    args: &'a [Cell<ValueSlot>],
    return_attrs: &'a AttributeStorage,
    arg_attrs: &'a [AttributeStorage],
}

fn call_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    inputs: CallKnownBitsInputs<'_>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
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
    args: &[Cell<ValueSlot>],
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
    callee_id: ValueSlot,
) -> Option<IntrinsicSemantic> {
    semantic_for_callee(value_from_id(anchor, callee_id))
}

fn intrinsic_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    semantic: IntrinsicSemantic,
    args: &[Cell<ValueSlot>],
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(anchor, query.data_layout()).unwrap_or(0);
    let arg = |idx: usize| args.get(idx).map(|cell| value_from_id(anchor, cell.get()));
    let arg_bits = |idx: usize, stack: &mut HashSet<ValueSlot>| -> IrResult<KnownBits> {
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

fn scalar_type_id(module: &ModuleCore, ty: TypeSlot) -> TypeSlot {
    match module.context().type_data(ty) {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => *elem,
        _ => ty,
    }
}

/// The known bits of an `and` / `or` / `xor` given both operands' known bits.
///
/// Ports `llvm::analyzeKnownBitsFromAndXorOr`, which upstream exposes so
/// `SimplifyDemandedUseBits` can reuse the reasoning with operand bits it has
/// already narrowed rather than recomputing them.
///
/// `None` when `operation` is not one of the three — upstream reaches an
/// `llvm_unreachable` there, which is a caller precondition rather than a
/// reachable state.
///
/// Upstream's wrapper pins demanded elements to all lanes rather than
/// inheriting the caller's, and that is reproduced: only the odd-operand
/// refinement reads them, and it recurses on an operand of the whole
/// operation, not on a lane of it.
pub fn analyze_known_bits_from_and_xor_or<'a, 'ctx, B: ModuleBrand + 'ctx>(
    operation: Value<'ctx, B>,
    known_lhs: &KnownBits,
    known_rhs: &KnownBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<Option<KnownBits>> {
    let (data, opcode) = match instruction_kind(operation) {
        Some(InstructionKindData::And(data)) => (data, BinaryOpcode::And),
        Some(InstructionKindData::Or(data)) => (data, BinaryOpcode::Or),
        Some(InstructionKindData::Xor(data)) => (data, BinaryOpcode::Xor),
        _ => return Ok(None),
    };

    let lanes = vector_shape(operation).filter(|(_, scalable)| !scalable);
    let demanded = ApInt::all_ones(lanes.map_or(1, |(lanes, _)| lanes));
    let query = query.with_temporary_demanded_elements(&demanded);

    let mut stack = HashSet::new();
    and_xor_or_known(
        operation,
        data,
        opcode,
        OperandKnownBits {
            lhs: known_lhs,
            rhs: known_rhs,
        },
        &query,
        depth,
        &mut stack,
    )
    .map(Some)
}

fn binary_operand_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<KnownBits> {
    let (lhs, rhs) = binary_operand_known_bits(anchor, data, query, depth, stack)?;
    and_xor_or_known(
        anchor,
        data,
        opcode,
        OperandKnownBits {
            lhs: &lhs,
            rhs: &rhs,
        },
        query,
        depth,
        stack,
    )
}

/// The two operands' known bits, as `getKnownBitsFromAndXorOr` takes them.
struct OperandKnownBits<'k> {
    lhs: &'k KnownBits,
    rhs: &'k KnownBits,
}

/// Ports `getKnownBitsFromAndXorOr`: the `and` / `or` / `xor` answer given both
/// operands' known bits, plus the two idiom arms and the odd-operand
/// refinement that sharpen it.
fn and_xor_or_known<'a, 'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
    opcode: BinaryOpcode,
    operands: OperandKnownBits<'_>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<KnownBits> {
    let OperandKnownBits { lhs, rhs } = operands;
    // Both idiom arms below need a bit already known set somewhere, since what
    // they do is clear everything *above* the lowest one.
    let has_known_one = !lhs.one_mask().is_zero() || !rhs.one_mask().is_zero();

    let mut known = match opcode {
        BinaryOpcode::And => {
            let mut known = KnownBits::bitand(lhs, rhs);
            // `and(x, -x)` clears all but the lowest set bit. Upstream's
            // comment: `-(-x) == x`, so take whichever side gives the better
            // answer. Its `TODO` about InstCombine reassociating the `and` and
            // hiding the pattern is inherited.
            if has_known_one && is_negation_pair(anchor, data) {
                known = if lhs.count_max_trailing_zeros() <= rhs.count_max_trailing_zeros() {
                    lhs.blsi()
                } else {
                    rhs.blsi()
                };
            }
            known
        }
        BinaryOpcode::Or => KnownBits::bitor(lhs, rhs),
        BinaryOpcode::Xor => {
            let mut known = KnownBits::bitxor(lhs, rhs);
            // `xor(x, x - 1)` likewise. Upstream's `TODO` — that `xor(x, x - C)`
            // agrees with it on the demanded bits for any `C` — is inherited.
            if has_known_one && let Some(base) = xor_with_self_minus_one(anchor, data) {
                // The answer is about `x`, so pick the side that *is* `x`.
                known = if data.lhs.get() == base {
                    lhs.blsmsk()
                } else {
                    rhs.blsmsk()
                };
            }
            known
        }
        // Upstream's `llvm_unreachable("Invalid Op used in
        // 'analyzeKnownBitsFromAndXorOr'")`. Callers inside this module dispatch
        // on the opcode first; the public entry point rejects the others.
        _ => KnownBits::unknown(lhs.bit_width()),
    };

    // `and(x, add(x, -1))` always clears the low bit and `xor`/`or` always set
    // it; upstream generalises to `add(x, y)` for any odd `y`.
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

/// Whether the two operands are `x` and `-x` in either order. Ports
/// `m_c_And(m_Value(X), m_Neg(m_Deferred(X)))`.
fn is_negation_pair<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
) -> bool {
    let lhs = value_from_id(anchor, data.lhs.get());
    let rhs = value_from_id(anchor, data.rhs.get());
    is_negation_of_operand(rhs, lhs) || is_negation_of_operand(lhs, rhs)
}

/// The `x` of `xor(x, add(x, -1))`, matched in either order. Ports
/// `m_c_Xor(m_Value(X), m_Add(m_Deferred(X), m_AllOnes()))`.
fn xor_with_self_minus_one<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
) -> Option<ValueSlot> {
    let lhs = data.lhs.get();
    let rhs = data.rhs.get();
    if is_add_of_all_ones(anchor, rhs, lhs) {
        return Some(lhs);
    }
    if is_add_of_all_ones(anchor, lhs, rhs) {
        return Some(rhs);
    }
    None
}

/// Whether `candidate` is `add base, -1`, with the `add` matched commutatively.
fn is_add_of_all_ones<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    candidate: ValueSlot,
    base: ValueSlot,
) -> bool {
    let candidate = value_from_id(anchor, candidate);
    let Some(InstructionKindData::Add(data)) = instruction_kind(candidate) else {
        return false;
    };
    let (lhs, rhs) = (data.lhs.get(), data.rhs.get());
    (lhs == base && is_all_ones_constant(value_from_id(candidate, rhs)))
        || (rhs == base && is_all_ones_constant(value_from_id(candidate, lhs)))
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
    base_id: ValueSlot,
    expr_id: ValueSlot,
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
    base_id: ValueSlot,
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
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
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

/// The binary operator closing a simple two-predecessor recurrence, split into
/// the parts the known-bits arms need.
struct SimpleRecurrence {
    /// The recurrence's binary opcode.
    opcode: BinaryOpcode,
    /// The value entering the phi from outside the loop.
    start: ValueSlot,
    /// The other operand of the binary operator — the step.
    step: ValueSlot,
    /// Whether the phi is the binary operator's *left* operand. Upstream reads
    /// this back as `BO->getOperand(0) != I` to reject the arms where operand
    /// order matters (`shl`/`lshr`/`ashr`/`udiv` and `sub`).
    phi_is_left_operand: bool,
    /// `nsw` on the binary operator, already gated on `UseInstrInfo`.
    no_signed_wrap: bool,
    /// `nuw` on the binary operator, already gated on `UseInstrInfo`.
    no_unsigned_wrap: bool,
    /// `exact` on the binary operator, already gated on `UseInstrInfo`.
    is_exact: bool,
    /// The binary operator itself — upstream's `BinaryOperator *&BO` out-parameter.
    increment: ValueSlot,
}

/// The binary operator's opcode and operands, for any binary opcode.
///
/// Upstream reaches this through `dyn_cast<BinaryOperator>`, which is opcode-
/// blind; the opcode is only inspected afterwards. Matching the same set here
/// keeps the *first* qualifying operand the match, exactly as upstream does —
/// narrowing this to the opcodes the known-bits arms use would let a second
/// incoming value be matched where upstream stops at the first.
fn binary_operator_parts(kind: &InstructionKindData) -> Option<(BinaryOpcode, &BinaryOpData)> {
    Some(match kind {
        InstructionKindData::Add(b) => (BinaryOpcode::Add, b),
        InstructionKindData::Sub(b) => (BinaryOpcode::Sub, b),
        InstructionKindData::Mul(b) => (BinaryOpcode::Mul, b),
        InstructionKindData::UDiv(b) => (BinaryOpcode::UDiv, b),
        InstructionKindData::SDiv(b) => (BinaryOpcode::SDiv, b),
        InstructionKindData::URem(b) => (BinaryOpcode::URem, b),
        InstructionKindData::SRem(b) => (BinaryOpcode::SRem, b),
        InstructionKindData::Shl(b) => (BinaryOpcode::Shl, b),
        InstructionKindData::LShr(b) => (BinaryOpcode::LShr, b),
        InstructionKindData::AShr(b) => (BinaryOpcode::AShr, b),
        InstructionKindData::And(b) => (BinaryOpcode::And, b),
        InstructionKindData::Or(b) => (BinaryOpcode::Or, b),
        InstructionKindData::Xor(b) => (BinaryOpcode::Xor, b),
        InstructionKindData::FAdd(b) => (BinaryOpcode::FAdd, b),
        InstructionKindData::FSub(b) => (BinaryOpcode::FSub, b),
        InstructionKindData::FMul(b) => (BinaryOpcode::FMul, b),
        InstructionKindData::FDiv(b) => (BinaryOpcode::FDiv, b),
        InstructionKindData::FRem(b) => (BinaryOpcode::FRem, b),
        _ => return None,
    })
}

/// Match `%iv = phi [start, %entry], [%iv.next, %backedge]` where `%iv.next`
/// is a binary operator with `%iv` as one operand.
///
/// Ports `matchSimpleRecurrence` / `matchTwoInputRecurrence`
/// (`ValueTracking.cpp`).
fn match_simple_recurrence<'ctx, B: ModuleBrand + 'ctx>(
    phi: Value<'ctx, B>,
    data: &PhiData,
    uses_instruction_info: bool,
) -> Option<SimpleRecurrence> {
    let incoming = data.incoming.borrow();
    if incoming.len() != 2 {
        return None;
    }
    for index in 0..2 {
        let candidate = value_from_id(phi, incoming[index].0.get());
        let ValueKindData::Instruction(inst) = &candidate.data().kind else {
            continue;
        };
        let Some((opcode, operands)) = binary_operator_parts(&inst.kind) else {
            continue;
        };
        let lhs = operands.lhs.get();
        let rhs = operands.rhs.get();
        let phi_slot = phi.slot();
        if lhs != phi_slot && rhs != phi_slot {
            continue;
        }
        let phi_is_left_operand = lhs == phi_slot;
        return Some(SimpleRecurrence {
            opcode,
            start: incoming[1 - index].0.get(),
            step: if phi_is_left_operand { rhs } else { lhs },
            phi_is_left_operand,
            no_signed_wrap: uses_instruction_info && operands.no_signed_wrap,
            no_unsigned_wrap: uses_instruction_info && operands.no_unsigned_wrap,
            is_exact: uses_instruction_info && operands.is_exact,
            increment: incoming[index].0.get(),
        });
    }
    None
}

/// Known bits for a `phi`.
///
/// Ports the `Instruction::PHI` arm of `computeKnownBitsFromOperator`
/// (`ValueTracking.cpp`): first the simple-recurrence facts, then — only if
/// those left the result unknown — the intersection over the incoming values.
///
/// Two pieces of upstream's arm are **not** ported, both because llvmkit does
/// not model what they read. Neither can make an answer wrong; each only
/// leaves it weaker:
///
/// - The per-edge context instruction (`RecQ.CxtI = P->getIncomingBlock(..)`)
///   that lets upstream evaluate an incoming value at the edge it flows in on.
/// - The `m_Br(m_c_ICmp(..))` refinement that narrows an incoming value by the
///   branch condition guarding its edge.
///
/// One piece is deliberately **not** copied. Upstream gates the intersection
/// loop on `Depth < MaxAnalysisRecursionDepth - 1` and then recurses at the
/// fixed depth `MaxAnalysisRecursionDepth - 1`, capping the search under an
/// incoming value at one level so it does not "spin around in loops". llvmkit
/// recurses at `depth + 1` instead, because it already terminates by a
/// different mechanism — the `stack` set rejects re-entering a value that is
/// mid-computation — and because [`compute_known_bits_inner`] memoizes on
/// `(slot, query)` with no depth component. Entering an incoming value at a
/// fixed deep depth would cache the weak answer computed there and hand it to
/// a later shallow query of the same value. The result is that llvmkit can
/// answer *more* precisely than upstream for a shallow phi, never less.
fn phi_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &PhiData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    let mut known = KnownBits::unknown(width);

    if let Some(recurrence) = match_simple_recurrence(value, data, query.uses_instruction_info()) {
        known = recurrence_known_bits(value, &recurrence, query, depth, stack)?;
    }

    // Unreachable blocks may have zero-operand PHI nodes.
    let incoming = data.incoming.borrow();
    if incoming.is_empty() {
        return Ok(known);
    }

    // Otherwise take the intersection of the incoming known-bit sets, taking
    // conservative care to avoid excessive recursion.
    if !known.is_unknown() {
        return Ok(known);
    }
    // `None` until the first non-self incoming is folded in, which is what
    // upstream's `Known.setAllConflict()` seed achieves — conflict is the
    // identity of `intersectWith`. It also covers upstream's
    // `isa_and_nonnull<UndefValue>(P->hasConstantValue())` guard: a phi whose
    // every incoming is a self reference leaves `result` at `None` and answers
    // unknown, which is where that guard's `break` lands too.
    let mut result: Option<KnownBits> = None;
    for (incoming_value, _) in incoming.iter() {
        // Skip direct self references.
        if incoming_value.get() == value.slot() {
            continue;
        }
        let next = compute_known_bits_inner(
            value_from_id(value, incoming_value.get()),
            query,
            depth + 1,
            stack,
        )?;
        result = Some(match result {
            Some(accumulated) => accumulated.intersect_with(&next),
            None => next,
        });
        // If all bits have been ruled out, there is no need to check more
        // operands.
        if result.as_ref().is_some_and(KnownBits::is_unknown) {
            break;
        }
    }
    Ok(result.unwrap_or(known))
}

/// The simple-recurrence half of the `PHI` arm.
fn recurrence_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    phi: Value<'ctx, B>,
    recurrence: &SimpleRecurrence,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<KnownBits> {
    let width = value_bit_width(phi, query.data_layout()).unwrap_or(0);
    let mut known = KnownBits::unknown(width);
    let start = value_from_id(phi, recurrence.start);

    match recurrence.opcode {
        // A shift or udiv recurrence tells us what is shifted in, which
        // combines with the start value to bound the result. For `urem` the
        // result can never exceed the start value, and the phi may be either
        // operand — so unlike the others it does not require the phi on the
        // left.
        BinaryOpcode::Shl
        | BinaryOpcode::LShr
        | BinaryOpcode::AShr
        | BinaryOpcode::UDiv
        | BinaryOpcode::URem => {
            if !recurrence.phi_is_left_operand && recurrence.opcode != BinaryOpcode::URem {
                return Ok(known);
            }
            let start_bits = compute_known_bits_inner(start, query, depth + 1, stack)?;
            match recurrence.opcode {
                // A shl recurrence will only increase the trailing zeros.
                BinaryOpcode::Shl => {
                    known.mark_low_bits_zero(start_bits.count_min_trailing_zeros());
                }
                // lshr, udiv, and urem recurrences preserve the leading zeros
                // of the start value.
                BinaryOpcode::LShr | BinaryOpcode::UDiv | BinaryOpcode::URem => {
                    known.mark_high_bits_zero(start_bits.count_min_leading_zeros());
                }
                // An ashr recurrence extends the initial sign bit.
                BinaryOpcode::AShr => {
                    known.mark_high_bits_zero(start_bits.count_min_leading_zeros());
                    known.mark_high_bits_one(start_bits.count_min_leading_ones());
                }
                other => unreachable!(
                    "the enclosing arm admits only shl/lshr/ashr/udiv/urem, got {other:?}"
                ),
            }
        }

        // Operations where low zero bits in both operands give low zero bits
        // in the result.
        BinaryOpcode::Add
        | BinaryOpcode::Sub
        | BinaryOpcode::And
        | BinaryOpcode::Or
        | BinaryOpcode::Mul => {
            let step = value_from_id(phi, recurrence.step);
            let start_bits = compute_known_bits_inner(start, query, depth + 1, stack)?;
            let step_bits = compute_known_bits_inner(step, query, depth + 1, stack)?;
            known.mark_low_bits_zero(
                start_bits
                    .count_min_trailing_zeros()
                    .min(step_bits.count_min_trailing_zeros()),
            );

            if !recurrence.no_signed_wrap {
                return Ok(known);
            }
            // With nsw, the sign of the start value and the step bound the
            // sign of every iterate: the recurrence can only stay on that side
            // or be poison.
            match recurrence.opcode {
                // (add nsw non-negative, non-negative) --> non-negative
                // (add nsw negative, negative) --> negative
                BinaryOpcode::Add => {
                    if start_bits.is_non_negative() && step_bits.is_non_negative() {
                        known.make_non_negative();
                    } else if start_bits.is_negative() && step_bits.is_negative() {
                        known.make_negative();
                    }
                }
                // (sub nsw non-negative, negative) --> non-negative
                // (sub nsw negative, non-negative) --> negative
                BinaryOpcode::Sub => {
                    if !recurrence.phi_is_left_operand {
                        return Ok(known);
                    }
                    if start_bits.is_non_negative() && step_bits.is_negative() {
                        known.make_non_negative();
                    } else if start_bits.is_negative() && step_bits.is_non_negative() {
                        known.make_negative();
                    }
                }
                // (mul nsw non-negative, non-negative) --> non-negative
                BinaryOpcode::Mul => {
                    if start_bits.is_non_negative() && step_bits.is_non_negative() {
                        known.make_non_negative();
                    }
                }
                // `and` and `or` reach here only if they somehow carry `nsw`;
                // upstream's `dyn_cast<OverflowingBinaryOperator>` rejects them
                // before the switch, and this is where that rejection lands.
                BinaryOpcode::And | BinaryOpcode::Or => {}
                other => {
                    unreachable!("the enclosing arm admits only add/sub/and/or/mul, got {other:?}")
                }
            }
        }

        // Every other binary opcode: upstream's `default: break` — the
        // recurrence contributes nothing.
        BinaryOpcode::SDiv
        | BinaryOpcode::SRem
        | BinaryOpcode::Xor
        | BinaryOpcode::FAdd
        | BinaryOpcode::FSub
        | BinaryOpcode::FMul
        | BinaryOpcode::FDiv
        | BinaryOpcode::FRem => {}
    }
    Ok(known)
}

// --------------------------------------------------------------------------
// Sign-bit counting
// --------------------------------------------------------------------------

/// Number of times the sign bit is replicated into the high bits of `value`.
///
/// At least one bit always equals the sign bit (itself), so the answer is
/// never zero. Ports `llvm::ComputeNumSignBits` (`ValueTracking.cpp`).
pub fn compute_num_sign_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<u32> {
    let mut stack = HashSet::new();
    compute_num_sign_bits_inner(value, query, 0, &mut stack)
}

/// Bits needed to represent `value` as a signed number — its scalar width less
/// the replicated sign bits, plus one for the sign itself. Ports
/// `llvm::ComputeMaxSignificantBits`.
pub fn compute_max_significant_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<u32> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    let sign_bits = compute_num_sign_bits(value, query)?;
    Ok(width.saturating_sub(sign_bits).saturating_add(1))
}

/// Ports `ComputeNumSignBits` — the assert-wrapped caller of
/// `ComputeNumSignBitsImpl`, which upstream uses to hold the "at least one
/// sign bit" invariant. Here the floor is enforced rather than asserted, so a
/// zero can never escape into arithmetic that subtracts from it.
fn compute_num_sign_bits_inner<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<u32> {
    Ok(compute_num_sign_bits_impl(value, query, depth, stack)?.max(1))
}

/// Ports `ComputeNumSignBitsImpl` (`ValueTracking.cpp`).
///
/// The vector arms (`BitCast` across element widths, `ShuffleVector`,
/// `ExtractElement`'s demanded-element tracking) are not ported: they read
/// `getShuffleDemandedElts`, which llvmkit does not model. Each falls through
/// to the `computeKnownBits` tail, which is what upstream's own `break` in
/// those arms does when the pattern does not match — weaker, never wrong.
fn compute_num_sign_bits_impl<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
) -> IrResult<u32> {
    let ty_bits = value_bit_width(value, query.data_layout()).unwrap_or(0);
    if ty_bits == 0 {
        return Ok(1);
    }
    if depth >= query.max_depth() {
        return Ok(1);
    }
    if stack.contains(&value.slot()) {
        return Ok(1);
    }
    stack.insert(value.slot());
    let answer = compute_num_sign_bits_operator(value, query, depth, stack, ty_bits);
    stack.remove(&value.slot());
    let first_answer = answer?;

    // `FirstAnswer` is what the operator switch established before falling
    // through; the tail below can only improve on it.
    if let Some(exact) = first_answer.exact {
        return Ok(exact);
    }

    // Finally, if the top bits are provably all zeros or all ones, use that.
    let known = compute_known_bits(value, query)?;
    Ok(first_answer.floor.max(known.count_min_sign_bits()))
}

/// What the operator switch concluded: either an `exact` answer it returned
/// outright, or a `floor` it established before falling through to the
/// `computeKnownBits` tail. Upstream spells these as `return` versus assigning
/// `FirstAnswer` and `break`ing.
struct SignBitsFromOperator {
    exact: Option<u32>,
    floor: u32,
}

impl SignBitsFromOperator {
    fn exact(bits: u32) -> Self {
        Self {
            exact: Some(bits),
            floor: 1,
        }
    }
    fn fall_through() -> Self {
        Self {
            exact: None,
            floor: 1,
        }
    }
    fn floor(bits: u32) -> Self {
        Self {
            exact: None,
            floor: bits,
        }
    }
}

/// The operator switch of `ComputeNumSignBitsImpl`.
fn compute_num_sign_bits_operator<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
    ty_bits: u32,
) -> IrResult<SignBitsFromOperator> {
    let ValueKindData::Instruction(inst) = &value.data().kind else {
        return Ok(SignBitsFromOperator::fall_through());
    };
    let limit = u64::from(ty_bits);

    match &inst.kind {
        InstructionKindData::Cast(data) => {
            let src = value_from_id(value, data.src.get());
            let src_bits = value_bit_width(src, query.data_layout()).unwrap_or(0);
            match data.kind {
                // sext adds exactly the widened bits to the source's count.
                CastOpcode::SExt => {
                    let tmp = compute_num_sign_bits_inner(src, query, depth + 1, stack)?;
                    Ok(SignBitsFromOperator::exact(
                        tmp.saturating_add(ty_bits.saturating_sub(src_bits)),
                    ))
                }
                // trunc keeps whatever sign bits survive the narrowing.
                CastOpcode::Trunc => {
                    let tmp = compute_num_sign_bits_inner(src, query, depth + 1, stack)?;
                    let lost = src_bits.saturating_sub(ty_bits);
                    Ok(SignBitsFromOperator::exact(if tmp > lost {
                        tmp - lost
                    } else {
                        1
                    }))
                }
                _ => Ok(SignBitsFromOperator::fall_through()),
            }
        }

        // sdiv X, C adds floor(log2 C) sign bits for a strictly positive C.
        InstructionKindData::SDiv(data) => {
            let rhs = value_from_id(value, data.rhs.get());
            let Some(denominator) = argument_constant(Some(rhs)) else {
                return Ok(SignBitsFromOperator::fall_through());
            };
            // Ignore a non-positive denominator.
            if !denominator.is_strictly_positive() {
                return Ok(SignBitsFromOperator::fall_through());
            }
            let lhs = value_from_id(value, data.lhs.get());
            let num_bits = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            let added = denominator.log_base2().unwrap_or(0);
            Ok(SignBitsFromOperator::exact(
                ty_bits.min(num_bits.saturating_add(added)),
            ))
        }

        // srem X, C lands in (-C, C) for a strictly positive C, which bounds
        // the leading sign bits below by `ty_bits - ceilLogBase2(C)`.
        InstructionKindData::SRem(data) => {
            let lhs = value_from_id(value, data.lhs.get());
            let mut tmp = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            let rhs = value_from_id(value, data.rhs.get());
            if let Some(denominator) = argument_constant(Some(rhs))
                && denominator.is_strictly_positive()
            {
                tmp = tmp.max(ty_bits.saturating_sub(denominator.ceil_log_base2()));
            }
            Ok(SignBitsFromOperator::exact(tmp))
        }

        // ashr X, C adds C sign bits.
        InstructionKindData::AShr(data) => {
            let lhs = value_from_id(value, data.lhs.get());
            let tmp = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            let rhs = value_from_id(value, data.rhs.get());
            let Some(shift_amount) = argument_constant(Some(rhs)) else {
                return Ok(SignBitsFromOperator::exact(tmp));
            };
            let amount = shift_amount.limited_value(limit);
            if amount >= limit {
                // Bad shift.
                return Ok(SignBitsFromOperator::fall_through());
            }
            let amount = u32::try_from(amount).unwrap_or(ty_bits);
            Ok(SignBitsFromOperator::exact(
                tmp.saturating_add(amount).min(ty_bits),
            ))
        }

        // shl destroys sign bits.
        InstructionKindData::Shl(data) => {
            let rhs = value_from_id(value, data.rhs.get());
            let Some(shift_amount) = argument_constant(Some(rhs)) else {
                return Ok(SignBitsFromOperator::fall_through());
            };
            let amount = shift_amount.limited_value(limit);
            if amount >= limit {
                // Bad shift.
                return Ok(SignBitsFromOperator::fall_through());
            }
            let amount = u32::try_from(amount).unwrap_or(ty_bits);
            // Upstream additionally looks through a `zext` whose extended bits
            // are all shifted out, treating it as a `sext`. That arm needs the
            // matcher DSL against the shift amount; without it this falls
            // through to the known-bits tail, which is weaker, never wrong.
            let lhs = value_from_id(value, data.lhs.get());
            let tmp = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            if amount >= tmp {
                // Shifted all sign bits out.
                return Ok(SignBitsFromOperator::fall_through());
            }
            Ok(SignBitsFromOperator::exact(tmp - amount))
        }

        // Logical binary ops preserve the sign bits at worst. Upstream records
        // this as `FirstAnswer` and breaks, so the known-bits tail can still
        // improve on it — hence `floor` rather than `exact`.
        InstructionKindData::And(data)
        | InstructionKindData::Or(data)
        | InstructionKindData::Xor(data) => {
            let lhs = value_from_id(value, data.lhs.get());
            let tmp = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            if tmp == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            let rhs = value_from_id(value, data.rhs.get());
            let tmp2 = compute_num_sign_bits_inner(rhs, query, depth + 1, stack)?;
            Ok(SignBitsFromOperator::floor(tmp.min(tmp2)))
        }

        // The minimum over both arms. Upstream's signed min/max clamp
        // recognition (`isSignedMinMaxClamp`) needs the matcher DSL and is not
        // ported, so this is the plain two-arm minimum.
        InstructionKindData::Select(data) => {
            let true_val = value_from_id(value, data.true_val.get());
            let tmp = compute_num_sign_bits_inner(true_val, query, depth + 1, stack)?;
            if tmp == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            let false_val = value_from_id(value, data.false_val.get());
            let tmp2 = compute_num_sign_bits_inner(false_val, query, depth + 1, stack)?;
            Ok(SignBitsFromOperator::exact(tmp.min(tmp2)))
        }

        // add carries at most one bit, so at worst one more than its inputs.
        InstructionKindData::Add(data) => {
            let lhs = value_from_id(value, data.lhs.get());
            let tmp = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            if tmp == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            // Special case decrementing a value (add X, -1).
            let rhs = value_from_id(value, data.rhs.get());
            if argument_constant(Some(rhs)).is_some_and(|c| c.is_all_ones()) {
                let known = compute_known_bits_inner(lhs, query, depth + 1, stack)?;
                // A 0-or-1 input gives 0/-1 out, which is all sign bits set.
                if known
                    .zero_mask()
                    .bitor(&ApInt::from_words(ty_bits, &[1]))
                    .is_all_ones()
                {
                    return Ok(SignBitsFromOperator::exact(ty_bits));
                }
                // Subtracting one from a positive number cannot carry out.
                if known.is_non_negative() {
                    return Ok(SignBitsFromOperator::exact(tmp));
                }
            }
            let tmp2 = compute_num_sign_bits_inner(rhs, query, depth + 1, stack)?;
            if tmp2 == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            Ok(SignBitsFromOperator::exact(tmp.min(tmp2).saturating_sub(1)))
        }

        InstructionKindData::Sub(data) => {
            let rhs = value_from_id(value, data.rhs.get());
            let tmp2 = compute_num_sign_bits_inner(rhs, query, depth + 1, stack)?;
            if tmp2 == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            // Handle negation (sub 0, X).
            let lhs = value_from_id(value, data.lhs.get());
            if argument_constant(Some(lhs)).is_some_and(|c| c.is_zero()) {
                let known = compute_known_bits_inner(rhs, query, depth + 1, stack)?;
                // A 0-or-1 input gives 0/-1 out, which is all sign bits set.
                if known
                    .zero_mask()
                    .bitor(&ApInt::from_words(ty_bits, &[1]))
                    .is_all_ones()
                {
                    return Ok(SignBitsFromOperator::exact(ty_bits));
                }
                // Negating a positive keeps the operand's sign-bit count.
                if known.is_non_negative() {
                    return Ok(SignBitsFromOperator::exact(tmp2));
                }
                // Otherwise fall into the generic sub reasoning below.
            }
            let tmp = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            if tmp == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            Ok(SignBitsFromOperator::exact(tmp.min(tmp2).saturating_sub(1)))
        }

        // A mul's output has at most the sum of its inputs' valid bits.
        InstructionKindData::Mul(data) => {
            let lhs = value_from_id(value, data.lhs.get());
            let lhs_bits = compute_num_sign_bits_inner(lhs, query, depth + 1, stack)?;
            if lhs_bits == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            let rhs = value_from_id(value, data.rhs.get());
            let rhs_bits = compute_num_sign_bits_inner(rhs, query, depth + 1, stack)?;
            if rhs_bits == 1 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            let valid = (ty_bits.saturating_sub(lhs_bits).saturating_add(1))
                .saturating_add(ty_bits.saturating_sub(rhs_bits).saturating_add(1));
            Ok(SignBitsFromOperator::exact(if valid > ty_bits {
                1
            } else {
                ty_bits - valid + 1
            }))
        }

        // The minimum over the incoming values. Upstream declines phis with
        // more than four incoming edges, and so does this.
        InstructionKindData::Phi(data) => {
            let incoming = data.incoming.borrow();
            // Unreachable blocks may have zero-operand PHI nodes.
            if incoming.is_empty() || incoming.len() > 4 {
                return Ok(SignBitsFromOperator::fall_through());
            }
            let mut tmp = ty_bits;
            for (incoming_value, _) in incoming.iter() {
                if tmp == 1 {
                    return Ok(SignBitsFromOperator::exact(1));
                }
                let operand = value_from_id(value, incoming_value.get());
                tmp = tmp.min(compute_num_sign_bits_inner(
                    operand,
                    query,
                    depth + 1,
                    stack,
                )?);
            }
            Ok(SignBitsFromOperator::exact(tmp))
        }

        _ => Ok(SignBitsFromOperator::fall_through()),
    }
}

// --------------------------------------------------------------------------
// Value-level sign, power-of-two and equality predicates
// --------------------------------------------------------------------------

/// The instruction payload behind `value`, or `None` when it is not one.
///
/// Upstream reaches the same values through `dyn_cast<Operator>`, which also
/// admits constant expressions. llvmkit stores those in a separate arm of
/// `ValueKindData`, so a caller that wants both has to ask twice; every arm
/// ported below inspects instructions only, matching what upstream's switches
/// actually reach for the opcodes they name.
fn instruction_kind<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<&'ctx InstructionKindData> {
    match &value.data().kind {
        ValueKindData::Instruction(inst) => Some(&inst.kind),
        _ => None,
    }
}

/// Return true when the sign bit of `value` is known zero.
///
/// Ports `llvm::isKnownNonNegative` (`ValueTracking.cpp`).
pub fn is_known_non_negative<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    Ok(compute_known_bits(value, query)?.is_non_negative())
}

/// Return true when the sign bit of `value` is known one.
///
/// Ports `llvm::isKnownNegative` (`ValueTracking.cpp`).
pub fn is_known_negative<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    Ok(compute_known_bits(value, query)?.is_negative())
}

/// Return true when `value` is known strictly greater than zero.
///
/// Ports `llvm::isKnownPositive` (`ValueTracking.cpp`).
///
/// The right disjunct calls `is_known_non_zero`, which llvmkit answers from
/// known bits alone — the same source as the left disjunct. Upstream's
/// `isKnownNonZero` is a separate operator walk, so there the two differ. The
/// call is kept in the shape upstream wrote it rather than folded away: when
/// `is_known_non_zero` grows its own walk this becomes load-bearing with no
/// second edit here.
pub fn is_known_positive<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    if let Some(constant) = argument_constant(Some(value)) {
        return Ok(constant.is_strictly_positive());
    }
    let known = compute_known_bits(value, query)?;
    Ok(known.is_non_negative() && (known.is_non_zero() || is_known_non_zero(value, query)?))
}

/// Return true when every bit set in `mask` is known zero in `value`.
///
/// Ports `llvm::MaskedValueIsZero` (`ValueTracking.cpp`).
pub fn masked_value_is_zero<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    mask: &ApInt,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let known = compute_known_bits(value, query)?;
    Ok(mask.is_subset_of(known.zero_mask()))
}

/// Classify `icmp <predicate> %x, <rhs>` as a test of `%x`'s sign bit.
///
/// Ports `llvm::isSignBitCheck` (`ValueTracking.cpp`). Upstream returns a
/// `bool` and writes the polarity through a `bool &TrueIfSigned`
/// out-parameter; llvmkit returns `Some(true_if_signed)` for a sign-bit check
/// and `None` otherwise, so the polarity is unreadable when the
/// classification failed rather than left at whatever the caller initialised.
pub fn is_sign_bit_check(predicate: IntPredicate, rhs: &ApInt) -> Option<bool> {
    match predicate {
        // True if LHS s< 0.
        IntPredicate::Slt => rhs.is_zero().then_some(true),
        // True if LHS s<= -1.
        IntPredicate::Sle => rhs.is_all_ones().then_some(true),
        // True if LHS s> -1.
        IntPredicate::Sgt => rhs.is_all_ones().then_some(false),
        // True if LHS s>= 0.
        IntPredicate::Sge => rhs.is_zero().then_some(false),
        // True if LHS u> RHS and RHS == sign-bit-mask - 1.
        IntPredicate::Ugt => rhs.is_max_signed_value().then_some(true),
        // True if LHS u>= RHS and RHS == sign-bit-mask (2^7, 2^15, 2^31, ...).
        IntPredicate::Uge => rhs.is_min_signed_value().then_some(true),
        // True if LHS u< RHS and RHS == sign-bit-mask.
        IntPredicate::Ult => rhs.is_min_signed_value().then_some(false),
        // True if LHS u<= RHS and RHS == sign-bit-mask - 1.
        IntPredicate::Ule => rhs.is_max_signed_value().then_some(false),
        IntPredicate::Eq | IntPredicate::Ne => None,
    }
}

/// Whether `value` satisfies upstream's `m_ZeroInt()`, and if so whether it is
/// a literal null rather than a lane pattern containing poison.
///
/// Ports the `cst_pred_ty<is_zero_int>` matcher together with the
/// `Constant::isNullValue` test its two callers pair it with. `Some(true)`
/// means matched *and* null; `Some(false)` means matched with at least one
/// poison lane standing in for a zero; `None` means no match.
fn zero_int_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<bool> {
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(_)) => {
            argument_constant(Some(value))?.is_zero().then_some(true)
        }
        // A vector constant matches when every lane is a zero or a poison
        // standing in for one; it is a null value only if no lane is poison.
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            let mut all_null = true;
            for element in elements.iter() {
                match zero_int_constant(value_from_id(value, *element)) {
                    Some(true) => {}
                    Some(false) => all_null = false,
                    None => return None,
                }
            }
            Some(all_null)
        }
        ValueKindData::Constant(ConstantData::Poison) => Some(false),
        _ => None,
    }
}

/// Return true when `x` and `y` are provably negations of one another.
///
/// Ports `llvm::isKnownNegation` (`ValueTracking.cpp`). `need_nsw` requires
/// the negating `sub` to carry `nsw`; `allow_poison` admits a subtrahend whose
/// zero is a poison lane rather than a literal zero, which is exactly what
/// upstream's `m_Neg` accepts and its `Zero->isNullValue()` check then filters.
pub fn is_known_negation<'ctx, B: ModuleBrand + 'ctx>(
    x: Value<'ctx, B>,
    y: Value<'ctx, B>,
    need_nsw: bool,
    allow_poison: bool,
) -> bool {
    let is_negation_of = |x: Value<'ctx, B>, y: Value<'ctx, B>| -> bool {
        // `m_Neg(m_Specific(Y))` is `m_Sub(m_ZeroInt(), Y)`.
        let Some(InstructionKindData::Sub(data)) = instruction_kind(x) else {
            return false;
        };
        if data.rhs.get() != y.slot() {
            return false;
        }
        let Some(zero_is_null) = zero_int_constant(value_from_id(x, data.lhs.get())) else {
            return false;
        };
        if need_nsw && !data.no_signed_wrap {
            return false;
        }
        allow_poison || zero_is_null
    };

    // X = -Y or Y = -X.
    if is_negation_of(x, y) || is_negation_of(y, x) {
        return true;
    }

    // X = sub (A, B), Y = sub (B, A), with `nsw` on both when required.
    let (Some(InstructionKindData::Sub(x_sub)), Some(InstructionKindData::Sub(y_sub))) =
        (instruction_kind(x), instruction_kind(y))
    else {
        return false;
    };
    if need_nsw && !(x_sub.no_signed_wrap && y_sub.no_signed_wrap) {
        return false;
    }
    x_sub.lhs.get() == y_sub.rhs.get() && x_sub.rhs.get() == y_sub.lhs.get()
}

/// Return true when `x` and `y` are provably inverse boolean conditions.
///
/// Ports `llvm::isKnownInversion` (`ValueTracking.cpp`).
pub fn is_known_inversion<'ctx, B: ModuleBrand + 'ctx>(
    x: Value<'ctx, B>,
    y: Value<'ctx, B>,
) -> bool {
    // X = icmp pred1 A, B and Y = icmp pred2 A, C — the second commutatively,
    // which swaps its predicate when A is on the right.
    let Some(InstructionKindData::ICmp(x_cmp)) = instruction_kind(x) else {
        return false;
    };
    let Some(InstructionKindData::ICmp(y_cmp)) = instruction_kind(y) else {
        return false;
    };
    let a = x_cmp.lhs.get();
    let b = x_cmp.rhs.get();
    let predicate1 = x_cmp.predicate;
    let (predicate2, c) = if y_cmp.lhs.get() == a {
        (y_cmp.predicate, y_cmp.rhs.get())
    } else if y_cmp.rhs.get() == a {
        (y_cmp.predicate.swapped(), y_cmp.lhs.get())
    } else {
        return false;
    };

    // They must both carry `samesign` or neither.
    if x_cmp.samesign != y_cmp.samesign {
        return false;
    }

    if b == c {
        return predicate1 == predicate2.inverse();
    }

    // Otherwise infer the relationship from the two constant right-hand sides.
    let (Some(rhs1), Some(rhs2)) = (
        argument_constant(Some(value_from_id(x, b))),
        argument_constant(Some(value_from_id(y, c))),
    ) else {
        return false;
    };

    // Sign bits of the two constants must match under `samesign`.
    if x_cmp.samesign && rhs1.is_negative() != rhs2.is_negative() {
        return false;
    }

    let range1 = ConstantRange::make_exact_icmp_region(predicate1, &rhs1);
    let range2 = ConstantRange::make_exact_icmp_region(predicate2, &rhs2);
    range1.inverse() == range2
}

/// Return true when every user of `instruction` compares it against zero.
///
/// Ports `llvm::isOnlyUsedInZeroComparison` (`ValueTracking.cpp`).
pub fn is_only_used_in_zero_comparison<'ctx, B: ModuleBrand + 'ctx>(
    instruction: Value<'ctx, B>,
) -> bool {
    zero_comparison_users(instruction, |_| true)
}

/// Return true when every user of `instruction` compares it against zero with
/// an equality predicate.
///
/// Ports `llvm::isOnlyUsedInZeroEqualityComparison` (`ValueTracking.cpp`).
pub fn is_only_used_in_zero_equality_comparison<'ctx, B: ModuleBrand + 'ctx>(
    instruction: Value<'ctx, B>,
) -> bool {
    zero_comparison_users(instruction, |predicate| {
        matches!(predicate, IntPredicate::Eq | IntPredicate::Ne)
    })
}

/// Shared body of the two zero-comparison predicates: a non-empty user list
/// whose every member is an `icmp` against zero accepted by `predicate_ok`.
fn zero_comparison_users<'ctx, B: ModuleBrand + 'ctx, F>(
    instruction: Value<'ctx, B>,
    predicate_ok: F,
) -> bool
where
    F: Fn(IntPredicate) -> bool,
{
    let mut users = instruction.users().peekable();
    if users.peek().is_none() {
        return false;
    }
    users.all(|user| {
        let user = user.to_erased();
        let Some(InstructionKindData::ICmp(cmp)) = instruction_kind(user) else {
            return false;
        };
        if !predicate_ok(cmp.predicate) {
            return false;
        }
        // `m_ICmp(m_Value(), m_Zero())` puts the zero on the right.
        argument_constant(Some(value_from_id(user, cmp.rhs.get())))
            .is_some_and(|constant| constant.is_zero())
    })
}

/// Return true when `value` is known to be a power of two.
///
/// Ports `llvm::isKnownToBeAPowerOfTwo` (`ValueTracking.cpp`). `or_zero`
/// widens the claim to "a power of two, or zero".
///
/// Three of upstream's sources are not consulted, each because llvmkit does
/// not model the input rather than because the reasoning was skipped, and each
/// omission only makes the answer weaker: the `@llvm.assume` refinement (no
/// `AssumptionCache`), the dominating-condition refinement (no `DomConditionCache`),
/// and the `vscale` arm (`vscale_range` is on `attribute_td_drift.rs`'s
/// `NOT_YET_MODELED` list, so the attribute it reads does not exist here).
pub fn is_known_to_be_a_power_of_two<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    or_zero: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    is_known_to_be_a_power_of_two_inner(value, or_zero, query, 0)
}

fn is_known_to_be_a_power_of_two_inner<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    or_zero: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    if let ValueKindData::Constant(_) = &value.data().kind {
        let Some(constant) = argument_constant(Some(value)) else {
            return Ok(false);
        };
        return Ok(if or_zero {
            constant.is_zero() || constant.is_power_of_2()
        } else {
            constant.is_power_of_2()
        });
    }

    // i1 is by definition a power of two or zero.
    if or_zero && value_bit_width(value, query.data_layout()) == Some(1) {
        return Ok(true);
    }

    let Some(kind) = instruction_kind(value) else {
        return Ok(false);
    };

    // `1 << X` is a power of two unless the one is shifted off the end, in
    // which case the result is poison rather than wrong.
    if let InstructionKindData::Shl(data) = kind
        && argument_is_const_one(Some(value_from_id(value, data.lhs.get())))
    {
        return Ok(true);
    }
    // `(signmask) >>l X` likewise.
    if let InstructionKindData::LShr(data) = kind
        && argument_constant(Some(value_from_id(value, data.lhs.get())))
            .is_some_and(|constant| constant.is_sign_mask())
    {
        return Ok(true);
    }

    // The remaining tests all recurse.
    if depth >= query.max_depth() {
        return Ok(false);
    }
    let depth = depth + 1;
    let operand = |slot: &Cell<ValueSlot>| value_from_id(value, slot.get());
    let recurse = |operand: Value<'ctx, B>, or_zero: bool| {
        is_known_to_be_a_power_of_two_inner(operand, or_zero, query, depth)
    };

    match kind {
        InstructionKindData::Cast(data) => match data.kind {
            CastOpcode::ZExt => recurse(value_from_id(value, data.src.get()), or_zero),
            CastOpcode::Trunc => {
                Ok(or_zero && recurse(value_from_id(value, data.src.get()), or_zero)?)
            }
            _ => Ok(false),
        },
        InstructionKindData::Shl(data) => {
            if or_zero
                || (query.uses_instruction_info() && (data.no_unsigned_wrap || data.no_signed_wrap))
            {
                recurse(operand(&data.lhs), or_zero)
            } else {
                Ok(false)
            }
        }
        InstructionKindData::LShr(data) => {
            if or_zero || (query.uses_instruction_info() && data.is_exact) {
                recurse(operand(&data.lhs), or_zero)
            } else {
                Ok(false)
            }
        }
        InstructionKindData::UDiv(data) => {
            if query.uses_instruction_info() && data.is_exact {
                recurse(operand(&data.lhs), or_zero)
            } else {
                Ok(false)
            }
        }
        InstructionKindData::Mul(data) => Ok(recurse(operand(&data.rhs), or_zero)?
            && recurse(operand(&data.lhs), or_zero)?
            && (or_zero || is_known_non_zero(value, query)?)),
        InstructionKindData::And(data) => {
            // A power of two and'd with anything is a power of two or zero.
            if or_zero && (recurse(operand(&data.rhs), true)? || recurse(operand(&data.lhs), true)?)
            {
                return Ok(true);
            }
            // `X & (-X)` is always a power of two or zero.
            let lhs = operand(&data.lhs);
            let rhs = operand(&data.rhs);
            if is_negation_of_operand(lhs, rhs) || is_negation_of_operand(rhs, lhs) {
                return Ok(or_zero || is_known_non_zero(lhs, query)?);
            }
            Ok(false)
        }
        InstructionKindData::Add(data) => power_of_two_add(value, data, or_zero, query, depth),
        InstructionKindData::Select(data) => Ok(recurse(operand(&data.true_val), or_zero)?
            && recurse(operand(&data.false_val), or_zero)?),
        InstructionKindData::Phi(data) => power_of_two_phi(value, data, or_zero, query, depth),
        InstructionKindData::Call(_) | InstructionKindData::Invoke(_) => {
            power_of_two_intrinsic(value, or_zero, query, depth)
        }
        _ => Ok(false),
    }
}

/// `m_Neg(m_Specific(other))` restricted to what the `and` arm of
/// `isKnownToBeAPowerOfTwo` asks: is `candidate` the negation of `other`?
fn is_negation_of_operand<'ctx, B: ModuleBrand + 'ctx>(
    candidate: Value<'ctx, B>,
    other: Value<'ctx, B>,
) -> bool {
    let Some(InstructionKindData::Sub(data)) = instruction_kind(candidate) else {
        return false;
    };
    data.rhs.get() == other.slot()
        && zero_int_constant(value_from_id(candidate, data.lhs.get())).is_some()
}

/// The `Instruction::Add` arm of `isKnownToBeAPowerOfTwo`.
fn power_of_two_add<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &BinaryOpData,
    or_zero: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    let lhs = value_from_id(value, data.lhs.get());
    let rhs = value_from_id(value, data.rhs.get());

    let no_wrap = query.uses_instruction_info() && (data.no_unsigned_wrap || data.no_signed_wrap);

    // Adding a power-of-two or zero to the same power-of-two or zero yields
    // the original power-of-two, a larger power-of-two, or zero.
    if or_zero || no_wrap {
        if is_and_with_operand(lhs, rhs)
            && is_known_to_be_a_power_of_two_inner(rhs, or_zero, query, depth)?
        {
            return Ok(true);
        }
        if is_and_with_operand(rhs, lhs)
            && is_known_to_be_a_power_of_two_inner(lhs, or_zero, query, depth)?
        {
            return Ok(true);
        }

        let lhs_bits = compute_known_bits(lhs, query)?;
        let rhs_bits = compute_known_bits(rhs, query)?;
        // If i8 V is a power of two or zero:
        //   ZeroBits: 1 1 1 0 1 1 1 1
        //  ~ZeroBits: 0 0 0 1 0 0 0 0
        if lhs_bits
            .zero_mask()
            .bitand(rhs_bits.zero_mask())
            .not()
            .is_power_of_2()
        {
            // Without `or_zero` the result must not be zero, so one side has
            // to have a known one bit.
            if or_zero || !rhs_bits.one_mask().is_zero() || !lhs_bits.one_mask().is_zero() {
                return Ok(true);
            }
        }
    }

    // `lshr(UINT_MAX, Y) + 1` is a power of two (when the add is `nuw`) or zero.
    if or_zero || (query.uses_instruction_info() && data.no_unsigned_wrap) {
        let is_all_ones_lshr = |candidate: Value<'ctx, B>| {
            matches!(instruction_kind(candidate), Some(InstructionKindData::LShr(shift))
                if argument_constant(Some(value_from_id(candidate, shift.lhs.get())))
                    .is_some_and(|constant| constant.is_all_ones()))
        };
        if is_all_ones_lshr(lhs) && argument_is_const_one(Some(rhs)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `m_c_And(m_Specific(other), m_Value())` — is `candidate` an `and` with
/// `other` as one of its operands?
fn is_and_with_operand<'ctx, B: ModuleBrand + 'ctx>(
    candidate: Value<'ctx, B>,
    other: Value<'ctx, B>,
) -> bool {
    matches!(instruction_kind(candidate), Some(InstructionKindData::And(data))
        if data.lhs.get() == other.slot() || data.rhs.get() == other.slot())
}

/// The `Instruction::PHI` arm of `isKnownToBeAPowerOfTwo`.
///
/// Upstream re-points the query's context instruction at each incoming block's
/// terminator before recursing; llvmkit does not model a per-edge context, so
/// that refinement is skipped exactly as the known-bits phi arm skips it. It
/// can only leave an answer weaker.
fn power_of_two_phi<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &PhiData,
    or_zero: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    if is_power_of_two_recurrence(value, data, or_zero, query, depth)? {
        return Ok(true);
    }

    // Recursion is limited to two levels so the search stays quadratic in the
    // operand count.
    let new_depth = depth.max(query.max_depth().saturating_sub(1));
    let incoming = data.incoming.borrow();
    for (operand, _) in incoming.iter() {
        // A value coming from the phi itself is a power of two by induction.
        if operand.get() == value.slot() {
            continue;
        }
        if !is_known_to_be_a_power_of_two_inner(
            value_from_id(value, operand.get()),
            or_zero,
            query,
            new_depth,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Ports `isPowerOfTwoRecurrence` (`ValueTracking.cpp`).
fn is_power_of_two_recurrence<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &PhiData,
    or_zero: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    let Some(recurrence) = match_simple_recurrence(value, data, query.uses_instruction_info())
    else {
        return Ok(false);
    };
    let start = value_from_id(value, recurrence.start);
    let step = value_from_id(value, recurrence.step);

    // The initial value must be a power of two.
    if !is_known_to_be_a_power_of_two_inner(start, or_zero, query, depth)? {
        return Ok(false);
    }

    // Except for `mul`, the induction variable must be the left operand,
    // otherwise the step's value can be arbitrary.
    if recurrence.opcode != BinaryOpcode::Mul && !recurrence.phi_is_left_operand {
        return Ok(false);
    }

    let no_wrap = recurrence.no_unsigned_wrap || recurrence.no_signed_wrap;
    let start_is_power_of_two_constant = argument_constant(Some(start))
        .is_some_and(|constant| constant.is_power_of_2() && !constant.is_sign_mask());

    match recurrence.opcode {
        // Power of two is closed under multiplication.
        BinaryOpcode::Mul => Ok((or_zero || no_wrap)
            && is_known_to_be_a_power_of_two_inner(step, or_zero, query, depth)?),
        // A signed division's start must not be the sign mask, so being a
        // power of two is not enough — it has to be a constant one.
        BinaryOpcode::SDiv if !start_is_power_of_two_constant => Ok(false),
        // The divisor must be a power of two. Without `or_zero` the induction
        // variable is only guaranteed non-zero when the division is exact.
        BinaryOpcode::SDiv | BinaryOpcode::UDiv => Ok((or_zero || recurrence.is_exact)
            && is_known_to_be_a_power_of_two_inner(step, false, query, depth)?),
        BinaryOpcode::Shl => Ok(or_zero || no_wrap),
        BinaryOpcode::AShr if !start_is_power_of_two_constant => Ok(false),
        BinaryOpcode::AShr | BinaryOpcode::LShr => Ok(or_zero || recurrence.is_exact),
        _ => Ok(false),
    }
}

/// The `Instruction::Call` / `Instruction::Invoke` arm of
/// `isKnownToBeAPowerOfTwo`.
fn power_of_two_intrinsic<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    or_zero: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    let (callee, arguments) = match instruction_kind(value) {
        Some(InstructionKindData::Call(data)) => (data.callee.get(), &data.args),
        Some(InstructionKindData::Invoke(data)) => (data.callee.get(), &data.args),
        _ => return Ok(false),
    };
    let Some(semantic) = intrinsic_semantic_for_callee(value, callee) else {
        return Ok(false);
    };
    let argument = |index: usize| {
        arguments
            .get(index)
            .map(|slot| value_from_id(value, slot.get()))
    };

    match semantic {
        IntrinsicSemantic::UMax
        | IntrinsicSemantic::SMax
        | IntrinsicSemantic::UMin
        | IntrinsicSemantic::SMin => {
            let (Some(first), Some(second)) = (argument(0), argument(1)) else {
                return Ok(false);
            };
            Ok(
                is_known_to_be_a_power_of_two_inner(second, or_zero, query, depth)?
                    && is_known_to_be_a_power_of_two_inner(first, or_zero, query, depth)?,
            )
        }
        // bswap/bitreverse move bits around without changing how many are set.
        IntrinsicSemantic::BSwap | IntrinsicSemantic::BitReverse => {
            let Some(first) = argument(0) else {
                return Ok(false);
            };
            is_known_to_be_a_power_of_two_inner(first, or_zero, query, depth)
        }
        // When both inputs are the same value this is a rotate, and
        // `is_pow2(rotate(x, y)) == is_pow2(x)`.
        IntrinsicSemantic::FShl | IntrinsicSemantic::FShr => {
            let (Some(first), Some(second)) = (argument(0), argument(1)) else {
                return Ok(false);
            };
            if first.slot() != second.slot() {
                return Ok(false);
            }
            is_known_to_be_a_power_of_two_inner(first, or_zero, query, depth)
        }
        _ => Ok(false),
    }
}

/// The operand pair of a binary instruction with the given opcode.
fn binary_operands_of<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    opcode: BinaryOpcode,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    let (found, data) = instruction_kind(value).and_then(binary_operator_parts)?;
    (found == opcode).then(|| {
        (
            value_from_id(value, data.lhs.get()),
            value_from_id(value, data.rhs.get()),
        )
    })
}

/// Whether `value` matches upstream's `m_AllOnes()`.
///
/// Ports `cst_pred_ty<is_all_ones>`, which accepts a scalar `-1` *and* a
/// vector constant whose every lane is one — the form a vector `not` takes.
fn is_all_ones_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(_)) => {
            argument_constant(Some(value)).is_some_and(|constant| constant.is_all_ones())
        }
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            !elements.is_empty()
                && elements
                    .iter()
                    .all(|element| is_all_ones_constant(value_from_id(value, *element)))
        }
        _ => false,
    }
}

/// `m_Not(V)`: `xor V, -1`, matched commutatively.
pub(crate) fn not_operand<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let (lhs, rhs) = binary_operands_of(value, BinaryOpcode::Xor)?;
    if is_all_ones_constant(rhs) {
        Some(lhs)
    } else if is_all_ones_constant(lhs) {
        Some(rhs)
    } else {
        None
    }
}

/// Every immediate value `value` could take, or `None` if that set cannot be
/// enumerated within `max_count`.
///
/// Ports `llvm::collectPossibleValues`, which walks back through `select` and
/// `phi` collecting the constants at the leaves. Upstream fills a caller-owned
/// set and returns whether the enumeration is *complete*; here the two are one
/// answer, because an incomplete set is exactly what a caller must not act on.
/// Its only in-tree caller (`SimplifyCFG`) passes a fresh set and reads the
/// bool first, so nothing is lost by not exposing the partial result.
///
/// `max_count` bounds the answer: reaching it is "incomplete", not an error.
/// Any leaf that is neither an immediate constant nor a `select`/`phi` — an
/// argument, a load, an `add` — makes the set unenumerable, which is also
/// `None`. When `allow_undef_or_poison` is false, a constant that is not
/// provably neither also gives up.
///
/// Upstream's `m_ImmConstant` is "a constant that is not, and does not
/// contain, a `ConstantExpr`". llvmkit keeps constant expressions in their own
/// `ConstantData::Expr` arm, so the check is structural here rather than a
/// predicate over one flat `Constant` class.
pub fn collect_possible_values<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    max_count: usize,
    allow_undef_or_poison: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<Option<Vec<Value<'ctx, B>>>> {
    let mut state = PossibleValues {
        constants: Vec::new(),
        visited: HashSet::new(),
        worklist: Vec::new(),
        max_count,
        allow_undef_or_poison,
    };

    if !state.push(value, query)? {
        return Ok(None);
    }
    while let Some(current) = state.worklist.pop() {
        match instruction_kind(current) {
            Some(InstructionKindData::Select(data)) => {
                let true_value = value_from_id(current, data.true_val.get());
                let false_value = value_from_id(current, data.false_val.get());
                if !state.push(true_value, query)? || !state.push(false_value, query)? {
                    return Ok(None);
                }
            }
            Some(InstructionKindData::Phi(data)) => {
                // Read the operands out before pushing: `push` walks the
                // module, and holding the `RefCell` borrow across it would
                // outlast what this loop needs.
                let incomings: Vec<ValueSlot> = data
                    .incoming
                    .borrow()
                    .iter()
                    .map(|(incoming, _)| incoming.get())
                    .collect();
                for incoming in incomings {
                    let incoming = value_from_id(current, incoming);
                    // Upstream's fast path for a recurrence phi: an operand
                    // that is the phi itself adds nothing.
                    if incoming == current {
                        continue;
                    }
                    if !state.push(incoming, query)? {
                        return Ok(None);
                    }
                }
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(state.constants))
}

/// The running state of [`collect_possible_values`] — upstream's `Constants`,
/// `Visited` and `Worklist` locals, plus the two bounds its `Push` lambda
/// closes over.
struct PossibleValues<'ctx, B: ModuleBrand> {
    constants: Vec<Value<'ctx, B>>,
    visited: HashSet<ValueSlot>,
    worklist: Vec<Value<'ctx, B>>,
    max_count: usize,
    allow_undef_or_poison: bool,
}

impl<'ctx, B: ModuleBrand + 'ctx> PossibleValues<'ctx, B> {
    /// Ports the `Push` lambda. `false` is upstream's "give up".
    fn push<'a>(
        &mut self,
        value: Value<'ctx, B>,
        query: &ValueTrackingQuery<'a, 'ctx, B>,
    ) -> IrResult<bool> {
        if is_immediate_constant(value) {
            if !self.allow_undef_or_poison && !is_known_not_undef_or_poison(value, query)? {
                return Ok(false);
            }
            // Check membership first, so a repeat does not spend a slot.
            if self.constants.contains(&value) {
                return Ok(true);
            }
            if self.constants.len() == self.max_count {
                return Ok(false);
            }
            self.constants.push(value);
            return Ok(true);
        }
        if matches!(value.data().kind, ValueKindData::Instruction(_)) {
            if self.visited.insert(value.slot()) {
                self.worklist.push(value);
            }
            return Ok(true);
        }
        Ok(false)
    }
}

/// Whether `value` is an immediate constant — upstream's `m_ImmConstant`.
///
/// Upstream asks for a `Constant` that is not a `ConstantExpr` and does not
/// contain one. llvmkit stores constant expressions in their own arm, so this
/// rejects that arm and recurses through aggregates for the "contains" half.
///
/// Upstream carries one further escape hatch: a vector whose splat value is
/// expression-free counts even when the vector itself is not. That exists for
/// scalable splats held as a `ConstantExpr` and is marked with a `TODO` to
/// delete; llvmkit stores aggregate elements as slots, so a splat with no
/// expression in it already passes the recursive check.
fn is_immediate_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    let ValueKindData::Constant(constant) = &value.data().kind else {
        return false;
    };
    match constant {
        ConstantData::Expr(_) => false,
        ConstantData::Aggregate(elements) => elements
            .iter()
            .all(|element| is_immediate_constant(value_from_id(value, *element))),
        _ => true,
    }
}

/// The inner value `X` of an expression `f(X)` that is zero exactly when `X`
/// is, or `None` if `value` is not of that form.
///
/// Ports `llvm::stripNullTest`, which recognises one shape:
///
/// ```text
/// (X >> C1) or/add zext(X & mask(C2) != 0)
/// ```
///
/// where `mask(C2)` is a low-bit mask whose population count is `C1`. The
/// shift carries every bit at or above `C1`, and the compare folds the `C1`
/// bits below it into a single "any of them set" flag, so between them the two
/// operands are non-zero exactly when some bit of `X` is. The `or` and `add`
/// spellings agree because the two operands share no set bit.
///
/// Upstream answers a null `Value *` for no match, which is the `None` here.
pub fn strip_null_test<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    // Upstream's `m_c_BinOp` is guarded by an opcode check for `add` or `or`.
    let data = match instruction_kind(value)? {
        InstructionKindData::Add(data) | InstructionKindData::Or(data) => data,
        _ => return None,
    };
    let lhs = value_from_id(value, data.lhs.get());
    let rhs = value_from_id(value, data.rhs.get());
    // `m_c_BinOp` matches either operand order.
    null_test_operands(lhs, rhs).or_else(|| null_test_operands(rhs, lhs))
}

/// One operand order of [`strip_null_test`]'s commutative match: `shifted` as
/// `lshr X, C1` and `flag` as `zext(icmp ne (and X, mask(C2)), 0)`.
fn null_test_operands<'ctx, B: ModuleBrand + 'ctx>(
    shifted: Value<'ctx, B>,
    flag: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    // m_LShr(m_Value(X), m_APInt(C1))
    let (base, shift_amount) = binary_operands_of(shifted, BinaryOpcode::LShr)?;
    let shift_amount = splat_or_scalar_constant(shift_amount)?;

    // m_ZExt(m_SpecificICmp(ICMP_NE, .., m_Zero()))
    let compare = zext_source(flag)?;
    let InstructionKindData::ICmp(compare_data) = instruction_kind(compare)? else {
        return None;
    };
    if compare_data.predicate != IntPredicate::Ne {
        return None;
    }
    let masked = value_from_id(compare, compare_data.lhs.get());
    let zero = value_from_id(compare, compare_data.rhs.get());
    if !splat_or_scalar_constant(zero).is_some_and(|constant| constant.is_zero()) {
        return None;
    }

    // m_And(m_Deferred(X), m_LowBitMask(C2))
    let (masked_base, mask) = binary_operands_of(masked, BinaryOpcode::And)?;
    if masked_base != base {
        return None;
    }
    let mask = splat_or_scalar_constant(mask)?;
    if !mask.is_mask() {
        return None;
    }

    // `C2->popcount() == C1->getZExtValue()`. Upstream's `getZExtValue`
    // asserts the shift fits in 64 bits; comparing at the shift's own width
    // is the same test without the precondition, since a population count
    // never exceeds a bit width.
    let popcount = ApInt::from_words(shift_amount.bit_width(), &[u64::from(mask.popcount())]);
    shift_amount.eq_ap_int(&popcount).then_some(base)
}

/// The constant `value` carries, matching upstream's `m_APInt`: a scalar
/// integer constant, or a vector whose lanes all carry the same one.
///
/// Upstream reaches the vector case through `getSplatValue(AllowPoison=true)`,
/// so a poison lane is skipped rather than treated as a disagreement — it can
/// be read as whatever the other lanes hold.
fn splat_or_scalar_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApInt> {
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(_)) => argument_constant(Some(value)),
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            let mut splat: Option<ApInt> = None;
            for element in elements.iter() {
                let element = value_from_id(value, *element);
                if matches!(
                    element.data().kind,
                    ValueKindData::Constant(ConstantData::Poison)
                ) {
                    continue;
                }
                let element = splat_or_scalar_constant(element)?;
                match &splat {
                    Some(seen) if !seen.eq_ap_int(&element) => return None,
                    Some(_) => {}
                    None => splat = Some(element),
                }
            }
            splat
        }
        _ => None,
    }
}

/// The source of a `zext`, matching upstream's `m_ZExt`.
fn zext_source<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    match instruction_kind(value)? {
        InstructionKindData::Cast(data) if data.kind == CastOpcode::ZExt => {
            Some(value_from_id(value, data.src.get()))
        }
        _ => None,
    }
}

/// The source of a `zext` or `sext`, matching upstream's `m_ZExtOrSExt`.
fn zext_or_sext_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    match instruction_kind(value)? {
        InstructionKindData::Cast(data)
            if matches!(data.kind, CastOpcode::ZExt | CastOpcode::SExt) =>
        {
            Some(value_from_id(value, data.src.get()))
        }
        _ => None,
    }
}

/// Return true when `lhs` and `rhs` provably have no bit set in common.
///
/// Ports `llvm::haveNoCommonBitsSet` (`ValueTracking.cpp`), including the
/// `haveNoCommonBitsSetSpecialCases` patterns tried in both operand orders.
///
/// The special cases are each gated on the operand being known not-undef.
/// llvmkit's `is_known_not_undef` does not yet consult UB reachability or
/// assumptions, so it proves less than upstream's; the effect here is that a
/// pattern upstream accepts may fall through to the plain known-bits test.
/// That answers `false` where upstream answers `true` — a missed fact, never a
/// wrong one.
pub fn have_no_common_bits_set<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    if have_no_common_bits_set_special_cases(lhs, rhs, query)?
        || have_no_common_bits_set_special_cases(rhs, lhs, query)?
    {
        return Ok(true);
    }
    let lhs_bits = compute_known_bits(lhs, query)?;
    let rhs_bits = compute_known_bits(rhs, query)?;
    Ok(KnownBits::have_no_common_bits_set(&lhs_bits, &rhs_bits))
}

/// Ports `haveNoCommonBitsSetSpecialCases`. Called once per operand order.
fn have_no_common_bits_set_special_cases<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let and_pair = |value: Value<'ctx, B>| binary_operands_of(value, BinaryOpcode::And);

    // Look for an inverted mask: (X & ~M) op (Y & M).
    if let Some((left_a, left_b)) = and_pair(lhs) {
        for masked in [left_a, left_b] {
            let Some(mask) = not_operand(masked) else {
                continue;
            };
            let Some((right_a, right_b)) = and_pair(rhs) else {
                continue;
            };
            if (right_a.slot() == mask.slot() || right_b.slot() == mask.slot())
                && is_known_not_undef(mask, query)?
            {
                return Ok(true);
            }
        }
    }

    // X op (Y & ~X).
    if let Some((right_a, right_b)) = and_pair(rhs) {
        for side in [right_a, right_b] {
            if not_operand(side).is_some_and(|inner| inner.slot() == lhs.slot())
                && is_known_not_undef(lhs, query)?
            {
                return Ok(true);
            }
        }
    }

    // X op ((X & Y) ^ Y) — the canonical form of the previous pattern for a
    // constant Y.
    if let Some((xor_a, xor_b)) = binary_operands_of(rhs, BinaryOpcode::Xor) {
        for (anded, deferred) in [(xor_a, xor_b), (xor_b, xor_a)] {
            let Some((and_a, and_b)) = and_pair(anded) else {
                continue;
            };
            let matches_shape = (and_a.slot() == lhs.slot() && and_b.slot() == deferred.slot())
                || (and_b.slot() == lhs.slot() && and_a.slot() == deferred.slot());
            if matches_shape
                && is_known_not_undef(lhs, query)?
                && is_known_not_undef(deferred, query)?
            {
                return Ok(true);
            }
        }
    }

    // Peek through extends to find a `not` of the other side: (ext Y) op ext(~Y).
    if let Some(extended) = zext_or_sext_source(lhs)
        && let Some(right_source) = zext_or_sext_source(rhs)
        && not_operand(right_source).is_some_and(|inner| inner.slot() == extended.slot())
        && is_known_not_undef(extended, query)?
    {
        return Ok(true);
    }

    // Look for: (A & B) op ~(A | B).
    if let Some((a, b)) = and_pair(lhs)
        && let Some(negated) = not_operand(rhs)
        && let Some((or_a, or_b)) = binary_operands_of(negated, BinaryOpcode::Or)
        && ((or_a.slot() == a.slot() && or_b.slot() == b.slot())
            || (or_b.slot() == a.slot() && or_a.slot() == b.slot()))
        && is_known_not_undef(a, query)?
        && is_known_not_undef(b, query)?
    {
        return Ok(true);
    }

    // Look for: (X << V) op (Y >> (BitWidth - V)), or the same with the two
    // shift directions exchanged.
    Ok(
        complementary_shift_pair(lhs, rhs, BinaryOpcode::LShr, BinaryOpcode::Shl, query)
            || complementary_shift_pair(lhs, rhs, BinaryOpcode::Shl, BinaryOpcode::LShr, query),
    )
}

/// One half of the shift pattern above: `lhs` shifts by `V` in `lhs_opcode`'s
/// direction while `rhs` shifts by `R - V` in the other, with `R` at least the
/// scalar bit width.
fn complementary_shift_pair<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    lhs_opcode: BinaryOpcode,
    rhs_opcode: BinaryOpcode,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> bool {
    let Some((_, rhs_amount)) = binary_operands_of(rhs, rhs_opcode) else {
        return false;
    };
    let Some((total, shift)) = binary_operands_of(rhs_amount, BinaryOpcode::Sub) else {
        return false;
    };
    let Some(total) = argument_constant(Some(total)) else {
        return false;
    };
    let Some((_, lhs_amount)) = binary_operands_of(lhs, lhs_opcode) else {
        return false;
    };
    if lhs_amount.slot() != shift.slot() {
        return false;
    }
    let Some(width) = value_bit_width(lhs, query.data_layout()) else {
        return false;
    };
    total.uge(&ApInt::from_words(total.bit_width(), &[u64::from(width)]))
}

/// Return true when `v1` and `v2` are provably different values.
///
/// Ports `llvm::isKnownNonEqual` (`ValueTracking.cpp`).
///
/// Two of upstream's arms are **not** ported, each because it reads something
/// llvmkit does not model, and each omission only makes the answer weaker:
/// `isNonEqualPointersWithRecursiveGEP` (needs
/// `stripAndAccumulateInBoundsConstantOffsets`) and `isKnownNonEqualFromContext`
/// (needs an `AssumptionCache`).
pub fn is_known_non_equal<'a, 'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    v2: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    is_known_non_equal_inner(v1, v2, query, 0)
}

fn is_known_non_equal_inner<'a, 'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    v2: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    if v1.slot() == v2.slot() {
        return Ok(false);
    }
    // Casts are not looked through.
    if v1.ty().id() != v2.ty().id() {
        return Ok(false);
    }
    if depth >= query.max_depth() {
        return Ok(false);
    }

    // Recurse through exactly one operand when the operation is invertible —
    // 1-to-1, mapping every input to exactly one output — because then the two
    // results are equal exactly when that operand pair is.
    if let Some((first, second)) = invertible_operands(v1, v2) {
        return is_known_non_equal_inner(first, second, query, depth + 1);
    }
    if let (Some(InstructionKindData::Phi(p1)), Some(InstructionKindData::Phi(p2))) =
        (instruction_kind(v1), instruction_kind(v2))
        && non_equal_phis(v1, p1, v2, p2, query, depth)?
    {
        return Ok(true);
    }

    if modifying_binop_of_non_zero(v1, v2, query)? || modifying_binop_of_non_zero(v2, v1, query)? {
        return Ok(true);
    }
    if non_equal_scaled(v1, v2, BinaryOpcode::Mul, query)?
        || non_equal_scaled(v2, v1, BinaryOpcode::Mul, query)?
    {
        return Ok(true);
    }
    if non_equal_scaled(v1, v2, BinaryOpcode::Shl, query)?
        || non_equal_scaled(v2, v1, BinaryOpcode::Shl, query)?
    {
        return Ok(true);
    }

    // Are any known bits in V1 contradictory to known bits in V2? If V1 has a
    // known zero where V2 has a known one, they cannot be equal.
    if matches!(
        v1.ty().kind(),
        TypeKind::Integer { .. } | TypeKind::FixedVector | TypeKind::ScalableVector
    ) {
        let known1 = compute_known_bits(v1, query)?;
        if !known1.is_unknown() {
            let known2 = compute_known_bits(v2, query)?;
            if known1.zero_mask().intersects(known2.one_mask())
                || known2.zero_mask().intersects(known1.one_mask())
            {
                return Ok(true);
            }
        }
    }

    if non_equal_select(v1, v2, query, depth)? || non_equal_select(v2, v1, query, depth)? {
        return Ok(true);
    }

    // `ptrtoint`s are non-equal when their pointers are, provided the integer
    // is exactly pointer-sized (upstream's `m_PtrToIntSameSize`).
    if let (Some(p1), Some(p2)) = (
        ptr_to_int_same_size(v1, query),
        ptr_to_int_same_size(v2, query),
    ) {
        return is_known_non_equal_inner(p1, p2, query, depth + 1);
    }

    Ok(false)
}

/// Ports `getInvertibleOperands`: when `v1` and `v2` are the same invertible
/// function, the operand pair whose equality decides theirs.
fn invertible_operands<'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    v2: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    let kind1 = instruction_kind(v1)?;
    let kind2 = instruction_kind(v2)?;
    let operand = |value: Value<'ctx, B>, slot: ValueSlot| value_from_id(value, slot);

    match (kind1, kind2) {
        // `or disjoint` behaves as `add`; a plain `or` is not invertible.
        (InstructionKindData::Or(a), InstructionKindData::Or(b)) if a.disjoint && b.disjoint => {
            invertible_commutative(v1, a, v2, b)
        }
        (InstructionKindData::Xor(a), InstructionKindData::Xor(b))
        | (InstructionKindData::Add(a), InstructionKindData::Add(b)) => {
            invertible_commutative(v1, a, v2, b)
        }
        (InstructionKindData::Sub(a), InstructionKindData::Sub(b)) => {
            if a.lhs.get() == b.lhs.get() {
                Some((operand(v1, a.rhs.get()), operand(v2, b.rhs.get())))
            } else if a.rhs.get() == b.rhs.get() {
                Some((operand(v1, a.lhs.get()), operand(v2, b.lhs.get())))
            } else {
                None
            }
        }
        // `A * B == (A * B) mod 2^N`, so a multiply is invertible when both
        // sides are no-wrap and the shared multiplier is a non-zero constant.
        (InstructionKindData::Mul(a), InstructionKindData::Mul(b)) => {
            if !no_wrap_pair(a, b) {
                return None;
            }
            let shared = a.rhs.get() == b.rhs.get();
            let non_zero_constant = argument_constant(Some(operand(v1, a.rhs.get())))
                .is_some_and(|constant| !constant.is_zero());
            (shared && non_zero_constant)
                .then(|| (operand(v1, a.lhs.get()), operand(v2, b.lhs.get())))
        }
        // As multiplies, minus the non-zero check: a shift always scales by a
        // non-zero factor.
        (InstructionKindData::Shl(a), InstructionKindData::Shl(b)) => {
            if !no_wrap_pair(a, b) {
                return None;
            }
            (a.rhs.get() == b.rhs.get())
                .then(|| (operand(v1, a.lhs.get()), operand(v2, b.lhs.get())))
        }
        (InstructionKindData::AShr(a), InstructionKindData::AShr(b))
        | (InstructionKindData::LShr(a), InstructionKindData::LShr(b)) => {
            if !(a.is_exact && b.is_exact) {
                return None;
            }
            (a.rhs.get() == b.rhs.get())
                .then(|| (operand(v1, a.lhs.get()), operand(v2, b.lhs.get())))
        }
        (InstructionKindData::Cast(a), InstructionKindData::Cast(b))
            if a.kind == b.kind && matches!(a.kind, CastOpcode::SExt | CastOpcode::ZExt) =>
        {
            let source1 = operand(v1, a.src.get());
            let source2 = operand(v2, b.src.get());
            (source1.ty().id() == source2.ty().id()).then_some((source1, source2))
        }
        (InstructionKindData::Phi(p1), InstructionKindData::Phi(p2)) => {
            invertible_recurrences(v1, p1, v2, p2)
        }
        _ => None,
    }
}

/// The commutative arm of `getInvertibleOperands` (`or disjoint` / `xor` /
/// `add`): whichever operand of `v2` matches one of `v1`'s pins the other pair.
fn invertible_commutative<'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    a: &BinaryOpData,
    v2: Value<'ctx, B>,
    b: &BinaryOpData,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    for (pinned, other) in [(a.lhs.get(), a.rhs.get()), (a.rhs.get(), a.lhs.get())] {
        if b.lhs.get() == pinned {
            return Some((value_from_id(v1, other), value_from_id(v2, b.rhs.get())));
        }
        if b.rhs.get() == pinned {
            return Some((value_from_id(v1, other), value_from_id(v2, b.lhs.get())));
        }
    }
    None
}

/// Both operators carry `nuw`, or both carry `nsw`.
fn no_wrap_pair(a: &BinaryOpData, b: &BinaryOpData) -> bool {
    (a.no_unsigned_wrap && b.no_unsigned_wrap) || (a.no_signed_wrap && b.no_signed_wrap)
}

/// The `Instruction::PHI` arm of `getInvertibleOperands`: two recurrences in
/// the same block whose increments are a single invertible function of the
/// start values.
fn invertible_recurrences<'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    p1: &PhiData,
    v2: Value<'ctx, B>,
    p2: &PhiData,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    if parent_block(v1)? != parent_block(v2)? {
        return None;
    }
    let recurrence1 = match_simple_recurrence(v1, p1, true)?;
    let recurrence2 = match_simple_recurrence(v2, p2, true)?;
    let (first, second) = invertible_operands(
        value_from_id(v1, recurrence1.increment),
        value_from_id(v2, recurrence2.increment),
    )?;

    // Mutually defined recurrences are not reasoned about: the pair the
    // increments reduce to has to be the two phis themselves.
    if first.slot() != v1.slot() || second.slot() != v2.slot() {
        return None;
    }
    Some((
        value_from_id(v1, recurrence1.start),
        value_from_id(v2, recurrence2.start),
    ))
}

/// The block an instruction belongs to.
pub(crate) fn parent_block<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<ValueSlot> {
    match &value.data().kind {
        ValueKindData::Instruction(inst) => Some(inst.parent.get()),
        _ => None,
    }
}

/// Ports `isNonEqualPHIs`.
fn non_equal_phis<'a, 'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    p1: &PhiData,
    v2: Value<'ctx, B>,
    p2: &PhiData,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    if parent_block(v1) != parent_block(v2) {
        return Ok(false);
    }
    let incoming1 = p1.incoming.borrow();
    let incoming2 = p2.incoming.borrow();
    let mut visited: HashSet<ValueSlot> = HashSet::new();
    let mut used_full_recursion = false;
    for (operand1, block) in incoming1.iter() {
        // Blocks already dealt with are not reprocessed.
        if !visited.insert(*block) {
            continue;
        }
        let Some((operand2, _)) = incoming2.iter().find(|(_, other)| other == block) else {
            return Ok(false);
        };
        let value1 = value_from_id(v1, operand1.get());
        let value2 = value_from_id(v2, operand2.get());
        if let (Some(c1), Some(c2)) = (
            argument_constant(Some(value1)),
            argument_constant(Some(value2)),
        ) && !c1.eq_ap_int(&c2)
        {
            continue;
        }

        // Only one pair of phi operands is allowed to recurse fully.
        if used_full_recursion {
            return Ok(false);
        }
        if !is_known_non_equal_inner(value1, value2, query, depth + 1)? {
            return Ok(false);
        }
        used_full_recursion = true;
    }
    Ok(true)
}

/// Ports `isModifyingBinopOfNonZero`: `v1 == (binop v2, X)` with `X` non-zero,
/// for the binops where that implies `v1 != v2`.
///
/// Upstream recurses into `isKnownNonZero` at `Depth + 1`. llvmkit's
/// `is_known_non_zero` answers from known bits and carries no depth of its own,
/// so there is nothing to thread; the recursion it would bound happens inside
/// `compute_known_bits`, which has its own limit.
fn modifying_binop_of_non_zero<'a, 'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    v2: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let Some(kind) = instruction_kind(v1) else {
        return Ok(false);
    };
    let data = match kind {
        InstructionKindData::Or(data) if data.disjoint => data,
        InstructionKindData::Xor(data) | InstructionKindData::Add(data) => data,
        _ => return Ok(false),
    };
    let other = if v2.slot() == data.lhs.get() {
        data.rhs.get()
    } else if v2.slot() == data.rhs.get() {
        data.lhs.get()
    } else {
        return Ok(false);
    };
    is_known_non_zero(value_from_id(v1, other), query)
}

/// Ports `isNonEqualMul` and `isNonEqualShl`, which differ only in opcode and
/// in whether the constant may be one: `v2 == v1 * C` (or `v1 << C`) with `v1`
/// non-zero, `C` non-trivial, and the operation no-wrap.
///
/// The depth note on `modifying_binop_of_non_zero` applies here too.
fn non_equal_scaled<'a, 'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    v2: Value<'ctx, B>,
    opcode: BinaryOpcode,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let Some((found, data)) = instruction_kind(v2).and_then(binary_operator_parts) else {
        return Ok(false);
    };
    if found != opcode || data.lhs.get() != v1.slot() {
        return Ok(false);
    }
    if !(data.no_unsigned_wrap || data.no_signed_wrap) {
        return Ok(false);
    }
    let Some(constant) = argument_constant(Some(value_from_id(v2, data.rhs.get()))) else {
        return Ok(false);
    };
    // A shift by zero is the identity, and so is a multiply by one.
    if constant.is_zero() || (opcode == BinaryOpcode::Mul && constant.is_one()) {
        return Ok(false);
    }
    is_known_non_zero(v1, query)
}

/// Ports `isNonEqualSelect`.
fn non_equal_select<'a, 'ctx, B: ModuleBrand + 'ctx>(
    v1: Value<'ctx, B>,
    v2: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    let Some(InstructionKindData::Select(s1)) = instruction_kind(v1) else {
        return Ok(false);
    };
    let true1 = value_from_id(v1, s1.true_val.get());
    let false1 = value_from_id(v1, s1.false_val.get());

    if let Some(InstructionKindData::Select(s2)) = instruction_kind(v2)
        && s1.cond.get() == s2.cond.get()
    {
        return Ok(is_known_non_equal_inner(
            true1,
            value_from_id(v2, s2.true_val.get()),
            query,
            depth + 1,
        )? && is_known_non_equal_inner(
            false1,
            value_from_id(v2, s2.false_val.get()),
            query,
            depth + 1,
        )?);
    }
    Ok(is_known_non_equal_inner(true1, v2, query, depth + 1)?
        && is_known_non_equal_inner(false1, v2, query, depth + 1)?)
}

/// Ports `m_PtrToIntSameSize`: a `ptrtoint` whose result width equals the
/// pointer's index width, returning the pointer operand.
fn ptr_to_int_same_size<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Cast(data) = instruction_kind(value)? else {
        return None;
    };
    if data.kind != CastOpcode::PtrToInt {
        return None;
    }
    let pointer = value_from_id(value, data.src.get());
    let address_space = pointer_addr_space(pointer.ty())?;
    let result_width = value_bit_width(value, query.data_layout())?;
    (result_width == query.data_layout().pointer_size_in_bits(address_space)).then_some(pointer)
}

// --------------------------------------------------------------------------
// Context-dependent known bits
// --------------------------------------------------------------------------

/// A condition a caller wants assumed while a query runs, together with the
/// values it constrains.
///
/// Ports `llvm::CondContext` (`SimplifyQuery.h`). Upstream leaves
/// `AffectedValues` for the caller to fill; [`Self::new`] fills it by running
/// [`find_values_affected_by_condition`], which is what upstream's callers do
/// immediately after constructing one.
pub struct CondContext<'ctx, B: ModuleBrand> {
    condition: Value<'ctx, B>,
    invert: bool,
    affected_values: HashSet<ValueSlot>,
}

impl<'ctx, B: ModuleBrand + 'ctx> CondContext<'ctx, B> {
    /// Assume `condition` holds, and index the values it constrains.
    pub fn new(condition: Value<'ctx, B>) -> Self {
        let mut affected_values = HashSet::new();
        find_values_affected_by_condition(condition, false, |affected| {
            affected_values.insert(affected.slot());
        });
        Self {
            condition,
            invert: false,
            affected_values,
        }
    }

    /// Assume the condition is *false* rather than true. Ports the `Invert`
    /// field, which upstream sets after construction.
    #[must_use]
    pub fn inverted(mut self) -> Self {
        self.invert = !self.invert;
        self
    }

    /// The condition being assumed.
    pub fn condition(&self) -> Value<'ctx, B> {
        self.condition
    }

    /// Whether the condition is assumed false.
    pub fn is_inverted(&self) -> bool {
        self.invert
    }

    /// Whether `value` is one of the values the condition constrains.
    pub fn affects(&self, value: Value<'ctx, B>) -> bool {
        self.affected_values.contains(&value.slot())
    }
}

/// Merge bits known from context-dependent facts into `known`.
///
/// Ports `llvm::computeKnownBitsFromContext`. Three sources feed it, each
/// attached to the query separately: an injected condition
/// ([`ValueTrackingQuery::with_condition_context`]), the dominating branch
/// conditions ([`ValueTrackingQuery::with_dominating_conditions`], which also
/// needs a dominator tree), and the `@llvm.assume` calls
/// ([`ValueTrackingQuery::with_assumptions`], which also needs a context
/// instruction). A query carrying none of them returns `known` unchanged.
///
/// Conflicting facts mean the path is unreachable; upstream resets to unknown
/// rather than propagating a contradiction, and so does this.
///
/// One arm is not ported: the operand-bundle alignment refinement, which needs
/// `getKnowledgeFromBundle` (`llvm/Analysis/AssumeBundleQueries.h`) — see the
/// [`assumptions`](crate::assumptions) module header. Its absence only leaves
/// bits unknown.
pub fn compute_known_bits_from_context<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    known: KnownBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> KnownBits {
    known_bits_from_context(value, known, query, 0)
}

/// Ports `computeKnownBitsFromContext` at an explicit recursion depth.
fn known_bits_from_context<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    known: KnownBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownBits {
    let mut known = known;

    // Handle the injected condition.
    if let Some(context) = query.condition_context()
        && context.affects(value)
    {
        known = known_bits_from_cond(
            value,
            context.condition(),
            known,
            query,
            context.is_inverted(),
            depth,
        );
    }

    let Some(context_instruction) = query.context_instruction() else {
        return known;
    };

    // Handle dominating conditions.
    if let (Some(cache), Some(dominator_tree), Some(context_block)) = (
        query.dominating_conditions(),
        query.dominator_tree(),
        parent_block(context_instruction),
    ) {
        for branch in cache.conditions_for(value) {
            let Some(InstructionKindData::Br(data)) = instruction_kind(branch) else {
                continue;
            };
            let (condition, then_block, else_block) = match &*data.kind.borrow() {
                BranchKind::Unconditional(_) => continue,
                BranchKind::Conditional {
                    cond,
                    then_bb,
                    else_bb,
                } => (cond.get(), *then_bb, *else_bb),
            };
            let Some(branch_block) = parent_block(branch) else {
                continue;
            };
            let condition = value_from_id(branch, condition);
            for (successor, invert) in [(then_block, false), (else_block, true)] {
                if dominator_tree.dominates_edge_slots(branch_block, successor, context_block) {
                    known = known_bits_from_cond(value, condition, known, query, invert, depth);
                }
            }
        }

        if known.has_conflict() {
            known = KnownBits::unknown(known.bit_width());
        }
    }

    let Some(cache) = query.assumptions() else {
        return known;
    };
    let Ok(context_view) = InstructionView::try_from(context_instruction) else {
        return known;
    };
    let bit_width = known.bit_width();
    let valid_here = |assume: &InstructionView<'ctx, B>| {
        is_valid_assume_for_context(assume, &context_view, query.dominator_tree(), false)
    };

    // Note: the patterns below must be kept in sync with
    // `find_values_affected_by_condition`, which is what filled the cache.
    for assumption in cache.assumptions_for(value) {
        // The operand-bundle half needs `getKnowledgeFromBundle`, which is not
        // ported; the index is still recorded, so the arm can be filled in
        // without re-scanning.
        if assumption.source() != AssumptionSource::Condition {
            continue;
        }
        let Some(assume) = assumption.assume(module_ref(value)) else {
            continue;
        };
        let Some(argument) = assume_argument(assume.to_erased()) else {
            continue;
        };

        // Upstream asserts the operand is `i1` in the first three arms.
        if argument == value && valid_here(&assume) {
            known.set_all_ones();
            return known;
        }
        if not_operand(argument) == Some(value) && valid_here(&assume) {
            known.set_all_zero();
            return known;
        }
        if let Some((source, no_unsigned_wrap)) = trunc_source_and_no_unsigned_wrap(argument)
            && source == value
            && valid_here(&assume)
        {
            if no_unsigned_wrap {
                return KnownBits::make_constant(ApInt::one_bit_set(bit_width, 0));
            }
            known.set_known_one_bit(0);
            return known;
        }

        // The remaining tests are all recursive, so bail out at the limit.
        if depth == query.max_depth() {
            continue;
        }
        if !matches!(
            instruction_kind(argument),
            Some(InstructionKindData::ICmp(_))
        ) {
            continue;
        }
        if !valid_here(&assume) {
            continue;
        }
        known = known_bits_from_int_compare_cond(value, argument, known, query, false);
    }

    // A conflicting assumption means undefined behaviour on this path.
    if known.has_conflict() {
        known = KnownBits::unknown(known.bit_width());
    }
    known
}

/// Adjust `known` for the select arm `arm` with what `condition` implies.
///
/// Ports `llvm::adjustKnownBitsForSelectArm`. `invert` picks the false arm.
///
/// Upstream's comment on the conflict case is inherited: `(x | 64) < 32 ? (x |
/// 64) : y` contradicts itself at bit 6, and the select is about to be
/// simplified away, so the original `known` comes back rather than a
/// contradiction.
pub fn adjust_known_bits_for_select_arm<'a, 'ctx, B: ModuleBrand + 'ctx>(
    known: KnownBits,
    condition: Value<'ctx, B>,
    arm: Value<'ctx, B>,
    invert: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<KnownBits> {
    // A constant arm is already as good as it gets.
    if known.is_constant() {
        return Ok(known);
    }

    // See what the condition implies about the bits of the arm.
    let from_condition = known_bits_from_cond(
        arm,
        condition,
        KnownBits::unknown(known.bit_width()),
        query,
        invert,
        1,
    );
    if from_condition.is_unknown() {
        return Ok(known);
    }

    let merged = from_condition.union_with(&known);
    if merged.has_conflict() {
        return Ok(known);
    }

    // Make sure what was found is valid. Relatively expensive, so left last.
    if !is_known_not_undef(arm, query)? {
        return Ok(known);
    }
    Ok(merged)
}

/// Ports `computeKnownBitsFromCond`.
fn known_bits_from_cond<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    condition: Value<'ctx, B>,
    known: KnownBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    invert: bool,
    depth: u32,
) -> KnownBits {
    let mut known = known;
    let bit_width = known.bit_width();

    if depth < query.max_depth()
        && let Some((a, b, is_and)) = logical_op_parts(condition)
    {
        let from_a = known_bits_from_cond(
            value,
            a,
            KnownBits::unknown(bit_width),
            query,
            invert,
            depth + 1,
        );
        let from_b = known_bits_from_cond(
            value,
            b,
            KnownBits::unknown(bit_width),
            query,
            invert,
            depth + 1,
        );
        // An assumed `and`, or an inverted `or`, gives both legs; the other way
        // round, only what the two legs agree on.
        let combined = if invert == is_and {
            from_a.intersect_with(&from_b)
        } else {
            from_a.union_with(&from_b)
        };
        return known.union_with(&combined);
    }

    if matches!(
        instruction_kind(condition),
        Some(InstructionKindData::ICmp(_))
    ) {
        return known_bits_from_int_compare_cond(value, condition, known, query, invert);
    }

    if let Some((source, no_unsigned_wrap)) = trunc_source_and_no_unsigned_wrap(condition)
        && source == value
    {
        let mut destination = KnownBits::unknown(1);
        if invert {
            destination.set_all_zero();
        } else {
            destination.set_all_ones();
        }
        let extended = if no_unsigned_wrap {
            destination.zext(bit_width)
        } else {
            destination.anyext(bit_width)
        };
        return known.union_with(&extended);
    }

    if depth < query.max_depth()
        && let Some(inner) = not_operand(condition)
    {
        known = known_bits_from_cond(value, inner, known, query, !invert, depth + 1);
    }
    known
}

/// Ports `computeKnownBitsFromICmpCond`.
fn known_bits_from_int_compare_cond<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    compare: Value<'ctx, B>,
    known: KnownBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    invert: bool,
) -> KnownBits {
    let Some(InstructionKindData::ICmp(data)) = instruction_kind(compare) else {
        return known;
    };
    let predicate = if invert {
        data.predicate.inverse()
    } else {
        data.predicate
    };
    let lhs = value_from_id(compare, data.lhs.get());
    let rhs = value_from_id(compare, data.rhs.get());

    // Handle `icmp pred (trunc V), C`.
    if let Some((source, no_unsigned_wrap)) = trunc_source_and_no_unsigned_wrap(lhs)
        && source == value
    {
        let destination_width = value_bit_width(lhs, query.data_layout()).unwrap_or(0);
        let destination = known_bits_from_compare(
            lhs,
            predicate,
            lhs,
            rhs,
            KnownBits::unknown(destination_width),
            query,
        );
        let extended = if no_unsigned_wrap {
            destination.zext(known.bit_width())
        } else {
            destination.anyext(known.bit_width())
        };
        return known.union_with(&extended);
    }

    known_bits_from_compare(value, predicate, lhs, rhs, known, query)
}

/// Ports `computeKnownBitsFromCmp`.
fn known_bits_from_compare<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    predicate: IntPredicate,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    known: KnownBits,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> KnownBits {
    let mut known = known;

    // A pointer compared against null is not covered by the integer logic
    // below, so upstream gives it its own arm and returns.
    if matches!(rhs.ty().kind(), TypeKind::Pointer { .. }) {
        if lhs == value && is_null_pointer(rhs) {
            match predicate {
                IntPredicate::Eq => known.set_all_zero(),
                IntPredicate::Sge | IntPredicate::Sgt => known.make_non_negative(),
                IntPredicate::Slt => known.make_negative(),
                _ => {}
            }
        }
        return known;
    }

    let bit_width = known.bit_width();
    // Upstream's `m_V` is `m_CombineOr(m_Specific(V), m_PtrToIntSameSize(DL, m_Specific(V)))`.
    let is_v = |candidate: Value<'ctx, B>| -> bool {
        candidate == value || ptr_to_int_same_size(candidate, query) == Some(value)
    };
    let Some(constant) = argument_constant(Some(rhs)) else {
        return known;
    };

    match predicate {
        IntPredicate::Eq => {
            if is_v(lhs) {
                // assume(V = C)
                known = known.union_with(&KnownBits::make_constant(constant));
            } else if let Some(other) = commutative_operand_beside(lhs, BitwiseOp::And, &is_v) {
                // assume(V & Mask = C): one bits in Mask carry C's bits to V.
                known.add_known_one_bits(&constant);
                if let Some(mask) = argument_constant(Some(other)) {
                    known.add_known_zero_bits(&(!constant).bitand(&mask));
                }
            } else if let Some(other) = commutative_operand_beside(lhs, BitwiseOp::Or, &is_v) {
                // assume(V | Mask = C): zero bits in Mask carry C's bits to V.
                known.add_known_zero_bits(&!constant.clone());
                if let Some(mask) = argument_constant(Some(other)) {
                    known.add_known_one_bits(&constant.bitand(&!mask));
                }
            } else if let Some(amount) = shift_of_by_constant(lhs, &is_v, ShiftDirection::Left)
                && amount < bit_width
            {
                // assume(V << ShAmt = C): C's known bits move right by ShAmt.
                let mut shifted = KnownBits::make_constant(constant);
                shifted >>= amount;
                known = known.union_with(&shifted);
            } else if let Some(amount) = shift_of_by_constant(lhs, &is_v, ShiftDirection::Right)
                && amount < bit_width
            {
                // assume(V >> ShAmt = C): C's known bits move left by ShAmt.
                let mut shifted = KnownBits::make_constant(constant);
                shifted <<= amount;
                known = known.union_with(&shifted);
            }
        }
        IntPredicate::Ne => {
            // assume(V & B != 0) where B is a power of two. Upstream writes
            // `m_And`, not `m_c_And`, so V must be the left operand.
            if constant.is_zero()
                && let Some(InstructionKindData::And(data)) = instruction_kind(lhs)
                && is_v(value_from_id(lhs, data.lhs.get()))
                && let Some(mask) = argument_constant(Some(value_from_id(lhs, data.rhs.get())))
                && mask.is_power_of_2()
            {
                known.add_known_one_bits(&mask);
            }
        }
        _ => {
            let offset = add_like_offset_beside(lhs, &is_v);
            if is_v(lhs) || offset.is_some() {
                let mut range = ConstantRange::make_allowed_icmp_region(
                    predicate,
                    &ConstantRange::single(constant.clone()),
                );
                if let Some(offset) = &offset {
                    range = range.sub(&ConstantRange::single(offset.clone()));
                }
                known = known.union_with(&range.to_known_bits());
            }
            if matches!(predicate, IntPredicate::Ugt | IntPredicate::Uge) {
                // X & Y u> C -> X u> C && Y u> C; X nuw- Y u> C -> X u> C.
                if commutative_operand_beside(lhs, BitwiseOp::And, &is_v).is_some()
                    || no_wrap_sub_beside(lhs, &is_v, WrapKind::NoUnsignedWrap)
                {
                    let bumped = if predicate == IntPredicate::Ugt {
                        constant.wrapping_add(&ApInt::one_bit_set(bit_width, 0))
                    } else {
                        constant.clone()
                    };
                    known.mark_high_bits_one(bumped.count_leading_ones());
                }
            }
            if matches!(predicate, IntPredicate::Ult | IntPredicate::Ule) {
                // X | Y u< C -> X u< C && Y u< C; X nuw+ Y u< C likewise.
                if commutative_operand_beside(lhs, BitwiseOp::Or, &is_v).is_some()
                    || no_wrap_add_beside(lhs, &is_v)
                {
                    let lowered = if predicate == IntPredicate::Ult {
                        constant.wrapping_sub(&ApInt::one_bit_set(bit_width, 0))
                    } else {
                        constant.clone()
                    };
                    known.mark_high_bits_zero(lowered.count_leading_zeros());
                }
            }
        }
    }
    known
}

/// Which bitwise operator [`commutative_operand_beside`] is looking for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BitwiseOp {
    And,
    Or,
}

/// Which direction [`shift_of_by_constant`] is looking for. `Right` covers both
/// `lshr` and `ashr`, matching upstream's `m_Shr`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShiftDirection {
    Left,
    Right,
}

/// Which no-wrap flag a matcher requires.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WrapKind {
    NoUnsignedWrap,
}

/// The other operand when `value` is `and`/`or` with one operand accepted by
/// `is_wanted`. Ports `m_c_And(m_V, m_Value(Y))` and its `or` twin.
fn commutative_operand_beside<'ctx, B, F>(
    value: Value<'ctx, B>,
    op: BitwiseOp,
    is_wanted: &F,
) -> Option<Value<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
    F: Fn(Value<'ctx, B>) -> bool,
{
    let data = match (instruction_kind(value)?, op) {
        (InstructionKindData::And(data), BitwiseOp::And) => data,
        (InstructionKindData::Or(data), BitwiseOp::Or) => data,
        _ => return None,
    };
    let lhs = value_from_id(value, data.lhs.get());
    let rhs = value_from_id(value, data.rhs.get());
    if is_wanted(lhs) {
        return Some(rhs);
    }
    is_wanted(rhs).then_some(lhs)
}

/// The shift amount when `value` shifts an operand accepted by `is_wanted` by a
/// constant. Ports `m_Shl(m_V, m_ConstantInt(ShAmt))` and `m_Shr`.
fn shift_of_by_constant<'ctx, B, F>(
    value: Value<'ctx, B>,
    is_wanted: &F,
    direction: ShiftDirection,
) -> Option<u32>
where
    B: ModuleBrand + 'ctx,
    F: Fn(Value<'ctx, B>) -> bool,
{
    let data = match (instruction_kind(value)?, direction) {
        (InstructionKindData::Shl(data), ShiftDirection::Left) => data,
        (
            InstructionKindData::LShr(data) | InstructionKindData::AShr(data),
            ShiftDirection::Right,
        ) => data,
        _ => return None,
    };
    if !is_wanted(value_from_id(value, data.lhs.get())) {
        return None;
    }
    let amount = argument_constant(Some(value_from_id(value, data.rhs.get())))?;
    // Upstream's `m_ConstantInt(ShAmt)` binds a `uint64_t`; the callers compare
    // it against the bit width, so saturating at `u32::MAX` cannot flip an arm.
    u32::try_from(amount.limited_value(u64::from(u32::MAX))).ok()
}

/// The constant offset when `value` is `add`/`or disjoint` of an operand
/// accepted by `is_wanted` and a constant. Ports
/// `m_AddLike(m_V, m_APInt(Offset))`.
fn add_like_offset_beside<'ctx, B, F>(value: Value<'ctx, B>, is_wanted: &F) -> Option<ApInt>
where
    B: ModuleBrand + 'ctx,
    F: Fn(Value<'ctx, B>) -> bool,
{
    let data = match instruction_kind(value)? {
        InstructionKindData::Add(data) => data,
        InstructionKindData::Or(data) if data.disjoint => data,
        _ => return None,
    };
    is_wanted(value_from_id(value, data.lhs.get()))
        .then(|| argument_constant(Some(value_from_id(value, data.rhs.get()))))?
}

/// Whether `value` is `sub nuw` with an operand accepted by `is_wanted` on the
/// left. Ports `m_NUWSub(m_V, m_Value())`.
fn no_wrap_sub_beside<'ctx, B, F>(value: Value<'ctx, B>, is_wanted: &F, wrap: WrapKind) -> bool
where
    B: ModuleBrand + 'ctx,
    F: Fn(Value<'ctx, B>) -> bool,
{
    let Some(InstructionKindData::Sub(data)) = instruction_kind(value) else {
        return false;
    };
    let flagged = match wrap {
        WrapKind::NoUnsignedWrap => data.no_unsigned_wrap,
    };
    flagged && is_wanted(value_from_id(value, data.lhs.get()))
}

/// Whether `value` is `add nuw` with an operand accepted by `is_wanted` on
/// either side. Ports `m_c_NUWAdd(m_V, m_Value())`.
fn no_wrap_add_beside<'ctx, B, F>(value: Value<'ctx, B>, is_wanted: &F) -> bool
where
    B: ModuleBrand + 'ctx,
    F: Fn(Value<'ctx, B>) -> bool,
{
    let Some(InstructionKindData::Add(data)) = instruction_kind(value) else {
        return false;
    };
    data.no_unsigned_wrap
        && (is_wanted(value_from_id(value, data.lhs.get()))
            || is_wanted(value_from_id(value, data.rhs.get())))
}

/// The two operands of a logical `and`/`or`, and which of the two it is.
///
/// Ports `m_LogicalOp`: the bitwise spelling on an `i1`, or the poison-blocking
/// `select` spelling — `L ? R : false` for `and`, `L ? true : R` for `or`.
pub(crate) fn logical_op_parts<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>, bool)> {
    if !matches!(scalar_type_kind(value), Some(TypeKind::Integer { bits: 1 })) {
        return None;
    }
    match instruction_kind(value)? {
        InstructionKindData::And(data) => Some((
            value_from_id(value, data.lhs.get()),
            value_from_id(value, data.rhs.get()),
            true,
        )),
        InstructionKindData::Or(data) => Some((
            value_from_id(value, data.lhs.get()),
            value_from_id(value, data.rhs.get()),
            false,
        )),
        InstructionKindData::Select(data) => {
            let condition = value_from_id(value, data.cond.get());
            // Don't match a scalar select of bool vectors.
            if condition.ty().id() != value.ty().id() {
                return None;
            }
            let true_value = value_from_id(value, data.true_val.get());
            let false_value = value_from_id(value, data.false_val.get());
            if argument_constant(Some(false_value)).is_some_and(|c| c.is_zero()) {
                return Some((condition, true_value, true));
            }
            argument_constant(Some(true_value))
                .is_some_and(|c| c.is_all_ones())
                .then_some((condition, false_value, false))
        }
        _ => None,
    }
}

/// The source of a `trunc` and whether it carries `nuw`. Ports the
/// `dyn_cast<TruncInst>` / `hasNoUnsignedWrap` pair.
fn trunc_source_and_no_unsigned_wrap<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, bool)> {
    let InstructionKindData::Cast(data) = instruction_kind(value)? else {
        return None;
    };
    (data.kind == CastOpcode::Trunc).then(|| (value_from_id(value, data.src.get()), data.nuw.get()))
}

/// The condition operand of an `@llvm.assume`.
pub(crate) fn assume_argument<'ctx, B: ModuleBrand + 'ctx>(
    assume: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Call(data) = instruction_kind(assume)? else {
        return None;
    };
    Some(value_from_id(assume, data.args.first()?.get()))
}

/// Whether `value` is the null pointer constant. Ports `m_Zero` at pointer type.
fn is_null_pointer<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    matches!(
        value.data().kind,
        ValueKindData::Constant(ConstantData::PointerNull)
    )
}

/// The kind of the value's scalar type, peeling one vector layer.
fn scalar_type_kind<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<TypeKind> {
    let ty = value.ty();
    Some(match ty.data().as_vector() {
        Some((element, _, _)) => Type::new(element, ty.module()).kind(),
        None => ty.kind(),
    })
}

// --------------------------------------------------------------------------
// Undef / poison reasoning
// --------------------------------------------------------------------------

/// Which of undef and poison a query is asking about. Ports upstream's
/// `UndefPoisonKind` (`ValueTracking.cpp`).
///
/// Upstream spells this as a bitmask (`PoisonOnly = 1 << 0`,
/// `UndefOnly = 1 << 1`, `UndefOrPoison = PoisonOnly | UndefOnly`) and reads it
/// back through `includesPoison` / `includesUndef`. Only three of the four bit
/// patterns are ever constructed, so llvmkit spells the same thing as a
/// three-variant enum and the two readers as methods — the empty mask is
/// unrepresentable rather than merely unused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UndefPoisonKind {
    PoisonOnly,
    UndefOnly,
    UndefOrPoison,
}

impl UndefPoisonKind {
    /// Ports `includesPoison`.
    fn includes_poison(self) -> bool {
        matches!(self, Self::PoisonOnly | Self::UndefOrPoison)
    }

    /// Ports `includesUndef`.
    fn includes_undef(self) -> bool {
        matches!(self, Self::UndefOnly | Self::UndefOrPoison)
    }
}

/// Return true when the operator can *create* poison that its operands did not
/// carry. Ports `llvm::canCreatePoison`.
///
/// `consider_flags_and_metadata` mirrors upstream's parameter: with it set, an
/// operator carrying a poison-generating annotation (`nsw`, `nuw`, `exact`,
/// `inbounds`, `nneg`, `disjoint`) answers true on that basis alone.
pub fn can_create_poison<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    consider_flags_and_metadata: bool,
) -> bool {
    can_create_undef_or_poison_kind(
        value,
        UndefPoisonKind::PoisonOnly,
        consider_flags_and_metadata,
    )
}

/// Return true when the operator can create undef *or* poison. Ports
/// `llvm::canCreateUndefOrPoison`.
pub fn can_create_undef_or_poison<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    consider_flags_and_metadata: bool,
) -> bool {
    can_create_undef_or_poison_kind(
        value,
        UndefPoisonKind::UndefOrPoison,
        consider_flags_and_metadata,
    )
}

/// Ports the static `canCreateUndefOrPoison(Op, Kind, ConsiderFlagsAndMetadata)`.
fn can_create_undef_or_poison_kind<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    kind: UndefPoisonKind,
    consider_flags_and_metadata: bool,
) -> bool {
    let ValueKindData::Instruction(inst) = &value.data().kind else {
        // Upstream reaches this through `dyn_cast<Operator>`; a non-operator is
        // not an operator that can create anything.
        return false;
    };

    if consider_flags_and_metadata
        && kind.includes_poison()
        && has_poison_generating_annotations(&inst.kind)
    {
        return true;
    }

    match &inst.kind {
        // Shifts are poison when the amount is out of range.
        InstructionKindData::Shl(data)
        | InstructionKindData::AShr(data)
        | InstructionKindData::LShr(data) => {
            kind.includes_poison()
                && !shift_amount_known_in_range(value_from_id(value, data.rhs.get()))
        }

        // fptosi/fptoui yield poison when the value does not fit the
        // destination type.
        InstructionKindData::Cast(data)
            if matches!(data.kind, CastOpcode::FpToSI | CastOpcode::FpToUI) =>
        {
            true
        }

        // addrspacecast can create poison; every other cast cannot. Upstream
        // reaches the latter through the `isa<CastInst>` test in `default`.
        InstructionKindData::Cast(data) => data.kind == CastOpcode::AddrSpaceCast,

        // Upstream returns true unless the call is annotated `noundef` on its
        // return, or `nocreateundeforpoison` on the callee. llvmkit does not
        // model `nocreateundeforpoison`, so only the `noundef` half is
        // consulted; the effect is that some calls answer true where upstream
        // would answer false — conservative in the safe direction.
        call @ (InstructionKindData::Call(_)
        | InstructionKindData::Invoke(_)
        | InstructionKindData::CallBr(_)) => !call_returns_noundef(call),

        // Out-of-range lane indices give poison.
        InstructionKindData::ExtractElement(data) => {
            kind.includes_poison()
                && !lane_index_known_in_range(
                    value_from_id(value, data.vector.get()),
                    value_from_id(value, data.index.get()),
                )
        }
        InstructionKindData::InsertElement(data) => {
            kind.includes_poison()
                && !lane_index_known_in_range(
                    value_from_id(value, data.vector.get()),
                    value_from_id(value, data.index.get()),
                )
        }

        // A poison mask element creates poison.
        InstructionKindData::ShuffleVector(data) => {
            kind.includes_poison() && data.mask.contains(&POISON_MASK_ELEM)
        }

        // These never create undef or poison of their own.
        InstructionKindData::FNeg(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::Select(_)
        | InstructionKindData::ExtractValue(_)
        | InstructionKindData::InsertValue(_)
        | InstructionKindData::Freeze(_)
        | InstructionKindData::ICmp(_)
        | InstructionKindData::FCmp(_)
        | InstructionKindData::Gep(_) => false,

        // Upstream's `default`: a binary operator cannot create undef or
        // poison on its own (its flags are handled above); anything else is
        // conservatively assumed to.
        other => !is_binary_operator_kind(other),
    }
}

/// Ports `Operator::hasPoisonGeneratingAnnotations` for the flags llvmkit
/// models. The metadata half (`!range`, `!nonnull`, `!align`) is not consulted
/// because those attach to loads and calls, whose arms already answer
/// conservatively.
fn has_poison_generating_annotations(kind: &InstructionKindData) -> bool {
    match kind {
        InstructionKindData::Add(data)
        | InstructionKindData::Sub(data)
        | InstructionKindData::Mul(data)
        | InstructionKindData::Shl(data) => data.no_signed_wrap || data.no_unsigned_wrap,
        InstructionKindData::UDiv(data)
        | InstructionKindData::SDiv(data)
        | InstructionKindData::LShr(data)
        | InstructionKindData::AShr(data) => data.is_exact,
        InstructionKindData::Or(data) => data.disjoint,
        InstructionKindData::Gep(data) => !data.flags.is_empty(),
        InstructionKindData::Cast(data) => data.nneg.get() || data.nuw.get(),
        _ => false,
    }
}

/// Ports `shiftAmountKnownInRange`: a constant shift amount strictly below the
/// bit width. A non-constant amount answers false.
fn shift_amount_known_in_range<'ctx, B: ModuleBrand + 'ctx>(shift_amount: Value<'ctx, B>) -> bool {
    let data_layout = shift_amount.module().data_layout();
    let Some(width) = value_bit_width(shift_amount, &data_layout) else {
        return false;
    };
    argument_constant(Some(shift_amount))
        .is_some_and(|amount| amount.limited_value(u64::from(width)) < u64::from(width))
}

/// True when `index` is a constant lane index within `vector`'s element count.
/// Ports the `InsertElement` / `ExtractElement` arm's bounds test.
///
/// A scalable vector answers false: upstream compares against
/// `getKnownMinValue()`, which cannot bound the real length, and llvmkit
/// declines rather than guessing.
fn lane_index_known_in_range<'ctx, B: ModuleBrand + 'ctx>(
    vector: Value<'ctx, B>,
    index: Value<'ctx, B>,
) -> bool {
    if vector.ty().kind() != TypeKind::FixedVector {
        return false;
    }
    let Some((_, lanes, _)) = vector.ty().data().as_vector() else {
        return false;
    };
    argument_constant(Some(index))
        .is_some_and(|idx| idx.limited_value(u64::from(lanes)) < u64::from(lanes))
}

/// True when the call/invoke/callbr's return carries `noundef`.
fn call_returns_noundef(kind: &InstructionKindData) -> bool {
    call_return_attrs(kind).is_some_and(|stored| {
        stored
            .iter()
            .any(|attr| matches!(attr, AttributeStored::Enum(AttrKind::NoUndef)))
    })
}

/// True when the call/invoke/callbr's return carries `noundef`,
/// `dereferenceable` or `dereferenceable_or_null`.
///
/// Ports the `dyn_cast<CallBase>` arm of `isGuaranteedNotToBeUndefOrPoison`,
/// which accepts all three because the two dereferenceability attributes imply
/// `noundef`. [`call_returns_noundef`] is the narrower `canCreateUndefOrPoison`
/// test, which reads only the first.
fn call_return_is_well_defined(kind: &InstructionKindData) -> bool {
    call_return_attrs(kind).is_some_and(|stored| stored.iter().any(is_well_defined_attribute))
}

/// The return-position attributes of a call/invoke/callbr.
fn call_return_attrs(kind: &InstructionKindData) -> Option<&[AttributeStored]> {
    let attrs = match kind {
        InstructionKindData::Call(data) => &data.attrs,
        InstructionKindData::Invoke(data) => &data.attrs,
        InstructionKindData::CallBr(data) => &data.attrs,
        _ => return None,
    };
    attrs.return_attrs().get(AttrIndex::Return)
}

/// Ports `llvm::propagatesPoison(const Use &)`: does poison in the
/// `operand_index`-th operand of `user` make the result poison?
///
/// Upstream takes a `Use`, which names both the user and the operand position.
/// llvmkit has no `Use` type — operands are read positionally — so the pair is
/// spelled out.
pub fn propagates_poison<'ctx, B: ModuleBrand + 'ctx>(
    user: Value<'ctx, B>,
    operand_index: usize,
) -> bool {
    let ValueKindData::Instruction(inst) = &user.data().kind else {
        return false;
    };
    match &inst.kind {
        InstructionKindData::Freeze(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::Invoke(_) => false,
        // Only the condition propagates; an unselected arm's poison does not.
        InstructionKindData::Select(_) => operand_index == 0,
        InstructionKindData::Call(_) => false,
        InstructionKindData::ICmp(_)
        | InstructionKindData::FCmp(_)
        | InstructionKindData::Gep(_) => true,
        // Upstream's `default`: binary, unary and cast operators propagate.
        other => {
            is_binary_operator_kind(other)
                || matches!(
                    other,
                    InstructionKindData::FNeg(_) | InstructionKindData::Cast(_)
                )
        }
    }
}

/// True when `kind` is one of LLVM's `BinaryOperator` opcodes.
fn is_binary_operator_kind(kind: &InstructionKindData) -> bool {
    binary_operator_parts(kind).is_some()
}

/// Return true when poison in `value_assumed_poison` implies poison in
/// `value`. Ports `llvm::impliesPoison`.
pub fn implies_poison<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value_assumed_poison: Value<'ctx, B>,
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    implies_poison_inner(value_assumed_poison, value, query, 0)
}

/// Ports the static `impliesPoison(ValAssumedPoison, V, Depth)`.
fn implies_poison_inner<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value_assumed_poison: Value<'ctx, B>,
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<bool> {
    // Upstream's `MaxDepth` here is 2, not the analysis-wide limit.
    const MAX_DEPTH: u32 = 2;

    let mut stack = HashSet::new();
    if is_guaranteed_not_to_be_undef_or_poison(
        value_assumed_poison,
        query,
        0,
        &mut stack,
        UndefPoisonKind::PoisonOnly,
    )? {
        return Ok(true);
    }
    if directly_implies_poison(value_assumed_poison, value, 0) {
        return Ok(true);
    }
    if depth >= MAX_DEPTH {
        return Ok(false);
    }

    // If the assumed-poison value cannot create poison itself, then it is
    // poison only because an operand is, so recurse into every operand.
    let ValueKindData::Instruction(_) = &value_assumed_poison.data().kind else {
        return Ok(false);
    };
    if can_create_poison(value_assumed_poison, true) {
        return Ok(false);
    }
    for operand in operands_of(value_assumed_poison) {
        if !implies_poison_inner(operand, value, query, depth + 1)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Ports `directlyImpliesPoison`.
///
/// The `extractvalue` / `WithOverflowInst` arm is not ported: llvmkit does not
/// model the overflow intrinsics as a distinct instruction class, so the
/// pattern it matches cannot arise. Its absence only makes the answer weaker.
fn directly_implies_poison<'ctx, B: ModuleBrand + 'ctx>(
    value_assumed_poison: Value<'ctx, B>,
    value: Value<'ctx, B>,
    depth: u32,
) -> bool {
    if value_assumed_poison.slot() == value.slot() {
        return true;
    }
    const MAX_DEPTH: u32 = 2;
    if depth >= MAX_DEPTH {
        return false;
    }
    let ValueKindData::Instruction(_) = &value.data().kind else {
        return false;
    };
    operands_of(value)
        .into_iter()
        .enumerate()
        .any(|(index, operand)| {
            propagates_poison(value, index)
                && directly_implies_poison(value_assumed_poison, operand, depth + 1)
        })
}

/// The SSA operands of `value`, as values. Empty for a non-instruction.
fn operands_of<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Vec<Value<'ctx, B>> {
    match &value.data().kind {
        ValueKindData::Instruction(inst) => inst
            .kind
            .operand_ids()
            .into_iter()
            .map(|slot| value_from_id(value, slot))
            .collect(),
        _ => Vec::new(),
    }
}

/// Return true when `value` is guaranteed to be neither poison nor a value
/// derived from poison. Ports `llvm::isGuaranteedNotToBePoison`.
pub fn is_known_not_poison<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let mut stack = HashSet::new();
    is_guaranteed_not_to_be_undef_or_poison(
        value,
        query,
        0,
        &mut stack,
        UndefPoisonKind::PoisonOnly,
    )
}

/// Return true when `value` is guaranteed not to be undef.
/// Ports `llvm::isGuaranteedNotToBeUndef`.
pub fn is_known_not_undef<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let mut stack = HashSet::new();
    is_guaranteed_not_to_be_undef_or_poison(value, query, 0, &mut stack, UndefPoisonKind::UndefOnly)
}

/// Return true when `value` is guaranteed to be neither undef nor poison.
/// Ports `llvm::isGuaranteedNotToBeUndefOrPoison`.
pub fn is_known_not_undef_or_poison<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<bool> {
    let mut stack = HashSet::new();
    is_guaranteed_not_to_be_undef_or_poison(
        value,
        query,
        0,
        &mut stack,
        UndefPoisonKind::UndefOrPoison,
    )
}

// --------------------------------------------------------------------------
// Constant ranges and overflow prediction
// --------------------------------------------------------------------------

/// The range of values `value` can take. Ports `llvm::computeConstantRange`.
///
/// `for_signed` selects the domain the range must not wrap in, which changes
/// which of two equally-correct answers is returned for a value whose sign is
/// unknown.
///
/// Three of upstream's sources are not consulted, each because llvmkit does
/// not model the input rather than because the reasoning was skipped: the
/// `@llvm.assume`-driven refinement (no `AssumptionCache`), the select-pattern
/// clamp (no `SelectPatternResult` — tranche 4), and `!range` metadata on
/// `call` returns. Each omission only widens the answer.
pub fn compute_constant_range<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    for_signed: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<ConstantRange> {
    compute_constant_range_inner(value, for_signed, query, 0)
}

fn compute_constant_range_inner<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    for_signed: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<ConstantRange> {
    let width = value_bit_width(value, query.data_layout()).unwrap_or(0);
    if depth >= query.max_depth() {
        return Ok(ConstantRange::full(width));
    }

    // A constant is its own one-element range.
    if let Some(constant) = argument_constant(Some(value)) {
        return Ok(ConstantRange::single(constant));
    }

    let mut range = ConstantRange::full(width);

    if let ValueKindData::Instruction(inst) = &value.data().kind {
        match &inst.kind {
            // A select is the union of its arms. Upstream additionally
            // intersects with `getRangeForSelectPattern`, which needs the
            // select-pattern matcher of tranche 4.
            InstructionKindData::Select(data) => {
                let true_range = compute_constant_range_inner(
                    value_from_id(value, data.true_val.get()),
                    for_signed,
                    query,
                    depth + 1,
                )?;
                let false_range = compute_constant_range_inner(
                    value_from_id(value, data.false_val.get()),
                    for_signed,
                    query,
                    depth + 1,
                )?;
                range = true_range.union_with(&false_range, preferred_for(for_signed));
            }
            _ => {
                // Everything else is reached through its known bits below,
                // which is where upstream's `setLimitsForBinOp` reasoning
                // lands for llvmkit.
            }
        }
    }

    // Upstream intersects `!range` metadata here. llvmkit already reads that
    // metadata inside `compute_known_bits`, so
    // [`compute_constant_range_including_known_bits`] — the form every caller
    // in this module uses — picks it up through the known-bits half rather
    // than twice.
    Ok(range)
}

/// The range of `value`, refined by its known bits. Ports
/// `llvm::computeConstantRangeIncludingKnownBits`.
///
/// The two sources are independent — known bits can pin bits a range cannot
/// express, and a range can bound values the bits cannot — so upstream
/// intersects them, and so does this.
pub fn compute_constant_range_including_known_bits<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    for_signed: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<ConstantRange> {
    let from_bits = ConstantRange::from_known_bits(&compute_known_bits(value, query)?, for_signed);
    let from_range = compute_constant_range(value, for_signed, query)?;
    Ok(from_bits.intersect_with(&from_range, preferred_for(for_signed)))
}

/// Which over-approximation a signed or unsigned query prefers.
fn preferred_for(for_signed: bool) -> PreferredRangeType {
    if for_signed {
        PreferredRangeType::Signed
    } else {
        PreferredRangeType::Unsigned
    }
}

/// Whether `lhs + rhs` overflows unsigned. Ports
/// `llvm::computeOverflowForUnsignedAdd`.
pub fn compute_overflow_for_unsigned_add<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<OverflowResult> {
    let lhs_range = compute_constant_range_including_known_bits(lhs, false, query)?;
    let rhs_range = compute_constant_range_including_known_bits(rhs, false, query)?;
    Ok(lhs_range.unsigned_add_may_overflow(&rhs_range))
}

/// Whether `lhs + rhs` overflows signed. Ports
/// `llvm::computeOverflowForSignedAdd`.
///
/// Upstream has a second overload taking the `add` instruction itself, which
/// consults `computeKnownBitsFromContext` — assumption-driven refinement
/// llvmkit does not model. Only the value-pair form is ported; the extra
/// refinement can only turn `MayOverflow` into `NeverOverflows`, so its
/// absence is conservative.
pub fn compute_overflow_for_signed_add<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<OverflowResult> {
    let lhs_range = compute_constant_range_including_known_bits(lhs, true, query)?;
    let rhs_range = compute_constant_range_including_known_bits(rhs, true, query)?;
    Ok(lhs_range.signed_add_may_overflow(&rhs_range))
}

/// Whether `lhs - rhs` overflows unsigned. Ports
/// `llvm::computeOverflowForUnsignedSub`.
pub fn compute_overflow_for_unsigned_sub<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<OverflowResult> {
    let lhs_range = compute_constant_range_including_known_bits(lhs, false, query)?;
    let rhs_range = compute_constant_range_including_known_bits(rhs, false, query)?;
    Ok(lhs_range.unsigned_sub_may_overflow(&rhs_range))
}

/// Whether `lhs - rhs` overflows signed. Ports
/// `llvm::computeOverflowForSignedSub`.
pub fn compute_overflow_for_signed_sub<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<OverflowResult> {
    let lhs_range = compute_constant_range_including_known_bits(lhs, true, query)?;
    let rhs_range = compute_constant_range_including_known_bits(rhs, true, query)?;
    Ok(lhs_range.signed_sub_may_overflow(&rhs_range))
}

/// Whether `lhs * rhs` overflows unsigned. Ports
/// `llvm::computeOverflowForUnsignedMul`.
///
/// `is_nsw` carries the `mul nsw` promise: a signed-non-wrapping product of
/// two non-negative values cannot wrap unsigned either.
pub fn compute_overflow_for_unsigned_mul<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    is_nsw: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<OverflowResult> {
    let lhs_range = compute_constant_range_including_known_bits(lhs, false, query)?;
    let rhs_range = compute_constant_range_including_known_bits(rhs, false, query)?;
    if is_nsw && lhs_range.is_all_non_negative() && rhs_range.is_all_non_negative() {
        return Ok(OverflowResult::NeverOverflows);
    }
    Ok(lhs_range.unsigned_mul_may_overflow(&rhs_range))
}

/// Whether `lhs * rhs` overflows signed. Ports
/// `llvm::computeOverflowForSignedMul`.
///
/// The reasoning is sign-bit counting rather than ranges: multiplying values
/// with `n` and `m` significant bits needs `n + m`, so enough leading sign
/// bits guarantees the product fits. Upstream credits *Hacker's Delight*.
pub fn compute_overflow_for_signed_mul<'a, 'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<OverflowResult> {
    let bit_width = value_bit_width(lhs, query.data_layout()).unwrap_or(0);
    // Under-estimating the sign-bit count only makes the answer more
    // conservative, which is why this is sound with llvmkit's partial
    // `compute_num_sign_bits`.
    let sign_bits =
        compute_num_sign_bits(lhs, query)?.saturating_add(compute_num_sign_bits(rhs, query)?);

    if sign_bits > bit_width.saturating_add(1) {
        return Ok(OverflowResult::NeverOverflows);
    }

    // Two counts leave no overflow possible: `bit_width + 1` and `bit_width`.
    // The second is hard to check, so upstream handles only the first.
    if sign_bits == bit_width.saturating_add(1) {
        // At this count the product overflows only when both operands are
        // negative and the true product is exactly the signed minimum, so one
        // non-negative operand rules it out.
        if compute_known_bits(lhs, query)?.is_non_negative()
            || compute_known_bits(rhs, query)?.is_non_negative()
        {
            return Ok(OverflowResult::NeverOverflows);
        }
    }
    Ok(OverflowResult::MayOverflow)
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
    stack: &mut HashSet<ValueSlot>,
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
    source_ty: TypeSlot,
    indices: I,
}

fn gep_known_bits_from_values<'a, 'ctx, B, I>(
    input: GepKnownBitsInput<'ctx, B, I>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
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
) -> Option<TypeSlot> {
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
    stack: &mut HashSet<ValueSlot>,
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
    stack: &mut HashSet<ValueSlot>,
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
        needs_element = demanded.bit(idx);
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
    stack: &mut HashSet<ValueSlot>,
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
        if !demanded.bit(lane) {
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
    elements: &[ValueSlot],
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
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
        if !demanded.bit(lane) {
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
    stack: &mut HashSet<ValueSlot>,
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

/// Ports the static `isGuaranteedNotToBeUndefOrPoison(V, AC, CtxI, DT, Depth,
/// Kind)` (`ValueTracking.cpp`).
///
/// One of upstream's arms is **not** ported, and can only make the answer weaker
/// — a `false` where upstream proves `true` — so no caller is misled: the
/// `@llvm.assume` arm (`getKnowledgeValidInContext`), which needs an
/// `AssumptionCache`.
///
/// One arm is an llvmkit **refinement**, marked at its site: a shift whose
/// amount is proven in range by known bits is not poison, where upstream's
/// `shiftAmountKnownInRange` demands a literal constant. It answers `true`
/// strictly more often, and only where the shift provably cannot be poison.
fn is_guaranteed_not_to_be_undef_or_poison<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
    stack: &mut HashSet<ValueSlot>,
    kind: UndefPoisonKind,
) -> IrResult<bool> {
    if depth >= query.max_depth() {
        return Ok(false);
    }

    match &value.data().kind {
        ValueKindData::MetadataAsValue(_) => return Ok(false),
        ValueKindData::Argument { parent_fn, slot } => {
            if argument_is_well_defined(value, *parent_fn, *slot) {
                return Ok(true);
            }
        }
        ValueKindData::Constant(constant) => {
            if let Some(answer) = constant_is_well_defined(value, constant, kind) {
                return Ok(answer);
            }
        }
        _ => {}
    }

    // An allocated object or a null pointer is always well defined.
    //
    // The strip is upstream's, and load-bearing: it peels a zero-offset
    // `inbounds` getelementptr, which would otherwise be poison-capable, and
    // the guarantee that it is not comes precisely from the stripped pointer
    // being an allocated object or null.
    let stripped = strip_pointer_casts_same_representation(value);
    if matches!(
        &stripped.data().kind,
        ValueKindData::GlobalVariable(_)
            | ValueKindData::Function(_)
            | ValueKindData::Constant(ConstantData::PointerNull)
    ) || matches!(
        instruction_kind(stripped),
        Some(InstructionKindData::Alloca(_))
    ) {
        return Ok(true);
    }

    let Some(operator) = instruction_kind(value) else {
        return Ok(false);
    };

    // A freeze can never be undef or poison.
    if matches!(operator, InstructionKindData::Freeze(_)) {
        return Ok(true);
    }
    // Nor can a call whose return is annotated `noundef`, `dereferenceable` or
    // `dereferenceable_or_null` — the latter two imply the first.
    if call_return_is_well_defined(operator) {
        return Ok(true);
    }

    if !can_create_undef_or_poison_kind(value, kind, true) {
        if let InstructionKindData::Phi(data) = operator {
            // Upstream evaluates each incoming value at its edge's terminator;
            // llvmkit has no per-edge context, so the incoming value is
            // evaluated where it stands. That can only weaken the answer.
            let incoming = data.incoming.borrow();
            let mut well_defined = true;
            for (operand, _) in incoming.iter() {
                if operand.get() == value.slot() {
                    continue;
                }
                if !is_guaranteed_not_to_be_undef_or_poison(
                    value_from_id(value, operand.get()),
                    query,
                    depth + 1,
                    stack,
                    kind,
                )? {
                    well_defined = false;
                    break;
                }
            }
            if well_defined {
                return Ok(true);
            }
        } else if let Some(splat) = shuffle_splat_source(value, operator) {
            // For a splat, only the value being splatted has to be checked.
            if is_guaranteed_not_to_be_undef_or_poison(splat, query, depth + 1, stack, kind)? {
                return Ok(true);
            }
        } else {
            let mut well_defined = true;
            for operand in operands_of(value) {
                if !is_guaranteed_not_to_be_undef_or_poison(operand, query, depth + 1, stack, kind)?
                {
                    well_defined = false;
                    break;
                }
            }
            if well_defined {
                return Ok(true);
            }
        }
    }

    // A load carrying `!noundef`, `!dereferenceable` or
    // `!dereferenceable_or_null` is well defined by declaration.
    if matches!(operator, InstructionKindData::Load(_)) && load_metadata_asserts_well_defined(value)
    {
        return Ok(true);
    }

    if program_undefined_for_value(value, !kind.includes_undef()) {
        return Ok(true);
    }

    if dominating_condition_proves_well_defined(value, query, kind) {
        return Ok(true);
    }

    // llvmkit refinement (no upstream counterpart): `shiftAmountKnownInRange`
    // is syntactic, so a shift by a non-constant amount reaches
    // `can_create_undef_or_poison_kind` as "can create poison" and the operand
    // walk above is skipped. Known bits can still prove the amount in range,
    // and a shift whose amount is in range and whose operands are well defined
    // provably is not poison.
    if kind.includes_poison()
        && let InstructionKindData::Shl(data)
        | InstructionKindData::LShr(data)
        | InstructionKindData::AShr(data) = operator
    {
        if query.uses_instruction_info()
            && (data.no_unsigned_wrap || data.no_signed_wrap || data.is_exact)
        {
            return Ok(false);
        }
        let lhs = value_from_id(value, data.lhs.get());
        let rhs = value_from_id(value, data.rhs.get());
        if !is_guaranteed_not_to_be_undef_or_poison(lhs, query, depth + 1, stack, kind)?
            || !is_guaranteed_not_to_be_undef_or_poison(rhs, query, depth + 1, stack, kind)?
        {
            return Ok(false);
        }
        let Some(width) = value_bit_width(lhs, query.data_layout()) else {
            return Ok(false);
        };
        let rhs_bits = compute_known_bits_inner(rhs, query, depth + 1, stack)?;
        return Ok(rhs_bits.max_value().limited_value(u64::from(width)) < u64::from(width));
    }

    Ok(false)
}

/// The value a `shufflevector` splats, when it is one.
///
/// Ports the `isa<ShuffleVectorInst>(Opr) ? getSplatValue(Opr) : nullptr` arm
/// of `isGuaranteedNotToBeUndefOrPoison`, via the shuffle half of
/// `llvm::getSplatValue` (`llvm/lib/Analysis/VectorUtils.cpp`):
///
/// ```text
/// shuf (inselt ?, Splat, 0), ?, <0, poison, 0, ...>
/// ```
///
/// The result is `Splat` — the value the `insertelement` put in lane 0 — not
/// the `insertelement` itself. That distinction is the whole point of the arm:
/// the `insertelement`'s own vector operand is typically `poison`, so walking
/// its operands would answer false where the splat is provably well defined.
///
/// `getSplatValue`'s other half, a constant vector whose lanes are all equal,
/// is not reached from here: this arm runs only for a `ShuffleVectorInst`.
fn shuffle_splat_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    operator: &InstructionKindData,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::ShuffleVector(shuffle) = operator else {
        return None;
    };
    // `m_ZeroMask`: every element zero, or poison standing in for one.
    if !shuffle
        .mask
        .iter()
        .all(|element| *element == 0 || *element == POISON_MASK_ELEM)
    {
        return None;
    }
    // `m_InsertElt(m_Value(), m_Value(Splat), m_ZeroInt())` on operand 0. The
    // shuffle's second operand is `m_Value()` — anything at all.
    let inserted = value_from_id(value, shuffle.lhs.get());
    let InstructionKindData::InsertElement(insert) = instruction_kind(inserted)? else {
        return None;
    };
    let index = value_from_id(inserted, insert.index.get());
    argument_constant(Some(index))
        .is_some_and(|index| index.is_zero())
        .then(|| value_from_id(inserted, insert.value.get()))
}

/// Whether a `load` carries `!noundef`, `!dereferenceable` or
/// `!dereferenceable_or_null`.
///
/// Ports the `dyn_cast<LoadInst>` arm's `hasMetadata` triple. The two
/// dereferenceability kinds imply `noundef`, which is why upstream accepts any
/// of the three.
fn load_metadata_asserts_well_defined<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    let ValueKindData::Instruction(instruction) = &value.data().kind else {
        return false;
    };
    let metadata = instruction.metadata.borrow();
    [
        MetadataAttachmentKind::NoUndef,
        MetadataAttachmentKind::Dereferenceable,
        MetadataAttachmentKind::DereferenceableOrNull,
    ]
    .iter()
    .any(|kind| metadata.get(kind).is_some())
}

/// Whether a branch or switch condition on a block dominating the query's
/// context instruction proves `value` well defined.
///
/// Ports the `Dominator = DNode->getIDom()` loop of
/// `isGuaranteedNotToBeUndefOrPoison`: if `value` is used as a branch condition
/// before control reaches the context instruction, then reaching that point at
/// all means `value` was neither undef nor poison.
///
/// ```text
///   br i1 %v, label %then, label %else
/// then:
///   ; %v cannot be undef or poison here
/// ```
///
/// Upstream walks the idom chain; llvmkit enumerates the same set through
/// [`DominatorTree::strictly_dominating_blocks`], which is order-independent
/// and gives the same answer because the loop is a pure existential. Upstream's
/// two early `return false`s — no context instruction or dominator tree, and an
/// unreachable context block — are `false` here rather than early returns,
/// because llvmkit has one more arm after this point.
fn dominating_condition_proves_well_defined<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    kind: UndefPoisonKind,
) -> bool {
    let Some(context) = query.context_instruction() else {
        return false;
    };
    let Some(dominator_tree) = query.dominator_tree() else {
        return false;
    };
    let Some(context_block) = parent_block(context) else {
        return false;
    };

    // Purely a compile-time guard upstream: the walk can be skipped when the
    // question includes undef and the value is not an integer.
    if kind.includes_undef() && !matches!(value.ty().kind(), TypeKind::Integer { .. }) {
        return false;
    }

    for block in dominator_tree.strictly_dominating_blocks(context_block) {
        let Some(terminator) = block_terminator(value, block) else {
            continue;
        };
        let Some(condition) = branch_or_switch_condition(terminator) else {
            continue;
        };
        if condition == value.slot() {
            return true;
        }
        // For poison — but not undef, which does not propagate eagerly — a
        // condition *built from* the value is enough, provided the operand
        // position propagates poison.
        if kind.includes_undef() {
            continue;
        }
        let condition = value_from_id(value, condition);
        let Some(condition_kind) = instruction_kind(condition) else {
            continue;
        };
        let propagates = condition_kind
            .operand_ids()
            .iter()
            .enumerate()
            .any(|(index, operand)| {
                *operand == value.slot() && propagates_poison(condition, index)
            });
        if propagates {
            return true;
        }
    }
    false
}

/// The terminator of `block`, as a value.
fn block_terminator<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Option<Value<'ctx, B>> {
    let module = module_ref(anchor);
    let ValueKindData::BasicBlock(data) = &module.value_data(block).kind else {
        return None;
    };
    let terminator = *data.instructions.borrow().last()?;
    Some(value_from_id(anchor, terminator))
}

/// The condition of a conditional `br` or a `switch`. Ports the
/// `dyn_cast_or_null<BranchInst>` / `dyn_cast_or_null<SwitchInst>` pair of the
/// dominating-condition walk.
fn branch_or_switch_condition<'ctx, B: ModuleBrand + 'ctx>(
    terminator: Value<'ctx, B>,
) -> Option<ValueSlot> {
    match instruction_kind(terminator)? {
        InstructionKindData::Br(data) => match &*data.kind.borrow() {
            BranchKind::Unconditional(_) => None,
            BranchKind::Conditional { cond, .. } => Some(cond.get()),
        },
        InstructionKindData::Switch(data) => Some(data.cond.get()),
        _ => None,
    }
}

/// The `dyn_cast<Argument>` arm: `noundef`, `dereferenceable` and
/// `dereferenceable_or_null` each imply a well-defined parameter.
fn argument_is_well_defined<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    parent_fn: ValueSlot,
    slot: u32,
) -> bool {
    let function = value_from_id(anchor, parent_fn);
    let ValueKindData::Function(data) = &function.data().kind else {
        return false;
    };
    let attributes = data.attributes.borrow();
    attributes
        .get(AttrIndex::Param(slot))
        .is_some_and(|stored| stored.iter().any(is_well_defined_attribute))
}

/// The three attributes upstream reads as "this operand is well defined".
fn is_well_defined_attribute(attribute: &AttributeStored) -> bool {
    matches!(
        attribute,
        AttributeStored::Enum(AttrKind::NoUndef)
            | AttributeStored::Int(AttrKind::Dereferenceable, _)
            | AttributeStored::Int(AttrKind::DereferenceableOrNull, _)
    )
}

/// The `dyn_cast<Constant>` arm. `None` means the constant fell through to the
/// operator tests, exactly as upstream's `ConstantExpr` case does.
fn constant_is_well_defined<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    constant: &ConstantData,
    kind: UndefPoisonKind,
) -> Option<bool> {
    match constant {
        ConstantData::Poison => Some(!kind.includes_poison()),
        ConstantData::Undef => Some(!kind.includes_undef()),
        ConstantData::Int(_)
        | ConstantData::Float(_)
        | ConstantData::PointerNull
        | ConstantData::GlobalValueRef { .. } => Some(true),
        // Upstream's vector arm: an element that is undef or poison of the
        // asked-about kind disqualifies the whole vector, and a constant
        // expression inside one is not analysed.
        ConstantData::Aggregate(elements)
            if matches!(
                value.ty().kind(),
                TypeKind::FixedVector | TypeKind::ScalableVector
            ) =>
        {
            for element in elements.iter() {
                let element = value_from_id(value, *element);
                match &element.data().kind {
                    ValueKindData::Constant(ConstantData::Undef) if kind.includes_undef() => {
                        return Some(false);
                    }
                    ValueKindData::Constant(ConstantData::Poison) if kind.includes_poison() => {
                        return Some(false);
                    }
                    ValueKindData::Constant(ConstantData::Expr(_)) => return Some(false),
                    _ => {}
                }
            }
            Some(true)
        }
        // A constant expression falls through to the operator tests upstream;
        // llvmkit models those in a separate arm that the operator walk below
        // does not reach, so the honest answer is "not proven".
        ConstantData::Expr(_) => Some(false),
        _ => None,
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
    id: ValueSlot,
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

fn erase_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Type<'ctx, DynBrand> {
    Type::new(ty.id(), ModuleRef::new(ty.module().core_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruction::build_instruction_value;
    use crate::module::Module;
    use crate::value::IsValue;

    fn fabricate_instruction<B: ModuleBrand>(
        m: &Module<B>,
        bb_id: ValueSlot,
        result_ty: TypeSlot,
        kind: InstructionKindData,
    ) -> ValueSlot {
        let core = m.core_ref();
        let value = build_instruction_value(result_ty, bb_id, kind, None);
        let id = core.context().push_value(value);
        let ValueKindData::BasicBlock(bb_data) = &core.context().value_data(bb_id).kind else {
            panic!("fabricate_instruction: bb_id is not a basic block");
        };
        bb_data.instructions.borrow_mut().push(id);
        id
    }

    fn fabricated_value<'ctx, B: ModuleBrand + 'ctx>(
        m: &'ctx Module<B>,
        id: ValueSlot,
        ty: TypeSlot,
    ) -> Value<'ctx, B> {
        Value::from_parts(id, ModuleRef::new(m.core_ref()), ty)
    }

    /// Mirrors `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBitsFromOperator`
    /// `getelementptr` handling: LLVM queries `DataLayout::getIndexTypeSizeInBits`
    /// with the GEP result type, whose pointer-vector element address space
    /// selects the index width.
    #[test]
    fn vector_gep_uses_element_pointer_address_space_for_index_width() -> crate::IrResult<()> {
        let m = crate::module_new!("vt-vector-gep-as")?;
        m.set_data_layout("p1:64:64:64:32")?;
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let ptr1_ty = m.ptr_type(1);
        let ptr_vec_ty = m.vector_type(ptr1_ty.as_type(), 2, false);
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, crate::Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");

        let base = ptr_vec_ty.const_vector([ptr1_ty.const_null(); 2])?;
        let minus_one = i32_ty.const_int(-1_i32);
        let gep_ty = ptr_vec_ty.as_type();
        let gep_id = fabricate_instruction(
            &m,
            entry.slot(),
            gep_ty.id(),
            InstructionKindData::Gep(GepInstData::new(
                i8_ty.as_type().id(),
                base.slot(),
                [minus_one.slot()],
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
    }
}
