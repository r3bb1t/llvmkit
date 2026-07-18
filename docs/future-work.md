# Future work

This document captures everything the `feature-1/irbuilder-type-safety`
session's audits found but did not implement. Each item cites the audit
source (source file or upstream reference) so a later session can pick it
up cold. Transcribed faithfully from that session's approved design and
plan documents (kept outside the repository), plus a "Session follow-ups"
section for items individual tasks punted during execution.

## Killer-feature designs (deferred)

- **Inline IR macro DSL** -- a `ir!{ %sum = add i32 %a, %b }` proc-macro
  added to the EXISTING `crates/llvmkit-macros/` crate (which already ships
  the `IrStruct` derive in `ir_struct.rs`; new sibling module `ir.rs` per the
  one-concept-per-file convention). Expands `.ll`-flavored syntax into typed
  builder calls at compile time, with typed Rust splices (`#lhs`)
  type-checked against the spelled IR types. Reuses `llvmkit-asmparser`'s
  lexer at proc-macro time for tokenization fidelity. Design sketch: parse to
  the existing instruction payload shapes, emit `build_*` calls; unsupported
  constructs fall back to a clear compile error naming the LangRef construct.
- **Rustc-quality diagnostics** -- when runtime checks do fail (dyn paths,
  parsed IR, verifier), render labeled spans into the printed IR with
  expected/found notes and suggestion hints. Builds on `llvmkit-support`'s
  `Span`/`SourceMap` (already used by the parser) plus a renderer; verifier
  errors gain an optional pretty-print path that quotes the offending
  instruction line from AsmWriter output. Candidate crate: keep in
  `llvmkit-support` as a `diagnostics` module.

## Upstream IRBuilder coverage gaps (from the comparison audit)

Signatures below are verified against the extracted `llvmorg-22.1.4` tree
(`orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/IR/IRBuilder.h`).

- Convenience casts: `CreateZExtOrTrunc`, `CreateSExtOrTrunc`,
  `CreateIntCast`, `CreateFPCast`, `CreateBitOrPointerCast`
  (IRBuilder.h ~1951-2038).
- Memory intrinsics: `CreateMemCpy` / `CreateMemSet` / `CreateMemMove` (each
  with `uint64_t` + `Value*` size overloads, plus `*Inline` and
  element-atomic variants); lifetime intrinsics `CreateLifetimeStart/End` --
  NOTE: in LLVM 22 these take only a pointer (size argument removed, allocas
  only; verified against 22.1.4).
- `CreateGlobalString` (needs globals + builder hookup; upstream
  `CreateGlobalStringPtr` is deprecated in its favor), `CreateAssumption`
  (takes operand bundles), the min/max family (`CreateMinNum`/`CreateMaxNum`
  with `FMFSource`, plus
  `CreateMinimum`/`CreateMaximum`/`CreateMinimumNum`/`CreateMaximumNum`), the
  intrinsic helper family (`CreateIntrinsic` 3 overloads,
  `CreateUnaryIntrinsic`, `CreateBinaryIntrinsic` -- the latter returns
  `Value*` because it folds), `CreateStepVector`, `CreateAggregateRet`
  (explicitly deferred in AGENTS.md).
- FMF-variant family completion: `CreateSelectFMF`, `CreateFPTruncFMF`,
  `CreateFPExtFMF` analogs (llvmkit has binop/fcmp `_fmf` variants already);
  consider an `FMFSource`-style "inherit FMF from instruction" helper.
- Const-index GEP shortcuts (`CreateConstGEP1_32` etc.).
- Named `build_icmp_*` per-predicate wrappers already exist; audit found no
  gap there.
- Debug-loc threading and operand-bundle infrastructure (deferred with
  metadata work).
- RAII-style `InsertPointGuard` / `FastMathFlagGuard` analogs (Rust shape:
  scoped closure `with_insert_point(bb, |b| ...)` rather than Drop guards).

## Ergonomics backlog (from the core audit)

- `build_atomic_cmpxchg` / `build_atomicrmw` builder-pattern variants (mirror
  `CallBuilder`).
- Load/store variant explosion (base / `_with_align` / `_volatile` /
  `_volatile_with_align` / `_atomic` = 10+ methods per op) -- consolidate
  behind `LoadBuilder`/`StoreBuilder` chainables while keeping the flat
  forms.
- Per-flag convenience wrappers (`build_int_add_nsw` etc.) mirroring upstream
  `CreateNSWAdd`.
- Folder trait ergonomics for third-party folders (default method bodies
  landed in this session's hardening workstream; a
  `TargetFolder`/`InstSimplifyFolder` analog remains future work).

## Inspiration-derived candidates (web-researched)

- **"No-panic" positioning vs inkwell** (marketing + README bullets,
  near-zero code): inkwell's own docs and issue tracker document runtime
  panics on misused conversions (`into_float_value()` on an int panics --
  e.g. [wasmer#962](https://github.com/wasmerio/wasmer/issues/962)), panics
  on interior-NUL strings, and no multithreaded mode ([inkwell
  README](https://github.com/TheDan64/inkwell)). llvmkit's counterpart story
  is exact: typed handles make conversion misuse a compile error, there are
  no C strings anywhere, and every crate is `#![forbid(unsafe_code)]`. This
  session's README update (Task 20) turned this into a "why llvmkit vs
  inkwell" comparison section; a fuller marketing pass (blog post, crates.io
  description) remains future work.
- **E-graph optimization substrate** (L, future): an equality-saturation
  InstCombine/peephole analog built on
  [egg](https://github.com/egraphs-good/egg)/egglog -- Cranelift is already
  exploring e-graph-based optimization ([SIGPLAN
  blog](https://blog.sigplan.org/2021/04/06/equality-saturation-with-egg/)).
  llvmkit's typed constant-fold kernels + pass infrastructure give it a
  natural home as a `PatchBody`/`ReshapeCfg`-rung pass family. Would be a genuine "LLVM 2.0"
  differentiator: phase-ordering-free peepholes.
- **Alive2-style refinement checking** (L, future, visionary):
  [Alive2](https://github.com/AliveToolkit/alive2) does bounded translation
  validation of LLVM transforms via SMT (found 47 bugs in LLVM's own test
  suite). A llvmkit-native `refines(before, after)` harness --
  property-test-based initially (interpret both modules over random inputs
  for the modeled subset), SMT-backed later -- would make llvmkit the only
  IR library with built-in transform validation. Pairs with Doctrine D10 (no
  silent UB).
- Note: the full 5-lens inspiration sweep + synthesis workflow did not
  complete during planning; the three findings above come from direct
  main-session searches instead. If deeper inspiration mining is wanted,
  see the session's archived plan for the sweep's methodology and rerun it.

## Type-system follow-ups

- **Const-generic `VectorType<E, Len<N>>` / `ArrayType<E, ArrLen<N>>` — shipped**
  (`feature-17/const-generic-vec-array`, S1–S6). `VectorType`/`VectorValue` and
  `ArrayType`/`ArrayValue` now carry a scalar **element** marker (the scalar
  itself — `i64`, `f64`, … via `VecElem`/`StaticVecElem` in `element.rs`) and a
  **length** marker (`Len<const N: u32>`/`LenDyn` in `vec_len.rs`;
  `ArrLen<const N: u64>`/`ArrLenDyn` in `array_len.rs`). The bare
  `VectorValue<'ctx>`/`ArrayValue<'ctx>` stay the all-`Dyn` (erased) form —
  parsed `.ll`, scalable vectors, and runtime lengths land there and narrow via
  `TryFrom` (`OperandWidthMismatch` for lane count, `IrError::ArrayLengthMismatch`
  for arrays, `TypeMismatch` for element). Constructors
  `Module::vector_type_n::<E, const N>()` / `array_type_n::<E, const N>()`. Typed
  ops make an element/length mismatch a **compile error**:
  `build_vec_int_{add,sub,mul,xor,and,or,shl,lshr,ashr}` (two
  `VectorValue<E, Len<N>>` with the same `E`,`N` ⇒ equal element+length for
  free), `build_vec_extract`/`build_vec_insert`/`build_vec_splat`, and array
  `build_arr_extract`/`build_arr_insert` (plus a typed-array `build_alloca`); the
  verifier's vector/array checks are unchanged (defense in depth). The old
  unwired `VectorElement`/`SizedElement`/`VectorDyn`/`ArrayDyn` markers were
  replaced by `VecElem`/`ElemDyn`. Residual, deliberately still erased / `Dyn`:
  - **Length-relating ops** — shufflevector output length, concat (`N1+N2`),
    compile-time index-in-bounds (`I<N`), cross-`Len` widen/narrow — **blocked on
    `generic_const_exprs` (unstable)**, the same wall as the integer `WiderThan`
    relations below.
  - **Scalable vectors** — always `Dyn` (**scoped out** this cycle).
  - **Pointer-element vectors** — **scoped out**, blocked on address-space
    markers (see the address-space-typed-pointers bullet below).
  - **Composite-element arrays** (`[N x {..}]` / `[N x [..]]` / `[N x <..>]`) — a
    scalar element marker can't name a composite element (**scoped out**).
  - **Float / div / rem vector binops and vector `icmp`/`fcmp`** — **scoped out**;
    no existing erased `_dyn` lowering to reuse.
  - `build_vec_splat` can't infer its element from the scalar (a Rust
    associated-type-projection limitation), so its callers annotate / turbofish
    the result.
- **A proof token that *carries* the validated `TypeId`** (residual after the
  unforgeable-markers cycle). The crate has five capability tokens -- `WrapWitness`
  (`element.rs`), `ValidatedFunctionParams` / `ValidatedCallResult`
  (`function_signature.rs`), `SelectNarrow` (`ir_builder.rs`), `ValidatedStructValue`
  (`struct_schema.rs`) -- each defending the *external* boundary and each a *unit*
  marker that proves "a check happened", not *which* type. The unforgeable-markers
  cycle made the builder's **int / float / pointer append surface** structural instead:
  a marker is attached to a freshly-appended instruction only through the typed-append
  constructor family (`append_int_like` / `_at` / `_load`, `append_fp_*`, `append_ptr`
  / `append_ptr_load`), each of which appends AT a typed handle so the marker matches
  the runtime type *by construction* — those ~40 sites no longer carry an implicit proof.
  What remains implicit is the smaller residual the family does not cover: the `CallInst`
  / `PhiInst` result accessors in `instructions.rs`, the arena / parameter lifts in
  `ssa_builder.rs` (`use_*_var`) and `function_signature.rs`, the vector / array append
  wraps (no `append_vec` / `append_arr` constructor yet), and the `IntoIntValue` /
  `IntoFloatValue` const-lifts in `int_width.rs` / `float_kind.rs`. A witness carrying the
  validated `TypeId` would let those *state* their proof instead of implying it. Note the
  confinement of `from_value_unchecked` is **audited, not compiler-enforced**: it stays
  `pub(crate)` because a hard seal is impossible (`value` and `ir_builder` are sibling
  modules and the constructors need `ir_builder`-private helpers), so the builder's fold
  re-checks remain the runtime backstop.
- `Width<M>`/`Width<N>` `WiderThan` relations blocked on stable
  const-generics (documented at `int_width.rs` ~105-116); revisit when
  `generic_const_exprs` stabilizes.
- Aggregate variable categories for auto-SSA (currently ships int/float/pointer
  only).
- Address-space-typed pointers (`PointerValue` currently erases address
  space; audit item from infra report).

## Session follow-ups

Items this session's own workstreams punted, beyond the plan's original
future-work list above. Each cites the source file/design decision that
deferred it.

- **Typed `fold_gep`/`fold_select` hooks** -- blocked on address-space-carrying
  pointer markers; `PointerValue` doesn't pin the address space and vector
  element typing is deferred to T4, so `fold_gep_dyn`/`fold_select_dyn` stay
  erased + runtime-checked (documented in `ir_builder/folder.rs` trait
  rustdoc).
- **`[F; N]` `IrField` arrays** -- fixed-size array fields in `#[derive(IrStruct)]`
  schemas; would let derived structs model `[i32; 4]`-shaped LLVM array
  members directly instead of requiring a hand-written wrapper.
- **Vector-of-pointer GEP bases** -- `build_gep`/`build_field_gep` currently
  assume a scalar pointer base; vectorized GEP (`<N x ptr>` base, per-lane
  offsets) is unmodeled.
- **Derive-generated field-index consts** -- `build_field_gep::<S, I>` takes
  the field index as a bare `const I: u32`; the derive macro could emit named
  constants (e.g. `Point::X_INDEX`) so call sites read `build_field_gep::<Point,
  { Point::X_INDEX }>` instead of a magic number.
- **`TypedInvokeInst<Ret>` schema wrapper** -- `build_invoke` returns
  `InvokeInst<Ret::Marker>` today; a `TypedCallInst`-style wrapper carrying
  the full `Ret: FunctionReturn` schema (not just the derived marker) is
  mechanical follow-up work noted in the typed-calls design (Workstream 1)
  as "deferred (mechanical later, reuses `CallArgs` unchanged)".
  Same design note also defers typed `callbr`, typed intrinsic calls, and
  varargs invoke -- all mechanical extensions of the shipped `CallArgs`/
  `IntoCallArg` machinery.
- **Auto-SSA aggregate variables + invoke/EH terminators** -- `ssa_builder.rs`
  currently ships int/float/pointer variables and `br`/`cond_br`/`switch`/`ret`/
  `ret_void`/`unreachable` terminators only. Aggregate variable categories
  (per-field fan-out through `StructSchema`) and `invoke`/`callbr`/EH
  terminators are the documented future scope in the module's own doc comment.
- **`IrField::ir_type` accepting `ModuleRef`** -- `IrField::ir_type` and
  `StructSchema::ir_type` currently demand `&Module<'ctx, B, Unverified>`.
  `build_field_gep` (`ir_builder.rs`) has to construct a temporary
  `Module::from_core(self.module)` wrapper solely to call `S::ir_type(...)`.
  Widening the trait method to accept `ModuleRef`/`&ModuleCore` directly
  would remove the need for `Module::from_core` entirely. Flagged during
  Task 6's review as a candidate for this document.
- **`proptest` `undef_var` index randomization** -- the auto-SSA property
  test suite's undefined-variable-read fixture hardcodes `Some(0)` as the
  undefined variable index instead of drawing from `0..var_count`; a one-line
  improvement to widen coverage (noted during Task 19's review).
- **`accept_folded/narrow_folded` helper-family factoring -- done**
  (`feature-22/generic-narrowing`, the "no silent erasure" cycle). The four
  near-identical bodies were folded into a single compare-and-report core,
  `Type::require_match` (`type.rs`), which every fold- and variable-def seam
  now routes through -- so the same type drift reports the same error wherever
  it is caught. That unification was not the goal but a consequence: the seams
  had to be touched anyway to delete a marker-keyed short-circuit, and leaving
  four copies would have meant four places for the error shape to diverge again
  (it already had -- `narrow_folded_int` reported `TypeMismatch { Integer,
  Integer }` where the acceptor reported `OperandWidthMismatch`).

## Upstream-parity review follow-ups (2026-07-06)

A six-agent audit of the shipped overhaul against
`orig_cpp/llvm-project-llvmorg-22.1.4/` confirmed the builder semantics clean
and produced two fix waves: the first (fold_phi poison skip,
definitive-initializer gate, i128 sign-extension, SSA poison-arm RAUW + chase
cycle detection, any-order flag parsing, call-site fn_ty independence), and the
LLVM 22.1.4 parity-completion pass (DataLayout default alignment on
load/store/alloca, alloca array-size / `inalloca` / `swifterror` / DL alloca
address space, GEP index validation, indirect invoke, musttail ellipsis rules,
unordered-atomic-load DCE + trivially-dead InstSimplify erase, and the
`llvmkit-default<On>` recipe rename). The `build_is_null`/`build_pointer_cmp`
folder-bypass item was already fixed on dev (`b06413e`). The items below remain
deliberately deferred; each cites its upstream anchor.

- **Indirect `callbr`** -- `callbr void %fp(...)` is invalid IR upstream
  (`Verifier::visitCallBrInst` requires a direct callee for non-asm callbr:
  "Callbr: indirect function / invalid signature"), so llvmkit rejects it at
  parse, which reaches the same verdict. A stricter port would accept it at
  parse and reject in the verifier. (Indirect *invoke* is now supported -- it
  is valid IR.)
- **DCE removable calls / allocs** -- llvmkit still keeps `willReturn`+readnone
  calls, removable allocation-function calls, `free(null)`, and lifetime-only
  allocas that upstream `wouldInstructionBeTriviallyDead`
  (`lib/Transforms/Utils/Local.cpp`) deletes. Porting these needs faithful
  allocation-function / attribute modeling to avoid over-removal (a miscompile
  if wrong), so the current DCE stays conservative-but-safe. `Value::has_uses`
  also counts debug-record uses upstream ignores (upstream salvages debug info
  instead). (Unordered atomic loads are now removed.)
- **InstSimplify unreachable-block skip** -- the pass still folds in
  unreachable blocks that upstream skips (`InstSimplifyPass.cpp:33-37`), a
  textual-only divergence in dead code; needs reachability (a dominator tree)
  threaded into the pass. No InstSimplify tests cover freeze folds or
  unreachable-block behavior yet (the latter blocked on this skip). (The
  erase-only-when-trivially-dead behavior is now matched.)
- **Deeper `swifterror` dataflow verification** -- the swifterror alloca
  support verifies the parse-level constraints (pointer type, non-array); the
  full `Verifier` use-site rules (swifterror values may only flow through
  specific call/load/store positions) are not yet enforced.
- **Plain add/sub/div/shift hook dispatch** -- `build_int_add`/`build_int_sub`
  consult the plain `fold_int_bin_op` hook where upstream `CreateAdd` funnels
  through `FoldNoWrapBinOp(.., false, false)` (and `CreateUDiv` et al. through
  `FoldExactBinOp(.., false)`). Identical results with the shipped folders;
  observable only by third-party folders that override just the
  no-wrap/exact hooks.
- **Vector-of-pointer GEP bases** -- `build_gep` / the parser assume a scalar
  pointer base; `<N x ptr>` vector GEP bases (`getGEPReturnType`'s vector arm)
  are unmodeled (documented earlier in this file). Consequence for the new GEP
  index validation: the struct-index-must-be-`i32` check (`StructType::indexValid`,
  upstream `isIntOrIntVectorTy(32)`) is enforced for scalar indices only; the
  `<N x i32>` vector-index case is unreachable here because a vector-index GEP
  requires a vector base, which is rejected earlier. Revisit the check when
  vector GEP bases land.

## Pass API — deferred

The `feature-4/pass-api-v2` branch shipped the capability-graded pass API
(rungs, contexts/mutators, `FunctionPass`/`ModulePass`, single-pass drivers,
static tuple pipelines, `Analyses` bundle, `Dyn` containers, and the
`#[function_pass]`/`#[module_pass]` sugar). What it deliberately scoped out:

- **Executable textual / string pipelines** -- `pass_pipeline.rs` parses
  opt-style pipeline strings into names and recipes
  (`parse_pass_pipeline_text`, `PassPipelineRecipe`, `PassPipelineTextName`),
  but there is no `NAME`->pass-constructor registry, so a parsed pipeline
  cannot yet be *run*. A registry mapping each pass's `NAME` to a boxed-pass
  constructor would let a textual pipeline drive the `Dyn` containers.
- **Per-function analyses in `ModRewrite::for_each_function`** -- the
  module->function visitor (`pass_context.rs`) builds each per-function mutator
  with empty results `()`, so a `FnPatch::analysis` call inside the visitor has
  no members to select. A future revision threads a per-function `Requires`
  list (prefetched per visited function) through the visitor.
- **Instrumentation wiring** -- the `const NAME` / `const REQUIRED` pass
  members and `PassInstrumentationCallbacks` (`pass_instrumentation.rs`) exist,
  but the single-pass drivers and pipelines (`pass_manager.rs`) do not yet fire
  before/after-pass instrumentation callbacks or honor skip decisions. The
  `pass_names()` / `has_required_pass()` accessors on the `Dyn` containers are
  the surfaced hooks awaiting a consumer.
- **Loop and CGSCC rungs** -- the capability lattice (`pass_access.rs`) covers
  single-function-body and whole-module rungs only; loop-nest and
  call-graph-SCC pass rungs (upstream `LoopPassManager` / `CGSCCPassManager`)
  are unmodeled.
- **First-class `ModRewrite` runtime-symbol/global/ctor triple** -- the
  `RewriteModule` mutator (`ModRewrite`, `pass_context.rs` ~1247) exposes only the raw
  `module_mut()` token today; a sanitizer reaches the
  function/global/constructor "triple" through it by hand. The author sugar for
  that pattern -- `declare_runtime_fn` / `append_ctor` / `add_global` helpers
  plus the `llvm.global_ctors` machinery -- is deferred until an in-tree
  consumer needs it.
- **`Module::scratch_unverified` footgun** -- the read-only `Dyn` containers
  (`pass_manager.rs`) call the `pub(crate)` `Module::scratch_unverified`
  (`module.rs` ~2977) to mint a throwaway `Unverified` alias purely to satisfy
  the erased pass signature; every member is `Inspect`, so the token projects
  to `()` and never reaches a mutator. It is sound today by that argument, but
  a `pub(crate)` unverify with no caller marker is a footgun -- a sealed
  caller-marker token would pin that only the read-only drivers can mint it.
- **Compile-fail `.stderr` canonical-rustc bless** -- the
  `typestate_compile_fail` suite carries two pre-existing `.stderr` drifts
  (`folder_typed_wrong_width`, `extract_value_empty_indices`) blessed against a
  different (CI) rustc, plus the six new pass-API fixtures (including
  `claim_preserved_after_mutate`) blessed on the local rustc. The whole set
  should be re-blessed on the canonical CI rustc so every fixture matches on the
  reference toolchain.

## Package 4 (analysis preservation) — deferred

Framework-witnessed analysis preservation shipped across `feature-8`
(Phase 1) and `feature-9` (the remainder): the `CfgUpdate` recording vocabulary
(`cfg_update.rs`), the `CfgIncremental` hook (`RepairOutcome` +
`apply_updates`/`recompute`), the reshape mutator's witnessed edit log, the
*unrepresentable* mid-reshape stale CFG-analysis read
(`FnReshape::analysis_repaired`, no `Deref`, compile-fail fixture),
`Requires`-without-`Default` (`PrefetchableAnalysis`), and the `done()`-flush
witnessing loop that keeps a reshape pass's dominator tree
(`DominatorTree::apply_updates` repairs correct-by-recompute → `Repaired`; the
driver marks preserved exactly what it watched repair). What remains deferred:

- **Sub-linear incremental dominator repair (perf).** `DominatorTree::apply_updates`
  is *correct* but repairs by full recompute — it does not yet use the recorded
  edge insert/delete list to do sub-linear work. A genuine incremental update
  (LLVM SemiNCA-style, driven by `updates`) is the perf follow-up. When it lands,
  a `debug_assert` comparing the incrementally-repaired tree to a from-scratch
  recompute (`repaired ≡ recomputed`) should guard every flush; the
  `dominator_tree_repairs_to_match_recompute` test is the seed of that property.
  Needs random-edit-sequence property tests (proptest). No behavior change vs.
  today when it lands (only speed), so low urgency without a large-function
  workload.
- **`PrefetchableModuleAnalysis` (module `Requires` without `Default`).** The
  function side dropped the `Default` bound via `PrefetchableAnalysis`; the
  module analysis-list macros still bound `+ Default`. There are no concrete
  module analyses yet, so a mirror trait would be untestable dead machinery —
  introduce it (same shape) with the first non-`Default` module analysis.
- **Value-analysis update vocabulary.** `CfgUpdate` is CFG-shaped only.
  Instruction-level events for value analyses (KnownBits/DemandedBits) are a
  possible extension, not designed here -- every mutating rung's floor already
  evicts them.
- **`ModRewrite::for_each_function` reshape flush.** The module→function visitor
  builds `FnReshape` mutators whose `done()` (and thus `CfgUpdate` log) the
  visitor never surfaces, so those reshapes do not run the witnessed flush. This
  is sound today: the enclosing `RewriteModule` floor is `none()`, which evicts
  every CFG analysis anyway. Wire the flush through the visitor if per-function
  analyses are ever threaded into `for_each_function`.
- **New `.stderr` under the canonical-rustc bless caveat.**
  `reshape_stale_cfg_analysis_across_edit` is blessed on the local rustc like
  the pass-API fixtures above; its `E0502` borrow-error wording is stable
  across toolchains, but it joins the set that should be re-blessed on the
  reference rustc.

## Phi authoring — shipped

The block-argument authoring surface (`append_block_with_params`,
`append_block_with_named_params`, `build_*_with_args`), dominance-witnessed
`FnReshape::insert_phi`, the "break" that made the raw phi builders (the six
`build_*_phi`, the open-phi `add_incoming`/`finish`) internal — block arguments
are now the *only* public phi-authoring surface — the **typed terminator edit
surface** (`FnReshape::edit_terminator` and the `edit_switch`/`edit_cond_br`/
`edit_br`/`edit_invoke`/`edit_callbr` narrows → `BrEdit`/`CondBrEdit`/
`SwitchEdit`/`InvokeEdit`/`CallBrEdit`) that *replaced* the dynamic
`remove_edge`/`redirect_edge` edge ops (`BranchInstData.kind` became a `RefCell`,
so a branch successor is retargeted and a `cond_br` collapses to a `br`,
deregistering the dead condition — now through role-named `redirect_*`/`remove_*`
whose very method set encodes the legal edits, so a structurally-invalid edge
edit is a *compile* error rather than a runtime rejection), the verifier
phi-result-type rule (`VerifierRule::PhiInvalidResultType`, defense in depth —
`check_phi` rejects a phi whose result is not a first-class data type, matching
the parser), and the zero-incoming-phi verifier backstop
(`VerifierRule::PhiEmptyInReachableBlock` — `check_phi` now rejects a phi that
carries no incomings in a block reachable from entry, gated on
`DominatorTree::is_reachable_from_entry`; such a phi is un-round-trippable
because `LLParser::parsePHI` rejects a bracket-less `%p = phi i32`, and the
shared `check_phi_incoming` count guard would otherwise miss it on the `0 == 0`
gap LLVM's `visitPHINode` shares) have all shipped.

The former follow-up here — **edge ops on `invoke`/`callbr`** — has also
shipped: the `invoke` (`normal_dest`/`unwind_dest`) and `callbr`
(`default_dest`/`indirect_dests`) successor `Cell`s are editable through the
typed edit surface — `edit_invoke` → `redirect_normal`/`redirect_unwind`,
`edit_callbr` → `redirect_default`/`redirect_indirect` — retargeting a successor
`Cell` in place exactly as the `br`/`cond_br` arms do. *Removal* is structurally
N/A for them: both `invoke` edges and the `callbr` default are mandatory and the
indirect count is fixed, and that absence is a compile-time guarantee (the
`InvokeEdit`/`CallBrEdit` handles carry no `remove_*`), not a gap.

**Typed block parameters** have also shipped (`feature-20/typed-block-params`) —
the block analog of the const-generic vector/array retrofit. A `BlockParams`
sealed marker with the erased `BlockParamsDyn` default sits as the last,
defaulted `Params` type parameter on `BasicBlockLabel`/`BasicBlock`, so all
erased authoring is unchanged; `IRBuilder::append_block_typed::<Params>` appends
a `Params`-stamped block with typed head-phi parameter handles; and the
`BlockCall<'ctx, R, B, Params>` edge (`head.call(args)` consumed by
`build_br_call`/`build_cond_br_call`) makes a wrong-arity or wrong-typed
block-argument a *compile* error. The erased surface
(`append_block_with_params`, `build_br_with_args`, `build_cond_br_with_args`) is
untouched. Two follow-ups remain deferred:

- **Edit-surface `BlockCall` integration.** The reshape edit surface's typed
  `redirect_*` phi-seeds stay erased (`&[Value]`): passes operate on `Dyn`
  block labels (`BasicBlockLabel<R, B, BlockParamsDyn>`), so a typed `BlockCall`
  built from a `Params`-stamped label is rarely usable at a redirect site — the
  pass would first have to recover (or carry) a typed label. Until a pass surface
  threads typed labels through, the typed `BlockCall` edge is a construction-time
  (`IRBuilder`) convenience only, and the edit surface keeps taking erased
  per-parameter value slices.
- **Typed params beyond arity 12.** `BlockParams` has a `Debug` supertrait and
  the standard library stops deriving `Debug` on tuples past arity 12, so a
  `>12`-arity typed parameter tuple cannot satisfy `BlockParams` even though it
  is a valid `FunctionParamList`; a block with more than twelve typed parameters
  must fall back to the erased `BlockParamsDyn` form. Lifting the ceiling needs a
  `Debug` path for larger tuples (drop the supertrait, or supply a manual `Debug`
  for a fixed-shape wrapper) — the same std-tuple `Debug` wall that caps typed
  function parameters.

**Typed terminator operands** have also shipped
(`feature-21/typed-terminator-operands`) — the program's move from a
terminator's *edges* to its *operands*. The `switch` condition/case integer
width is now a last, defaulted `W: IntWidth = IntDyn` parameter on
`SwitchInst<'ctx, P, B, W>`; `IRBuilder::build_switch::<W>` pins `W` and
its `add_case` carries an `IntoIntValue<'ctx, W, B>` bound, so a wrong-width
case value is a **compile error** (the erased `build_switch_dyn` keeps the runtime
`TypeMismatch` check). And `build_indirectbr`'s address bound tightened from
`IsValue` to `IntoPointerValue`, so a typed non-pointer jump address is a
**compile error** (the pointer-ness check moves from `verify()` to build time;
erased `Value` addresses are unchanged). Parser / SSA-builder paths and the
whole erased authoring surface are untouched.

With the edit surface, typed block parameters, and now operand typing all
shipped, the **"branching bugs impossible at the type level"** program's typed
surfaces are largely complete. What remains is deliberately out of scope rather
than pending:

- **Universal per-function branding** — deferred on feasibility grounds (the
  full-program lifetime/branding gymnastics do not pay for themselves versus the
  per-module brand already in place); the locked API decisions from the program's
  design remain, but this rung is not being pursued.
- **Whole-graph verifier territory** — phi-incoming completeness against the
  final predecessor set for builder-constructed IR, and dominance, are permanent
  residents of `Module::verify()` (defense in depth). These are whole-graph facts
  that cannot be a local construction- or parse-time guarantee, so they stay the
  verifier's job by design, not a gap to close.
