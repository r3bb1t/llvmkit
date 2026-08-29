# Future work

The live backlog: what is known-missing, what was deliberately deferred, and
**why** in each case. Several rustdoc comments in `crates/` point here rather
than restating a deferral inline, so entries are written to be read cold.

> **Behavioural differences from LLVM live in [`divergences.md`](divergences.md).**
> This file is the backlog — work not done. That one is the ledger of places
> where llvmkit is *observably* different from the vendored reference: input
> accepted or rejected differently, different diagnostic text, different
> printed bytes, a public query answering differently. When an entry here is
> also a divergence, it is cross-referenced rather than duplicated.

Each item cites its source — a file and symbol, an upstream reference, or the
cycle that decided it. Items that later shipped are struck through and dated
rather than deleted, so a reader can tell "never done" from "done, here is what
actually landed".

It began as the residue of the `feature-1/irbuilder-type-safety` audits and has
accumulated every cycle since; the oldest sections are still organised that way.

## LLParser diagnostics — 46 real gaps left of 516 messages (recounted 2026-08-16, LLParser parity W14d; 8 closed since, see below)

The parity ledger was regenerated at this commit. Of the **516 exact message
literals** `LLParser.cpp` reaches through its five diagnostic channels
(`tokError`, `error`, `parseToken`, `checkValueID`, `parseValueAsMetadata`),
**466 are covered** and **50 are not**. Of those 50, **4 are `N/A`** and
**46 are real gaps**.

The four `N/A`s, each checked against `LLParser.cpp` rather than assumed:

- `Can't read textual IR with a Context that discards named Values` — no
  discard-names mode exists to check (D10 in [`divergences.md`](divergences.md)).
- `argument can not have void type` — dead upstream: `parseArgumentList` reads
  the type with `AllowVoid = false`, so `parseType` has already refused a
  literal `void`.
- `non-sanitizer token passed to LLParser::parseSanitizer()` — assert-only; the
  sole call site is guarded by `isSanitizer(Lex.getKind())`.
- `name is too long …` — see the divergence below.

The remainder are almost all one shape: llvmkit runs the check at the same point
and words it differently (`expected '>' at end of packed struct` where upstream
says `expected '}' at end of struct`). They are concentrated in `parseValID`'s
blockaddress and vector-splat forms (5), `parseArrayVectorType` (4) and `parsePHI`
(2). **The count of 46 predates 0.0.4's funclet work** (2026-08-20), which
closed the `parseCatchSwitch` (4) and `parseCleanupRet` (3) messages this
paragraph used to count under "funclet EH terminators", plus
`expected 'from' after catchret` — the example this paragraph used to lead
with. That is 8 of the 46 by this paragraph's own arithmetic, so the live
figure should be 38; it has **not** been re-derived, because `ledger_v2.py` is
not in the tree (`find . -name 'ledger*.py'` returns nothing). One further
message closed after that, on 2026-08-21: `expected 'tail call', 'musttail
call', or 'notail call'`, when `LLParser::parseCall`'s first guard was ported
(it had been not merely missing but a verdict difference — llvmkit accepted
`tail void @f()`). Re-run the ledger before quoting either number. This overlaps gap **G17** in [`fixture-coverage.md`](fixture-coverage.md)
but is not the same set: G17 counts *fixtures* that fail on wording, this counts
*messages*. The full per-message list, with llvmkit's current spelling beside
each, is the classification section of the parity ledger.

### The ledger tool undercounted, twice, and the recorded history is wrong

Two defects in `ledger_v2.py`, both found here, both of which had been silently
deflating the score:

1. **Its llvmkit-side extractor was a regex over `"..."`.** A lone `"` inside a
   `//` comment desynchronizes it for the rest of the file, and llvmkit quotes
   upstream prose that way — `ll_parser.rs` has one at the `DIExpression` arm of
   `parse_named_metadata`, so every literal after it was invisible. It now
   lexes Rust properly (line and nested block comments, raw/byte/C strings, char
   literals, lifetimes).
2. **It harvested only `#[error("…")]` from `llvmkit-ir`.** The ptrauth constant
   checks carry their text in a plain `message:` field of a struct variant, so
   five messages llvmkit *does* emit were counted missing. It now reads every
   literal in `llvmkit-ir/src`, as it already did for the parser crate.

A third effect is not a defect but was mistaken for one: llvmkit collapses
upstream's per-opcode copies into one interpolated message, so
`{opcode} constexprs are no longer supported`, `expected scope value for {pad}`
and `invalid type for {what} constant` cover **34** upstream literals that no
literal search can match. The ledger now has a `[~]` template column for these.
With it applied to the lexer section too, **`LLLexer.cpp`'s 11 messages are
11/11 covered** — the last two, `hexadecimal constant too large for half/bfloat
(16-bit)`, come from one `#[error("… for {} (16-bit)", .target.upstream_name())]`
in `ll_lexer.rs` and had been reading as gaps.

**Consequence for the record: the "411 present / 105 missing" figure carried in
the program's notes was never right.** Re-measuring the W11 commit with the
fixed extractor gives 412/104, and the W12/W13 commits give 427/89 — where the
old tool reported 335, which would have read as a 76-message regression that
never happened. Numbers from before this commit should not be compared with
numbers after it.

### `-non-global-value-max-name-size` has no counterpart (divergence, not just a message)

`getVal`'s `name is too long which can result in name collisions…` fires only
when `ValueSymbolTable` renamed a value on insert, which it does when the name
exceeds `MaxNameSize` — fed by `NonGlobalValueMaxNameSize` in `lib/IR/Function.cpp`.
That option is `cl::init(1024)`, **not** off by default, so this is a real
behavioural difference and not merely an unimplemented flag: LLVM truncates and
then rejects a function-local name longer than 1024 characters, and llvmkit
accepts it unchanged. Closing it means giving llvmkit's function-local symbol
table a name cap, not adding a string. Upstream pins the behaviour in
`test/Assembler/non-global-value-max-name-size.ll`, which drives it with an
explicit `-non-global-value-max-name-size=4`; that fixture is the single `N/A`
row in [`fixture-coverage.md`](fixture-coverage.md). (Its `-2.ll` sibling is
`ported` — under `-non-global-value-max-name-size=5` it only checks that
inlining does not *generate* an over-long label, which parses either way.)

### `UPSTREAM.md` — tests still without a provenance row

The registry recount that this wave owed is done. The residue is real and
inherited from the type-safety and pass-API programs; a missing row means
missing *provenance*, never "no upstream counterpart".

**Its size is not stated here, and no longer in `UPSTREAM.md`'s header either.**
Every version of that figure was carried forward by arithmetic from an older
audit rather than re-derived, and successive carries ended up disagreeing with
each other inside one paragraph. Nor is a naive re-derivation available: a
`sort -u` of the rows' `::name` segments against the `#[test]` attributes counts
every test covered by a *group* row — `` `…/module_ownership.rs` (whole file) ``
and its kind — as unrowed, and there are enough of those to dominate the answer.
A fresh audit means expanding those rows against the files they cite, by hand.
`UPSTREAM.md`'s header says the same and names what it would take.

What *is* mechanically checked, since 2026-08-22, is the half that can be:
`crates/llvmkit-ir/tests/upstream_registry_drift.rs` fails if any row names a
file that does not exist, or a test its cited file does not define — the failure
mode that had put eleven rows on the wrong file and survived earlier sweeps.

## `llvm/test/Assembler` — the blocked fixtures, by gap

[`fixture-coverage.md`](fixture-coverage.md) classifies every fixture in
`llvm/test/Assembler` as `ported`, `blocked-model` or `N/A`, and each blocked
row names one of its catalogued gaps. **That file is the backlog for this
area** — this section exists so the backlog points at it rather than restating
it, so no tally is repeated here. It used to repeat three, and they were a
branch out of date within a week of being written: the gap catalogue moves
whenever a gap closes, and only one file can be the source. Counts, per-gap
lists and the derivation command all live there.

The gaps with the most fixtures behind them, and why each is worth pulling out:

- **G18** — a check upstream's `llvm-as` performs at parse or verify time
  that llvmkit does not: `MDField` range bounds (`count`, `lowerBound`,
  `emissionKind`, `language`, `tag`), attribute applicability (`align` on a
  function, `byval` on an unsized type, `captures(none)` on a non-pointer), and
  target-extension-type legality.
- **G17** — diagnostic text that differs from upstream's. Most of it is one
  wording bug: a complete upstream message routed through llvmkit's
  `expected …` wrapper (`expected
  valid mask value for 'nofpclass'`, `expected intrinsic signature mismatch`),
  where upstream reaches it through `error(...)` rather than
  `tokError("expected …")`. This is the cheapest parity work left in the
  directory.
- **G1** — `AutoUpgrade` coverage, which the section below tracks in its
  own right.

One more worth pulling out because it is small and self-contained: **G22** — the two
fixtures `2004-02-27-SelfUseAssertError.ll` and `2004-06-07-VerifierBug.ll`, which
`llvm-as` accepts and llvmkit's Verifier rejects, because
`Verifier::verifyDominatesUse` returns early when
`!DT.isReachableFromEntry(...)` and llvmkit's dominance/self-use checks have no
such exemption. Both fixtures exist precisely to pin that unreachable-block
behaviour.

`docs/fixture-coverage.md` also records the provenance defect this measurement
turned up: citations in the tree name `test/Assembler/*.ll` files that do not
exist in the vendored tree, some of them nowhere under `llvm/test/` at all. The
tests are real; their cited source is not. Repointing them is open work. That
entry deliberately carries **no figure** — three were written and all three were
wrong, because the prose naming the phantom paths is itself counted by any sweep
that looks for them — so no figure is repeated here either.

## `llvm/test/Verifier` — no corpus manifest, so nothing measures the drift (found 2026-08-27, divergence-closing wave 5)

Every `VerifierRule` now carries upstream's `Check` literal at the head of
`IrError::VerifierFailure`'s `message`, so a `llvm/test/Verifier/*.ll` fixture
can be driven by its own `CHECK` lines — that was the point of closing the
"verifier diagnostics are house-worded" divergence, and it is why
`test/Verifier/SelfReferential.ll` is now a port rather than a paraphrase.

What did **not** land with it is the gate: there is no `test/Verifier`
counterpart to the manifest `crates/llvmkit-asmparser/tests/parser_corpus.rs`
drives over `test/Assembler`. Each vendored `test/Verifier` fixture is
`include_str!`ed by one hand-written test, so a rule whose literal drifts is
caught only where a test happens to look, and a fixture nobody vendored is
invisible. The absence of exactly this measurement is what let the whole
verifier drift from `Verifier.cpp` unnoticed in the first place; a per-fixture
test is a narrower answer than a manifest.

Wanted, in this order:

1. A `test/Verifier` classification the way
   [`fixture-coverage.md`](fixture-coverage.md) classifies `test/Assembler` —
   `ported` / `blocked` / `N/A`, each blocked row naming its gap. Derive it,
   do not estimate it; the directory listing is the denominator.
2. A manifest-driven runner over the vendored subset: parse, verify, and run
   the fixture's own `CHECK` block through
   `crates/llvmkit-asmparser/tests/support/mod.rs`'s FileCheck subset. The
   per-fixture tests collapse into rows.
3. A rule-coverage check: every `VerifierRule` variant either appears in the
   manifest's expectations or carries a comment at its check site saying it has
   no upstream `Check` literal to reproduce. Those comments exist today
   (`check_freeze`, `check_va_arg`, `check_cmpxchg`, `check_switch`'s condition
   arm, the arena-level result-type guards); nothing enforces that a *new* rule
   writes one.

Two fixtures are vendored and waiting on a divergence rather than on this work:
`PhiGrouping.ll` (entry 26 — the parser pre-empts the rule) and
`AmbiguousPhi.ll` (entry 130 — the builder does). Both assert their blocker
today, so they fail the day it closes.

## AutoUpgrade — five of nine `validateEndOfModule` call sites still open (measured 2026-08-29, divergence-closing W10)

`crates/llvmkit-ir/src/auto_upgrade.rs` exists now and carries the
target-independent, module-level half of `llvm/lib/IR/AutoUpgrade.cpp`. This
records what is still missing, and what each piece is blocked on.

### The count, verified

`LLParser::validateEndOfModule` contains **nine** `AutoUpgrade.h` entry points,
not the eight the program plan lists. The plan's own prose names nine symbols;
only the number is wrong. In call order:

| # | upstream symbol | status |
|---|---|---|
| 1 | `UpgradeIntrinsicFunction` (from the `ForwardRefVals` sweep) | not ported |
| 2 | `UpgradeIntrinsicCall` (same sweep) | not ported |
| 3 | `UpgradeTBAANode` (over `InstsWithTBAATag`) | **ported (W13d)** |
| 4 | `UpgradeCallsToIntrinsic` (per `Function`) | not ported |
| 5 | `llvm::UpgradeDebugInfo` | not ported |
| 6 | `UpgradeModuleFlags` | **ported (W13d)** |
| 7 | `UpgradeNVVMAnnotations` | **ported (W10)** |
| 8 | `UpgradeSectionAttributes` | **ported (W13d)** |
| 9 | `copyModuleAttrToFunctions` | not ported |

`copyModuleAttrToFunctions` is declared in `llvm/include/llvm/IR/AutoUpgrade.h`
like the other eight, which is why it counts.

### 1/2/4 — the intrinsic-upgrade framework

The largest item, and the one every clang-21 module needs
(`llvm.lifetime.start/end` still ship with the dropped size argument). It is
**not** blocked on transcription effort but on where llvmkit puts the check:

* `LLParser::parseDeclare` builds the `Function` and lets the *verifier* judge
  the signature. llvmkit's `Parser::parse_declare` resolves the name through
  `resolve_intrinsic_name`, then demands
  `intrinsic_descriptor_from_signature` succeed — so
  `declare void @llvm.lifetime.start.p0(i64, ptr)` is rejected at parse with
  `intrinsic signature mismatch` before any upgrade could see it. Same for a
  call: `Parser::resolve_direct_callee` compares `f.signature()` against the
  parsed type and errors on disagreement.
* Upstream's `upgradeIntrinsicFunction1` therefore runs against a `Function`
  that llvmkit refuses to create. Porting the arms is only useful once the
  legacy signature can *exist* long enough to be upgraded, which means moving
  the descriptor check out of `parse_declare` /`resolve_direct_callee` and into
  the end-of-module path — the same restructuring `docs/divergences.md`
  entries 15 and 19 describe.
* `UpgradeIntrinsicCall`'s generic arms then need instruction-level surgery
  llvmkit has only in part: `Instruction::erase_from_parent` and
  `replace_all_uses_with` exist, but there is no way to erase a `Function` from
  a module (`F->eraseFromParent()`), and no builder entry point that inserts a
  call *at* an existing instruction's position outside the pass framework.
  The lifetime arm needs both, plus `stripPointerCasts` (present, but
  `pub(crate)` in `pointer_analysis.rs`).

The generic (non-target) arms of `upgradeIntrinsicFunction1`, extracted so the
next pass does not have to re-read 600 lines of C++: `ctlz.`/`cttz.` with one
argument; `coro.end` with two; the five `dbg.*` names; `experimental.vector.`
{extract, insert, reverse, interleave2, deinterleave2, partial.reduce.add},
`experimental.vector.reduce.*`, `experimental.vector.splice`,
`experimental.stepvector.`; `flt.rounds`; `invariant.group.barrier`;
`lifetime.start`/`lifetime.end` with two arguments; `memcpy.`/`memmove.`/
`memset.` with five; `masked.{load,gather,store,scatter}` with four;
`objectsize.` with two or three; `ptr.annotation.` with four;
`stackprotectorcheck`; `thread.pointer`; `var.annotation` with four;
`vector.splice` (excluding `.left`/`.right`); then the two tail rules —
literalising a non-literal struct return type, and
`remangleIntrinsicFunction`. Everything under `case 'a'`, `'n'`, `'r'`, `'w'`
and `'x'` is target-specific and belongs to the milestone below.

### 5 — `llvm::UpgradeDebugInfo`

Blocked on `StripDebugInfo(Module&)` (`lib/IR/DebugInfo.cpp`), which llvmkit
does not have, and on a verifier that can report *broken debug info*
separately from a broken module (`verifyModule(M, &OS, &BrokenDebugInfo)`).
Porting only the "read the Debug Info Version flag" half would be a silent
no-op, which the fallibility rule forbids — so it stays out until the strip
exists. This is also why `ParserConfig::upgrade_debug_info` (W13c) still
selects nothing: it is the flag on the call site that does not exist yet, and
`docs/divergences.md` D11 counts it among the settings that read as inert.

### 7 — `UpgradeNVVMAnnotations` — closed (W10)

Ported as `auto_upgrade::upgrade_nvvm_annotations` with
`upgrade_single_nvvm_annotation`, `upgrade_nvvm_fn_vector_attr` and `is_xyz`,
wired between `upgrade_module_flags` and `upgrade_section_attributes`.
`test/CodeGen/NVPTX/upgrade-nvvm-annotations.ll` passes whole.

This entry used to record three blockers — `NamedMDNode::clearOperands`,
`Function::addParamAttr` at a computed index, and `CallingConv::PTX_Kernel`.
**Two of the three were wrong**, which is the worked example of why a recorded
reason is a hypothesis: `CallingConv::PTX_KERNEL` had been in
`crates/llvmkit-ir/src/calling_conv.rs` since the table was written, and
`addParamAttr` at a computed index is `FunctionValue::add_attribute` with
`AttrIndex::Param`. Only `clearOperands` was genuinely absent. A primitive the
record did *not* name was also needed and is now present:
`mdconst::dyn_extract_or_null<GlobalValue>`, as
`Module::metadata_constant_global_value`.

### 9 — `copyModuleAttrToFunctions`

Blocked on a `Triple`. The routine's first statement is
`Triple T(M.getTargetTriple()); if (!T.isThumb() && !T.isARM() && !T.isAArch64()) return;`,
and llvmkit models the target triple as an opaque `Option<String>` — there is
no `llvm/lib/TargetParser/Triple.cpp` port anywhere in the tree, and
`parseARMArch` alone is a table of its own. It also needs
`AttrBuilder::removeAttribute(StringRef)` — `AttributeStorage::remove` takes an
`AttrKind`, so a *string* function attribute cannot be removed today.

### The target-specific rewrite bodies — a named milestone

Out of scope for the parity program and recorded here as the milestone the
2026-08-07 decision named: the x86/ARM/AArch64/AMDGPU/NVVM/RISC-V/WebAssembly
intrinsic rewrite bodies (the bulk of `AutoUpgrade.cpp`'s 6,646 lines), plus
`UpgradeARCRuntime`, `UpgradeBitCastInst` / `UpgradeBitCastExpr`,
`UpgradeInlineAsmString` and `UpgradeDataLayoutString`. Ledger rows satisfiable
only by target-specific legacy input keep `N/A(autoupgrade-milestone)`.

### Blocked fixture ports

`test/Assembler/autoupgrade-lifetime-intrinsics.ll`,
`auto_upgrade_intrinsics.ll`, `autoupgrade-thread-pointer.ll`,
`autoupgrade-wasm-intrinsics.ll`, `autoupgrade-invalid-mem-intrinsics.ll`,
`autoupgrade-invalid-masked-align.ll`, `autoupgrade-invalid-name-mangling.ll`,
`struct-ret-without-upgrade.ll` and `auto_upgrade_nvvm_intrinsics.ll` all need
the intrinsic framework above; none was trimmed to fit. Note that
`auto_upgrade_nvvm_intrinsics.ll` is about NVVM *intrinsics*, not
`!nvvm.annotations`, so call site 7 closing does not unblock it. What has been
ported so far, with each row's fidelity spelled out in `UPSTREAM.md`:
`test/Bitcode/upgrade-module-flag.ll`,
`upgrade-garbage-collection-for-objc.ll`, `upgrade-section-name.ll` and
`test/CodeGen/NVPTX/upgrade-nvvm-annotations.ll` whole, plus
`upgrade-garbage-collection-for-swift.ll`'s four `CHECK` lines (its
typed-pointer function body is named by no `CHECK`) — see
`crates/llvmkit-asmparser/tests/parser_auto_upgrade.rs`.

## The gate is ~90% build, and trybuild builds `dev` whatever you ask for (measured 2026-08-16)

Two findings from one gate run, both measured rather than estimated. They
matter together: the second is why the first cannot be fixed by the obvious
lever.

### trybuild ignores `--release`

`trybuild` (1.0.116) does not compile fixtures in-process. It synthesises a
scratch package — `target/tests/trybuild/llvmkit-ir/`, named
`llvmkit-ir-tests` — and shells out to `cargo build` inside it **with no
profile flag**, so every registered fixture always builds `dev`. The gate
output says so in the middle of a `--release` run:

```text
Compiling llvmkit-ir-tests v0.0.0 (…\target\tests\trybuild\llvmkit-ir)
 Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.71s
```

So the standing "release builds only, delete `target/debug`" rule has always
had a hole in it, invisible because the artifacts are not under
`target/debug`:

| | |
|---|---|
| `target/release` | 5537 MB |
| `target/debug` | absent — the rule holds here |
| `target/tests/trybuild` | **625 MB, unoptimized + debuginfo** |

There is no supported knob to make trybuild build release. Two consequences
worth carrying: `target/tests` is a real disk consumer and should be swept
alongside `target/debug`; and every gate already pays for a full dev-profile
compile of `llvmkit-ir` inside that scratch crate, work the release build
cannot share because the scratch project has its own target directory.

### Where the gate's time actually goes

Measured at the W12 boundary, when the suite was 212 binaries: **test execution
totals 109.05 s**, and only 22 binaries have a non-zero time at all. The rest
of a ~17-minute gate is compile and link. The ratio is what matters and it does
not move with the binary count — the suite is 214 binaries as of W14 and the
execution time is dominated by the same handful below. The slowest five:

| secs | tests | binary |
|---|---|---|
| **66.07** | **1** | `typestate_compile_fail` |
| 9.91 | 19 | `known_bits` |
| 9.28 | 6 | `constant_range_dispatch` |
| 7.44 | 9 | `llvmkit_tablegen` (lib) |
| 7.13 | 4 | `constant_range_saturating` |

> **Re-measured at W14d: 214 binaries, 65.48 s, 21 with a non-zero time.** The
> shape of the finding is unchanged but the total is not comparable — that run
> had a warm trybuild scratch crate, which drops `typestate_compile_fail` from
> 66.07 s to 26.49 s and accounts for essentially the whole difference. The
> table above is the cold-cache number and is the one that describes a CI run;
> keep both in mind before quoting either. On the binary count: the tree holds
> 193 integration-test files against 192 at `bd90449`, the one addition being
> W14b's `lexer_token_drift.rs`; the earlier 212 predates that and one other
> target added before it.

The 66 s entry is one `#[test]` that is really 87 `rustc` invocations plus that
dev-profile dependency build. It is on the slow `cargo build` path rather than
`cargo check` **deliberately**, and the reason is in the test's own doc
comment: `extract_value_empty_indices.rs` asserts a `const { assert!(N > 0) }`
(D3) whose `E0080` is a monomorphisation-time diagnostic, and under `check`
that fixture reports "succeeded" — verified empirically. Speeding this up by
switching to `check` would silently stop testing a type-level law.

The `known_bits` / `constant_range_*` cost is inherited: they are ports of
upstream's `KnownBitsTest` and `ConstantRangeTest`, which verify by exhaustive
enumeration over small bit widths (43 exhaustive loops in `known_bits.rs`
alone). Cutting it means weakening the port.

**The build cost, not the test cost, is the target.** The workspace has no
`[profile]` section and no `.cargo/config.toml`, so `--release` means cargo's
defaults: `opt-level = 3`, `codegen-units = 16`, and incremental **off**
(release disables it). And there are **192 integration test files**, each built
as its own crate and linked as its own binary against the whole `llvmkit-ir`
rlib. Three levers, unranked because none has been measured against the others:

1. **Consolidate the 192 binaries into ~10** (`mod` per area). Removes the
   dominant cost rather than shrinking it. Does not touch trybuild.
2. **A profile with `debug = false` and a lower `opt-level`.** The disk rule is
   about debuginfo, not optimisation, so this preserves it. Note that dropping
   to a dev-like profile turns on `debug_assert!` and overflow checks — argued
   as a *benefit* for this codebase, since it forbids `as` casts and uses
   `wrapping_*`/`saturating_*` deliberately, but it can surface new failures
   that are findings rather than regressions.
3. **`rust-lld` via `.cargo/config.toml`.** Link-bound builds on Windows
   typically gain; costs nothing semantically.

`cargo build --timings` would settle the split between codegen and link, and
has not been run.

## Summary index — printing a module and its index is two calls (found 2026-08-15, LLParser parity W10)

`llvm-dis` prints a module and its summary index together because
`AssemblyWriter` is handed both, and `printModuleSummaryIndex` runs after
`printModule`. llvmkit keeps them apart: the index comes back in
`ParsedModule::summary_index`, it is not attached to the `Module`, and
`format!("{module}")` therefore never emits `^N` entries. A caller reproducing
`llvm-dis` writes the module and then the index.

That is a plumbing difference, not a byte difference — `Display for
ModuleSummaryIndex` reproduces `printModuleSummaryIndex` exactly, leading blank
line included. It is recorded so the absence of `^N` from a printed module is
not read as a missing feature. Attaching the index to the module would mean
giving `Module` a field that only the parser ever fills, which is a bigger
question than W10 needed to answer.

## Summary index — two things the `.ll` surface cannot reach (found 2026-08-15, LLParser parity W10)

Both are recorded so a later reader can tell "deliberately outside the textual
surface" from "missed".

**`AllocationType` models the four values the parser accepts, not the ORed
set.** Upstream's `enum class AllocationType : uint8_t` has `None = 0`,
`NotCold = 1`, `Cold = 2`, `Hot = 4` and `All = 7`, and the values are powers
of two *so that a context reaching an allocation more than one way can OR
them*. `AllocInfo::Versions` is a `SmallVector<uint8_t>` for exactly that
reason. But `LLParser::parseAllocType` reads one of four keywords, and
`AssemblyWriter::printFunctionSummary`'s `AllocTypeName` lambda handles those
four and `llvm_unreachable`s on anything else — so an ORed value can only come
from bitcode, which llvmkit does not have. The enum is the `.ll` surface,
exactly.

**`test/Assembler/thinlto-vtable-summary2.ll` is not ported.** Its `RUN` line
is `opt %s -S -module-summary`, which *generates* a summary index from the
module's type metadata rather than parsing one; there is no `^N` block in the
input at all. That is the module-summary analysis, not the parser, and llvmkit
has neither. The other sixteen summary fixtures in `test/Assembler` are ported
in `crates/llvmkit-asmparser/tests/parser_summary.rs`.

## llvmkit-ir — three copies of the aggregate index walk (found 2026-08-14, LLParser parity W9c)

`ExtractValueInst::getIndexedType` now has a public port,
`llvmkit_ir::indexed_aggregate_type` (`instructions.rs`), because the parser
needs it for `invalid indices for {extract,insert}value`. Two private
near-copies already existed and were left in place:

- `ir_builder.rs::walk_aggregate_for_builder`
- `verifier.rs::walk_aggregate_path` (which additionally distinguishes
  *why* the walk failed, via `AggWalkErr`)

All three implement the same upstream routine. This is the shape W4 found with
`type_contains_scalable_vector` — three private copies, none matching the
upstream predicate exactly — and the fix is the same: one port, three callers.
The verifier's error-distinguishing variant needs a richer return type than
`Option`, so the consolidation is not a pure deletion.

Not urgent: all three currently agree. It is recorded because a predicate with
three implementations is one diagnostic away from having three behaviours.

## Parser — `resolve_direct_callee` returns a three-way sum where `convertValIDToValue` returns one `Value *` (found 2026-08-21, divergence-closing task 6)

`LLParser::convertValIDToValue` switches over `ValID::Kind` internally and
writes **one** `Value *Callee` through its out-parameter. `parseCall`,
`parseInvoke` and `parseCallBr` each then run a single construction tail —
`CallInst::Create` / `InvokeInst::Create` / `CallBrInst::Create` — with no
direct/indirect/inline-asm distinction anywhere.

llvmkit's `resolve_direct_callee` returns `ParsedCallee::{Function, InlineAsm,
Indirect}` and hands the fork to its caller. `parse_call` no longer forks — it
calls `ParsedCallee::as_erased` and then one `IrBuilder::call_erased` — but
`parse_invoke` and `parse_callbr` still `match` on the variant and dispatch
per callee shape, to a separate builder entry point each.

The remaining fork has no observable behaviour of its own, so it is recorded
here rather than in [`divergences.md`](divergences.md): every builder entry
point those two arms reach takes a `CallSiteConfig`, so the same information
reaches the instruction on every path that builds one. The risk is structural, and it is not hypothetical
— it is the shape that let `parse_call` lose every call-site attribute on two
of its three arms without a compiler warning, because `call_attrs` *was* moved
into the one arm that used it.

**The fix:** change `resolve_direct_callee`'s return type to
`llvmkit_ir::Value<'ctx, B>` and delete `enum ParsedCallee` along with the two
remaining `match`es. `ParsedCallee::as_erased` is the transitional shim and
goes with it.

**Why it is deferred:** the blocker named here — `parse_callbr` needing the
directness distinction to keep a parse-time rejection of an indirect `callbr`
— is gone: `IrBuilder::indirect_callbr_with_config` landed in W8 and the
rejection moved to `Verifier::visitCallBrInst` where upstream has it. What is
left is the mechanical half: each of the two remaining `match`es reaches three
*different* builder entry points, and collapsing them means routing `invoke`
and `callbr` through one erased-callee constructor apiece, the way
`call_erased` already serves `parse_call`. That is its own commit.

## IR builder — three call-site builders accept a `call_site_type` override and ignore it (found 2026-08-21, divergence-closing task 6)

`IrBuilder::indirect_invoke_dyn_with_config`,
`IrBuilder::inline_asm_invoke_with_config` and
`IrBuilder::inline_asm_callbr_with_config` each take a `CallSiteConfig`, which
carries an optional call-site function type set by
`CallSiteConfig::call_site_type`. None of the three reads it: the
indirect-invoke form uses its own `fn_ty` parameter and the two inline-asm
forms use `asm.function_type()`. A caller that sets the override gets no error
and no effect — the exact shape `CLAUDE.md` bans ("never a silent no-op or
swallowed error"). The declared-callee siblings `invoke_dyn_seeded` and
`callbr_with_config` do honour it, through `resolve_call_site_type`, and
`call_erased` honours it through `resolve_call_site_type_for_erased_callee`.

This is llvmkit's own API surface rather than an upstream behaviour, and no
caller in the tree sets `call_site_type` on those three paths — `parse_invoke`
and `parse_callbr` pass the call-site type positionally — so it is recorded
here rather than in [`divergences.md`](divergences.md). It is reachable by any
external caller.

**The fix:** route the three through `resolve_call_site_type_for_erased_callee`,
with `asm.function_type()` as the fallback for the two inline-asm forms.

**Why it is deferred:** that is a behaviour change for any caller that was
setting the field, on three entry points unrelated to the `call` construction
the same commit rewrote. It wants its own commit rather than a rider.

## An upstream calling-convention bug, reproduced (found 2026-08-13, LLParser parity W6)

Found by the round-trip drift lock in `calling_conv_drift.rs`, so the choice is
recorded here.

**Reproduced: bare `riscv_vls_cc` consumes the following token.**
`parseOptionalCallingConv`'s `kw_riscv_vls_cc` arm calls `Lex.Lex()` itself and
then, when no `(` follows, `break`s to the switch's shared tail — which calls
`Lex.Lex()` again. Every other arm reaches that tail without having consumed
anything. So `define riscv_vls_cc void @f()` loses its return type upstream,
and now here too (`Parser::parse_riscv_vls_calling_conv`). It is unreachable
from printed IR, because `printCallingConv` writes those twelve conventions
only as `riscv_vls_cc(<N>)`. Reproduced because the contract is upstream's
behaviour, not its intent — revisit if upstream fixes it.

The second finding recorded here — that the numeric fallback should print
`cc 11` rather than `printCallingConv`'s `cc11`, because `LLLexer` would read
`cc11` as one unknown identifier — rested on a false premise and is gone.
`LLLexer::LexIdentifier` rewinds a word opening `cc` to `kw_cc`, which is why
`test/Bitcode/compatibility.ll` round-trips `declare cc11 void @f.cc11()`
through `llvm-as | llvm-dis` unchanged; `Lexer::lex_identifier` ports the same
rewind. The printer now writes `cc11` too.

## Printer — no option surface, so `printAddressSpace`'s symbolic branch cannot be reached (found 2026-08-21, `call addrspace(N)` port)

`printAddressSpace` (`lib/IR/AsmWriter.cpp`) prints `addrspace("global")`
instead of `addrspace(2)` when the datalayout named that address space *and*
`PrintAddrspaceName` is set — `static cl::opt<bool> PrintAddrspaceName(
"print-addrspace-name", cl::Hidden, cl::init(false), …)`, which `llvm-dis`
exposes as `--print-addrspace-name`. llvmkit's `print_address_space`
(`crates/llvmkit-ir/src/asm_writer.rs`) ports the `else` half only, and drops
upstream's `const Module *M` parameter, which exists solely to feed the branch.

**Not a divergence.** The flag's default is `false`, so no llvmkit input yields
different bytes from `llvm-dis`'s default; this is a feature llvmkit does not
have, which is why it lives here and not in
[`divergences.md`](divergences.md). The *data* is modelled —
`DataLayout::address_space_name` mirrors `getAddressSpaceName` and is tested by
`data_layout_round_trip.rs::address_space_name`, a port of
`unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, AddressSpaceName)`. What is
missing is a printer-option surface, which is a design question a one-flag port
should not settle unilaterally: a `PrintOptions` struct threaded through
`Display`? a second entry point beside `format!("{module}")`? The `Display`
impls are the whole public printer API today and take no arguments.

**Cost:** the `--print-addrspace-name=true` parts of
`test/Assembler/symbolic-addrspace-datalayout.ll` stay blocked, as gap **G6**
in [`fixture-coverage.md`](fixture-coverage.md) records.

**Fix:** decide the option surface first. The branch itself is then four lines:
`let name = module.data_layout().address_space_name(addr_space); if
!name.is_empty() { write!(f, "\"{name}\"") } else { write!(f, "{addr_space}") }`.

## Printer — function attributes are never hoisted into an attribute group (found 2026-08-13, LLParser parity W5)

`AssemblyWriter` prints function attributes **inline on the header**
(`define void @f() alignstack(4)`), where upstream prints a group reference
and emits the group at the end of the module:

    define void @f23() #13
    ...
    attributes #13 = { alignstack=4 }

Upstream does this in `SlotTracker::CreateAttributeSetSlot` (one slot per
distinct `AttributeSet`) plus `AssemblyWriter::writeAllAttributeGroups`.
llvmkit has neither: `asm_writer.rs` emits an `attributes #N = { … }` block
only for groups the *input* already carried, kept in `Module`'s attribute-group
table.

Consequences, all real:

- `test/Bitcode/attributes.ll` cannot be ported as a round-trip. It is the
  fixture that pins the `InAttrGrp` spelling of all four kinds that have one,
  and its CHECK lines are group lines. W5 ported the parse half and pins the
  printed group spelling from group-carrying input instead
  (`attribute_group_equals_grammar_round_trips`); the writer half is
  unpinned.
- llvmkit's output is bulkier than upstream's and diverges byte-for-byte from
  `llvm-dis` for any module with function attributes, which the parser/printer
  contract says should not happen.
- The `align` hack has no visible effect yet: upstream moves a group's
  `Alignment` to `Fn->setAlignment()` in `validateEndOfModule`, so
  `attributes #0 = { align = 8 }` re-prints as `define void @f() align 8`
  with the attribute gone from the group. llvmkit keeps it in the group.

Land it with the `validateEndOfModule` group-merge work (W13) rather than
alone: the merge is what decides which attributes survive into the printed
group, so doing the writer first would pin output the merge then changes.

## `ConstantRangeList` — three set operations not ported (decided 2026-08-12, LLParser parity W5)

`crates/llvmkit-ir/src/constant_range_list.rs` ports
`llvm/include/llvm/IR/ConstantRangeList.h` as far as the `.ll` surface needs
and one step further: `isOrderedRanges`, `getConstantRangeList`, `print`, and
`insert` (with its `int64_t` overload), the last of which the parser never
calls but the upstream unit tests pin.

**Not ported: `subtract`, `unionWith`, `intersectWith`.** They have no
consumer anywhere in llvmkit — upstream's callers are Attributor- and
`MemoryLocation`-style passes this tree has not ported — so porting them would
add public API with no in-tree user and no way to be sure it stays right.
Three of the six `unittests/IR/ConstantRangeListTest.cpp` cases (`Subtract`,
`Union`, `Intersect`) are therefore not portable yet; the other three
(`Basics`, `getConstantRangeList`, `Insert`) are ported verbatim. Land the
three methods together with their first real caller, and take the three tests
in the same commit.

One upstream detail to carry forward if they are ported: `insert`'s no-op
check uses `ConstantRange::contains`, which compares **unsigned**, while every
other comparison in the class is signed. `subtract` carries the comment saying
signed checking is what is wanted. llvmkit reproduces the inconsistency rather
than correcting it (behaviour is upstream's), and a `subtract` port must be
read against that comment, not against `insert`.

## Parser — the `inrange` bounds have a second, parallel APSInt reader (found 2026-08-12, LLParser parity W5)

`Parser::parse_inrange_bound` (`ll_parser.rs`) and its helpers
(`ParsedInRangeBound`, `inrange_bound_to_apint_words`,
`signed_magnitude_to_apint_words`, `apsint_to_apint_words`,
`hex_apsint_bit_width`, `decimal_digits_to_words`, `hex_digits_to_words`)
implement the same `LLLexer` APSInt semantics that `Parser::parse_int_literal`
now implements — the `[us]0x` active-bit truncation and the signed widening.
Both are currently *correct*: the `inrange` path was written with the
truncation and is pinned by
`parser_constants.rs::constant_expr_gep_inrange_signed_hex_active_bits_are_preserved`,
and `parse_int_literal` gained it in W5 when the `initializes` bounds needed
it.

Two implementations of one lexer rule is the shape this program keeps finding
bugs in (three private copies of the scalable-vector walk in W4.5, none
matching `Type::isScalableTy`). Collapse `parse_inrange_bound` onto
`parse_int_literal` + `ParsedApsInt::extend_or_truncate`; the only real work
is that `ConstantExprInRange::new` takes `Box<[u64]>` rather than an `ApInt`.
Not done in W5 because it is on the GEP constant-expression path, not the
attribute path — it belongs with W9a.

## Parser — a misplaced `phi` is rejected at parse time, not by the verifier (found 2026-08-08, LLParser parity W1)

`LLParser` accepts a `phi` written after a non-phi instruction and lets
`Verifier::visitPHINode` reject it with `PHI nodes not grouped at top of basic
block!`. llvmkit rejects it in `ll_parser.rs::parse_basic_block` instead
(`phi must be grouped at the top of its basic block`). Same verdict, wrong
layer, and a message upstream never prints.

**Do not "fix" this by deleting the parser check** — that was the LLParser
parity plan's first instinct and it is wrong. Every phi llvmkit builds goes
through `IrBuilder::make_phi_in_block`, which calls
`BasicBlock::insert_instruction_at_phi_head`: the phi is placed at the block's
phi head regardless of the builder's insertion point. Drop the parse-time
check and a misplaced phi is *silently hoisted* into a legal position, so the
verifier's own `VerifierRule::PhiNotAtTop` — which llvmkit does implement —
never sees a violation. llvmkit would then accept invalid IR and quietly
rewrite it, which is strictly worse than the current strictness.

Closing it needs a non-hoisting insertion path for parsed phis, so the
instruction lands where it was written. That is entangled with llvmkit's
head-phi design (block parameters are operandless head-phis, per
`IrBuilder::append_block_with_params`), so it wants deciding alongside that
model rather than as a parser patch. `insert_instruction_at_phi_head` is the
only phi insertion path today.

The fixture is now vendored — `PhiGrouping.ll`, under
`crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/` — and
`upstream_phi_grouping_fixture_is_rejected_at_parse_time` asserts the parse
diagnostic above, so this item's closure is what makes that test fail and get
replaced by the real port. `VerifierRule::PhiNotAtTop` itself now renders
upstream's literal; that half is closed.

## ~~Parser — `syncscope("system")` collapsed to the default scope~~ (decided and fixed 2026-08-06, W8)

The API-idioms plan left this as an implementer decision: preserve the
spelling, or pin the collapse as a recorded divergence. **Decided: preserve.**
`parse_optional_syncscope` (`crates/llvmkit-asmparser/src/ll_parser.rs`) used
to map a source-level `syncscope("system")` to `SyncScope::System`, so the
printer dropped the qualifier. Upstream disagrees: `LLVMContext::LLVMContext`
(`lib/IR/LLVMContext.cpp`) seeds `getOrInsertSyncScopeID` with only
`"singlethread"` and the **empty** string — the empty string is `System`'s
canonical name — so `"system"` registers as an ordinary named scope with a
fresh ID and round-trips as text. No corpus fixture or byte-lock covered the
spelling (checked before deciding), so the parser now yields
`SyncScope::Named("system")`, locked by
`crates/llvmkit-asmparser/tests/parser_remaining_opcodes.rs::fence_syncscope_system_round_trips`.

## ~~Printer — a scalable vector of uniform pointers or `undef` prints an element list~~ (found 2026-08-05, fixed 2026-08-05)

**Fixed** by `asm_writer::prints_as_splat`, which lifts the int/fp restriction
on the `splat (…)` shorthand for scalable vector types — where it is not a
shorthand at all but the only spelling — while keeping it for fixed vectors, so
output for every constant LLVM can also build stays byte-identical. Covered by
`crates/llvmkit-asmparser/tests/scalable_vector_splat_printing.rs`, which round
trips parse → verify → print → re-parse; the fixture that used to pin the buggy
output is re-aimed at the fix. The original analysis follows, since the
mechanism is worth keeping.

~~A **non-uniform** scalable aggregate still reaches the element-list fallback
and still prints text LLVM would reject~~ — **closed 2026-08-29**: it is now
unconstructible, which is the answer the policy question below settled on. The
element-list fallback is fixed-width only.

---

**This was invalid IR.** A scalable vector's lane count is a minimum, not a
count, so LLVM has no element-list constant form for one. llvmkit could print:

```text
@g = global <vscale x 4 x ptr> <ptr undef, ptr undef, ptr undef, ptr undef>
```

which LLVM would reject, and which quietly asserts `vscale == 1`.

**The mechanism, which is not where it first looks.** llvmkit represents a
scalable splat as `min_len` equal elements — a representation upstream cannot
have, because `ConstantVector::get` requires a fixed count. Five tests in
`crates/llvmkit-ir/tests/constant_fold.rs` depend on it
(`scalable_vector_trunc_splat_folds` and neighbours), and it round-trips
because `asm_writer::fmt_aggregate_constant` collapses a uniform vector back to
`splat (…)`. That collapse is gated on `is_int_or_fp_splat_value`, which
faithfully mirrors `AsmWriter.cpp`'s own `isa<ConstantInt> || isa<ConstantFP>`
restriction — correct for a *fixed* vector, where the element list is a legal
fallback, and wrong for a scalable one, where it is not.

So the folder was not at fault and must not be "fixed": an attempt to guard
`vector_splat_constant` against scalable types broke all five tests, because
the representation is deliberate. The fix belonged in the printer, and that is
where it landed.

## ~~Parser — no vector floating-point binary operators~~ (found 2026-08-05, fixed 2026-08-05)

**Fixed.** `fadd <vscale x 2 x double> %b, splat (double 1.5)` answered
"float-typed lhs": the parser narrowed operands through the scalar-only
`IntoFloatValue`, exactly as it used to for integers. Closed by three erased
builders beside their integer siblings —
`IrBuilder::fp_binop_erased`, `fp_cmp_erased` and
`fp_neg_erased` — with `parse_fp_binop`, `parse_fcmp` and `parse_fneg`
routed through them. Covered by
`crates/llvmkit-asmparser/tests/parser_vector_fp_ops.rs` (parse → verify →
print).

The scope was wider than the entry first said: `fcmp` and `fneg` narrowed the
same way, so fixing only the binary operators would have left
`fcmp oeq <4 x float> …` broken beside a working `fadd`. All three moved
together.

Found while writing `scalable_vector_splat_printing.rs`, which routed around it
with a `ret` instead of an `fadd`; that workaround stands, since the case is
about printing rather than parsing.

### The folders were audited and are clean (2026-08-05)

Every element-wise path in `constant_fold.rs` and `constant_folding.rs` was
checked for the neighbouring failure — reading a vector lane count and then
building or indexing an element list without a scalability guard. **No
violations.** All seven `fixed_vector_elements_for_rebuild` call sites bail on
the `scalable` flag first; `constant_folding.rs`'s only element walk is gated on
`TypeData::FixedVector`; the uniform builders (`null_constant_for_type`,
`bool_constant_for_type`, `all_ones_constant_for_type`, `vector_splat_constant`)
are correct by construction, since a uniform list *is* llvmkit's scalable-splat
representation. `constant_fold_extract_element_instruction` deliberately skips
the out-of-range→poison rule for a scalable operand, which would assert
`vscale == 1`, and answers "don't know" instead.

### ~~Open policy question: llvmkit permits a scalable constant LLVM cannot express~~ (decided 2026-08-29)

**Decided: uniformity is required.** `VectorType::const_vector` now demands
exactly `min_len` lanes and demands that they agree when the type is scalable,
which makes it llvmkit's spelling of `ConstantVector::getSplat(ElementCount, V)`
— the only constructor upstream has for a scalable vector constant. The
element-count skip was not "correct because the count carries no meaning": it
let `<vscale x 4 x i32>` hold a two-element list.

Two claims in the original entry were wrong, and both were load-bearing.

- **"the parser reaches it: `@g = global <vscale x 4 x i32> <i32 7, i32 8, i32
  7, i32 7>` parses today"** — it does not, and did not. `LLParser`'s `<…>` arm
  builds a *fixed* vector (`ConstantVector::get` → `FixedVectorType::get`) and
  `convertValIDToValue`'s `t_Constant` arm then compares types exactly;
  llvmkit's parser mirrors both, so that text answers `constant expression type
  mismatch`. The builder was the only entry point, which is why the fix is
  builder-side. [[verify-recorded-premises]]
- **"two deliberate tests depend on the permissive behaviour"** — three did. The
  third, `constant_folder_builder.rs::constant_folder_scalable_shuffle_builds_scalable_mask_expr`,
  did more than depend on it: its assertion pinned the invalid printed text
  `<vscale x 2 x i32> <i32 1, i32 2>` as expected output. A test written against
  the defect is what "requiring uniformity was tried and reverted" actually
  measured.

Of the three, one lost its premise entirely and is gone
(`scalable_i1_non_splat_divrem_does_not_use_scalar_i1_shortcuts`), one kept the
splat halves of its case
(`scalable_vector_fsub_negative_zero_pattern_controls_undef_fold`), and the
shuffle one took splat operands. `builder_aggregate_vector.rs::scalable_const_vector_admits_only_a_splat`
is the rule's own test, and asserts the printed form as well as the rejection —
the printer is what the rule exists to protect.

## ~~Parser — deferred alias/ifunc targets~~ (found and fixed 2026-07-31)

~~The printer emits aliases and ifuncs before function declarations, but the
parser resolved their target eagerly, so a printed module whose ifunc resolver
was declared later did not re-parse.~~ **Fixed:** forward targets become a null
placeholder patched at end of module, mirroring `personality`. Covered by
`parser_attribute_matrix.rs::alias_and_ifunc_forward_targets`.

## ~~Parser — lexer diagnostics carry no text~~ (found 2026-07-31, "fixed" 2026-07-31, **reverted** 2026-08-16, LLParser parity W14a)

~~`LexError::UnknownToken` renders as a bare `invalid token`, so an unknown
attribute keyword, a bogus global property, and a malformed `uwtable(...)` kind
all produce the same uninformative message. Fixed in
`feature-39/lexer-diagnostics`: the variant gained a
`reason: UnknownTokenReason` payload and every one of the ten construction
sites names its own failure — `unknown keyword 'nocalback'`, `no token starts
with '\x01'`, `expected a comdat name after '$'`, `expected hexadecimal digits
after '0xK'`, and so on.~~

**This was the wrong fix, and W14a undid it.** The entry's own caveat — "there
is no upstream message to port … so these messages are a deliberate
improvement on upstream rather than a port" — was the tell. `LLLexer` returns a
silent `lltok::Error` **token** at those sites precisely so that `LLParser` can
answer from the production it is in the middle of; writing a message in the
lexer instead does not add information, it *replaces* upstream's message with a
worse one and makes it unreachable. Eighteen `test/Assembler` fixtures and
splits could not be ported for exactly that reason, and the seven invented
messages were named as the highest-leverage remaining item in the parity
ledger.

`Token::Error` now carries those sites, `UnknownTokenReason` is gone, and
`LexError` is exactly the set of `LLLexer::LexError` call sites — the ones
where upstream *does* record a message and `ErrorPriority::Lexer` makes it
outrank the parser's.

The lesson to carry: **a "deliberate improvement" on a routine being ported
1:1 is a divergence with better marketing.** If the improvement is worth
having, it belongs behind an API that does not displace the ported behaviour —
here, a caller that wants the lexeme can read the `Token::Error` span out of
the source and say so itself.

## ~~ApFloat / `ApInt` bit-exactness audit~~ — closed (2026-08-01)

Both halves are **complete**. Every `APFloatTest.cpp` family covering the seven
modeled semantics and every `APIntTest.cpp` family llvmkit can express is
ported, and the fourteen defects they found are fixed. `Float8*` / `Float6*` /
`Float4*` / `TF32` stay out of scope — llvmkit does not model those semantics.

What is deliberately **not** ported from `APIntTest.cpp`, and why:

| Family | Why |
|---|---|
| `rvalue_arithmetic`, `rvalue_bitwise`, `rvalue_invert`, `SelfMoveAssignment` | C++ move-semantics fixtures; Rust move semantics make the property they check unrepresentable. |
| `tcDecrement` | The `tc*` word-level primitives are upstream's internal API. llvmkit stores words in a `Box<[u64]>` and exposes no equivalent surface, by design. |
| `StringBitsNeeded2` / `8` / `10` / `16`, `StringDeath` | `APInt::getBitsNeeded` sizes a width from a string. `ApInt::from_string` always takes an explicit width, so nothing calls it; `StringDeath` is an assertion-death test. |
| `mul_clear` | Checks that `operator*=` clears unused bits the same way `operator*` does. llvmkit has one `wrapping_mul`, so the two paths it compares are the same path. |
| `LargeAPIntConstruction` | Builds `APInt(UINT32_MAX, 0)` — a four-billion-bit value that allocates half a gigabyte. The behavior it guards (no crash on a legal but absurd width) is structural here. |
| `isAligned` | Needs upstream's `Align` type, which llvmkit does not model on `ApInt`. |
| `binaryOpsWithRawIntegers` | Upstream's scalar *arithmetic* overloads (`APInt + uint64_t`). llvmkit deliberately requires both operands to name a width; the comparison half of that story is covered by `unsigned_cmp_u64` / `signed_cmp_i64`. |
| `SolveQuadraticEquationWrap` | SCEV-specific; belongs with a SCEV port, not with `ApInt`. |
| `GetMostSignificantDifferentBitExaustive` | The non-exhaustive fixture is ported; the exhaustive variant re-derives the same property over an 8-bit sweep. |

## ValueTracking.h — remaining tranches, and the order to take them

**Status after the residue port (2026-08-04):** 93 of 101 entry
points modeled, 8 gaps, all symbol-keyed in
`crates/llvmkit-ir/tests/value_tracking_parity.rs` and asserted to sum to the
audited surface.

`computeKnownFPClass` is modeled but its **dispatch is partial** — the entry
point exists and every unported arm is named in `known_fp_class.rs`'s header.
That is deliberate: an unported arm answers `fcAllFlags`, so it weakens a
result rather than falsifying one, and the arms are independent enough to land
one at a time. The ledger counts the entry point, not the arm coverage, so
"modeled" here means less than it does for a total function; the module header
is the honest record.

| Tranche | Left | Note |
|---|---|---|
| ~~4b select matching~~ | 0 | **done** — the matching half of `crates/llvmkit-ir/src/select_pattern.rs` |
| ~~5 pointer/object~~ | 0 | **done** — `crates/llvmkit-ir/src/pointer_analysis.rs` |
| ~~6 speculation/UB~~ | 0 | **done** — `crates/llvmkit-ir/src/speculation.rs` |
| ~~8 assumptions~~ | 0 | **done** — `crates/llvmkit-ir/src/assumptions.rs` |
| ~~implied conditions~~ | 0 | **done** — `crates/llvmkit-ir/src/implied_conditions.rs` |
| ~~7 FP class~~ | 0 | **done** — `fp_class.rs` (lattice + every `KnownFPClass.cpp` operation) and `known_fp_class.rs` (`computeKnownFPClass` + the nine predicates) |
| ~~FP select arm~~ | 0 | **done** — `fp_predicate.rs` (`fcmpImpliesClass`) plus `adjust_known_fp_class_for_select_arm` and the `select` dispatch arm |
| dead upstream declaration | 1 | `analyzeKnownFPClassFromSelect`, see below |
| residue | 3 | `analyzeKnownBitsFromAndXorOr`, `stripNullTest` and `collectPossibleValues` are done; see below for what the last three need |
| expose-only | 2 | `matchSimpleRecurrence` and `computeKnownBitsFromRangeMetadata` already exist as crate-private helpers |
| ~~`getInverseMinMaxIntrinsic`~~ | 0 | **done** — all ten arms, see below |
| sibling | 1 | `matchSimpleBinaryIntrinsicRecurrence` |
| blocked | 1 | `getVScaleRange`, see above |

### Found while porting tranche 4b: three sites inherited one false premise — **closed 2026-08-04**

`getInverseMinMaxIntrinsic` was recorded as partially modeled on the grounds
that "llvmkit models no floating-point min/max intrinsic". That was true when
tranche 4b wrote it and **false since tranche 7a**: `MinMaxKind` in
`fp_class.rs` has exactly the six variants upstream inverts — `Minimum`/
`Maximum`, `MinimumNum`/`MaximumNum`, `MinNum`/`MaxNum` — and
`known_fp_class.rs` already maps intrinsic names onto them. The claim had been
copied to three places: the ledger reason, `MinMaxIntrinsic::inverse`'s doc
comment, and `can_convert_to_min_or_max_intrinsic`, which *narrowed its own
behaviour* on the strength of it and answered `None` where upstream answers
`maxnum` / `minnum`.

The open design question was how one public entry point spans two enums. The
answer is [`MinMaxOperation`](../crates/llvmkit-ir/src/select_pattern.rs), a
two-arm sum of `MinMaxIntrinsic` (the four integer intrinsics, exactly the
range of `getMinMaxIntrinsic`) and `MinMaxKind` (the six floating-point ones,
which port `KnownFPClass::MinMaxKind` and are deliberately IR-independent).
The arms are disjoint and the union is exactly upstream's ten, so
`MinMaxOperation::inverse` is total where upstream needs
`llvm_unreachable("Unexpected intrinsic")` — the price `Intrinsic::ID` pays for
being one flat type naming every intrinsic there is. `MinMaxKind::inverse` and
`MinMaxIntrinsic::inverse` are each half of it, and
`can_convert_to_min_or_max_intrinsic` now returns the sum, which also closes
its narrowing.

**The lesson generalises:** a comment claiming llvmkit lacks a feature is a
claim to verify, not to inherit. Three of them were false in the 2026-08-04
sweep, and two caused real defects rather than merely stale prose.

### Found while porting `stripNullTest`: the parser could not express a vector cast — **closed 2026-08-04**

`llvm/test/Transforms/InstCombine/ceil-shift.ll` is the upstream test file for
`stripNullTest`, and two of its cases — `ceil_shift4_v4i32` and
`ceil_shift4_v8i16` — are the vector spellings of the idiom. They could not be
ported: llvmkit parsed vector `lshr`, `and` and `icmp` and `splat` constants,
but not a vector `zext`, which every form of the idiom needs.

The fix completed a pattern the vector binop work had started rather than
inventing one. `TryFrom<Value> for IntValue<IntDyn>` requires
`TypeData::Integer`, so a vector converts to no typed integer handle at all;
binops and compares had already grown erased builders for exactly that reason
(`int_binop_erased`, `int_cmp_erased`), and the casts had not.
`int_cast_erased` is the third, with `IntCastFlags` as the runtime-opcode
counterpart of `IntBinOpFlags`, and the parser branches on
`is_vector_type` the way `parse_int_binop` already did.

**The verifier had the same scalar assumption and it was the harder half.**
Its integer-cast arm read widths through an integer-only accessor, so vector
casts parsed and then failed verification. It now compares
`getScalarSizeInBits` and checks the vector shapes separately — which is what
`CastInst::castIsValid` actually says — and the old scalar-only helper is gone
rather than left behind.

Two upstream cases returned to the `stripNullTest` port as a result; nothing in
the analysis changed, because `strip_null_test` was already splat-aware.

### Found while closing that row: the ledger did not check its own ValueTracking column — **closed 2026-08-04**

`value_tracking_parity.rs`'s module header claimed that
`exercises_every_modeled_*` reaches every row of the modeled tables. That was
enforced for `KnownBits` — `every_modeled_known_bits_row_is_exercised` reads
the test file's own source and proves it — and enforced by nothing at all for
`ValueTracking`, whose exercise fn was hand-maintained against a ~91-row table.

It had drifted. `getInverseMinMaxFlavor`, `getMinMaxIntrinsic`, `getMinMaxLimit`
and `getMinMaxPred` were listed as modeled with no code naming them; the new
`every_modeled_value_tracking_row_is_exercised` then found three more —
`getSelectPattern`, `SelectPatternResult` and `SelectPatternNaNBehavior`. All
seven are exercised now, and the header says which table has been proven since
when instead of implying both always were.

The probe differs from the `KnownBits` twin because the shapes do: a
`KnownBits` row is a method, so `.name(` is the call site to look for, while a
`ValueTracking` row is mostly a free function or a type that the exercise fn
*names as a value*. So it matches the identifier path at a word boundary, which
is what keeps `compute_known_bits` from being satisfied by
`compute_known_bits_from_context`.


### Found while porting tranche 6: the `and` / `xor` known-bits arm is narrow — **closed 2026-08-04**

`bitwise_known` in `value_tracking.rs` ported only part of upstream's
`getKnownBitsFromAndXorOr` (`ValueTracking.cpp`). It had the general
"add/sub of an odd operand sets or clears bit 0" refinement, but neither of the
two idiom arms above it:

- `and(x, -x)` clears everything above the lowest set bit — upstream answers
  `KnownLHS.blsi()` or `KnownRHS.blsi()`, whichever operand has the smaller
  `countMaxTrailingZeros`.
- `xor(x, x - 1)` likewise, answering `XBits.blsmsk()`.

`KnownBits::blsi` and `blsmsk` were both already modeled here, and
`is_negation_of_operand` already existed for the `m_Neg(m_Deferred(X))` half, so
this was a small, self-contained improvement rather than a new subsystem — and
the prerequisite for `analyzeKnownBitsFromAndXorOr`, which is exactly
`getKnownBitsFromAndXorOr` with the operand known-bits passed in.

Nothing was *wrong* — a missing refinement only leaves the answer weaker —
which is why no test caught it. It was found by reading the C++ beside the
Rust, not by running anything.

Both arms are now ported, and the prediction held: with `bitwise_known`
complete, `analyzeKnownBitsFromAndXorOr` was a thin wrapper over it, so the
public entry point fell out and the ledger row moved with it. The shared body
is `and_xor_or_known` in `value_tracking.rs`, mirroring upstream's split
between `getKnownBitsFromAndXorOr` and its public re-export.

No upstream fixture isolates either arm — LLVM reaches them through
InstCombine's demanded-bits path, and the nearest `.ll` files
(`test/Transforms/InstCombine/ispow2.ll`) go through `isKnownToBeAPowerOfTwo`
instead. The tests here are therefore llvmkit-specific, but their expected
values are upstream's own formulas (`KnownLHS.blsi()`, `XBits.blsmsk()`)
applied to the operand bits, so what they pin is that the *matcher* routes
correctly rather than a mask derived by hand.

### Found while porting tranche 8: the integer min/max matcher was narrow *and* too permissive

`int_min_max_against_constant` (tranche 4b) ported `m_SMin` / `m_SMax` /
`m_UMin` / `m_UMax` as the `select(icmp …)` shape only. Upstream's
`MaxMin_match` (`PatternMatch.h`) matches **two** shapes — that select *and* a
direct call to the matching `llvm.{s,u}{min,max}` intrinsic — so the port
declined programs `matchClamp` accepts. In the other direction it accepted its
two operands in either order, which is `m_c_SMin`; `matchClamp` writes the
non-commutative `m_SMin`. Both are fixed, and the select arm now binds operands
the way upstream does: `L` is the *compare's* first operand, with the predicate
**inverted** — not swapped — when the true arm is the compare's right-hand side.

The commutativity papered over the binding-order slip, which is why nothing
failed. Found by reading `MaxMin_match` while porting `isTruePredicate`, which
needs the genuinely commutative `m_c_SMax` family.

### Found while porting the FP select arm

Four things, each caught by a test that should have passed and did not.

**`analyzeKnownFPClassFromSelect` has no definition anywhere in LLVM.** The name
occurs exactly once across the whole `llvm/` tree — its own declaration in
`ValueTracking.h` — with no body and no caller. There is nothing to port, and it
stays a gap on that ground rather than as work not yet done. The arm it names is
real and is now ported, as `adjustKnownFPClassForSelectArm` plus the `Select`
case of `computeKnownFPClass`. Earlier notes here called the pair "one arm
counted twice"; that was inferred from the name, not read from the source.

**`computeKnownFPClass` was using the dynamic denormal mode everywhere.**
Tranche 7b's comment said llvmkit models no `denormal-fp-math` attribute. It
does — `FunctionValue::denormal_mode`, a faithful port of
`Function::getDenormalMode` — and, more importantly, upstream's
`parseDenormalFPAttribute` maps an *absent* attribute to `ieee` ("Assume ieee on
unspecified attribute"), not to dynamic. Every arm keyed on the mode was
therefore weaker than upstream, and the whole zero-comparison family of
`fcmpImpliesClass` would have learned nothing at all, since it bails unless
inputs are IEEE.

The related trap: upstream's `queryDenormalMode(F, Val)` takes the *type* from
the value but the *mode* from the function the caller passes — which for
`computeKnownFPClassFromCond` is the function containing the **condition**, not
the value. Deriving it from the value instead silently fails whenever the value
is an argument, which is the common case.

**Fast-math flags on a `call` were unrepresentable end to end.** `parse_call`
did not accept them, the AsmWriter did not print them, and `IrBuilder`'s flags
apply only to the FP binops — so `CallAttributeData::fmf` could never become
non-empty, and the `nsz` arm of `computeKnownFPClass`'s `sqrt` case was dead
code. Both halves are now ported (`LLParser::parseCall` eats the flags before
the calling convention; `writeOptimizationInfo` prints them straight after the
opcode), including upstream's rejection of flags on a call whose return type is
not floating-point. This is what made
`ComputeKnownFPClassTest.SqrtNszSignBit` portable.

**The tranche-7 tests claimed upstream had none.** `ValueTrackingTest.cpp` has a
whole `ComputeKnownFPClassTest` fixture — 40-odd cases with exact class masks and
sign bits, plus four `FCmpToClassTest_*` and one `fcmpImpliesClass_fabs_zero`.
The tranche-7 tests were written as llvmkit-specific inventions on the strength
of a claim nobody checked. They are now ports; `UPSTREAM.md` records which case
each one takes. Two hand-written tests were deleted rather than kept, their
coverage being subsumed by the real ports.

`nofpclass` on a parameter or call return is still unmodeled, which is why two of
`SqrtNszSignBit`'s four blocks are not ported.

### Order for the remaining 8 (recorded 2026-08-04)

1. **Residue (3 left of 5).** `stripNullTest` and `collectPossibleValues`
   landed 2026-08-04 — both were pure pattern/worklist code with no missing
   dependency, which is why they went first. What remains is not "small and
   independent" the way that phrasing implied; each is blocked on something
   structural, re-checked 2026-08-04:
   - `getFlippedStrictnessPredicateAndConstant` **mints a constant**
     (`(icmp sgt X, 0)` → `(icmp sle X, 1)` needs the `1`), which is the
     standing "not ported" ground. Leaving it a recorded gap is more honest
     than a port that returns the predicate and makes the caller build the
     constant, which is a different function.
   - ~~`getIntrinsicForCallSite` returns an `Intrinsic::ID`, and **llvmkit has
     no public intrinsic-id type** — `IntrinsicSemantic` is `pub(crate)`…~~
     **Wrong reason; struck 2026-08-05.** `IntrinsicId` has been public,
     generated-backed and whole-space all along (`intrinsics.rs`, re-exported
     from `lib.rs`, with `lookup`, `all`, `from_raw` and `base_name`). The
     `pub(crate)` type is `IntrinsicSemantic`, a convenience enum over a
     31-name *subset*, and this entry conflated the two — as did both parity
     ledgers, which recorded nine functions as blocked on it. Five of those are
     now ported (`vector_utils.rs`'s intrinsic classifiers) against the public
     type, matching on `IntrinsicId::base_name` the way
     `speculation::intrinsic_propagates_poison` already did. What remains
     genuinely undone here is the library-function half: upstream maps ~60
     `LibFunc` values onto intrinsics and gates them on
     `CallBase::onlyReadsMemory`. `target_library_info.rs::LibFunc` and
     `lib_func_for_name` exist, so this is a porting task, not a blocker.
   - `isOverflowIntrinsicNoWrap` needs `BasicBlockEdge::isSingleEdge` and
     edge-dominance (`DT.dominates(edge, use)`), plus the with-overflow
     intrinsics as a modeled family rather than plain calls.
2. **Expose-only (2).** `computeKnownBitsFromRangeMetadata` is not a rename:
   upstream takes an `MDNode` where llvmkit's helper is value-shaped, so the
   public parameter is a real design decision.
3. **Sibling and blocked (2).** `matchSimpleBinaryIntrinsicRecurrence` needs
   `match_simple_recurrence` generalised over the intrinsic-call form, so that
   its `II == I` identity check has something to bind. `getVScaleRange` stays
   recorded: upstream reads a packed `(min, max)` out of one attribute, and
   `vscale_range` is on `attribute_td_drift.rs`'s `NOT_YET_MODELED` list with a
   single-`u64` payload — so the parser cannot even produce a function carrying
   one, and porting it would mean inventing the max half. It unblocks when the
   *attribute* is modeled, which is an attribute-layer task, not this one.
4. **`analyzeKnownFPClassFromSelect` (1)** never closes: there is no upstream
   definition to port. It is recorded, not scheduled.

Separately, and moving no ledger row: **fill in `computeKnownFPClass`'s
dispatch.** The entry point already counts as modeled; the arms are where the
analysis earns its answers. `known_fp_class.rs`'s module header is the
authoritative list of what is missing. Biggest wins are the arithmetic arms and
the assumption arm, whose caches tranche 8 already built.

### Historical note on ordering

**Tranches 6 and 8 were the unblockers, and both have landed.** The advice at
the time was "take 6 and 8 before 5 and 7, they are unblockers rather than
additions", and both paid out as predicted.

Tranche 6 closed three arms `is_known_not_undef_or_poison` had recorded as
deferred — the `programUndefinedIfUndefOrPoison` walk, the
dominating-branch-condition walk, and (found along the way) the
`shufflevector` splat arm — plus the `!noundef` load-metadata arm, without
editing that function's own logic.

Tranche 8 closed its `@llvm.assume` arm, and `compute_known_bits` now runs
`computeKnownBitsFromContext` where upstream does, so every existing known-bits
caller gets assumption- and branch-driven facts by attaching a cache to the
query. **Unverified follow-on:** the note that motivated the ordering also
claimed `is_known_to_be_a_power_of_two` and `is_known_non_equal` each skip an
assumption-driven refinement. Both reach assumptions through
`compute_known_bits` now, so that may have closed itself — nobody has checked.
Read the two upstream functions beside ours before recording it either way.

Tranche 7 followed as the standalone finish it was predicted to be — a new
lattice rather than a new consumer, with nothing waiting on it.

Two arms of tranche 8 stay narrower than upstream, each recorded at its site:
`computeKnownBitsFromContext`'s operand-bundle alignment refinement needs
`getKnowledgeFromBundle` (`llvm/Analysis/AssumeBundleQueries.h`), and
`isImpliedCondFCmps`'s constant-versus-constant conclusion needs
`ConstantFPRange`. Neither header is ported; both omissions only weaken an
answer. `AssumptionCache` still records the bundle indices, so the first can be
filled in without re-scanning.

**Run both doc gates per slice, not just fmt/clippy/tests.** Three tranches ran
without them and a `private_intra_doc_links` error — a public doc linking the
crate-private `ConstantData` — sat unnoticed until it was asked for. **It
recurred on 2026-08-04** (`fp_predicate.rs`'s module header linking the
crate-private `denormal_mode_of`), which is the argument for running the gate
every slice rather than remembering to:

```
RUSTDOCFLAGS="-D warnings" cargo +1.96.0 doc --workspace --no-deps --all-features
cargo +1.96.0 test --workspace --doc --all-features
```

Two things tranche 6 turned up that are worth carrying forward:

- **Upstream's own unit tests for the dominating-condition arm do not cover
  it.** `isGuaranteedNotToBePoison_exploitBranchCond` and
  `isGuaranteedNotToBePoison_phi` are both satisfied by the *earlier*
  `programUndefinedIfUndefOrPoison` arm, in LLVM as much as here, because their
  branch sits in the same block as the value. Deleting the idom walk leaves
  both green. Porting a test is not the same as covering the code it names —
  red-green each arm separately.
- **`isGuaranteedToExecuteForEveryIteration` did not need `LoopInfo`.** It
  takes a `const Loop *` and reads exactly one thing out of it,
  `L->getHeader()`, so llvmkit takes the header block directly. Check what a
  blocked-looking signature actually *reads* before recording it as blocked.
  Tranche 5 repeated the lesson: `getUnderlyingObjects` takes a `LoopInfo` for
  one refinement and behaves fine without it, because `LI == nullptr` is
  upstream's own default.

### Found while porting tranche 4b

- **`m_SMin` and friends match the `select` form, not the intrinsic.**
  `MaxMin_match<ICmpInst, ..>` looks for `select(icmp PRED L, R, L, R)`
  structurally; it does *not* match a call to `llvm.smin`. A first draft of
  `matchClamp` matched the intrinsic, which would have accepted programs
  upstream's own `matchClamp` never sees. When porting a `PatternMatch`
  combinator, read the combinator, not its name.
- **Two arms are deliberately narrower, both because they would need to mint a
  constant.** `getNotValue` folds `~C` for a constant operand, and
  `lookThroughCastConst` builds a casted constant and checks it round-trips.
  Creating a constant is a module mutation, which an analysis has no business
  performing; both are recorded at their sites. Each forgoes a match rather
  than inventing one.

### Found while porting tranche 5

- **`CallBase::getReturnedArgOperand` reads the callee's parameter attributes,
  not just the call site's.** `declare ptr @f(ptr returned)` puts `returned` on
  the *declaration*; a call that does not repeat it still returns its argument.
  `pointer_analysis.rs` ports both halves — an upstream fixture caught the
  missing one — but `value_tracking.rs`'s own `returned_arg_operand` still
  reads only what its caller hands it, which is the call site's `arg_attrs`.
  Same shortfall, different function; closing it would sharpen the `returned`
  arm of `call_known_bits`.
- **Constant uniquing does work that upstream's pointer identity does for
  free.** `isBytewiseValue` compares against a single `UndefValue::get(i8)`
  sentinel by pointer, which works because LLVM uniques constants. llvmkit
  names the sentinel up front instead (`ConstantData::Undef` before the `i8`
  arm). Any port that leans on `==` between `Value *`s deserves the same
  check.

The full working spec — upstream anchors, settled design decisions, the
surface-audit recipe, and the traps already hit — is at
`docs/superpowers/specs/2026-08-04-valuetracking-remaining-tranches.md`. That
directory is gitignored, so the file is local to the working tree; the
decisions that outlive it are recorded here.

## ValueTracking.h — surface audit (2026-08-03)

Re-measured through the compiler rather than by grep, the same way
`KnownBits.h` was: preprocess with `g++ -E` against the configured
`build/llvm/include`, attribute the output back to its originating header via
the `# <line> "<file>"` markers, then parse at namespace depth 1. The recipe
is recorded on `VALUE_TRACKING_SURFACE_AUDITED` in
`crates/llvmkit-ir/tests/value_tracking_parity.rs`.

**101 entry points** — 96 namespace-scope functions plus 5 types defined in the
header (`ConstantDataArraySlice`, `OverflowResult`, `SelectPatternFlavor`,
`SelectPatternNaNBehavior`, `SelectPatternResult`). **At the time of the audit**
32 were modeled and 69 were not; the running count lives at the top of this
section (93 / 8 as of 2026-08-04 — the eight rows of `VALUE_TRACKING_GAPS`).
The 101 is the part that does not move.

The audit's real finding was about the ledger, not the port. The gap table was
keyed by *family* — seven prose rows, on the reasoning that "enumerating ~76
individually would be noise". The result: **47 of the 101 entry points appeared
in neither the modeled table nor any gap reason.** Not recorded as missing;
invisible. The ledger read as though the gap were seven families wide rather
than sixty-nine symbols, and nothing could detect the difference.

Two smaller corrections fell out of the same pass:

- `computeKnownBitsFromOperator` sat in the *modeled* table, which tracks
  `ValueTracking.h`. It is `static` in `ValueTracking.cpp` and appears in no
  header. It moved to a `VALUE_TRACKING_PRIVATE_UPSTREAM` list, the same
  treatment `KnownBits.h`'s `flipSignBit` / `remGetLowBits` already had.
- `OverflowResult` is public in llvmkit and had simply never been listed.

The gap table is now symbol-keyed, and `value_tracking_surface_is_accounted_for`
asserts modeled + gaps equals the audited 101 — so a symbol can no longer be
neither. Verified red-green: dropping one gap row fails it.

## ValueTracking.h — the road to 100% (measured 2026-08-01, **prerequisites now all met**)

This section's premise was that `ValueTracking.h`'s unmodeled entry points were
blocked on **missing types**, not on missing effort. That premise has since been
worked through: every prerequisite below is done. It is kept as the record of
what the sizing looked like going in, with each row re-measured 2026-08-04.

| Prerequisite | Upstream size | llvmkit now | Status |
|---|---|---|---|
| `ConstantRange` | 632 (`.h`) + 2314 (`.cpp`) | `constant_range.rs`, 3038 lines, 95 public methods | **done 2026-08-02** — all but `castOp`, `shlWithNoWrap` and the signedness-flipping helpers, each recorded in slice 3 below |
| `KnownFPClass` / `FPClassTest` | `FloatingPointMode.h` 290 + `KnownFPClass.h` 324, plus ~1500 lines of `computeKnownFPClass` | `fp_class.rs` (lattice + every operation), `known_fp_class.rs` (`computeKnownFPClass` + predicates), `fp_predicate.rs` (`fcmpImpliesClass`) | **done 2026-08-04**; the dispatch is partial and its module header names each missing arm |
| `AssumptionCache` | 280 + 310 | `assumptions.rs`, with `DomConditionCache` alongside | **done 2026-08-04** (tranche 8) |
| `SelectPatternResult` | declared in `ValueTracking.h` | `select_pattern.rs` | **done 2026-08-03** (tranches 4a/4b) |
| `TargetLibraryInfo` | 664 | `target_library_info.rs`, 427 lines | partial; check what it already answers before recording `getIntrinsicForCallSite` as blocked on it |
| `ValueTracking.cpp` itself | 10535 | `value_tracking.rs`, 6062 | the remaining 8 gaps are listed above, not a line-count gap |

The original estimate — roughly 12–14k lines of ported logic plus D11-compliant
tests, comparable to the whole ApFloat/ApInt sweep — held. It was delivered as
the tranche sequence recorded above rather than as one program.

**Suggested tranche order.** Tranche 1 needs no new types at all and unblocks
the most callers, so it should go first regardless of how the rest is
sequenced:

1. ~~**No new types**~~ ✅ **done 2026-08-03** — `ComputeNumSignBits` and
   `ComputeMaxSignificantBits` landed with the sign-bits arm (2026-08-02); the
   rest followed: `isKnownNegative` / `isKnownPositive` / `isKnownNonNegative`,
   `MaskedValueIsZero` (with a real mask — the ledger previously mapped it to
   `is_known_zero`, which takes none), `isSignBitCheck` (an `Option<bool>`
   rather than upstream's `bool` return plus `bool &TrueIfSigned`
   out-parameter), `isKnownToBeAPowerOfTwo` including `isPowerOfTwoRecurrence`,
   `isKnownNonEqual` with `getInvertibleOperands` and its five helpers,
   `isKnownInversion`, `isKnownNegation`, `isOnlyUsedInZeroComparison` and its
   equality sibling, and the Value-level `haveNoCommonBitsSet` with all six
   `haveNoCommonBitsSetSpecialCases` patterns.

   The same slice replaced `isGuaranteedNotToBePoison`'s placeholder — which
   handled constants and shifts and answered `false` for everything else — with
   the real `isGuaranteedNotToBeUndefOrPoison` walk, and added
   `isGuaranteedNotToBeUndef` / `isGuaranteedNotToBeUndefOrPoison` as public
   entry points. Three of its arms are deferred, each only weakening the
   answer: `programUndefinedIfUndefOrPoison` and the dominating branch-condition
   walk that follows it (both CFG reachability, tranche 6), the `@llvm.assume`
   arm (tranche 8), and `stripPointerCastsSameRepresentation` before the
   allocated-object test (tranche 5). One arm is a deliberate llvmkit
   refinement, marked at its site: a shift whose amount known bits prove in
   range is not poison, where upstream's `shiftAmountKnownInRange` demands a
   literal constant.

   Two upstream fixtures do **not** port, and each gap is recorded rather than
   papered over:

   - The `<2 x i32>` third of `TEST_F(ValueTrackingTest, HaveNoCommonBitsSet)`.
     llvmkit cannot express it — see the vector-binop entry below.
   - `known-power-of-two.ll`'s positive cases (`@shl_is_pow2`,
     `@trunc_is_pow2_or_zero`, and their siblings). Their `shl` carries no
     `nuw`/`nsw` in the source; upstream reaches `true` only because
     `instcombine` infers the flags first, which the printed `CHECK` line shows
     (`shl nuw nsw`). That is a missing transform, not a missing analysis.
     `crates/llvmkit-asmparser/tests/value_tracking_predicates.rs` asserts them
     **false** so closing the gap trips the test rather than passing silently.
2. ~~**Poison / UB family**~~ ✅ **done 2026-08-03** (tranche 6) —
   `canCreatePoison`, `canCreateUndefOrPoison`, `impliesPoison`,
   `propagatesPoison`, `programUndefinedIfPoison`, `mustTriggerUB`,
   `isGuaranteedNotToBeUndef`, in `speculation.rs`. It extended the existing
   `is_guaranteed_not_to_be_poison` seed and needed no new types, as predicted.
3. ~~**`ConstantRange` to completion**, then the families it gates.~~
   **DONE 2026-08-02** — all five slices. `ConstantRange` went from 13 of 83
   public methods to 90 llvmkit methods covering all but `castOp`,
   `shlWithNoWrap` and the signedness-flipping helpers, each recorded below.
   Cut into five sub-slices, each mergeable on its own (2026-08-01); the count
   per row below is what that slice added. (An earlier revision of this
   paragraph still said "llvmkit maps 13 of the 78 public methods today" *after*
   the DONE line — the pre-slice figure, left in place and contradicting the
   sentence above it.)

   | Slice | Adds | Content |
   |---|---|---|
   | **3a** | ~16 | Bounds and predicates: `getSignedMax`/`Min`, `isSignWrappedSet`, `isUpperSignWrapped`, `isSingleElement`, `isAllNegative`/`isAllNonNegative`/`isAllPositive`, `isSizeLargerThan`, `isSizeStrictlySmallerThan`, `getActiveBits`, `getMinSignedBits`, `getNonEmpty`, `toKnownBits`, `fromKnownBits`. No arithmetic; exhaustively testable at 4 bits. |
   | ~~**3b**~~ | ~12 | ~~Set operations~~ **done 2026-08-02**: `intersect_with`, `union_with`, `difference`, `subtract`, `inverse`, `split_pos_neg`, `zero_extend`, `sign_extend`, `truncate`, `zext_or_trunc`, `sext_or_trunc`. `castOp` is **not** ported — it dispatches on `Instruction::CastOps` over the ten cast opcodes, but eight of them are no-ops or bail to full; the two that matter (`zext`, `sext`, `trunc`) are the methods above, and a caller with a `CastOpcode` in hand can match on it directly. Revisit if a consumer wants the dispatcher itself. |
   | ~~**3c**~~ | ~9 | ~~ICmp regions~~ **done 2026-08-02**: `make_allowed_icmp_region`, `make_satisfying_icmp_region`, `make_exact_icmp_region`, `make_mask_not_equal_range`, `equivalent_icmp` / `equivalent_icmp_with_offset`, `icmp`, plus the `single` and `contains_range` constructors/queries they needed. The signedness-flipping helpers (`areInsensitiveToSignednessOfICmpPredicate`, `getEquivalentPredWithFlippedSignedness`) are **not** ported — nothing in llvmkit calls them, and they exist upstream for InstCombine's predicate canonicalization, which llvmkit does not have. |
   | ~~**3d**~~ ✅ | ~37 | **done 2026-08-02** across six sub-slices: |
   | 3d-i ✅ | 7 | **done 2026-08-02**: `add`, `sub`, `multiply`, `smax`/`smin`/`umax`/`umin`. These are exactly what `computeOverflowFor*` in 3e needs, which is why they went first. |
   | 3d-ii ✅ | 5 | **done 2026-08-02**: `udiv`, `sdiv`, `urem`, `srem`, plus `abs` pulled forward from 3d-v because `srem` needs it. |
   | 3d-iii ✅ | 7 | **done 2026-08-02**: `binary_and`, `binary_or`, `binary_xor`, `binary_not`, `shl`, `lshr`, `ashr`. Found and fixed a real `ApInt::ashr` bug on the way — see the CHANGELOG. |
   | 3d-iv ✅ | 8 | **done 2026-08-02**: the saturating family. Six share one frame (`saturating_pairwise`); `smul_sat` needs all four corners and `sshl_sat` picks its shift by endpoint sign. |
   | 3d-v ✅ | 3 | **done 2026-08-02**: `ctlz`, `cttz`, `ctpop` (`abs` landed in 3d-ii). |
   | 3d-vi ✅ | 8 | **done 2026-08-02**: `binary_op`, `overflowing_binary_op`, `intrinsic`, `is_intrinsic_supported`, `add_with_no_wrap`, `sub_with_no_wrap`, `multiply_with_no_wrap`, `smul_fast`. **`shlWithNoWrap` is not ported** — it is three helper functions (`computeShlNUW`, `computeShlNSWWithNNegLHS`, `computeShlNSWWithNegLHS`) plus a dispatcher, and no llvmkit caller needs it yet; `overflowing_binary_op` sends `shl` to the plain `shl`, which is sound and only weaker. |
   | ~~**3e**~~ ✅ | 8 | **done 2026-08-02**: `compute_constant_range`, `compute_constant_range_including_known_bits`, and all six `compute_overflow_for_*`. Also added the five `ConstantRange` overflow predicates (`unsigned_add_may_overflow` and siblings) that an earlier count missed — the real public surface is **83**, not 78; those five span two lines in the header and the extraction grep skipped them. **`getVScaleRange` is not ported**: it reads `vscale_range`'s packed `(min, max)` pair, and that attribute is already on `attribute_td_drift.rs`'s `NOT_YET_MODELED` list with a single-`u64` payload here. Porting it would mean inventing the second half. |

   `ConstantRangeTest.cpp` (~2800 lines) is the test source throughout; its
   exhaustive-over-4-bit-ranges harness ports directly and is the right
   oracle for every slice.
4. ~~**`SelectPatternResult`**~~ ✅ **done 2026-08-03** (tranches 4a/4b) —
   `getSelectPattern`, `matchDecomposedSelectPattern`, `getMinMaxIntrinsic` in
   `select_pattern.rs`. `getInverseMinMaxIntrinsic`, the one row of this tranche
   left open, closed 2026-08-04 with `MinMaxOperation`; see the note under the
   tranche table above.
5. ~~**Pointer / object analysis**~~ ✅ **done 2026-08-03** (tranche 5) —
   `pointer_analysis.rs`.
6. ~~**Speculation safety**~~ ✅ **done 2026-08-03** (tranche 6) —
   `speculation.rs`; `isValidAssumeForContext` landed with tranche 8 in
   `assumptions.rs`.
7. ~~**`KnownFPClass`**~~ ✅ **done 2026-08-04** (tranche 7a/7b/7c + the select
   arm). It was the largest single piece and the only one needing a new lattice,
   as predicted. `computeKnownFPClass`'s dispatch remains partial by design.
8. ~~**`AssumptionCache`**~~ ✅ **done 2026-08-04** (tranche 8) —
   `assumptions.rs`, and `compute_known_bits` now runs
   `computeKnownBitsFromContext` where upstream does.

The ledger in `crates/llvmkit-ir/tests/value_tracking_parity.rs` tracks
progress: each tranche moves rows from `VALUE_TRACKING_GAPS` into
`MODELED_VALUE_TRACKING`, and the modeled column is held to the crate by
calling every entry.

## ~~The parser cannot read vector integer binops~~ — closed (2026-08-03)

`and <2 x i32> %a, %b` did not parse, even though llvmkit could *build* it:
`ll_parser.rs::parse_int_binop` converted both operands to
`IntValue<'ctx, IntDyn, B>` first, and that marker describes a **scalar**
width. `icmp` took the same route.

Closed by routing vector operands to the erased builder family, which already
emitted element-wise vector IR. The scalar path is untouched, so the
one-literal-one-width story still holds where it can.

What it took, beyond the routing itself:

- Four missing erased builders — `int_udiv_dyn`, `int_sdiv_dyn`,
  `int_urem_dyn`, `int_srem_dyn`. The family had stopped at the
  shifts.
- **Flag plumbing.** The erased builders built `BinaryOpData::new(..)` and left
  every flag false, so routing the parser through them as-written would have
  made `add nuw <2 x i32>` parse and print as a plain `add`. `IntBinOpFlags`
  carries all four flags for a caller holding a runtime opcode, and
  `BinaryOpcode::accepted_flags` drops the ones the opcode cannot express.
- **A vector-capable `icmp`.** `int_cmp_with_flags_dyn` is *not* part of
  the erased family — its `_dyn` means dynamic *width*, it routes through the
  scalar-only `IntoIntValue`, and it mints an `IntValueId<bool, B>` that cannot
  describe `<N x i1>`. `int_cmp_erased` computes the lane-matched result
  type and returns an erased id.
- **Operand-type validation** in the erased path, which previously left it to
  the verifier. A caller reaching it has a runtime type in hand and no
  conversion to bounce off, so an `and` on two floats would have built silently.

One follow-on fidelity fix in `value_tracking.rs`: `m_AllOnes` matching was
scalar-only, so `xor %v, splat (i32 -1)` was not recognised as a `not` and the
`(A & B) op ~(A | B)` case of `haveNoCommonBitsSet` failed on vectors.

~~Note the `_dyn` suffix now carries two meanings in `ir_builder.rs` — *erased
value* (`int_add_dyn`) and *dynamic width* (`int_cmp_with_flags_dyn`).
The repo's naming law reserves `_dyn` for the erased half of a typed/erased
pair, so the compare family is the misnamed one. Renaming it is a breaking
change and was left out of this fix.~~ **Resolved (2026-08-06)** by the
three-tier suffix vocabulary that landed with the 0.0.4 API-idioms renames:
`_dyn` means exactly one thing again — the `Dyn`-marker member of a
typed/erased pair (`int_add_dyn` and `int_cmp_with_flags_dyn` both qualify;
their operands lift at `IntDyn`) — and the fully-erased, vector-capable family
is spelled `_erased` (`int_binop_erased`, `int_cmp_erased`: erased `Value`
operands plus a runtime opcode). No rename of the compare family was needed;
what was missing was the vocabulary's third tier, now stated in the naming law
(`AGENTS.md`, Code Conventions).

## ~~KnownBits — operations not modeled~~ — closed (2026-08-01)

`KnownBits.h`'s **public** surface is now fully modeled. The ledger
(`crates/llvmkit-ir/tests/value_tracking_parity.rs`) asserts an empty gap list,
so a regression or a newly-synced upstream method has to be acknowledged rather
than absorbed.

Three were implemented to close it: `setAllOnes` → `set_all_ones`,
`isSignUnknown` → `is_sign_unknown`, and the plain two-argument `sdiv`
alongside the existing `sdiv_with_exact` (upstream spells the pair as one
function with `Exact = false` defaulted; llvmkit spells it as a pair, matching
`udiv` / `udiv_with_exact`).

Two entries an earlier revision of the ledger listed as gaps were **wrong in
both directions**: `flipSignBit` and `remGetLowBits` are declared outside
`public:` in `KnownBits.h`, and both already existed in llvmkit as
module-private free functions in `known_bits.rs`. They are neither public
upstream nor absent here, and the ledger now records them as such.

## KnownBits / ValueTracking — the PHI recurrence arm (2026-08-01)

`computeKnownBitsFromOperator`'s `Instruction::PHI` arm is ported, including
`matchSimpleRecurrence`. `llvm/test/Analysis/ValueTracking/recurrence-knownbits.ll`
is checked in verbatim and driven by
`crates/llvmkit-asmparser/tests/value_tracking_recurrence.rs`; twelve of its
fifteen functions reproduce their CHECK line exactly.

**Three do not, and in every case the reason is a missing transform rather than
missing analysis.** They are asserted to stay *unfolded*, so closing either gap trips
the test rather than passing silently.

| Function | Upstream CHECK | Why it is out of reach |
|---|---|---|
| `@test_mul` | `0` | Needs bit 1 of `%iv` known zero. The `mul` arm keeps only `min(countMinTrailingZeros(8), countMinTrailingZeros(2))` = 1 trailing zero. Upstream first canonicalizes `mul i64 %iv, 2` to `shl i64 %iv, 1`, and the *shift* arm then keeps all three trailing zeros of the start value — `@test_shl` is that same recurrence pre-canonicalized, and it does reach its CHECK. Closes when InstCombine's mul-by-power-of-two canonicalization runs. |
| `@test_and` | `2047` | Needs bits 11..63 known zero *and* bit 10 known **one**. The `and`/`or` arm only ever sets low zero bits, and `min(countMinTrailingZeros(1025), countMinTrailingZeros(1024))` is 0; the fallthrough intersection leaves bit 10 unknown. Upstream gets `2047` by simplifying the loop away, not from known bits. |
| `@test_or` | `2047` | As `@test_and`. The intersection *does* prove bit 10 is one here, but bits 11..63 stay unknown. |

Two further pieces of the arm are **not** ported because llvmkit does not model
what they read — the per-edge context instruction
(`RecQ.CxtI = P->getIncomingBlock(..)`) and the `m_Br(m_c_ICmp(..))` refinement
that narrows an incoming value by the branch condition guarding its edge.
Neither can make an answer wrong; each only leaves it weaker.

One divergence is **deliberate**. Upstream gates the incoming-value
intersection on `Depth < MaxAnalysisRecursionDepth - 1` and recurses at that
fixed depth, capping the search at one level. llvmkit recurses at `depth + 1`,
because it already terminates by a different mechanism (the `stack` set rejects
re-entering a value mid-computation) and because `compute_known_bits_inner`
memoizes on `(slot, query)` with no depth component — entering at a fixed deep
depth would cache the weak answer computed there and hand it to a later shallow
query. llvmkit therefore answers *more* precisely than upstream for a shallow
phi, never less. `@test_udiv_neg` witnesses it: llvmkit proves 60 leading zeros
where upstream proves none, and the fixture's own claim (bit 2 unknown) is
untouched. **If the cache ever becomes depth-keyed, revisit this** — the
upstream cap would then be portable as-is.

The `nsw` sign-inference paths of the `add`/`sub`/`mul` arm
(`makeNonNegative` / `makeNegative`) have **no ported fixture**: every
recurrence in `recurrence-knownbits.ll` uses `nuw` or no flag at all. Worth a
sweep for an upstream fixture that exercises them.

## Bare brands / `Branded` derive — home and follow-ups (2026-07-31)

- **`llvmkit-macros` is the permanent home of the `Branded` derive, not a
  stopgap.** Upstream LLVM's answer to drift-prone repetitive definitions is
  build-time generation (its lexer and parser both `#include` the
  TableGen-generated `Attributes.inc` — `LLLexer::LexIdentifier`
  (`LLLexer.cpp`), `tokenToAttribute` (`LLParser.cpp`)); the
  `llvmkit-tablegen` crate and the macros crate are this project's two arms of
  the same philosophy. The ecosystem norm agrees (`serde_derive`, `thiserror-impl` are
  permanent companion crates).
- **Optional simplification, not planned work:** RFC 3698 (`macro_derive`,
  rust-lang/rust#143549) would let the derive live inside `llvmkit-ir` as a
  `macro_rules!` derive. It is nightly-only with open stabilization blockers.
  If it ever stabilizes at or below the pinned toolchain, the migration is one
  re-export line (`lib.rs`'s `pub(crate) use llvmkit_macros::Branded`) — worth
  doing only if `llvmkit-macros` optionality matters then.
- **Fold backlog: ~30 hand-written bound-free impl families remain** (≈150
  impls) that predate the derive and could migrate to `#[derive(Branded)]`
  one family at a time: `Type`, `Value`, `IntValue`, `FloatValue`,
  `ArrayValue`, `VectorValue`, `ModuleRef`, `ModuleView`, `FunctionValue`,
  `CallInst`/`TypedCallInst`, the phi handles, `SwitchInst`/`IndirectBrInst`/
  `InvokeInst`/`LandingPadInst`/`CatchSwitchInst`, `IntType`/`FloatType`/
  `ArrayType`/`StructType`/`VectorType`, `TypedPointerValue`, `SsaBlock`, the
  SSA variables, `MetadataId`, `Instruction`. Each fold must compare the
  hand-written `PartialEq`/`Hash` field walk against the full field list and
  keep custom `Debug` bodies manual (several print computed values). Sound as
  they are; this is deduplication, not a fix.

## 0.0.4 program plan — deviation records (2026-07-31)

A post-freeze verification pass compared the shipped library against the
2026-07-24 id-first program plan. Four deviations; one resolved in code (the
supertrait drop above), three resolved by record — reality wins:

1. **`BasicBlockLabel` lives by design.** The plan's law 6 said "deleted",
   contradicting law 1 ("every handle survives as the view layer"). The
   implementation resolved the contradiction correctly: `BlockId` is the
   storable id, `BasicBlockLabel` the view it resolves to.
2. **The tag check lives at the id→slot boundary, not inside the arena
   accessors.** Plan invariant 5 predates A1's decision to make slots bare
   untagged indices; once slots carry no tag, `Context::value_data` has
   nothing to check. The check sits in `into_stored` / `resolve_in`.
   Verified unbypassable: public methods *return* raw slots (19 of them),
   none accept one, none construct one.
3. **`ValueSlot`/`TypeSlot` stay `pub`** — `llvmkit-asmparser` genuinely
   consumes them. Narrowing that surface (the parser carrying tagged ids
   instead) is folded into the Milestone 0 parser cycle, which reworks that
   crate anyway.

## Stringly-typed surfaces the 0.0.4 API-idioms sweep did not close (2026-08-06)

C-CUSTOM-TYPE ("do not use raw types where a bespoke one carries the
invariant") drove the 0.0.4 renames — `Signedness` for the signedness bools,
the per-opcode flag structs, `ModuleFlagBehavior` / `ModuleFlagKey` for module
flags, `NamedMetadataName` for named-metadata keys. Six surfaces are still raw
`String` or bare tuple where a type belongs. They are listed here so the next
sweep does not re-derive the list — and because five of the six are **not**
rename work: they need a port, a generated table, or a consumer that does not
exist yet. Each bullet says which. Where no reason is on record, the bullet
says that too rather than inventing one.

- **Debug-info enumerations are strings end to end.** Every `DW_TAG_*`,
  `DW_ATE_*`, `DW_VIRTUALITY_*`, `DW_LANG_*`, `DW_LNAME_*`, `DW_CC_*`,
  `DW_OP_*`, `DW_MACINFO_*` and `DW_APPLE_ENUM_KIND_*` word is lexed to its own
  `Token::Dwarf*` variant carrying the **full keyword text**
  (`crates/llvmkit-asmparser/src/ll_token.rs`), and the parser then collapses
  all nine — plus `DIFlag*`, `DISPFlag*`, `CSK_*`, and the emission-,
  name-table- and fixed-point-kind keywords — into a single
  `MetadataFieldValue::Enum(String)` (`parse_metadata_field_value`,
  `ll_parser.rs`). Nothing validates the spelling, and a consumer that wants
  the tag has to string-match. The typed form is the `Dwarf.def` tables
  (`llvm/BinaryFormat/Dwarf.def`, surfaced as `llvm::dwarf::Tag` and friends in
  `Dwarf.h`) — several hundred constants across nine families, which is
  generation work for `llvmkit-tablegen`'s sibling arm rather than a hand-typed
  enum. **Deferred to the debug-info/metadata round-trip work**
  (`ROADMAP.md`, Milestone 12), where a consumer that needs the values exists.

  One consequence is worth stating plainly. `DIExpression` operands
  (`DwarfExpressionOperand::Operation`) keep their source spelling instead of
  the `uint64_t` upstream stores through `dwarf::getOperationEncoding` /
  `getAttributeEncoding` (`LLParser::parseDIExpressionBody`).

  **Half of that gap closed in LLParser-parity W11 (2026-08-15).** The
  spellings are now *validated* against the same tables on the way in, so an
  unrecognised `DW_OP_*` or `DW_ATE_*` is rejected with upstream's
  `invalid DWARF op '...'` / `invalid DWARF attribute encoding '...'` rather
  than round-tripping silently. What remains is storage: llvmkit keeps the
  name, upstream keeps the encoding, and the difference is only observable
  through **normalisation** — `!DIExpression(15)` prints back as `15` here
  where `llvm-dis` prints `DW_OP_...` for the operation that value encodes.
  Closing it needs the reverse tables to be consulted at print time, which is
  this same milestone.

  The parity plan recorded this divergence as *closed* at W11. It was not: the
  claim was written from the plan's own intent rather than from the tree, and
  the operands were still unvalidated names when W11 opened them. Recorded
  here because a false "closed" is worse than an open item.
  [[verify-recorded-premises]]
- ~~**`DIFlags` / `DISPFlags` are not bitflags.**~~ **Closed 2026-08-29
  (divergence-closing W9).** `metadata::DiFlags` and `metadata::DispFlags` are
  the two `u32` bitfields, carrying ports of `DINode::getFlag` /
  `getFlagString` / `splitFlags` and their `DISubprogram` twins;
  `MetadataFieldValue` grew a variant for each, `parseMDField`'s two typed
  overloads are ported as `parse_di_flag_field` / `parse_disp_flag_field`, and
  the printer emits through `splitFlags` the way `MDFieldPrinter::printDIFlags`
  and `printDISPFlags` do.

  What that reasoning had missed, and what the joined-source-text
  representation could not express: `parseFlag`'s **first** arm is an unsigned
  `lltok::APSInt`, so `flags: 4 | DIFlagPublic` is legal upstream and was
  rejected here; and `printDIFlags` re-derives the printed form rather than
  echoing it, so a written order, a duplicate term, an alias spelling and a
  zero field were all wrong on output. The recorded reason — that the bitflag
  type is only worth its keep once something reads it — was true of storage and
  false of the parser and the printer, both of which already read it.
  [[verify-recorded-premises]]
- **`MetadataField::name` is a `String` — validated, but still stringly typed.**
  ~~Nothing validates it~~ — **the divergence half is closed (2026-08-07).**
  `SpecializedMetadataKind::fields` / `required_fields` port each class's
  `VISIT_MD_FIELDS` block from `LLParser.cpp`, and the specialized-node loop in
  `ll_parser.rs` now makes all three of upstream's rejections: `invalid field
  '<name>'` (the `PARSE_MD_FIELD` fall-through), `field '<name>' cannot be
  specified more than once` (`LLParser::parseMDField`'s `Result.Seen` guard),
  and `missing required field '<name>'` (`REQUIRE_FIELD`, reported against the
  closing `)`). `!DILocation(lien: 3)` no longer parses. Ported from the
  `test/Assembler/invalid-di*.ll` family.

  What remains is the ergonomics half: the field name is still a `String`
  checked against a `&'static [&'static str]` table, not a per-node
  `MetadataFieldName` enum. That enum is worth writing only alongside the
  per-node modeling it would key, so it stays with the same milestone. The
  table also cannot be drift-tested the way `attribute_td_drift.rs` tests
  `Attributes.td`, for two reasons worth recording so nobody re-derives them:
  that `.td` is **vendored and tracked** under
  `crates/llvmkit-asmparser/tablegen/`, whereas the field lists live in
  `LLParser.cpp` under the **gitignored** `orig_cpp/`, so a test reading it
  would pass locally and fail in CI; and even vendored, the lists are
  `VISIT_MD_FIELDS` preprocessor macro blocks rather than a re-readable `.def`.
  Vendoring an 8k-line C++ file to scrape macro text is a far larger commitment
  than vendoring a `.td`. The two halves are pinned against each other instead
  (`required_fields ⊆ fields`, per kind), which catches a typo'd or dropped
  required field but not upstream adding one.

  **Half of that reason is now spent (W14b, 2026-08-16).** `LLLexer.cpp` and
  `LLToken.h` *are* vendored under `crates/llvmkit-asmparser/tablegen/`, and
  `lexer_token_drift.rs` scrapes their `KEYWORD` / `TYPEKEYWORD` /
  `INSTKEYWORD` / `DWKEYWORD` macro tables with a paren-balanced reader — so
  "vendored and tracked" is no longer a property only `.td` files have, and
  scraping C++ macro text is no longer unprecedented. What still holds for
  `LLParser.cpp` is the size (8k lines against `LLLexer.cpp`'s 1.2k) and the
  shape: `VISIT_MD_FIELDS` interleaves the field name with a *type* and a
  default, so a reader must parse three things per entry rather than lift one
  identifier. Weigh it on those grounds now, not on the vendoring question.
- **`Module::target_triple` returns `Option<String>`.** The triple is stored
  and printed verbatim, never decoded, so nothing can ask for the architecture,
  vendor, OS or environment without re-splitting the string. A structured
  `Triple` needs a port of `llvm::Triple` (`llvm/TargetParser/Triple.h`) with
  its normalization rules and its several hundred enumerator spellings.
  **Deliberately not started**: target parsing sits next to the code-generation
  and target-backend work that is permanently out of scope, so the port would
  have to justify itself on IR-level consumers alone (DataLayout consistency
  checks are the only candidate today).
- **`SourceMap::line_col` returns a bare `(u32, u32)`** (`llvmkit-support`), so
  a caller can transpose line and column with no complaint from the compiler —
  exactly the shape C-CUSTOM-TYPE exists to prevent. A `LineCol { line, column }`
  struct is a contained change with one in-tree consumer (the parser's own
  diagnostics) and no dependency on anything else — it is the smallest item in
  this list. No reason for the 0.0.4 sweep passing it over is recorded, so do
  not assume one; take it whenever `llvmkit-support` is next touched.
- ~~**Inline-asm constraints are never parsed.**~~ **Closed 2026-08-12**
  (LLParser parity W4). `parse_constraints` / `verify_inline_asm` in
  `inline_asm.rs` port `InlineAsm::ParseConstraints` and the static
  `InlineAsm::verify`; `ConstraintInfo` carries the typed records, all nine
  `verify` messages are reachable from the parser, and the `!`-counting
  heuristic behind `label_constraint_count` is gone along with the
  `arg_constraints`-hardcoded-to-`0` summary struct. The per-operand
  `elementtype` half of `Verifier::verifyInlineAsmCall` followed on
  **2026-08-27** (`ConstraintInfo::has_arg` plus the three `Check`s, driven by
  `test/Verifier/inline-asm-indirect-operand.ll`); the reason recorded here for
  deferring it — "the call surface cannot spell per-operand `elementtype`
  attributes yet" — was already stale when written, since the parser stores
  per-argument attribute lists and the AsmWriter prints them back. What is
  *not* ported: the `Flag` / `ConstraintCode` bit encodings, which are backend
  serialization and out of scope.

## Killer-feature designs (deferred)

- **Inline IR macro DSL** -- a `ir!{ %sum = add i32 %a, %b }` proc-macro
  added to the EXISTING `crates/llvmkit-macros/` crate (which already ships
  the `IrStruct` derive in `ir_struct.rs`; new sibling module `ir.rs` per the
  one-concept-per-file convention). Expands `.ll`-flavored syntax into typed
  builder calls at compile time, with typed Rust splices (`#lhs`)
  type-checked against the spelled IR types. Reuses `llvmkit-asmparser`'s
  lexer at proc-macro time for tokenization fidelity. Design sketch: parse to
  the existing instruction payload shapes, emit builder calls; unsupported
  constructs fall back to a clear compile error naming the LangRef construct.
- **Rustc-quality diagnostics** -- when runtime checks do fail (dyn paths,
  parsed IR, verifier), render labeled spans into the printed IR with
  expected/found notes and suggestion hints. Builds on `llvmkit-support`'s
  `Span`/`SourceMap` (already used by the parser) plus a renderer; verifier
  errors gain an optional pretty-print path that quotes the offending
  instruction line from AsmWriter output. Candidate crate: keep in
  `llvmkit-support` as a `diagnostics` module.

  **The crate graph is the real constraint, and this sketch skips it.**
  `llvmkit-ir` does not depend on `llvmkit-support` — its only intra-workspace
  dependency is `llvmkit-macros` (`crates/llvmkit-ir/Cargo.toml`), and no file
  under `crates/llvmkit-ir/src/` names `llvmkit_support`. Support enters the
  graph one level up, at `llvmkit-asmparser`. So the verifier half of this
  design needs a **new** `llvmkit-ir → llvmkit-support` edge, or the renderer
  has to live above both. Decide that before writing any of it; the parser half
  is unaffected and could ship alone.

## Upstream IRBuilder coverage gaps (from the comparison audit)

Signatures below are verified against the extracted `llvmorg-22.1.4` tree
(`orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/IR/IRBuilder.h`).

- Convenience casts: `CreateZExtOrTrunc`, `CreateSExtOrTrunc`,
  `CreateIntCast`, `CreateFPCast`, `CreateBitOrPointerCast`.
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
- Named `icmp_*` per-predicate wrappers already exist; audit found no
  gap there.
- Debug-loc threading and operand-bundle infrastructure (deferred with
  metadata work).
- RAII-style `InsertPointGuard` / `FastMathFlagGuard` analogs (Rust shape:
  scoped closure `with_insert_point(bb, |b| ...)` rather than Drop guards).

## Ergonomics backlog (from the core audit)

- `Display` for the 39 typed instruction handles (`LoadInst`, `CallInst`, …;
  every `pub struct *Inst` under `crates/llvmkit-ir/src/`, all 39 re-exported.
  This read "~25" until it was re-counted 2026-08-06).
  Cycle C gave `Display` to every public *value* handle, which prints the
  operand form, but deliberately stopped at the instruction handles: their
  natural rendering is a full instruction line, not an operand, so they need
  an explicit decision (delegate to `InstructionView`'s `Display`, or print
  the operand form for consistency with the value handles) rather than a
  mechanical sweep. Whichever is chosen should be stated in each impl's
  rustdoc, as the value handles now do.
- `atomic_cmpxchg` / `atomicrmw` builder-pattern variants (mirror
  `CallBuilder`).
- ~~Load/store variant explosion (base / `_with_align` / `_volatile` /
  `_volatile_with_align` / `_atomic` = 10+ methods per op) -- consolidate
  behind `LoadBuilder`/`StoreBuilder` chainables while keeping the flat
  forms.~~ **Done (2026-08-06, W10 of the 0.0.4 API-idioms program):**
  `IrBuilder::load_from(ptr)` and `store_to(value, ptr)` open the two
  chainables (`.volatile()` / `.align()` / `.atomic()` / `.sync_scope()`, with
  a typed terminal — `.int::<W>(name)` / `.fp::<K>(name)` / `.pointer(name)` /
  `.typed::<T>(name)` / `.erased(ty, name)` — picking the load's result shape),
  and `alloca_builder(ty)` does the same for `alloca`. The flat forms stayed;
  the seven flag-*combination* forms (`load_volatile`,
  `load_volatile_with_align`, `store_volatile`, `store_volatile_with_align`,
  `int_load_atomic`, `load_atomic`, `store_atomic`), `alloca_dyn`, and the
  `AtomicLoadConfig` / `AtomicStoreConfig` bags were deleted, so each knob has
  one spelling.
- Per-flag convenience wrappers (`int_add_nsw` etc.) mirroring upstream
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
  natural home as a `PatchBody`/`ReshapeCfg`-rung pass family. Would be a
  genuine next-generation differentiator: phase-ordering-free peepholes.
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

- ~~**Block-argument edges are only half-guarded, and only on half the
  terminators**~~ (found 2026-07-27 re-reading
  `docs/design/phi-type-guarantees-design.md` against the tree; the design
  promised both halves and neither was noticed missing for 17 days, through the
  0.0.4 freeze) — **done (2026-07-27, `feature-34/polish-freeze`).** Both gaps
  were closed in one change, as the entry insisted they had to be: closing
  either alone would have left `br`/`cond_br` guarded while `switch`/`invoke`
  silently were not.

  1. **A plain branch into a parameterised block is now rejected.**
     `IrBuilder::br` / `cond_br` / `switch(_dyn)` (default
     edge) / `SwitchInst::add_case` / every `invoke*` (both edges) /
     `callbr*` (default and indirect edges) /
     `IndirectBrInst::add_destination` all route their successors through one
     guard, `basic_block.rs::require_no_block_parameters`, which reports the
     existing `IrError::PhiArgArityMismatch` — the same error a wrong argument
     *count* already got from `br_with_args`, so one mistake reads the
     same wherever it is caught. The check runs before the terminator is
     emitted, so a rejected edge leaves no half-formed instruction.

     **The early-out is not the one this entry proposed, and the difference is
     load-bearing.** "Is the target's first instruction a phi?" would have
     broken the two authoring paths that legitimately branch into a block whose
     head-phis are not block *parameters*: the `.ll` parser (a back-edge to an
     already-parsed loop header) and `SsaBuilder` (a back-edge to an unsealed
     header whose reads have minted operandless phis, completed later at
     `seal_block`). Both seed those phis through their own checked paths, and
     neither can spell an argument list. So a block instead records the
     parameter count it was *created* with — a `Cell<usize>` on
     `BasicBlockData`, set only by `append_block_with_params`,
     `append_block_with_named_params`, and `append_block_typed` — and the guard
     early-outs on that single read. It is cheaper than the proposed scan-gate
     (no instruction-list touch at all on the hot path) and it is the more
     honest predicate: *parameterised* is a property of how the block was
     authored, not of what its first instruction happens to be. The scan
     survives as the shared `block_parameter_phis`, which both `add_block_args`
     and the guard's error message read, so "how many parameters" has one
     definition. `plain_branch_into_auto_ssa_phi_block_still_builds`
     (`tests/block_args_terminators.rs`) pins the distinction.

  2. **`switch` and `invoke` have argument-carrying forms.**
     `switch_with_args` / `switch_dyn_with_args` take the default
     edge as a `(target, args)` pair plus a `(case_value, target, args)` triple
     per case — the whole case list at the call, so the returned `SwitchInst`
     is already `TermClosed` and no later `add_case` can bolt on an unseeded
     edge. `invoke_with_args` / `invoke_dyn_with_args` take a
     `(destination, args)` pair for each of the two mandatory edges. All four
     bundle each edge with its arguments into one parameter (the case list
     forces that shape, and it keeps `invoke`'s call arguments and result name
     out of an eight-parameter signature), and all four validate arity and
     argument types up front, per edge, exactly as `cond_br_with_args`
     does — sharing its documented non-atomicity across edges.

  **Residual, deliberately reject-only:** `indirectbr`, `callbr`, and the
  *indirect-callee* and *inline-asm-callee* `invoke` shapes have no
  argument-carrying twin — an `indirectbr`/`callbr` indirect edge is selected at
  run time, so there is nothing to hang a per-edge argument list on, and the two
  exotic invoke callees would need their own signature explosion for no known
  consumer. A parameterised destination is rejected there rather than silently
  under-seeded; authoring a phi in such a block goes through `SsaBuilder` or
  `FnReshape::insert_phi`, on a block created with plain `append_basic_block`.
  For `indirectbr` this is precisely the restriction the design asked for.

- ~~**Six `#[cfg_attr(not(test), allow(dead_code))]` violate the `#[allow]` ban**~~
  — **done (2026-07-27, at the 0.0.4 freeze).** AGENTS.md bans `#[allow(...)]`
  unconditionally — "not anything else" — and a `cfg_attr` wrapper does not
  exempt it. All six sat on the crate-internal raw-phi authoring surface
  (`PhiInst`/`FpPhiInst`/`PointerPhiInst::add_incoming` in `instructions.rs`,
  and `IrBuilder::int_phi`/`fp_phi`/`pointer_phi`), which went
  dead in non-test builds when block arguments took over as the public
  phi-authoring surface.

  Fixed the way the law prescribes — drop the dead code — by marking all six
  `#[cfg(test)]`, since their only callers were ever `src/phi_raw_tests/`. Two
  imports (`IntoFloatValue`, `IntoPointerValue`) became test-only with them and
  are gated the same way rather than suppressed.

  This re-pointed `tests/compile_fail/raw_phi_builder_is_unnameable.rs`: it used
  to prove the builders are *private* (`E0624`) and now proves they *do not
  exist* in a dependent crate's build (`E0599`). That is the stronger claim — a
  private method still exists and a later `pub` slip would expose it, whereas a
  method compiled out cannot be reached at all. Fixture doc comment rewritten
  and `.stderr` regenerated on 1.96.0.

- ~~**Metadata is the one currency the id-first redesign did not tag**~~ —
  **done (2026-07-27, at the 0.0.4 freeze).** Found during the cycle E freeze
  sweep and pre-existing rather than a regression: the id-first handle redesign
  (cycles A–E, once informally called "2.0" — an internal codename, never a
  version) tagged the *value* currency and left this one behind.
  `MetadataSlot` was a bare `usize` arena index and the `ValueSlot`
  inside `DebugMetadataOperand::Value` was likewise bare, so **neither half of
  D7 reached metadata** — no `B` for two modules' handles to differ in, and no
  tag for the arena boundary to check. An *in-range* slot minted in module A and
  attached in module B resolved against **B's** arena in
  `asm_writer::fmt_debug_metadata_operand` / `fmt_metadata_operand` and printed
  the wrong node, silently. Cycle E had bounded the *reachability* of that
  mistake (every attach point demands the target module's `Unverified` token,
  and out-of-range slots became `IrError::UnknownMetadataSlot`) without closing
  the hole.

  Closed by mirroring cycle A's value split exactly:

  - `MetadataSlot` stays the bare arena index and became **crate-internal**,
    alongside `MetadataStore`. It is reachable only from the storage side.
  - **`MetadataId<B: ModuleBrand>`** is the public currency —
    `{ tag: ModuleId, slot: MetadataSlot }`, `Copy + Send + 'static`, brand
    phantom `PhantomData<fn(B) -> B>`. `MetadataRef` (a `pub` newtype with a
    `pub` field — a forgery hole of its own) is gone; `MetadataId` replaces it
    everywhere.
  - Every vocabulary type that carries a metadata reference gained the `B`:
    `MetadataKind`, `SpecializedMetadataNode`, `MetadataField`,
    `MetadataFieldValue`, `DebugRecord`, `DebugVariableRecord`,
    `DebugMetadataOperand` (whose `Value` arm now carries `ValueId<B>`), and
    `MetadataAttachmentSet`. `ModuleCore` is brand-free and cannot store a
    generic, so the arena holds those same types at a crate-private
    `StoredBrand`; the two forms meet at exactly two crate-internal
    conversions — `into_stored`, which performs the tag check, and
    `from_stored`, a pure retag of ids the arena already owns.
  - The check lands at **one** choke point. `MetadataId::slot` exists only on
    `MetadataId<StoredBrand>`, so the only route from a caller's id to an arena
    index is `MetadataId::into_stored` / `ModuleCore::metadata_slot_of`, which
    compares the `ModuleId` first. Forgetting the check one level up is not
    expressible.
  - A foreign id is **`IrError::ForeignMetadataId`** (new; the metadata twin of
    `ForeignValueId`) on every entry point that accepts one, which made
    `metadata_tuple` / `metadata_tuple_with_distinct` / `metadata_specialized` /
    `metadata_node` / `metadata_as_value`, every `set_metadata` setter
    (instruction, function, and the three globals), and `push_debug_record`
    fallible. `metadata_constant` joined them for the same reason on the value
    side. `UnknownMetadataSlot` keeps the
    out-of-range case, now reachable only for a *native* id.

  The `.ll` parser holds `MetadataId<B>` in its `!N` bookkeeping and needed no
  raw-slot escape hatch: every id it hands back was minted by the module it is
  populating. Printed IR is byte-identical — the byte-locked example suites and
  the parser round-trip corpus are unchanged. Locked by
  `tests/module_ownership.rs::a_metadata_id_from_another_module_is_refused_everywhere`
  (runtime tag, two `DynBrand` modules with identically-shaped arenas) and
  `tests/compile_fail/cross_module_metadata_attachment.rs` (two named brands).

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
  `vector_int_{add,sub,mul,xor,and,or,shl,lshr,ashr}` (two
  `VectorValue<E, Len<N>>` with the same `E`,`N` ⇒ equal element+length for
  free), `vector_extract`/`vector_insert`/`vector_splat`, and array
  `array_extract`/`array_insert` (plus a typed-array `alloca`); the
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
  - `vector_splat` can't infer its element from the scalar (a Rust
    associated-type-projection limitation), so its callers annotate / turbofish
    the result.
- **A proof token that *carries* the validated `TypeSlot`** (residual after the
  unforgeable-markers cycle). The crate has five capability tokens -- `WrapWitness`
  (`element.rs`), `ValidatedFunctionParams` / `ValidatedCallResult`
  (`function_signature.rs`), `SelectNarrow` (`ir_builder.rs`), `ValidatedStructValue`
  (`struct_schema.rs`) -- each defending the *external* boundary and each a *unit*
  marker that proves "a check happened", not *which* type. The unforgeable-markers
  cycle made the builder's **int / float / pointer append surface** structural instead:
  a marker is attached to a freshly-appended instruction only through the typed-append
  constructor family (`append_int_{like,at,load}`, `append_fp_{like,at,load}`,
  `append_ptr` / `append_ptr_load`), each of which appends AT a typed handle so the marker matches
  the runtime type *by construction* — those ~40 sites no longer carry an implicit proof.
  What remains implicit is the smaller residual the family does not cover: the `CallInst`
  / `PhiInst` result accessors in `instructions.rs`, the arena / parameter lifts in
  `ssa_builder.rs` (`use_*_var`) and `function_signature.rs`, the vector / array append
  wraps (no `append_vec` / `append_arr` constructor yet), and the `IntoIntValue` /
  `IntoFloatValue` const-lifts in `int_width.rs` / `float_kind.rs`. A witness carrying the
  validated `TypeSlot` would let those *state* their proof instead of implying it —
  llvmkit has no `TypeId`, so the slot is the only thing there is to carry (types
  are reached as `Type<'ctx, B>` views, never as tagged ids). Note the
  confinement of `from_value_unchecked` is **audited, not compiler-enforced**: it stays
  `pub(crate)` because a hard seal is impossible (`value` and `ir_builder` are sibling
  modules and the constructors need `ir_builder`-private helpers), so the builder's fold
  re-checks remain the runtime backstop.
- `Width<M>`/`Width<N>` `WiderThan` relations blocked on stable
  const-generics (documented under "Limitations on stable Rust" on `Width`'s
  own rustdoc, `int_width.rs`); revisit when `generic_const_exprs` stabilizes.
- Aggregate variable categories for auto-SSA (currently ships int/float/pointer
  only).
- Address-space-typed pointers (`PointerValue` currently erases address
  space; audit item from infra report).
- **Infallible statically-sized aggregate constants.**
  `ArrayType<E, ArrLen<N>>::const_array([C; N])` /
  `VectorType<E, Len<N>>::const_vector([C; N])` could drop the `IrResult`
  entirely — the length is known at compile time and the elements would be
  materialized at the type-level element `E` — but this needs typed
  `ArrayType<E, ArrLen<N>>` / `VectorType<E, Len<N>>` **type constructors**
  first: today every `const_array`/`const_vector` caller builds an *erased*
  `ArrayType<ElemDyn, ArrLenDyn>` / `VectorType<ElemDyn, LenDyn>` via
  `m.array_type(...)` / `m.vector_type(...)`, so an infallible static
  constructor would be dead code (no statically-typed receiver exists to call
  it on). The fallible `const_array`/`const_struct`/`const_vector` now accept
  `impl IntoConstantValue` elements (literals work), but stay `IrResult` — the
  element-vs-container type check still guards the erased receivers. Bundle the
  infallible variants with a typed aggregate-**type**-constructor slice; the
  same applies to a `StructSchema`-keyed tuple-struct constant.

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
- **Typed vector-GEP handles** -- `IrBuilder::gep_erased` builds a `<N x ptr>`
  base and `<N x iM>` indices, and returns the erased `ValueId` because a
  vector GEP's result is no `PointerValue`. The typed tier (`gep`,
  `gep_with_flags`, `field_gep`) still takes a scalar `IntoPointerValue` base
  and `IntoIntValue<IntDyn>` indices; a `VectorValue`-shaped typed door on top
  of `gep_erased` is mechanical follow-up.
- **Derive-generated field-index consts** -- `field_gep::<S, I>` takes
  the field index as a bare `const I: u32`; the derive macro could emit named
  constants (e.g. `Point::X_INDEX`) so call sites read `field_gep::<Point,
  { Point::X_INDEX }>` instead of a magic number.
- **`TypedInvokeInst<Ret>` schema wrapper** -- `invoke` returns
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
- **`IrField::ir_type` accepting a module view -- done** (0.0.4 cycle C,
  `feature-32/owned-modules`). `IrField::ir_type`, `StructSchema::field_types` /
  `ir_type`, `FunctionReturn::ir_type`, `FunctionParam::ir_type` and
  `FunctionParamList::ir_types` now take `ModuleView<'ctx, B>` instead of
  `&Module<..., Unverified>`, so `field_gep` no longer wraps a temporary
  module token to call `S::ir_type(...)` — `Module::from_core` is gone with it.
  Type construction is preservation-neutral, which is why the view already
  carried the constructor surface.
- ~~**`proptest` `undef_var` index randomization** -- the auto-SSA property
  test suite's undefined-variable-read fixture hardcodes `Some(0)` as the
  undefined variable index instead of drawing from `0..var_count`; a one-line
  improvement to widen coverage (noted during Task 19's review).~~ **Done
  (2026-07-06, `6d2fb24`) — struck 2026-08-06 after re-reading the fixture.**
  `generated_case_strategy` in `crates/llvmkit-ir/tests/ssa_builder.rs` draws
  the index with `proptest::option::of(0_usize..var_count)`, and its own
  comment states why: the emitters advertise a randomised which-variable
  schedule, so every declared variable has to be exercised as the undefined
  one, not just the first. The entry outlived the fix by a month.
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
`llvmkit-default<On>` recipe rename). The `is_null`/`pointer_cmp`
folder-bypass item was already fixed on dev (`b06413e`). The items below remain
deliberately deferred; each cites its upstream anchor.

- **DCE removable calls / allocs** -- llvmkit still keeps `willReturn`+readnone
  calls, removable allocation-function calls, `free(null)`, and lifetime-only
  allocas that upstream `wouldInstructionBeTriviallyDead`
  (`lib/Transforms/Utils/Local.cpp`) deletes. Porting these needs faithful
  allocation-function / attribute modeling to avoid over-removal (a miscompile
  if wrong), so the current DCE stays conservative-but-safe. `Value::has_uses`
  also counts debug-record uses upstream ignores (upstream salvages debug info
  instead). (Unordered atomic loads are now removed.)
- **InstSimplify freeze folds** -- no InstSimplify test covers a `freeze`
  fold yet. (The unreachable-block skip that used to be named here is closed:
  the pass declares `DominatorTreeAnalysis` and carries `runImpl`'s
  `if (!SQ.DT->isReachableFromEntry(&BB)) continue;`, pinned by
  `crates/llvmkit-ir/tests/scalar_cleanup_passes.rs::instsimplify_leaves_unreachable_blocks_alone`.
  The erase-only-when-trivially-dead behavior was already matched.)
- **Deeper `swifterror` dataflow verification** -- the swifterror alloca
  support verifies the parse-level constraints (pointer type, non-array); the
  full `Verifier` use-site rules (swifterror values may only flow through
  specific call/load/store positions) are not yet enforced.
- **Plain add/sub/div/shift hook dispatch** -- `int_add`/`int_sub`
  consult the plain `fold_int_bin_op` hook where upstream `CreateAdd` funnels
  through `FoldNoWrapBinOp(.., false, false)` (and `CreateUDiv` et al. through
  `FoldExactBinOp(.., false)`). Identical results with the shipped folders;
  observable only by third-party folders that override just the
  no-wrap/exact hooks.
- **Vector-GEP `computeKnownBits` coverage** -- `gep_known_bits`
  (`value_tracking.rs`) runs on vector GEPs now that `IrBuilder::gep_erased`
  builds them, conflating lanes the way upstream's `computeKnownBits` does and
  bailing to unknown wherever an index is not a scalar `ApInt`. No upstream
  `ValueTracking` fixture covers a vector GEP, so there is nothing to port;
  the behaviour is reasoned, not pinned.

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
- **Per-function analyses in `ModRewrite::patch_functions` /
  `reshape_functions`** -- the module->function iterators (`pass_context.rs`)
  build each per-function mutator with empty results `()`, so a
  `FnPatch::analysis` call inside the loop has no members to select. A future
  revision threads a per-function `Requires` list (prefetched per yielded
  function) through them.
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
- **Typed `BlockCall` redirect twins -- REFUTED, not deferred.** A typed
  `redirect_*_call(BlockCall<Params>)` surface on the pass-side edit handles
  (`BrEdit`/`SwitchEdit`/`InvokeEdit`/`CallBrEdit`, `pass_context.rs`) would
  move the phi-seeding arity/type check from run time to compile time, the
  redirect analog of the shipped typed *creation* path
  (`br_call`/`cond_br_call`). It was planned for cycle D and cut
  after a four-source audit found it out-types every relevant consumer:
  - **LLVM** has no atomic redirect at all -- the idiom is a manual two-step
    (`replaceSuccessorWith` / `SwitchInst::setSuccessor` then
    `BasicBlock::removePredecessor` to fix phis by hand). llvmkit's *erased*
    `redirect_*(new_to, phi_values)` is already stronger (atomic, validated).
  - **MLIR** -- the one upstream built around block arguments -- passes
    successor operands *dynamically* (`BranchOpInterface::getSuccessorOperands`
    returns a mutable `SuccessorOperands` range checked by the runtime
    `verifyBranchSuccessorOperands`), not by a static per-block signature.
  - **Mergen** (the C++ lifter this project's `bin_lift` consumer reimplements
    and extends) redirects/rebuilds edges only on *runtime-recovered* CFGs:
    `CustomPasses.hpp` rebuilds a `switch` with a runtime case count
    (`CreateSwitch(op, default, newNumCases)` + `addCase` over discovered
    constants) and seeds phis by looping `addIncoming(v, cb)` over runtime
    predecessor vectors, returning `PreservedAnalyses::none()` -- the erased
    `ReshapeCfg` floor. A static `BasicBlockLabel<Params>` schema is unknowable
    in principle there: case counts, targets, and phi incomings are *recovered*,
    not *authored*.
  - **`bin_lift`'s `lift-core` `IrBuilder`** is creation-only (`new_block` /
    `br` / `cond_br` / `switch`), with no redirect/split/phi/block-param surface.

  Because pass-side labels arrive erased-by-origin (from
  `FunctionView::basic_blocks()`), a typed twin would also force a narrowing
  round-trip (`try_into_typed::<Params>`) that the erased redirect avoids. Typed
  `BlockCall` earns its keep at block *creation*, where the typed label is in
  hand; it has no consumer at pass-side redirect. Revisit only if a pass appears
  that authors fresh, statically-shaped merge blocks *and* redirects existing
  edges into them -- a shape none of the four surveyed systems exhibits.
- **First-class `ModRewrite` runtime-symbol/global/ctor triple** -- the
  `RewriteModule` mutator (`ModRewrite`, `pass_context.rs`) exposes only the raw
  `module_mut()` token today; a sanitizer reaches the
  function/global/constructor "triple" through it by hand. The author sugar for
  that pattern -- `declare_runtime_fn` / `append_ctor` / `add_global` helpers
  plus the `llvm.global_ctors` machinery -- is deferred until an in-tree
  consumer needs it.
- ~~**`Module::scratch_unverified` footgun**~~ -- **done (2026-07-26, cycle
  cycle C1)**. `scratch_unverified` doesn't exist anymore. `feat(cycle C1): Module
  owns its ModuleCore` replaced it with `Module<B, Unverified>::assume_verified`
  (`module.rs`, still `pub(crate)`), called from the read-only `Dyn`
  pipelines' `run` methods (`DynReadOnlyFunctionPipeline::run` /
  `DynReadOnlyModulePipeline::run`, `pass_manager.rs`):
  `module.unverify()` hands out the token, every queued pass is
  `Inspect` so it projects to `()` and never reaches a mutator, and
  `unverified.assume_verified()` re-stamps that same token `Verified` with no
  re-verification. The original footgun doesn't apply anymore: a `Module` now
  *owns* its core by move rather than pointing at shared storage, so there is
  no way to mint a second live token over the same data. `assume_verified`
  round-trips the one token the pipeline already holds -- it doesn't conjure a
  throwaway one, so the caller-marker gap the old bullet worried about has
  nothing left to guard against.
- ~~**Compile-fail `.stderr` canonical-rustc bless**~~ -- **done (2026-07-26,
  cycle cycle D/E)**. There were never two "environmental" drifts: gated on the
  pinned CI toolchain (`cargo +1.96.0`), `folder_typed_wrong_width` and
  `extract_value_empty_indices` both pass, and the whole suite's baseline is
  **0 failures** across the registered fixtures — `CLAUDE.md` carries the live
  count, which this entry used to duplicate and had let drift to 83. The
  mismatch only ever appeared
  when the suite was run on a *newer* rustc than the pin. Every `.stderr` in
  the tree is blessed on 1.96.0; re-bless there and nowhere else.

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
- **`ModRewrite::reshape_functions` reshape flush.** The module→function
  iterator yields `FnReshape` mutators whose `done()` (and thus `CfgUpdate` log)
  never reaches the driver, so those reshapes do not run the witnessed flush.
  This is sound today: the enclosing `RewriteModule` floor is `none()`, which
  evicts every CFG analysis anyway. Reclaiming the lost *precision* means
  raising that floor above `none()`, which in turn means witnessing the
  module-structural mutation `ModRewrite::module_mut` deliberately hands out
  unwitnessed -- so this waits on a decision about `module_mut`, not on
  plumbing alone. Wire the flush through the iterators if per-function analyses
  are ever threaded into them.
- **New `.stderr` under the canonical-rustc bless caveat.**
  `reshape_stale_cfg_analysis_across_edit` is blessed on the local rustc like
  the pass-API fixtures above; its `E0502` borrow-error wording is stable
  across toolchains, but it joins the set that should be re-blessed on the
  reference rustc.

## Phi authoring — shipped

The block-argument authoring surface (`append_block_with_params`,
`append_block_with_named_params`, `*_with_args`), dominance-witnessed
`FnReshape::insert_phi`, the "break" that made the raw phi builders (the six
`*_phi`, the open-phi `add_incoming`/`finish`) internal — block arguments
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
the parser) have all shipped.

One more shipped alongside them and has since been **withdrawn**: a
zero-incoming-phi verifier backstop (`VerifierRule::PhiEmptyInReachableBlock`),
justified on the ground that a bracket-less `%p = phi i32` has no legal textual
form. It has one — `AsmWriter`'s phi arm prints the type and then an empty
`ListSeparator` loop, `LLParser::parsePHI` and llvmkit's `parse_phi` both stop
their pair loop at the first token that is not `[`, and
`test/Assembler/zero-input-phi.ll` round-trips exactly that through
`llvm-as | llvm-dis`. The rule rejected IR LLVM accepts, so it is gone;
`check_phi_incoming`'s `numIncoming == numPreds` guard — upstream's own, from
`Verifier::visitBasicBlock` — is the only length rule again.

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
erased authoring is unchanged; `IrBuilder::append_block_typed::<Params>` appends
a `Params`-stamped block with typed head-phi parameter handles; and the
`BlockCall<'ctx, R, B, Params>` edge (`head.call(args)` consumed by
`br_call`/`cond_br_call`) makes a wrong-arity or wrong-typed
block-argument a *compile* error. The erased surface
(`append_block_with_params`, `br_with_args`, `cond_br_with_args`) is
untouched. Two follow-ups remain deferred:

- **Edit-surface `BlockCall` integration.** The reshape edit surface's typed
  `redirect_*` phi-seeds stay erased (`&[Value]`): passes operate on `Dyn`
  block labels (`BasicBlockLabel<R, B, BlockParamsDyn>`), so a typed `BlockCall`
  built from a `Params`-stamped label is rarely usable at a redirect site — the
  pass would first have to recover (or carry) a typed label. Until a pass surface
  threads typed labels through, the typed `BlockCall` edge is a construction-time
  (`IrBuilder`) convenience only, and the edit surface keeps taking erased
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
`SwitchInst<'ctx, P, B, W>`; `IrBuilder::switch::<W>` pins `W` and
its `add_case` carries an `IntoIntValue<'ctx, W, B>` bound, so a wrong-width
case value is a **compile error** (the erased `switch_dyn` keeps the runtime
`TypeMismatch` check). And `indirectbr`'s address bound tightened from
`IsValue` to `IntoPointerValue`, so a typed non-pointer jump address is a
**compile error** (the pointer-ness check moves from `verify()` to build time;
erased `Value` addresses are unchanged). Parser / SSA-builder paths and the
whole erased authoring surface are untouched.

With the edit surface, typed block parameters, and now operand typing all
shipped, the **"branching bugs impossible at the type level"** program's typed
surfaces are largely complete. What remains is deliberately out of scope rather
than pending:

- **Universal per-function branding (`build_body`)** — designed in full, then
  **deferred out of the type-safety
  program** after re-analysis against the actual consumers. The evidence points
  one way: (1) regular authoring gains nothing — the default is `FnErased`, so
  existing `position_at_end` sites stay byte-identical and untouched, leaving the
  brand pure opt-in ceremony for them; (2) the primary consumer, the `bin_lift`
  lifter, stores SSA registers as **function arguments** (for concolic
  constant-visibility) and **rebuilds CFGs from runtime-recovered structure**, so
  a compile-time, un-nameable per-function `'fid` fights its model rather than
  helping it — its edges are recovered, not statically authored.

  **The locked design, recorded here because its spec does not ship.** The
  original write-up lives under `docs/superpowers/`, which is gitignored, so it
  is unreachable for anyone reading the repository; these are the decisions
  worth keeping. `FunctionValue` gains a defaulted brand parameter
  `Fb: FnBrand = FnErased`, so every existing call site is unchanged. Opting in
  goes through `func.build_body(|fb| …)`, which mints a generative
  `FnScoped<'fid>` for the closure body; `br` (and the other
  branch builders) require the target label's `Fb` to match the builder's, so a
  label minted in one function's body is not the right *type* to branch to from
  another's. That is what makes a cross-function branch — LLVM's "Referring to a
  basic block in another function!" — a compile error instead of a verifier
  finding.

  Two things changed under it since: 0.0.4 deleted the closure-scoped
  module constructor, so `build_body` would reintroduce the one shape the
  redesign removed; and block targets are now storable `BlockId<R, B, Params>`
  ids rather than borrowed labels, so a per-function *lifetime* has nothing to
  attach to. A revival in the id-first shape would need a per-function marker
  on `BlockId` itself. The proposed name would also need rethinking: the 0.0.4
  API-idioms sweep dropped the `build_` prefix from every `IrBuilder` emitter,
  so `build_body` no longer fits the naming law it was written against.

  Until then the rule stays a `Module::verify()` check — see the "br
  target is not a basic block of the parent function" family in `verifier.rs`.
  Revisit only as its own opt-in cycle if a concrete authoring need appears.
- **Whole-graph verifier territory** — phi-incoming completeness against the
  final predecessor set for builder-constructed IR, and dominance, are permanent
  residents of `Module::verify()` (defense in depth). These are whole-graph facts
  that cannot be a local construction- or parse-time guarantee, so they stay the
  verifier's job by design, not a gap to close.

## Constant-folding parity — deferred / known divergences (2026-07-23)

The `feature-29/constfold-parity` cycle fixed the divergences a three-agent
audit found against vendored `llvmorg-22.1.4` (see CHANGELOG "Constant-folding
parity"). A whole-branch review confirmed no mis-folds and no over-folds; the
items below are the deferred / known-remaining points.

- ~~**Constants are not uniqued like upstream `Constant*`.**~~ **Closed
  2026-08-01.** `GlobalValueRef`, `GepOffset`, `SymbolDelta`, and
  `SymbolDeltaPlus` now intern on their structural fingerprint like every other
  constant kind, so identity comparison is structural everywhere and the
  `base_identity` workaround is retired. The under-folding arms this entry
  listed — `fold_phi`, select's `true_value == false_value`,
  `constant_splat_value`, and the pointer-base comparison — all read plain `==`
  now, matching upstream. Forward `blockaddress` placeholders stay un-uniqued
  by design (each is a distinct pending reference); see
  `tests/constant_uniquing.rs` for the law and its exceptions.

- **`ptrtoint`/`ptrtoaddr` mid-width on `pointer_size != index_size` (CHERI-like)
  layouts — a deliberate, reasoned divergence (needs a decision).** In the
  `isEliminableCastPair` case-11 *declined* sub-case (`MidSize < SrcSize &&
  MidSize < DstSize`), llvmkit's `fold_ptr_to_int_pair` always two-steps through
  the case-11 mid (`ptrtoint`→pointer size, `ptrtoaddr`→index/address size),
  whereas upstream falls to its switch path (`ConstantFoldCastOperand`'s
  `PtrToInt`/`PtrToAddr` case, `ConstantFolding.cpp`: `PtrToInt` takes
  `DL.getAddressType`, `PtrToAddr` takes `DL.getIntPtrType` — the inverse).
  Example on
  `p:128:128:128:64`: `ptrtoaddr(inttoptr(i128 x)):i128` → llvmkit `x mod 2^64`
  (the semantically-correct address extraction), upstream `x`. llvmkit's value is
  arguably the *more correct* side; matching upstream would introduce a wrong
  mask to copy an upstream quirk, only on layouts x86/bin_lift never use.
  Deliberately NOT matched; recorded for an explicit decision if CHERI parity is
  ever in scope.

- **`SymbolicallyEvaluateGEP` sub-cases not ported (each only declines, never
  mis-folds):** `CastGEPIndices` vector-index width normalization; nested-GEP
  `in_range` preservation; the null/`inttoptr`-nonzero-base → `inttoptr` fold
  (needs `mustNotIntroduceIntToPtr` / `APInt::insertBits`); "infer inbounds for
  GEPs of globals" (needs a dereferenceable-bytes query). Each produces weaker /
  fewer folds than upstream, never a wrong one.

- ~~**Proactive ApFloat bit-exactness audit (deferred to its own cycle).** No
  constant-folder fix in this cycle touched ApFloat arithmetic, and the folding
  audit found `ap_float` structurally faithful and test-backed. A full
  bit-for-bit verification across all seven float semantics (incl. PPC
  double-double, every rounding/denormal/NaN-payload path) against known IEEE /
  LLVM `APFloat` values is a large, standalone effort worth its own cycle.~~
  **Done (2026-08-01) — struck 2026-08-06.** The cycle this entry asked for
  ran: see "ApFloat / `ApInt` bit-exactness audit — closed (2026-08-01)" at the
  top of this file, which records the fourteen defects it found and the
  `APIntTest.cpp` families deliberately not ported. This entry sat open for
  five days after its own successor closed it.
## Tests — two CHECK oracles in one crate, and the ordered one cannot express CHECK-NEXT (found 2026-08-20, fix round 3)

`crates/llvmkit-asmparser/tests` carries two substitutes for FileCheck.
`check_directives` implements `CHECK` and `CHECK-NEXT` against
`FileCheckString::Check` / `FileCheckString::CheckNext` / `Pattern::match` /
`FileCheck::CanonicalizeFile`. `assert_check_lines` — byte-identical copies at
parser_calls.rs, parser_constants.rs, parser_modifiers.rs and
parser_remaining_opcodes.rs — has upstream's byte cursor but **no CHECK-NEXT
concept**, so an upstream `CHECK-NEXT` ported into one of those files silently
becomes an unordered "somewhere later" check. That is a false-pass risk,
strictly worse than the symptom the fix round repaired in `check_directives`.

**Partly done (2026-08-21, `call addrspace(N)` port).** `check_directives`,
`Check`, `canonicalize_horizontal_whitespace` and `count_newlines_between` now
live in `crates/llvmkit-asmparser/tests/support/mod.rs`, `mod`-included by
parser_eh_funclet.rs, parser_calls.rs and parser_types.rs. That is the shared
home this item asked for, and `canonicalize_horizontal_whitespace` now has one
definition rather than two. That port had first added a *fifth*
`assert_check_lines` copy, in parser_types.rs; its fix round converted that
file too, so the list above is back to what it was. What remains is the
routing: those copies are untouched, and parser_calls.rs still drives its older
fixtures through one.

The fixtures those files drive that carry `CHECK-NEXT` today:
`insertextractvalue/{extractvalue,insertvalue}_round_trips.ll`,
`vectorInstructions.3.2/shufflevector_round_trips.ll`,
`zero-input-phi/phi_int_round_trips.ll`, and
`ConstantExprFold/constant_expr_fold_full_vector_gep_and_bitcast_fixture.ll`.
The extractvalue case is the sharpest: upstream is `CHECK: @foo` plus five
`CHECK-NEXT:`, and a printer regression inserting one line between `@foo` and
`load` fails upstream and passes here.

The operand-bundle commit (2026-08-20) added another such fixture,
`operand-bundles/operand-bundles.ll`, whose `CHECK-NEXT` and `CHECK-LABEL`
directives are asserted through `parser_calls.rs::assert_check_lines` as
ordered `CHECK`es — stated in that test's doc comment rather than hidden. It
also copied `canonicalize_horizontal_whitespace` into `parser_calls.rs`, since
that fixture's `CHECK` text carries a doubled space; that copy is gone, and the
routine now lives only in `support/`.

The work that is left: route the `assert_check_lines` call sites through
`support::check_directives`, delete the `assert_check_lines` copies, and
re-widen the flattened needle lists to their fixtures' own CHECK blocks — using
`Check::Next` where upstream writes `CHECK-NEXT`. The operand-bundle fixture needs no re-widening,
since it already carries every directive; it needs only the `Check::Next` and
`CHECK-LABEL` conversion. Doing that also unblocks
pointing `parser_calls.rs::callbr_successor_structure_round_trips` at the whole
`fixtures/upstream/assembler-corpus/callbr.ll` and asserting all eight of
`@test_kill`'s directives, retiring the trimmed fixture and its
`llvmkit-specific subset` row.

`parser_summary.rs`'s `check_lines` is **not** part of this. It extracts a
fixture's CHECK lines and compares the whole list to the printed `^` lines with
`assert_eq!` — a deliberate full-equality check, justified in that file's module
doc. Folding it into a substring oracle would weaken it.

## Tests — the corpus `error=` oracle cannot see a wrapper or an anchor (found 2026-08-20, operand-bundle fix round 3)

`parser_corpus.rs` checks a reject row's pinned diagnostic with
`rendered.contains(pin)`. A substring test passes whenever llvmkit's message
merely *contains* upstream's, so any wrapper that adds text around it — the
`expected ` prefix `ParseError::Expected` renders, most of all — satisfies the
row while the printed diagnostic differs from `llvm-as`. Rows that set no
`loc=` leave the caret column unchecked as well, so an anchor that drifts to a
later token is invisible too.

The worked example was `zeroinitializer`'s wrapper, which survived for
exactly this reason. Two rows pin `error=invalid type for null constant` and
both are green, but only one of them was green *because of* the oracle:
`target-type-properties/zeroinit-error.ll` rendered
`expected invalid type for null constant`, while
`2004-11-28-InvalidTypeCrash.ll` takes a different arm of the same routine and
rendered the bare text exactly. The wrapper is gone and both arms are pinned by
`parser_constants.rs` on variant and column; the two rows are unchanged, so the
hole they leave is the harness's, not those rows'.

**How big it is.** No figure is given here on purpose. The population is every
`error=` row in `parser_corpus_manifest.txt`, which grows with the corpus, and a
number written into a backlog paragraph is re-derived by nothing. Derive it when
you do the work: run `target/release/examples/parse_file.exe` over each `error=`
row's fixture, take the first stderr line, strip the `<path>:<line>:<col>: `
prefix (which yields exactly the harness's `rendered`, since
`examples/parse_file.rs` prints `eprintln!("{path}:{line}:{col}: {err}")` and the
harness compares `format!("{error}")`), and bucket against the pin.

Two things that sweep will show, and they are the point of it. First, the sweep
is small and per-*site* rather than per-row: the rows that fail an equality
oracle cluster on a handful of code sites, which is why this was never worth its
own cycle. Second, a flagged row is not automatically a defect. Some are llvmkit
printing upstream's text verbatim beside a pin that is a truncated `FileCheck`
fragment: `2003-04-15-ConstantInitAssertion.ll` pins `struct initializer doesn't
match struct element type` while `LLParser::convertValIDToValue` prints
`element 0 of struct initializer doesn't match struct element type`, and
`2007-03-18-InvalidNumberedVar.ll` pins `'%0' defined with type 'i1'` while
`LLParser::checkValidVariableType` prints
`'%0' defined with type 'i1' but expected 'i32'`. Being *stricter* than the
fixture's own `RUN` line is a divergence exactly as being weaker is, so neither
of those is work — the pin is what would change if a strict tier ever covered
them. Genuine defects the sweep has surfaced so far are recorded where they
belong: `zeroinit-error` in [`divergences.md`](divergences.md),
`musttail-invalid-1` and `invalid-datalayout-override` under **G17** in
[`fixture-coverage.md`](fixture-coverage.md), with the fix stated there.

**The work, in three tiers.** Not one switch: the oracle has to come from each
fixture's own `FileCheck` line, not from a house preference.

- **Equality, where upstream anchors.** Where the upstream directive line
  carrying the pin ends in `{{$}}`, upstream itself demands the message end
  there, so equality *is* that fixture's contract. Find them by scanning each
  row's upstream original for a line containing the pin and ending in `{{$}}`.
  The `*-parse-error*` attribute family (`byref`, `byval`, `inalloca`, `sret`)
  is written that way and already renders exactly, so the tier switches on at no
  cost. **The `symbolic-addrspace/bad-*` family is *not* anchored**:
  `test/Assembler/symbolic-addrspace.ll` writes
  `; ERR-BAD-CHAR: [[#@LINE-1]]:26: error: invalid symbolic addrspace 'D'` with
  no end anchor, and the only `{{$}}` in that file belongs to
  `ALLOCA-IN-GLOBALS` lines of a `status=pass` row. Putting those in an equality
  tier would be the defect this item exists to prevent.
- **`loc=`, the real remedy for the rest.** Add `loc=` wherever the upstream
  `CHECK` carries a column — which is most reject rows, and many more than
  carry one today. The matcher is "a line of the upstream original that
  contains the pin and also matches `:[0-9]+: *error:` **or**
  `\]\]:[0-9]+:`", and it has to be run with `grep -a`:
  `test/Assembler/invalid-name.ll` and `invalid-name2.ll` each contain a
  literal NUL byte, so GNU `grep` prints `Binary file … matches` instead of the
  matching line and a piped second `grep` then sees no `:N: error:` — the rows
  vanish. Widening the regex does not fix that; `-a` does. Record the matcher
  beside any figure you derive: the spellings upstream uses include
  `[[@LINE+1]]:1:`, `[[#@LINE-1]]:26:` and the bare `; ERR0: :41:` of the
  `invalid-atomicrmw-scalable` rows.
- **`contains` everywhere else, deliberately.** The pin is upstream's
  `FileCheck` text and `FileCheck` matches substrings, so containment *is* that
  fixture's contract. Tightening it without an end anchor invents a stricter
  test than upstream runs.

**Landed 2026-08-21 (caret-anchor fix round): the location half, for the rows
upstream can adjudicate — and nothing else.**

Note first what did *not* need building. `loc=` already existed, in
`parse_manifest_entry` and in the harness's
`assert_eq!(line_and_column(&source, offset), (line, column), …)`, and rows were
already using it. What was missing was **application**: rows whose upstream
original pins a column while the manifest row pinned only text. So this is not a
new capability; it is the existing one reaching a place it had not reached.

What was applied, and the boundary: the rows exercising
`LLParser::convertValIDToValue` / `getGlobalVal` / `checkValidVariableType` —
the routine whose anchor was being fixed — were enumerated from the manifest by
their pinned message, and each one's vendored fixture was checked for a column.
Exactly the `*-nonzero-program-addrspace` family carries one
(`[[@LINE-1]]:25`, `[[@LINE-1]]:11`, `[[@LINE-1]]:22`); those rows now pin it,
and llvmkit reports those columns exactly. Every other row in that group —
`2007-03-18-InvalidNumberedVar.ll`, `invalid-uselistorder-type.ll`,
`getelementptr_vec_idx1.ll` / `_idx3.ll`, `2008-02-18-IntPointerCrash.ll`,
`2006-09-28-CrashOnInvalid.ll`, `range-attribute-invalid-type.ll`, the
`constant-splat-diagnostics` trio — has **no** upstream column at all. Pinning
those would bless llvmkit's own output as ground truth, which is the failure
this file exists to prevent, so they stay on text alone.

Cross-check run at the same time, and it is the reason to trust the three: every
row that *already* carried `loc=` and whose fixture also carries its own pin was
compared against it. No disagreement.

**A caution this round paid for.** The first sweep for "rows whose fixture pins
a column" used a regex matching only `[[@LINE-N]]` and `<stdin>:L:C`, and
reported a set of three for the whole corpus. That was wrong: upstream also
writes `[[@LINE+1]]`, `[[@LINE+2]]` and `[[#@LINE-1]]`, and the corrected
matcher — the one this item already records above, which exists precisely
because of these spellings — returns a much larger population. The narrow regex
made a large backlog look finished. Use the recorded matcher, with `-a`.

**Landed 2026-08-29 (divergence-closing wave 12): the `loc=` retrofit, over the
whole corpus.** Every `status=reject` row carrying an `error=` pin was swept:
its checked-in fixture was searched for a line holding the pin *and* a column,
in any of upstream's spellings (`[[@LINE+N]]:C:`, `[[#@LINE-N]]:C:`, a bare
`:L:C: error:`), and where the fixture pins one the row now pins it too. Two
things had to be fixed for the sweep to be trustworthy at all, and both are
findings the old oracle could not see:

- The corpus' `split-file` parts were not what `split-file` writes, so the line
  numbers a `[[#@LINE-1]]` resolves against were wrong for thirty of them. That
  is fixed and now guarded — see
  `parser_corpus.rs::split_file_parts_are_what_split_file_emits`.
- Nine rows disagreed with their fixture's column, in two clusters, and both
  were llvmkit defects rather than pin defects: `parseValID`'s constant-expr
  arms reported at the current token instead of `ID.Loc` (`invalid cast opcode
  for cast from …` landed at end of file), and `check_metadata_field_value`
  reported at the token *after* the value it rejected (`arg: -1` put `expected
  unsigned integer` on the `)`). Both are fixed; the rows pin the columns now.

The population is not written here — derive it with the matcher this item
records, over `parser_corpus_manifest.txt` at whatever commit you are asking
about.

**Still open:**

- **`contains` rather than equality**, everywhere. None of the three tiers above
  switched. The location half and the containment half are separate weaknesses;
  only the first has moved.
- **Reject rows whose fixture pins no column at all** get no location oracle.
  Pinning those would bless llvmkit's own output as ground truth, which is the
  failure this file exists to prevent, so they stay on text alone.
- **Diagnostics with no vendored fixture get no location oracle from this
  harness at all.** `getGlobalVal`'s `"@" + Name` / `"@" + Twine(ID)` spellings
  are exactly that case — the vendored tree pins the `%`-spelling only — so
  `crates/llvmkit-asmparser/tests/parser_val_id.rs::assert_diagnostic` asserts
  message *and* caret directly instead. `line_and_column` moved from
  `parser_corpus.rs` into `crates/llvmkit-asmparser/tests/support/mod.rs` so the
  two harnesses share one copy rather than growing a second.

**Why the location half was worth doing alone.** A wrong caret shipped behind a
green corpus: the message was upstream's verbatim while the caret pointed at an
unrelated *line*. Containment saw the first half and nothing at all saw the
second.

A blanket equality switch is what this item used to propose, on the reasoning
that it would "surface every wrapper in one run" and so deserved its own cycle.
That reasoning was wrong twice over: the genuine sweep is a handful of rows
across roughly three code sites, not a large one, and a blanket switch breaks
correct rows. The prefix shortcut was wrong too — it flags only the mid-message
rows and would have missed the `nofpclass` rows, the largest genuine group at
the time, because those added a *suffix*.

## Docs — the cite-by-symbol sweep (found 2026-08-20, fix round 3)

`docs/divergences.md` states the law for its own file ("Upstream is cited **by
symbol, never by line number**") and its body breaks it about **157** times —
**147** matching
`grep -oE '[A-Za-z_]+\.(cpp|h|def):[0-9]+' docs/divergences.md | wc -l`
(re-derived at the operand-bundle-parity commit, down one from 148 at
`71806d3` because closing entry 14 deleted its evidence block), plus roughly
ten spelled as bare coordinates a grep cannot see (`(~:1128)`, `(~4202)`,
`defined at :5010`, in entries 40 and 47). All of them sit inside
`Correction from verification` and `<details>` evidence blocks; **none**
appears in an entry's **LLVM:** / **llvmkit:** / **Why:** / **Fix:** bullets,
and all resolve correctly against the pinned 22.1.4 tree today. Some name the
symbol adjacent to the number and so survive a version bump in recoverable
form; most are bare.

`UPSTREAM.md` carries the same debt in a different shape: **167 rows** carry
a `line N` / `lines N-M` coordinate, from

    grep -cE '^\|.*lines? [0-9]+' UPSTREAM.md

re-derived at the operand-bundle-parity fix-round-2 commit. Read that as "167
rows spelled that way", not as a census of the debt: it is neither a subset
nor a superset of "rows citing an upstream `.ll` or `.cpp` by line". The same
spelling also appears over headers, `.def` tables and in-repo docs, and **5**
further rows write the coordinate as `file:N` and so fall outside that grep
entirely, from

    grep -cE '^\|.*\.(ll|cpp|h|td|def|py|md):[0-9]+' UPSTREAM.md


Fix round 3 converted 19 of them — nine `UPSTREAM.md` rows, nine rustdoc twins
and one inline comment, all naming `test/Bitcode/compatibility.ll` blocks that
the funclet commit had just vendored, which is what made the rewrite mechanical
and risk-free. **That opens the class, it does not close it.** The header of
`docs/divergences.md` now discloses the debt rather than implying the file is
clean.

## Docs — `mirror` rows that hand-write their IR (found 2026-08-20, fix round 3)

`UPSTREAM.md`'s audit rule: a `mirror` row over an upstream `.ll` test must load
a checked-in copy or exact excerpt through `include_bytes!` / `include_str!`,
and must not rewrite the IR by hand unless the row says `llvmkit-specific
subset`. Fix round 3 converted ten tests to that shape and vendored five new
fixtures for them, but the general sweep — every `mirror` row whose test still
inlines an `r#"…"#` literal — is unmeasured.

Note what the benefit is and is not: **auditability by one `diff`**, not drift
detection. `orig_cpp/` is gitignored and no test in the workspace reads it, so a
`.ll` copied into `tests/fixtures/upstream/` is exactly as frozen at 22.1.4 as a
Rust string literal. The five drift guards that do exist parse tracked vendored
copies under `crates/llvmkit-asmparser/tablegen/`.

Two unported RUN lines surfaced in the same sweep and were left open, both
`verify-uselistorder`: on `test/Assembler/2002-08-15-ConstantExprProblem.ll`
and on `test/Assembler/numbered-values.ll`. Nothing in llvmkit re-materialises a
use list from a shuffled `uselistorder` directive and compares, so every fixture
carrying that RUN line is a half-port wherever a row cites it.

**The class is far wider than those two, and almost none of it is disclosed.**
Measured at this commit: **219** `UPSTREAM.md` rows cite one of **47** distinct
upstream `.ll` fixtures whose RUN lines include `verify-uselistorder` without
naming that line — `test/Bitcode/compatibility.ll` alone accounts for 102 of
them and `test/Assembler/flags.ll` for 34 — against exactly **3** rows that do
disclose it (`parser_function_body.rs::an_unreachable_block_prints_no_predecessors`
over `2002-08-15-ConstantExprProblem.ll`, and
`parser_debug_metadata.rs::diexpression_forms_round_trip` /
`::metadata_string_hex_escapes_print_uppercase` over `diexpression.ll` and
`debug-info.ll`). At `ea57b14`, where this measurement was first taken, it read
220 / 47 / 2; the one-row move is fix round 4's own disclosure on the
`debug-info.ll` row. So fix round 3's sweep disclosed the line on rows it
rewrote for other reasons, not on the class; the rows it regraded for hex-case,
block-label and `compatibility.ll` reasons are themselves inside the 219.
`numbered-values.ll` is no longer cited by any `UPSTREAM.md` row at all (the
fixture itself is still driven at `status=pass` by the corpus manifest), so the
pair named above is a sweep finding, not a disclosure pair. Derivation:

```bash
R=orig_cpp/llvm-project-llvmorg-22.1.4/llvm
grep -o 'test/[A-Za-z0-9_./+-]*\.ll' UPSTREAM.md | sort -u |
  while read -r f; do [ -f "$R/$f" ] &&
    grep -qE '^; *RUN:.*verify-uselistorder' "$R/$f" && echo "$f"; done > ul.txt
grep '^| `' UPSTREAM.md | grep -v verify-uselistorder | grep -cF -f ul.txt   # 219
grep '^| `' UPSTREAM.md | grep -v verify-uselistorder |
  grep -oF -f ul.txt | sort -u | wc -l                                      # 47
```
