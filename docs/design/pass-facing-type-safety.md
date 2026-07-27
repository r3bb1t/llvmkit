# Pass-Facing Type Safety — Design

Date: 2026-07-10. Target: `llvmkit` workspace (`crates/llvmkit-ir`, `crates/llvmkit-macros`), branch flow `feature-N/*` → `dev`.

## Status — all four packages SHIPPED and merged to `dev`

A concrete, code-first tour of the shipped surface is in [API at a glance](#api-at-a-glance-shipped) just below; the prose that follows it is the original per-package design rationale.

> **Reading the two halves.** This status list and the "API at a glance" section are kept current — they describe llvmkit as it is at 0.1.0. Everything from "## Context" onward is the 2026-07-10 design record, preserved as written; its `file:line` citations are from that tree and have drifted, so treat the symbol names as the stable reference and the line numbers as archaeology. Where a package shipped in a different shape than it was designed, the difference is called out in this list.

- **Package 1 — instruction taxonomy: shipped** (branch `feature-5/instruction-taxonomy`). Delivered total `classify()` over `Classified { Inst, Term }`; exhaustive `InstructionKind`/`TerminatorKind`; the `CastKind` (14 per-opcode handles) and `PhiKind { Int, Fp, Ptr, Other }` sub-enums; `PointerValue`-typed pointer operands and `CallInst::classify_callee()`; readers for switch/indirectbr/landingpad/catchswitch entries; `as_binary_op()`/`as_cmp()` groupings plus `OverflowingBinaryOperator`(+`shl`)/`PossiblyExactOperator`; and the `AtomicRMWInst::set_value_operand` token fix. All breaking (pre-1.0).
- **Package 2 — pattern DSL: shipped** (branch `feature-6/pattern-matchers`). A `PatternMatch.h`-style combinator module (`matchers.rs`) whose matchers **return** their bindings as a flat tuple (composed type-level via `Combine`), so a failed match is `None` — never a half-filled slot. Delivers `m_value`/`m_specific`, constant predicates (`m_zero`/`m_one`/`m_all_ones`/`m_power2`/`m_negative`/`m_non_negative`, `m_ap_int`, `m_specific_int`), `m_one_use`, per-opcode binops (`m_add`..`m_frem`) with commutative `m_c_*` variants, sugar (`m_not`/`m_neg`), `m_combine_or`/`m_combine_and`, and `m_load`/`m_gep`. Backed by new `Value::has_one_use()`/`as_const_int()`. Scalar constants only (vector-splat and poison-lane awareness deferred); in-pattern `m_deferred`-style unification is out of scope (use two `match_view` calls with `m_specific`, mirroring InstCombine's cross-`match()` reuse). Cast/`m_icmp`/`m_fcmp`/`m_intrinsic` matchers deferred. Tests transliterate real InstCombine folds.
- **Package 3 — pass ergonomics: part 1 shipped** (branch `feature-7/pass-ergonomics`). The witnessed dirty-bit: `FnPatch`/`FnReshape` track whether any mutation actually happened (a `Cell<bool>` set by `erase`/`replace_all_uses`/`split_block`), and `done()` reports everything-preserved when clean, the rung floor when dirty — so a no-op mutating pass no longer needlessly invalidates cached analyses. This deleted the duplicated read-only pre-scans in `dce.rs`/`inst_simplify.rs` (both rewritten to route mutation through the mutator so the flag is witnessed; output byte-identical). Plus the small fix `BasicBlockView::instructions()`. **Part 2 shipped the `NonTerminator` compile-safe erase:** `FnPatch::erase` now accepts only a `NonTerminator` (obtained from `InstructionView::as_non_terminator`, which returns `None` for a terminator), so a terminator-erase — which would break the `PatchBody` "CFG preserved" floor — is a *compile* error (pinned by the `patchbody_cannot_erase_terminator` trybuild fixture) rather than the old runtime rejection. `erase` is now infallible. **Part 3 shipped** (`feature-10/worklist-cursor`, own spec [`docs/worklist-erase-safe-cursor-design.md`](worklist-erase-safe-cursor-design.md)): the erase-safe `body_instructions()` cursor + a **mutation-driven worklist** killed the O(n²) restart-scan in `dce.rs`/`inst_simplify.rs` while keeping the output byte-identical. Cascade direction is intrinsic to the *mutation* — `FnPatch::erase` pushes the erased instruction's operand-defs and self-removes, `replace_all_uses` pushes former users — so there is no per-pass knob and the inactive path stays zero-cost. `FnReshape: Deref<FnPatch>` shipped in Part 3 but was then deliberately **removed** in Package 4 (the blanket `Deref` re-exposed a mid-reshape stale-read footgun). The in-pass builder later shipped in cycle D in a narrower shape — `FnPatch::builder_at` / `FnReshape::builder_at` restore a builder at a previously-saved `InsertPoint`, and handing one out witnesses the dirty flag — but *fresh* positioning inside `run()` is still not offered. Still deferred (YAGNI / marginal): the `DataLayout` cache on the mutator, and `ModRewrite`'s dirty-bit (its mutations go through the raw `module_mut()` token; reporting the floor is conservative/safe).
- **Package 4 — analysis system (Option C): shipped** (`feature-8/analysis-plumbing` + `feature-9/analysis-preservation-phase2`). Framework-witnessed preservation, no author-claims API: `CfgUpdate` recording on `FnReshape`, the `CfgIncremental` hook (`apply_updates`/`recompute`, `RepairOutcome`), the **unrepresentable** mid-reshape stale CFG-analysis read (`FnReshape::analysis_repaired`, and the `Deref` removal that makes it so), the `done()`-flush that keeps a reshape pass's dominator tree by *witnessing* its repair (marked preserved only because the framework watched `apply_updates` succeed), and `Requires`-without-`Default` via the `PrefetchableAnalysis` trait. Deferred to a follow-up: sub-linear incremental dominator repair (perf only — `apply_updates` correctly repairs by recompute today); `PrefetchableModuleAnalysis`; the `reshape_functions` reshape flush.
- **Phi guarantees — wave 1: shipped** (branch `feature-11/phi-guarantees`, a follow-on beyond the four packages above). Pushes the *local* phi invariants into construction and parsing: `FnReshape::split_block` now maintains successor-block phi incomings (bug fix); `add_incoming` (typed and untyped paths) type-checks the incoming and rejects a differing duplicate for one predecessor (`IrError::AmbiguousPhiIncoming`); the `build_*_phi` builders insert at the PHI head (placement correct-by-construction); the `.ll` parser rejects misplaced phis and checks phi completeness at parse time once all predecessors are known; plus the new `m_phi()` matcher and an InstSimplify uniform-phi fold. `Module::verify()` remains the final gate for whole-graph phi coherence and dominance.
- **Phi guarantees — wave 2: shipped** (branch `feature-12/phi-block-args`, own spec [`docs/phi-type-guarantees-design.md`](phi-type-guarantees-design.md)). Block arguments became the **only** public phi-authoring surface: `IRBuilder::append_block_with_params` / `append_block_with_named_params` mint a block whose parameters are operandless head-phis, and `build_br_with_args` / `build_cond_br_with_args` carry one argument per parameter along the edge, arity- and type-checked at the call site. The three typed raw builders (`build_int_phi`, `build_fp_phi`, `build_pointer_phi`) and every phi-handle `add_incoming` are now `pub(crate)` — unnameable from outside the crate, pinned by `raw_phi_builder_is_unnameable`. Their erased twins (`build_int_phi_dyn`, `build_fp_phi_dyn`, `build_pointer_phi_in_addrspace`, `build_phi_dyn`) plus `phi_add_incoming_from_value` stayed technically reachable but are `#[doc(hidden)]` and documented as the internal contract for the in-tree `.ll` parser — not supported API. `FnReshape::insert_phi` (typed) / `insert_phi_dyn` (erased) create a phi pass-side with completeness, type, duplicate **and incoming-value dominance** all witnessed at the call. The designed `remove_edge`/`redirect_edge` pair shipped instead as the **typed terminator edit surface** — `FnReshape::edit_terminator` plus the `edit_br` / `edit_cond_br` / `edit_switch` / `edit_invoke` / `edit_callbr` narrows, each returning an edit handle whose *method set* encodes the legal edits, so a structurally-invalid edge edit is a compile error rather than a runtime rejection. Typed block parameters (`BlockParams`, `append_block_typed`, `BlockCall` + `build_br_call` / `build_cond_br_call`) landed later on `feature-20/typed-block-params`.

## API at a glance (shipped)

Concrete usage of each package's shipped surface — the doctrine in code.

**Package 1 — total, honest classification.** No overloaded `None`; the enum is exhaustive (a new opcode breaks the match, on purpose), and operands are typed where the IR grammar guarantees it.

```rust
use llvmkit_ir::{Classified, InstructionKind, TerminatorKind, CastKind, PhiKind, Callee};

match view.classify() {                       // total: Inst | Term, no is_terminator() dance
    Classified::Inst(InstructionKind::Cast(CastKind::PtrToInt(c))) => {
        let src: PointerValue = c.src();      // ptrtoint's source is a pointer, by construction
    }
    Classified::Inst(InstructionKind::Phi(PhiKind::Fp(phi))) => { /* fp-typed phi handle */ }
    Classified::Term(TerminatorKind::Switch(sw)) => {
        // real reader over stored cases; `dest` is a storable `BlockId<Dyn, B>`
        for (case_val, dest) in sw.cases() { /* … */ }
    }
    _ => {}                                   // exhaustive — this arm must be spelled
}

let ptr: PointerValue = load.pointer();       // Load/Store/Gep/VAArg/Atomic* pointer() is typed
match call.classify_callee() {                // Callee { Direct(FunctionValue), Indirect(PointerValue) }
    Callee::Direct(f)   => { /* direct call */ }
    Callee::Indirect(p) => { /* through a function pointer */ }
}
```

**Package 2 — pattern matchers that return their bindings.** A partial match is `None`, never a half-filled out-param.

```rust
use llvmkit_ir::matchers::*;

// (x - y) & -1  →  binds (x, y) only on a full match:
if let Some((x, y)) = m_add(m_one_use(m_sub(m_value(), m_value())), m_all_ones())
        .match_view(&view) {
    // use x, y
}
// commutative + constant capture: tries (a, k) then (k, a), binds the ApInt:
let _ = m_c_add(m_specific(a), m_ap_int()).match_view(&view);
```

**Package 3 — a worklist pass with witnessed preservation.** Seed once, drain to fixpoint; mutations cascade automatically; terminator-erase is a *compile* error.

```rust
let patch = cx.mutate();
let scope = patch.worklist();                 // seeds every non-terminator; RAII, rejects nesting
while let Some(inst) = scope.next() {          // NonTerminator, erase-safe
    if is_trivially_dead(&inst.as_view()) {
        patch.erase(&inst);                    // auto-pushes operand-defs + self-removes (cascade)
    }
}
drop(scope);
Ok(patch.done())      // clean run → all-preserved; any mutation → the rung floor (both *witnessed*)
// patch.erase(terminator) does not compile: erase takes only NonTerminator.
```

**Package 4 — CFG reshape with framework-witnessed analysis preservation.** The mutator records edits; the driver keeps an analysis only by watching it repair.

```rust
let mut reshape = cx.mutate();
// `block.id()` is a storable `BlockId<Dyn, B>`; the *return* is deliberately the linear
// `BasicBlock<Unterminated>` — the fresh block owes a terminator, and a `Copy` id would
// silently discard that obligation.
let new_bb = reshape.split_block(block.id(), &before, "split")?; // records the CfgUpdate decomposition

// Mid-pass, read the dominator tree *repaired* to the current CFG. Holding a plain cached
// CFG-analysis ref across the edit is a COMPILE error — this is the only sound way to read it:
let dt = reshape.analysis_repaired::<DominatorTreeAnalysis, _>();
let _ = dt.is_reachable_from_entry(&new_bb);

Ok(reshape.done())
// At done(), the driver offers the recorded edits to each cached CFG analysis; the dominator
// tree repairs and is marked preserved — because the framework *witnessed* it, never a claim.
// (A `Requires` list no longer needs `Default`: register a configured instance instead.)
```

## Context

llvmkit's `InstructionKind` enum already beats C++ `isa`/`dyn_cast` at the opcode level, but the pass-facing story degrades one operand deep: accessors return erased `Value` where types are provable, casts/phis collapse into single variants (the phi one produces a semantically wrong `as_int_value()` on fp/ptr phis), variable-arity terminators expose counts but no readers, and there are no grouping abstractions. Surveying the original C++ passes (`orig_cpp/llvm-project-llvmorg-22.1.4`) showed the real pass-facing interface there is `PatternMatch.h` (2,105 `match()` sites in InstCombine; 898 `m_OneUse`) plus a pass skeleton (erase-safe iteration, worklists, mid-pass IRBuilder, `PreservedAnalyses`). llvmkit's own two passes (`dce.rs`, `inst_simplify.rs`) hand-roll clone-the-id-list + restart-scan-on-first-change (O(n²)), duplicate their match logic in read-only pre-scans forced by consuming `mutate()`, and cannot create IR mid-pass.

Goal: make the instruction surface fully honest ("more strongly typed enum") and give pass authors the C++-proven ergonomics — under a strict safety doctrine, with breaking changes explicitly accepted (pre-1.0).

**Doctrine (goes verbatim into the spec/docs):** every guarantee is tiered *unrepresentable* (compiler rejects) > *witnessed* (framework observes the runtime fact; no author assertions) > *tested* (in-crate algorithms, property-tested once) — and **nothing runs on trust**. Author-claimed analysis preservation (C++/MLIR model) is explicitly rejected. Human judgment remains only in: (1) transformation semantics (future complement: the Alive2-style `refines` item already in `docs/future-work.md`), (2) in-crate algorithms + their test oracles, (3) downstream custom analyses' own hooks (blast radius contained to their consumers).

Decisions locked with user: all four packages; nested sub-enums (not flat, not runtime-tag); DSL = combinators returning bindings (no mutable slots, no macro sugar for now); analysis preservation = Option C "framework-witnessed repair" (no `done_preserving` claims API); drop `#[non_exhaustive]`; breaking changes fine.

## Package 1 — Instruction taxonomy (`crates/llvmkit-ir/src/instruction.rs`, `instructions.rs`, `instr_types.rs`, `operator.rs`)

1. **Total classification.** Add `Classified<'ctx,B> { Inst(InstructionKind), Term(TerminatorKind) }` and total `InstructionView::classify()` / `Instruction::classify()`. Keep `kind()`/`terminator_kind()` as filters delegating to `classify()`. Kills the overloaded-`None` convention (`dce.rs`'s `is_trivially_dead` had to remember `is_terminator()` first).
2. **Drop `#[non_exhaustive]`** on `InstructionKind`, `TerminatorKind`, and the new sub-enums. New opcodes must break downstream matches (exhaustiveness is the safety feature). Note in CHANGELOG.
3. **`CastKind` sub-enum** replacing `InstructionKind::Cast(CastInst)`: exactly one variant per existing `CastOpcode` variant (the `CastOpcode` enum in `instr_types.rs` is the source of truth), each carrying a new per-cast handle (`TruncInst`, `ZExtInst`, `SExtInst`, `FpTruncInst`, `FpExtInst`, `FpToUIInst`, `FpToSIInst`, `UIToFpInst`, `SIToFpInst`, `PtrToIntInst`, `IntToPtrInst`, `BitCastInst`, `AddrSpaceCastInst`, …). Handles are macro-generated (extend the `decl_handle_scaffold!`/binop-macro approach in `instructions.rs`; the cast family got its own `decl_cast_handle!`). Typed truths per opcode where the grammar guarantees them (e.g. `PtrToIntInst::src() -> PointerValue`, `IntToPtrInst::result`-side typing via `loaded`-style accessor). Group-level API stays on `CastKind`: `src() -> Value`, `opcode() -> CastOpcode`.
4. **`PhiKind` sub-enum** replacing the single `Phi` variant carrying a closed int-phi handle: `Int(PhiInst<'ctx, IntDyn, B>)`, `Fp(FpPhiInst<'ctx, FloatDyn, B>)`, `Ptr(PointerPhiInst<'ctx, B>)`, `Other(OtherPhiInst<'ctx, B>)` for vector/aggregate phis — the `Other` payload is a phi handle with a fully type-erased marker (reuse the existing `Dyn` erasure convention from `marker.rs`; it exposes only erased accessors, no typed narrowing). Discriminate on the phi's result type at `classify()` time. Removes the lying `as_int_value()` path in `instructions.rs`; the `Other` handle exposes only type-erased accessors.
5. **Typed operands — rule: type exactly what the IR grammar guarantees.** `pointer() -> PointerValue` on `LoadInst`, `StoreInst`, `GepInst`, `AtomicCmpXchgInst`, `AtomicRMWInst` and `VAArgInst` (all in `instructions.rs`). `CallInst::classify_callee() -> Callee::{Direct(FunctionValue), Indirect(PointerValue)}` (new enum; `callee() -> Value` may remain as escape hatch). Binop/cmp/select operands stay `Value` (int-or-vector legitimately). Reuse existing `PointerValue` (`typed_pointer_value.rs`); do NOT invent new typed wrappers beyond what exists.
6. **Missing readers.** `SwitchInst::cases()` (case constants surfaced as stored), `IndirectBrInst::destinations()`, `LandingPadInst::clauses()` (catch/filter distinction, via `LandingPadClauseKind`), `CatchSwitchInst::handlers()`. Data already lives in per-variant payloads (`operand_ids` in `instruction.rs` shows the storage); this is read-surface only, valid on `term_open_state::Closed` handles (spelled `TermClosed` in-crate). **As shipped:** the block-valued readers yield the storable `BlockId<Dyn, B>`, not a borrowing label — `cases()` is `impl ExactSizeIterator<Item = (Value<'ctx, B>, BlockId<Dyn, B>)>`. (`BasicBlockLabel` still exists; since llvmkit 2.0 it is the ephemeral view a `BlockId` resolves to through `Module::view`, not the currency an API hands back.)
7. **Groupings.** `InstructionKind::as_binary_op(&self) -> Option<BinaryOp<'ctx,B>>` — one grouped view with `lhs()`, `rhs()`, `opcode() -> BinOpcode`, `is_commutative()`; generate impls inside the existing binop macro. `as_cmp(&self) -> Option<Cmp<'ctx,B>>` unifying ICmp/FCmp with a shared predicate view (reuse `cmp_predicate.rs`). Extend `OverflowingBinaryOperator` (`operator.rs`) to `ShlInst` (C++ parity: add/sub/mul/**shl**); add `PossiblyExactOperator` trait for `UDiv`/`SDiv`/`LShr`/`AShr` (`is_exact` already exists per-handle via the macro; the trait unifies it).
8. **Fix the token gap:** `AtomicRMWInst::set_value_operand` (`instructions.rs`) gains a `&Module<B, Unverified>` parameter, restoring the "no mutation without a token" rule. Pinned by `tests/compile_fail/atomicrmw_set_value_requires_token.rs`.

## Package 2 — Pattern DSL (`crates/llvmkit-ir/src/matchers.rs`, new)

Combinators whose bindings are the return value:

```rust
pub trait Matcher<'ctx, B> { type Bindings; fn try_match(&self, v: Value<'ctx,B>) -> Option<Self::Bindings>; }
// m_add(m_one_use(m_sub(m_value(), m_value())), m_all_ones()).match_view(&view) -> Option<(Value, Value)>
```

- Bindings compose type-level (nested tuples flattened via a `Combine` helper trait, or plainly nested — implementer's choice, but the public signature must read as a flat tuple for ≤4 binders).
- Entry points: `match_view(&InstructionView)` and `try_match(Value)` (values auto-narrow to instructions where the pattern requires it).
- Core set (priority order from C++ frequency evidence): `m_value()`, `m_specific(v)`, per-opcode binops (macro-generated alongside Package 1's binop macro), commutative `m_c_add`/`m_c_mul`/`m_c_and`/`m_c_or`/`m_c_xor`/`m_c_icmp` (try (0,1) then (1,0); stable left-first order), constant predicates `m_zero`/`m_one`/`m_all_ones`/`m_power2`/`m_negative`/`m_non_negative` (scalar + splat-vector aware, poison-lane-tolerant by default, mirroring C++ defaults), `m_ap_int()` binding `&ApInt`, `m_specific_int(n)`, `m_one_use(inner)`, `m_load(ptr_pat)`/`m_store(val, ptr)`/`m_gep(...)`, cast matchers `m_trunc`/`m_zext`/`m_sext`/`m_bitcast`/`m_zext_or_self`..., `m_icmp(pred_bind, l, r)`/`m_fcmp(...)`, `m_combine_or`/`m_combine_and`, sugar `m_not`/`m_neg`, `m_intrinsic::<ID>(args...)`, `m_same_as(&x)` for already-bound equality (C++ `m_Specific` analog; full `m_Deferred`-style in-pattern unification documented as a known gap, revisit after usage).
- **Value-API prerequisites** (in `value.rs`/`constant.rs`): `has_one_use()`, `use_count()`, `users()` iterator, `as_const_int() -> Option<&ApInt>` + splat extraction for vector constants.
- Tests: transliterate the surveyed InstCombine folds (from `InstCombineAndOrXor.cpp` and `InstCombineAddSub.cpp`) as unit tests over builder-constructed IR.

## Package 3 — Pass ergonomics (`pass_context.rs`, `iter.rs`, `basic_block.rs`, `dce.rs`, `inst_simplify.rs`)

1. **Erase-safe cursor** on `FnPatch`: early-inc semantics (advance past current before yielding); yields a new linear `NonTerminator<'ctx,B>` handle; `FnPatch::erase` re-typed to accept only `NonTerminator` — terminator erasure becomes unrepresentable, deleting the old runtime rejection in `pass_context.rs`. Terminator reachable only via a separate non-erasable view. Cursor checks id-liveness on advance (skips instructions erased behind its back by cascades) — witnessed, documented. **As shipped:** `erase` takes the `NonTerminator` by shared reference (`patch.erase(&inst)`), not by value; the cursor (`body_instructions`) is a plain snapshot-and-walk iterator, and cascade-skipping is the *worklist*'s job.
2. **Worklist helper**: dedup set + stack (`SetVector` semantics), `push(inst)`, `push_users_of(&value)`, `push_operands_of(&inst)`, drain loop integrating with the cursor. Mirrors `DCE.cpp::eliminateDeadCode` / InstCombine's `InstructionWorklist` conventions. **As shipped** (see [`docs/worklist-erase-safe-cursor-design.md`](worklist-erase-safe-cursor-design.md)) the cascade pushes are *not* pass-facing methods: `Worklist<B>` exposes `push`/`pop`/`remove`/`contains`/`is_empty`, and `FnPatch::erase` / `replace_all_uses` push the correct cascade themselves, so no pass ever writes a `push`.
3. **Dirty-bit (witnessed)**: `FnPatch`/`FnReshape`/`ModRewrite` track whether any mutation actually occurred; clean `done()` returns the all-preserved report, dirty returns the rung floor. Deletes the duplicated read-only pre-scans in `dce.rs` / `inst_simplify.rs`. `cx.done()` before `mutate()` remains valid. **As shipped:** `ModRewrite` has no dirty bit — its mutations go through the raw `module_mut()` token, so it always reports the (conservative) floor.
4. **In-pass builder** (shipped in cycle D as a different shape): `patch.builder_at(ip)` / `reshape.builder_at(ip)` restore the existing typestate `IRBuilder` at a previously-saved `InsertPoint`, anchored in the function's module; existing builder guarantees carry over unchanged, and handing one out witnesses the dirty flag. This is a *restore-from-saved-point* capability, not the fresh `builder_before(&inst)` / `builder_at_end(&block)` positioning originally sketched here — freshly positioning a `PatchBody` builder inside `run()` needs the now-crate-private module token, so it is deliberately not offered. Materializing folds (InstSimplify's `None` DataLayout-builder limitation noted at `inst_simplify.rs`) remain future work on top of `builder_at`.
5. **Rewrite `DcePass` and `InstSimplifyPass`** onto cursor + worklist + dirty-bit as the acceptance proof: no pre-scan, no restart-scan, no `instruction_ids()` cloning, no `from_parts` re-materialization in pass code. Before/after output equivalence tested on the existing corpora.
6. **Small fixes**: `BasicBlockView::instructions()` for the Inspect rung (read-only passes had to escape via `as_function()`); `impl Deref<Target=FnPatch> for FnReshape` replacing the hand-copied delegations in `pass_context.rs`; `DataLayout` cached on the mutator (kills the per-iteration clone in `inst_simplify.rs`). **As shipped:** only the first landed. The `Deref` shipped and was then removed again in Package 4 (see the status list), and the `DataLayout` cache stayed deferred — `InstSimplifyPass` clones it once before the drain loop instead.

## Package 4 — Analysis system, Option C (`analysis.rs`, `pass_context.rs`, `pass_manager.rs`, `dominator_tree.rs`)

Framework-witnessed preservation; **no author claims API anywhere** (rejected Option A).

1. **`CfgUpdate` recording**: `enum CfgUpdate { InsertEdge(CfgEdge), DeleteEdge(CfgEdge) }` in `cfg_update.rs` (crate-private constructors `CfgUpdate::insert` / `::delete`). Every structural edit method on `FnReshape` — `split_block` at design time, since joined by the whole `edit_terminator` redirect/remove surface — records its own decomposition into the mutator's queue. Authors cannot construct, submit, reorder, or omit updates — the C++ `DomTreeUpdater` misuse class is unconstructible.
2. **`CfgIncremental` trait** for analysis results: `fn apply_updates(&mut self, updates: &[CfgUpdate], f: FunctionView) -> RepairOutcome`; `enum RepairOutcome { Repaired, PreferRecompute }`. Hookless or refusing analyses fall back to floor eviction (today's behavior = the degenerate case).
3. **Flush points**: (a) at `done()` — driver dedups cancelling pairs, feeds cached hooked results, marks preserved only what it watched succeed (added on top of the floor in the returned report); (b) mid-pass — `FnReshape::analysis_repaired::<A>(&mut self) -> &A::Result`, which flushes the queue into the cached result first. Borrow rules make holding a CFG-analysis ref across an edit a compile error; CFG analyses without the hook are not readable mid-reshape at all. This fixes the existing latent stale-read footgun (prefetched dom tree read after `split_block`).
4. **Phase 1 ships plumbing only**: `DominatorTree` implements the hook returning `PreferRecompute` (fresh recompute on mid-pass read; eviction at end). **Phase 2 (separate follow-up spec, not this one)**: real incremental repair, property-tested (random edit sequences: repaired ≡ recomputed) + debug-build recompute-compare at every flush. **As shipped:** `DominatorTree::apply_updates` returns `Repaired`, but repairs *by full recompute* — it is correct, not yet sub-linear. Genuine incremental repair (and the property tests) remain the deferred perf item.
5. **`Requires` without `Default`** (`analysis.rs`): add instance registration; `prefetch` uses the registered instance, falling back to the default only for `A: Default`. Parameterized analyses become declarable requirements. **As shipped:** the trait is `PrefetchableAnalysis` and the registration entry points are `register_pass` / `register_cfg_pass` on the analysis manager (plus `register_function_analysis` / `register_module_analysis` on `Analyses`), not a single `analyses.register::<A>(instance)`. The module side still bounds `+ Default`; `PrefetchableModuleAnalysis` waits on a first non-`Default` module analysis.
6. Interplay: dirty-bit covers "nothing happened" (all preserved); CfgUpdate covers "CFG changed, specific analyses repaired"; both witnessed. Scope honesty: update vocabulary is CFG-shaped — value analyses (KnownBits/DemandedBits) are out of scope (already evicted by every mutating floor today; instruction-level events are a documented possible extension, not designed here).

## Testing / verification

- **Compile-fail (trybuild)** pinning every Tier-1 claim: terminator-erase-through-cursor impossible; analysis-ref-across-CFG-edit borrow error; hookless-analysis-mid-reshape; AtomicRMW-mutation-without-token; DSL binding type/arity mismatches; reopen-through-sub-enum variants (the `finished_switch` / `finished_landingpad` reopen fixtures; the phi reopen/typestate fixtures were retired when the raw phi builders went internal, replaced by `raw_phi_builder_is_unnameable`, since block arguments are now the only public phi surface). **Known gotcha:** `.stderr` files must be blessed on the pinned CI rustc — `cargo +1.96.0`, there and nowhere else (see the "Compile-fail `.stderr` canonical-rustc bless" entry in [`docs/future-work.md`](../future-work.md)).
- **Runtime**: DSL unit tests transliterated from real InstCombine folds; DCE/InstSimplify rewrite equivalence vs current outputs; cursor semantics under erase-ahead/erase-behind; dirty-bit report correctness (clean run → all-preserved; single erase → floor); CfgUpdate recording unit tests for `split_block`; `cases()`/`clauses()` reader round-trips against builder-constructed IR.
- `cargo test --workspace --all-targets --all-features` green on Windows dev box + CI (master+dev), on the pinned toolchain.

## Sequencing and branching

**No work happens on `dev` directly.** First step before any change: `git checkout -b feature-5/instruction-taxonomy` off current `dev`. This spec doc is the first commit on that branch.

One branch per package, each cut from `dev` after the previous merges (per the established feature-N → dev workflow, push at task boundaries). **As executed and merged:**

1. `feature-5/instruction-taxonomy` — spec doc + Package 1 ✅
2. `feature-6/pattern-matchers` — Package 2 ✅
3. `feature-7/pass-ergonomics` — Package 3 dirty-bit + `NonTerminator` erase ✅
4. `feature-8/analysis-plumbing` — Package 4 Phase 1 (recording + hook + `analysis_repaired`) ✅
5. `feature-9/analysis-preservation-phase2` — Package 4 remainder (`done()`-flush, `PrefetchableAnalysis`) ✅
6. `feature-10/worklist-cursor` — Package 3's deferred perf: erase-safe cursor + worklist ✅ (own spec)
7. `feature-11/phi-guarantees` — phi wave 1 ✅ (own spec)
8. `feature-12/phi-block-args` — phi wave 2, the block-argument break ✅ (own spec)

User-visible changes accumulated in the top-level [`CHANGELOG.md`](../../CHANGELOG.md) under **Unreleased** (started with the phi-guarantees work; earlier packages were recorded per-commit); everything above now sits under the released **[0.1.0]** entry, which froze the public API. CI runs on `master`+`dev`; every merge to `dev` was green first.

> **Historical note.** This document once recorded "two pre-existing environmental `.stderr` mismatches" (`folder_typed_wrong_width`, `extract_value_empty_indices`) as an expected local failure. That caveat is **retired and was never real**: both fixtures pass on the pinned CI toolchain, and the mismatch only ever appeared when the suite was run on a newer rustc than the pinned one. Gate on `cargo +1.96.0` and the trybuild baseline is **0 failures of 83 registered fixtures** (82 `compile_fail` + 1 `pass`).

## Out of scope (recorded as considered/deferred with reasons)

InstVisitor port (exhaustive `match` + groupings supersede it); Salsa-style dependency tracking (requires revision-tracked IR storage — rearchitecture); DSL macro sugar (`ir_match!`) — later layer over combinators; debug-info salvage on erase (debug-loc story too thin); Phase-2 incremental dom-tree repair (own spec); instruction-level update events for value analyses; everything already listed in `docs/future-work.md` (executable pipelines, instrumentation wiring, loop/CGSCC rungs, ModRewrite per-function analyses).
