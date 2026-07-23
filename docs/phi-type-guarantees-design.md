# Phi Type-Level Guarantees — Design Spec (final: block-args-first, breaking OK)

Date: 2026-07-10. Target: `crates/llvmkit-ir` + `crates/llvmkit-asmparser`. Two waves: `feature-11/phi-guarantees` (slices 1–5) then `feature-12/phi-block-args` (slices 6–10), each off `dev`, merged `--no-ff` back, gates green per slice. Once approved, this spec is copied to `docs/superpowers/specs/2026-07-10-phi-type-guarantees-design.md` and committed first. Execution: subagent-driven development per established workflow.

## Context

Phi nodes are the most bug-prone IR feature in LLVM-family compilers. Research-confirmed failure classes: (1) predecessor↔incoming desync after CFG edits (LLVM's manual `removePredecessor` is the forgettable step; real miscompiles), (2) incoming-type mismatch (LLVM #54831/#2361), (3) duplicate entries for one block with different values (InstCombine #196954), (4) phis not grouped at block top. MLIR and Cranelift eliminated the class with block arguments; Swift SIL uses block arguments and **lowers them to LLVM phis in IRGen** — proving the authoring-layer-over-phi-storage shape at production scale. Cranelift's `seal_block` ("all predecessors known") makes phi completeness locally checkable during construction.

llvmkit today (explorer-verified): solid `Open/Closed` typestate on typed phi handles; typed `add_incoming` checks value type at runtime; but the untyped `IRBuilder::phi_add_incoming_from_value` (parser + `ssa_builder` path) checks nothing until `verify()`; `build_*_phi` inserts at the cursor (misplacement possible); no predecessor-mutation/phi-fixup API exists; and **`FnReshape::split_block` leaves successor phis naming the stale predecessor — a correct pass produces IR failing `verify()` (live bug)**. The parser also **rejects valid LLVM IR** (`phi <4 x i32>` — "phi result type must be int, float, or pointer", `ll_parser.rs:7328`). `docs/type-safety-vs-llvm.md:799-810` declares phi/pred coherence "intentionally verifier-only" — this spec revises that boundary.

**Decisions locked with user (in order):** full sweep; CFG strategy = mutation carries the fix; maximum construction strictness; close the gaps (Cranelift sealing + SIL block-args research); **breaking changes are fine — go as far as keeping LLVM behavior parity allows, without reinventing** → block arguments become the ONLY public authoring surface; phi storage, parser, printer, and verifier semantics stay LLVM-shaped; the raw phi authoring APIs go internal.

Doctrine: unrepresentable > witnessed > tested, never trusted.

---

## Wave 1 — `feature-11/phi-guarantees` (fix + harden the existing machinery)

### Slice 1 — Fix the live bug: `split_block` carries the phi fix

- `FnReshape::split_block` (`pass_context.rs:893-925`): the terminator moves from `source` to `new_block`, so each `source→succ` edge becomes `new_block→succ`. For every phi in every successor, rewrite incoming entries whose block == `source` to `new_block` (`PhiData.incoming` second slot, `instr_types.rs:694-699`). Mechanical, no author input — desync unrepresentable for this op.
- TDD: failing test first (split over a phi-successor; today `verify()` fails `PhiPredecessorMismatch`; after: passes, incoming block is `new_block`, value unchanged).
- Doc-comment the `FnReshape` contract: every structural op maintains phis in the blocks it touches; ops that make phis *gain* entries take the values as typed arguments (fulfilled by slice 9).

### Slice 2 — Placement unrepresentable at build time

- All six `build_*_phi` builders (`ir_builder.rs:5572-5690`) insert at the block's **phi head** (after existing phis, before first non-phi) via a new crate-private `insert_phi_at_head`. Cursor position becomes irrelevant to phi placement. (`ssa_builder::emit_operandless_phi` already head-inserts — converges.)
- **Parser duty:** auto-hoisting builders would silently reorder ill-formed `.ll`; the parser (`ll_parser.rs::parse_phi` ~7299) must instead track "seen non-phi in current block" and reject with a parse error. Fixture test.
- Verifier `PhiNotAtTop` stays (defense in depth).

### Slice 3 — Every edge-adding path checked

- `IRBuilder::phi_add_incoming_from_value` (`ir_builder.rs:507-547`) gains the type check (`value.ty == phi.ty` → `IrError::TypeMismatch`). Parser immediate edges, parser forward-ref resolution (`ll_parser.rs:9218` — value exists by then), and `ssa_builder::phi_add_incoming_raw` (`ssa_builder.rs:1654-1663`) inherit it. (Braun reads are same-typed — belt-and-braces, tested.)
- **Differing-duplicate rejection** (InstCombine #196954 class): all add paths (typed `add_incoming` ×3 + `phi_add_incoming_from_value`) reject a second entry for the same block with a different value → new `IrError::AmbiguousPhiIncoming`. Same block + same value stays legal (switch multi-edge). Verifier `AmbiguousPhi` stays.
- Update the `ssa_builder.rs:1496-1521` invariant comment.

### Slice 4 — Shared `check_phi` helper + parse-time completeness (internal sealing)

- Extract the verifier's per-phi logic (`verifier.rs:2395-2487`: count == preds with multiplicity, membership, multiplicity cap, differing-duplicate) into one shared crate helper used by the verifier, the parser, and later slices 8–9 — single source of truth, cannot drift.
- **Parser seals after each function**: once a function's body is parsed, all predecessors are known (Cranelift's insight), so the parser runs the helper over every phi and reports failures as source-located parse errors instead of distant `verify()` failures.
- No *public* seal API: after slice 7 the public surface cannot create incomplete phis at all, so user-facing sealing has nothing to check (YAGNI). `ssa_builder` keeps its own Braun sealing.

### Slice 5 — Pass-facing surface

- `matchers.rs`: `m_phi()` binding `PhiKind`; composes with existing combinators.
- `inst_simplify.rs`: uniform-phi fold (all incomings the same value `v`, self-references allowed) → replace with `v` via the existing worklist/RAUW machinery (user-cascade already pushes former users). Mirrors LLVM `SimplifyPHINode`'s conservative core. `scalar_cleanup` corpus diffs are intentional new folds, hand-reviewed.
- DCE: no change (unused phi already eligible; self-kept cycles out of scope, as upstream).

---

## Wave 2 — `feature-12/phi-block-args` (the breaking flip + remaining closures)

### Slice 6 — Block-argument authoring surface (SIL model)

- `append_block_with_params(fn, [types]) -> (BasicBlockLabel, params)` — each param is a `Value` backed by an operandless phi at the block's head (storage stays `PhiData`).
- `build_br_with_args(target, [values])`, `build_cond_br_with_args(...)`, `build_switch_with_args(...)` (+ an invoke variant if the builder ships `build_invoke` — pin at planning): append the terminator **and** add each value as an incoming from the current block in one call, arity- and type-checked at the call site. Edge and values move together — desync and incompleteness are structural non-events.
- Plain `build_br`/`build_cond_br` to a param-block → immediate arity error. `indirectbr` to a param-block → error (documented restriction; classic phi-with-indirectbr still parses from `.ll`).
- Entry block takes no params (function arguments serve that role, as in MLIR/SIL).
- Storage, parser, printer, verifier: unchanged. Printed IR is ordinary phis.

### Slice 7 — THE BREAK: raw phi authoring goes internal

- Demoted from public API: the six `build_*_phi` builders, `PhiInst`/`FpPhiInst`/`PointerPhiInst` `Open`-state mutators (`add_incoming`/`finish`), and `phi_add_incoming_from_value`. Public phi authoring = slice 6's surface + slice 8's `insert_phi`. The public **read** surface (`PhiKind`, `incoming_count`, `incoming`, matchers) is unchanged.
- Mechanics: `pub(crate)` where possible; the asmparser is a separate crate, so the entry points it needs become `#[doc(hidden)] pub` in an explicitly-internal module (`llvmkit_ir::__asmparser_raw` or similar) documented as "internal contract for llvmkit-asmparser, may change without notice" — the standard Rust pattern for cross-crate internals.
- Migrate all in-tree tests/examples off the raw builders onto block-args; compile-fail fixture pinning that `build_int_phi` is unnameable publicly (E0603). CHANGELOG: major breaking entry with a short migration table (old call → new call).
- The `Open/Closed` typestate and its compile-fail fixtures remain (internal correctness + `PhiKind` rediscovery stays `Closed`).

### Slice 8 — Pass-side phi creation, dominance-witnessed

- `FnReshape::insert_phi_dyn(block, ty, incomings) -> IrResult<...>` (this erased signature was named `insert_phi` at planning; cycle D added a typed `insert_phi<V>` twin and moved the erased form to `insert_phi_dyn`): creates a phi at the block's phi head. A pass sees a **complete** CFG, so everything is witnessed at the call: completeness vs predecessors (slice-4 helper), types, duplicates, **and incoming-value dominance** (each value dominates its edge's source) via the pass context's dominator tree (`analysis_repaired::<DominatorTreeAnalysis>`, shipped in P4). Strikes the `FnReshape` "inserting PHIs is future work" note.
- Scope guard: phi-scoped insertion only; the general in-pass IRBuilder stays future work.

### Slice 9 — Edge ops with mandatory phi resolution

- `FnReshape::remove_edge(from, to)`: terminator surgery (exact shipped shape pinned at planning — e.g. drop one `switch`/`condbr` successor) + mechanical drop of `from`'s entries in `to`'s phis (LLVM `removePredecessor`, unforgettable). Records `CfgUpdate::delete`. Single-entry leftover phis are legal; slice-5 fold cleans them.
- `FnReshape::redirect_edge(from, old_to, new_to, phi_values: PhiValues)`: `old_to`'s phis lose entries mechanically; `new_to`'s phis **gain** entries whose values are a required, per-phi type-checked argument — "forgot the target's phis" doesn't typecheck. Records delete+insert `CfgUpdate`s.
- Consumers: in-crate tests now, SimplifyCFG later; fulfils slice 1's contract note.

### Slice 10 — Vector/aggregate phi support (parser bug fix)

- Parser accepts any first-class phi result type (fixes rejection of valid LLVM IR at `ll_parser.rs:7328`), routed through an internal erased `build_phi_dyn(ty)` + the (now-checked) internal add path. `OtherPhiInst` stays read-only classification; no new typed handle family (YAGNI). Round-trip tests: parse → print → parse for vector + aggregate phis.

---

## Docs & bookkeeping

- `docs/type-safety-vs-llvm.md`: rewrite the phi sections (§499-540 open/closed, §785-789 PhiKind, §799-810 "verifier-only" boundary): public authoring is block-arguments (SIL-style) so desync/incompleteness are unrepresentable; internal paths are witnessed (type + duplicate checks, parse-time completeness); mutation carries the fix; `Module::verify()` remains the final gate (dominance, plus everything, for defense in depth).
- `docs/pass-facing-type-safety.md`: add the phi package to the shipped list per wave. `CHANGELOG`: wave-1 entries (placement, new error variant, parser rejections) + wave-2 major breaking entry (raw phi authoring internal → block-args surface) with migration table.
- `docs/future-work.md`: strike "inserting PHIs is future work" (slice 8); keep the general in-pass builder note; record the indirectbr-to-param-block restriction.

## Testing / gates

- TDD per slice; 5 CI gates per slice commit (fmt, clippy `-D warnings`, rustdoc `-D warnings`, `test --workspace --all-features`, audit). trybuild `.stderr` blessing caveat for touched compile-fail fixtures.
- Wave 1: split-over-phi-successor verify-pass; parser phi-after-non-phi rejection; parser forward-ref type mismatch fails at resolution; differing-duplicate rejection on all paths; parse-time completeness errors (bad `.ll` phi → parse error, source-located); uniform-phi fold incl. self-ref; ssa_builder full regression.
- Wave 2: SIL-style loop (header params, back-edge via `br_with_args`) builds + verifies clean; arity/type mismatch at branch site; plain `br` to param-block rejected; `build_int_phi` unnameable publicly (compile-fail); `insert_phi` dominance rejection (value defined below its edge); `remove_edge` entry-drop + `CfgUpdate` recording; `redirect_edge` without values doesn't typecheck (fixture); vector/aggregate phi round-trip.

## Genuinely remaining open (with backstops)

- **Block arguments as the storage model** — authoring-only by decision: storage/parser/printer stay LLVM-phi-shaped for behavior parity, UPSTREAM test parity, and printed entry-order fidelity. Backstop: the authoring surface + mutation-carries-fix cover the same bug class.
- **General in-pass IRBuilder** — only phi-scoped insertion ships. Backstop: documented future work.
- **Builder-time dominance** — undefined mid-construction; checked in pass contexts (slice 8) and by the verifier everywhere. Backstop: verifier + future Alive2-style `refines`.
