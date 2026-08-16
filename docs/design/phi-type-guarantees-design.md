# Phi Type-Level Guarantees — Design Spec (final: block-args-first, breaking OK)

Date: 2026-07-10. Target: `crates/llvmkit-ir` + `crates/llvmkit-asmparser`. Two waves: `feature-11/phi-guarantees` (slices 1–5) then `feature-12/phi-block-args` (slices 6–10), each off `dev`, merged `--no-ff` back, gates green per slice. Execution: subagent-driven development per established workflow. This document is the spec of record; it was never copied into `docs/superpowers/specs/`.

> **Outcome, recorded 2026-07-27 (llvmkit 0.0.4).** Both waves shipped, all ten slices. This is the dated design record, kept as written; everything below described as "today" is the 2026-07-10 tree, and the `file:line` citations have drifted — the symbol names are the stable reference. Four places where the shipped shape differs from the design are flagged inline (slices 6, 7, 9, 10). One correction that touches every slice: `BasicBlockLabel` was **not** replaced. It survives as the *borrowing view* a `BlockId` resolves to through `Module::view`; what changed in 0.0.4 is that `BlockId<R, B, Params>` — `Copy + Send + 'static` — is the **storable** currency that pass-facing and reader APIs hand back and take.

## Context

Phi nodes are the most bug-prone IR feature in LLVM-family compilers. Research-confirmed failure classes: (1) predecessor↔incoming desync after CFG edits (LLVM's manual `removePredecessor` is the forgettable step; real miscompiles), (2) incoming-type mismatch (LLVM #54831/#2361), (3) duplicate entries for one block with different values (InstCombine #196954), (4) phis not grouped at block top. MLIR and Cranelift eliminated the class with block arguments; Swift SIL uses block arguments and **lowers them to LLVM phis in IRGen** — proving the authoring-layer-over-phi-storage shape at production scale. Cranelift's `seal_block` ("all predecessors known") makes phi completeness locally checkable during construction.

llvmkit as of 2026-07-10 (explorer-verified; every defect named here is fixed by the slices below): solid `Open/Closed` typestate on typed phi handles; typed `add_incoming` checks value type at runtime; but the untyped `IrBuilder::phi_add_incoming_from_value` (parser + `ssa_builder` path) checks nothing until `verify()`; `*_phi` inserts at the cursor (misplacement possible); no predecessor-mutation/phi-fixup API exists; and **`FnReshape::split_block` leaves successor phis naming the stale predecessor — a correct pass produces IR failing `verify()` (live bug)**. The parser also **rejects valid LLVM IR** (`phi <4 x i32>` — "phi result type must be int, float, or pointer", in `ll_parser.rs::parse_phi`). `docs/type-safety-vs-llvm.md`'s phi section declares phi/pred coherence "intentionally verifier-only" — this spec revises that boundary.

**Decisions locked with user (in order):** full sweep; CFG strategy = mutation carries the fix; maximum construction strictness; close the gaps (Cranelift sealing + SIL block-args research); **breaking changes are fine — go as far as keeping LLVM behavior parity allows, without reinventing** → block arguments become the ONLY public authoring surface; phi storage, parser, printer, and verifier semantics stay LLVM-shaped; the raw phi authoring APIs go internal.

Doctrine: unrepresentable > witnessed > tested, never trusted.

---

## Wave 1 — `feature-11/phi-guarantees` (fix + harden the existing machinery)

### Slice 1 — Fix the live bug: `split_block` carries the phi fix

- `FnReshape::split_block` (`pass_context.rs`): the terminator moves from `source` to `new_block`, so each `source→succ` edge becomes `new_block→succ`. For every phi in every successor, rewrite incoming entries whose block == `source` to `new_block` (`PhiData.incoming`'s second slot, `instr_types.rs`). Mechanical, no author input — desync unrepresentable for this op.
- TDD: failing test first (split over a phi-successor; today `verify()` fails `PhiPredecessorMismatch`; after: passes, incoming block is `new_block`, value unchanged).
- Doc-comment the `FnReshape` contract: every structural op maintains phis in the blocks it touches; ops that make phis *gain* entries take the values as typed arguments (fulfilled by slice 9).

### Slice 2 — Placement unrepresentable at build time

- All six `*_phi` builders (`ir_builder.rs`) insert at the block's **phi head** (after existing phis, before first non-phi) via a new crate-private `insert_phi_at_head`. Cursor position becomes irrelevant to phi placement. (`ssa_builder::emit_operandless_phi` already head-inserts — converges.)

> **Shipped as** (recorded 2026-08-06, 0.0.4). The helper is `BasicBlock::insert_instruction_at_phi_head` (`basic_block.rs`, `pub(crate)`), not a free `insert_phi_at_head`; every phi-emitting path in `ir_builder.rs` routes through it, so placement is correct-by-construction exactly as designed.
- **Parser duty:** auto-hoisting builders would silently reorder ill-formed `.ll`; the parser (`ll_parser.rs::parse_phi`) must instead track "seen non-phi in current block" and reject with a parse error. Fixture test.
- Verifier `PhiNotAtTop` stays (defense in depth).

### Slice 3 — Every edge-adding path checked

- `IrBuilder::phi_add_incoming_from_value` (`ir_builder.rs`) gains the type check (`value.ty == phi.ty` → `IrError::TypeMismatch`). Parser immediate edges, parser forward-ref resolution (in `ll_parser.rs` — the value exists by then), and `ssa_builder::phi_add_incoming_raw` inherit it. (Braun reads are same-typed — belt-and-braces, tested.)
- **Differing-duplicate rejection** (InstCombine #196954 class): all add paths (typed `add_incoming` ×3 + `phi_add_incoming_from_value`) reject a second entry for the same block with a different value → new `IrError::AmbiguousPhiIncoming`. Same block + same value stays legal (switch multi-edge). Verifier `AmbiguousPhi` stays.
- Update the `ssa_builder.rs` invariant comment on the raw phi-edge path.

### Slice 4 — Shared `check_phi` helper + parse-time completeness (internal sealing)

- Extract the verifier's per-phi logic (`verifier.rs`: count == preds with multiplicity, membership, multiplicity cap, differing-duplicate) into one shared crate helper used by the verifier, the parser, and later slices 8–9 — single source of truth, cannot drift. **Shipped as** the `phi_check` module: `check_phi_incoming` (crate-internal, the per-phi rule) and `check_function_phi_coherence` (`#[doc(hidden)] pub`, the whole-function sweep the parser calls), with `PhiCoherenceError` as its report.
- **Parser seals after each function**: once a function's body is parsed, all predecessors are known (Cranelift's insight), so the parser runs the helper over every phi and reports failures as source-located parse errors instead of distant `verify()` failures.
- No *public* seal API: after slice 7 the public surface cannot create incomplete phis at all, so user-facing sealing has nothing to check (YAGNI). `ssa_builder` keeps its own Braun sealing.

### Slice 5 — Pass-facing surface

- `matchers.rs`: `m_phi()` binding `PhiKind`; composes with existing combinators.
- `inst_simplify.rs`: uniform-phi fold (all incomings the same value `v`, self-references allowed) → replace with `v` via the existing worklist/RAUW machinery (user-cascade already pushes former users). Mirrors LLVM `SimplifyPHINode`'s conservative core. `scalar_cleanup` corpus diffs are intentional new folds, hand-reviewed.
- DCE: no change (unused phi already eligible; self-kept cycles out of scope, as upstream).

---

## Wave 2 — `feature-12/phi-block-args` (the breaking flip + remaining closures)

### Slice 6 — Block-argument authoring surface (SIL model)

- `append_block_with_params(fn, [types]) -> (block, params)` — each param is a `Value` backed by an operandless phi at the block's head (storage stays `PhiData`).
- `br_with_args(target, [values])`, `cond_br_with_args(...)`, `switch_with_args(...)` (+ an invoke variant if the builder ships `invoke` — pin at planning): append the terminator **and** add each value as an incoming from the current block in one call, arity- and type-checked at the call site. Edge and values move together — desync and incompleteness are structural non-events.

> **Shipped as.** `IrBuilder::append_block_with_params(function, param_types: &[Type], name) -> IrResult<BlockWithParams<'ctx, R, B>>`, where `BlockWithParams` is `(BasicBlock<'ctx, R, Unterminated, B>, Vec<Value<'ctx, B>>)` — the block comes back as the linear, non-`Copy` insertion handle that still owes a terminator, and `.id()` mints the storable `BlockId`. A naming twin, `append_block_with_named_params`, takes `(type, name)` pairs so block-argument authoring can reproduce named-phi output byte-for-byte. Wave 2 landed only two of the three argument-carrying terminators — `br_with_args` and `cond_br_with_args`; the arity and type guards live on the `*_with_args` builders themselves (`IrError::PhiArgArityMismatch`, `IrError::TypeMismatch`, both checked up front so a bad edge leaves no half-formed terminator). The typed layer that arrived later (`feature-20/typed-block-params`) covers the same two edges: `append_block_typed::<Params>` plus `BlockCall` consumed by `br_call` / `cond_br_call`, where a wrong-arity or wrong-typed block argument is a compile error. **The missing `switch` and `invoke` forms landed after the 0.0.4 freeze** (`feature-34/polish-freeze`, 2026-07-27): `switch_with_args` / `switch_dyn_with_args` take the default edge as a `(target, args)` pair plus a `(case_value, target, args)` triple per case — the whole case list at the call, so the returned `SwitchInst` comes back already `TermClosed` and no later `add_case` can bolt on an unseeded edge — and `invoke_with_args` / `invoke_dyn_with_args` take a `(destination, args)` pair for each of the invoke's two mandatory edges.
- Plain `br`/`cond_br` to a param-block → immediate arity error. **Designed, shipped late** (`feature-34/polish-freeze`, 2026-07-27, after the 0.0.4 freeze; for 17 days `br` resolved its target and recorded the edge with no param-count check, so the guarantee sat at `verify()` / parse time instead). Every plain terminator edge now routes through one guard (`basic_block.rs::require_no_block_parameters`) and reports `IrError::PhiArgArityMismatch` before the terminator is emitted: `br`, `cond_br`, `switch(_dyn)`'s default and `SwitchInst::add_case`, both edges of every `invoke*`, `callbr*`'s default and indirect edges, and `IndirectBrInst::add_destination` — so `indirectbr` to a param-block → error is shipped too (classic phi-with-`indirectbr` still parses from `.ll`, because the guard keys on *block parameters*, not on "the block contains phis"). That distinction is what keeps the `.ll` parser's back-edges and `SsaBuilder`'s unsealed loop headers working: a block records the parameter count it was *created* with (a `Cell` on `BasicBlockData`, set only by the three `append_block_*params` constructors), and that single read is the guard's hot-path early-out; the leading-head-phi scan (`block_parameter_phis`) is shared with `add_block_args` and only supplies the arity named in the error.
- Entry block takes no params (function arguments serve that role, as in MLIR/SIL).
- Storage, parser, printer, verifier: unchanged. Printed IR is ordinary phis.

### Slice 7 — THE BREAK: raw phi authoring goes internal

- Demoted from public API: the six `*_phi` builders, `PhiInst`/`FpPhiInst`/`PointerPhiInst` `Open`-state mutators (`add_incoming`/`finish`), and `phi_add_incoming_from_value`. Public phi authoring = slice 6's surface + slice 8's `insert_phi`. The public **read** surface (`PhiKind`, `incoming_count`, `incoming`, matchers) is unchanged.
- Mechanics: `pub(crate)` where possible; the asmparser is a separate crate, so the entry points it needs become `#[doc(hidden)] pub` in an explicitly-internal module (`llvmkit_ir::__asmparser_raw` or similar) documented as "internal contract for llvmkit-asmparser, may change without notice" — the standard Rust pattern for cross-crate internals.

> **Shipped as.** No `__asmparser_raw` module was introduced, and the demotion split two ways. The three **typed** raw builders — `int_phi`, `fp_phi`, `pointer_phi` — are `pub(crate)`, as is every phi-handle `add_incoming`; they are unnameable from outside the crate, which is what `raw_phi_builder_is_unnameable` pins. **Tightened at the 0.0.4 freeze:** all six carried `#[cfg_attr(not(test), allow(dead_code))]`, which violates the repo's unconditional `#[allow]` ban — their only callers were ever the `#[cfg(test)]` module `src/phi_raw_tests/`. They are now `#[cfg(test)]` outright, so a dependent crate's build does not contain them and the fixture asserts `E0599` "no method named" rather than `E0624` "private method" — a method compiled out cannot be reached by a later visibility slip, which a private one could. Their four **erased** counterparts — `int_phi_dyn`, `fp_phi_dyn`, `pointer_phi_in_addrspace`, `phi_dyn` — plus `phi_add_incoming_from_value` stayed on `IrBuilder` as `#[doc(hidden)] pub`, each carrying "internal contract shared with the in-tree `.ll` parser … block arguments are the public phi-authoring surface" in its own doc comment. So the public *supported* phi-authoring surface is block arguments, `SsaBuilder`, and `FnReshape::insert_phi`; the erased builders are a documented internal contract rather than a compile-time seal. The one crate-level re-export under that banner is the shared phi checker — `#[doc(hidden)] pub use phi_check::{PhiCoherenceError, check_function_phi_coherence}` — so the parser and the verifier run the same algorithm and cannot drift (slice 4).
- Migrate all in-tree tests/examples off the raw builders onto block-args; compile-fail fixture pinning that `int_phi` is unnameable publicly (E0603 was the guess; it shipped as **E0624**, "private method", and became **E0599**, "no method named", when the freeze made the builders `#[cfg(test)]`). CHANGELOG: major breaking entry with a short migration table (old call → new call).
- The `Open/Closed` typestate and its compile-fail fixtures remain (internal correctness + `PhiKind` rediscovery stays `Closed`).

### Slice 8 — Pass-side phi creation, dominance-witnessed

- `FnReshape::insert_phi_dyn(block: BlockId<Dyn, B>, ty, incomings: &[(ValueId<B>, BlockId<Dyn, B>)]) -> IrResult<ValueId<B>>` (this erased signature was named `insert_phi` at planning; cycle D added a typed `insert_phi<Id>` twin — taking and returning a typed id, with the result type derived from the first incoming — and moved the erased form to `insert_phi_dyn`): creates a phi at the block's phi head. A pass sees a **complete** CFG, so everything is witnessed at the call: completeness vs predecessors (slice-4 helper), types, duplicates, **and incoming-value dominance** (each value dominates its edge's source) via the pass context's dominator tree (`analysis_repaired::<DominatorTreeAnalysis>`, shipped in P4). Strikes the `FnReshape` "inserting PHIs is future work" note.
- Scope guard: phi-scoped insertion only; the general in-pass IrBuilder stays future work.

### Slice 9 — Edge ops with mandatory phi resolution

- `FnReshape::remove_edge(from, to)`: terminator surgery (exact shipped shape pinned at planning — e.g. drop one `switch`/`condbr` successor) + mechanical drop of `from`'s entries in `to`'s phis (LLVM `removePredecessor`, unforgettable). Records `CfgUpdate::delete`. Single-entry leftover phis are legal; slice-5 fold cleans them.
- `FnReshape::redirect_edge(from, old_to, new_to, phi_values: PhiValues)`: `old_to`'s phis lose entries mechanically; `new_to`'s phis **gain** entries whose values are a required, per-phi type-checked argument — "forgot the target's phis" doesn't typecheck. Records delete+insert `CfgUpdate`s.
- Consumers: in-crate tests now, SimplifyCFG later; fulfils slice 1's contract note.

> **Shipped as.** The two dynamic edge ops were replaced by a **typed terminator edit surface**, which is strictly stronger: `FnReshape::edit_terminator` returns a `TermEdit` enum, and the `edit_br` / `edit_cond_br` / `edit_switch` / `edit_invoke` / `edit_callbr` narrows return `BrEdit` / `CondBrEdit` / `SwitchEdit` / `InvokeEdit` / `CallBrEdit`. Each handle carries only the edits its opcode admits — `BrEdit` has `redirect` and no `remove_*` at all; `CondBrEdit` has `redirect_then` / `redirect_else` plus `remove_then` / `remove_else` that *consume* the handle (removing an arm collapses the `cond_br` to a `br` and deregisters the dead condition); `SwitchEdit` has `redirect_successor` / `redirect_default` / `remove_successor` but no default-removal; `InvokeEdit` and `CallBrEdit` are redirect-only, because both `invoke` edges and the `callbr` default are mandatory and the indirect count is fixed. So "you cannot remove that edge" is a *compile* error rather than a runtime rejection, pinned by `uncond_br_edit_has_no_remove`, `cond_br_edit_remove_consumes`, `switch_edit_has_no_remove_default`, `invoke_edit_has_no_remove` and `callbr_edit_has_no_remove`. The design's intent survives at the value level: every `redirect_*` takes `phi_values: &[ValueId<B>]` as a required argument, so the new target's phis cannot be forgotten, and each edit records its own `CfgUpdate`s.

### Slice 10 — Vector/aggregate phi support (parser bug fix)

- Parser accepts any first-class phi result type (fixes the rejection of valid LLVM IR in `ll_parser.rs::parse_phi`), routed through an internal erased `phi_dyn(ty)` + the (now-checked) internal add path. `OtherPhiInst` stays read-only classification; no new typed handle family (YAGNI). Round-trip tests: parse → print → parse for vector + aggregate phis.

> **Shipped as** designed, plus two verifier backstops the slice did not anticipate: `VerifierRule::PhiInvalidResultType` (a phi whose result is not a first-class data type — mirrors what the parser now enforces) and `VerifierRule::PhiEmptyInReachableBlock` (a zero-incoming phi in a block reachable from entry, gated on `DominatorTree::is_reachable_from_entry`; the shared incoming-count guard misses it on the `0 == 0` gap, and such a phi is un-round-trippable because upstream's `LLParser::parsePHI` rejects a bracket-less `%p = phi i32`).

---

## Docs & bookkeeping

- `docs/type-safety-vs-llvm.md`: rewrite the phi sections (the open/closed typestate account, the `PhiKind` rediscovery account, and the "verifier-only" boundary): public authoring is block-arguments (SIL-style) so desync/incompleteness are unrepresentable; internal paths are witnessed (type + duplicate checks, parse-time completeness); mutation carries the fix; `Module::verify()` remains the final gate (dominance, plus everything, for defense in depth).
- `pass-facing-type-safety.md`: add the phi package to the shipped list per wave. `CHANGELOG`: wave-1 entries (placement, new error variant, parser rejections) + wave-2 major breaking entry (raw phi authoring internal → block-args surface) with migration table.
- `docs/future-work.md`: strike "inserting PHIs is future work" (slice 8); keep the general in-pass builder note; record the indirectbr-to-param-block restriction.

## Testing / gates

- TDD per slice; the CI gates per slice commit (fmt, `check --examples`, clippy `-D warnings`, rustdoc `-D warnings`, `test --workspace --all-targets --all-features`, doctests, audit), run on the pinned toolchain. Bless touched `.stderr` fixtures on `cargo +1.96.0` and nowhere else; the baseline is **0 failures across every registered fixture** (83 while this wave was in flight; 87 at 0.0.4 — 86 `compile_fail` + 1 `pass`, since the count grows with every new type-level law). There is no "environmental" `.stderr` drift — that claim was investigated and disproved.
- Wave 1: split-over-phi-successor verify-pass; parser phi-after-non-phi rejection; parser forward-ref type mismatch fails at resolution; differing-duplicate rejection on all paths; parse-time completeness errors (bad `.ll` phi → parse error, source-located); uniform-phi fold incl. self-ref; ssa_builder full regression.
- Wave 2: SIL-style loop (header params, back-edge via `br_with_args`) builds + verifies clean; arity/type mismatch at branch site (`block_args_br_arity_mismatch_errors`, `block_args_br_type_mismatch_errors`); `int_phi` unnameable publicly (`raw_phi_builder_is_unnameable`); `insert_phi` dominance rejection (value defined below its edge); edge-removal entry-drop + `CfgUpdate` recording; vector/aggregate phi round-trip. The designed "`redirect_edge` without values doesn't typecheck" fixture became the five edit-handle fixtures instead (`uncond_br_edit_has_no_remove`, `cond_br_edit_remove_consumes`, `switch_edit_has_no_remove_default`, `invoke_edit_has_no_remove`, `callbr_edit_has_no_remove`) — phi values are a required argument of every `redirect_*`, so omitting them is an arity error rather than something a fixture needs to pin.

## Genuinely remaining open (with backstops)

- **Block arguments as the storage model** — authoring-only by decision: storage/parser/printer stay LLVM-phi-shaped for behavior parity, UPSTREAM test parity, and printed entry-order fidelity. Backstop: the authoring surface + mutation-carries-fix cover the same bug class.
- **General in-pass IrBuilder** — only phi-scoped insertion ships here. Backstop: documented future work. (Cycle D later added `FnPatch::builder_at` / `FnReshape::builder_at`, which *restore* a builder at a previously-saved `InsertPoint`; freshly positioning one inside `run()` is still not offered.)
- **Builder-time dominance** — undefined mid-construction; checked in pass contexts (slice 8) and by the verifier everywhere. Backstop: verifier + future Alive2-style `refines`.
