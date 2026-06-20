# llvmkit Roadmap

This roadmap is focused on making `llvmkit` a practical pure-Rust replacement for common `inkwell` usage in IR construction, binary-lifting-oriented IR cleanup, analysis-heavy deobfuscation, and controlled obfuscation pipelines. It is intentionally biased toward optimizations, value tracking, and pass infrastructure rather than code generation.

## Current baseline

Shipped today:

- Textual `.ll` lexer and constructive-subset parser.
- Typed IR model, constants, globals, functions, basic blocks, instructions, verifier, AsmWriter.
- CFG and dominator-tree queries.
- Effect-typed module/function pass managers.
- One built-in analysis: `DominatorTreeAnalysis`.
- A minimal `ConstantFolder` that only folds integer `add` / `sub` / `mul` for constants representable through the current `u128` helper path.

Hard gaps for replacing more LLVM/Inkwell workflows:

- No real LLVM-equivalent constant folder.
- No KnownBits / ValueTracking layer.
- No canonical scalar optimization pipeline (`instcombine`, `simplifycfg`, SCCP, GVN, DCE, etc.).
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

llvmkit does not need to copy Mergen. The actionable takeaway is that a practical lifter/deobfuscator needs constant folding, KnownBits, CFG/value analyses, memory-aware simplification, and repeatable optimization pipelines before it can stop depending on LLVM for cleanup.

---

## Roadmap priorities

| Priority | Area | Why it is first-class |
|---|---|---|
| P0 | Real constant folder | Without it, builder output stays noisy and optimization passes have no cheap local simplifier. |
| P0 | KnownBits / ValueTracking | Needed for opaque predicates, alignment, bit-mask simplification, flag recovery, indirect-branch reasoning. |
| P0 | Core scalar cleanup passes | Needed to replace the most common LLVM `O1` / `O2` cleanup wins after lifting. |
| P1 | Lifter-oriented memory/stack passes | Needed for pseudo-memory models, concrete image loads, stack promotion, load-width cleanup. |
| P1 | Alias/memory analyses | Required before memory optimizations are trustworthy. |
| P1 | PassBuilder-style pipeline API | Needed for ergonomic `"default<O1>"` / custom pipeline use. |
| P2 | Obfuscation passes | Useful once CFG/analysis/pipeline infrastructure is stable. |
| P2 | Loop / CGSCC PM | Needed for serious optimization composition. |
| P2 | Bitcode + richer metadata/intrinsics | Needed for broader LLVM ecosystem interop. |

---

## Milestone 1: APInt/APFloat foundations and real constant folding

### Goal

Replace the minimal `ConstantFolder` with a trustworthy LLVM-style folding layer.

### Work items

1. **Arbitrary-width integer core**
   - Add an `APInt`-equivalent representation for all LLVM integer widths, not just widths that fit in `u128`.
   - Implement wrapping arithmetic, signed/unsigned comparisons, shifts, rotates, population/count helpers, masks, trunc/zext/sext, and bit slicing.
   - Use it under constants, folder, KnownBits, integer range analysis, and DataLayout-sized calculations.

2. **Floating-point constant core**
   - Add an `APFloat`-equivalent layer or a narrow internal model sufficient for `half`, `bfloat`, `float`, `double`, `fp128`, `x86_fp80`, and `ppc_fp128` folding.
   - Respect LLVM semantics for NaNs, signed zero, infinities, rounding, and fast-math flags.
   - Keep conservative no-fold behavior where exact parity is not implemented.

3. **ConstantFolder parity layer**
   - Mirror LLVM `ConstantFolder`, `ConstantFoldInstruction`, and `ConstantFoldConstant` behavior for the modeled opcode set.
   - Fold integer/float binops, unary ops, casts, comparisons, `select`, `freeze`, extract/insert value, extract/insert element, shufflevector where legal, GEP constant expressions, and simple aggregate constants.
   - Fold identities with one constant operand where LLVM does: `x + 0`, `x * 1`, `x & -1`, `x | 0`, `xor x, 0`, shifts by zero, double negation, redundant casts.
   - Make DataLayout-aware folds explicit: pointer-size casts, GEP offsets, alignment-derived facts, target extension types.

4. **Folder trait expansion**
   - Expand `IRBuilderFolder` beyond `fold_int_add/sub/mul` into a broad trait matching builder families.
   - Keep `NoFolder` and allow custom folders.
   - Add `TargetFolder` later if DataLayout-dependent folds should be separated from target-independent folds.

### Acceptance criteria

- The folder handles constants wider than 128 bits.
- Folding never panics on legal IR inputs.
- Every fold cites the upstream LLVM folding entry point or test fixture.
- Builder tests cover folded and non-folded paths for every modeled opcode family.
- There is a clear conservative fallback for each unsupported fold.

---

## Milestone 2: KnownBits and ValueTracking

### Goal

Implement a reusable `KnownBits` analysis equivalent to LLVM's ValueTracking core and expose it as a pass-manager analysis.

### Public API shape

```rust
pub struct KnownBits {
    pub zero: APInt,
    pub one: APInt,
}

pub struct KnownBitsAnalysis;

impl KnownBits {
    pub fn unknown(width: u32) -> Self;
    pub fn from_constant(value: ConstantIntValue<'_, IntDyn>) -> Self;
    pub fn is_known_zero(&self, bit: u32) -> bool;
    pub fn is_known_one(&self, bit: u32) -> bool;
    pub fn count_min_trailing_zeros(&self) -> u32;
    pub fn count_min_leading_zeros(&self) -> u32;
}
```

High-level query API:

```rust
pub fn compute_known_bits<'ctx, B>(
    value: Value<'ctx, B>,
    data_layout: &DataLayout,
    ctx: &KnownBitsContext<'_, 'ctx, B>,
) -> IrResult<KnownBits>;
```

### Work items

1. **KnownBits data model**
   - Store known-zero and known-one masks as `APInt`.
   - Enforce `zero & one == 0`.
   - Support integer and pointer values; reject or return unknown for unsupported categories without panics.

2. **Transfer functions**
   - Constants, `undef`, `poison`, `freeze`.
   - `and`, `or`, `xor`, `not`.
   - `add`, `sub`, `mul` conservative known-bit transfer.
   - `shl`, `lshr`, `ashr`.
   - `trunc`, `zext`, `sext`, `bitcast` where bit-preserving.
   - `select` as intersection of arms.
   - PHI as meet over incoming values, with recursion budget.
   - `icmp` result known width/value where trivially constant.
   - GEP/pointer alignment low-bit facts from DataLayout and align attributes.

3. **Query context**
   - Recursion depth budget.
   - Visited set to avoid cycles.
   - Optional demanded-bits mask.
   - DataLayout access.
   - Per-function cache integrated with `FunctionAnalysisManager`.

4. **Dependent utilities**
   - `SimplifyDemandedBits` equivalent.
   - `isKnownNonZero` / `isKnownZero` / `isKnownOne` helpers.
   - Alignment inference helper for pointers.
   - Possible-values enumeration for small PHIs/selects to support branch and jump-table recovery.

### Binary-lifting/deobfuscation use cases

- Prove masked flags are zero/non-zero.
- Simplify opaque predicates.
- Infer pointer alignment from GEP/base masks.
- Collapse flag-materialization chains.
- Recognize constant or bounded indirect-branch targets.
- Prove trunc/zext/sext pairs preserve relevant bits.

### Acceptance criteria

- KnownBits handles all integer widths.
- It is safe on pointer values and non-integer values.
- It is deterministic and budgeted.
- Unit tests mirror LLVM `KnownBits` / `ValueTracking` behavior for bitwise ops, shifts, casts, PHIs, selects, and pointers.
- It powers at least one real simplification pass in Milestone 3.

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
- `cleanup-lift` can be run from a typed `ModulePassManager<MutatesIr>` and returns `Module<Unverified>`.

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

- `default<O0>`: verifier-only / no-op cleanup.
- `default<O1>`: conservative scalar cleanup.
- `default<O2>`: stronger scalar + memory + loop cleanup as available.
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

- APInt.
- Real ConstantFolder for integer/cast/compare/select/GEP basics.
- KnownBits for integer/pointer scalar values.
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

- Do not implement optimization transforms before the constant folder and KnownBits foundation exists.
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
