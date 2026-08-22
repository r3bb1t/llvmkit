# llvmkit documentation

Two tiers. Everything at this level describes the library **as it is now**;
everything under [`design/`](design/) is a **dated record** of how a decision was
reached, correct as of its date and not maintained afterwards.

## Current

| Document | What it is |
|---|---|
| [Type Safety: llvmkit vs. LLVM C++](type-safety-vs-llvm.md) | The main technical reference. Maps LLVM C++ failure modes to the llvmkit type that forecloses each one, with worked "bad program" samples, the Doctrine IDs (D1–D11), and — importantly — where a guarantee is a *compile* error versus a `verify()` check. |
| [`IrStruct` derive macro](ir-struct-derive.md) | User guide for `#[derive(IrStruct)]`: generated wrappers, `StructFields<S>`, the helper attributes, and the struct shapes it rejects. |
| [Migrating from inkwell](inkwell-migration.md) | Per-API delta against the [`inkwell`](https://crates.io/crates/inkwell) crate: the three structural differences, then a row-by-row mapping of inkwell's spelling to llvmkit's. Read this before porting an existing codebase. |
| [Future work](future-work.md) | The live backlog: known gaps, deferred items, and the reasoning behind each deferral. Several rustdoc comments in `crates/` point here for *why* a hole is still open. |
| [Known divergences from LLVM](divergences.md) | The behavioural-difference ledger, graded by severity. Each entry is a hypothesis with a citation, not a fact — and so is its evidence block. |
| [`llvm/test/Assembler` coverage](fixture-coverage.md) | All 500 upstream fixtures classified `ported` / `blocked-model` / `N/A`, with the named gap per blocked row. |

Start with the repository [`README.md`](../README.md) for the quick tour and
[`CHANGELOG.md`](../CHANGELOG.md) for the 0.0.4 migration — each break is spelled
out under the cycle that made it.

## Design records

[`design/`](design/) holds the specs for subsystems that have already shipped.
They are kept because they record **why the code looks the way it does**, and
each carries a dated *Shipped as* note wherever the implementation diverged from
the design — which is the part you cannot reconstruct from the code.

Read them as history. Where one describes an API, the source is the authority.

| Record | Subsystem |
|---|---|
| [Pass-facing type safety](design/pass-facing-type-safety.md) | The four capability rungs, the pattern-matcher DSL, pass ergonomics, and framework-witnessed analysis preservation. Also the index for the three below. |
| [Phi type-level guarantees](design/phi-type-guarantees-design.md) | Block arguments as the public phi-authoring surface, and the raw phi builders going internal. |
| [Unforgeable markers](design/unforgeable-markers-design.md) | The sanctioned-constructor family, and why the seal shipped *audited* rather than compiler-enforced. |
| [Worklist and erase-safe cursor](design/worklist-erase-safe-cursor-design.md) | The mutation-driven worklist that made `DcePass`/`InstSimplifyPass` amortized-linear without changing a byte of output. |

## Not in this directory

`docs/superpowers/` is git-ignored working material and is **not** part of the
repository — if a document points you there, the content is unavailable, and
that is a bug worth reporting.

`docs/` also sits at the workspace root, outside every package directory, so it
is **not** included in the published `.crate` and does not appear on docs.rs.
It is a GitHub-facing tree; API documentation ships as rustdoc.
