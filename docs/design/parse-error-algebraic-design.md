# `ParseError` becomes an algebraic type

**Status:** design, approved for planning. Every figure below carries the command that produced
it and was measured at `4736ca5` — re-derive before quoting, they move with the parser.

An earlier revision carried figures measured at two commits, and a re-derivation found ten of
them wrong. `ll_parser.rs` has the same blob at `1034272` and at `4736ca5`
(`git rev-parse 1034272:crates/llvmkit-asmparser/src/ll_parser.rs` against `HEAD:`), so none of
the ten was staleness; each was an error at the moment of writing. Most were repaired by
**deleting** the number rather than correcting it, per the `claims-and-counts` skill: a count
that no decision rests on is rot surface and nothing more.

## The problem

`VerifierRule` is algebraic — 114 variants, no strings. `ParseError` is not: the parser builds
every diagnostic through one of six helpers, and **two `Cow<'static, str>` carriers absorb all
six**. The per-helper counts are in **Measured surface** below; the total is not quoted here,
because a synthesized sum is the one figure in this document that no command reproduced.

```rust
#[error("expected {expected}")]
Expected { expected: Cow<'static, str>, loc: DiagLoc },

#[error("{message}")]
Message { message: Cow<'static, str>, loc: DiagLoc },
```

This violates three doctrine rules.

**D1 — "State machines are typestates."** `ParseError` has two operational states today — *has
a source location* and *has none* — and the distinction is carried by a runtime predicate,
`loc() -> Option<DiagLoc>`, whose `match` must be hand-extended for every one of the 21 variants
(`grep -c '#\[error(' parse_error.rs`, at `84a66ea`). That is exactly the `is_attached()` shape
D1 exists to forbid. Exactly three return `None`, and they are the same three that are not
diagnostics at all: `Io`, `BrandInUse`, `BrandRetired`.

**D3 — "Erased forms are explicitly opt-in."** `Message` *is* an erased form: it erases which
diagnostic this is down to a string. It is the opposite of opt-in — it is the default, and the
easiest thing to reach for. `CLAUDE.md` is blunter: *"No silent erasure. Erasure is spelled
`as_dyn()` / `as_erased()`."* `Message` is silent erasure with no spelling at all.

**D5 — "Operand registration is structural: one exhaustive place per primitive."** The
diagnostic registry today is scattered literals with no exhaustive match anywhere.

The cost is not theoretical. Every invented message this program deleted —
`expected valid shufflevector mask`, `a vector-of-pointers getelementptr base is not yet
supported`, `expected 'label'` at thirteen sites — was a `Cow` someone typed. **A `Cow` accepts
anything; a variant must be added, and adding one is visible in review.** Divergence 121 was
closed in a single sweep precisely because `VerifierRule` is an enum you can enumerate; there is
no equivalent lever for the parser today.

**And the erasure destroys structure, not just names.** The parser has a pair of
error-constructing helpers whose whole bodies are:

```rust
fn builder_err(&self, label: &str, e: IrError) -> ParseError {
    ParseError::Expected { expected: format!("valid {label}: {e}").into(), loc: … }
}
fn builder_err_at(&self, loc: Span, label: &str, e: IrError) -> ParseError {
    ParseError::Expected { expected: format!("{label}: {e}").into(), loc: … }
}
```

They are called **123 times** — 113 and 10 (`grep -o 'builder_err(' ll_parser.rs | wc -l` and
the `_at` twin, each less its own definition). Each call takes a fully structured `IrError` and
flattens it into text, so nothing downstream can `match` on why the builder refused — it can
only parse the rendering. That is the single largest cluster in the file and on its own it
justifies the change. Counting only direct `format!` sites misses it entirely; the helpers are
where the mass is.

**Note the two render differently** — `builder_err` prepends `valid `, `builder_err_at` does
not. The prefix split this document later uses to argue against a collapsed `BuilderRejected`
already exists *inside* the `Expected` half, between two helpers that otherwise look identical.

**Worse, the labels duplicate an enum that already exists.** The two helpers pass **97 distinct
literal labels** — 87 and 10, with no overlap
(`rg -oN 'builder_err\(\s*"(?:[^"\x5c]|\x5c.)*"' ll_parser.rs | sort -u | wc -l`, and the `_at`
twin; the escape-aware pattern matters, because one label is `c\"...\" constant`). Most are
exactly the `.ll` keyword `Opcode::keyword()` already returns. The rest — the ones that must
become their own variants — are **not opcodes at all**, and six of those are an opcode *plus a
step*:

```
phi.add_incoming   switch.add_case         catchswitch.add_handler
landingpad.catch   landingpad.filter       indirectbr.add_destination
```

A `{ opcode: Opcode }` field cannot carry those six; they are six diagnostics, not one
parameterised by an opcode.

**The same opcode-to-keyword mapping is written out by hand four times in `ll_parser.rs`**, not
twice as an earlier revision said: `IntBinOp::mnemonic`, `IntCast::mnemonic`, an inline
`Opcode -> &'static str` match inside `unsupported_constant_expr_at`, and an
`FpBinOp -> (BinaryOpcode, &str)` tuple match in the FP-arithmetic arm. Each is a parallel copy
of `Opcode::keyword()`. The third is not a pure duplicate and must not be deleted as one: its
`_ =>` arm emits a *different* diagnostic, so the match is simultaneously a rendering and a
membership test for "constexpr form upstream removed" — the set is the data, and it wants a
named constant, with `keyword()` doing only the rendering.

## Doctrine anchors

In the row form `docs/type-safety-vs-llvm.md` uses. The right-hand column is what this design
delivers; none of it exists today.

| Problem shape | Doctrine | Upstream LLVM C++ | llvmkit after this change |
| --- | --- | --- | --- |
| Invent a diagnostic upstream never emits | D3, D5 | `error(Loc, "any text you like")` — a `Twine` accepts anything | A `ParseErrorKind` variant must be added, and a drift test rejects text absent from the vendored `LLParser.cpp` / `LLLexer.cpp` |
| Ask why the builder refused a construct | D3 | n/a — upstream's parser calls IR constructors that assert | A variant per diagnostic identity keeps the structured `IrError` instead of `format!`-ing it into 123 strings |
| Handle an error that has no source position | D1 | `SMDiagnostic` carries an *optional* `SMLoc`; the open-file failure uses the empty one | Unrepresentable: `loc: Span` is a field, not an `Option`, so a non-diagnostic cannot enter the type |
| Discover which entry points can fail how | Fallibility honesty | one `SMDiagnostic &Err` out-parameter for every failure kind | Every entry point returns `Result<T, ParseError>`, and no entry point can report a failure that is not a diagnostic |
| Match on a diagnostic rather than its rendering | D3, D6 | `Err.getMessage()` is a `std::string` | `match e.kind { … }`, exhaustive, with typed fields |

The last row is the one that pays for the rest: it is what let divergence 121 be closed in a
single sweep over `VerifierRule`, and what the parser has no equivalent of today.

**These rows belong in `docs/type-safety-vs-llvm.md`, and none of them is there** —
`grep -n "ParseError\|asmparser" docs/type-safety-vs-llvm.md` returns nothing, at `4736ca5`.
That document is the user-facing map from LLVM C++ failure modes to doctrine ids, so each row
lands in it *in the commit that makes the row true*: the D1 row with Phase 1, the D3/D5 row with
the drift test, the builder row with Phase 3. Copying them all up front would put claims in a
user-facing document ahead of the code, which is the failure this design is otherwise built to
avoid.

## Measured surface

| what | count | command |
|---|---|---|
| `self.expected(…)` sites in `ll_parser.rs` | 163 | `grep -cE 'self\.expected\(' ll_parser.rs` |
| distinct `expected` literals, crate-wide | 105 | `grep -rhoE '\.expected\("[^"]*"' src/ \| sort -u \| wc -l` |
| `expected` sites using `format!` | **0** | `grep -oE 'self\.expected\(&?format!' ll_parser.rs \| wc -l` |
| `self.message_at(…)` sites | 167 | `grep -o 'self\.message_at(' ll_parser.rs \| wc -l` |
| distinct literal `message_at` texts | 100 | `grep -oE 'message_at\([a-z_]+, *"[^"]*"' ll_parser.rs \| grep -oE '"[^"]*"' \| sort -u \| wc -l` |
| **`self.message(…)` sites** | **75** | `grep -o 'self\.message(' ll_parser.rs \| wc -l` |
| **distinct literal `message` texts** | **58** | `rg -oN 'self\.message\(\s*"(?:[^"\x5c]\|\x5c.)*"' ll_parser.rs \| sort -u \| wc -l` |
| `Message` sites using `format!` | 57 | `grep -cE 'message_at\([a-z_]+, *&?format!\|ParseError::Message *\{' ll_parser.rs` |
| corpus rows matching on text | 319 | `grep -c 'error=' parser_corpus_manifest.txt` |
| test assertions on rendered text | 216 | `grep -rc 'to_string()' tests/*.rs \| awk -F: '{s+=$2} END {print s}'` |

The two bolded rows are new. An earlier revision measured `self.message_at(` and labelled the
row `message_at / message`, so **75 sites and 58 distinct literals were never counted** — and
the variant target derived from that table inherited the omission.

**The target is not stated here.** An earlier revision predicted "~205 nullary and ~57 typed"
from `105 + 100`, which is the same omission again. Phase 0 exists precisely to enumerate the
real set, in both directions and with the `builder_err` family bucketed separately; **size the
work from its output, not from a sum written in advance.** What is safe to say without counting:
the `expected` half has **zero** `format!` uses, so that half of the conversion is mechanical.

## Design

**One type. No sum, no wrapper, no optional location.**

```rust
/// A diagnostic at a source position. Every one has a location, so `loc` is not optional.
pub struct ParseError { pub kind: ParseErrorKind, pub loc: Span }

/// One variant per upstream diagnostic. No `Cow`, no free-form string.
pub enum ParseErrorKind { /* sized from Phase 0's census */ }

pub type ParseResult<T> = Result<T, ParseError>;   // alias unchanged; the claim becomes true
```

`loc` moves out of the variants onto the struct, appearing **once** per error instead of once
per variant, and the hand-maintained `loc() -> Option<DiagLoc>` accessor disappears — D1.

**`DiagLoc` is deleted; the location is a `Span`.** `DiagLoc` was
`{ span: Span, file: Option<FileLocRange> }`, and the `file` half is dead: `with_file` has no
caller anywhere in the workspace (`grep -rn "with_file(" crates/ llvmkit/ --include=*.rs`
returns only its own definition), so every diagnostic in the tree carries `file: None`. That is
the same optional-state shape D1 forbids one level up, hiding inside the type the fix keeps.
With `file` gone the struct wraps a single `Span` and earns nothing, and `Span` *is* our
`SMLoc` — a byte range into the caller's buffer, which is exactly what the lexer argument below
rests on. Line and column stay a caller-side projection, which is what the corpus harness
already does. `FileLocRange` itself stays: it belongs to `AsmParserContext`, not to diagnostics.

**Neither type is `#[non_exhaustive]`.** `ParseError` carries the attribute today and
`ParseErrorKind` does not inherit it. The diagnostic vocabulary is closed by construction —
every variant is either a text the vendored `LLParser.cpp` / `LLLexer.cpp` emits or a recorded
llvmkit divergence — and an exhaustive `match` in *user* code is the whole D3/D6 payoff; keeping
the attribute would hand every downstream caller a `_ =>` arm to fill, which is the defect this
document spends its Phase 1 removing from our own code. A new variant is then a breaking change,
flagged inline in `CHANGELOG.md` under the pre-1.0 policy. `SymbolKind` loses it on the same
grounds (`grep -rln "SymbolKind::" crates/ llvmkit/ --include=*.rs | grep -v asmparser/src`
returns nothing, so nothing outside the crate matches on it yet).

> **Superseded 2026-09-05 — the scope widened past this document.** This
> paragraph used to end "`IrError` and `VerifierRule` keep theirs; that is a
> separate question this document does not open." The user opened it and ruled
> the other way: **no `#[non_exhaustive]` anywhere in the workspace.** All 21
> attributes are gone, `IrError` and `VerifierRule` among them, and the rule
> now lives in `CLAUDE.md` under *Rules that fail CI or review* so it binds new
> enums as well. Nothing above changes — `ParseErrorKind` is simply born
> exhaustive under a project-wide rule rather than a parser-local one.

**The fields are public, and that is a deliberate exception** to `AGENTS.md`'s *"public
config/result structs keep fields private and expose accessors"*. An error is matched and
destructured, never encapsulated — `let ParseError { kind, loc } = e;` is the point of the
redesign — and the accessor pair would be ceremony over two `Copy`-ish fields. The `AGENTS.md`
bullet gains the carve-out in the same commit, so the rule and the tree agree.

The three non-diagnostic variants are **not** relocated into a wrapper type. An earlier draft of
this spec proposed a `ParseFailure { Parse, Io, BrandInUse, BrandRetired }`; that was wrong. It
preserved the coupling and added a level. They are removed along with the entry points that
produce them, because each of those entry points does work the parser has no business doing.

### The entry points shrink 24 → 17

`grep -oE "^pub fn [a-z_]+" parser.rs | wc -l` gives 24. Classified by which of the three
non-diagnostic variants each can actually produce:

| entry points | can produce | disposition |
| --- | --- | --- |
| `parse_file_dynamic`, `parse_file_branded`, `parse_assembly_file`, `parse_assembly_file_with_config`, `parse_summary_index_assembly_file` | `Io` | **deleted** — the caller reads the file |
| `parse_branded`, `parse_branded_with_config` | `BrandInUse` / `BrandRetired` | **deleted** — the caller claims the brand |
| the remaining **17** | none, yet declare all three | unchanged, and now honest |

**"Honest" is not "exact", and the difference matters given what this document argues.** The
fragment parsers — `parse_type`, `parse_constant_value` and their `_with_slots` twins — still
cannot produce every variant of `ParseError`; `parse_type` will never report `Redefinition`. By
the over-declaration rule this document applies to `Module::branded` and `DataLayout::parse`,
that is the same shape. It is left alone for a reason that must be stated rather than assumed:
`AGENTS.md` prescribes one error enum per subsystem, and an over-declaration only *costs*
something once a consumer writes a catch-all to absorb the unreachable arms. No consumer of the
fragment parsers does. Phase 1's last step measures that claim across every `IrResult` the
parser consumes rather than resting on it.

Both replacements are one line, and both are strictly more explicit than what they replace:

```rust
// was: parse_file_dynamic("dir/foo.ll")   — derived the module name from the path
let m = parse_into(Module::dynamic("dir/foo.ll"), std::fs::read("dir/foo.ll")?)?;

// was: parse_branded::<MyBrand, _>(src)   — silently named the module "asm"
let m = parse_into(Module::branded::<MyBrand, _>("foo.ll")?, src)?;
```

`parse_into` was always the primitive; these seven were conveniences that **supplied inputs the
caller never wrote down**. `parse_file_dynamic` derived a module name from a `Path` through
`module_name_for` (`path.file_name()`), and that name is printed as `; ModuleID = '…'` — so a
`Path` argument silently became bytes of output, with the directory component dropped on the
way. `parse_branded` hardcoded `"asm"`.

### Why the parser performs no I/O — in either tree

This is not an llvmkit preference. Upstream's parser primitive takes a buffer, not a stream:

```cpp
// Parser.cpp — the primitive every entry point funnels into
static bool parseAssemblyInto(MemoryBufferRef F, Module *M, ModuleSummaryIndex *Index,
                              SMDiagnostic &Err, …);
```

and the file read lives in **Support**, outside `lib/AsmParser` entirely
(`MemoryBuffer::getFileOrSTDIN`). `parseAssemblyFile` is a six-line wrapper around it. So
`ParseError::Io` is not merely a doctrine violation — it merges a Support-layer concern into the
parser's error type at a boundary upstream keeps separate.

A `Read`-based lexer is not an option in either tree, and `LLLexer.h` shows why:

```cpp
const char *CurPtr;
StringRef   CurBuf;
const char *PrevTokEnd = nullptr;
const char *TokStart;
```

Backtracking, `PrevTokEnd`, and token spans are pointer arithmetic into one buffer. A stream
forces internal buffering, and spans can then no longer be byte offsets into the caller's bytes —
which is what a `Span` — and so a `ParseError`'s `loc` — is built on. `Lexer::new(src: &'src
[u8])` already matches.

### The module name cannot come from the text

Worth recording, because it is the obvious alternative and it does not work. `; ModuleID = '…'`
is a **comment**: `LLLexer.cpp` has `case ';': SkipLineComment();` and no `ModuleID` token. It is
written by `AsmWriter` and never read back, in either tree. The directive that *is* parsed —
`source_filename`, `LLParser::parseSourceFileName` — is a different field (the original source,
`foo.c` for clang output), printed separately, and aliasing the two would be exactly the silent
derivation this change removes.

That is why upstream hardcodes `MemoryBufferRef F(AsmString, "<string>")` in
`parseAssemblyString` and passes the filename in `parseAssemblyFile`: the identifier can only
come from the caller. Making the caller pass it is the only honest source, not ceremony.

**Divergence, verified, closed by this change.** `parse_dynamic` calls `Module::dynamic("asm")`
where `parseAssemblyString` uses `"<string>"`, and both trees print the identifier verbatim
(`fmt_module_with_options`, mirroring `AssemblyWriter::printModule`) — so a string-parsed module
round-trips with a different `; ModuleID` line than upstream's. Unrecorded in either ledger:

```bash
grep -rnE 'ModuleID|parseAssemblyString|module name|module identifier|buffer identifier|"asm"' \
  docs/divergences.md docs/future-work.md   # no output, at 4736ca5
```

Fixed by changing the constant, in the same commit that deletes the file entry points; no ledger
row is opened for a defect closed on arrival.

*(The command is deliberately **case-sensitive** and spells the concept six ways. An earlier
revision used a case-insensitive pattern and claimed it returned nothing; it returns four lines,
every one a false positive on `Option<String>` or the `ModuleId` type. The conclusion survived,
but an absence claim whose command does not return nothing is exactly the defect `CLAUDE.md`
names — so the command was fixed, not the sentence around it.)*

### Text carrier

**One rendering path, not two.** An earlier revision gave nullary variants a hand-written
exhaustive `message()` (as `VerifierRule` does) while parameterised variants used `thiserror`
interpolation — two `Display` mechanisms inside one enum, and a second list to keep in step.
`thiserror` already renders a nullary variant from its own `#[error("…")]`, so every variant
uses that, and the drift test's input is a `NULLARY` slice the macro emits beside the enum. One
source of truth, generated from one declaration (D5); a new nullary variant lands in the slice
because it cannot be declared without doing so.

The reason parameterised variants cannot adopt the literal-at-head shape stands: upstream builds
those mid-string (`"invalid symbolic addrspace '" + AddrSpaceStr + "'"`), so pinning a
head-literal would change the text, which is contractual.

**The drift test checks nullary variants on their whole text, and parameterised ones on their
literal prefix.** Both are structural rather than an exemption list: a new variant of either
shape is checked automatically, and you cannot opt out without changing the variant's form — a
visible act in review. An enumerated exemption list would rot; this cannot.

An earlier revision excluded parameterised variants outright, calling them uncheckable because
upstream builds those messages mid-string. That is a reason they cannot be compared *whole*, not
a reason they cannot be compared at all — and Phase 0's census already demonstrates the
technique it claimed did not exist:

```rust
// the census, already running
let head = ours.split('{').next().unwrap_or(ours).trim_end();
if head.len() >= 8 && upstream.iter().any(|u| u.starts_with(head)) {
    return "parameterised";   // upstream extends our literal prefix
}
```

`invalid symbolic addrspace '` either appears as a prefix in the vendored sources or it does
not, and an invented parameterised message is caught by exactly that. The minimum-length guard
is what keeps a short prefix from matching half of upstream by accident; carry it over.

**What remains genuinely uncheckable, and must be said rather than glossed:** the
builder-rejected family. Upstream asserts inside its IR constructors where llvmkit raises a
diagnostic, so there is no upstream text to compare those against at any granularity. That is
roughly forty variants of llvmkit-authored wording that no oracle in this plan validates.

### Field typing — store the thing, not its rendering

**D6: "modeled directly rather than flattened into weak runtime predicates."** A parameterised
variant's fields carry the *value*, not its rendering.

**Count the helpers, not just `format!`.** `builder_err` and `builder_err_at` build their
message internally, so a `format!` census misses all 123 of them. Full picture:

| construction path | sites | becomes |
|---|---|---|
| `builder_err` / `builder_err_at` | **123** | one variant per diagnostic identity, `source: IrError` |
| `expected` / `expected_at` | 177 | nullary variants |
| `message_at` / `message` | **242** | nullary variants, or fields below |
| direct `format!` | fields below | fields below |

Each site count is `grep -o '<helper>(' ll_parser.rs | wc -l`, less the helper's own definition.
**The `message` row was 167 in an earlier revision, which is `message_at` alone**; `self.message(`
adds 75 sites carrying 58 distinct literals.

What the interpolating sites should become, by what they carry:

| interpolates | becomes |
|---|---|
| a nested `IrError` | `source: IrError` — the whole point of the refactor |
| a slot, id, index or limit | `u32` / `u64` |
| an `AtomicRmwBinOp` | the enum itself |
| a rendered type | `TypeKindLabel` **plus** the spelling as owned text — see below |
| a symbol or field name | owned text — genuinely arbitrary input |
| a fixed set spelled as `&'static str` | a new enum, or `SymbolKind`, which already exists |

**No site counts in that table, deliberately.** An earlier revision carried five, and four were
wrong — the type-rendering row said 7 where the tree has 14, the `{name}` row counted display
strings the same revision excluded two paragraphs earlier, and the "enum it already is" row
double-counted the two helper bodies already tallied as the 123. They existed only to feed a
"~19 owned-text fields" prediction that Phase 5 was told to check itself against, and that
prediction was already false when written: the type renderings alone exceed it. The counts are
gone with the prediction; what replaces both is a rule, below.

**The last row's "enum it already is" named no enum that exists.** The three fixed-set `&str`
parameters behind it are `check_value_id`'s `(kind, prefix)` pair — a hand-written twin of
`SymbolKind` and its `sigil()`, both of which this file already defines — plus a
`pad: &'static str` and a `what` compared with `if what == "array"`. Two of those become small
enums; the first becomes the `SymbolKind` it was duplicating.

**A `format!` census must exclude the non-diagnostics and read multi-line calls.** Some
`format!` sites build a *symbol display name* (`@{name}`, `@{id}`, `%{name}`, `%{id}`) passed
into `check_valid_variable_type` / `check_resolved_global_type`; they are untouched by this
refactor. And a single-line regex misses the calls whose template sits on the following line —
an earlier revision's count did, and all of those are diagnostics.

**The nested-error sites are the most valuable part of this refactor, and they are worse than
stringly-typing.** `format!("valid ret: {e}")` takes a fully structured `IrError` and flattens it
into a string: D3 erasure at its most destructive, since the entire error is destroyed and only
its rendering survives. A caller wanting to know *why* the builder refused has to parse text.
They become:

**Split them by diagnostic identity. Do not parameterise the identity.** An earlier draft of
this spec proposed a single `BuilderRejected { context: BuilderContext, source: IrError }`
covering all 123. That is `ErrorExpected { expected: str }` with the string typed: the variant
stops naming the finding, and a `match` on the error tells you nothing until you also match the
context field. The governing rule is **`ExpectedOpcode { got, expected: Opcode }` beats
`ErrorExpected { got, expected: str }`** — a field carries data that varies *within* one
diagnostic, never the diagnostic's identity.

The sites confirm it: they do not even render alike. `cannot create forward reference: {e}` goes
through `Message` and `valid alias definition: {e}` through `Expected`, so one renders with an
`expected ` prefix and one without. A collapsed variant would need a field to decide the
*prefix* — and as noted above, the two `builder_err` helpers already disagree on that same
prefix between themselves.

```rust
/// ONE shape — "the builder refused this instruction" — where the opcode is
/// genuinely data. Reuses `llvmkit_ir::Opcode` rather than minting a parallel
/// set: most hand-typed labels are already exactly `Opcode::keyword()`.
///
/// `Opcode` has no `Display`, so the rendering names `keyword()` explicitly —
/// the shape `LexError` already uses for `.kind.unterminated_message()`.
/// `keyword()` is also what makes this correct rather than nearly correct:
/// four labels (`catchret`, `cleanupret`, `cmpxchg`, `va_arg`) are keywords
/// whose `Opcode` variant is spelled differently, so a rendering derived from
/// variant names would silently change four diagnostics.
#[error("expected valid {}: {source}", .opcode.keyword())]
InstructionBuilderRejected { opcode: Opcode, source: IrError },

/// Distinct shapes. Each is its own diagnostic, not a parameterisation.
#[error("expected function parameter slot {slot}: {source}")]
FunctionParameterRejected { slot: u32, source: IrError },
#[error("expected valid datalayout: {source}")]           DataLayoutRejected       { source: IrError },
#[error("expected valid alias definition: {source}")]     AliasDefinitionRejected  { source: IrError },
#[error("expected valid global definition: {source}")]    GlobalDefinitionRejected { source: IrError },
#[error("cannot create forward reference: {source}")]     ForwardReferenceCreationFailed { source: IrError },
#[error("cannot resolve forward reference: {source}")]    ForwardReferenceUnresolved     { source: IrError },
```

Output is byte-identical, structure intact, and the underlying `IrError` is finally `match`-able.
The cost of obeying the rule is roughly one variant per non-opcode label instead of one variant
total, and `BuilderContext` is deleted — it was the collapsed identity wearing an enum.

**All four hand-written opcode-to-keyword copies go with it.** `IntBinOp::mnemonic` and
`IntCast::mnemonic` are straight parallel copies and simply disappear into
`BinaryOpcode::keyword()` / `CastOpcode::keyword()`. The `FpBinOp` tuple match is the same,
one arm wider. The fourth — the inline `Opcode -> &'static str` match in
`unsupported_constant_expr_at` — needs care, because its `_ =>` arm emits a different
diagnostic: the match is a rendering *and* a membership test for "constexpr form upstream
removed". Split those. The set becomes a named constant, `keyword()` does the rendering, and the
fall-through stays its own diagnostic.

The same rule keeps pairs apart that look collapsible:

```rust
#[error("atomicrmw {op} operand must be an integer")]
AtomicRmwOperandNotInteger { op: AtomicRmwBinOp },
#[error("atomicrmw {op} operand must be a floating point type")]
AtomicRmwOperandNotFloat   { op: AtomicRmwBinOp },
```

A single `AtomicRmwOperandWrongType { op, wanted: TypeKind }` would render both, and would be
exactly the collapse this rule forbids.

The rest follow the same rule:

```rust
ExpectedKeyword { keyword: TokenKind },               // not text — the token is an enum
MetadataFieldValueTooLarge { value: u64, max: u64 },  // not text — they are numbers
UndefinedTypeNamed { name: Box<str> },                // owned text: arbitrary input
```

**A rendered type carries its kind beside its spelling. It is not text alone.** Two earlier
revisions got this wrong in opposite directions — one deferred it to Phase 5 as the design's
open question, the next declared it settled as plain owned text. The second reasoned that a
`TypeId` renders only against the module that owns it, that `ParseError` carries no brand and no
module, and that an error outlives the borrow that produced it. All true, and none of it implies
the field must be a bare string: it rules out a *handle*, not a *description*.

The middle option already exists and already crosses this exact boundary. `llvmkit_ir` exports
`TypeKindLabel` — brand-free, `Copy`, with its own `Display` — and `IrError` uses it today:

```rust
// IrError, shipped
#[error("type mismatch: expected {expected}, got {got}")]
TypeMismatch { expected: TypeKindLabel, got: TypeKindLabel },
```

So the sibling error type in the same workspace does the typed thing this document was about to
decline. A `ParseError` variant carrying a rendered type takes both halves:

```rust
#[error("'{name}' defined with type '{defined_spelling}' but expected '{expected_spelling}'")]
DefinedWithWrongType {
    name: Box<str>,
    defined: TypeKindLabel,            // matchable
    defined_spelling: Box<str>,        // contractual text: `<4 x i32>` prints byte-for-byte
    expected: TypeKindLabel,
    expected_spelling: Box<str>,
    loc: Span,
},
```

The spelling stays because the rendered text is upstream's and must not move. The label is what
makes the error answerable: `matches!(err, DefinedWithWrongType { defined: TypeKindLabel::Vector, .. })`
instead of sniffing the string for a leading `<`, which is precisely the weak runtime predicate
D6 forbids. This is the largest single group of owned-text fields, so getting it wrong would
have set the tone for the rest.

**Where a spelling has no kind to pair with, it stays text alone** — that is not a defect, just
the absence of a label worth carrying.

Owned text is spelled `Box<str>` rather than `String`: 16 bytes against 24, immutable, and it
says the payload is fixed. `Cow<'static, str>` cannot help:
input-derived text is never `'static`, so the `Cow` could only ever be its `Owned` arm — a
`String` with ceremony, which is why the present code reaches for `format!`. Borrowing properly
would mean `Cow<'a, str>` and a lifetime on `ParseError`, which is lifetime-free today,
`Clone + Eq + Hash`, and crosses the public API. That lifetime would infect every signature
touching an error to save one allocation on a path that runs once, at failure, after which the
parse ends. **Rejected: the cost is structural, the saving is not.**

**Phase 5's quality signal is a rule, not a number.** An earlier revision set the threshold at
"~19 owned-text fields, materially more means stop" — a number that was already exceeded by the
type renderings alone on the day it was written, so as a trip-wire it would have fired
immediately and told the reader nothing. The signal that survives measurement:

> Every owned-text field must be owned text **because the input is arbitrary** — a symbol name, a
> field name, a user-supplied spelling — and never because the value *had* a type that was
> inconvenient to carry. Type renderings are the one sanctioned exception, ruled above.

That is checkable field by field at review time, it does not decay, and it catches the failure
the number was aiming at: stringly-typing moved rather than removed. Phase 5 reports both counts
— arbitrary-input fields and type renderings — as measurements, with the command; neither is
predicted in advance.

### One source of truth

A declarative macro (`macro_rules!`, no proc-macro needed) declares the enum, its `thiserror`
renderings, and the `ALL_NULLARY` slice together, so the list cannot drift from the type:

```rust
parse_error_kinds! {
    ExpectedTypeKeyword      => "expected type",
    InvalidSymbolicAddrSpace => "invalid symbolic addrspace" { name: Box<str> },
}
```

## Migration — six phases, each independently green

`Display` output must stay **byte-identical** throughout. That makes the refactor
behaviour-preserving, which makes the existing suite the oracle.

**Phase 0 — inventory, `#[ignore]`d. This is the go/no-go.** Extract every `error(…)` /
`tokError(…)` literal from the vendored `LLParser.cpp` **and `LLLexer.cpp`**; compare against our
current literals in both directions; print the counts. This measures how many invented messages
exist **today**. If that number is near zero, the case rests entirely on preventing future ones —
decide on the measurement, not on the argument above.

**The census must not be written the obvious way, or it will undercount its own subject.** Four
requirements, each corresponding to a way the first draft got it wrong:

1. **Parse the file, not its lines.** A helper call whose literal sits on the following line is
   invisible to a line-at-a-time scan, and those calls are all diagnostics.
2. **Include every construction path**, not just `expected` / `message_at`: `self.message(`, the
   direct `ParseError::Expected { … }` / `ParseError::Message { … }` constructions, the
   `builder_err` family, and `format!` templates compared on the literal prefix before the first
   `{`. A first draft covered two of the six and would have reported a fraction of the surface as
   though it were the whole.
3. **Match exactly, in three buckets** — exact, prefix-of-upstream (i.e. parameterised), absent.
   A `contains` test passes an invented message whenever it happens to be a substring of any
   upstream literal, which is precisely the class the census exists to find.
4. **Bucket the `builder_err` family separately.** Every one of its labels is absent from
   upstream *by design* — upstream's IR constructors assert rather than diagnose — so folding
   them in would swamp the invented count that the go/no-go decision reads.

**Phase 1 — narrow the surface. Independent of everything else, and shippable alone.** Delete
the seven entry points in the table above, delete `Io` / `BrandInUse` / `BrandRetired`, delete
`module_name_for`, `branded_module` and `impl From<std::io::Error> for ParseError`, and change
`parse_dynamic`'s module name from `"asm"` to `"<string>"`. **`loc()` can then return the
location itself rather than an `Option` — the D1 fix lands here**, before any variant work. No
`ParseErrorKind` yet; `Expected` and `Message` are untouched, so the diff is small and the
existing suite is the whole oracle.

**`DiagLoc` is deleted in the same phase**, for the same reason and with a smaller blast radius
than it looks: no integration test names it
(`grep -rl "DiagLoc" crates/llvmkit-asmparser/tests/` returns nothing), so the change is
confined to the crate's `src/` and the corpus harness's `error.loc().span` reduces to
`error.loc`.

**And `read_to_owned` goes**, along with `lib.rs`'s `use std::io::{self, Read}`. It is a
four-line wrapper over `Read::read_to_end` living in the parser crate, and its only callers are
two examples that can spell `std::fs::read`. Afterwards the crate's `src/` names no `std::io`,
`std::fs` or `std::path` at all, which is upstream's own layering: `lib/AsmParser` reads no
files.

**Lock that structurally, not with a grep.** With `Io` and its `From` impl gone, `?` on an
`io::Result` inside a `ParseResult` function stops compiling — so a trybuild fixture proving
exactly that is what keeps I/O out of the parser's error type. It goes in `llvmkit-asmparser`
(`llvmkit-ir` cannot depend on the parser crate), mirroring `typestate_compile_fail.rs`'s
registration pattern, with `trybuild` added as a dev-dependency and the `.stderr` blessed only
on `+1.96.0`.

One call site moves — `parser_corpus.rs`, from `parse_assembly_file_with_config` to a read
plus `parse_assembly_with_config`. Four tests go with the functions they cover:
`parse_file_dynamic_returns_an_owned_module_named_after_the_file` and
`parse_branded_returns_the_named_brand` cover deleted conveniences and simply go. The other two
need their reasoning written down rather than being quietly dropped:

- `io_errors_keep_their_kind` (`parse_error.rs`) declares itself *"llvmkit-specific (no upstream
  counterpart)"* and asserts our own invention. It never noticed that our I/O message drops the
  filename `SMDiagnostic(Filename, …)` carries — an oracle that pins a behaviour nobody chose is
  worse than no oracle. Deleted with the variant.
- `parse_assembly_file_reads_file` (`parser_facade.rs`) cites *"Ports
  `Parser.cpp::parseAssemblyFile` file-loading wrapper shape"* — a port of an **API shape**, not
  of behaviour, so under D11 it was never a ported test. Deleted with the function.

Both `UPSTREAM.md` rows go in the same commit, and `upstream_registry_drift.rs` is what catches
it if they do not.

**Phase 1 also narrows `Module::branded`, and that is not scope creep.** Deleting
`branded_module` removes the *consumer* of a defect without removing the defect. `Module::branded`
returns `IrResult<Module<B, Unverified>>` — the 56-variant, `#[non_exhaustive]` `IrError` — for
an operation that structurally reports exactly `BrandInUse` or `BrandRetired`. Rust demands an
arm for the other 54, and `branded_module` filled it by stuffing a stringified `IrError` into
`ParseError::Io` with `ErrorKind::Other`, its own comment conceding *"the honest label for 'not
an I/O failure at all'"*. Ask what would have caught that: nothing did, and nothing would, because
an over-wide return type forces the same lie at whatever boundary consumes it next. A two-variant
`BrandError` in `llvmkit-ir` makes the catch-all arm unspellable — which is the fix, and it lands
with the deletion rather than after it. Same rule applies to `branded_once`.

`BrandError` lives in `crates/llvmkit-ir/src/error.rs`, beside `IrError` and `VerifierRule` —
that file already holds four top-level enums, so no new module. It can derive `Copy`, which
`IrError` cannot:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum BrandError {
    #[error("module brand `{brand}` is already held by a live module")]
    InUse { brand: &'static str },
    #[error("module brand `{brand}` was permanently retired")]
    Retired { brand: &'static str },
}
```

The change is bounded: `grep -rn "BrandInUse\|BrandRetired" crates/ --include=*.rs` at `84a66ea`
finds exactly **one** producer (`module.rs`, the two `Err(IrError::Brand*)` arms in the registry
claim), 4 doctests, and ~11 test assertions — plus the parser sites Phase 1 is already deleting.
The two variants then leave `IrError` entirely, since nothing constructs them any more; that is a
breaking change to a public enum and gets its own `CHANGELOG.md` line. (`IrError` carried
`#[non_exhaustive]` when this was written; it no longer does — see the 2026-09-05 note above.)

**`DataLayout::parse` has the same disease, and Phase 1 fixes it too.** `set_data_layout`
(`LLParser::set_data_layout`) matches on the returned `IrError` with a two-arm `match` whose second arm
is a catch-all rendering *differently* from the first — so one failure prints two ways depending
on which arm catches it. `DataLayout::parse` returns `IrResult<Self>` and constructs exactly one
variant (`grep -oE "IrError::[A-Za-z]+" data_layout.rs | sort -u` → `IrError::InvalidDataLayout`,
3 sites, at `84a66ea`). Narrowing its return type to that one error deletes the catch-all and
collapses the parser's `match` to a single `.map_err`. Three over-declarations found in one
session by asking the same question of each `IrResult`; there may be more, and Phase 1's
last step is to ask it of every `IrResult` the parser consumes.

**Phase 2 opens by giving `LexError` the same shape**, before `ParseErrorKind` exists — so the
kind enum is born holding `Lex(LexErrorKind)` rather than being retrofitted. `LexError` today is
eight variants that each carry a `span`, plus a hand-maintained `span()` match: the identical D1
shape this document is removing from `ParseError`, one layer down. Left alone, it would make
`ParseErrorKind::Lex(LexError)` store the location twice, once in the struct's `loc` and once
inside the kind.

```rust
pub struct LexError { pub kind: LexErrorKind, pub span: Span }
pub enum LexErrorKind { /* the same eight, without their spans */ }
```

**One duplicate diagnostic dies with it.** `ll_parser.rs`'s `map_lex_error` rewrites
`LexError::IntegerWidthOutOfRange` into a second, identically-rendered
`ParseError::IntegerWidthOutOfRange` — one diagnostic modelled twice, each a translation of the
other. Upstream emits this text from `LLLexer::LexIdentifier` alone. Keep the lexer's, delete
the parser's, and delete `map_lex_error`'s special arm.

The parser also builds that variant in its `PrimitiveTy::Integer` arm via
`.map_err(|_| …)`, discarding a structured `IrError` — the same erasure this document condemns
in `builder_err`, in the one place it is also unreachable: the lexer already rejects `iN` above
`MAX_INT_BITS`, `custom_width_int_type` range-checks the same bound, and the width is a
`NonZero`, so no input reaches it. It becomes an `unreachable!` naming that invariant, or the
width gets a type that carries the lexer's guarantee. It does not become a second variant, and
it does not keep silently swallowing the `IrError`.

This is the error *carrier*, not the lexer's error-**retention** model — divergences 101 and 32
stay untouched, and the commit should say so.

**Then the shape, no site changes.** Introduce `ParseError`/`ParseErrorKind` with `Expected` and
`Message` retained as transitional kind variants, neither type `#[non_exhaustive]`. The
`self.expected(…)`, `self.message(…)` and `self.message_at(…)` helpers absorb the change, so
most call sites do not move.

**Phase 3 — the `builder_err` family, and do it before the bulk.** 123 sites, two helpers, the
largest single win: a structured `IrError` stops being flattened into text. Land the variants
reusing `llvmkit_ir::Opcode` through `keyword()`, convert both helpers, and delete all four
hand-written opcode-to-keyword copies — remembering that the fourth also carries a membership
test that must survive as data. Those 123 sites stop constructing `Expected`, which shrinks
Phase 4 before it starts. **This phase is worth doing even if everything after it is abandoned.**

**Phase 4 — the `expected` half.** 105 distinct literals, zero `format!`, mechanical. Delete
`Expected` once it reaches zero uses. Split commits by parser routine, not by variant — a
several-hundred-site diff is unreviewable as one commit.

**Phase 5 — the `message` half**, which is both helpers: `message_at` and the `message` twin an
earlier revision's table omitted. Literals become nullary variants; the rest become variants with
typed fields per the table above. **No `Cow` survives.** Real defects surface here: a field the
message interpolates that the site does not cleanly have is a bug the string form was hiding.
Delete `Message`.

**Phase 6 — make the corpus oracle's *location* half total, and leave its text half alone.**

An earlier revision proposed widening the oracle from substring to kind-and-fields, citing a
`contains`-not-equality "hole" in `docs/future-work.md`. That reading inverts what the ledger
says. The entry records containment as **deliberate**: the pinned text is upstream's `FileCheck`
line, `FileCheck` itself matches substrings, and tightening to equality without an end anchor
would invent a stricter test than upstream runs. Only the *location* half was ever recorded as a
weakness.

The wrapper problem the widening was aiming at is closed from the other side, and more cheaply:
a wrapped rendering such as `expected udiv constexprs are no longer supported` does not appear
verbatim in `LLParser.cpp`, so the ours-to-upstream drift test rejects it as a nullary variant
before any fixture runs. What Phase 6 actually delivers:

- the location oracle becomes total — the harness's `unwrap_or_else(|| panic!(…))` on a missing
  location disappears, because after Phase 1 there is no missing location to handle;
- manifest rows *may* additionally pin a variant name, additively and optionally, for rows where
  the text alone is ambiguous.

**State the residual honestly rather than claiming closure.** Parameterised variants are checked
on their literal prefix, not their whole text, so an invention that keeps a genuine upstream
prefix and diverges after the first `{` still passes. And the whole `*Rejected` family is
llvmkit-authored text with no upstream counterpart at any granularity — upstream asserts where we
diagnose — so no oracle in this plan validates it. Those are the two places an invented message
can still hide, and the second is the larger of them.

### The prose is part of the API, and Phase 1 owns it

Deleting seven entry points leaves the documentation recommending them. This is not a tidy-up to
schedule afterwards — a reader following `AGENTS.md` today is told to reach for functions Phase 1
removes, so the sweep lands in the same phase as the deletion. Scope, at `4736ca5`:

```bash
git ls-files '*.md' | grep -vE '^CHANGELOG.md|^docs/design/' | xargs grep -nE \
  "parse_file_dynamic|parse_file_branded|parse_assembly_file|parse_branded\b|read_to_owned|IrError::Brand(InUse|Retired)|IrError::InvalidDataLayout|ParseError::Io|DiagLoc"
```

31 lines at `4736ca5`: `AGENTS.md` 9, this crate's `README.md` 6, `ROADMAP.md` 5,
`docs/inkwell-migration.md` 3, `README.md` 3, `docs/divergences.md` 2, `UPSTREAM.md` 2,
`docs/fixture-coverage.md` 1. The command must return nothing when the phase closes.

Driving it from `git ls-files` rather than a recursive `grep` is deliberate: the untracked
tooling directories carry their own copies of this prose, and a recursive search doubles the
apparent scope with files no reader will ever see. `CHANGELOG.md` is excluded because its
historical entries are history and are not edited; this document is excluded because it is
narrative and quotes the old names on purpose. The two `UPSTREAM.md` rows are the ones Phase 1
already deletes with the tests they trace.

**Two of those are wrong rather than merely stale, and need rewriting, not renaming.**

- **`AGENTS.md` § *Generic I/O via traits, not file paths*** prescribes `impl AsRef<Path>` entry
  points and closes with *"Default to streaming; load into a `Vec<u8>` only when the parser
  genuinely requires random access."* The lexer argument above establishes that the parser
  **always** requires the whole buffer, in both trees. The section is replaced: the parser takes
  `&[u8]`, the caller reads, there is no streaming form to default to, `parse_into` is the
  primitive and `parse_assembly_with_name` is the closure form.
- **`docs/divergences.md`'s entry on the missing `_with_config` twins** describes a gap belonging
  to two functions Phase 1 deletes. It closes *by deletion*, and the entry is amended to say so
  rather than left describing functions that no longer exist.

`AGENTS.md` also gains the public-fields carve-out for error types, ruled above.

## Testing

**Behaviour preservation.** 319 corpus `error=` rows and 216 test `to_string()` assertions are
the oracle. **Zero re-blesses are expected. Any re-bless is a finding** — it means the old text
was wrong, and it gets reported as a defect rather than absorbed.

**Drift, both directions**, mirroring `attribute_td_drift.rs`, which drives from the vendored
`Attributes.td` rather than from a Rust list:

- **ours → upstream** — every nullary variant's text appears verbatim in the vendored
  `LLParser.cpp` **or `LLLexer.cpp`**, and every parameterised variant's literal prefix — the
  text before its first `{` — appears as the prefix of some upstream literal. Catches invented
  messages of both shapes. Both files, because the lexer owns eight of the texts, `bitwidth for
  integer type out of range` among them; a test reading only `LLParser.cpp` would report all
  eight as inventions.
- **upstream → ours** — every `error`/`tokError` literal upstream is some variant's text, or is
  named in an explicit `NOT_YET_PORTED` list. Catches missing ones.
- **the list itself** — `NOT_YET_PORTED` is checked for stale entries, exactly as
  `not_yet_modeled_list_has_no_stale_entries` does today. A recorded gap that has silently closed
  is its own defect.

Two mechanics the first direction needs, or it will report false inventions:

- **Join adjacent C string literals** (`"a" "b"`) before matching. Where upstream instead
  concatenates a `Twine`, the variant is parameterised and is outside the nullary check by
  definition.
- **A deliberate difference from upstream carries its `docs/divergences.md` entry number** in an
  allowlist beside `NOT_YET_PORTED`, and that allowlist is checked for stale entries by the same
  mechanism. A text we chose to differ on is not an invention, but it must be a *recorded*
  choice, and the recording must be able to go stale loudly.

**Phase 0 will surface texts with no upstream counterpart in either direction** — `invalid slot
id: …` wraps our own `SlotAddError` and mirrors an upstream `assert` rather than a diagnostic.
Those are inventions today. Classifying each as keep-and-record or delete is Phase 0 output, not
something to settle here.

Under **D11** all three drift tests are llvmkit-specific — upstream has no error enum — so each
must say it has no upstream counterpart rather than cite a source, and carry an `UPSTREAM.md` row
marked as such. The trybuild fixture from Phase 1 is a fourth in the same category.

## Risks, and the case against

- **Phase 5 is where this can stall.** The signal is the owned-text rule above, applied field by
  field: a field that holds text because its value *had* a type that was awkward to carry has
  moved the stringly-typing rather than removed it. If that keeps happening, stop after Phase 3
  and record why rather than pushing through. Phase 3 is designed to be worth keeping alone.
- **Phase 1 introduces one new free-form carrier, and it is a deliberate exception.**
  `DataLayoutError { reason: String }` is the shape this document exists to delete. It lands
  anyway because it narrows a 56-variant return to a one-outcome type and kills a live catch-all
  that rendered the same failure two ways — and because upstream's `DataLayout::parse` is itself
  stringly, building its failures with `createStringError` throughout `DataLayout.cpp`, so an
  algebraic `reason` would be llvmkit inventing a taxonomy rather than porting one. Making it
  algebraic is a separate piece of work and gets a `docs/future-work.md` row carrying the command
  that measures its literals.
- **This closes no ledger entry directly.** Its value is making a *class* of defect unspellable
  and mechanically checkable, and it competes for time against entries that close a defect each.
- **Phase 1's public breaks are wider than seven functions.** The seven entry points go, and so
  do `read_to_owned`, `DiagLoc`, `ParseError`'s three non-diagnostic variants, `#[non_exhaustive]`
  on the parser's error vocabulary, and — through `llvmkit`'s blanket re-export of the crate —
  every one of those a second time under the umbrella. Each replacement is one line and the
  project is pre-1.0, but this is a real break and `CHANGELOG.md` carries it as one. The
  alternative — keeping the entry points and wrapping the error in a `ParseFailure` sum — was
  considered and rejected above: it keeps the coupling and adds a level.
- **`ParseError` and `ParseErrorKind` are one variant apart from being the same type.** If the
  variant count makes the struct wrapper feel like overhead, the check is whether any variant
  would want a `loc` that is not the parser's current position. None does today, which is what
  makes the split safe rather than merely tidy.

## Not in scope

`VerifierRule` is the model being copied, not changed. The lexer error-**retention** model
(divergences 101 and 32) touches the same file and must not be interleaved with this.

Two things an earlier revision put here are **in scope**, and are named so that "not in scope"
is not read as "already fine":

- **`LexError`'s carrier is in scope, at the top of Phase 2.** It was listed here as "already
  algebraic", which is true of its variants and false of its shape: every variant carries a
  `span` and a hand-written `span()` reads them back, which is the D1 defect this document is
  built to remove. Its *retention* model stays out, as above.
- **`Module::branded`'s over-declaration is in scope, as part of Phase 1.** The obvious reading
  of "delete `branded_module`" is that the problem left with it. It did not — an over-wide return
  type forces the same lie at whatever boundary consumes it next.

Also out of scope, and recorded rather than fixed: `DataLayoutError`'s free-form `reason`, per
the risk section above.
