# llvmkit Roadmap

This roadmap is focused on making `llvmkit` a practical pure-Rust replacement for common `inkwell` usage in IR construction, binary-lifting-oriented IR cleanup, analysis-heavy deobfuscation, and controlled obfuscation pipelines. It is intentionally biased toward optimizations, value tracking, and pass infrastructure rather than code generation.

## Current baseline

Shipped today:

- Textual `.ll` lexer and constructive-subset parser.
- Typed IR model, constants, globals, functions, basic blocks, instructions, verifier, AsmWriter.
- Schema-typed IR construction: compile-checked calls (`build_call` +
  `TypedCallInst`), typed pointers (`TypedPointerValue` + compile-time field
  GEPs), typed folder hooks, and Braun-style auto-SSA (`SsaBuilder`).
- CFG and dominator-tree queries.
- Capability-graded module/function passes (Pass API v2): a pass declares a
  capability rung and the driver derives preservation and the output module's
  verified-state, so over-claiming what a pass preserves is a compile error.
- Built-in analyses: `DominatorTreeAnalysis`, `KnownBitsAnalysis`, and
  `DemandedBitsAnalysis`; initial `SimplifyDemandedBitsPass`.
- LLVM 22.1.4-style `ConstantFolder` for the modeled IR-builder surface plus
  target-independent pure-constant `ConstantFold.cpp` folds for represented
  `ConstantExpr`, integer/float, cast, compare, select, GEP, vector, and
  aggregate cases; DataLayout / TLI-heavy folds stay in analysis-only APIs.

Hard gaps for replacing more LLVM/Inkwell workflows:

- Constant folding outside the modeled target-independent builder surface is
  still partial: DataLayout / TLI / libcall / load-through-bitcast folds are
  represented only where the analysis APIs implement them, and InstSimplify-
  style nonconstant folds are still future transform work.
- KnownBits / ValueTracking is still a represented integer, pointer,
  fixed-vector, and intrinsic-fact subset; LLVM ValueTracking parity remains
  incomplete.
- No pass-builder/textual-pipeline surface.
- No loop PM / CGSCC PM.
- No alias analysis, MemorySSA, ScalarEvolution, LazyValueInfo, or post-dominance.
- No bitcode reader/writer.
- Metadata is parsed in places but instruction metadata propagation and full debug-info modeling are incomplete.
- Intrinsic coverage is not yet broad enough for arbitrary optimized or lifted IR.

## External workload reference: Mergen

Mergen (`https://github.com/NaC-L/Mergen`) is a useful reference workload because it lifts protected x64 PE functions into LLVM IR and then relies on LLVM optimization plus custom passes to recover readable semantics.

Relevant Mergen facts read for this roadmap:

- Its optimization loop runs `O1`, then custom passes, until instruction count stabilizes, then runs final `O2`.
- Custom passes include:
  - `GEPLoadPass`: fold concrete PE-image loads through `memory`-base GEPs.
  - `ReplaceTruncWithLoadPass`: rewrite `trunc(load wide, ptr)` to a narrow load on little-endian targets.
  - `PromotePseudoStackPass`: convert pseudo-memory stack-window accesses into real stack allocas/GEPs.
  - `PromotePseudoMemory`: convert remaining pseudo-memory GEPs into raw pointer operations.
- Its notes call out LLVM `computeKnownBits(Value*, DataLayout&)` as important and warn callers to guard integer/pointer types.
- Its loop handling depends on CFG shape recognition, PHI construction, possible-value enumeration through PHIs, and downstream optimization to simplify loop/generalized-state IR.

llvmkit does not need to copy Mergen. The actionable takeaway is that a practical lifter/deobfuscator now has a local ConstantFolder foundation, but still needs broader KnownBits, CFG/value analyses, memory-aware simplification, and repeatable optimization pipelines before it can stop depending on LLVM for cleanup.

---

## Roadmap priorities

| Priority | Area | Why it is first-class |
|---|---|---|
| P0 | ConstantFold / ConstantFolder parity maintenance and extension | Keep the shipped local simplifier aligned with LLVM as new modeled opcodes, types, and ConstantExpr forms land. |
| P0 | KnownBits / ValueTracking | Needed for opaque predicates, alignment, bit-mask simplification, flag recovery, indirect-branch reasoning. |
| P0 | Core scalar cleanup passes | Needed to replace the most common LLVM `O1` / `O2` cleanup wins after lifting. |
| P1 | Lifter-oriented memory/stack passes | Needed for pseudo-memory models, concrete image loads, stack promotion, load-width cleanup. |
| P1 | Alias/memory analyses | Required before memory optimizations are trustworthy. |
| P1 | PassBuilder-style pipeline API | Needed for ergonomic `"default<O1>"` / custom pipeline use. |
| P2 | Obfuscation passes | Useful once CFG/analysis/pipeline infrastructure is stable. |
| P2 | Loop / CGSCC PM | Needed for serious optimization composition. |
| P2 | Bitcode + richer metadata/intrinsics | Needed for broader LLVM ecosystem interop. |

---

## Milestone 1: Constant folding parity

### Status

Shipped for the modeled target-independent surface. The default
`ConstantFolder` mirrors LLVM 22.1.4 `ConstantFolder.h` hooks for all-constant
builder inputs, and `constant_fold.rs` ports pure-constant `ConstantFold.cpp`
behavior for represented integer/float binops, unary `fneg`, casts,
comparisons, `select`, GEP no-op / poison / undef cases, extract/insert value,
extract/insert element, `shufflevector`, vector splats/fixed vectors, and
aggregate constants.

Represented `ConstantExpr` construction/folding covers the parser-needed
add/sub/xor, GEP, vector, and cast forms, including upstream vector GEP,
bitcast, cast, and select fixtures.

Analysis-only behavior remains split out: DataLayout / TLI / libcall,
denormal, load-through-bitcast, and other target/library-dependent folds live in
`constant_folding.rs` where represented, not in the default builder folder.

### Remaining work

1. **APFloat and unmodeled-surface follow-up**
   - Close helper-operation gaps needed by future LLVM folding formulas,
     especially floating-point edge cases around NaNs, signed zero, infinities,
     rounding modes, and fast-math flags.
   - Keep conservative no-fold behavior where exact parity is not implemented.

2. **ConstantFold / ConstantExpr extension**
   - Extend parity only when new opcodes, types, or parser-needed expression
     forms become represented.
   - Keep LLVM 22.1.4 provenance tests current for every new fold.
   - Keep InstSimplify-style nonconstant identities (`x + 0`, `x * 1`,
     redundant casts, and similar transforms) in the optimization-pass roadmap,
     not in the all-constant default folder.

3. **Folder trait expansion**
   - Expand `IRBuilderFolder` hooks as new builder families need folding.
   - Keep `NoFolder` and allow custom folders.
   - Consider a `TargetFolder` only if builder-time DataLayout-dependent folds
     are intentionally exposed; today those folds remain in analysis folding
     APIs.

### Ongoing invariants

- Folding never panics on legal modeled IR inputs.
- Every fold cites the upstream LLVM folding entry point or test fixture.
- Builder tests cover folded and non-folded paths for every modeled opcode
  family.
- Unsupported folds have clear conservative fallback behavior.

---

## Milestone 2: KnownBits and ValueTracking

### Goal

Continue growing the already-landed KnownBits / ValueTracking subset against
LLVM 22.1.4 reference behavior, enabling only facts whose IR representation and
verifier dependencies are modeled.

### Shipped baseline

The current baseline already includes:

1. **KnownBits data model and core queries**
   - `KnownBits` with private known-zero / known-one `ApInt` masks and public mask
     accessors.
   - `compute_known_bits`, `KnownBitsAnalysis`, recursion budgeting,
     dominator-tree hooks, and conservative handling for unsupported value
     categories.
   - `ValueTrackingQuery` carries context instruction, demanded elements,
     instruction-info policy (`UseInstrInfo`), optional dominator tree, and a
     reusable per-analysis cache keyed by the query facts that affect precision.

2. **KnownBits transfer functions**
   - Constants, bitwise ops, direct `KnownBits.cpp` formula ports for add/sub
     carry/borrow, flagged add/sub, saturation add/sub overflow-direction
     clamps (`uadd.sat`, `usub.sat`, `sadd.sat`, `ssub.sat`), wide
     shifts, division/remainder, comparisons, `abs`, `sextInReg`,
     concat/extract, bit permutations, high-half multiply, averages, and
     reduction helpers.
   - Conservative or enumerated fallbacks remain only for other
     unsupported-by-representation cases while the parity ledger tracks the
     remaining direct ports.
   - ValueTracking already uses the shipped integer/pointer operator subset:
     signed div/rem, casts, select/phi/freeze/icmp, null pointer, alloca, and
     DataLayout-derived pointer low-zero-bit facts.

3. **Represented intrinsic subset and intrinsic facts**
   - The represented `llvm.*` intrinsic signature families are
     `llvm.assume(i1)`; integer or fixed-vector integer overloads of `abs`,
     `bswap`, `bitreverse`, `ctlz`, `cttz`, `ctpop`, `fshl`, `fshr`, `umax`,
     `umin`, `smax`, `smin`, `uadd.sat`, `usub.sat`, `sadd.sat`, and
     `ssub.sat`; fixed-vector `vector.reduce.add`; `ptrmask`; `vscale`; and
     the lifetime/memory/runtime helpers `lifetime.start`, `lifetime.end`,
     `memcpy`, `memmove`, `memset`, `trap`, `donothing`, `readcyclecounter`,
     `read_register.i64`, and `write_register.i64`.
   - ValueTracking computes known bits for the represented integer facts:
     `abs`, `bswap`, `bitreverse`, `ctlz`, `cttz`, `ctpop`, constant-amount
     `fshl`/`fshr`, `uadd.sat`, `usub.sat`, `sadd.sat`, `ssub.sat`,
     `umax`, `umin`, `smax`, `smin`, fixed-vector `vector.reduce.add`, and
     `ptrmask`; represented intrinsics outside that fact subset return
     unknown facts.
   - DemandedBits intrinsic operand masks are shipped for `bitreverse`,
     `bswap`, `ctlz`, `cttz`, constant-amount `fshl`/`fshr` source masks plus
     their shift-amount masks, and `umax`, `umin`, `smax`, and `smin`.

4. **Demanded-bits and SimplifyDemandedBits slice**
   - `DemandedBitsAnalysis` includes the represented scalar integer rules for
     add/sub/mul, bitwise ops, casts, select, extract/insert/shuffle vectors,
     constant shifts, known-range variable shifts, and the shipped intrinsic
     operand-mask subset listed above.
   - `SimplifyDemandedBitsPass` ships scalar-integer constant replacement,
     no-use dead instruction-chain erasure, and the upstream
     `assoc-cast-assoc.ll::AndZextAnd` demanded-mask transform
     (operand replacement, demanded-constant shrink, and `zext nneg` marking).

5. **APInt and DataLayout dependencies**
   - Wide `ApInt` arithmetic/comparison/shift/truncation/count helpers used by
     constants, folding, KnownBits, and demanded-bits code.
   - DataLayout pointer-size, pointer-alignment, type-size, and struct-layout
     accessors used by pointer and aggregate facts.

6. **Metadata constants and range facts**
   - `ConstantAsMetadata`-style typed constant operands such as `!{i64 1, i64 5}`
     are represented, parsed, and printed.
   - `ConstantRange` plus `!range` / `!absolute_symbol` verifier support match
     upstream `range-1.ll`, `range-2.ll`, and `absolute_symbol.ll` cases that
     llvmkit can represent today.
   - `ValueTracking.cpp::computeKnownBitsFromRangeMetadata` is ported for
     load/call/invoke range attachments, with malformed metadata producing
     unknown facts rather than panics.
   - Range attributes (`range(T lo, hi)`) are represented, parsed, printed for
     function/call return attributes, and used by call/invoke known-bits queries.
   - `returned` call/invoke arguments contribute known bits when the returned
     operand has the call result type.

7. **Structural value edges through metadata/debug records**
   - Reverse use-lists distinguish instruction operands, constant operands,
     typed metadata constants, and debug-record value operands.
   - `Value::users()` stays instruction-view-only while `num_uses` /
     `has_uses`, RAUW, and erase account for the non-instruction edges LLVM
     preserves.

8. **Analysis cache invalidation**
   - `KnownBitsAnalysisResult` reuses a per-result query cache and records a
     cached `DominatorTree` dependency when one is already available.
   - KnownBits invalidation follows new-PM preservation: IR changes invalidate
     unless KnownBits is preserved, and a captured dominator-tree dependency is
     invalidated unless dominator tree / CFG analyses are preserved.
   - `DemandedBits` invalidates unless the analysis or all function analyses are
     preserved.
   - Module-level function-analysis invalidation mirrors
     `FunctionAnalysisManagerModuleProxy::Result::invalidate`: clear cached
     function analyses when the proxy is not preserved, otherwise walk
     functions and honor each cached result's `PreservedAnalyses` decision.

### Remaining parity work

1. **KnownBits formula parity follow-up**
   - Replace the remaining conservative/enumerated fallbacks with direct
     `KnownBits.cpp` ports for facts not listed in the shipped baseline, where
     representation dependencies now exist and expanded ValueTracking users need
     additional facts.
   - Keep LLVM conflict-state behavior (`zero & one` may be non-zero internally)
     because upstream uses it for intersections and diagnostics.

2. **ValueTracking operator parity**
   - Port remaining `computeKnownBitsFromOperator` arms in upstream order for
     represented opcodes: and/or/xor refinements, additional call/callee
     attribute facts beyond the shipped range and `returned` cases, pointer
     alignment/GEP/cast facts, select edge facts, PHI recurrences,
     fixed-vector demanded-element facts, freeze poison checks, and additional
     intrinsic facts not listed in the shipped baseline above.

3. **Attribute and intrinsic dependencies**
   - Remaining intrinsic work means additional or currently unrepresented
     `llvm.*` IDs/signature families, or new facts beyond the shipped intrinsic
     facts above. Add IDs and verifier signatures before adding KnownBits or
     DemandedBits facts; unsupported ordinary functions stay unknown, and
     unsupported `llvm.*` intrinsics stay errors until represented.

4. **DemandedBits parity**
   - Extend demanded-bit rules only for additional intrinsic IDs/signatures or
     facts beyond the shipped `bitreverse` / `bswap` / `ctlz` / `cttz` /
     constant-amount `fshl` / `fshr` source masks, funnel-shift amount masks,
     and `umax` / `umin` / `smax` / `smin` operand-mask subset as those IDs,
     signatures, and facts land.
   - Add printer/display support only after the analysis facts are verified.

5. **SimplifyDemandedBits parity**
   - Continue growing the pass along
     `InstCombineSimplifyDemanded.cpp::SimplifyDemandedUseBits`, verifying
     printed IR and `Module::verify_borrowed()` for every transform.
   - Remaining near-term cases include high-bit binary constant shrink with flag
     repair and additional operand-return transforms that have direct upstream
     fixtures.

6. **Analysis invalidation follow-up**
   - Add explicit dependency checks for future analysis inputs as ValueTracking
     starts using them.
   - The outer-analysis invalidation registration path used by LLVM's
     `ModuleAnalysisManagerFunctionProxy` remains future work; do not claim
     full proxy parity until that surface exists.

7. **Parity ledger and provenance**
   - Track every upstream anchor needed to close the parity ledger across
     KnownBits, ValueTracking, DemandedBits, and SimplifyDemandedBits.
   - Every new test cites its upstream source in a doc comment and in
     `UPSTREAM.md`; roadmap wording must name the exact shipped subset and must
     not present the parity ledger as closed while incomplete rows remain.

### Binary-lifting/deobfuscation use cases

- Prove masked flags are zero/non-zero.
- Simplify opaque predicates.
- Infer pointer alignment from GEP/base masks.
- Collapse flag-materialization chains.
- Recognize constant or bounded indirect-branch targets.
- Prove trunc/zext/sext pairs preserve relevant bits.

### Acceptance criteria

- KnownBits handles all integer widths without panicking on legal IR inputs.
- It is safe on pointer values and non-integer values.
- It is deterministic, budgeted, and cache invalidation is explicit.
- Unit tests mirror LLVM `KnownBits`, `ValueTracking`, `DemandedBits`, and
  `InstCombineSimplifyDemanded` behavior for every represented fact.
- Unsupported-by-construction facts return unknown only while their
  representation dependency remains incomplete and tracked by the parity ledger.

---

## Milestone 3: Core scalar cleanup pipeline

### Goal

Provide enough optimization to replace common LLVM `O1` cleanup for lifted or generated IR.

### Passes to implement first

1. **InstSimplify**
   - Local algebraic simplifications that do not create new instructions.
   - Uses constant folder and KnownBits.

2. **InstCombine subset**
   - Canonicalize arithmetic/logical/cast patterns.
   - Fold redundant casts and mask chains.
   - Normalize compare/select idioms.
   - Prefer upstream-compatible canonical forms.

3. **DCE / ADCE**
   - Remove dead side-effect-free instructions.
   - Preserve terminators and side-effecting memory/atomic/call instructions.

4. **SimplifyCFG**
   - Remove unreachable blocks.
   - Merge trivial blocks.
   - Fold constant branches/switches.
   - Thread obvious branches where no PHI repair complexity is needed.

5. **SCCP**
   - Sparse conditional constant propagation over SSA values and executable blocks.
   - Use lattice values: unknown, constant, overdefined, poison/undef-aware states.

6. **EarlyCSE**
   - Common-subexpression elimination using dominator tree.
   - Initially only pure instructions; later memory-aware with MemorySSA.

7. **GVN subset**
   - Value-number pure expressions and simple load redundancies once alias analysis exists.

8. **BDCE / demanded bits**
   - Remove computations for bits that are never demanded.
   - Depends on KnownBits / demanded-bits engine.

### Default pipelines

Add named pipelines before trying to clone LLVM's full PassBuilder:

```text
cleanup-min = instsimplify,dce,simplifycfg
cleanup-lift = instcombine,simplifycfg,sccp,instcombine,dce,bdce,simplifycfg
cleanup-o1-ish = cleanup-lift,early-cse,gvn-lite,dce
```

### Acceptance criteria

- A lifted flag-heavy branch sample reduces to a short compare/branch or select.
- Constant branch and switch targets are eliminated.
- Repeated cleanup reaches a deterministic fixpoint.
- `cleanup-lift` can be assembled as a `module_pipeline((..))` (or a runtime `DynModulePipeline`) whose mutating members downgrade the result to `Module<Unverified>`, forcing an explicit re-`verify()`.

---

## Milestone 4: Lifter-oriented memory and stack cleanup passes

### Goal

Implement passes equivalent in spirit to the Mergen custom cleanup loop while keeping APIs generic and not tied to PE-only assumptions.

### Passes

1. **ConcreteImageLoadFoldPass**
   - Generalization of Mergen `GEPLoadPass`.
   - Inputs: a named memory parameter/root value, address map, endianness, byte provider.
   - Fold `load` from `gep i8, ptr %memory, constant_offset` into integer/aggregate constants when bytes are available and the load is non-volatile/non-atomic.

2. **NarrowLoadFromTruncPass**
   - Generalization of `ReplaceTruncWithLoadPass`.
   - Rewrite `trunc (load iN, ptr p)` to `load iM, ptr p` only when endianness, alignment, volatility, atomic ordering, and aliasing rules make it legal.
   - Little-endian first; big-endian requires offset adjustment.

3. **PseudoStackPromotionPass**
   - Generalization of `PromotePseudoStackPass`.
   - Inputs: memory root, stack base, stack window, stack alloca policy.
   - Convert pseudo-memory GEPs in stack window into `alloca`-backed GEPs.
   - Feed standard `mem2reg` / SROA later.

4. **PseudoMemoryLoweringPass**
   - Generalization of `PromotePseudoMemory`.
   - Convert leftover pseudo-memory GEPs into `inttoptr` or explicit memory intrinsics according to a configured memory model.
   - Must run after concrete load folding and stack promotion.

5. **FlagCanonicalizationPass**
   - Simplify CPU flag materialization idioms: parity/sign/zero/carry/overflow chains.
   - Use KnownBits and InstCombine.
   - Keep architecture-specific flag semantics in a separate module so x86 does not leak into generic IR APIs.

6. **JumpTargetRecoveryPass**
   - Use constant folder, KnownBits, possible-values PHI/select enumeration, and concrete image reads to recover indirect branch/switch targets.
   - Emit structured `switch` where a bounded set of targets is known.

### Acceptance criteria

- Pass order is explicit: concrete loads before pseudo-memory lowering.
- Passes are configurable and not hardcoded to one binary format.
- Each pass returns precise `PreservedAnalyses`.
- Golden IR tests cover before/after outputs.

---

## Milestone 5: Memory, alias, and dependence analyses

### Goal

Make memory transforms safe enough for optimization and lifting cleanup.

### Analyses

1. **BasicAliasAnalysis**
   - Distinguish allocas, globals, function arguments, GEP-derived locations, disjoint stack slots.
   - DataLayout-aware offset reasoning.

2. **MemoryLocation model**
   - Pointer value, size, alignment, volatility/atomic flags, invariant groups when modeled.

3. **MemorySSA**
   - Track memory definitions/uses for loads/stores/calls.
   - Enable load CSE, DSE, and memory-aware GVN.

4. **Dependence / ModRef basics**
   - Classify calls and memory ops conservatively.
   - Add attributes and function effects into the model.

5. **PostDominatorTree**
   - Needed for ADCE, control dependence, and some obfuscation/deobfuscation transforms.

### Acceptance criteria

- No memory optimization runs without an alias result or explicit conservative fallback.
- Volatile/atomic operations block unsafe rewrites.
- Load/store simplifications have negative tests for aliasing hazards.

---

## Milestone 6: Promote memory to SSA: mem2reg and SROA

### Goal

Recover readable SSA from stack-heavy or pseudo-stack-heavy lifted IR.

### Work items

1. **PromoteMemToReg**
   - Port LLVM mem2reg for promotable allocas.
   - Requires dominator tree and dominance-frontier/IDF support.
   - Insert PHIs, rewrite loads/stores, erase dead allocas.

2. **SROA subset**
   - Split aggregate allocas into scalar allocas.
   - Handle integer/array/struct slices relevant to lifted stack memory.
   - Preserve alignment and DataLayout correctness.

3. **Alloca canonicalization**
   - Move static allocas to entry where legal.
   - Normalize alloca naming and alignment.

### Acceptance criteria

- Pseudo-stack promotion feeds mem2reg and yields SSA scalars.
- PHI insertion is deterministic.
- Verifier passes after every promotion test.

---

## Milestone 7: Loop analyses and loop transforms

### Goal

Support real cleanup of loop-heavy lifted IR and prepare for higher-level optimization.

### Analyses

- `LoopInfo`.
- `LoopSimplify` form checker.
- `LCSSA` checker / transformer.
- `ScalarEvolution` subset for affine induction variables.
- Backedge-taken count where provable.

### Transforms

- Loop simplify.
- LCSSA.
- LICM with alias checks.
- IndVarSimplify subset.
- Simple loop deletion when trip count is zero or body is side-effect-free.
- Optional loop unroll for small constant trip counts.

### Acceptance criteria

- Canonical loop tests ported from LLVM.
- Lifter-style dispatcher loops can be analyzed without exponential recursion.
- Loop passes compose with `cleanup-lift` without invalid analysis reuse.

---

## Milestone 8: PassBuilder and textual pipelines

### Goal

Make optimization UX close enough to LLVM/Inkwell users.

### API shape

```rust
let mut pb = PassBuilder::new();
let mut mpm = pb.parse_module_pipeline("cleanup-lift,instcombine,simplifycfg")?;
let unverified = mpm.run(module.verify()?, &mut mam, &mut fam)?;
let verified = unverified.verify()?;
```

Named pipelines:

- `llvmkit-default<O0>`: verifier-only / no-op cleanup. (llvmkit-specific
  subset, deliberately named apart from upstream's non-empty `default<O0>`.)
- `llvmkit-default<O1>`: conservative scalar cleanup.
- `llvmkit-default<O2>`: stronger scalar + memory + loop cleanup as available.
- `cleanup-lift`: binary-lifting/deobfuscation-biased cleanup.
- `obfuscate<...>`: obfuscation pipeline once Milestone 10 lands.

### Acceptance criteria

- Unknown pass names produce useful diagnostics.
- Pipeline parser has tests for nesting and options.
- Read-only vs mutating pipeline effects remain visible in types.

---

## Milestone 9: Inkwell replacement completeness

### Goal

Cover APIs commonly used by Rust projects that currently depend on Inkwell for IR generation and optimization setup.

### Work items

- Builder coverage for remaining common LLVM IR operations and intrinsics.
- Full metadata attachment storage and printing.
- Debug-info model sufficient to preserve parsed debug metadata conservatively.
- Inline asm parity where textual IR accepts it.
- Intrinsic declaration and overloaded intrinsic typing.
- Attribute groups and function/callsite attribute APIs.
- Bitcode reader/writer or an explicit bridge plan if bitcode stays out longer.
- Stable `Module` parse/print entry points with ergonomic `from_str` / `from_path` helpers that still preserve generative branding.
- Better error spans and diagnostics for parser/verifier failures.

### UX goals

- Keep typed APIs for correctness.
- Provide ergonomic helpers for common cases so users do not need to write turbofish-heavy code for simple functions.
- Keep `Dyn` fallbacks for parsed or runtime-known IR.
- Document the safe path and the expert path separately.

---

## Milestone 10: Obfuscation passes inspired by O-LLVM

### Goal

Add opt-in obfuscation transforms that can be used for testing deobfuscators and controlled IR hardening.

### Passes

1. **BasicBlockSplitPass**
   - Split blocks at safe instruction boundaries.
   - Preserve PHI/terminator correctness.

2. **InstructionSubstitutionPass**
   - Replace arithmetic/logical ops with equivalent instruction sequences.
   - Examples: `x + y` via `xor/and/shl` identities, `sub` via add/neg, boolean rewrites.
   - Must be width-aware and poison/overflow-flag-aware.

3. **BogusControlFlowPass**
   - Add opaque predicate branches and cloned/dead blocks.
   - Requires side-effect and dominance safety.

4. **ControlFlowFlatteningPass**
   - Dispatcher loop with state variable and switch.
   - Must have verifier-backed PHI repair and deterministic naming.

5. **OpaquePredicatePass**
   - Generate predicates whose result is known by construction but hard for simple syntactic analysis.
   - Should integrate with KnownBits/range analysis to avoid accidentally generating trivially folded predicates unless requested.

6. **String/DataObfuscationPass**
   - Encrypt constant byte arrays/globals and inject decode stubs.
   - Requires global initializers, function insertion, and metadata/attribute care.

7. **AntiSimplify mode**
   - Optional mode to produce patterns resistant to the crate's own `cleanup-lift` pipeline for testing.
   - Must be disabled by default.

### Safety and ethics guardrails

- Obfuscation passes are opt-in and never part of default optimization pipelines.
- Every pass must preserve verifier correctness.
- Tests should include both obfuscation output shape and deobfuscation cleanup behavior where appropriate.

---

## Milestone 11: Deobfuscation analyses and transforms

### Goal

Provide analysis-heavy passes specifically useful for recovering readable IR from obfuscated or lifted code.

### Analyses

- Opaque predicate detection using KnownBits, integer ranges, SCCP, and demanded bits.
- Possible-values analysis for small PHIs/selects and jump targets.
- Value-set analysis for bounded integer domains.
- Control-dependence graph from post-dominators.
- Region/structural CFG analysis.
- Dispatcher-loop detection.
- Stack/memory object recovery.

### Transforms

- Opaque predicate removal.
- Dead bogus-control-flow removal.
- Dispatcher switch recovery.
- Jump-table normalization.
- Flag-chain simplification.
- Arithmetic MBA simplification subset.
- Stack slot recovery and scalarization.

### Acceptance criteria

- Each transform has adversarial negative tests: no rewrite when proof is insufficient.
- Analysis results are inspectable for diagnostics, not just consumed internally.
- Pipelines can emit a before/after simplification report.

---

## Milestone 12: IR compatibility and ecosystem interop

### Goal

Widen from controlled textual IR to broader LLVM ecosystem compatibility.

### Work items

- Bitcode reader/writer.
- More complete metadata/debug-info round-trip.
- Broader intrinsic modeling.
- Target-library-info-like hooks where transforms need libc semantics.
- DataLayout parity hardening.
- `llvm-dis` / `llvm-as` textual parity fixture expansion where external tools are available manually.

### Acceptance criteria

- Representative `.ll` from Clang/Rust optimized output parses or fails with precise unsupported-feature diagnostics.
- Round-trip preserves unknown metadata conservatively.
- Unsupported constructs are never silently dropped.

---

## Suggested release sequence

### 0.1: Folding and ValueTracking foundation

- ConstantFolder / ConstantFold parity foundation for the modeled IR surface.
- ValueTracking hardening required by initial cleanup passes.
- InstSimplify + DCE.

### 0.2: Lifting cleanup pipeline

- InstCombine subset.
- SimplifyCFG.
- SCCP.
- ConcreteImageLoadFoldPass.
- NarrowLoadFromTruncPass.
- PseudoStackPromotionPass.
- `cleanup-lift` named pipeline.

### 0.3: Memory and SSA promotion

- BasicAA.
- MemoryLocation.
- Mem2Reg.
- SROA subset.
- EarlyCSE.
- GVN-lite.

### 0.4: Loop and stronger analysis

- LoopInfo.
- PostDominatorTree.
- ScalarEvolution subset.
- LICM subset.
- ADCE / BDCE.
- Opaque predicate detection.

### 0.5: Obfuscation and deobfuscation suite

- Basic-block splitting.
- Instruction substitution.
- Bogus control flow.
- Control-flow flattening.
- Opaque predicate generation/removal.
- Dispatcher/jump-table recovery improvements.

### 0.6+: Ecosystem compatibility

- Bitcode.
- Debug metadata preservation.
- Broader intrinsics.
- Textual PassBuilder compatibility.
- Larger upstream fixture corpus.

---

## Non-negotiable engineering rules

- New optimization transforms must layer on the shipped ConstantFolder and KnownBits foundations, and must not assume unshipped full ValueTracking, InstCombine, or PassBuilder parity.
- Do not add memory transforms without alias/memory safety checks or conservative refusal paths.
- Do not make obfuscation passes part of default optimization pipelines.
- Every optimization must preserve verifier correctness.
- Every pass must return accurate `PreservedAnalyses`.
- Every new analysis/transform needs upstream-provenance tests where LLVM has coverage, plus llvmkit-specific tests for typestate and Rust-only APIs.
- Prefer conservative no-op over an unsound rewrite.

## References

- Mergen: `https://github.com/NaC-L/Mergen`
- Mergen architecture: `https://github.com/NaC-L/Mergen/blob/main/ARCHITECTURE.md`
- Mergen LLVM API notes: `https://github.com/NaC-L/Mergen/blob/main/LLVM_API_NOTES.md`
- Mergen scope: `https://github.com/NaC-L/Mergen/blob/main/docs/SCOPE.md`
- Mergen loop handling: `https://github.com/NaC-L/Mergen/blob/main/docs/LOOP_HANDLING.md`
- LLVM ValueTracking / KnownBits: `llvm/include/llvm/Analysis/ValueTracking.h`, `llvm/include/llvm/Support/KnownBits.h`, `llvm/lib/Support/KnownBits.cpp`
- LLVM constant folding: `llvm/include/llvm/IR/ConstantFolder.h`, `llvm/lib/Analysis/ConstantFolding.cpp`, `llvm/lib/IR/ConstantFold.cpp`
- LLVM scalar transforms: `llvm/lib/Transforms/InstCombine`, `llvm/lib/Transforms/Scalar`, `llvm/include/llvm/Transforms/Scalar`
