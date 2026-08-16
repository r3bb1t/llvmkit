# llvmkit Roadmap

This roadmap is focused on making `llvmkit` a practical pure-Rust replacement for common `inkwell` usage in IR construction, binary-lifting-oriented IR cleanup, analysis-heavy deobfuscation, and controlled obfuscation pipelines. It is intentionally biased toward optimizations, value tracking, and pass infrastructure rather than code generation.

## Current baseline

In development toward **0.0.4**, tracking LLVM 22.1.4 (`llvmorg-22.1.4`). The
last release published to crates.io is 0.0.3; 0.0.4 is unreleased and settles
the public API after a broad reshape.

Shipped today:

- **Owned modules and storable ids — the id-first handle model, shipped in
  0.0.4.** `Module<B, S>` has no lifetime parameter, owns its storage, and is
  `Send`, so it can be returned from a function, stored in a struct or a `Vec`,
  and moved across a thread boundary. Declarations and value-producing
  builder calls return a `Copy + Send` **id** (`IntValueId<W, B>`,
  `FunctionId<R, B>`, `GlobalId<B>`, …) that carries the module's identity
  without borrowing it; the by-name lookups (`global`, `alias`,
  `ifunc`, `function`, `function_dyn`) return the same
  currency their `add_*` twins do. Blocks are minted as linear, `!Copy` handles
  instead (`append_basic_block`, `append_block_with_params`), and `.id()` on one
  gives the storable `BlockId`; terminator builders consume the builder and hand
  back the terminated block alongside the new instruction, not an id. Borrowing
  handles are minted per operation from `m.view(id)` / `m.try_view(id)`. A
  module's identity is the `B: ModuleBrand` *type*, in three rungs —
  `module_new!`, `Module::branded::<B>`, `Module::dynamic`. `Module::with_new`
  and the generative lifetime brand `Brand<'id>` no longer exist.
- Textual `.ll` lexer and constructive-subset parser, with closure-free entry
  points (`parse_branded::<B>`, `parse_dynamic`, `parse_file_branded::<B>`,
  `parse_file_dynamic`, `parse_into`) that return the owned `Module`.
- Typed IR model, constants, globals, functions, basic blocks, instructions, verifier, AsmWriter.
- Schema-typed IR construction: compile-checked calls (`call` +
  `TypedCallInst`), typed pointers (`TypedPointerValue` + compile-time field
  GEPs), typed folder hooks, and Braun-style auto-SSA (`SsaBuilder`).
- CFG and dominator-tree queries.
- Capability-graded module/function passes: a pass declares a
  capability rung and the driver derives preservation and the output module's
  verified-state, so over-claiming what a pass preserves is a compile error.
- Built-in analyses: `DominatorTreeAnalysis`, `KnownBitsAnalysis`,
  `DemandedBitsAnalysis`, and `PassInstrumentationAnalysis`.
- Built-in transform passes: worklist-driven `DcePass` and `InstSimplifyPass`
  (fold-to-constant), plus `SimplifyDemandedBitsPass`.
- LLVM 22.1.4-style `ConstantFolder` for the modeled IR-builder surface plus
  target-independent pure-constant `ConstantFold.cpp` folds for represented
  `ConstantExpr`, integer/float, cast, compare, select, GEP, vector, and
  aggregate cases; DataLayout / TLI-heavy folds stay in analysis-only APIs.
- **A Rust-API-Guidelines sweep over the whole public surface, shipped in
  0.0.4.** `IrBuilder` emitters lost the `build_` prefix (`b.int_add`, `ret`,
  `br`, `call`, `gep`); by-name lookups became bare nouns with `get_` reserved
  for the `get_or_insert_*` family; enum variants and macro-declared types took
  strict RFC-430 casing (`Opcode::Icmp`, `CastOpcode::Zext`, `IrBuilder`);
  read APIs that allocated a `Vec` now return iterators (CFG
  successors/predecessors, `debug_records`, `attribute_groups`,
  `module_flags`, …); `LoadBuilder` / `StoreBuilder` / `AllocaBuilder` replaced
  the combinatorial `*_volatile_with_align` / `*_atomic` flats with one
  spelling per operation; module flags and named metadata became typed
  (`ModuleFlagBehavior`, `ModuleFlagKey`, `NamedMetadataId`,
  `NamedMetadataName`) instead of raw strings and `usize`; `llvmkit-asmparser`
  grew root re-exports and error messages that no longer embed `{:?}`; and
  `Debug` / `Hash` / `FromStr` / `#[must_use]` were completed across the
  family. The TableGen generator also became a real crate with a library API
  (`llvmkit-tablegen`) rather than a binary inside `llvmkit-ir`.

Hard gaps for replacing more LLVM/Inkwell workflows:

- ~~Ordinary `clang` output does not parse.~~ **Closed 2026-07-31** (Milestone
  0). `clang -O0` / `-O2` output parses, verifies, and round-trips; a guard over
  the vendored `Attributes.td` keeps the keyword table from drifting again.
- No runnable pass pipeline. `pass_pipeline.rs` parses
  `"cleanup-lift,instcombine"` into scope-typed data, and nothing consumes it:
  there is no NAME→pass-constructor registry, so a parsed recipe cannot be run
  (Milestone 8). The shipped transform inventory is three passes — `DcePass`,
  `InstSimplifyPass`, `SimplifyDemandedBitsPass` — over four analyses
  (`DominatorTreeAnalysis`, `KnownBitsAnalysis`, `DemandedBitsAnalysis`,
  `PassInstrumentationAnalysis`).
- Roughly a quarter of the public API carries documentation. `missing_docs` is
  denied per-module in seven `llvmkit-ir` modules (`analysis`, `cfg_update`,
  `error`, `pass_access`, `pass_context`, `pass_manager`, `worklist`), so the
  ratchet has started — but no crate-level `#![deny(missing_docs)]` holds the
  line anywhere, so the figure is still free to drift.
- Constant folding outside the modeled target-independent builder surface is
  still partial: DataLayout / TLI / libcall / load-through-bitcast folds are
  represented only where the analysis APIs implement them, and InstSimplify-
  style nonconstant folds are still future transform work.
- `KnownBits.h` is **complete** (the ledger asserts an empty gap list);
  `ValueTracking.h` is at **93 of 101 entry points** with eight recorded gaps.
  The transfer functions underneath remain a represented integer, pointer,
  fixed-vector, and intrinsic-fact subset — entry-point coverage is not arm
  coverage, and `computeKnownFPClass` is the clearest case: it counts as one
  modeled entry point while its opcode dispatch is deliberately partial.
- No pass-builder or pipeline *execution* engine yet: the named recipes and a
  text parser ship (`pass_pipeline.rs`), but running a parsed pipeline still
  needs a NAME→pass-constructor registry.
- No loop PM / CGSCC PM.
- No alias analysis, MemorySSA, ScalarEvolution, LazyValueInfo, or post-dominance.
- No bitcode reader/writer.
- Metadata is parsed in places but instruction metadata propagation and full debug-info modeling are incomplete.
- Metadata *nodes* are the one currency the id work did not tag. 0.0.4 closed
  half of this: named metadata now has `NamedMetadataId<B>`, which carries a
  `ModuleId` tag and a brand like every other id. Metadata nodes themselves did
  not follow — `MetadataSlot` (and the `ValueSlot` inside
  `DebugMetadataOperand::Value`) is still a bare arena index
  carrying neither a `ModuleId` tag nor a brand, so neither half of D7 reaches
  it. An out-of-range slot is rejected
  (`IrError::UnknownMetadataSlot`); an *in-range* slot from another module still
  mis-resolves silently when printed. Every API that attaches one demands the
  target module's `Unverified` token, which bounds the exposure but does not
  close it. Tracked in `docs/future-work.md`.
- Intrinsic *modeling* is not yet broad enough for arbitrary optimized or lifted
  IR. The distinction matters: every `llvm.*` name in the vendored LLVM 22.1.4
  TableGen data is recognized, target-specific ones included, so the names are
  not the limit. What is narrow is the represented signature families and the
  KnownBits / DemandedBits facts attached to them (Milestone 2).

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
| ~~P0~~ | ~~Textual `.ll` parser completeness~~ | **Done 2026-07-31 (Milestone 0).** Ordinary `clang` output parses, verifies, and round-trips; the keyword table is guarded against drift from `Attributes.td`. |
| P0 | ConstantFold / ConstantFolder parity maintenance and extension | Keep the shipped local simplifier aligned with LLVM as new modeled opcodes, types, and ConstantExpr forms land. |
| P0 | KnownBits / ValueTracking | Needed for opaque predicates, alignment, bit-mask simplification, flag recovery, indirect-branch reasoning. |
| P0 | Core scalar cleanup passes | Needed to replace the most common LLVM `O1` / `O2` cleanup wins after lifting. |
| P1 | Lifter-oriented memory/stack passes | Needed for pseudo-memory models, concrete image loads, stack promotion, load-width cleanup. |
| P1 | Alias/memory analyses | Required before memory optimizations are trustworthy. |
| P1 | PassBuilder-style pipeline API | Needed for ergonomic `"default<O1>"` / custom pipeline use. |
| P2 | Obfuscation passes | Useful once CFG/analysis/pipeline infrastructure is stable. |
| P2 | Loop / CGSCC PM | Needed for serious optimization composition. |
| P2 | Bitcode + richer metadata/intrinsics | Needed for broader LLVM ecosystem interop. |
| P2 | Public API documentation coverage | `docs.rs` is the storefront, and it currently reports roughly a quarter of the crate documented. |

---

## Milestone 0: Textual `.ll` parser completeness

### Status (2026-07-31)

**The keyword inventory below is closed.** All listed function attributes,
parameter/return attributes (including the typed `byval(T)`/`sret(T)` family
and `uwtable`'s kind grammar), and `dso_local`/`dso_preemptable` on globals,
aliases, and ifuncs now parse, print, and round-trip. Landing the probe matrix
as a test (`parser_attribute_matrix.rs`) immediately surfaced and fixed three
more gaps the keyword census could not see: `c"..."` string-constant
initializers (in every clang module as `@.str`), the printer emitting
parameter `align(4)` where the grammar everywhere is `align 4`, and
`alignstack` parsing a space form while printing the upstream paren form.
Both clang-shaped acceptance programs (-O0 and -O2) parse, verify, and
round-trip in that test file.

**Milestone 0 is complete.** The three remaining items closed on the same
date:

- **Work item 4, diagnostics.** A declaration-linkage global carrying an
  initializer reported `expected top-level entity`, blaming the wrong
  construct; it now names the actual problem. Two messages that read wrong
  through the `expected {}` frame were fixed alongside.
- **Deferred alias/ifunc targets.** Aliases and ifuncs resolved their target
  eagerly, so a printed module whose ifunc resolver was declared later did not
  re-parse. Forward targets now become a placeholder patched at end of module,
  reusing the mechanism `personality` already had.
- **Anti-drift, by guard rather than generation.** `Attributes.td` is vendored
  under `crates/llvmkit-asmparser/tablegen/` (tracked, unlike `orig_cpp/`) and
  `attribute_td_drift.rs` asserts every LLVM 22.1.4 attribute is either
  accepted by the parser in a declared position or named in an explicit
  `NOT_YET_MODELED` list. Generating the keyword table outright was considered
  and rejected: llvmkit deliberately models a subset, so generation would force
  modeling all of it and would mean generating part of the 700-variant
  `Keyword` enum. The guard gives the same "cannot silently drift" property —
  verified red-green in both directions — at a fraction of the cost. Growing
  the modeled set is now a matter of deleting lines from that list, which
  currently names 42 attributes.

### Why this was first — the 2026-07-27 measurement (historical)

> Everything from here to the end of this milestone is the record of why the
> work was scheduled and what it consisted of. All of it landed on 2026-07-31;
> nothing below is open. Kept because the measurement is the evidence for the
> priority ordering, not because the gaps still exist.

Measured 2026-07-27 by consuming the published crate surface from an external
test crate: of seven `.ll` shapes a user would realistically hand to llvmkit,
five parsed, verified, and round-tripped clean — and the two that failed were
plain `clang -O0` and `clang -O2` output.

The failures were **not structural**. Aggregates, GEP, `switch`, vectors,
`invoke` / `landingpad` / `resume` / `personality`, full debug info
(`DICompileUnit`, `DISubprogram`, `DILocation`, `!dbg` attachments), atomics,
`cmpxchg`, `atomicrmw`, `fence`, `callbr`, `blockaddress`, `indirectbr`,
`va_arg`, `musttail`, inline asm, scalable vectors, comdats, aliases, ifuncs,
and `i128` / `x86_fp80` / `half` / `bfloat` literals already parsed. So did
every intrinsic name in the vendored table, target-specific ones included.

What failed was a **keyword list**: 15 failures across 88 single-feature
probes, in three clusters. It was the cheapest large win available — it moved
llvmkit from "parses IR written for it" to "parses IR clang produced" — and
nothing else on this roadmap was worth much to an outside user until it landed.

### Work items — all closed 2026-07-31

1. **Function attributes** — accepted inside `attributes #N = { … }`. Was
   missing:
   `uwtable`, `norecurse`, `hot`, `inlinehint`, `sanitize_address`, `ssp`,
   `sspstrong`, `nonlazybind`, `minsize`.

   (Already accepted, for contrast: `noinline`, `nounwind`, `optnone`,
   `readnone`, `readonly`, `willreturn`, `mustprogress`, `nofree`, `nosync`,
   `cold`, `noreturn`, `speculatable`, `alwaysinline`, `optsize`, `convergent`,
   `nocallback`, `strictfp`, `noduplicate`, every `memory(…)` form, and
   string-valued attributes such as `"target-cpu"="x86-64"`.)

2. **Parameter and return attributes.** Was missing on parameters: `byval(T)`,
   `sret(T)`, `byref(T)`, `inalloca(T)`, `elementtype(T)`,
   `dereferenceable(N)`, `dereferenceable_or_null(N)`, `inreg`, `nest`,
   `swiftself`, `captures(none)`. Missing on returns: `dereferenceable(N)`.

   `byval` / `sret` were the load-bearing pair: any C source that passes or
   returns a struct by value produces them, so their absence rejected a large
   share of ordinary clang output.

3. **Runtime preemption specifiers on globals.** `dso_local` and
   `dso_preemptable` were accepted on `define` and `declare` but rejected on
   global variables and aliases, including in combination with linkage and
   `unnamed_addr`. Every `clang` invocation that is not `-fPIC` emits
   `@g = dso_local global …`.

4. **Diagnostics for genuinely invalid input.** `@g = external global i32 0`
   (an `external` global carrying an initializer — rejected by `llvm-as` too)
   reported `expected top-level entity`, pointing at the wrong construct. The
   error surface was swept once the above landed, so invalid IR names the
   actual problem.

### Acceptance criteria — all met 2026-07-31

- `clang -O0` and `clang -O2` output for a small C translation unit parses,
  verifies, and round-trips through `format!("{module}")`.
- Every attribute keyword LLVM 22.1.4 accepts in a modeled position either
  parses or produces a diagnostic naming the keyword.
- The single-feature probe matrix that found these gaps ships as a test file,
  so the next missing keyword fails CI rather than a user's first attempt.
- No silent acceptance: an attribute that parses but is then dropped on print
  is a round-trip failure, not a pass.

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

A 2026-07-23 audit against the vendored `llvmorg-22.1.4` sources re-verified
this surface and fixed the handful of divergences it found — one real bug
(`icmp` of a global vs `null` in a non-zero address space) plus several
safe-but-not-identical over-precisions and previously-declined folds; a
whole-branch review confirmed no mis-folds and no over-folds remained. See the
CHANGELOG ("Constant-folding parity with LLVM 22.1.4") and `docs/future-work.md`
for the fixes. Two of the four points that audit deferred have since closed:
**constant uniquing** (2026-08-01 — `GlobalValueRef`, `GepOffset`,
`SymbolDelta`, and `SymbolDeltaPlus` now intern on their structural
fingerprint, so identity comparison is structural everywhere; the law is pinned
in `tests/constant_uniquing.rs`) and the **proactive ApFloat / `ApInt`
bit-exactness audit** (2026-08-01 — both halves complete, fourteen defects
fixed). Still open and deliberate: CHERI-like `ptrtoint`/`ptrtoaddr` on
`pointer_size != index_size` layouts, and the remaining
`SymbolicallyEvaluateGEP` sub-cases, each of which only ever declines a fold.

Represented `ConstantExpr` construction/folding covers the parser-needed
add/sub/xor, GEP, vector, and cast forms, including upstream vector GEP,
bitcast, cast, and select fixtures.

Analysis-only behavior remains split out: DataLayout / TLI / libcall,
denormal, load-through-bitcast, and other target/library-dependent folds live in
`constant_folding.rs` where represented, not in the default builder folder.

### Remaining work

1. **Unmodeled-surface follow-up**
   - The ApFloat side of this item **closed 2026-08-01**: the bit-exactness
     audit ported every `APFloatTest.cpp` family covering the seven modeled
     semantics and fixed the defects it found. What remains here is not
     ApFloat's own arithmetic but the helper operations a *future* LLVM folding
     formula may need once new opcodes become represented.
   - Keep conservative no-fold behavior where exact parity is not implemented.

2. **ConstantFold / ConstantExpr extension**
   - Extend parity only when new opcodes, types, or parser-needed expression
     forms become represented.
   - Keep LLVM 22.1.4 provenance tests current for every new fold.
   - Keep InstSimplify-style nonconstant identities (`x + 0`, `x * 1`,
     redundant casts, and similar transforms) in the optimization-pass roadmap,
     not in the all-constant default folder.

3. **Folder trait expansion**
   - Expand `IrBuilderFolder` hooks as new builder families need folding.
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

> **The ledger is the authority for items 1–2, not this list.**
> `crates/llvmkit-ir/tests/value_tracking_parity.rs` is symbol-keyed and
> asserts `modeled + gaps` equals the audited surface, with a recorded reason
> per gap. This section is prose and drifts; the ledger cannot.

1. ~~**KnownBits formula parity follow-up**~~ — **closed 2026-08-01.**
   `KnownBits.h`'s public surface is fully modeled and `KNOWN_BITS_GAPS` is an
   empty slice, so a regression or a newly-synced upstream method has to be
   acknowledged rather than absorbed. LLVM's conflict-state behaviour
   (`zero & one` may be non-zero internally) is kept, as upstream relies on it
   for intersections and diagnostics.

2. **ValueTracking operator parity** — **93 of 101 entry points** as of
   2026-08-04; eight gaps, each with a reason in the ledger. Landed since this
   list was written: the and/or/xor refinements (including the `and(x, -x)` and
   `xor(x, x - 1)` idiom arms), select edge facts on both the integer and
   floating-point sides, PHI recurrences with `matchSimpleRecurrence`, the
   assumption and dominating-condition arms, pointer/object analysis,
   speculation safety, and the whole floating-point classification subsystem.
   Still open: additional call/callee attribute facts beyond the shipped range
   and `returned` cases, pointer alignment/GEP/cast facts, fixed-vector
   demanded-element facts, freeze poison checks, additional intrinsic facts, and
   `computeKnownFPClass`'s remaining dispatch arms — which move no ledger row,
   since the entry point already counts, and are enumerated in
   `known_fp_class.rs`'s module header.

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

> Partly shipped already: a fold-to-constant `InstSimplifyPass` (item 1's
> constant-folding slice) and a worklist-driven `DcePass` (item 3) are in-tree
> and tested (`scalar_cleanup_passes.rs`). The list below is the fuller target
> set — the algebraic/InstCombine depth and the CFG/SCCP/CSE/GVN passes remain.

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
- `cleanup-lift` can be assembled as a `function_pipeline((..))` — run per
  function, or dropped into a module pipeline through `for_each_function(..)`,
  or assembled at run time as a `DynFunctionPipeline` — whose mutating members
  downgrade the result to `Module<B, Unverified>`, forcing an explicit
  re-`verify()`. (`cleanup-lift` is a *function*-scoped recipe name today:
  `pass_pipeline::CLEANUP_LIFT` is a `PipelineName<FunctionPipelineScope>`.)

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

Sketch of the *unshipped* surface. `PassBuilder` does not exist today; the
spelling below is what it has to look like against the shipped types — one
`Analyses` bundle threaded through the run, and verification as a typestate on
the owned `Module<B, S>`:

```rust
let mut analyses = Analyses::new();
let mut pb = PassBuilder::new();
let mut mpm = pb.parse_module_pipeline("cleanup-lift,instcombine,simplifycfg")?;
let unverified: Module<B, Unverified> = mpm.run(module.verify()?, &mut analyses)?;
let verified = unverified.verify()?;
```

> Partly shipped: the named recipes (`cleanup-min`/`cleanup-lift`/`cleanup-o1-ish`,
> `llvmkit-default<O0/O1>`) and a recursive-descent parser for the `name(a,b)`
> syntax already ship as typed data (`pass_pipeline.rs`, tested in
> `pass_pipeline_data.rs`). The names are scope-typed: `CLEANUP_MIN` /
> `CLEANUP_LIFT` / `CLEANUP_O1_ISH` are `PipelineName<FunctionPipelineScope>`,
> `DEFAULT_O0` / `DEFAULT_O1` are `PipelineName<ModulePipelineScope>`. What
> remains is the `PassBuilder` and an execution engine — a NAME→pass-constructor
> registry that turns a parsed recipe into a runnable pipeline.

Named pipelines (the first three ship as recipe *names* today; the last two do
not exist yet):

- `llvmkit-default<O0>`: verifier-only / no-op cleanup. (llvmkit-specific
  subset, deliberately named apart from upstream's non-empty `default<O0>`.)
- `llvmkit-default<O1>`: conservative scalar cleanup.
- `cleanup-lift`: binary-lifting/deobfuscation-biased cleanup (function scope,
  alongside `cleanup-min` and `cleanup-o1-ish`).
- `llvmkit-default<O2>`: stronger scalar + memory + loop cleanup as available.
- `obfuscate<...>`: obfuscation pipeline once Milestone 10 lands.

### Acceptance criteria

- Unknown pass names produce useful diagnostics.
- Pipeline parser has tests for nesting and options.
- Read-only vs mutating pipeline effects remain visible in types.

---

## Milestone 9: Inkwell replacement completeness

### Goal

Cover APIs commonly used by Rust projects that currently depend on Inkwell for IR generation and optimization setup.

The per-API delta is tracked in [`docs/inkwell-migration.md`](docs/inkwell-migration.md).

### Shipped

> - **Owned, brand-preserving parse entry points.** `parse_branded::<B>(src)`,
>   `parse_dynamic(src)`, `parse_file_branded::<B>(path)`,
>   `parse_file_dynamic(path)`, and `parse_into(module, src)` all return the
>   owned `Module` with its brand type intact, so a parsed module can be
>   verified, stored in a struct, and moved across a thread boundary. The
>   closure form (`parse_assembly`) remains only for callers who need the
>   `ParsedModule` slot mapping, which borrows the module it was parsed from.
>   Printing is `Display` on `Module` / `ModuleView` (`format!("{module}")`).
> - **A settled public API.** 0.0.4 stops the churn in the module, handle/id,
>   builder, and pass surfaces, and closes it out with a Rust API Guidelines
>   sweep: no `build_` prefix on emitters, bare-noun lookups with `get_`
>   reserved for `get_or_insert_*`, RFC-430 casing on every enum variant and
>   macro-declared type, iterator returns in place of allocated `Vec`s,
>   `LoadBuilder` / `StoreBuilder` / `AllocaBuilder` in place of the
>   combinatorial memory-op flats, typed module flags and named-metadata ids,
>   and completed `Debug` / `Hash` / `FromStr` / `#[must_use]` coverage. It is
>   *not* a stability promise — the crate is pre-1.0 and every `0.0.x` is
>   mutually incompatible under Cargo's rules. Expect further breaks; expect
>   them to be deliberate and spelled out.
> - **Attribute groups and function / call-site attribute APIs.** Attribute
>   groups are readable (`Module::attribute_groups`, an iterator) and writable
>   (`set_attribute_group`), and the call builders take `call_attributes`, with
>   typed readers for parameter, return, and string-valued attributes. The
>   modeled keyword set is guarded against upstream drift by
>   `attribute_td_drift.rs`; what remains is the 42 keywords its
>   `NOT_YET_MODELED` list still names.
> - **Intrinsic declaration APIs.** `get_or_insert_intrinsic_declaration`, plus
>   its `_by_id` and `_by_name` twins. Overloaded-intrinsic *typing* beyond the
>   represented signature families is still open (Milestone 2).
> - **Inline asm.** `Module::inline_asm` plus `inline_asm_call` /
>   `inline_asm_invoke` / `inline_asm_callbr` on the builder; the textual form
>   parses and round-trips.

### Work items

- Builder coverage for remaining common LLVM IR operations and intrinsics.
- Full metadata attachment storage and printing.
- Debug-info model sufficient to preserve parsed debug metadata conservatively.
- The remaining unmodeled attribute keywords (`NOT_YET_MODELED`, 42 today).
- Overloaded intrinsic typing beyond the represented signature families.
- Bitcode reader/writer or an explicit bridge plan if bitcode stays out longer.
- Better error spans for parser/verifier failures. The *message* half landed in
  0.0.4 — asmparser errors no longer embed `{:?}`-formatted locations and `Io`
  carries a structured `{ kind, message }` — but spans are still coarse.

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
- **AutoUpgrade completeness.** `llvm/lib/IR/AutoUpgrade.cpp`'s module-level,
  target-independent half landed at LLParser parity W13d as
  `crates/llvmkit-ir/src/auto_upgrade.rs` (`UpgradeModuleFlags`,
  `UpgradeSectionAttributes`, `UpgradeTBAANode`). What belongs to this
  milestone is the remainder: the intrinsic-upgrade framework
  (`UpgradeIntrinsicFunction` / `UpgradeIntrinsicCall` /
  `UpgradeCallsToIntrinsic`) with both its generic and its target-specific
  arms — x86, ARM/AArch64, AMDGPU, NVVM, RISC-V, WebAssembly — plus
  `UpgradeARCRuntime`, `UpgradeBitCastInst` / `UpgradeBitCastExpr`,
  `UpgradeInlineAsmString` and `UpgradeDataLayoutString`. `UpgradeDebugInfo`
  comes with `StripDebugInfo`; `copyModuleAttrToFunctions` comes with a
  `Triple`. Per-item blockers are in `docs/future-work.md`.
- Target-library-info-like hooks where transforms need libc semantics.
- DataLayout parity hardening.
- `llvm-dis` / `llvm-as` textual parity fixture expansion where external tools are available manually.

### Acceptance criteria

- Representative `.ll` from Clang/Rust optimized output parses or fails with precise unsupported-feature diagnostics.
- Round-trip preserves unknown metadata conservatively.
- Unsupported constructs are never silently dropped.

---

## Suggested release sequence

Ordered stages, not version numbers. Which release carries which stage is decided
when the stage lands, not now — the crate is pre-1.0, versions stay `0.0.x`
until there is a reason to claim otherwise, and pinning a feature set to `0.1`
before knowing what `0.1` should mean is a promise this file cannot keep.

### Stage 1: Folding and ValueTracking foundation — **ships as 0.0.4**

Carries everything this entry planned, plus two things that were not on the
list when the list was written — the id-first handle redesign, and the Rust API
Guidelines sweep that followed it:

- ConstantFolder / ConstantFold parity foundation for the modeled IR surface.
- ValueTracking hardening required by initial cleanup passes.
- InstSimplify + DCE.
- Owned modules, storable ids, brand-as-type module identity, and the settled
  public API (see "Current baseline"). This ships as 0.0.4, the version the
  workspace already carried — no bump is needed, because 0.0.4 was never
  published. Under Cargo's pre-1.0 rules every `0.0.x` is already mutually
  incompatible, so the break needs no wider signal, and a minor bump would
  imply a stability the crate does not yet have.
- The API-idiomatics sweep: dropped `build_` prefixes, bare-noun lookups,
  RFC-430 casing, iterator returns, the Load/Store/Alloca builders, typed
  module flags and `NamedMetadataId`, asmparser root re-exports and honest
  error messages, and `llvmkit-tablegen` as a crate with a library API. Every
  item in it is breaking; all of it is spelled out in `CHANGELOG.md`.

### Stage 2: Parser completeness and release hygiene — **parser done; publish remains**

Small, mechanical, and independently checkable against `llvm-as`. It is first
because every later stage is worth more once real-world IR can get in.

- ~~Milestone 0 in full~~ — **done 2026-07-31**: the attribute keywords,
  `dso_local` on every global object, `c"..."` constants, deferred alias/ifunc
  targets, the diagnostics sweep, the probe matrix, and the `Attributes.td`
  drift guard.
- The crates.io release checklist below.
- Continue the `missing_docs` ratchet. It has started, but inside `llvmkit-ir`
  rather than on the small crates: `analysis`, `cfg_update`, `error`,
  `pass_access`, `pass_context`, `pass_manager`, and `worklist` each carry a
  module-level `#![deny(missing_docs)]`. The small crates — `llvmkit-support`,
  `llvmkit-macros`, `llvmkit-tablegen` — are still the cheapest place to take a
  crate-level `#![deny(missing_docs)]` all the way.

### Stage 3: Lifting cleanup pipeline

- InstCombine subset.
- SimplifyCFG.
- SCCP.
- ConcreteImageLoadFoldPass.
- NarrowLoadFromTruncPass.
- PseudoStackPromotionPass.
- `cleanup-lift` named pipeline — and the NAME→constructor registry that makes
  a parsed pipeline runnable, since a recipe nobody can execute is not a
  feature.

### Stage 4: Memory and SSA promotion

- BasicAA.
- MemoryLocation.
- Mem2Reg.
- SROA subset.
- EarlyCSE.
- GVN-lite.

### Stage 5: Loop and stronger analysis

- LoopInfo.
- PostDominatorTree.
- ScalarEvolution subset.
- LICM subset.
- ADCE / BDCE.
- Opaque predicate detection.

### Stage 6: Obfuscation and deobfuscation suite

- Basic-block splitting.
- Instruction substitution.
- Bogus control flow.
- Control-flow flattening.
- Opaque predicate generation/removal.
- Dispatcher/jump-table recovery improvements.

### Stage 7 and beyond: Ecosystem compatibility

- Bitcode. Lower urgency than its prominence suggests: `llc` and `opt` both
  read textual `.ll`, so `format!("{module}")` is already a working handoff to
  the LLVM toolchain. Bitcode buys speed and producer/consumer parity, not
  basic interoperability.
- Debug metadata preservation.
- Broader intrinsics.
- Textual PassBuilder compatibility.
- Larger upstream fixture corpus.
- Full `missing_docs` coverage across `llvmkit-ir` and `llvmkit-asmparser`.

---

## Release checklist (crates.io)

0.0.3 is the last release on crates.io; 0.0.4 is the next one, and this is its
checklist. State as of 2026-07-27, verified with `cargo package --workspace`:
every member packages and verifies from its own tarball, metadata is complete
(description, license, repository, homepage, `rust-version`, keywords,
categories), sizes are far inside the limit, and no `todo!()` / `unimplemented!()`
remains in library source. Publishing works today.

The workspace has **six** members as of 0.0.4 — `llvmkit`, `llvmkit-support`,
`llvmkit-asmparser`, `llvmkit-ir`, `llvmkit-macros`, and `llvmkit-tablegen`.
`llvmkit-tablegen` was split out of `llvmkit-ir` during the 0.0.4 cycle, so
0.0.4 is its **first** publication; the other five are already on crates.io
through 0.0.3. What is left:

- [x] Ship the license text inside every `.crate`. `LICENSE` lived only at the
      workspace root, and Cargo auto-includes a license file only from the
      *package* directory, so the published tarballs went out with the `license`
      field set and no license text in them — a real defect for a derivative
      work of the LLVM Project, since Apache-2.0 section 4(a) requires
      recipients of a distribution to receive a copy. Each package directory now
      carries a verbatim copy, and CI compares all six against the root.
- [x] Add a `README.md` for `llvmkit-macros`, the one member without one; its
      crates.io page is otherwise blank. **Done 2026-07-31.**
- [x] Add a `README.md` for `llvmkit-tablegen`. Splitting it out of
      `llvmkit-ir` re-opened the problem the item above closed: it was the one
      member with no `README.md`, so its crates.io page would have been blank on
      the very release that first publishes it. **Done 2026-08-07** — every
      workspace member now ships one.
- [ ] Add `[package.metadata.docs.rs]` so docs.rs builds are pinned and
      deterministic rather than default-feature guesses. `llvmkit` and
      `llvmkit-ir` both declare a `default = ["macros"]` feature now, so
      "default-feature guess" is no longer a hypothetical.
- [ ] Add a `cargo package --workspace` step to CI. It is the gate that proves
      the published artifact builds, and nothing in CI covers it today.
- [ ] Give `[0.0.4]` a date in `CHANGELOG.md` and collapse the two
      "unreleased" headings into one at release time.
- [ ] Re-point the handful of rustdoc comments that cite `docs/…` paths; `docs/`
      sits outside every package directory, so those references dangle for a
      docs.rs reader.
- [ ] Tag the release. The repository currently carries no tags at all.

Publish order is `llvmkit-support`, `llvmkit-macros` and `llvmkit-tablegen`,
then `llvmkit-ir`, then `llvmkit-asmparser`, then `llvmkit`.

That order follows the actual graph, which is worth writing down because it is
not the one people assume:

| Crate | Depends on (within the workspace) |
|---|---|
| `llvmkit-support` | — |
| `llvmkit-macros` | — |
| `llvmkit-tablegen` | — |
| `llvmkit-ir` | `llvmkit-macros`; `llvmkit-tablegen` as a **build**-dependency |
| `llvmkit-asmparser` | `llvmkit-macros`, `llvmkit-support`, `llvmkit-ir` |
| `llvmkit` | `llvmkit-support`, `llvmkit-asmparser`, `llvmkit-ir` |

Note that `llvmkit-ir` does **not** depend on `llvmkit-support`: spans are the
parser's currency, so `llvmkit-support` enters the graph at `llvmkit-asmparser`.
The three leaves are interchangeable among themselves; only their position
before `llvmkit-ir` matters.

`llvmkit-tablegen` is a **build-dependency** of `llvmkit-ir`, so it has to be on
crates.io before `llvmkit-ir` can build from the registry at all — a build
dependency is not optional the way a dev-dependency is. It carries the vendored
`.td` tree, so it is also the crate that ships the 2.2 MiB.

### On the vendored TableGen

`llvmkit-tablegen` ships 2.2 MiB of LLVM 22.1.4 `.td` files, and `llvmkit-ir`'s
`build.rs` calls it to expand them into roughly 217k lines of intrinsic tables.
Both obvious "optimizations" were measured on 2026-07-27 and rejected:

- *Pre-generating and committing the expansion* would replace 2.2 MiB of input
  with a 13 MB generated file in git and in every tarball, to save the ~2 s the
  build script actually costs. The `.td` form is the smaller artifact by 6×; the
  build script is a compression win, not a tax.
- *Feature-gating intrinsics per target* is mechanically easy — 96.8% of records
  are target-specific and already partitioned by contiguous offset/count — and
  buys only a few seconds of a ~23 s build, which paired trimmed-vs-full
  rebuilds put near the noise floor. It would also make
  `resolve_intrinsic_name` feature-dependent: `llvm.x86.sse2.pause` would answer
  `UnknownIntrinsic` instead of `Known(..)` depending on enabled features, and
  because Cargo features are additive and unified across the dependency graph, a
  transitive dependency could silently change how a `.ll` file reads.

Keep the vendored `.td` tree and the build script.

---

## Bindings (Python, Java)

Planned, not written. They were blocked on exactly one thing: an API not stable
enough to wrap, since wrapping a moving surface means rewriting the wrapper on
every break. 0.0.4 is where that surface stops moving week to week — settled,
not frozen, since the crate is pre-1.0. Bindings are **not**
out of scope — what is out of scope for the project is code generation, target
backends, linking / object emission, and any dependency on `llvm-sys`,
`inkwell`, or `libLLVM`.

Keeping the surface wrappable was a standing constraint on the 0.0.4 redesign,
which is why the shape already fits: nothing is reachable only from inside a
closure (`module_new!` / `Module::branded::<B>` / `Module::dynamic` all return
an owned module), no lifetime appears in a storable type (every id is
`Copy + Send + 'static`), and `Module::dynamic`'s `DynBrand` is the rung a
dynamic language uses — registry-exempt, so many live modules are legal, with
`IrError::ForeignValueId` as the separation verdict a wrapper raises as an
exception. See the README's "Bindings" section for the detail, including the id
table a wrapper still has to supply itself.

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
