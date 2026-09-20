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
| [Leftover API-rewrite work](leftover-api-rewrite-work.md) | The residue of the handle-and-error rewrite: what it reached, understood, and deliberately did not close. Narrower than `future-work.md` — every entry exists because that program's own changes created or exposed it. |

Start with the repository [`README.md`](../README.md) for the quick tour and
[`CHANGELOG.md`](../CHANGELOG.md) for the 0.0.4 migration — each break is spelled
out under the cycle that made it.

Three more repository-root documents are not in the table above because they are
not user guides, but they are where a specific kind of question is answered:

| Document | Answers |
|---|---|
| [`AGENTS.md`](../AGENTS.md) | The exhaustive internal reference — API laws, the C++-to-Rust translation idioms (how a sentinel, out-parameter, union or `assert` is spelled here), the per-subsystem porting anchor tables, and the per-workstream history. |
| [`UPSTREAM.md`](../UPSTREAM.md) | Which upstream test, fixture or reference each llvmkit test came from. Coverage is a ratchet, not complete; the file says so and names the frozen debt list. |
| [`ROADMAP.md`](../ROADMAP.md) | What ships in which release, and the crates.io checklist. |

## Design records

[`design/`](design/) holds the specs for subsystems that have shipped — with one
exception noted in the table. They are kept because they record **why the code
looks the way it does**, and a shipped one carries a dated *Shipped as* note
wherever the implementation diverged from the design, which is the part you
cannot reconstruct from the code.

Read them as history. Where one describes an API, the source is the authority.

| Record | Subsystem |
|---|---|
| [Pass-facing type safety](design/pass-facing-type-safety.md) | The four capability rungs, the pattern-matcher DSL, pass ergonomics, and framework-witnessed analysis preservation. Also the index for the three below. |
| [Phi type-level guarantees](design/phi-type-guarantees-design.md) | Block arguments as the public phi-authoring surface, and the raw phi builders going internal. |
| [Unforgeable markers](design/unforgeable-markers-design.md) | The sanctioned-constructor family, and why the seal shipped *audited* rather than compiler-enforced. |
| [Worklist and erase-safe cursor](design/worklist-erase-safe-cursor-design.md) | The mutation-driven worklist that made `DcePass`/`InstSimplifyPass` amortized-linear without changing a byte of output. |
| [Algebraic `ParseError`](design/parse-error-algebraic-design.md) | The parser's error type as `{ kind, loc }`: no `Cow` text carrier, no I/O variants, and a drift test against the vendored `LLParser.cpp`. Design only — not yet executed. |

## Not in this directory

`docs/superpowers/` is git-ignored working material and is **not** part of the
repository — if a document points you there, the content is unavailable, and
that is a bug worth reporting.

`docs/` also sits at the workspace root, outside every package directory, so it
is **not** included in the published `.crate` and does not appear on docs.rs.
It is a GitHub-facing tree; API documentation ships as rustdoc.
