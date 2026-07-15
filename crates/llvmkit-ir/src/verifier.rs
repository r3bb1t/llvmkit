//! Module verifier. Mirrors `llvm/include/llvm/IR/Verifier.h` and
//! `llvm/lib/IR/Verifier.cpp` for the constructive subset of opcodes
//! modeled by `llvmkit-ir`: arithmetic, casts, compares, memory, GEP,
//! calls, select, PHI, terminators, aggregate/vector operations, atomics,
//! EH / funclet pads, and the parser-era terminator families.
//!
//! The verifier walks every function, block, and instruction in
//! declaration order, applying per-opcode invariants. Each rule cites
//! its `Verifier::visit*` method in the upstream C++.
//!
//! ## Public surface
//!
//! - [`Module::verify_borrowed`](crate::Module::verify_borrowed) — borrow-only
//!   diagnostic check.
//! - [`Module::verify`](crate::Module::verify) — consumes the module and returns
//!   `Module<'ctx, B, Verified>`, which the pass manager requires for
//!   module pipelines that assume well-formed IR.
//!
//! ## Coverage gaps (deferred)
//!
//! - Metadata / debug-info / intrinsic / inline-asm verifier rules are not
//!   fully ported yet.
//! - GEP index-walks-the-aggregate-type checks are deferred; today the
//!   verifier checks that every GEP index is integer-typed and that the
//!   source type is sized.
//! - Per-function attribute coherence rules (`noalias` /
//!   `byval` / ...) are out of scope for the current verifier.

use std::collections::HashMap;

use crate::attributes::AttributeStorage;
use crate::basic_block::BasicBlock;
use crate::constant_range::{ConstantRange, metadata_constant_int};
use crate::derived_types::SizedType;
use crate::dominator_tree::DominatorTree;
use crate::error::{IrError, IrResult, VerifierRule};
use crate::function::FunctionValue;
use crate::instr_types::{
    BinaryOpData, BranchInstData, BranchKind, CastOpData, CastOpcode, CmpInstData, FCmpInstData,
    GepInstData, PhiData, ReturnOpData,
};
use crate::instruction::{InstructionKindData, InstructionView};
use crate::marker::Dyn;
use crate::metadata::{MetadataAttachmentKind, MetadataId, MetadataKind};
use crate::module::{ModuleCore, ModuleView};
use crate::phi_check::{PhiViolation, check_phi_incoming};
use crate::r#type::{Type, TypeData, TypeId};
use crate::value::{ValueId, ValueKindData};

// --------------------------------------------------------------------------
// Verifier
// --------------------------------------------------------------------------

/// CFG context built once per function and threaded through every
/// per-block / per-instruction visit. Mirrors LLVM's transient
/// per-function state inside `Verifier::visit*`.
struct FunctionContext<'a> {
    /// Predecessor multiset per block id.
    predecessors: &'a HashMap<ValueId, Vec<ValueId>>,
    /// Declaration-order index of every block in the parent function.
    block_index: &'a HashMap<ValueId, usize>,
    /// Recomputed dominator tree for cross-block SSA dominance checks.
    dom_tree: &'a DominatorTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeLikeMetadataKind {
    Range,
    AbsoluteSymbol,
}

/// Module verifier. Stateless apart from the per-function CFG cache
/// it builds during a [`Self::run`] traversal.
pub(crate) struct Verifier<'ctx> {
    module: &'ctx ModuleCore,
}

impl<'ctx> Verifier<'ctx> {
    pub(crate) fn new<B: crate::module::ModuleBrand + 'ctx>(module: ModuleView<'ctx, B>) -> Self {
        Self {
            module: module.core_ref(),
        }
    }

    /// Verify every function in the module. Returns the first
    /// invariant violation encountered. Stops on first error to keep
    /// `IrError` single-shot; a later revision can add a multi-error
    /// collecting variant if pass infrastructure needs it.
    pub(crate) fn run(&self) -> IrResult<()> {
        for g in self.module.iter_globals() {
            self.visit_global_variable(g)?;
        }
        for f in self.module.iter_functions() {
            self.visit_function(f)?;
        }
        Ok(())
    }

    /// Mirrors `Verifier::visitGlobalVariable` for the constructive
    /// subset shipped today (initializer type/sized, common-linkage
    /// invariants, scalable-type rejection). The intrinsic-globals
    /// (`llvm.global_ctors` / `llvm.used` / etc.) and metadata
    /// attachment rules are deferred -- they need the metadata layer.
    fn visit_global_variable(
        &self,
        g: crate::global_variable::GlobalVariable<'ctx>,
    ) -> IrResult<()> {
        let value_ty = g.value_type();

        if type_contains_scalable(self.module, value_ty.id()) {
            return Err(self.fail_global(
                g,
                VerifierRule::GlobalScalableType,
                format!("@{}: globals cannot contain scalable types", g.name()),
            ));
        }

        if let Some(init) = g.initializer() {
            if init.ty() != value_ty {
                return Err(self.fail_global(
                    g,
                    VerifierRule::GlobalInitializerTypeMismatch,
                    format!(
                        "@{}: initializer type {} does not match value type {}",
                        g.name(),
                        init.ty().kind_label(),
                        value_ty.kind_label(),
                    ),
                ));
            }
            if !value_ty.is_sized() {
                return Err(self.fail_global(
                    g,
                    VerifierRule::GlobalInitializerUnsized,
                    format!("@{}: initializer must be sized", g.name()),
                ));
            }
            if g.linkage() == crate::global_value::Linkage::Common {
                let init_data = self.module.context().value_data(init.as_value().id);
                let zero = matches!(
                    &init_data.kind,
                    ValueKindData::Constant(crate::constant::ConstantData::Int(words))
                        if words.iter().all(|w| *w == 0)
                ) || matches!(
                    &init_data.kind,
                    ValueKindData::Constant(crate::constant::ConstantData::Float(0))
                ) || matches!(
                    &init_data.kind,
                    ValueKindData::Constant(crate::constant::ConstantData::PointerNull)
                );
                if !zero || g.is_constant() || g.comdat().is_some() {
                    return Err(self.fail_global(
                        g,
                        VerifierRule::CommonLinkageInvariantViolated,
                        format!(
                            "@{}: common-linkage global must have a zero initializer, must not be constant, and must not be in a comdat",
                            g.name()
                        ),
                    ));
                }
            }
            self.verify_constant_tree(init)?;
        }
        if let Some(range_id) = g.metadata().get(&MetadataAttachmentKind::AbsoluteSymbol) {
            let pointer_width = self
                .module
                .data_layout()
                .pointer_size_in_bits(g.address_space());
            let pointer_int_ty = self.module.context().int_type(pointer_width);
            self.verify_range_like_metadata_global(
                g,
                range_id,
                pointer_int_ty,
                RangeLikeMetadataKind::AbsoluteSymbol,
            )?;
        }

        Ok(())
    }

    fn verify_constant_tree(&self, constant: crate::constant::Constant<'ctx>) -> IrResult<()> {
        let value_data = self.module.context().value_data(constant.as_value().id);
        let ValueKindData::Constant(data) = &value_data.kind else {
            return Ok(());
        };
        match data {
            crate::constant::ConstantData::Expr(expr) => {
                crate::constants::verify_constant_expr_data(self.module, expr)?;
                for operand in expr.operands.iter() {
                    let operand_data = self.module.context().value_data(*operand);
                    if matches!(operand_data.kind, ValueKindData::Constant(_)) {
                        self.verify_constant_tree(crate::constant::Constant::try_from(
                            crate::value::Value::from_parts(*operand, self.module, operand_data.ty),
                        )?)?;
                    }
                }
            }
            crate::constant::ConstantData::BlockAddress { function, block } => {
                let block = crate::basic_block::BasicBlock::<'ctx, crate::marker::Dyn>::from_parts(
                    *block,
                    self.module,
                    self.module.label_type().as_type().id(),
                );
                if block.parent_function().map(|f| f.as_value().id) != Some(*function) {
                    return Err(IrError::InvalidOperation {
                        message: "blockaddress block must belong to referenced function",
                    });
                }
            }
            crate::constant::ConstantData::DSOLocalEquivalent { function } => {
                let value = crate::value::Value::from_parts(
                    *function,
                    self.module,
                    self.value_type(*function),
                );
                match &value.data().kind {
                    ValueKindData::Function(_) => {}
                    ValueKindData::GlobalAlias(_) => {
                        if !crate::GlobalAlias::try_from(value)?
                            .value_type()
                            .is_function()
                        {
                            return Err(IrError::InvalidOperation {
                                message: "dso_local_equivalent expects a function, alias to function, or ifunc",
                            });
                        }
                    }
                    ValueKindData::GlobalIFunc(_) => {
                        if !crate::GlobalIFunc::try_from(value)?
                            .value_type()
                            .is_function()
                        {
                            return Err(IrError::InvalidOperation {
                                message: "dso_local_equivalent expects a function, alias to function, or ifunc",
                            });
                        }
                    }
                    _ => {
                        return Err(IrError::InvalidOperation {
                            message: "dso_local_equivalent expects a function, alias to function, or ifunc",
                        });
                    }
                }
            }
            crate::constant::ConstantData::NoCfi { function } => {
                let value = crate::value::Value::from_parts(
                    *function,
                    self.module,
                    self.value_type(*function),
                );
                match &value.data().kind {
                    ValueKindData::Function(_)
                    | ValueKindData::GlobalVariable(_)
                    | ValueKindData::GlobalAlias(_)
                    | ValueKindData::GlobalIFunc(_) => {}
                    _ => {
                        return Err(IrError::InvalidOperation {
                            message: "no_cfi expects a global value",
                        });
                    }
                }
            }
            crate::constant::ConstantData::TokenNone => {
                if !constant.ty().is_token() {
                    return Err(IrError::InvalidOperation {
                        message: "token none must have token type",
                    });
                }
            }
            crate::constant::ConstantData::TargetExtNone => {
                if !constant.ty().is_target_ext() {
                    return Err(IrError::InvalidOperation {
                        message: "target extension none must have target extension type",
                    });
                }
            }
            crate::constant::ConstantData::PtrAuth {
                pointer,
                key,
                discriminator,
                addr_discriminator,
                deactivation_symbol,
            } => {
                let pointer = crate::value::Value::from_parts(
                    *pointer,
                    self.module,
                    self.value_type(*pointer),
                );
                let key = crate::value::Value::from_parts(*key, self.module, self.value_type(*key));
                let discriminator = crate::value::Value::from_parts(
                    *discriminator,
                    self.module,
                    self.value_type(*discriminator),
                );
                let addr_discriminator = crate::value::Value::from_parts(
                    *addr_discriminator,
                    self.module,
                    self.value_type(*addr_discriminator),
                );
                let deactivation_symbol = crate::value::Value::from_parts(
                    *deactivation_symbol,
                    self.module,
                    self.value_type(*deactivation_symbol),
                );
                if !pointer.ty().is_pointer()
                    || !addr_discriminator.ty().is_pointer()
                    || !deactivation_symbol.ty().is_pointer()
                    || key.ty() != self.module.i32_type().as_type()
                    || discriminator.ty() != self.module.i64_type().as_type()
                    || constant.ty() != pointer.ty()
                    || !matches!(
                        &self
                            .module
                            .context()
                            .value_data(deactivation_symbol.id)
                            .kind,
                        ValueKindData::Constant(
                            crate::constant::ConstantData::GlobalValueRef { .. }
                                | crate::constant::ConstantData::PointerNull
                        )
                    )
                {
                    return Err(IrError::InvalidOperation {
                        message: "invalid ptrauth constant",
                    });
                }
            }
            crate::constant::ConstantData::Aggregate(ids) => {
                for id in ids.iter() {
                    let operand_data = self.module.context().value_data(*id);
                    if matches!(operand_data.kind, ValueKindData::Constant(_)) {
                        self.verify_constant_tree(crate::constant::Constant::try_from(
                            crate::value::Value::from_parts(*id, self.module, operand_data.ty),
                        )?)?;
                    }
                }
            }
            crate::constant::ConstantData::BlockAddressPlaceholder => {
                return Err(IrError::InvalidOperation {
                    message: "unresolved forward blockaddress placeholder",
                });
            }
            crate::constant::ConstantData::GlobalValueRef { .. }
            | crate::constant::ConstantData::PointerNull
            | crate::constant::ConstantData::GepOffset { .. }
            | crate::constant::ConstantData::SymbolDelta { .. }
            | crate::constant::ConstantData::SymbolDeltaPlus { .. }
            | crate::constant::ConstantData::Int(_)
            | crate::constant::ConstantData::Float(_)
            | crate::constant::ConstantData::Undef
            | crate::constant::ConstantData::Poison => {}
        }
        Ok(())
    }

    fn fail_global(
        &self,
        g: crate::global_variable::GlobalVariable<'ctx>,
        rule: VerifierRule,
        message: String,
    ) -> IrError {
        IrError::VerifierFailure {
            rule,
            function: Some(format!("@{}", g.name())),
            block: None,
            message,
        }
    }

    // ------------------------------------------------------------------
    // Per-function walk
    // ------------------------------------------------------------------

    fn visit_function(&self, f: FunctionValue<'ctx, Dyn>) -> IrResult<()> {
        self.verify_intrinsic_function(f)?;
        // Build a CFG predecessor map for this function so phi-validation
        // and use-before-def checks can consult it without re-walking
        // every terminator. Mirrors `Verifier::predecessorMultiset`
        // in `Verifier.cpp`.
        let predecessors = build_predecessors(f);
        // Collect block ids in declaration order so use-before-def
        // can check forward references between blocks (cross-block
        // checks are conservative -- see deferred-coverage note).
        let block_ids: Vec<ValueId> = f.basic_blocks().map(|bb| bb.as_value().id).collect();
        let block_index: HashMap<ValueId, usize> = block_ids
            .iter()
            .copied()
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect();

        let dom_tree = DominatorTree::new(f);
        let cx = FunctionContext {
            predecessors: &predecessors,
            block_index: &block_index,
            dom_tree: &dom_tree,
        };
        for bb in f.basic_blocks() {
            let bb = bb.retag_termination::<crate::block_state::Unterminated>();
            self.visit_block(f, &bb, &cx)?;
        }
        Ok(())
    }

    fn verify_intrinsic_function(&self, f: FunctionValue<'ctx, Dyn>) -> IrResult<()> {
        let name = f.name();
        match crate::intrinsics::resolve_intrinsic_name(name) {
            crate::intrinsics::IntrinsicNameResolution::NonIntrinsic => return Ok(()),
            crate::intrinsics::IntrinsicNameResolution::UnknownIntrinsic => {
                return Err(IrError::UnknownIntrinsic {
                    name: name.to_owned(),
                });
            }
            crate::intrinsics::IntrinsicNameResolution::Known(_) => {}
        }
        let descriptor = self
            .module
            .intrinsic_descriptor_from_signature::<crate::module::Brand<'ctx>>(name, f.signature())
            .map_err(|err| match err {
                IrError::UnknownIntrinsic { .. } | IrError::IntrinsicSignatureMismatch { .. } => {
                    err
                }
                _ => IrError::IntrinsicSignatureMismatch {
                    name: name.to_owned(),
                },
            })?;
        if f.is_intrinsic() && f.intrinsic_descriptor().as_ref() != Some(&descriptor) {
            return Err(IrError::IntrinsicSignatureMismatch {
                name: name.to_owned(),
            });
        }
        if f.basic_blocks().next().is_some() {
            return Err(IrError::InvalidOperation {
                message: "intrinsic functions should never be defined",
            });
        }
        let expected_attrs = descriptor
            .declaration_attributes(f.signature())
            .map_err(|err| match err {
                IrError::UnknownIntrinsic { .. } | IrError::IntrinsicSignatureMismatch { .. } => {
                    err
                }
                _ => IrError::IntrinsicSignatureMismatch {
                    name: name.to_owned(),
                },
            })?;
        let Some(actual_attrs) = self.function_attrs_with_groups(f) else {
            return Err(IrError::InvalidOperation {
                message: "intrinsic declaration modifier",
            });
        };
        if !expected_attrs.is_subset_of(&actual_attrs) {
            return Err(IrError::InvalidOperation {
                message: "intrinsic declaration modifier",
            });
        }
        let intrinsic_value = f.as_value();
        for user in intrinsic_value.users() {
            let used_as_callee = match user.kind() {
                Some(crate::instruction::InstructionKind::Call(call)) => {
                    call.callee().id() == intrinsic_value.id()
                }
                _ => match user.terminator_kind() {
                    Some(crate::instruction::TerminatorKind::Invoke(invoke)) => {
                        invoke.callee().id() == intrinsic_value.id()
                    }
                    Some(crate::instruction::TerminatorKind::CallBr(callbr)) => {
                        callbr.callee().id() == intrinsic_value.id()
                    }
                    _ => false,
                },
            };
            if !used_as_callee {
                return Err(IrError::InvalidOperation {
                    message: "intrinsic can only be used as callee",
                });
            }
        }
        Ok(())
    }

    fn function_attrs_with_groups(&self, f: FunctionValue<'ctx, Dyn>) -> Option<AttributeStorage> {
        let module_attr_groups = self.module.attribute_groups();
        let mut attrs = f.data().attributes.borrow().clone();
        for group in f.function_attr_groups() {
            let (_, group_attrs) = module_attr_groups
                .iter()
                .rev()
                .find(|(id, _)| *id == group)?;
            attrs.merge_from(group_attrs);
        }
        Some(attrs)
    }

    fn visit_block(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        let instructions: Vec<InstructionView<'ctx>> = bb.instructions().collect();

        // Empty block is malformed (LLVM accepts `unreachable` as the
        // sole instruction; an empty list has no terminator at all).
        let Some(last) = instructions.last() else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::MissingTerminator,
                format!(
                    "block {:?} has no instructions",
                    bb.name().as_deref().unwrap_or("<anon>")
                ),
            ));
        };

        // Terminator placement: every non-last instruction must be a
        // non-terminator, the last instruction must be a terminator.
        // Mirrors the prologue of `Verifier::visitInstruction`.
        for (idx, inst) in instructions.iter().enumerate() {
            let is_last = idx + 1 == instructions.len();
            if inst.is_terminator() && !is_last {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MisplacedTerminator,
                    "terminator appears before the end of the block".into(),
                ));
            }
            if !inst.is_terminator() && is_last {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::MissingTerminator,
                    "block does not end with a terminator instruction".into(),
                ));
            }
        }
        // Independently: there must be a terminator at the end. The
        // pair of checks above covers it but the explicit assertion
        // makes the intent obvious to readers and mirrors
        // `Verifier::visitBasicBlock`.
        let _ = last;

        // PHI grouping rule: phi nodes must come before any non-phi
        // instruction. Mirrors `Verifier::visitPHINode`'s
        // "PHI nodes not grouped at top of block" assertion.
        let mut seen_non_phi = false;
        for inst in &instructions {
            let is_phi = matches!(inst.kind(), Some(crate::InstructionKind::Phi(_)));
            if is_phi && seen_non_phi {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiNotAtTop,
                    "phi node appears after a non-phi instruction".into(),
                ));
            }
            if !is_phi {
                seen_non_phi = true;
            }
        }

        // Per-instruction rules.
        for (idx, inst) in instructions.iter().enumerate() {
            self.visit_instruction(f, bb, inst, idx, &instructions, cx)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Per-instruction dispatch
    // ------------------------------------------------------------------

    fn visit_instruction(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        index_in_block: usize,
        block_instructions: &[InstructionView<'ctx>],
        cx: &FunctionContext<'_>,
    ) -> IrResult<()> {
        // Universal invariants applied to every opcode (mirrors the
        // shared prologue of `Verifier::visitInstruction`):
        //   1. Self-reference -- only PHI may reference its own value.
        //   2. In-block use-before-def -- an operand whose defining
        //      instruction lives in the same block AND comes after
        //      the use is malformed.
        // The PHI exception lives where the storage payload is read
        // (we know the kind here, and PHI's "incoming" pairs are
        // semantically uses on predecessor edges, not at the phi).
        self.check_self_reference_and_in_block_dom(
            f,
            bb,
            inst,
            index_in_block,
            block_instructions,
        )?;
        self.check_dominates_uses(f, bb, inst, cx.dom_tree)?;

        // Per-opcode dispatch. Reaches into the storage payload
        // directly because every typed handle re-narrows the same
        // payload anyway; one match arm per opcode keeps the dispatch
        // table local.
        let kind = match &inst.as_value().data().kind {
            ValueKindData::Instruction(i) => &i.kind,
            // Instruction's invariant (asserted at handle construction)
            // is that the value-kind is Instruction.
            _ => unreachable!("instruction handle invariant: value kind is Instruction"),
        };
        let opcode_result = match kind {
            InstructionKindData::Add(b)
            | InstructionKindData::Sub(b)
            | InstructionKindData::Mul(b)
            | InstructionKindData::UDiv(b)
            | InstructionKindData::SDiv(b)
            | InstructionKindData::URem(b)
            | InstructionKindData::SRem(b)
            | InstructionKindData::Shl(b)
            | InstructionKindData::LShr(b)
            | InstructionKindData::AShr(b)
            | InstructionKindData::And(b)
            | InstructionKindData::Or(b)
            | InstructionKindData::Xor(b) => self.check_int_binary(f, bb, inst, b),
            InstructionKindData::FAdd(b)
            | InstructionKindData::FSub(b)
            | InstructionKindData::FMul(b)
            | InstructionKindData::FDiv(b)
            | InstructionKindData::FRem(b) => self.check_float_binary(f, bb, inst, b),
            InstructionKindData::ICmp(c) => self.check_icmp(f, bb, inst, c),
            InstructionKindData::FCmp(c) => self.check_fcmp(f, bb, inst, c),
            InstructionKindData::Cast(c) => self.check_cast(f, bb, inst, c),
            InstructionKindData::Alloca(a) => self.check_alloca(f, bb, inst, a),
            InstructionKindData::Load(l) => self.check_load(f, bb, inst, l),
            InstructionKindData::Store(s) => self.check_store(f, bb, inst, s),
            InstructionKindData::Gep(g) => self.check_gep(f, bb, inst, g),
            InstructionKindData::Call(c) => self.check_call(f, bb, inst, c),
            InstructionKindData::Select(s) => self.check_select(f, bb, inst, s),
            InstructionKindData::Phi(p) => {
                let reachable = cx.dom_tree.is_reachable_from_entry(bb);
                self.check_phi(f, bb, inst, p, cx.predecessors, reachable)
            }
            InstructionKindData::Ret(r) => self.check_ret(f, bb, inst, r),
            InstructionKindData::Br(b) => self.check_br(f, bb, inst, b, cx.block_index),
            InstructionKindData::FNeg(u) => self.check_fneg(f, bb, inst, u),
            InstructionKindData::Freeze(u) => self.check_freeze(f, bb, inst, u),
            InstructionKindData::VAArg(u) => self.check_va_arg(f, bb, inst, u),
            InstructionKindData::ExtractValue(d) => self.check_extract_value(f, bb, inst, d),
            InstructionKindData::InsertValue(d) => self.check_insert_value(f, bb, inst, d),
            InstructionKindData::ExtractElement(d) => self.check_extract_element(f, bb, inst, d),
            InstructionKindData::InsertElement(d) => self.check_insert_element(f, bb, inst, d),
            InstructionKindData::ShuffleVector(d) => self.check_shuffle_vector(f, bb, inst, d),
            InstructionKindData::Fence(d) => self.check_fence(f, bb, inst, d),
            InstructionKindData::AtomicCmpXchg(d) => self.check_cmpxchg(f, bb, inst, d),
            InstructionKindData::AtomicRMW(d) => self.check_atomicrmw(f, bb, inst, d),
            InstructionKindData::Switch(d) => self.check_switch(f, bb, inst, d, cx.block_index),
            InstructionKindData::IndirectBr(d) => {
                self.check_indirectbr(f, bb, inst, d, cx.block_index)
            }
            InstructionKindData::Invoke(d) => self.check_invoke(f, bb, inst, d, cx.block_index),
            InstructionKindData::CallBr(d) => self.check_callbr(f, bb, inst, d, cx.block_index),
            InstructionKindData::LandingPad(_) => Ok(()),
            InstructionKindData::Resume(_) => Ok(()),
            InstructionKindData::CleanupPad(_)
            | InstructionKindData::CatchPad(_)
            | InstructionKindData::CatchReturn(_)
            | InstructionKindData::CleanupReturn(_)
            | InstructionKindData::CatchSwitch(_) => Ok(()),
            InstructionKindData::Unreachable(_) => Ok(()),
        };
        opcode_result?;
        self.check_instruction_metadata(f, bb, inst, kind)
    }

    fn check_instruction_metadata(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        kind: &InstructionKindData,
    ) -> IrResult<()> {
        let Some(range_id) = inst.metadata().get(&MetadataAttachmentKind::Range) else {
            return Ok(());
        };
        if !matches!(
            kind,
            InstructionKindData::Load(_)
                | InstructionKindData::Call(_)
                | InstructionKindData::Invoke(_)
        ) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::RangeMetadataInvalidAttachment,
                "Ranges are only for loads, calls and invokes!".to_string(),
            ));
        }
        self.verify_range_like_metadata_inst(
            f,
            bb,
            inst,
            range_id,
            scalar_type_id(self.module, inst.ty().id),
            RangeLikeMetadataKind::Range,
        )
    }

    fn verify_range_like_metadata_inst(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        id: MetadataId,
        expected_scalar_ty: TypeId,
        kind: RangeLikeMetadataKind,
    ) -> IrResult<()> {
        self.verify_range_like_metadata(id, expected_scalar_ty, kind, |rule, message| {
            self.fail(f, bb, rule, message)
        })
    }

    fn verify_range_like_metadata_global(
        &self,
        g: crate::global_variable::GlobalVariable<'ctx>,
        id: MetadataId,
        expected_scalar_ty: TypeId,
        kind: RangeLikeMetadataKind,
    ) -> IrResult<()> {
        self.verify_range_like_metadata(id, expected_scalar_ty, kind, |rule, message| {
            self.fail_global(g, rule, message)
        })
    }

    fn verify_range_like_metadata<F>(
        &self,
        id: MetadataId,
        expected_scalar_ty: TypeId,
        kind: RangeLikeMetadataKind,
        mut fail: F,
    ) -> IrResult<()>
    where
        F: FnMut(VerifierRule, String) -> IrError,
    {
        let store = self.module.metadata_store();
        let Some(MetadataKind::Tuple { operands, .. }) = store.get(id) else {
            return Err(fail(
                VerifierRule::RangeMetadataMalformed,
                "range metadata must be a tuple".to_string(),
            ));
        };
        if operands.len() % 2 != 0 {
            return Err(fail(
                VerifierRule::RangeMetadataMalformed,
                "Unfinished range!".to_string(),
            ));
        }
        let num_ranges = operands.len() / 2;
        if num_ranges == 0 {
            return Err(fail(
                VerifierRule::RangeMetadataMalformed,
                "It should have at least one range!".to_string(),
            ));
        }

        let mut first_range = None;
        let mut last_range = None;
        for (idx, pair) in operands.chunks_exact(2).enumerate() {
            let Some((low_ty, low)) = metadata_constant_int(self.module, &store, pair[0].0) else {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "The lower limit must be an integer!".to_string(),
                ));
            };
            let Some((high_ty, high)) = metadata_constant_int(self.module, &store, pair[1].0)
            else {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "The upper limit must be an integer!".to_string(),
                ));
            };
            if high_ty != low_ty {
                return Err(fail(
                    VerifierRule::RangeMetadataTypeMismatch,
                    "Range pair types must match!".to_string(),
                ));
            }
            if high_ty != expected_scalar_ty {
                return Err(fail(
                    VerifierRule::RangeMetadataTypeMismatch,
                    "Range types must match instruction type!".to_string(),
                ));
            }
            if low.eq_ap_int(&high) && !low.is_max_value() && !low.is_min_value() {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "The upper and lower limits cannot be the same value".to_string(),
                ));
            }
            let range = ConstantRange::new(low.clone(), high.clone())
                .map_err(|err| fail(VerifierRule::RangeMetadataTypeMismatch, err.to_string()))?;
            if range.is_empty_set() || (kind == RangeLikeMetadataKind::Range && range.is_full_set())
            {
                return Err(fail(
                    VerifierRule::RangeMetadataMalformed,
                    "Range must not be empty!".to_string(),
                ));
            }
            if let Some(prev) = &last_range {
                if range.intersects_with(prev) {
                    return Err(fail(
                        VerifierRule::RangeMetadataOverlapping,
                        "Intervals are overlapping".to_string(),
                    ));
                }
                if !low.sgt(prev.lower()) {
                    return Err(fail(
                        VerifierRule::RangeMetadataOutOfOrder,
                        "Intervals are not in order".to_string(),
                    ));
                }
                if range.is_contiguous_with(prev) {
                    return Err(fail(
                        VerifierRule::RangeMetadataContiguous,
                        "Intervals are contiguous".to_string(),
                    ));
                }
            }
            if idx == 0 {
                first_range = Some(range.clone());
            }
            last_range = Some(range);
        }
        if num_ranges > 2
            && let (Some(first), Some(last)) = (&first_range, &last_range)
        {
            if first.intersects_with(last) {
                return Err(fail(
                    VerifierRule::RangeMetadataOverlapping,
                    "Intervals are overlapping".to_string(),
                ));
            }
            if first.is_contiguous_with(last) {
                return Err(fail(
                    VerifierRule::RangeMetadataContiguous,
                    "Intervals are contiguous".to_string(),
                ));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Per-opcode checks
    // ------------------------------------------------------------------

    /// `Verifier::visitBinaryOperator` -- integer flavor.
    /// `add`/`sub`/`mul`/`udiv`/`sdiv`/`urem`/`srem`/`shl`/`lshr`/`ashr`/
    /// `and`/`or`/`xor`.
    fn check_int_binary(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        b: &BinaryOpData,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(b.lhs.get());
        let rhs_ty = self.value_type(b.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryOperandsTypeMismatch,
                format!(
                    "lhs is {} but rhs is {}",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_int_or_int_vector(self.module, lhs_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IntegerOpNonIntegerOperand,
                format!("operand type {} is not integer", self.type_label(lhs_ty)),
            ));
        }
        if inst.ty().id != lhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryResultTypeMismatch,
                format!(
                    "result {} != operand {}",
                    self.type_label(inst.ty().id),
                    self.type_label(lhs_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitBinaryOperator` -- float flavor.
    /// `fadd`/`fsub`/`fmul`/`fdiv`/`frem`.
    fn check_float_binary(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        b: &BinaryOpData,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(b.lhs.get());
        let rhs_ty = self.value_type(b.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryOperandsTypeMismatch,
                format!(
                    "lhs is {} but rhs is {}",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_fp_or_fp_vector(self.module, lhs_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FloatOpNonFloatOperand,
                format!(
                    "operand type {} is not floating-point",
                    self.type_label(lhs_ty)
                ),
            ));
        }
        if inst.ty().id != lhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::BinaryResultTypeMismatch,
                format!(
                    "result {} != operand {}",
                    self.type_label(inst.ty().id),
                    self.type_label(lhs_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitFNeg`. The `fneg` opcode produces an FP value
    /// whose type matches the operand type.
    fn check_fneg(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        u: &crate::instr_types::FNegInstData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(u.src.get());
        if !is_fp_or_fp_vector(self.module, src_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FNegTypeMismatch,
                format!(
                    "operand type {} is not floating-point",
                    self.type_label(src_ty)
                ),
            ));
        }
        if inst.ty().id != src_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FNegTypeMismatch,
                format!(
                    "result {} != operand {}",
                    self.type_label(inst.ty().id),
                    self.type_label(src_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitFreeze`. The result type must match the operand
    /// type. Operand type is otherwise unconstrained (LangRef permits
    /// any first-class type except aggregates of tokens).
    fn check_freeze(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        u: &crate::instr_types::FreezeInstData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(u.src.get());
        if inst.ty().id != src_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FreezeTypeMismatch,
                format!(
                    "result {} != operand {}",
                    self.type_label(inst.ty().id),
                    self.type_label(src_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitVAArgInst`. The source operand must be a
    /// pointer to a `va_list`; the destination type is independent.
    fn check_va_arg(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        u: &crate::instr_types::VAArgInstData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(u.src.get());
        if !self.module.context().type_data(src_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VAArgNonPointerOperand,
                format!("va_arg source {} is not a pointer", self.type_label(src_ty)),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitExtractValueInst`. Walks the aggregate type by
    /// the index list and checks the leaf matches the result.
    fn check_extract_value(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        d: &crate::instr_types::ExtractValueInstData,
    ) -> IrResult<()> {
        let agg_ty = self.value_type(d.aggregate.get());
        let leaf_ty =
            walk_aggregate_path(self.module, agg_ty, &d.indices).map_err(|e| match e {
                AggWalkErr::NotAggregate(at) => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateOpNonAggregate,
                    format!("operand type {} is not aggregate", self.type_label(at)),
                ),
                AggWalkErr::OutOfRange { idx, count } => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateIndexOutOfRange,
                    format!("index {idx} >= {count}"),
                ),
            })?;
        if inst.ty().id != leaf_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AggregateOpNonAggregate,
                format!(
                    "result {} != leaf {}",
                    self.type_label(inst.ty().id),
                    self.type_label(leaf_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitInsertValueInst`.
    fn check_insert_value(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        d: &crate::instr_types::InsertValueInstData,
    ) -> IrResult<()> {
        let agg_ty = self.value_type(d.aggregate.get());
        let val_ty = self.value_type(d.value.get());
        let leaf_ty =
            walk_aggregate_path(self.module, agg_ty, &d.indices).map_err(|e| match e {
                AggWalkErr::NotAggregate(at) => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateOpNonAggregate,
                    format!("operand type {} is not aggregate", self.type_label(at)),
                ),
                AggWalkErr::OutOfRange { idx, count } => self.fail(
                    f,
                    bb,
                    VerifierRule::AggregateIndexOutOfRange,
                    format!("index {idx} >= {count}"),
                ),
            })?;
        if val_ty != leaf_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::InsertValueLeafTypeMismatch,
                format!(
                    "inserted value {} != leaf {}",
                    self.type_label(val_ty),
                    self.type_label(leaf_ty)
                ),
            ));
        }
        if inst.ty().id != agg_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::InsertValueLeafTypeMismatch,
                format!(
                    "result {} != aggregate {}",
                    self.type_label(inst.ty().id),
                    self.type_label(agg_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitExtractElementInst`. Vector operand element type
    /// must equal the result type; the index must be integer-typed.
    fn check_extract_element(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        d: &crate::instr_types::ExtractElementInstData,
    ) -> IrResult<()> {
        let vec_ty = self.value_type(d.vector.get());
        let idx_ty = self.value_type(d.index.get());
        let elem = match self.module.context().type_data(vec_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::VectorElementOpTypeMismatch,
                    format!("vector operand {} is not a vector", self.type_label(vec_ty)),
                ));
            }
        };
        if self
            .module
            .context()
            .type_data(idx_ty)
            .as_integer()
            .is_none()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!("index {} is not an integer", self.type_label(idx_ty)),
            ));
        }
        if inst.ty().id != elem {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "result {} != element {}",
                    self.type_label(inst.ty().id),
                    self.type_label(elem)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitInsertElementInst`.
    fn check_insert_element(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        d: &crate::instr_types::InsertElementInstData,
    ) -> IrResult<()> {
        let vec_ty = self.value_type(d.vector.get());
        let val_ty = self.value_type(d.value.get());
        let idx_ty = self.value_type(d.index.get());
        let elem = match self.module.context().type_data(vec_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::VectorElementOpTypeMismatch,
                    format!("vector operand {} is not a vector", self.type_label(vec_ty)),
                ));
            }
        };
        if val_ty != elem {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "inserted value {} != element {}",
                    self.type_label(val_ty),
                    self.type_label(elem)
                ),
            ));
        }
        if self
            .module
            .context()
            .type_data(idx_ty)
            .as_integer()
            .is_none()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!("index {} is not an integer", self.type_label(idx_ty)),
            ));
        }
        if inst.ty().id != vec_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::VectorElementOpTypeMismatch,
                format!(
                    "result {} != vector {}",
                    self.type_label(inst.ty().id),
                    self.type_label(vec_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitShuffleVectorInst`. Both vector operands must
    /// agree on element type; the result is `<mask.len() x elem>`.
    fn check_shuffle_vector(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        d: &crate::instr_types::ShuffleVectorInstData,
    ) -> IrResult<()> {
        let l_ty = self.value_type(d.lhs.get());
        let r_ty = self.value_type(d.rhs.get());
        let l_elem = match self.module.context().type_data(l_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::ShuffleVectorTypeMismatch,
                    format!("lhs {} is not a vector", self.type_label(l_ty)),
                ));
            }
        };
        let r_elem = match self.module.context().type_data(r_ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::ShuffleVectorTypeMismatch,
                    format!("rhs {} is not a vector", self.type_label(r_ty)),
                ));
            }
        };
        if l_elem != r_elem {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::ShuffleVectorTypeMismatch,
                format!(
                    "lhs element {} != rhs element {}",
                    self.type_label(l_elem),
                    self.type_label(r_elem)
                ),
            ));
        }
        // Result type element should equal the operand element; result
        // length should equal mask length. We compare via vector data.
        match self.module.context().type_data(inst.ty().id).as_vector() {
            Some((re, n, _)) => {
                let Ok(result_len) = usize::try_from(n) else {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::ShuffleVectorTypeMismatch,
                        "result vector length does not fit this host".to_string(),
                    ));
                };
                if re != l_elem || result_len != d.mask.len() {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::ShuffleVectorTypeMismatch,
                        "result vector shape disagrees with operands or mask length".to_string(),
                    ));
                }
            }
            None => {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::ShuffleVectorTypeMismatch,
                    format!("result {} is not a vector", self.type_label(inst.ty().id)),
                ));
            }
        }
        Ok(())
    }

    /// `Verifier::visitFenceInst`. The ordering must be one of
    /// `acquire`/`release`/`acq_rel`/`seq_cst`.
    fn check_fence(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        d: &crate::instr_types::FenceInstData,
    ) -> IrResult<()> {
        use crate::atomic_ordering::AtomicOrdering as AO;
        if !matches!(
            d.ordering,
            AO::Acquire | AO::Release | AO::AcquireRelease | AO::SequentiallyConsistent
        ) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!("fence ordering {} is invalid", d.ordering),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitAtomicCmpXchgInst`. The pointer must be a
    /// pointer; cmp / new value types must match; orderings must be at
    /// least monotonic and the failure ordering must not be Release /
    /// AcqRel.
    fn check_cmpxchg(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        d: &crate::instr_types::AtomicCmpXchgInstData,
    ) -> IrResult<()> {
        use crate::atomic_ordering::AtomicOrdering as AO;
        let ptr_ty = self.value_type(d.ptr.get());
        if !self.module.context().type_data(ptr_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicNonPointerOperand,
                format!(
                    "cmpxchg pointer {} is not a pointer",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let cmp_ty = self.value_type(d.cmp.get());
        let new_ty = self.value_type(d.new_val.get());
        if cmp_ty != new_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicRMWOperandTypeMismatch,
                format!(
                    "cmpxchg cmp {} != new {}",
                    self.type_label(cmp_ty),
                    self.type_label(new_ty)
                ),
            ));
        }
        let strong_enough = |o: AO| {
            matches!(
                o,
                AO::Monotonic
                    | AO::Acquire
                    | AO::Release
                    | AO::AcquireRelease
                    | AO::SequentiallyConsistent
            )
        };
        if !strong_enough(d.success_ordering) || !strong_enough(d.failure_ordering) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!(
                    "cmpxchg orderings ({}, {}) must be at least monotonic",
                    d.success_ordering, d.failure_ordering
                ),
            ));
        }
        if matches!(d.failure_ordering, AO::Release | AO::AcquireRelease) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!(
                    "cmpxchg failure ordering {} cannot be Release/AcqRel",
                    d.failure_ordering
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitAtomicRMWInst`.
    fn check_atomicrmw(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        d: &crate::instr_types::AtomicRMWInstData,
    ) -> IrResult<()> {
        use crate::atomic_ordering::AtomicOrdering as AO;
        let ptr_ty = self.value_type(d.ptr.get());
        if !self.module.context().type_data(ptr_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicNonPointerOperand,
                format!(
                    "atomicrmw pointer {} is not a pointer",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let val_ty = self.value_type(d.value.get());
        if d.op.is_fp_operation() && !is_fp_or_fp_vector(self.module, val_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicRMWOperandTypeMismatch,
                format!(
                    "atomicrmw {} operand {} is not floating-point",
                    d.op.keyword(),
                    self.type_label(val_ty)
                ),
            ));
        }
        if !matches!(
            d.ordering,
            AO::Monotonic
                | AO::Acquire
                | AO::Release
                | AO::AcquireRelease
                | AO::SequentiallyConsistent
        ) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicInvalidOrdering,
                format!(
                    "atomicrmw ordering {} must be at least monotonic",
                    d.ordering
                ),
            ));
        }
        if inst.ty().id != val_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicRMWOperandTypeMismatch,
                format!(
                    "atomicrmw result {} != value {}",
                    self.type_label(inst.ty().id),
                    self.type_label(val_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitICmpInst`.
    fn check_icmp(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        c: &CmpInstData,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(c.lhs.get());
        let rhs_ty = self.value_type(c.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IcmpOperandTypeMismatch,
                format!(
                    "lhs {} differs from rhs {}",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_int_or_int_vector(self.module, lhs_ty)
            && !is_pointer_or_pointer_vector(self.module, lhs_ty)
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IcmpOperandTypeMismatch,
                format!(
                    "operand type {} is neither integer nor pointer",
                    self.type_label(lhs_ty)
                ),
            ));
        }
        // Result type must be i1 (or vector of i1 for vector compares).
        // Predicate is statically a valid IntPredicate; nothing extra
        // to assert beyond the type-level guarantee.
        let _ = c.predicate;
        let res = inst.ty();
        let res_ok = is_i1(self.module, res.id) || is_i1_vector(self.module, res.id);
        if !res_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IcmpOperandTypeMismatch,
                format!("icmp result type {} is not i1", self.type_label(res.id)),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitFCmpInst`.
    fn check_fcmp(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        c: &FCmpInstData,
    ) -> IrResult<()> {
        let lhs_ty = self.value_type(c.lhs.get());
        let rhs_ty = self.value_type(c.rhs.get());
        if lhs_ty != rhs_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FcmpOperandTypeMismatch,
                format!(
                    "lhs {} differs from rhs {}",
                    self.type_label(lhs_ty),
                    self.type_label(rhs_ty)
                ),
            ));
        }
        if !is_fp_or_fp_vector(self.module, lhs_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FcmpOperandTypeMismatch,
                format!(
                    "operand type {} is not floating-point",
                    self.type_label(lhs_ty)
                ),
            ));
        }
        let res_ok = is_i1(self.module, inst.ty().id) || is_i1_vector(self.module, inst.ty().id);
        if !res_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::FcmpOperandTypeMismatch,
                format!(
                    "fcmp result type {} is not i1",
                    self.type_label(inst.ty().id)
                ),
            ));
        }
        Ok(())
    }

    /// Cast opcodes. Mirrors the per-opcode `Verifier::visit{Cast}Inst`
    /// family.
    fn check_cast(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        c: &CastOpData,
    ) -> IrResult<()> {
        let src_ty = self.value_type(c.src.get());
        let dst_ty = inst.ty().id;
        match c.kind {
            CastOpcode::Trunc | CastOpcode::ZExt | CastOpcode::SExt => {
                let src_w = self.int_width_or_err(f, bb, src_ty, "source")?;
                let dst_w = self.int_width_or_err(f, bb, dst_ty, "destination")?;
                let ok = match c.kind {
                    CastOpcode::Trunc => dst_w < src_w,
                    CastOpcode::ZExt | CastOpcode::SExt => dst_w > src_w,
                    _ => unreachable!("matched only int-to-int casts here"),
                };
                if !ok {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastWidthMismatch,
                        format!("{} from i{src_w} to i{dst_w}", c.kind.keyword()),
                    ));
                }
            }
            CastOpcode::FpTrunc | CastOpcode::FpExt => {
                let src_rank = fp_rank(self.module, src_ty);
                let dst_rank = fp_rank(self.module, dst_ty);
                match (src_rank, dst_rank) {
                    (Some(s), Some(d)) => {
                        let ok = match c.kind {
                            CastOpcode::FpTrunc => d < s,
                            CastOpcode::FpExt => d > s,
                            _ => unreachable!(),
                        };
                        if !ok {
                            return Err(self.fail(
                                f,
                                bb,
                                VerifierRule::CastWidthMismatch,
                                format!(
                                    "{} from {} to {}",
                                    c.kind.keyword(),
                                    self.type_label(src_ty),
                                    self.type_label(dst_ty)
                                ),
                            ));
                        }
                    }
                    _ => {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            format!(
                                "{} requires floating-point operands; got {} -> {}",
                                c.kind.keyword(),
                                self.type_label(src_ty),
                                self.type_label(dst_ty)
                            ),
                        ));
                    }
                }
            }
            CastOpcode::FpToUI | CastOpcode::FpToSI => {
                if !is_fp_or_fp_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "{} source must be floating-point, got {}",
                            c.kind.keyword(),
                            self.type_label(src_ty)
                        ),
                    ));
                }
                if !is_int_or_int_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "{} destination must be integer, got {}",
                            c.kind.keyword(),
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
            CastOpcode::UIToFp | CastOpcode::SIToFp => {
                if !is_int_or_int_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "{} source must be integer, got {}",
                            c.kind.keyword(),
                            self.type_label(src_ty)
                        ),
                    ));
                }
                if !is_fp_or_fp_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "{} destination must be floating-point, got {}",
                            c.kind.keyword(),
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
            CastOpcode::PtrToAddr | CastOpcode::PtrToInt => {
                if !is_pointer_or_pointer_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "{} source must be pointer, got {}",
                            c.kind.keyword(),
                            self.type_label(src_ty)
                        ),
                    ));
                }
                if !is_int_or_int_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "{} destination must be integer, got {}",
                            c.kind.keyword(),
                            self.type_label(dst_ty)
                        ),
                    ));
                }
                if c.kind == CastOpcode::PtrToAddr {
                    let Some((addr_space, src_shape)) = pointer_source_shape(self.module, src_ty)
                    else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            "ptrtoaddr source must be pointer".to_owned(),
                        ));
                    };
                    let Some((dst_bits, dst_shape)) = integer_result_shape(self.module, dst_ty)
                    else {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            "ptrtoaddr destination must be integer".to_owned(),
                        ));
                    };
                    let index_bits = self.module.data_layout().index_size_in_bits(addr_space);
                    if dst_bits != index_bits || src_shape != dst_shape {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            "ptrtoaddr result must be address width".to_owned(),
                        ));
                    }
                }
            }
            CastOpcode::IntToPtr => {
                if !is_int_or_int_vector(self.module, src_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "inttoptr source must be integer, got {}",
                            self.type_label(src_ty)
                        ),
                    ));
                }
                if !is_pointer_or_pointer_vector(self.module, dst_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "inttoptr destination must be pointer, got {}",
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
            CastOpcode::BitCast => {
                // Bitcast must preserve bit width. Pointer-to-pointer
                // bitcasts in the same address space are identity in
                // LLVM 17+ (opaque pointers); LangRef accepts them.
                // Cross-address-space pointer reinterpretation must use
                // `addrspacecast` instead.
                let src_data = self.module.context().type_data(src_ty);
                let dst_data = self.module.context().type_data(dst_ty);
                if src_ty == dst_ty {
                    // Identity is always fine.
                } else if let (Some(src_as), Some(dst_as)) =
                    (src_data.as_pointer(), dst_data.as_pointer())
                {
                    if src_as != dst_as {
                        return Err(self.fail(
                            f,
                            bb,
                            VerifierRule::CastTypeMismatch,
                            format!(
                                "bitcast across address spaces ({src_as} -> {dst_as}); use addrspacecast"
                            ),
                        ));
                    }
                } else {
                    let src_bits = type_bit_width(self.module, src_ty);
                    let dst_bits = type_bit_width(self.module, dst_ty);
                    match (src_bits, dst_bits) {
                        (Some(s), Some(d)) if s == d => {}
                        (Some(s), Some(d)) => {
                            return Err(self.fail(
                                f,
                                bb,
                                VerifierRule::BitCastSizeMismatch,
                                format!("bitcast {s}-bit -> {d}-bit"),
                            ));
                        }
                        _ => {
                            return Err(self.fail(
                                f,
                                bb,
                                VerifierRule::CastTypeMismatch,
                                format!(
                                    "bitcast requires sized scalar/vector/pointer types; got {} -> {}",
                                    self.type_label(src_ty),
                                    self.type_label(dst_ty)
                                ),
                            ));
                        }
                    }
                }
            }
            CastOpcode::AddrSpaceCast => {
                if !is_pointer_or_pointer_vector(self.module, src_ty)
                    || !is_pointer_or_pointer_vector(self.module, dst_ty)
                {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::CastTypeMismatch,
                        format!(
                            "addrspacecast requires pointer operands; got {} -> {}",
                            self.type_label(src_ty),
                            self.type_label(dst_ty)
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// `Verifier::visitAllocaInst`.
    fn check_alloca(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        a: &crate::instr_types::AllocaInstData,
    ) -> IrResult<()> {
        let allocated = Type::new(a.allocated_ty, self.module);
        if SizedType::try_from(allocated).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AllocaUnsizedType,
                format!(
                    "alloca'd type {} is unsized",
                    self.type_label(a.allocated_ty)
                ),
            ));
        }
        if let Some(count_id) = a.num_elements.get() {
            let count_ty = self.value_type(count_id);
            if !is_int_or_int_vector(self.module, count_ty) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::AllocaNonIntegerCount,
                    format!(
                        "alloca count operand has type {} (expected integer)",
                        self.type_label(count_ty)
                    ),
                ));
            }
        }
        // Verifier.cpp `visitAllocaInst`: a swifterror alloca must have
        // pointer type and must not be an array allocation.
        if a.flags.is_swifterror() {
            if !self
                .module
                .context()
                .type_data(a.allocated_ty)
                .is_pointer_data()
            {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SwiftErrorAlloca,
                    format!(
                        "swifterror alloca must have pointer type, got {}",
                        self.type_label(a.allocated_ty)
                    ),
                ));
            }
            // `isArrayAllocation()` is false for an omitted size or a
            // constant-`1` size, so those are permitted.
            if a.num_elements
                .get()
                .is_some_and(|count| !crate::constants::is_constant_int_one(self.module, count))
            {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SwiftErrorAlloca,
                    "swifterror alloca must not be an array allocation".to_owned(),
                ));
            }
        }
        // Result type must be a pointer; the IRBuilder construction
        // path always emits one, but assert it for parsed/foreign IR.
        if !self
            .module
            .context()
            .type_data(inst.ty().id)
            .is_pointer_data()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AllocaUnsizedType,
                format!(
                    "alloca result type {} is not a pointer",
                    self.type_label(inst.ty().id)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitLoadInst`.
    fn check_load(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        l: &crate::instr_types::LoadInstData,
    ) -> IrResult<()> {
        let ptr_ty = self.value_type(l.ptr.get());
        if !is_pointer_or_pointer_vector(self.module, ptr_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::LoadNonPointer,
                format!(
                    "load pointer operand has type {} (expected pointer)",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let pointee = Type::new(l.pointee_ty, self.module);
        if SizedType::try_from(pointee).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::LoadUnsizedType,
                format!(
                    "load pointee type {} is unsized",
                    self.type_label(l.pointee_ty)
                ),
            ));
        }
        // Result type must equal pointee type.
        if inst.ty().id != l.pointee_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::LoadUnsizedType,
                format!(
                    "load result type {} != pointee {}",
                    self.type_label(inst.ty().id),
                    self.type_label(l.pointee_ty)
                ),
            ));
        }
        // Atomic-specific rules. Mirrors `Verifier::visitLoadInst`.
        if l.is_atomic() {
            use crate::atomic_ordering::AtomicOrdering;
            if matches!(
                l.ordering,
                AtomicOrdering::Release | AtomicOrdering::AcquireRelease
            ) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::AtomicLoadInvalidOrdering,
                    format!("atomic load has ordering {}", l.ordering),
                ));
            }
            self.check_atomic_access_type(f, bb, l.pointee_ty, "load")?;
            self.check_atomic_access_size(f, bb, l.pointee_ty)?;
        } else if !l.sync_scope.is_default() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::NonAtomicWithSyncScope,
                "non-atomic load carries a non-default syncscope".to_string(),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitStoreInst`.
    fn check_store(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        s: &crate::instr_types::StoreInstData,
    ) -> IrResult<()> {
        let ptr_ty = self.value_type(s.ptr.get());
        if !is_pointer_or_pointer_vector(self.module, ptr_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::StoreNonPointer,
                format!(
                    "store pointer operand has type {} (expected pointer)",
                    self.type_label(ptr_ty)
                ),
            ));
        }
        let val_ty = self.value_type(s.value.get());
        if SizedType::try_from(Type::new(val_ty, self.module)).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::StoreUnsizedType,
                format!("store value type {} is unsized", self.type_label(val_ty)),
            ));
        }
        // Atomic-specific rules. Mirrors `Verifier::visitStoreInst`.
        if s.is_atomic() {
            use crate::atomic_ordering::AtomicOrdering;
            if matches!(
                s.ordering,
                AtomicOrdering::Acquire | AtomicOrdering::AcquireRelease
            ) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::AtomicStoreInvalidOrdering,
                    format!("atomic store has ordering {}", s.ordering),
                ));
            }
            self.check_atomic_access_type(f, bb, val_ty, "store")?;
            self.check_atomic_access_size(f, bb, val_ty)?;
        } else if !s.sync_scope.is_default() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::NonAtomicWithSyncScope,
                "non-atomic store carries a non-default syncscope".to_string(),
            ));
        }
        Ok(())
    }

    /// Mirrors `Verifier::visitLoadInst` / `visitStoreInst` operand-type
    /// branch: atomic load/store operands must be integer, pointer,
    /// floating-point, or a vector thereof.
    fn check_atomic_access_type(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        ty: TypeId,
        kind: &str,
    ) -> IrResult<()> {
        if is_int_or_int_vector(self.module, ty)
            || is_fp_or_fp_vector(self.module, ty)
            || is_pointer_or_pointer_vector(self.module, ty)
        {
            return Ok(());
        }
        Err(self.fail(
            f,
            bb,
            VerifierRule::AtomicLoadStoreInvalidType,
            format!("atomic {} operand has type {}", kind, self.type_label(ty)),
        ))
    }

    /// Mirrors `Verifier::checkAtomicMemAccessSize` in `lib/IR/Verifier.cpp`:
    /// the operand bit width must be at least 8 (byte-sized) and a power
    /// of two.
    fn check_atomic_access_size(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        ty: TypeId,
    ) -> IrResult<()> {
        let Some(bits) = type_bit_width(self.module, ty) else {
            // Pointers (no statically-known bit width) are accepted by
            // upstream because the data layout decides; we have no
            // DataLayout yet, so accept silently.
            return Ok(());
        };
        if bits < 8 || (bits & (bits - 1)) != 0 {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::AtomicLoadStoreInvalidSize,
                format!(
                    "atomic access bit width {} is not byte-sized and power-of-two",
                    bits
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitGetElementPtrInst`. Constructive subset.
    fn check_gep(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        g: &GepInstData,
    ) -> IrResult<()> {
        let base_ty = self.value_type(g.ptr.get());
        if !is_pointer_or_pointer_vector(self.module, base_ty) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepNonPointerBase,
                format!(
                    "getelementptr base operand has type {} (expected pointer)",
                    self.type_label(base_ty)
                ),
            ));
        }
        let source = Type::new(g.source_ty, self.module);
        if SizedType::try_from(source).is_err() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepUnsizedSourceType,
                format!(
                    "getelementptr source element type {} is unsized",
                    self.type_label(g.source_ty)
                ),
            ));
        }
        for (slot, idx_id) in g.indices.iter().map(|c| c.get()).enumerate() {
            let idx_ty = self.value_type(idx_id);
            if !is_int_or_int_vector(self.module, idx_ty) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::GepNonIntegerIndex,
                    format!(
                        "getelementptr index #{slot} has type {} (expected integer)",
                        self.type_label(idx_ty)
                    ),
                ));
            }
        }
        // The index sequence must index into the source element type
        // (`Verifier::visitGetElementPtrInst` checks
        // `GetElementPtrInst::getIndexedType(SourceTy, Idxs)` is non-null).
        let idx_ids: Vec<_> = g.indices.iter().map(|c| c.get()).collect();
        if crate::constants::gep_indexed_type(self.module, g.source_ty, &idx_ids).is_none() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::GepInvalidIndices,
                format!(
                    "getelementptr indices do not index into source type {}",
                    self.type_label(g.source_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitCallBase`.
    fn check_call(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        c: &crate::instr_types::CallInstData,
    ) -> IrResult<()> {
        // Callee must be a function value, OR a pointer of address
        // space 0 with a separately-tracked function-type (LLVM 17+
        // opaque-pointer model). The IRBuilder always emits a
        // function-typed callee; we accept either function or pointer
        // here so future indirect-call construction does not require
        // a verifier change.
        let callee_ty = self.value_type(c.callee.get());
        let callee_ok = self
            .module
            .context()
            .type_data(callee_ty)
            .is_function_data()
            || self.module.context().type_data(callee_ty).is_pointer_data();
        if !callee_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallNonFunction,
                format!(
                    "call callee has type {} (expected function or pointer)",
                    self.type_label(callee_ty)
                ),
            ));
        }
        // Argument count and types must match `c.fn_ty`.
        let fn_ty_data = self.module.context().type_data(c.fn_ty);
        let Some((_ret, params, is_var_arg)) = fn_ty_data.as_function() else {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallNonFunction,
                format!(
                    "call fn_ty {} is not a function type",
                    self.type_label(c.fn_ty)
                ),
            ));
        };
        let n_args = c.args.len();
        let n_params = params.len();
        if (is_var_arg && n_args < n_params) || (!is_var_arg && n_args != n_params) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallArgCountMismatch,
                format!(
                    "call passes {n_args} args but signature expects {n_params}{}",
                    if is_var_arg { "+ (vararg)" } else { "" }
                ),
            ));
        }
        for (slot, (arg_cell, &param_ty)) in c.args.iter().zip(params.iter()).enumerate() {
            let arg_ty = self.value_type(arg_cell.get());
            if arg_ty != param_ty {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgTypeMismatch,
                    format!(
                        "call arg #{slot} has type {} but signature expects {}",
                        self.type_label(arg_ty),
                        self.type_label(param_ty)
                    ),
                ));
            }
        }
        self.check_intrinsic_call(f, bb, c.callee.get(), c.fn_ty, &c.args)?;
        if let ValueKindData::InlineAsm(_) = &self.module.context().value_data(c.callee.get()).kind
        {
            let inline_asm =
                crate::inline_asm::InlineAsm::from_parts(c.callee.get(), self.module, callee_ty);
            let summary = inline_asm.constraint_summary();
            let _arg_constraints = summary.arg_constraints;
            if summary.label_count != 0 {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgCountMismatch,
                    "Label constraints can only be used with callbr".to_owned(),
                ));
            }
            // Full indirect-constraint / elementtype parity is deferred: the
            // current call surface cannot spell per-operand elementtype attrs.
        }

        Ok(())
    }

    fn check_intrinsic_call(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        callee_id: ValueId,
        fn_ty: TypeId,
        args: &[core::cell::Cell<ValueId>],
    ) -> IrResult<()> {
        let callee_data = self.module.context().value_data(callee_id);
        let ValueKindData::Function(_) = &callee_data.kind else {
            return Ok(());
        };
        let callee = crate::value::Value::from_parts(callee_id, self.module, callee_data.ty);
        let Some(descriptor) = crate::intrinsics::descriptor_for_callee(callee) else {
            return Ok(());
        };
        let expected = descriptor
            .function_type_ref(crate::module::ModuleRef::new(self.module))
            .map_err(|_| {
                self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgTypeMismatch,
                    "intrinsic signature mismatch".to_string(),
                )
            })?;
        if expected.as_type().id() != fn_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::CallArgTypeMismatch,
                "intrinsic signature mismatch".to_string(),
            ));
        }
        for index in descriptor.immarg_operand_indices() {
            let Some(arg) = args.get(index) else {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgCountMismatch,
                    "intrinsic signature mismatch".to_string(),
                ));
            };
            if !matches!(
                self.module.context().value_data(arg.get()).kind,
                ValueKindData::Constant(
                    crate::constant::ConstantData::Int(_) | crate::constant::ConstantData::Float(_)
                )
            ) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::CallArgTypeMismatch,
                    "immarg operand has non-immediate parameter".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// `Verifier::visitSelectInst`.
    fn check_select(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        s: &crate::instr_types::SelectInstData,
    ) -> IrResult<()> {
        let cond_ty = self.value_type(s.cond.get());
        let result_ty = inst.ty().id;
        let true_ty = self.value_type(s.true_val.get());
        let false_ty = self.value_type(s.false_val.get());
        // Condition must be i1 or <N x i1>; if vector, its element
        // count must match the result-vector element count.
        let cond_ok = if is_i1(self.module, cond_ty) {
            true
        } else if let Some((cond_elem, cond_n, _)) =
            self.module.context().type_data(cond_ty).as_vector()
            && is_i1_data(self.module.context().type_data(cond_elem))
        {
            // Result must also be a vector with the same length.
            if let Some((_, res_n, _)) = self.module.context().type_data(result_ty).as_vector() {
                cond_n == res_n
            } else {
                false
            }
        } else {
            false
        };
        if !cond_ok {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::SelectConditionNotI1,
                format!(
                    "select condition has type {} (expected i1 or <N x i1>)",
                    self.type_label(cond_ty)
                ),
            ));
        }
        if true_ty != false_ty || true_ty != result_ty {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::SelectArmTypeMismatch,
                format!(
                    "select arms have types {}/{} (result {})",
                    self.type_label(true_ty),
                    self.type_label(false_ty),
                    self.type_label(result_ty)
                ),
            ));
        }
        Ok(())
    }

    /// `Verifier::visitPHINode`.
    fn check_phi(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        p: &PhiData,
        predecessors: &HashMap<ValueId, Vec<ValueId>>,
        reachable: bool,
    ) -> IrResult<()> {
        let result_ty = inst.ty().id;

        // The phi result type must be a first-class *data* type. `is_first_class`
        // is not a sufficient gate — it admits `label`/`metadata`/`token` — so
        // enumerate the valid kinds exactly as the `.ll` parser's `parse_phi`
        // whitelist does (int / float / pointer / vector / array / struct), with
        // the `is_first_class` conjunct on struct excluding opaque structs. This
        // runs before the coherence delegation so an invalid result type is
        // rejected regardless of incoming coherence.
        let rty = Type::new(result_ty, self.module);
        let valid_result = rty.is_integer()
            || rty.is_floating_point()
            || rty.is_pointer()
            || rty.is_typed_pointer()
            || rty.is_vector()
            || rty.is_array()
            || (rty.is_struct() && rty.is_first_class());
        if !valid_result {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiInvalidResultType,
                format!(
                    "phi result type {} is not a valid first-class data type",
                    self.type_label(result_ty)
                ),
            ));
        }

        let preds = predecessors
            .get(&bb.as_value().id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        // Snapshot the (value, predecessor-block) pairs and delegate to
        // the shared coherence core so the parser (which runs the same
        // helper) cannot drift from the verifier. Each `PhiViolation`
        // maps back to the verifier's existing byte-identical diagnostic.
        let incoming: Vec<(ValueId, ValueId)> = p
            .incoming
            .borrow()
            .iter()
            .map(|(v, b)| (v.get(), *b))
            .collect();

        // Defense in depth (stricter than upstream). A phi with zero incomings
        // in a block reachable from entry prints as `%p = phi i32` with no
        // `[ … ]` pairs — un-round-trippable, since `LLParser::parsePHI` rejects
        // it. `check_phi_incoming` below would miss this: its only length guard
        // is `incoming.len() != preds.len()`, so a zero-incoming phi in a
        // zero-predecessor block passes on `0 == 0` (the same gap as LLVM's
        // `visitPHINode`). We run before that delegation and gate on
        // reachability — an unreachable block may legitimately have no
        // predecessors, so we do not force its phis to carry incomings.
        if reachable && incoming.is_empty() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiEmptyInReachableBlock,
                "phi in a block reachable from entry has no incoming values".into(),
            ));
        }

        let value_ty_of = |id: ValueId| self.value_type(id);
        match check_phi_incoming(result_ty, &incoming, preds, &value_ty_of) {
            Ok(()) => Ok(()),
            Err(PhiViolation::CountMismatch { entries, preds }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                format!("phi has {entries} incoming entries but block has {preds} predecessors"),
            )),
            Err(PhiViolation::NotAPredecessor { block }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                format!(
                    "phi incoming block %{} is not a predecessor",
                    slot_label(self.module, block)
                ),
            )),
            Err(PhiViolation::TooManyFromBlock { block }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                format!(
                    "phi has too many incoming entries from block %{}",
                    slot_label(self.module, block)
                ),
            )),
            Err(PhiViolation::AmbiguousValues { block }) => Err(self.fail(
                f,
                bb,
                VerifierRule::AmbiguousPhi,
                format!(
                    "phi has multiple entries for block %{} with different values",
                    slot_label(self.module, block)
                ),
            )),
            Err(PhiViolation::IncomingTypeMismatch { block, value_ty }) => Err(self.fail(
                f,
                bb,
                VerifierRule::PhiIncomingTypeMismatch,
                format!(
                    "phi expects {} but incoming from %{} is {}",
                    self.type_label(result_ty),
                    slot_label(self.module, block),
                    self.type_label(value_ty)
                ),
            )),
        }
    }

    /// `Verifier::visitReturnInst`.
    fn check_ret(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        r: &ReturnOpData,
    ) -> IrResult<()> {
        let expected = f.return_type();
        match (r.value.get(), expected.is_void()) {
            (None, true) => Ok(()),
            (None, false) => Err(self.fail(
                f,
                bb,
                VerifierRule::ReturnTypeMismatch,
                format!(
                    "ret has no operand but function returns {}",
                    expected.kind_label()
                ),
            )),
            (Some(_), true) => Err(self.fail(
                f,
                bb,
                VerifierRule::ReturnTypeMismatch,
                "void function cannot return a value".into(),
            )),
            (Some(v), false) => {
                let actual = self.value_type(v);
                if actual == expected.id {
                    Ok(())
                } else {
                    Err(self.fail(
                        f,
                        bb,
                        VerifierRule::ReturnTypeMismatch,
                        format!(
                            "ret operand has type {} but function returns {}",
                            self.type_label(actual),
                            expected.kind_label()
                        ),
                    ))
                }
            }
        }
    }

    /// `Verifier::visitSwitchInst`. The condition must be an integer
    /// type; every case value must share that type; every successor
    /// (default + cases) must belong to the parent function.
    fn check_switch(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        d: &crate::instr_types::SwitchInstData,
        block_index: &HashMap<ValueId, usize>,
    ) -> IrResult<()> {
        let cond_ty = self.value_type(d.cond.get());
        if self
            .module
            .context()
            .type_data(cond_ty)
            .as_integer()
            .is_none()
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::SwitchOperandTypeMismatch,
                format!(
                    "switch condition {} is not integer",
                    self.type_label(cond_ty)
                ),
            ));
        }
        if !block_index.contains_key(&d.default_bb.get()) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                "switch default target is not a basic block of the parent function".into(),
            ));
        }
        for (case_v, case_bb) in d.cases.borrow().iter() {
            let v_ty = self.value_type(case_v.get());
            if v_ty != cond_ty {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SwitchOperandTypeMismatch,
                    format!(
                        "switch case value {} != condition {}",
                        self.type_label(v_ty),
                        self.type_label(cond_ty)
                    ),
                ));
            }
            if !block_index.contains_key(case_bb) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiPredecessorMismatch,
                    "switch case target is not a basic block of the parent function".into(),
                ));
            }
        }
        Ok(())
    }

    /// `Verifier::visitIndirectBrInst`. The address operand must be a
    /// pointer; every destination must belong to the parent function.
    fn check_indirectbr(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        d: &crate::instr_types::IndirectBrInstData,
        block_index: &HashMap<ValueId, usize>,
    ) -> IrResult<()> {
        let addr_ty = self.value_type(d.addr.get());
        if !self.module.context().type_data(addr_ty).is_pointer_data() {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::IndirectBrNonPointerAddress,
                format!(
                    "indirectbr address {} is not a pointer",
                    self.type_label(addr_ty)
                ),
            ));
        }
        for &dest in d.destinations.borrow().iter() {
            if !block_index.contains_key(&dest) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiPredecessorMismatch,
                    "indirectbr destination is not a basic block of the parent function".into(),
                ));
            }
        }
        Ok(())
    }

    /// `Verifier::visitInvokeInst`. Constructive subset: every
    /// destination is a basic block of the parent function. Callee /
    /// arg type checks reuse the same logic as [`Self::check_call`]
    /// but specialised inline since the storage payload differs.
    fn check_invoke(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        d: &crate::instr_types::InvokeInstData,
        block_index: &HashMap<ValueId, usize>,
    ) -> IrResult<()> {
        if !block_index.contains_key(&d.normal_dest.get())
            || !block_index.contains_key(&d.unwind_dest.get())
        {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                "invoke destination is not a basic block of the parent function".into(),
            ));
        }
        self.check_intrinsic_call(f, bb, d.callee.get(), d.fn_ty, &d.args)?;
        Ok(())
    }

    /// `Verifier::visitCallBrInst`. Constructive subset: every
    /// destination is a basic block of the parent function.
    fn check_callbr(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        d: &crate::instr_types::CallBrInstData,
        block_index: &HashMap<ValueId, usize>,
    ) -> IrResult<()> {
        if !block_index.contains_key(&d.default_dest.get()) {
            return Err(self.fail(
                f,
                bb,
                VerifierRule::PhiPredecessorMismatch,
                "callbr default destination is not a basic block of the parent function".into(),
            ));
        }
        for ic in d.indirect_dests.iter() {
            if !block_index.contains_key(&ic.get()) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::PhiPredecessorMismatch,
                    "callbr indirect destination is not a basic block of the parent function"
                        .into(),
                ));
            }
        }
        self.check_intrinsic_call(f, bb, d.callee.get(), d.fn_ty, &d.args)?;
        Ok(())
    }

    /// `Verifier::visitBranchInst`.
    fn check_br(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        _inst: &InstructionView<'ctx>,
        b: &BranchInstData,
        block_index: &HashMap<ValueId, usize>,
    ) -> IrResult<()> {
        match &*b.kind.borrow() {
            BranchKind::Unconditional(target) => {
                if !block_index.contains_key(target) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::PhiPredecessorMismatch,
                        "br target is not a basic block of the parent function".into(),
                    ));
                }
            }
            BranchKind::Conditional {
                cond,
                then_bb,
                else_bb,
            } => {
                let cond_ty = self.value_type(cond.get());
                if !is_i1(self.module, cond_ty) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::BranchConditionNotI1,
                        format!(
                            "br condition has type {} (expected i1)",
                            self.type_label(cond_ty)
                        ),
                    ));
                }
                if !block_index.contains_key(then_bb) || !block_index.contains_key(else_bb) {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::PhiPredecessorMismatch,
                        "br target is not a basic block of the parent function".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Cross-block SSA dominance. Mirrors `Verifier::verifyDominatesUse`,
    /// using `DominatorTree` directly rather than the analysis manager.
    fn check_dominates_uses(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        dom_tree: &DominatorTree,
    ) -> IrResult<()> {
        let operands = inst.operand_ids();
        for (index, op_id) in operands.into_iter().enumerate() {
            let op_data = self.module.context().value_data(op_id);
            let operand = crate::Value::from_parts(op_id, self.module, op_data.ty);
            let index = u32::try_from(index)
                .unwrap_or_else(|_| unreachable!("instruction operand index exceeds u32::MAX"));
            let use_edge = crate::Use::new(inst.as_value(), operand, index);
            if !dom_tree.dominates_use(operand, use_edge) {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::UseBeforeDef,
                    format!(
                        "operand %{} does not dominate its use in block %{}",
                        slot_label(self.module, op_id),
                        slot_label(self.module, bb.as_value().id)
                    ),
                ));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Universal in-block invariants
    // ------------------------------------------------------------------

    /// Self-reference + in-block use-before-def. PHI is exempt because
    /// its incoming-pair operands are semantically uses on the
    /// predecessor edge, not at the phi's own slot.
    fn check_self_reference_and_in_block_dom(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        inst: &InstructionView<'ctx>,
        index_in_block: usize,
        block_instructions: &[InstructionView<'ctx>],
    ) -> IrResult<()> {
        let is_phi = matches!(inst.kind(), Some(crate::InstructionKind::Phi(_)));
        if is_phi {
            return Ok(());
        }
        let kind = match &inst.as_value().data().kind {
            ValueKindData::Instruction(i) => &i.kind,
            _ => unreachable!("instruction handle invariant: value kind is Instruction"),
        };
        for op_id in kind.operand_ids() {
            // Self-reference (`Verifier/SelfReferential.ll`).
            if op_id == inst.as_value().id {
                return Err(self.fail(
                    f,
                    bb,
                    VerifierRule::SelfReference,
                    "non-phi instruction references its own value".into(),
                ));
            }
            // In-block use-before-def. For operands that are themselves
            // instructions in the same block: the operand's index
            // must be strictly less than `index_in_block`.
            if let ValueKindData::Instruction(op_inst) =
                &self.module.context().value_data(op_id).kind
                && op_inst.parent.get() == bb.as_value().id
            {
                // Find op_id's index in block.
                if let Some(op_idx) = block_instructions
                    .iter()
                    .position(|i| i.as_value().id == op_id)
                    && op_idx >= index_in_block
                {
                    return Err(self.fail(
                        f,
                        bb,
                        VerifierRule::UseBeforeDef,
                        "operand defined after its use within the same block".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Diagnostic helpers
    // ------------------------------------------------------------------

    fn fail(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        rule: VerifierRule,
        message: String,
    ) -> IrError {
        IrError::VerifierFailure {
            rule,
            function: Some(f.name().to_owned()),
            block: bb.name(),
            message,
        }
    }

    fn value_type(&self, id: ValueId) -> TypeId {
        self.module.context().value_data(id).ty
    }

    fn type_label(&self, id: TypeId) -> String {
        format!("{}", Type::new(id, self.module))
    }

    /// Read the integer width of `ty`, erroring with the given role
    /// label (`"source"` / `"destination"`) if it is not an integer.
    fn int_width_or_err(
        &self,
        f: FunctionValue<'ctx, Dyn>,
        bb: &BasicBlock<'ctx, Dyn>,
        ty: TypeId,
        role: &str,
    ) -> IrResult<u32> {
        match self.module.context().type_data(ty).as_integer() {
            Some(b) => Ok(b),
            None => Err(self.fail(
                f,
                bb,
                VerifierRule::CastTypeMismatch,
                format!("{role} type {} is not integer", self.type_label(ty)),
            )),
        }
    }
}

// --------------------------------------------------------------------------
// Predecessor map
// --------------------------------------------------------------------------

/// CFG predecessor map for one function. Mirrors LLVM's `pred_iterator`
/// exposed via `BasicBlock::pred_begin`; shared successor semantics live in
/// [`crate::cfg::FunctionCfg`] so every terminator family is handled in one place.
fn build_predecessors(f: FunctionValue<'_, Dyn>) -> HashMap<ValueId, Vec<ValueId>> {
    let cfg = crate::cfg::FunctionCfg::new(f);
    let mut preds: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    for edge in cfg.edges() {
        preds
            .entry(edge.end().as_value().id)
            .or_default()
            .push(edge.start().as_value().id);
    }
    preds
}

// --------------------------------------------------------------------------
// Type predicates (lifetime-free, operate on TypeId via the context)
// --------------------------------------------------------------------------

/// Recursively detects whether a type contains any scalable vector.
/// Mirrors `Type::isScalableTy` in `llvm/lib/IR/Type.cpp`.
fn type_contains_scalable(m: &ModuleCore, ty: TypeId) -> bool {
    match m.context().type_data(ty) {
        TypeData::ScalableVector { .. } => true,
        TypeData::FixedVector { elem, .. } | TypeData::Array { elem, .. } => {
            type_contains_scalable(m, *elem)
        }
        TypeData::Struct(s) => match s.body.borrow().as_ref() {
            None => false,
            Some(body) => body.elements.iter().any(|e| type_contains_scalable(m, *e)),
        },
        _ => false,
    }
}

fn scalar_type_id(m: &ModuleCore, ty: TypeId) -> TypeId {
    match m.context().type_data(ty) {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => *elem,
        _ => ty,
    }
}

fn is_int_or_int_vector(m: &ModuleCore, ty: TypeId) -> bool {
    let d = m.context().type_data(ty);
    if d.as_integer().is_some() {
        return true;
    }
    if let Some((elem, _, _)) = d.as_vector()
        && m.context().type_data(elem).as_integer().is_some()
    {
        return true;
    }
    false
}

enum AggWalkErr {
    NotAggregate(TypeId),
    OutOfRange { idx: u32, count: u32 },
}

fn walk_aggregate_path(
    m: &ModuleCore,
    root: TypeId,
    indices: &[u32],
) -> Result<TypeId, AggWalkErr> {
    let mut cur = root;
    for &idx in indices {
        let d = m.context().type_data(cur);
        match d {
            TypeData::Array { elem, n } => {
                let n_u32 = u32::try_from(*n).unwrap_or(u32::MAX);
                if idx >= n_u32 {
                    return Err(AggWalkErr::OutOfRange { idx, count: n_u32 });
                }
                cur = *elem;
            }
            TypeData::Struct(s) => {
                let body = s.body.borrow();
                match body.as_ref() {
                    Some(b) => {
                        let count = u32::try_from(b.elements.len()).unwrap_or(u32::MAX);
                        if idx >= count {
                            return Err(AggWalkErr::OutOfRange { idx, count });
                        }
                        let Ok(field_index) = usize::try_from(idx) else {
                            return Err(AggWalkErr::OutOfRange { idx, count });
                        };
                        cur = b.elements[field_index];
                    }
                    None => return Err(AggWalkErr::NotAggregate(cur)),
                }
            }
            _ => return Err(AggWalkErr::NotAggregate(cur)),
        }
    }
    Ok(cur)
}

fn is_fp_or_fp_vector(m: &ModuleCore, ty: TypeId) -> bool {
    let d = m.context().type_data(ty);
    if is_fp_data(d) {
        return true;
    }
    if let Some((elem, _, _)) = d.as_vector()
        && is_fp_data(m.context().type_data(elem))
    {
        return true;
    }
    false
}

fn is_pointer_or_pointer_vector(m: &ModuleCore, ty: TypeId) -> bool {
    let d = m.context().type_data(ty);
    if d.is_pointer_data() {
        return true;
    }
    if let Some((elem, _, _)) = d.as_vector()
        && m.context().type_data(elem).is_pointer_data()
    {
        return true;
    }
    false
}
fn pointer_source_shape(m: &ModuleCore, ty: TypeId) -> Option<(u32, Option<(u32, bool)>)> {
    match m.context().type_data(ty) {
        TypeData::Pointer { addr_space } => Some((*addr_space, None)),
        TypeData::FixedVector { elem, n } => match m.context().type_data(*elem) {
            TypeData::Pointer { addr_space } => Some((*addr_space, Some((*n, false)))),
            _ => None,
        },
        TypeData::ScalableVector { elem, min } => match m.context().type_data(*elem) {
            TypeData::Pointer { addr_space } => Some((*addr_space, Some((*min, true)))),
            _ => None,
        },
        _ => None,
    }
}

fn integer_result_shape(m: &ModuleCore, ty: TypeId) -> Option<(u32, Option<(u32, bool)>)> {
    match m.context().type_data(ty) {
        TypeData::Integer { bits } => Some((*bits, None)),
        TypeData::FixedVector { elem, n } => match m.context().type_data(*elem) {
            TypeData::Integer { bits } => Some((*bits, Some((*n, false)))),
            _ => None,
        },
        TypeData::ScalableVector { elem, min } => match m.context().type_data(*elem) {
            TypeData::Integer { bits } => Some((*bits, Some((*min, true)))),
            _ => None,
        },
        _ => None,
    }
}

fn is_i1(m: &ModuleCore, ty: TypeId) -> bool {
    matches!(m.context().type_data(ty).as_integer(), Some(1))
}

fn is_i1_vector(m: &ModuleCore, ty: TypeId) -> bool {
    if let Some((elem, _, _)) = m.context().type_data(ty).as_vector() {
        is_i1(m, elem)
    } else {
        false
    }
}

fn is_i1_data(d: &TypeData) -> bool {
    matches!(d.as_integer(), Some(1))
}

fn is_fp_data(d: &TypeData) -> bool {
    matches!(
        d,
        TypeData::Half
            | TypeData::BFloat
            | TypeData::Float
            | TypeData::Double
            | TypeData::Fp128
            | TypeData::X86Fp80
            | TypeData::PpcFp128
    )
}

/// Floating-point precision rank for `fpext` / `fptrunc` ordering.
/// Mirrors LLVM's `Type::getFPMantissaWidth`-driven comparison.
/// `bfloat` and `half` share a width but bfloat has fewer mantissa
/// bits; LangRef accepts conversions in either direction so long as
/// they are not the identity, which the per-opcode width check
/// (`s != d`) catches separately.
fn fp_rank(m: &ModuleCore, ty: TypeId) -> Option<u32> {
    match m.context().type_data(ty) {
        TypeData::Half => Some(16),
        TypeData::BFloat => Some(16),
        TypeData::Float => Some(32),
        TypeData::Double => Some(64),
        TypeData::X86Fp80 => Some(80),
        TypeData::Fp128 => Some(128),
        TypeData::PpcFp128 => Some(128),
        _ => None,
    }
}

/// Bit width of a value-bearing type, or `None` if it has no defined
/// width (function/void/label/...). Mirrors `Type::getPrimitiveSizeInBits`
/// for the cases bitcast cares about.
fn type_bit_width(m: &ModuleCore, ty: TypeId) -> Option<u32> {
    match m.context().type_data(ty) {
        TypeData::Integer { bits } => Some(*bits),
        TypeData::Half | TypeData::BFloat => Some(16),
        TypeData::Float => Some(32),
        TypeData::Double => Some(64),
        TypeData::X86Fp80 => Some(80),
        TypeData::Fp128 | TypeData::PpcFp128 => Some(128),
        // Pointers don't have a portable bit-width here; LLVM uses the
        // data-layout. We don't ship a DataLayout yet, so two opaque
        // pointers in the same address space round-trip as bitcast
        // identity (caught by source==dest equality before width).
        TypeData::Pointer { .. } => None,
        TypeData::FixedVector { elem, n } => type_bit_width(m, *elem).map(|w| w * *n),
        _ => None,
    }
}

// --------------------------------------------------------------------------
// Slot label helper
// --------------------------------------------------------------------------

/// Best-effort label for a basic-block id. Used in diagnostics; not a
/// faithful slot tracker.
fn slot_label(m: &ModuleCore, block_id: ValueId) -> String {
    let v = m.context().value_data(block_id);
    if let Some(name) = v.name.borrow().as_ref() {
        return name.clone();
    }
    format!("{:?}", block_id)
}

// --------------------------------------------------------------------------
// TypeData crate-private helper trait
// --------------------------------------------------------------------------

/// Crate-private projections used only by the verifier. Live here so
/// `TypeData` does not grow new pub(crate) helpers that the rest of
/// the IR layer would not benefit from.
trait TypeDataExt {
    fn is_pointer_data(&self) -> bool;
    fn is_function_data(&self) -> bool;
}

impl TypeDataExt for TypeData {
    fn is_pointer_data(&self) -> bool {
        matches!(self, TypeData::Pointer { .. })
    }
    fn is_function_data(&self) -> bool {
        matches!(self, TypeData::Function { .. })
    }
}

// --------------------------------------------------------------------------
// Negative tests
// --------------------------------------------------------------------------
//
// The IRBuilder is sufficiently type-safe that most invalid IR shapes
// are unrepresentable through its public API. To exercise each
// `VerifierRule` we fabricate pathological IR by reaching into the
// crate-internal value arena directly. Each test cites the upstream
// `test/Verifier/<file>.ll` fixture whose CHECK rule it ports.

/// Upstream provenance: per-rule negative tests for `class Verifier` in
/// `lib/IR/Verifier.cpp`. Each `#[test]` ports a CHECK rule from
/// `test/Verifier/*.ll` (or the equivalent `Verifier::visit*` rule), with
/// the per-test doc comments naming the specific upstream fixture or
/// member function.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Linkage;
    use crate::constant::ConstantData;
    use crate::function::FunctionValue;
    use crate::instr_types::{BinaryOpData, BranchInstData, BranchKind, PhiData, ReturnOpData};
    use crate::instruction::{InstructionKindData, build_instruction_value};
    use crate::marker::Dyn;
    use crate::module::Module;
    use crate::value::{ValueData, ValueId, ValueKindData};

    /// Append a fabricated instruction to a block, bypassing the
    /// IRBuilder's typestate. Returns the new instruction's value id.
    fn fabricate_instruction(
        m: &Module<'_>,
        bb_id: ValueId,
        result_ty: TypeId,
        kind: InstructionKindData,
    ) -> ValueId {
        let m = m.core_ref();
        let v = build_instruction_value(result_ty, bb_id, kind, None);
        let id = m.context().push_value(v);
        let bb_data = match &m.context().value_data(bb_id).kind {
            ValueKindData::BasicBlock(b) => b,
            _ => panic!("fabricate_instruction: bb_id is not a basic block"),
        };
        bb_data.instructions.borrow_mut().push(id);
        id
    }

    /// Push a fresh constant-int value of the given type.
    fn fab_const_int_id(m: &Module<'_>, ty: TypeId, value: u64) -> ValueId {
        let m = m.core_ref();
        m.context().push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Int(Box::new([value]))),
            use_list: core::cell::RefCell::new(Vec::new()),
        })
    }

    /// Push a fresh `ptr null` value.
    fn fab_null_ptr_id(m: &Module<'_>, ptr_ty: TypeId) -> ValueId {
        let m = m.core_ref();
        m.context().push_value(ValueData {
            ty: ptr_ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::PointerNull),
            use_list: core::cell::RefCell::new(Vec::new()),
        })
    }
    fn skeleton<'ctx, R: crate::marker::ReturnMarker>(
        m: &Module<'ctx>,
        ret_ty: crate::Type<'ctx>,
        params: &[crate::Type<'ctx>],
        name: &str,
    ) -> (ValueId, ValueId) {
        let fn_ty = m.fn_type(ret_ty, params.iter().copied(), false);
        let f = m
            .add_function::<R, _>(name, fn_ty, Linkage::External)
            .unwrap();
        let bb = f.append_basic_block(m, "entry");
        // Reach the value-id pair without leaking R into the return.
        let f_id = {
            // FunctionValue<R> has a private id field; widen via as_dyn.
            f.as_dyn().as_value().id
        };
        let bb_id = bb.as_dyn().as_value().id;
        (f_id, bb_id)
    }

    /// Append a `ret void` to a block via direct fabrication.
    fn append_ret_void(m: &Module<'_>, bb_id: ValueId) {
        fabricate_instruction(
            m,
            bb_id,
            m.void_type().as_type().id(),
            InstructionKindData::Ret(ReturnOpData::new(None)),
        );
    }

    fn assert_rule(err: &IrError, expected: VerifierRule) {
        match err {
            IrError::VerifierFailure { rule, .. } if *rule == expected => {}
            _ => panic!("expected VerifierRule::{expected:?}, got {err:?}"),
        }
    }

    /// Mirrors `Verifier::visitFunction`: generated intrinsic declarations
    /// must carry the generated declaration attributes, not a subset with
    /// silently missing `immarg` / memory attributes.
    #[test]
    fn intrinsic_declaration_missing_generated_attrs_is_rejected() {
        let err = Module::with_new("intrinsic-missing-attrs", |m| {
            let f = m
                .get_or_insert_intrinsic_declaration_by_name("llvm.abs.i32")
                .expect("intrinsic declaration");
            *f.data().attributes.borrow_mut() = crate::attributes::AttributeStorage::new();
            m.verify_borrowed()
                .expect_err("missing generated attrs rejected")
        });

        match err {
            IrError::InvalidOperation { message } => {
                assert_eq!(message, "intrinsic declaration modifier")
            }
            other => panic!("unexpected verifier error: {other:?}"),
        }
    }

    /// Mirrors `Verifier::visitFunction`: intrinsic declaration attribute
    /// groups must resolve before generated attributes can be checked.
    #[test]
    fn intrinsic_declaration_extra_attr_group_is_rejected() {
        let err = Module::with_new("intrinsic-extra-group", |m| {
            let f = m
                .get_or_insert_intrinsic_declaration_by_name("llvm.bswap.i32")
                .expect("intrinsic declaration");
            f.data().function_attr_groups.borrow_mut().push(0);
            m.verify_borrowed().expect_err("extra attr group rejected")
        });

        match err {
            IrError::InvalidOperation { message } => {
                assert_eq!(message, "intrinsic declaration modifier")
            }
            other => panic!("unexpected verifier error: {other:?}"),
        }
    }

    /// `test/Verifier/2002-04-13-RetTypes.ll` -- ret operand type
    /// (ptr) does not match function return type (i32).
    #[test]
    fn ret_type_mismatch_ptr_in_i32_function() {
        Module::with_new("t", |m| {
            let i32_ty = m.i32_type().as_type();
            let ptr_ty = m.ptr_type(0).as_type();
            let (_, bb_id) = skeleton::<i32>(&m, i32_ty, &[], "f");
            let null_id = fab_null_ptr_id(&m, ptr_ty.id());
            fabricate_instruction(
                &m,
                bb_id,
                m.void_type().as_type().id(),
                InstructionKindData::Ret(ReturnOpData::new(Some(null_id))),
            );
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::ReturnTypeMismatch);
        });
    }

    /// `test/Verifier/2008-11-15-RetVoid.ll` -- void function with a
    /// returned operand.
    #[test]
    fn ret_value_in_void_function() {
        Module::with_new("t", |m| {
            let void_ty = m.void_type().as_type();
            let i32_ty = m.i32_type().as_type();
            let (_, bb_id) = skeleton::<()>(&m, void_ty, &[], "f");
            let zero_id = fab_const_int_id(&m, i32_ty.id(), 0);
            fabricate_instruction(
                &m,
                bb_id,
                void_ty.id(),
                InstructionKindData::Ret(ReturnOpData::new(Some(zero_id))),
            );
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::ReturnTypeMismatch);
        });
    }

    /// Binary operands have differing types: `add i32 %a, i64 %b`.
    /// Mirrors `Verifier::visitBinaryOperator` operand-equality rule.
    #[test]
    fn binary_operand_type_mismatch() {
        Module::with_new("t", |m| {
            let i32_ty = m.i32_type().as_type();
            let i64_ty = m.i64_type().as_type();
            let void_ty = m.void_type().as_type();
            let (f_id, bb_id) = skeleton::<()>(&m, void_ty, &[i32_ty, i64_ty], "f");
            let f = FunctionValue::<'_, Dyn>::from_parts_unchecked(f_id, m.as_view());
            let p0 = f.param(0).unwrap();
            let p1 = f.param(1).unwrap();
            fabricate_instruction(
                &m,
                bb_id,
                i32_ty.id(),
                InstructionKindData::Add(BinaryOpData::new(p0.as_value().id, p1.as_value().id)),
            );
            append_ret_void(&m, bb_id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::BinaryOperandsTypeMismatch);
        });
    }

    /// Conditional branch with non-i1 condition.
    /// Mirrors `Verifier::visitBranchInst`.
    #[test]
    fn br_condition_not_i1() {
        Module::with_new("t", |m| {
            let void_ty = m.void_type().as_type();
            let i32_ty = m.i32_type().as_type();
            let (f_id, entry_id) = skeleton::<()>(&m, void_ty, &[i32_ty], "f");
            let f = FunctionValue::<'_, Dyn>::from_parts_unchecked(f_id, m.as_view());
            let then_bb = f.append_basic_block(&m, "then");
            let else_bb = f.append_basic_block(&m, "else");
            append_ret_void(&m, then_bb.as_value().id);
            append_ret_void(&m, else_bb.as_value().id);
            let p0 = f.param(0).unwrap();
            fabricate_instruction(
                &m,
                entry_id,
                void_ty.id(),
                InstructionKindData::Br(BranchInstData {
                    kind: core::cell::RefCell::new(BranchKind::Conditional {
                        cond: core::cell::Cell::new(p0.as_value().id),
                        then_bb: then_bb.as_value().id,
                        else_bb: else_bb.as_value().id,
                    }),
                }),
            );
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::BranchConditionNotI1);
        });
    }

    /// Two terminators in a row -- second one is misplaced.
    /// Mirrors `Verifier::visitInstruction` terminator-position rule.
    #[test]
    fn misplaced_terminator() {
        Module::with_new("t", |m| {
            let void_ty = m.void_type().as_type();
            let (_, bb_id) = skeleton::<()>(&m, void_ty, &[], "f");
            for _ in 0..2 {
                append_ret_void(&m, bb_id);
            }
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::MisplacedTerminator);
        });
    }

    /// `test/Verifier/PhiGrouping.ll` -- phi appears after a non-phi.
    #[test]
    fn phi_not_at_top() {
        Module::with_new("t", |m| {
            let i32_ty = m.i32_type().as_type();
            let void_ty = m.void_type().as_type();
            let (f_id, entry_id) = skeleton::<()>(&m, void_ty, &[i32_ty, i32_ty], "f");
            let f = FunctionValue::<'_, Dyn>::from_parts_unchecked(f_id, m.as_view());
            let p0 = f.param(0).unwrap();
            let p1 = f.param(1).unwrap();
            fabricate_instruction(
                &m,
                entry_id,
                i32_ty.id(),
                InstructionKindData::Add(BinaryOpData::new(p0.as_value().id, p1.as_value().id)),
            );
            fabricate_instruction(
                &m,
                entry_id,
                i32_ty.id(),
                InstructionKindData::Phi(PhiData::new()),
            );
            append_ret_void(&m, entry_id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::PhiNotAtTop);
        });
    }

    /// `test/Verifier/SelfReferential.ll` -- non-phi instruction whose
    /// operand is itself.
    #[test]
    fn self_reference_in_non_phi() {
        Module::with_new("t", |m| {
            let i32_ty = m.i32_type().as_type();
            let void_ty = m.void_type().as_type();
            let (_, bb_id) = skeleton::<()>(&m, void_ty, &[], "f");
            // Predict the next value-id by pushing a probe and reading
            // its arena index.
            let probe = fab_const_int_id(&m, i32_ty.id(), 0);
            let next_index = probe.arena_index() + 1;
            let next_id = ValueId::from_index(next_index);
            // Push an `add i32 next_id, probe` -- next_id IS this add's id.
            let pushed = fabricate_instruction(
                &m,
                bb_id,
                i32_ty.id(),
                InstructionKindData::Add(BinaryOpData::new(next_id, probe)),
            );
            assert_eq!(pushed, next_id, "id prediction must match arena order");
            append_ret_void(&m, bb_id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::SelfReference);
        });
    }

    /// `Verifier::visitPHINode` -- "PHI nodes cannot have token type", plus the
    /// general rule that a phi result must be a first-class data type. The
    /// verifier now rejects an invalid phi *result* type before any coherence
    /// check, mirroring the `.ll` parser's parse-time rejection so the
    /// guarantee holds regardless of construction path (the raw phi builders
    /// are internal, but `build_phi_dyn`/`make_phi_in_block` still take an
    /// erased type).
    #[test]
    fn phi_with_invalid_result_type_rejected() {
        // `token`: LLVM's explicit "PHI nodes cannot have token type".
        Module::with_new("t", |m| {
            let void_ty = m.void_type().as_type();
            let token_ty = m.token_type().as_type();
            let (_f_id, entry_id) = skeleton::<()>(&m, void_ty, &[], "f");
            fabricate_instruction(
                &m,
                entry_id,
                token_ty.id(),
                InstructionKindData::Phi(PhiData::new()),
            );
            append_ret_void(&m, entry_id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::PhiInvalidResultType);
        });
        // `void`: not a first-class type, so also not a valid phi result.
        Module::with_new("t", |m| {
            let void_ty = m.void_type().as_type();
            let (_f_id, entry_id) = skeleton::<()>(&m, void_ty, &[], "f");
            fabricate_instruction(
                &m,
                entry_id,
                void_ty.id(),
                InstructionKindData::Phi(PhiData::new()),
            );
            append_ret_void(&m, entry_id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::PhiInvalidResultType);
        });
    }

    /// The result-type rule must NOT reject a *typed* pointer (`i32*`, the legacy
    /// `TypedPointerType`): it is a first-class data type and a valid phi result,
    /// distinct from the opaque `ptr` that `Type::is_pointer` matches. Regression
    /// guard — the first cut of this rule enumerated only `is_pointer()`, so it
    /// rejected `phi i32*`, IR that verified clean before.
    ///
    /// The phi is fabricated in an **unreachable** block so this case isolates
    /// the result-type gate (`PhiInvalidResultType`, which runs unconditionally
    /// ahead of the reachable check) without tripping the zero-incoming backstop
    /// (`PhiEmptyInReachableBlock`): a zero-incoming phi is only rejected in a
    /// block reachable from entry.
    #[test]
    fn phi_with_typed_pointer_result_type_verifies() {
        Module::with_new("t", |m| {
            let i32_ty = m.i32_type().as_type();
            let void_ty = m.void_type().as_type();
            let tptr_ty = m.typed_pointer_type(i32_ty, 0).as_type();
            let (f_id, entry_id) = skeleton::<()>(&m, void_ty, &[], "f");
            let f = FunctionValue::<'_, Dyn>::from_parts_unchecked(f_id, m.as_view());
            let dead = f.append_basic_block(&m, "dead");
            let dead_id = dead.as_value().id;
            fabricate_instruction(
                &m,
                dead_id,
                tptr_ty.id(),
                InstructionKindData::Phi(PhiData::new()),
            );
            append_ret_void(&m, dead_id);
            append_ret_void(&m, entry_id);
            m.verify_borrowed()
                .expect("a typed-pointer phi result must remain valid");
        });
    }

    /// `test/Verifier/AmbiguousPhi.ll` -- duplicate predecessor with
    /// differing values.
    #[test]
    fn ambiguous_phi_duplicate_predecessor() {
        Module::with_new("t", |m| {
            let i1_ty = m.bool_type().as_type();
            let i32_ty = m.i32_type().as_type();
            let void_ty = m.void_type().as_type();
            let (f_id, entry_id) = skeleton::<()>(&m, void_ty, &[i1_ty], "f");
            let f = FunctionValue::<'_, Dyn>::from_parts_unchecked(f_id, m.as_view());
            let target = f.append_basic_block(&m, "target");
            let cond_id = f.param(0).unwrap().as_value().id;
            fabricate_instruction(
                &m,
                entry_id,
                void_ty.id(),
                InstructionKindData::Br(BranchInstData {
                    kind: core::cell::RefCell::new(BranchKind::Conditional {
                        cond: core::cell::Cell::new(cond_id),
                        then_bb: target.as_value().id,
                        else_bb: target.as_value().id,
                    }),
                }),
            );
            let one = fab_const_int_id(&m, i32_ty.id(), 1);
            let two = fab_const_int_id(&m, i32_ty.id(), 2);
            let phi = PhiData::new();
            phi.incoming
                .borrow_mut()
                .push((core::cell::Cell::new(one), entry_id));
            phi.incoming
                .borrow_mut()
                .push((core::cell::Cell::new(two), entry_id));
            fabricate_instruction(
                &m,
                target.as_value().id,
                i32_ty.id(),
                InstructionKindData::Phi(phi),
            );
            append_ret_void(&m, target.as_value().id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::AmbiguousPhi);
        });
    }

    /// Phi references a block that is not a CFG predecessor.
    #[test]
    fn phi_predecessor_mismatch() {
        Module::with_new("t", |m| {
            let i32_ty = m.i32_type().as_type();
            let void_ty = m.void_type().as_type();
            let (f_id, entry_id) = skeleton::<()>(&m, void_ty, &[], "f");
            let f = FunctionValue::<'_, Dyn>::from_parts_unchecked(f_id, m.as_view());
            let target = f.append_basic_block(&m, "target");
            let unrelated = f.append_basic_block(&m, "unrelated");
            fabricate_instruction(
                &m,
                entry_id,
                void_ty.id(),
                InstructionKindData::Br(BranchInstData {
                    kind: core::cell::RefCell::new(BranchKind::Unconditional(target.as_value().id)),
                }),
            );
            append_ret_void(&m, unrelated.as_value().id);
            let bogus = fab_const_int_id(&m, i32_ty.id(), 7);
            let phi = PhiData::new();
            phi.incoming
                .borrow_mut()
                .push((core::cell::Cell::new(bogus), unrelated.as_value().id));
            fabricate_instruction(
                &m,
                target.as_value().id,
                i32_ty.id(),
                InstructionKindData::Phi(phi),
            );
            append_ret_void(&m, target.as_value().id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::PhiPredecessorMismatch);
        });
    }

    /// Call argument count mismatch -- non-vararg callee with wrong
    /// argc. Mirrors `Verifier::visitCallBase`.
    #[test]
    fn call_arg_count_mismatch() {
        Module::with_new("t", |m| {
            let i32_ty = m.i32_type().as_type();
            let void_ty = m.void_type().as_type();
            // Callee: `define i32 @callee(i32, i32)` -- empty body, terminator
            // fabricated to make it valid.
            let callee_fn_ty = m.fn_type(i32_ty, [i32_ty, i32_ty], false);
            let callee = m
                .add_function::<i32, _>("callee", callee_fn_ty, Linkage::External)
                .unwrap();
            let cb = callee.append_basic_block(&m, "entry");
            let zero = fab_const_int_id(&m, i32_ty.id(), 0);
            fabricate_instruction(
                &m,
                cb.as_value().id,
                void_ty.id(),
                InstructionKindData::Ret(ReturnOpData::new(Some(zero))),
            );
            // Caller: passes only ONE arg.
            let caller_fn_ty = m.fn_type(void_ty, [i32_ty], false);
            let caller = m
                .add_function::<(), _>("caller", caller_fn_ty, Linkage::External)
                .unwrap();
            let entry = caller.append_basic_block(&m, "entry");
            let arg_id = caller.param(0).unwrap().as_value().id;
            fabricate_instruction(
                &m,
                entry.as_value().id,
                i32_ty.id(),
                InstructionKindData::Call(crate::instr_types::CallInstData::new(
                    callee.as_value().id,
                    callee_fn_ty.as_type().id(),
                    [arg_id],
                    crate::CallingConv::default(),
                    crate::instr_types::TailCallKind::None,
                )),
            );
            append_ret_void(&m, entry.as_value().id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::CallArgCountMismatch);
        });
    }
    /// Mirrors `Verifier::visitPtrToAddrInst`: result integer width must match
    /// the `DataLayout` index width for the source pointer address space.
    #[test]
    fn ptrtoaddr_result_uses_index_width() {
        Module::with_new("t", |m| {
            m.set_data_layout("p1:64:64:64:32").unwrap();
            let void_ty = m.void_type().as_type();
            let i64_ty = m.i64_type().as_type();
            let ptr1_ty = m.ptr_type(1).as_type();
            let (_f_id, bb_id) = skeleton::<()>(&m, void_ty, &[], "f");
            let ptr = fab_null_ptr_id(&m, ptr1_ty.id());
            fabricate_instruction(
                &m,
                bb_id,
                i64_ty.id(),
                InstructionKindData::Cast(CastOpData::new(CastOpcode::PtrToAddr, ptr)),
            );
            append_ret_void(&m, bb_id);
            let err = m.verify_borrowed().unwrap_err();
            assert_rule(&err, VerifierRule::CastTypeMismatch);
            match err {
                IrError::VerifierFailure { message, .. } => {
                    assert!(message.contains("ptrtoaddr result must be address width"));
                }
                _ => panic!("expected verifier failure"),
            }
        });
    }
}
