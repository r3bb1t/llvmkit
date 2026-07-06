# Future work

This document captures everything the `feature-1/irbuilder-type-safety`
session's audits found but did not implement. Each item cites the audit
source (design doc, source file, or upstream reference) so a later session
can pick it up cold. Source: the approved plan's "Future-work document"
section (`docs/superpowers/plans/2026-07-05-irbuilder-type-safety.md`'s
originating design at
`docs/superpowers/specs/2026-07-05-irbuilder-typed-calls-autossa-design.md`),
transcribed faithfully, plus a "Session follow-ups" section for items
individual tasks punted during execution.

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
  natural home as a `MutatesIr` pass family. Would be a genuine "LLVM 2.0"
  differentiator: phase-ordering-free peepholes.
- **Alive2-style refinement checking** (L, future, visionary):
  [Alive2](https://github.com/AliveToolkit/alive2) does bounded translation
  validation of LLVM transforms via SMT (found 47 bugs in LLVM's own test
  suite). A llvmkit-native `refines(before, after)` harness --
  property-test-based initially (interpret both modules over random inputs
  for the modeled subset), SMT-backed later -- would make llvmkit the only
  IR library with built-in transform validation. Pairs with Doctrine D10 (no
  silent UB).
- Note: the full 5-lens inspiration sweep + synthesis workflow died on the
  session usage limit during planning (resets 21:00 UTC+3). The three
  findings above come from direct main-session searches during planning. If
  deeper inspiration mining is wanted, rerun the sweep after the reset: the
  workflow script was saved at
  `C:\Users\olegg\.claude\projects\C--Users-olegg-Desktop-llvmkit\4db9a8ff-25ec-4cae-aa22-4c6eca15f44c\workflows\scripts\irbuilder-inspiration-sweep-wf_faed8e78-60c.js`
  (relaunch with `Workflow({scriptPath})`).

## Type-system follow-ups

- Const-generic `VectorType<E, const N, Scalable>` / `ArrayType<E, const N>`
  (T4 follow-up already on the AGENTS.md roadmap).
- `Width<M>`/`Width<N>` `WiderThan` relations blocked on stable
  const-generics (documented at `int_width.rs` ~105-116); revisit when
  `generic_const_exprs` stabilizes.
- Aggregate variable categories for auto-SSA (v1 ships int/float/pointer
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
  v1 ships int/float/pointer variables and `br`/`cond_br`/`switch`/`ret`/
  `ret_void`/`unreachable` terminators only. Aggregate variable categories
  (per-field fan-out through `StructSchema`) and `invoke`/`callbr`/EH
  terminators are the documented v2 scope in the module's own doc comment.
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
- **`accept_folded/narrow_folded` helper-family factoring** -- the typed
  folder's `accept_folded_int`/`accept_folded_fp`/`narrow_folded_int`/
  `narrow_folded_fp` helper family (`ir_builder.rs`) is 4 near-identical
  bodies, kept unfactored deliberately to keep monomorphization legible
  (reviewer judged factoring optional during Task 5's review). Revisit if a
  fifth category (e.g. vector) needs the same shape.
