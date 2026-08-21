# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`AGENTS.md` is the exhaustive reference — API laws, translation idioms, per-workstream history. This file is the operating manual: what to run, how the pieces fit, and which rules fail CI or a review. When the two disagree, check the tree and fix whichever is stale.

## What this is

A from-scratch Rust reimplementation of LLVM IR — **not** a binding. Nothing links `libLLVM` or `llvm-sys`; every crate is `#![forbid(unsafe_code)]`. Behavior is ported from the read-only C++ tree at `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/`, which is documentation, never built. Tracking LLVM 22.1.4, workspace version 0.0.4 (unreleased).

Out of scope: code generation, target backends, linking. In scope and merely unfinished: optimization transforms, a runnable pass pipeline, bitcode. Python/Java bindings are **planned**, not out of scope — never write otherwise in user-facing docs.

## Commands

Every gate runs on the pinned toolchain. CI installs rustc 1.96.0; an unpinned run rewords trybuild `.stderr` diagnostics and produces mismatches that look like regressions and are not. If you see a `.stderr` diff, re-run on `+1.96.0` before touching a fixture.

```bash
cargo +1.96.0 test --workspace --all-targets --all-features    # full suite (2560 tests, 214 binaries)
cargo +1.96.0 test -p llvmkit-ir --test ap_float               # one integration file
cargo +1.96.0 test <substring>                                 # one test by name
cargo +1.96.0 clippy --workspace --all-targets --all-features -- -D warnings
cargo +1.96.0 fmt --all
cargo +1.96.0 doc --workspace --no-deps --all-features         # CI adds RUSTDOCFLAGS=-D warnings
cargo +1.96.0 test --workspace --doc --all-features            # doctests
cargo audit
```

The CI gate is exactly that list plus a per-package license check. Baseline on the pin: **0 trybuild failures of 87 registered fixtures** (86 `compile_fail` + 1 `pass`). All fixtures live in `crates/llvmkit-ir/tests/compile_fail/` and are registered in `tests/typestate_compile_fail.rs`; a fixture that is not registered there does not run.

- **Do not set `CARGO_INCREMENTAL=0`.** Leave the cache alone; other work may be running on this machine.
- `llvmkit-ir` has a `build.rs` — it expands the vendored `crates/llvmkit-tablegen/tablegen/` `.td` files into intrinsic tables (~2 s). Keep it; the `.td` input is 6× smaller than its output.

## Architecture

### The handle model — read this before writing any API

Three currencies, and mixing them within one subsystem is the smell to watch for:

| Layer | Shape | Lifetime |
|---|---|---|
| **Slot** | bare arena index (`ValueSlot`, `TypeSlot`, `MetadataSlot`) | crate-internal, `Copy`, no lifetime |
| **Id** | `(ModuleId, slot)` + brand — `IntValueId<W, B>`, `BlockId<R, B, Params>`, … | **storable**: `Copy + Send + 'static` |
| **View** | `(slot, ModuleRef<'ctx, B>)` — `IntValue<'ctx, W, B>`, … | **ephemeral**: borrows the module, never stored |

`handle.slot()` is the internal index; `handle.id()` mints the public tagged id; `m.view(id)` / `m.try_view(id)` goes the other way. Declarations and value-producing builders return **ids**. Block appenders and terminator builders are the deliberate exceptions — they return linear `!Copy` handles, because their job is to be consumed exactly once.

`Module<B: ModuleBrand, S = Unverified>` has **no lifetime parameter**: it owns `Box<ModuleCore>`, is `Send`, and moves into structs, `Vec`s, and threads. `S` is the verification typestate. There is no closure-scoped construction anywhere — `module_new!`, `Module::branded::<B>`, `Module::dynamic` all return an owned value.

A brand is a bare unit struct (`pub trait ModuleBrand: 'static {}`). Two modules with **distinct** brands cannot exchange operands — a type error. Where they deliberately **share** one (every `Module::dynamic` is `DynBrand`), the `ModuleId` tag is the backstop: `IrError::ForeignValueId`, `None` from `try_view`, or a deterministic `view` panic. New brand-generic types derive **`Branded`** (from `llvmkit-macros`), never the std traits — a std `derive` bounds every type parameter, silently reintroducing bounds a bare brand no longer satisfies, and the compiler blames the *use* site.

### Crate graph

`llvmkit-ir` is the bulk (data model, builder, verifier, AsmWriter, analyses, passes) and `llvmkit-asmparser` (lexer + `.ll` parser) sits on top of it; `llvmkit` re-exports both plus `llvmkit-support` as the umbrella.

Note the edge that is **not** there: `llvmkit-ir` does **not** depend on `llvmkit-support` — it names no `llvmkit_support` path anywhere. Support (spans, `SourceMap`) enters at `llvmkit-asmparser`, which needs it for token spans and diagnostics, and at the umbrella. Do not draw the graph as a single support→ir→asmparser chain.

`llvmkit-macros` is a **required** dependency of `llvmkit-ir` and `llvmkit-asmparser` (proc-macro crates run in `rustc` and contribute nothing to the artifact). The `macros` feature is `llvmkit-ir`'s, forwarded by `llvmkit`'s own `macros` feature; both default on, and both gate only the user-facing re-exports, never whether the proc-macro crate is built. `llvmkit-tablegen` is a **build**-dependency of `llvmkit-ir` (via `build.rs`) and is absent from the runtime graph.

### Parser ↔ printer contract

`format!("{module}")` must match `AsmWriter.cpp` byte-for-byte, and printed output must re-parse. Both halves are locked: byte-lock tests pin `examples/*.rs` output, and `parser_corpus.rs` drives a manifest of fixtures that must parse, verify, and match checked-in expected text. Ordinary `clang -O0`/`-O2` output parses today; `attribute_td_drift.rs` fails CI if the attribute keyword table drifts from the vendored `Attributes.td`.

When adding an opcode formatter, read the matching `printInstruction` arm first. A parser/printer disagreement is a bug in ours unless it is a documented deliberate improvement.

The `^N` **module summary index** follows the same split, and its model lives in `llvmkit-ir` (`module_summary_index.rs`), not in the parser crate — upstream's own layering, since `AsmWriter.cpp` prints it. `^N` numbers are *not* stored: `SlotTracker::processIndex` re-derives them on output from sorted module paths, then ascending GUIDs, so input order is not preserved. `Module`'s `Display` does not append an index; a caller reproducing `llvm-dis` prints the module and then `ParsedModule::summary_index`.

A caution the summary work paid for twice: **an upstream `.ll` `CHECK` block is a pipeline's output, not `AsmWriter`'s.** Several `test/Assembler` fixtures run through `llvm-as | llvm-dis`, and the bitcode writer drops fields the printer would emit (`relbf` on a combined summary, memprof stack ids under `-combined-index-memprof-context=false`). Check what the fixture's `RUN` line does to a field before treating a mismatch as a bug.

### Pass model — capabilities, not declarations

A pass declares a **rung** (`Inspect` / `PatchBody` / `ReshapeCfg` / `RewriteModule`) plus its `Requires` analyses, and writes one `run(cx)`. Preservation is **derived from the rung, never declared** — the report constructor is `pub(crate)`, so over-claiming is unspellable. An `Inspect` context has no `cx.mutate()` at all. A pipeline's output typestate is likewise derived: any mutating member downgrades the module to `Unverified` until `verify()` runs again.

`pass_pipeline.rs` parses pipeline strings into typed data, but **nothing can run one** — there is no NAME→constructor registry yet. That is the gating item for the transform work.

## Rules that fail CI or review

These are not style preferences. Each has a lint, a test, or a reviewer behind it.

- **No `#[allow(...)]` in any form** — not `dead_code`, not `clippy::*`, not `cfg_attr`-wrapped. Fix the code; silencing the compiler is silencing the codebase.
- **No `unsafe`, no FFI, no `as` casts, no pointer identity, no runtime panics in production paths.** `unreachable!("…invariant…")` only where the branch is provably dead by construction. `Module::view` is the one sanctioned panic, paired with `try_view`.
- **Fallibility honesty.** A return type is a claim. No `IrResult` on an infallible operation, and never a silent no-op or swallowed error.
- **No silent erasure.** A typed handle or id never widens to an erased one implicitly. Erasure is spelled `as_dyn()` / `as_erased()`, or a `_dyn` or `_erased` method. The suffix vocabulary is three-tier: typed forms carry no suffix (`int_add`); `_dyn` is the `Dyn`-marker member of a typed/erased pair (`IntDyn` / `FloatDyn` / `LenDyn` / `Dyn` — `int_load_dyn`); `_erased` is the fully-erased third tier — `Value` operands plus a runtime opcode, the vector-capable forms (`int_binop_erased`).
- **Full words, no abbreviations**: `instruction` not `inst`, `predecessor` not `pred`. Internal indices are `*Slot`; public tagged ids are `*Id`. Do not blur them. Lookups are bare nouns (`global(name)`, `function::<R>(name)` — C-GETTER); `get_` appears only in the std-consistent `get_or_insert_*` entry points, and accessors never take a `get_` prefix.
- **Cite upstream by symbol, never line number** — in comments, rustdoc, tests, `UPSTREAM.md`, `CHANGELOG.md`, and `docs/`. `// Mirrors LLParser::parseTopLevelEntities (LLParser.cpp)`. Line numbers rot the moment the vendored tree moves.
- **Counts and completeness claims carry their derivation.** Any number, or any "all / every / none / now say so" claim, written into `docs/`, `UPSTREAM.md`, `CHANGELOG.md` or a test doc comment names the command that produced it and the commit it was measured at. Nothing in CI checks these — re-derive rather than copy, this file included. Procedure, and the tool-hazard table that makes a count wrong before you write it: the `claims-and-counts` skill. Full rule in `AGENTS.md` under **Testing & QA**.
- **Doctrine D1–D11** governs every public API (full prose in `README.md`, short list in `AGENTS.md`). Cite ids in commits and reviews.

## Testing

**Tests are ported, not invented** (D11). Source them from `orig_cpp/.../llvm/test/{Assembler,Verifier}/*.ll` or `unittests/{IR,ADT}/*Test.cpp`.

**Port faithfully — no deviation in logic.** Same inputs, built the way upstream builds them; same expected results, spelled the way upstream spells them; same comparison. Do not substitute your own oracle — precomputing expectations by another route tests your derivation instead of LLVM's answer, and produces an invention that looks like a port. If the port is blocked because llvmkit cannot express what the fixture is written in, *that gap is the finding*: close it or record it, do not route around it.

The same standard binds the **routine** you are porting, not only its tests — same control flow, same branch order, same guards, each diagnostic at the same point and token. See the `porting-from-orig-cpp` skill.

Genuinely llvmkit-specific tests (an internal representation choice, a parse/print idempotence law) are legitimate — but must say they have no upstream counterpart rather than implying one.

Every `#[test]` needs a doc comment citing its source and an `UPSTREAM.md` row **in the same commit**.

Test shapes in use: unit tests per module, manifest-driven round-trip corpus, trybuild compile-fail fixtures (a new type-level law lands with a fixture proving the wrong program does not compile — bless `.stderr` only on `+1.96.0`), byte-lock tests on examples, and `proptest` generative coverage.

## Commits

Conventional Commits — `type(scope): summary`, `!` after the scope for a breaking change. Cite the doctrine id when a change turns on one. Every user-visible change gets a `CHANGELOG.md` entry; the project is pre-1.0, so breaking changes are expected and flagged inline. Version labels must not overclaim — pre-1.0 means the next patch version, never a marketing number.

## Skills

Project skills live in `.claude/skills/`. Reach for them by the moment you are in, not by topic:

- about to read `orig_cpp/` to decide what llvmkit should do — a port, a diagnostic change, a parity review, or explaining why the two answer differently → `porting-from-orig-cpp`
- about to write a number, or an "all / every / none" sentence, into a tracked file, a commit message or a report → `claims-and-counts`

## Where the detail lives

Each entry names the question it answers — read the section, not the file.

- `AGENTS.md` § *Rust Idioms & Translation Patterns* — how a C++ sentinel, out-parameter, union or `assert` is spelled here. Also: full API laws, workstream history, the porting anchor tables for each LLVM subsystem.
- `README.md` — user-facing docs, authoritative Doctrine D1–D11 prose.
- `ROADMAP.md` — milestones, release sequence, the crates.io checklist.
- `docs/future-work.md` — the live backlog: what is known-missing, what was deferred, and **why** in each case. Read before proposing work that looks unfinished; it is often deliberate.
- `docs/divergences.md` — the behavioural-difference ledger: every place llvmkit's observable behaviour differs from the vendored tree, graded by severity (`rejects-valid` is the worst). Distinct from `future-work.md`, which is unimplemented features. Every entry **and its evidence block** is a hypothesis with a citation, not a fact.
- `docs/fixture-coverage.md` — all 500 `llvm/test/Assembler` fixtures classified `ported` / `blocked-model` / `N/A`, each blocked row naming its gap. The completeness proof behind the corpus manifest.
- `UPSTREAM.md` — per-test provenance registry. Coverage is **not** total and the header says so: 2560 tests, 2127 rows, 320 tests with no row, all inherited from the type-safety and pass-API programs. A missing row means missing *provenance*, never "no upstream counterpart".
- `docs/inkwell-migration.md` — per-API delta against inkwell.

## Before you start

1. **Read the reference before editing.** Porting `Foo` means opening the C++ header *and* the `.cpp` — the invariants live in the `.cpp`.
2. **Read the current signature, not a doc's memory of it.** The API settled at 0.0.4 after a broad reshape; any prose can lag the tree. `grep` the `pub fn` before calling it.
3. **Prefer one well-modeled subsystem over many half-modeled ones.** Do not add empty stub files.
4. **Surface uncertainty.** If a C++ behavior is ambiguous, state the choice and the rationale in a comment rather than silently picking.
