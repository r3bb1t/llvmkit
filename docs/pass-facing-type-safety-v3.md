# Pass-Facing Type Safety v3 — Design Spec

Date: 2026-07-10. Target: `llvmkit` workspace (`crates/llvmkit-ir`, `crates/llvmkit-macros`), branch flow `feature-N/*` → `dev`.

## Context

llvmkit's `InstructionKind` enum already beats C++ `isa`/`dyn_cast` at the opcode level, but the pass-facing story degrades one operand deep: accessors return erased `Value` where types are provable, casts/phis collapse into single variants (the phi one produces a semantically wrong `as_int_value()` on fp/ptr phis), variable-arity terminators expose counts but no readers, and there are no grouping abstractions. Surveying the original C++ passes (`orig_cpp/llvm-project-llvmorg-22.1.4`) showed the real pass-facing interface there is `PatternMatch.h` (2,105 `match()` sites in InstCombine; 898 `m_OneUse`) plus a pass skeleton (erase-safe iteration, worklists, mid-pass IRBuilder, `PreservedAnalyses`). llvmkit's own two passes (`dce.rs`, `inst_simplify.rs`) hand-roll clone-the-id-list + restart-scan-on-first-change (O(n²)), duplicate their match logic in read-only pre-scans forced by consuming `mutate()`, and cannot create IR mid-pass.

Goal: make the instruction surface fully honest ("more strongly typed enum") and give pass authors the C++-proven ergonomics — under a strict safety doctrine, with breaking changes explicitly accepted (pre-1.0, v0.0.4).

**Doctrine (goes verbatim into the spec/docs):** every guarantee is tiered *unrepresentable* (compiler rejects) > *witnessed* (framework observes the runtime fact; no author assertions) > *tested* (in-crate algorithms, property-tested once) — and **nothing runs on trust**. Author-claimed analysis preservation (C++/MLIR model) is explicitly rejected. Human judgment remains only in: (1) transformation semantics (future complement: the Alive2-style `refines` item already in `docs/future-work.md`), (2) in-crate algorithms + their test oracles, (3) downstream custom analyses' own hooks (blast radius contained to their consumers).

Decisions locked with user: all four packages; nested sub-enums (not flat, not runtime-tag); DSL = combinators returning bindings (no mutable slots, no macro sugar for now); analysis preservation = Option C "framework-witnessed repair" (no `done_preserving` claims API); drop `#[non_exhaustive]`; breaking changes fine.

## Package 1 — Instruction taxonomy (`crates/llvmkit-ir/src/instruction.rs`, `instructions.rs`, `instr_types.rs`, `operator.rs`)

1. **Total classification.** Add `Classified<'ctx,B> { Inst(InstructionKind), Term(TerminatorKind) }` and total `InstructionView::classify()` / `Instruction::classify()`. Keep `kind()`/`terminator_kind()` as filters delegating to `classify()`. Kills the overloaded-`None` convention (today `dce.rs:74` must remember `is_terminator()` first).
2. **Drop `#[non_exhaustive]`** on `InstructionKind`, `TerminatorKind`, and the new sub-enums. New opcodes must break downstream matches (exhaustiveness is the safety feature). Note in CHANGELOG.
3. **`CastKind` sub-enum** replacing `InstructionKind::Cast(CastInst)`: exactly one variant per existing `CastOpcode` variant (`instr_types.rs:229` is the source of truth), each carrying a new per-cast handle (`TruncInst`, `ZExtInst`, `SExtInst`, `FPTruncInst`, `FPExtInst`, `FPToUIInst`, `FPToSIInst`, `UIToFPInst`, `SIToFPInst`, `PtrToIntInst`, `IntToPtrInst`, `BitCastInst`, `AddrSpaceCastInst`, …). Handles are macro-generated (extend the `decl_handle_scaffold!`/binop-macro approach in `instructions.rs:57-240`). Typed truths per opcode where the grammar guarantees them (e.g. `PtrToIntInst::src() -> PointerValue`, `IntToPtrInst::result`-side typing via `loaded`-style accessor). Group-level API stays on `CastKind`: `src() -> Value`, `opcode() -> CastOpcode`.
4. **`PhiKind` sub-enum** replacing `Phi(PhiInst<IntDyn, PhiClosed>)`: `Int(PhiInst<IntDyn, PhiClosed>)`, `Fp(FpPhiInst<..>)`, `Ptr(PointerPhiInst<..>)`, `Other(..)` for vector/aggregate phis — the `Other` payload is a phi handle with a fully type-erased marker (reuse the existing `Dyn` erasure convention from `marker.rs`; it exposes only erased accessors, no typed narrowing). Discriminate on the phi's result type at `classify()` time. Removes the lying `as_int_value()` path (`instructions.rs:1049`); the `Other` handle exposes only type-erased accessors.
5. **Typed operands — rule: type exactly what the IR grammar guarantees.** `pointer() -> PointerValue` on `LoadInst` (`instructions.rs:311`), `StoreInst` (`:373`), `GepInst` (`:428`), `AtomicCmpXchgInst` (`:1780`), `AtomicRMWInst` (`:1843`), `VAArgInst` (`:1507`). `CallInst::classify_callee() -> Callee::{Direct(FunctionValue), Indirect(PointerValue)}` (new enum; `callee() -> Value` may remain as escape hatch). Binop/cmp/select operands stay `Value` (int-or-vector legitimately). Reuse existing `PointerValue` (`typed_pointer_value.rs`); do NOT invent new typed wrappers beyond what exists.
6. **Missing readers.** `SwitchInst::cases() -> impl ExactSizeIterator<Item = (Value, BasicBlockLabel<Dyn>)>` (case constants surfaced as stored), `IndirectBrInst::destinations()`, `LandingPadInst::clauses() -> impl Iterator<Item = LandingPadClause>` (catch/filter distinction), `CatchSwitchInst::handlers()`. Data already lives in per-variant payloads (`instruction.rs:205-207` `operand_ids` shows the storage); this is read-surface only, valid on `TermClosed` handles.
7. **Groupings.** `InstructionKind::as_binary_op(&self) -> Option<BinaryOp<'ctx,B>>` — one grouped view with `lhs()`, `rhs()`, `opcode() -> BinOpcode`, `is_commutative()`; generate impls inside the existing binop macro. `as_cmp(&self) -> Option<Cmp<'ctx,B>>` unifying ICmp/FCmp with a shared predicate view (reuse `cmp_predicate.rs`). Extend `OverflowingBinaryOperator` (`operator.rs:18`) to `ShlInst` (C++ parity: add/sub/mul/**shl**); add `PossiblyExactOperator` trait for `UDiv`/`SDiv`/`LShr`/`AShr` (`is_exact` already exists per-handle via the macro; the trait unifies it).
8. **Fix the token gap:** `AtomicRMWInst::set_value_operand` (`instructions.rs:1855`) gains a `&Module<'ctx,B,Unverified>` parameter, restoring the "no mutation without a token" rule. Compile-fail test pins it.

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
- Tests: transliterate the surveyed InstCombine examples (`InstCombineAndOrXor.cpp:1936`, `:1965`, `InstCombineAddSub.cpp:878`) as unit tests over builder-constructed IR.

## Package 3 — Pass ergonomics (`pass_context.rs`, `iter.rs`, `basic_block.rs`, `dce.rs`, `inst_simplify.rs`)

1. **Erase-safe cursor** on `FnPatch`: early-inc semantics (advance past current before yielding); yields a new linear `NonTerminator<'ctx,B>` handle; `FnPatch::erase` re-typed to accept only `NonTerminator` (consuming) — terminator erasure becomes unrepresentable, deleting the runtime rejection at `pass_context.rs:506`. Terminator reachable only via a separate non-erasable view. Cursor checks id-liveness on advance (skips instructions erased behind its back by cascades) — witnessed, documented.
2. **Worklist helper**: dedup set + stack (`SetVector` semantics), `push(inst)`, `push_users_of(&value)`, `push_operands_of(&inst)`, drain loop integrating with the cursor. Mirrors `DCE.cpp:89-107` / InstCombine worklist conventions.
3. **Dirty-bit (witnessed)**: `FnPatch`/`FnReshape`/`ModRewrite` track whether any mutation actually occurred; clean `done()` returns the all-preserved report, dirty returns the rung floor. Deletes the duplicated read-only pre-scans (`dce.rs:44-51`, `inst_simplify.rs:53-68`). `cx.done()`/`unchanged()` before `mutate()` remain valid.
4. **In-pass builder**: `patch.builder_before(&inst)` / `patch.builder_at_end(&block)` returning the existing typestate `IRBuilder` positioned inside the function; existing builder guarantees carry over unchanged. Unlocks materializing folds (InstSimplify's `None` DataLayout-builder limitation noted at `inst_simplify.rs:6,62,93`).
5. **Rewrite `DcePass` and `InstSimplifyPass`** onto cursor + worklist + dirty-bit as the acceptance proof: no pre-scan, no restart-scan, no `instruction_ids()` cloning, no `from_parts` re-materialization in pass code. Before/after output equivalence tested on the existing corpora.
6. **Small fixes**: `BasicBlockView::instructions()` for the Inspect rung (today read-only passes escape via `as_function()`, `dce.rs:47-50`); `impl Deref<Target=FnPatch> for FnReshape` replacing the six hand-copied delegations (`pass_context.rs:582-628`); `DataLayout` cached on the mutator (kills the per-iteration clone at `inst_simplify.rs:73`).

## Package 4 — Analysis system, Option C (`analysis.rs`, `pass_context.rs`, `pass_manager.rs`, `dominator_tree.rs`)

Framework-witnessed preservation; **no author claims API anywhere** (rejected Option A).

1. **`CfgUpdate` recording**: `enum CfgUpdate { InsertEdge{from,to}, DeleteEdge{from,to} }` (crate-private constructor). Every structural edit method on `FnReshape` (today `split_block`; all future CFG ops) records its own decomposition into the mutator's queue. Authors cannot construct, submit, reorder, or omit updates — the C++ `DomTreeUpdater` misuse class is unconstructible.
2. **`CfgIncremental` trait** for analysis results: `fn apply_updates(&mut self, updates: &[CfgUpdate], f: FunctionView) -> RepairOutcome`; `enum RepairOutcome { Repaired, PreferRecompute }`. Hookless or refusing analyses fall back to floor eviction (today's behavior = the degenerate case).
3. **Flush points**: (a) at `done()` — driver dedups cancelling pairs, feeds cached hooked results, marks preserved only what it watched succeed (added on top of the floor in the returned report); (b) mid-pass — `FnReshape::analysis_repaired::<A>(&mut self) -> &A::Result`, which flushes the queue into the cached result first. Borrow rules make holding a CFG-analysis ref across an edit a compile error; CFG analyses without the hook are not readable mid-reshape at all. This fixes the existing latent stale-read footgun (prefetched dom tree read after `split_block`).
4. **Phase 1 ships plumbing only**: `DominatorTree` implements the hook returning `PreferRecompute` (fresh recompute on mid-pass read; eviction at end). **Phase 2 (separate follow-up spec, not this one)**: real incremental repair, property-tested (random edit sequences: repaired ≡ recomputed) + debug-build recompute-compare at every flush.
5. **`Requires` without `Default`** (`analysis.rs:1103`): add instance registration `analyses.register::<A>(instance)`; `prefetch` uses the registered instance, falling back to `ensure_registered_default` only for `A: Default`. Parameterized analyses become declarable requirements.
6. Interplay: dirty-bit covers "nothing happened" (all preserved); CfgUpdate covers "CFG changed, specific analyses repaired"; both witnessed. Scope honesty: update vocabulary is CFG-shaped — value analyses (KnownBits/DemandedBits) are out of scope (already evicted by every mutating floor today; instruction-level events are a documented possible extension, not designed here).

## Testing / verification

- **Compile-fail (trybuild)** pinning every Tier-1 claim: terminator-erase-through-cursor impossible; analysis-ref-across-CFG-edit borrow error; hookless-analysis-mid-reshape; AtomicRMW-mutation-without-token; DSL binding type/arity mismatches; reopen-through-sub-enum variants (extend the existing finished-phi/switch fixtures). **Known gotcha:** `.stderr` files must be blessed on the canonical CI rustc (past CI breakage; see `docs/future-work.md:270-276`).
- **Runtime**: DSL unit tests transliterated from real InstCombine folds; DCE/InstSimplify rewrite equivalence vs current outputs; cursor semantics under erase-ahead/erase-behind; dirty-bit report correctness (clean run → all-preserved; single erase → floor); CfgUpdate recording unit tests for `split_block`; `cases()`/`clauses()` reader round-trips against builder-constructed IR.
- `cargo test --workspace` green on Windows dev box + CI (master+dev).

## Sequencing and branching

**No work happens on `dev` directly.** First step before any change: `git checkout -b feature-5/instruction-taxonomy` off current `dev`. This spec doc is the first commit on that branch.

One branch per package, each cut from `dev` after the previous merges (per the established feature-N → dev workflow, push at task boundaries):

1. `feature-5/instruction-taxonomy` — spec doc + Package 1
2. `feature-6/pattern-matchers` — Package 2 (needs P1's typed accessors/macros)
3. `feature-7/pass-ergonomics` — Package 3
4. `feature-8/analysis-plumbing` — Package 4 (P3/P4 touch mostly disjoint files; may overlap if convenient)

Version stays 0.0.x; each package notes its breaking changes in CHANGELOG. CI runs on `master`+`dev`; each merge to `dev` must be green first.

## Out of scope (recorded as considered/deferred with reasons)

InstVisitor port (exhaustive `match` + groupings supersede it); Salsa-style dependency tracking (requires revision-tracked IR storage — rearchitecture); DSL macro sugar (`ir_match!`) — later layer over combinators; debug-info salvage on erase (debug-loc story too thin); Phase-2 incremental dom-tree repair (own spec); instruction-level update events for value analyses; everything already listed in `docs/future-work.md` (executable pipelines, instrumentation wiring, loop/CGSCC rungs, ModRewrite per-function analyses).
