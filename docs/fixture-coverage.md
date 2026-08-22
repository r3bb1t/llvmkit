# `llvm/test/Assembler` coverage

Every `.ll` fixture in `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/`,
classified against what llvmkit actually does with it — **no exceptions and no
sampling**: the row count below equals
`ls orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/*.ll | wc -l`,
against the vendored tag `llvmorg-22.1.4` (no repo commit pins `orig_cpp/`,
which is gitignored).

| Class | Meaning |
|---|---|
| `ported` | Every unit of the fixture runs — normally as a row in `crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt` driven by `parser_corpus.rs`, and where upstream's `RUN` line passes a flag the manifest has no spelling for, in a named test the row's Detail column points at. |
| `blocked-model` | At least one unit is held back by a named llvmkit gap. The gap is named per row and catalogued below. |
| `N/A` | The fixture's contract needs a tool or flag llvmkit does not model, and there is nothing left about the parse for llvmkit to assert. |

**The per-class tallies are not written here.** They moved on every commit that
closed a gap, and each rewrite seeded a stale copy somewhere else. Derive them
from the tables below instead — the classification is the data, a tally is not:

```
grep -oE '\| (ported|blocked-model|N/A) \|' docs/fixture-coverage.md | sort | uniq -c
```

(The pattern needs a pipe, a space, a bare class name and a space before the
next pipe. Written inside the alternation above, each class name is flanked by a
parenthesis or a pipe with no space, so the command does not count its own
quotation — the trap that once returned 92 where the truth was 91. Do not
restate the pattern anywhere in this file in a spelling that *would* match.)

## How this was measured

Not read off, not guessed. Each fixture was fed to `llvmkit_asmparser::parser`
and the answer recorded:

1. **Units, not files.** 21 of the 500 fixtures are `split-file` containers: the
   `.ll` holds several independent modules separated by `;--- <name>` and
   upstream's `RUN` lines run `llvm-as` over each part on its own. Parsing such
   a container whole measures nothing. They were split the way
   `llvm/utils/split-file` splits them (honouring `--leading-lines`), giving
   **624 units** — 479 whole fixtures plus 145 parts.
2. **Positive or negative** comes from the `RUN` lines: a unit is negative when
   `not` guards the `llvm-as` or `opt` invocation for it, positive otherwise.
   263 of the 500 fixtures have at least one negative unit and 259 are negative
   throughout; at unit granularity 380 of the 624 are negatives. (Counting only
   `not llvm-as` and ignoring `not opt` gives 257 — the figure the stage brief
   used.)
3. **Every unit was parsed**, then verified, then printed, re-parsed and
   re-printed. Errors were recorded with their rendered message *and* the
   1-based line/column of the reported span.
4. **Negatives were matched against upstream's own `FileCheck` lines**, with
   the fixture's real prefixes (`--check-prefix=OUTER-LEFT` and friends, not
   just `CHECK`), `[[@LINE±N]]` resolved, `CHECK-NOT` excluded, and `{{...}}`
   treated as the regex it is. A match is `FileCheck`'s own rule: the rendered
   message must *contain* upstream's text.
5. **Diagnostics with no `FileCheck` line at all** (the pre-2010 `not llvm-as >
   /dev/null` fixtures) were checked a second way: the rendered message's
   literal fragments were searched for in the vendored
   `lib/{AsmParser,IR,Support,Bitcode/Reader}` sources, normalised so that
   line-wrapped and `+`-concatenated C++ string literals read as one run. A
   message llvmkit invented does not appear there.

## What "ported" asserts, and what it does not

A `ported` negative asserts the parse **fails**, and on almost every `reject`
row also that the message contains upstream's own `FileCheck` text. A smaller
subset additionally pins upstream's `<stdin>:LINE:COL:` through a `loc=` option.
Each of those pins was re-derived from the cited upstream file, not from
llvmkit's output.

That is a **containment lock, not a text-parity measure**: the harness matches
with `rendered.contains(pin)`
(`parser_corpus.rs::parser_corpus_round_trips_checked_in_fixtures`), so a message
that wraps upstream's satisfies the row, and a message *shorter* than upstream's
is invisible too, since the pin is itself `FileCheck` text and `FileCheck`
matches substrings. Only the `loc=` rows assert the caret; on the rest the
anchor is unchecked. Some rows pass while diverging in text — entry 114's
`zeroinit-error`, and `musttail-invalid-1` / `invalid-datalayout-override` under
**G17** below, plus rows where llvmkit is exactly right and the pin is a
truncated `FileCheck` fragment. See the corpus-oracle item in
`docs/future-work.md`, which carries the derivation.

**No manifest tallies are written here any more.** They were, in five figures,
and every one of them was stale within a commit or two of being written — the
manifest gains rows whenever a gap closes. If you need them, split
`parser_corpus_manifest.txt` on `|` field-exactly as `parse_manifest_entry`
does — skipping blank and `#` lines and reading only `status=` / `error=` /
`loc=` / `config=` options, each recognised by `strip_prefix`. A plain
`grep -c 'status=reject'` over the file answers higher, because the manifest's
own `#` documentation header explains the option and the grep counts that line
too; the parser skips `#` lines. That discrepancy, not any particular number, is
the thing worth knowing before counting this manifest.

A `ported` positive asserts the fixture **parses**, **verifies**, and is
**round-trip stable** (print → re-parse → print reproduces the first print byte
for byte). It does **not** assert that llvmkit's print matches the fixture's own
`CHECK` lines: those are `llvm-dis`' output, and comparing against them is the
separate byte-for-byte AsmWriter job the byte-lock tests do on
`examples/*.rs`. Where a `ported` row's fixture also has a focused excerpt test
elsewhere under `tests/fixtures/upstream/`, that test is unchanged and still
runs; the corpus row is additional coverage of the whole file, not a
replacement.

## Tools and flags in the corpus, and how llvmkit maps them

The reason `N/A` is one row and not fifty: `test/Assembler` is overwhelmingly
`llvm-as` and `llvm-dis`, both of which llvmkit models.

| Upstream spelling | Fixtures | llvmkit |
|---|---|---|
| `llvm-as` / `not llvm-as` | most | `parser::parse_assembly_file` succeeding or failing |
| `llvm-as \| llvm-dis` | many | parse then `format!("{module}")` |
| `opt` (43 fixtures; mostly `opt -S` with no passes) | 43 | same as `llvm-as \| llvm-dis` for these fixtures |
| `-disable-output`, `-o /dev/null` | many | no output is compared; irrelevant here |
| `split-file` | 21 | parts materialised as separate fixtures, see above |
| `-allow-incomplete-ir` | 3 | `ParserConfig::allow_incomplete_ir`, spelled `config=allow-incomplete-ir` in the manifest. Two of the three are ported under it; `incomplete-ir-metadata.ll` is gap **G14** |
| `verify-uselistorder` | many | not a separate assertion here; W12 covers ordered use-lists |
| `-non-global-value-max-name-size` | 2 | **no equivalent** — the one `N/A` row |
| `-mtriple=`, target intrinsics | 5 | gap **G2**, not a tooling limit |
| bitcode (`%t.bc`), `llvm-dis --print-addrspace-name` | 3 | out of scope (bitcode) / gap **G6** |

## Gap catalogue

Each `blocked-model` row names one of these. The per-gap fixture counts are the
lists under *Which fixture sits on which gap* — a separate tally column stood
here and drifted from those lists, so it is gone. A letter missing from this
table carries no meaning: one closed gap was retained as a `Closed.` row and
another was deleted outright, so the letters are not a stable namespace.

| Gap | What is missing |
|---|---|
| **G1** | `AutoUpgrade` is not ported: an intrinsic name or signature upstream silently rewrites is rejected instead. |
| **G2** | Target-specific intrinsic tables (`llvm.amdgcn.*`, `llvm.nvvm.*`, `llvm.wasm.*`, `llvm.aarch64.*`) are not modelled. |
| **G3** | An unknown `llvm.`-prefixed declaration is rejected; `LLParser::parseFunctionHeader` keeps it and leaves the complaint to the Verifier. |
| **G4** | An alias/ifunc aliasee may be a constant expression (`getelementptr`, `addrspacecast`); llvmkit's `parse_alias_or_ifunc` sends everything through the TYPE VALUE branch where `LLParser::parseAliasOrIFunc` branches on the aliasee's *first token* and routes those keywords through a bare `parseValID`. This is the "self-typed aliasee does not parse" entry already in [`future-work.md`](future-work.md), and these five fixtures are what it costs. |
| **G6** | Symbolic address-space **printing** (`llvm-dis --print-addrspace-name=true`) is not modelled: `printAddressSpace`'s `PrintAddrspaceName` branch is `static cl::opt<bool>`-gated and llvmkit has no printer-option layer, so it has no reachable trigger. Parsing `addrspace("A"/"G"/"P")` and `addrspace("<datalayout name>")` is ported, and the data the branch would print is modelled (`DataLayout::address_space_name`). |
| **G7** | *Closed.* `getelementptr` with a vector-of-pointers base or vector indices is now modelled (`IrBuilder::gep_erased`, `GetElementPtrInst::getGEPReturnType`). Kept as a row so a reader meeting the letter in an older commit message finds it; see the note above about letters that are simply absent. |
| **G8** | Metadata fields that take a value or a brace list (`!DITemplateValueParameter(value: i32 7)`, `!GenericDINode(operands: {...})`) are not parsed. |
| **G9** | Metadata strings and metadata names are required to be UTF-8; LLVM allows arbitrary bytes. |
| **G10** | `!DIEnumerator` values wider than i128 are rejected; upstream stores an `APInt` of any width. |
| **G11** | A global variable's trailing `"key" = "value"` attribute list is not parsed. |
| **G12** | `fpext` (and its siblings) reject a scalable-vector source. |
| **G13** | A forward-referenced function whose later definition/ifunc has the same name is rejected instead of resolved. |
| **G14** | `-allow-incomplete-ir`'s `dropUnknownMetadataReferences` half is not implemented (recorded in docs/divergences.md). |
| **G15** | A forward reference to an explicitly numbered global (`@6`) is not resolved. |
| **G16** | The `typeidCompatibleVTable:` module-summary entry kind is not parsed. |
| **G17** | Diagnostic text differs from upstream's: llvmkit routes a complete upstream message through an `expected ...` wrapper, or words the check differently. |
| **G18** | The check runs at a different stage than upstream's, or not at all: upstream's `llvm-as` rejects at parse/verify time and llvmkit accepts. |
| **G19** | Print / re-parse is not idempotent: printing the module and re-parsing the print yields different text. |
| **G20** | A **function** used as a `ptr` constant keeps its function type instead of its pointer type. `Function::getType()` upstream *is* a `PointerType` and only `getValueType()` is the `FunctionType`, so this both **rejects valid input** — `declare void @f()` with `call void @use(ptr @f)` answers `call argument #0 type mismatch: expected ptr, got void ()` — and breaks the print: `call void @f() [ "foo"(ptr @f) ]` prints `[ "foo"(void () @f) ]`, which re-parses to `functions are not values, refer to them as pointers`. **Globals are not affected** — `@gv = global i32 0` used as `call void @use(ptr @gv)` parses, prints and re-parses; the row said "function or global" and the global half was never true. All three probed at 2026-08-22 with `target/release/examples/parse_file.exe`. |
| **G21** | `!DIFile(source: ...)` and neighbouring fields in a summary-bearing module are not accepted. |
| **G22** | llvmkit's Verifier does not exempt **unreachable** blocks: `Verifier::verifyDominatesUse` returns early when `!DT.isReachableFromEntry(...)`, so upstream accepts a self-referencing or out-of-order instruction in a block with no path from entry, and llvmkit rejects it. |
| **G23** | *Closed.* A `@name` / `@N` call-family callee was looked up only among functions; `resolve_direct_callee` now runs `getGlobalVal`'s own lookup — the symbol table for a name, `NumberedVals` for a number — and keeps a non-function `GlobalValue` as the bare pointer upstream returns. Kept as a row so a reader meeting the letter in an older commit message finds it. |

Which fixture sits on which gap:

- **G1**: `auto_upgrade_intrinsics.ll`, `autoupgrade-invalid-masked-align.ll`, `autoupgrade-invalid-mem-intrinsics.ll`, `autoupgrade-invalid-name-mangling.ll`, `autoupgrade-lifetime-intrinsics.ll`, `implicit-intrinsic-declaration-invalid.ll`, `implicit-intrinsic-declaration-invalid3.ll`, `implicit-intrinsic-declaration.ll`, `invalid-vecreduce.ll`, `metadata.ll`, `opaque-ptr-intrinsic-remangling.ll`, `remangle.ll`, `struct-ret-without-upgrade.ll`
- **G2**: `amdgcn-unreachable.ll`, `amdgpu-image-atomic-attributes.ll`, `auto_upgrade_nvvm_intrinsics.ll`, `autoupgrade-thread-pointer.ll`, `autoupgrade-wasm-intrinsics.ll`
- **G3**: `immarg-param-attribute.ll`, `invalid-immarg.ll`, `invalid-immarg4.ll`, `invalid-immarg5.ll`, `metadata-function-local.ll`, `token.ll`
- **G4**: `ConstantExprNoFold.ll`, `addrspacecast-alias.ll`, `alias-use-list-order.ll`, `getelementptr.ll`, `uselistorder.ll`
- **G6**: `symbolic-addrspace-datalayout.ll`
- **G7**: closed
- **G8**: `DIDefaultTemplateParam.ll`, `ditemplateparameter.ll`, `generic-debug-node.ll`
- **G9**: `difile-escaped-chars.ll`, `named-metadata.ll`
- **G10**: `DIEnumeratorBig.ll`
- **G11**: `globalvariable-attributes.ll`
- **G12**: `fast-math-flags.ll`
- **G13**: `2003-05-15-AssemblerProblem.ll`
- **G14**: `incomplete-ir-metadata.ll`
- **G15**: `opaque-ptr.ll`, `skip-value-numbers-globals.ll`
- **G16**: `index-value-order.ll`, `thinlto-vtable-summary.ll`
- **G17**: `2007-01-16-CrashOnBadCast.ll`, `alias-redefinition.ll`, `dicompileunit-invalid-language.ll`, `invalid-disubrange-count-negative.ll`, `invalid-fp80hex.ll`, `invalid-label-call-arg.ll`, `invalid-metadata-function-local-attachments.ll`, `invalid-metadata-function-local-complex-1.ll`, `invalid-metadata-function-local-complex-2.ll`, `invalid-metadata-function-local-complex-3.ll`, `invalid_cast.ll`, `invalid_cast2.ll`, `nofpclass-invalid.ll`, `opaque-ptr-invalid-forward-ref.ll`, `ptrtoaddr-invalid.ll`
- **G18**: `attribute-builtin.ll`, `call-invalid-1.ll`, `captures-errors.ll`, `invalid-byval-type3.ll`, `invalid-dicompileunit-emissionkind-bad.ll`, `invalid-dicompileunit-language-overflow.ll`, `invalid-diexpression-verify.ll`, `invalid-disubrange-count-large.ll`, `invalid-disubrange-count-node.ll`, `invalid-disubrange-lowerBound-max.ll`, `invalid-disubrange-lowerBound-min.ll`, `invalid-generic-debug-node-tag-overflow.ll`, `invalid-generic-debug-node-tag-wrong-type.ll`, `invalid_cast3.ll`, `ptrtoaddr-invalid-constexpr.ll`, `summary-parsing-error.ll`, `target-type-properties.ll`
- **G19**: `2010-02-05-FunctionLocalMetadataBecomesNull.ll`, `DICommonBlock.ll`, `DIEnumerator.ll`, `dbg_declare_value.ll`, `debug-label-bitcode.ll`, `disubprogram-targetfuncname.ll`, `drop-debug-info-nonzero-alloca.ll`, `drop-debug-info.ll`, `export-symbol-anonymous-class.ll`, `metadata-use-uselistorder.ll`, `thinlto-vtable-summary2.ll`
- **G20**: `MultipleReturnValueType.ll`, `anon-functions.ll`
- **G21**: `thinlto-summary.ll`
- **G22**: `2004-02-27-SelfUseAssertError.ll`, `2004-06-07-VerifierBug.ll`
- **G23**: closed

## Findings this classification produced

These came out of the measurement and are recorded here rather than buried:

1. **Citations in the tree name `test/Assembler/*.ll` files that do not
   exist.** `call.ll`, `phi.ll`, `switch.ll`, `datalayout.ll`, `bitcast.ll`,
   `freeze.ll` and others in the same shape, in `UPSTREAM.md` rows and in
   rustdoc on `ll_parser.rs` tests. Some of those filenames exist nowhere under
   `llvm/test/` at all. The tests themselves are real; their *provenance* is
   not. Repointing them is not part of this stage.

   *Update, 2026-08-20 (funclet parity).* The phantoms carried by the routines
   and tests the Windows-EH funclet commit rewrote — `catchpad.ll`,
   `catchswitch.ll`, `catchret.ll`, `cleanuppad.ll`, `cleanupret.ll`,
   `resume.ll`, `landingpad.ll` and `invoke.ll` — were repointed there, because
   leaving a false citation on a line being edited was not defensible. The rest
   stay deferred as this finding says. A later round repaired one of a different
   kind: `builder_icmp_named.rs` cited a path carrying a literal `...` elision
   for a fixture that **does** exist, at
   `test/Assembler/auto_upgrade_nvvm_intrinsics.ll` — a mis-pathed citation of a
   real file, not a citation of a nonexistent one. Genuine `test/CodeGen/*`
   phantoms are also on record, in `UPSTREAM.md`'s rows for
   `call_with_metadata_argument_roundtrip` and
   `call_with_metadata_and_value_argument_roundtrip` and in the rustdoc on the
   former; the nearest real register fixtures live under `test/CodeGen/AMDGPU/`.

   **No count is given for this finding, deliberately.** It carried one, then a
   correction to it, then a correction to that correction, and each figure was
   dead on arrival. The reason is structural and does not go away: this
   paragraph, and the rustdoc it describes, *quote* phantom paths in order to
   name them, and no regex sweep can tell a citation from a disclaimer, so any
   command counts its own prose. Anyone who needs a number must fix the scope
   first — which trees, `*.rs` only or `*.md` too, citations of
   `test/Assembler/` only or of every `test/` path — exclude the prose that
   discusses a phantom rather than relying on one, and publish that scope beside
   the figure. Three attempts without it produced three numbers that meant
   nothing.

   *(What a sweep like that no longer has to find: `UPSTREAM.md`'s rows are now
   checked mechanically for the half that is checkable —
   `crates/llvmkit-ir/tests/upstream_registry_drift.rs` fails if a row's own
   `path.rs::test` target does not resolve in this tree. It says nothing about
   the upstream citation in the second column, which is where the phantoms
   live.)*
2. **`AssemblyWriter` printed named metadata after numbered metadata**, where
   `AssemblyWriter::printModule` runs the `M->named_metadata()` loop *before*
   `writeAllMDNodes()`. Fixed here. It was also the cause of 12 of the 24
   round-trip-unstable fixtures.
3. **A function's `addrspace(...)` was dropped when it was zero**, even under a
   non-zero program address space, where `printFunction` forces it via
   `ForcePrintAddressSpace`. Fixed here. Without it a module carrying `target
   datalayout = "P2"` printed IR that re-parsed into a different address space —
   caught by the new round-trip law on an *existing* corpus fixture,
   `upstream/blockaddress-addrspace/return_self_good.ll`.
4. **`LLParser::parseStandaloneMetadata` checks the id for reuse *after*
   parsing the node body; llvmkit checks it first.**
   `invalid-disubrange-count-negative.ll` therefore reports `Metadata id is
   already used` where upstream reports `value for 'count' too small, limit is
   -1`. Gap **G17**.
5. **Diagnostic location agrees with upstream in 52 of the 54 places both pin
   one.** 59 whole fixtures carry a `<stdin>:LINE:COL:` `CHECK`; five of them
   llvmkit does not reject at all (**G18**), leaving 54 comparable. The two
   disagreements are `invalid-dilocalvariable-arg-negative.ll` (llvmkit points
   at 7:42, past the consumed `-1`; upstream at 7:40, its start) and
   `invalid-disubrange-count-negative.ll` (finding 4 above). All 52 agreements
   are locked by `loc=` rows. Eight `split-file` parts also pin a location and
   none is locked: `[[@LINE+1]]` there resolves against the *container's*
   numbering, which is not the part's, so the pin would mean something else.
6. **llvmkit's Verifier does not exempt unreachable blocks.**
   `Verifier::verifyDominatesUse` returns early on
   `!DT.isReachableFromEntry(...)`, which is the whole point of
   `2004-02-27-SelfUseAssertError.ll` ("*`%inc2` uses it's own value, but that's
   ok, as it's unreachable!*") and `2004-06-07-VerifierBug.ll`. llvmkit rejects
   both. Gap **G22**.
7. **A recurring wording bug: a complete upstream message routed through an
   `expected …` wrapper.** `expected invalid type for null constant`, `expected
   unexpected ellipsis in argument list for musttail call in non-varargs
   function`, `expected valid datalayout: address space must be a 24-bit
   integer`, `expected valid mask value for 'nofpclass'`, `expected valid
   ptrtoaddr: …`, `expected valid trunc: …`, `expected valid alias definition:
   …`, `expected intrinsic signature mismatch`, `expected unknown intrinsic`.
   Upstream reaches all of these through `error(...)`, not `tokError("expected
   …")`. This is the single largest share of **G17** and the cheapest parity
   work left in the directory.

## A directory-name trap

The new fixtures live in `tests/fixtures/upstream/assembler-corpus/`, not
`upstream/assembler/`. `tests/fixtures/upstream/Assembler/` already exists (it
holds `2002-05-02-InvalidForwardRef.ll`, loaded by
`parser_forward_refs.rs::upstream_forward_global_reference_fixture_parses`), and
on a case-insensitive filesystem `assembler` and `Assembler` are the same
directory — creating one silently renames the other, and the rename then breaks
that `include_bytes!` on a case-sensitive CI runner. Do not reintroduce the
collision.

## Per-fixture classification

| Fixture | Class | Detail |
|---|---|---|
| `2002-03-08-NameCollision.ll` | ported | 1 pass |
| `2002-03-08-NameCollision2.ll` | ported | 1 pass |
| `2002-04-07-HexFloatConstants.ll` | ported | 1 pass |
| `2002-04-07-InfConstant.ll` | ported | 1 pass |
| `2002-04-29-NameBinding.ll` | ported | 1 pass |
| `2002-05-02-InvalidForwardRef.ll` | ported | 1 pass |
| `2002-07-14-OpaqueType.ll` | ported | 1 pass |
| `2002-07-25-QuoteInString.ll` | ported | 1 pass |
| `2002-07-25-ReturnPtrFunction.ll` | ported | 1 pass |
| `2002-07-31-SlashInString.ll` | ported | 1 pass |
| `2002-08-15-CastAmbiguity.ll` | ported | 1 pass |
| `2002-08-15-ConstantExprProblem.ll` | ported | 1 pass |
| `2002-08-15-UnresolvedGlobalReference.ll` | ported | 1 pass |
| `2002-08-16-ConstExprInlined.ll` | ported | 1 pass |
| `2002-08-19-BytecodeReader.ll` | ported | 1 pass |
| `2002-08-22-DominanceProblem.ll` | ported | 1 pass |
| `2002-10-08-LargeArrayPerformance.ll` | ported | 1 pass |
| `2002-10-13-ConstantEncodingProblem.ll` | ported | 1 pass |
| `2002-12-15-GlobalResolve.ll` | ported | 1 pass |
| `2003-01-30-UnsignedString.ll` | ported | 1 pass |
| `2003-04-15-ConstantInitAssertion.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2003-04-25-UnresolvedGlobalReference.ll` | ported | 1 pass |
| `2003-05-03-BytecodeReaderProblem.ll` | ported | 1 pass |
| `2003-05-12-MinIntProblem.ll` | ported | 1 pass |
| `2003-05-15-AssemblerProblem.ll` | blocked-model | **G13** — rejected at 11:13: `expected forward function definition with matching signature` |
| `2003-05-15-SwitchBug.ll` | ported | 1 pass |
| `2003-05-21-ConstantShiftExpr.ll` | ported | 1 pass |
| `2003-05-21-EmptyStructTest.ll` | ported | 1 pass |
| `2003-05-21-MalformedStructCrash.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2003-08-20-ConstantExprGEP-Fold.ll` | ported | 1 pass |
| `2003-08-21-ConstantExprCast-Fold.ll` | ported | 1 pass |
| `2003-11-11-ImplicitRename.ll` | ported | 1 reject (0 with upstream's diagnostic pinned) |
| `2003-11-12-ConstantExprCast.ll` | ported | 1 pass |
| `2003-11-24-SymbolTableCrash.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2004-01-11-getelementptrfolding.ll` | ported | 1 pass |
| `2004-01-20-MaxLongLong.ll` | ported | 1 pass |
| `2004-02-01-NegativeZero.ll` | ported | 1 pass |
| `2004-02-27-SelfUseAssertError.ll` | blocked-model | **G22** — upstream accepts it (`%inc.2` is in an unreachable block); llvmkit's verifier rejects: `only PHI nodes may reference their own value` |
| `2004-03-30-UnclosedFunctionCrash.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2004-04-04-GetElementPtrIndexTypes.ll` | ported | 1 pass |
| `2004-06-07-VerifierBug.ll` | blocked-model | **G22** — upstream accepts it (the `loop` block is unreachable); llvmkit's verifier rejects: `instruction does not dominate all uses` |
| `2004-10-22-BCWriterUndefBug.ll` | ported | 1 pass |
| `2004-11-28-InvalidTypeCrash.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2005-01-03-FPConstantDisassembly.ll` | ported | 1 pass |
| `2005-01-31-CallingAggregateFunction.ll` | ported | 1 pass |
| `2005-05-05-OpaqueUndefValues.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2005-12-21-ZeroInitVector.ll` | ported | 1 pass |
| `2006-09-28-CrashOnInvalid.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2006-12-09-Cast-To-Bool.ll` | ported | 1 pass |
| `2007-01-02-Undefined-Arg-Type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2007-01-05-Cmp-ConstExpr.ll` | ported | 1 pass |
| `2007-01-16-CrashOnBadCast.ll` | blocked-model | **G17** — reported `expected integer destination type for trunc/zext/sext`, upstream `invalid cast opcode for cast from 'i64' to 'ptr'` |
| `2007-01-16-CrashOnBadCast2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2007-03-18-InvalidNumberedVar.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2007-03-19-NegValue.ll` | ported | 1 pass |
| `2007-04-20-AlignedLoad.ll` | ported | 1 pass |
| `2007-04-20-AlignedStore.ll` | ported | 1 pass |
| `2007-05-21-Escape.ll` | ported | 1 pass |
| `2007-07-19-ParamAttrAmbiguity.ll` | ported | 1 pass |
| `2007-08-06-AliasInvalid.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2007-09-10-AliasFwdRef.ll` | ported | 1 pass |
| `2007-09-29-GC.ll` | ported | 1 pass |
| `2007-11-26-AttributeOverload.ll` | ported | 1 reject (0 with upstream's diagnostic pinned) |
| `2007-12-11-AddressSpaces.ll` | ported | 1 pass |
| `2008-01-11-VarargAttrs.ll` | ported | 1 pass |
| `2008-02-18-IntPointerCrash.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `2008-07-10-APInt.ll` | ported | 1 pass |
| `2008-09-02-FunctionNotes.ll` | ported | 1 pass |
| `2008-09-29-RetAttr.ll` | ported | 1 pass |
| `2008-10-14-QuoteInName.ll` | ported | 1 pass |
| `2009-02-01-UnnamedForwardRef.ll` | ported | 1 pass |
| `2009-02-28-CastOpc.ll` | ported | 1 pass |
| `2009-02-28-StripOpaqueName.ll` | ported | 1 pass |
| `2009-07-24-ZeroArgGEP.ll` | ported | 1 pass |
| `2010-02-05-FunctionLocalMetadataBecomesNull.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `ConstantExprFold.ll` | ported | 1 pass |
| `ConstantExprFoldCast.ll` | ported | 1 pass |
| `ConstantExprFoldSelect.ll` | ported | 1 pass |
| `ConstantExprNoFold.ll` | blocked-model | **G4** — rejected at 24:23: `expected type` |
| `DICommonBlock.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `DIDefaultTemplateParam.ll` | blocked-model | **G8** — rejected at 60:62: `expected metadata field value` |
| `DIEnumerator.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `DIEnumeratorBig.ll` | blocked-model | **G10** — rejected at 13:79: `expected metadata integer literal in i128 range` |
| `DIGlobalVariableExpression.ll` | ported | 1 pass |
| `DIMacroFile.ll` | ported | 1 pass |
| `MultipleReturnValueType.ll` | blocked-model | **G20** — printed module does not re-parse: functions are not values, refer to them as pointers |
| `aarch64-intrinsics-attributes.ll` | ported | 1 pass |
| `absolute_symbol.ll` | ported | 1 pass |
| `addrspacecast-alias.ll` | blocked-model | **G4** — rejected at 7:40: `expected type` |
| `aggregate-constant-values.ll` | ported | 1 pass |
| `aggregate-return-single-value.ll` | ported | 1 pass |
| `alias-redefinition.ll` | blocked-model | **G17** — reported `expected valid alias definition: a global named "bar" already exists in this module`, upstream `redefinition of global '@bar'` |
| `alias-use-list-order.ll` | blocked-model | **G4** — rejected at 10:26: `expected type` |
| `align-inst-alloca.ll` | ported | 1 reject (0 with upstream's diagnostic pinned) |
| `align-inst-load.ll` | ported | 1 reject (0 with upstream's diagnostic pinned) |
| `align-inst-store.ll` | ported | 1 reject (0 with upstream's diagnostic pinned) |
| `align-inst.ll` | ported | 1 pass |
| `align-param-attr-error0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `align-param-attr-error1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `align-param-attr-error2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `align-param-attr-format.ll` | ported | 1 pass |
| `alignstack.ll` | ported | 1 pass |
| `alloca-addrspace-elems.ll` | ported | 1 pass |
| `alloca-addrspace-parse-error-0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `alloca-addrspace-parse-error-1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `alloca-addrspace0.ll` | ported | 1 pass |
| `alloca-invalid-type-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `alloca-invalid-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `alloca-size-one.ll` | ported | 1 pass |
| `allockind-missing.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `allockind.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `amdgcn-intrinsic-attributes.ll` | ported | 1 pass |
| `amdgcn-unreachable.ll` | blocked-model | **G2** — rejected at 17:3: `expected valid call: call argument #0 type mismatch: expected ptr, got void ()` |
| `amdgpu-cs-chain-cc.ll` | ported | 1 pass |
| `amdgpu-image-atomic-attributes.ll` | blocked-model | **G2** — rejected at 5:17: `expected intrinsic signature mismatch` |
| `anon-functions.ll` | blocked-model | **G20** — printed module does not re-parse: functions are not values, refer to them as pointers |
| `asm-path-writer.ll` | ported | 1 pass |
| `associated-metadata.ll` | ported | 1 pass |
| `atomic.ll` | ported | 1 pass |
| `atomicrmw.ll` | ported | 1 pass |
| `attribute-builtin.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `declare ptr @foo(ptr) [[NOBUILTIN:#[0-9]+]]` |
| `auto_upgrade_intrinsics.ll` | blocked-model | **G1** — rejected at 7:12: `expected intrinsic signature mismatch` |
| `auto_upgrade_nvvm_intrinsics.ll` | blocked-model | **G2** — rejected at 5:13: `expected unknown intrinsic` |
| `autoupgrade-invalid-masked-align.ll` | blocked-model | **G1** — 6/6 parts blocked. reported `expected intrinsic signature mismatch`, upstream `LLVM ERROR: Invalid alignment argument` |
| `autoupgrade-invalid-mem-intrinsics.ll` | blocked-model | **G1** — rejected at 7:14: `expected intrinsic signature mismatch` |
| `autoupgrade-invalid-name-mangling.ll` | blocked-model | **G1** — reported `expected intrinsic signature mismatch`, upstream `Intrinsic called with incompatible signature` |
| `autoupgrade-lifetime-intrinsics.ll` | blocked-model | **G1** — rejected at 14:13: `expected intrinsic signature mismatch` |
| `autoupgrade-thread-pointer.ll` | blocked-model | **G2** — rejected at 4:13: `expected unknown intrinsic` |
| `autoupgrade-wasm-intrinsics.ll` | blocked-model | **G2** — rejected at 9:25: `expected unknown intrinsic` |
| `bcwrap.ll` | ported | 1 pass |
| `bfloat.ll` | ported | 1 pass |
| `block-labels.ll` | ported | 1 pass — round-trip only in the manifest; both CHECK blocks (`@test1`'s 17 lines and `@test2`'s 2) are asserted line by line by targeted tests in `parser_function_body.rs`. The `"$N"` quoting that used to be the one gap is closed — `printLLVMNameWithoutPrefix` does not carry `$` in its unquoted set, and neither does llvmkit now. |
| `br-single-destination.ll` | ported | 1 pass |
| `byref-parse-error-0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-10.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-4.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-5.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-6.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-7.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-8.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byref-parse-error-9.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byval-parse-error0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `byval-type-attr.ll` | ported | 1 pass |
| `c-style-comment.ll` | ported | 1 pass |
| `call-arg-is-callee.ll` | ported | 1 pass |
| `call-invalid-1.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `Attribute 'align 8' does not apply to functions!` |
| `call-nonzero-program-addrspace-2.ll` | ported | 1 reject; the `-data-layout=P42` unit runs in `parser_calls.rs::numbered_callee_addrspace_matches_upstream_in_both_program_addrspaces` |
| `call-nonzero-program-addrspace.ll` | ported | 1 reject; the `-data-layout=P42` unit runs in `parser_calls.rs::call_addrspace_round_trips_under_a_nonzero_program_addrspace` |
| `callbr.ll` | ported | 1 pass |
| `callee-type-metadata.ll` | ported | 1 pass; the `; CHECK` line is asserted in `parser_calls.rs::indirect_call_parameter_attribute_round_trips` |
| `captures-errors.ll` | blocked-model | **G18** — 1/9 parts blocked. llvmkit accepts it; upstream reports `Attribute 'captures(none)' applied to incompatible type!` |
| `captures.ll` | ported | 1 pass |
| `cmpxchg-ordering-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `cmpxchg-ordering-3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `cmpxchg-ordering-4.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `cmpxchg-ordering.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `comment.ll` | ported | 1 pass |
| `constant-getelementptr-scalable_pointee.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `constant-splat-diagnostics.ll` | ported | 5 reject (5 with upstream's diagnostic pinned) |
| `constant-splat.ll` | ported | 1 pass |
| `convergence-control.ll` | ported | 1 pass |
| `datalayout-alloca-addrspace.ll` | ported | 1 pass |
| `datalayout-anypointersize.ll` | ported | 1 pass |
| `datalayout-program-addrspace.ll` | ported | 1 pass |
| `dbg-checksum.ll` | ported | 1 pass |
| `dbg-record-invalid-0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-4.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-5.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-6.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-7.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg-record-invalid-8.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dbg_declare_value.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `debug-info.ll` | ported | 1 pass |
| `debug-label-bitcode.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `debug-variant-discriminator.ll` | ported | 1 pass |
| `dicompileunit-conflicting-language-fields.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `dicompileunit-invalid-language-version.ll` | ported | 4 reject (4 with upstream's diagnostic pinned) |
| `dicompileunit-invalid-language.ll` | blocked-model | **G17** — 2/4 parts blocked. reported `invalid DWARF language 'DW_LNAME_C'`, upstream `expected DWARF language` |
| `dicompileunit.ll` | ported | 1 pass |
| `dicompositetype-members.ll` | ported | 1 pass |
| `diexpression.ll` | ported | 1 pass |
| `difile-empty-source.ll` | ported | 1 pass |
| `difile-escaped-chars.ll` | blocked-model | **G9** — rejected at 9:24: `expected UTF-8 string constant` |
| `diglobalvariable.ll` | ported | 1 pass |
| `diimportedentity.ll` | ported | 1 pass |
| `dilexicalblock.ll` | ported | 1 pass |
| `dilocalvariable-arg-large.ll` | ported | 1 pass |
| `dilocalvariable.ll` | ported | 1 pass |
| `dilocation.ll` | ported | 1 pass |
| `dimodule.ll` | ported | 1 pass |
| `dinamespace.ll` | ported | 1 pass |
| `diobjcproperty.ll` | ported | 1 pass |
| `distinct-mdnode.ll` | ported | 1 pass |
| `disubprogram-targetfuncname.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `disubprogram.ll` | ported | 1 pass |
| `disubrange-empty-array.ll` | ported | 1 pass |
| `disubroutinetype.ll` | ported | 1 pass |
| `ditemplateparameter.ll` | blocked-model | **G8** — rejected at 20:60: `expected metadata field value` |
| `ditype-large-values.ll` | ported | 1 pass |
| `dll-storage-class-local-linkage.ll` | ported | 12 reject (12 with upstream's diagnostic pinned) |
| `dllimport-dsolocal-diag.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `drop-debug-info-nonzero-alloca.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `drop-debug-info.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `dso_local_equivalent.ll` | ported | 1 pass |
| `export-symbol-anonymous-class.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `externally-initialized.ll` | ported | 1 pass |
| `extractvalue-invalid-idx.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `extractvalue-no-idx.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `fast-math-flags.ll` | blocked-model | **G12** — rejected at 52:3: `expected float-typed source for fpext` |
| `flags.ll` | ported | 1 pass |
| `fp-intrinsics-attr.ll` | ported | 1 pass |
| `function-operand-uselistorder.ll` | ported | 1 pass |
| `generic-debug-node.ll` | blocked-model | **G8** — rejected at 11:64: `expected metadata field value` |
| `getelementptr.ll` | blocked-model | **G4** — rejected at 28:29: `expected type` |
| `getelementptr_invalid_ptr.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_struct.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_vec_ce.ll` | ported | 1 pass |
| `getelementptr_vec_ce2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_vec_idx1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_vec_idx2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_vec_idx3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_vec_idx4.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_vec_struct.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `getelementptr_vscale_struct.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `global-addrspace-forwardref.ll` | ported | 1 pass |
| `globalvariable-attributes.ll` | blocked-model | **G11** — rejected at 3:20: `expected top-level entity` |
| `gv-invalid-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `half-constprop.ll` | ported | 1 pass |
| `half-conv.ll` | ported | 1 pass |
| `half.ll` | ported | 1 pass |
| `hex-float-overflow.ll` | ported | 2 reject (2 with upstream's diagnostic pinned) |
| `huge-array.ll` | ported | 1 pass |
| `ifunc-asm.ll` | ported | 1 pass |
| `ifunc-dsolocal.ll` | ported | 1 pass |
| `ifunc-program-addrspace.ll` | ported | 1 pass |
| `ifunc-stripPointerCastsAndAliases.ll` | ported | 1 pass |
| `ifunc-use-list-order.ll` | ported | 1 pass |
| `immarg-param-attribute.ll` | blocked-model | **G3** — rejected at 4:14: `expected unknown intrinsic` |
| `implicit-intrinsic-declaration-invalid.ll` | blocked-model | **G1** — reported `expected intrinsic signature mismatch`, upstream `invalid intrinsic signature` |
| `implicit-intrinsic-declaration-invalid2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `implicit-intrinsic-declaration-invalid3.ll` | blocked-model | **G1** — reported `expected unknown intrinsic`, upstream `unknown intrinsic 'llvm.foobar'` |
| `implicit-intrinsic-declaration.ll` | blocked-model | **G1** — rejected at 31:19: `expected intrinsic signature mismatch` |
| `inalloca-parse-error0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `inalloca.ll` | ported | 1 pass |
| `incomplete-ir-declarations.ll` | ported | 1 pass |
| `incomplete-ir-metadata-unsupported.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `incomplete-ir-metadata.ll` | blocked-model | **G14** — rejected at 16:33: `use of undefined metadata '!1'` |
| `incorrect-tdep-attrs-parsing.ll` | ported | 1 pass |
| `index-value-order.ll` | blocked-model | **G16** — rejected at 25:6: `Expected 'gv', 'module', 'typeid', 'flags' or 'blockcount' at the start of summary entry` |
| `initializes-attribute-invalid.ll` | ported | 10 reject (10 with upstream's diagnostic pinned) |
| `inline-asm-constraint-error.ll` | ported | 9 reject (9 with upstream's diagnostic pinned) |
| `inrange-errors.ll` | ported | 5 reject (5 with upstream's diagnostic pinned) |
| `insertextractvalue.ll` | ported | 1 pass |
| `insertvalue-invalid-idx.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `insertvalue-invalid-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `internal-hidden-alias.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `internal-hidden-function.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `internal-hidden-variable.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `internal-protected-alias.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `internal-protected-function.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `internal-protected-variable.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-addrspace.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-atomicrmw-add-must-be-integer-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-atomicrmw-fadd-must-be-fp-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-atomicrmw-fsub-must-be-fp-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-atomicrmw-scalable.ll` | ported | 5 reject (5 with upstream's diagnostic pinned) |
| `invalid-atomicrmw-xchg-fp-vector.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-attrgrp.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-byval-type2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-byval-type3.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `Attribute 'byval' does not support unsized types!` |
| `invalid-c-style-comment0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-c-style-comment1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-c-style-comment2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-c-style-comment3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-comdat.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-comdat2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-datalayout-override.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-debug-info-version.ll` | ported | 1 pass |
| `invalid-dicompileunit-emissionkind-bad.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `value for 'emissionKind' too large` |
| `invalid-dicompileunit-language-bad.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dicompileunit-language-overflow.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `value for 'language' too large, limit is 65535` |
| `invalid-dicompileunit-missing-language.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dicompileunit-null-file.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dicompileunit-uniqued.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dicompositetype-missing-tag.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diderivedtype-missing-basetype.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diderivedtype-missing-tag.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dienumerator-missing-name.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dienumerator-missing-value.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diexpression-large.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diexpression-verify.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `!named = !{!DIExpression(0, 1, 9, 7, 2)}` |
| `invalid-difile-missing-directory.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-difile-missing-filename.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diflag-bad.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diglobalvariable-empty-name.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diimportedentity-missing-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-diimportedentity-missing-tag.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilexicalblock-missing-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilexicalblock-null-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilexicalblockfile-missing-discriminator.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilexicalblockfile-missing-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilexicalblockfile-null-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocalvariable-arg-large.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocalvariable-arg-negative.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocalvariable-missing-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocalvariable-null-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocation-field-bad.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocation-field-twice.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocation-missing-scope-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocation-missing-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocation-null-scope.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocation-overflow-column.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dilocation-overflow-line.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-dinamespace-missing-namespace.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-disubprogram-uniqued-definition.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-disubrange-count-large.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `value for 'count' too large, limit is 9223372036854775807` |
| `invalid-disubrange-count-negative.ll` | blocked-model | **G17** — reported `Metadata id is already used`, upstream `value for 'count' too small, limit is -1` |
| `invalid-disubrange-count-node.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `'count' cannot be null` |
| `invalid-disubrange-lowerBound-max.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `value for 'lowerBound' too large, limit is 9223372036854775807` |
| `invalid-disubrange-lowerBound-min.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `value for 'lowerBound' too small, limit is -9223372036854775808` |
| `invalid-disubroutinetype-missing-types.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ditemplatetypeparameter-missing-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ditemplatevalueparameter-missing-value.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-fp80hex.ll` | blocked-model | **G17** — reported `expected '=' after global name`, upstream `expected '=' in global variable` |
| `invalid-generic-debug-node-tag-bad.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-generic-debug-node-tag-missing.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-generic-debug-node-tag-overflow.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `value for 'tag' too large, limit is 65535` |
| `invalid-generic-debug-node-tag-wrong-type.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `expected DWARF tag` |
| `invalid-gep-missing-explicit-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-hexint.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-immarg.ll` | blocked-model | **G3** — reported `expected unknown intrinsic`, upstream `Attribute 'immarg' is incompatible with other attributes except the 'range' attribute` |
| `invalid-immarg2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-immarg3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-immarg4.ll` | blocked-model | **G3** — reported `expected unknown intrinsic`, upstream `Attribute 'range(i32 1, 145)' applied to incompatible type!` |
| `invalid-immarg5.ll` | blocked-model | **G3** — reported `expected unknown intrinsic`, upstream `(no CHECK text)` |
| `invalid-inline-constraint.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-inttype.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-label-call-arg.ll` | blocked-model | **G17** — reported `number of input constraints does not match number of parameters`, upstream `invalid type for function argument` |
| `invalid-label.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-landingpad.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-load-missing-explicit-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-mdnode-badref.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-mdnode-vector.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-mdnode-vector2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-metadata-attachment-has-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-metadata-function-local-attachments.ll` | blocked-model | **G17** — reported `expected constant value`, upstream `invalid use of function-local name` |
| `invalid-metadata-function-local-complex-1.ll` | blocked-model | **G17** — reported `expected constant value`, upstream `invalid use of function-local name` |
| `invalid-metadata-function-local-complex-2.ll` | blocked-model | **G17** — reported `expected constant value`, upstream `invalid use of function-local name` |
| `invalid-metadata-function-local-complex-3.ll` | blocked-model | **G17** — reported `expected constant value`, upstream `invalid use of function-local name` |
| `invalid-metadata-has-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-name.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-name2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-opaque-ptr-addrspace.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-opaque-ptr-double-addrspace.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-opaque-ptr.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ptrauth-const1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ptrauth-const2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ptrauth-const3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ptrauth-const4.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ptrauth-const5.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-ptrauth-const6.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-safestack-param.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-safestack-return.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-specialized-mdnode.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-target-type-mixed.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-untyped-metadata.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-function-between-blocks.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-function-missing-named.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-function-missing-numbered.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-global-missing.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-indexes-duplicated.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-indexes-empty.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-indexes-one.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-indexes-ordered.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-indexes-range.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-indexes-toofew.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-indexes-toomany.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder_bb-missing-bb.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder_bb-missing-body.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder_bb-missing-func.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder_bb-not-bb.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder_bb-not-func.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-uselistorder_bb-numbered.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invalid-vecreduce.ll` | blocked-model | **G1** — reported `expected intrinsic signature mismatch`, upstream `Intrinsic has incorrect return type!` |
| `invalid_cast.ll` | blocked-model | **G17** — reported `expected valid trunc: invalid operation: trunc/zext/sext changes the vector element count`, upstream `invalid cast opcode for cast from '<4 x i64>' to '<3 x i8>'` |
| `invalid_cast2.ll` | blocked-model | **G17** — reported `expected valid trunc: invalid operation: trunc/zext/sext changes the vector element count`, upstream `invalid cast opcode for cast from '<4 x i64>' to 'i8'` |
| `invalid_cast3.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `invalid cast opcode for cast from '<4 x ptr>' to '<2 x ptr>'` |
| `invalid_cast4.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `invoke-nonzero-program-addrspace.ll` | ported | 1 reject; the `-data-layout=P200` unit runs in `parser_calls.rs::invoke_addrspace_matches_upstream_in_both_program_addrspaces` |
| `large-comdat.ll` | ported | 1 pass |
| `local-unnamed-addr.ll` | ported | 1 pass |
| `lround.ll` | ported | 1 pass |
| `masked-load-store-intrinsics-attributes.ll` | ported | 1 pass |
| `max-inttype.ll` | ported | 1 pass |
| `memory-attribute-errors.ll` | ported | 8 reject (8 with upstream's diagnostic pinned) |
| `memory-attribute.ll` | ported | 1 pass |
| `metadata-annotations.ll` | ported | 1 pass |
| `metadata-decl.ll` | ported | 1 pass |
| `metadata-function-local.ll` | blocked-model | **G3** — rejected at 4:14: `expected unknown intrinsic` |
| `metadata-null-operands.ll` | ported | 1 pass |
| `metadata-use-uselistorder.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `metadata.ll` | blocked-model | **G1** — rejected at 21:13: `expected unknown intrinsic` |
| `missing-tbaa.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `multi-mod-disassemble.ll` | ported | 1 pass |
| `multi-summary-disassemble.ll` | ported | 1 pass |
| `mustprogress-parse-error-0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `mustprogress-parse-error-1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `mustprogress-parse-error-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `musttail-invalid-1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `musttail-invalid-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `musttail.ll` | ported | 1 pass |
| `mutually-recursive-types.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `named-metadata.ll` | blocked-model | **G9** — rejected at 28:1: `expected valid UTF-8 metadata name` |
| `no-mdstring-upgrades.ll` | ported | 1 pass |
| `noalias-addrspace-md.ll` | ported | 1 pass |
| `nofpclass-invalid.ll` | blocked-model | **G17** — 8/18 parts blocked. reported `expected valid mask value for 'nofpclass'`, upstream `invalid mask value for 'nofpclass'`. The ten ported parts include nine that carried a second, distinct defect until 2026-08-20: `expected '(' in nofpclass attribute` / `expected ')' in nofpclass attribute` where `LLParser::parseNoFPClassAttr` prints the labels bare. That was not G17 (the wrapper was right, the label had a suffix) and is recorded as `docs/divergences.md` entry 115; the two labels are now bare. |
| `nofpclass.ll` | ported | 1 pass |
| `non-global-value-max-name-size-2.ll` | ported | 1 pass |
| `non-global-value-max-name-size.ll` | N/A | Contract is `opt -non-global-value-max-name-size=N`, a value-naming knob llvmkit has no equivalent for; nothing about the parse is under test. |
| `numbered-values.ll` | ported | 1 pass |
| `opaque-ptr-cmpxchg.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `opaque-ptr-intrinsic-remangling.ll` | blocked-model | **G1** — rejected at 30:1: `expected valid invoke: call argument #2 type mismatch: expected ptr, got void ()` |
| `opaque-ptr-invalid-forward-ref-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `opaque-ptr-invalid-forward-ref.ll` | blocked-model | **G17** — reported `forward reference and definition of global have different types`, upstream `invalid forward reference to function 'f' with wrong type: expected 'ptr' but was 'ptr addrspace(1)'` |
| `opaque-ptr-struct-types.ll` | ported | 1 pass |
| `opaque-ptr.ll` | blocked-model | **G15** — rejected at 173:13: `use of undefined global '@0'` |
| `phi-first-class-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `pr119818.ll` | ported | 1 pass |
| `private-hidden-alias.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `private-hidden-function.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `private-hidden-variable.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `private-protected-alias.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `private-protected-function.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `private-protected-variable.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `ptrauth-const.ll` | ported | 1 pass |
| `ptrtoaddr-invalid-constexpr.ll` | blocked-model | **G18** — 2/10 parts blocked. llvmkit's own verifier rejects it: invalid operation: PtrToAddr result must be address width |
| `ptrtoaddr-invalid.ll` | blocked-model | **G17** — 9/9 parts blocked. reported `expected valid ptrtoaddr: invalid operation: PtrToAddr result must be address width`, upstream `assembly parsed, but does not verify as correct!` |
| `ptrtoaddr.ll` | ported | 1 pass |
| `range-attribute-invalid-range.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `range-attribute-invalid-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `range.ll` | ported | 1 pass |
| `remangle.ll` | blocked-model | **G1** — rejected at 24:16: `expected intrinsic signature mismatch` |
| `riscv_vls_cc.ll` | ported | 1 pass |
| `scalable-vector-struct.ll` | ported | 1 pass |
| `select.ll` | ported | 1 pass |
| `short-hexpair.ll` | ported | 1 pass |
| `skip-value-numbers-globals.ll` | blocked-model | **G15** — rejected at 18:15: `use of undefined value '@6'` |
| `skip-value-numbers-invalid.ll` | ported | 5 reject (5 with upstream's diagnostic pinned) |
| `skip-value-numbers.ll` | ported | 1 pass |
| `source-filename-backslash.ll` | ported | 1 pass |
| `source-filename.ll` | ported | 1 pass |
| `sret-parse-error0.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `sret-type-attr.ll` | ported | 1 pass |
| `struct-ret-without-upgrade.ll` | blocked-model | **G1** — rejected at 23:19: `expected unknown intrinsic` |
| `summary-flags.ll` | ported | 1 pass |
| `summary-flags2.ll` | ported | 1 pass |
| `summary-parsing-error.ll` | blocked-model | **G18** — llvmkit accepts it; upstream reports `Reference to undefined global "does_not_exist"` |
| `symbolic-addrspace-datalayout.ll` | blocked-model | **G6** — 2/4 parts blocked (`num-to-sym.ll`, `sym-to-sym.ll`, both `--print-addrspace-name=true`) |
| `symbolic-addrspace.ll` | ported | 7 split-file parts |
| `target-type-mangled.ll` | ported | 1 pass |
| `target-type-param-errors.ll` | ported | 3 reject (3 with upstream's diagnostic pinned) |
| `target-type-params.ll` | ported | 1 pass |
| `target-type-properties.ll` | blocked-model | **G18** — 7/8 parts blocked. llvmkit accepts it; upstream reports `Global @global_var has illegal target extension type` |
| `target-types.ll` | ported | 1 pass |
| `thinlto-bad-summary1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `thinlto-bad-summary2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `thinlto-bad-summary3.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `thinlto-blockcount-summary.ll` | ported | 1 pass |
| `thinlto-flags-summary.ll` | ported | 1 pass |
| `thinlto-memprof-summary.ll` | ported | 1 pass |
| `thinlto-multiple-summaries-for-guid.ll` | ported | 1 pass |
| `thinlto-summary-visibility.ll` | ported | 1 pass |
| `thinlto-summary.ll` | blocked-model | **G21** — rejected at 119:14: `expected field label here` |
| `thinlto-vtable-summary.ll` | blocked-model | **G16** — rejected at 36:6: `Expected 'gv', 'module', 'typeid', 'flags' or 'blockcount' at the start of summary entry` |
| `thinlto-vtable-summary2.ll` | blocked-model | **G19** — printed module does not re-print identically |
| `tls-models.ll` | ported | 1 pass |
| `token.ll` | blocked-model | **G3** — rejected at 6:14: `expected unknown intrinsic` |
| `unnamed-addr.ll` | ported | 1 pass |
| `unnamed-alias.ll` | ported | 1 pass |
| `unnamed-comdat.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `unnamed.ll` | ported | 1 pass |
| `unsized-recursive-type.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `unsupported-constexprs.ll` | ported | 2 reject (2 with upstream's diagnostic pinned) |
| `uselistorder.ll` | blocked-model | **G4** — rejected at 7:16: `expected type` |
| `uselistorder_bb.ll` | ported | 1 pass |
| `uselistorder_global.ll` | ported | 1 pass |
| `uwtable-1.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `uwtable-2.ll` | ported | 1 reject (1 with upstream's diagnostic pinned) |
| `vbool-cmp.ll` | ported | 1 pass |
| `vector-select.ll` | ported | 1 pass |
| `vector-shift.ll` | ported | 1 pass |
| `x86_intrcc.ll` | ported | 1 pass |
| `zero-input-phi.ll` | ported | 1 pass |
