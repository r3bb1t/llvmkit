# `ParseError` becomes an algebraic type

**Status:** design, approved for planning. Diagnostic-surface figures measured at `1034272`;
entry-point and divergence figures at `84a66ea`. Every figure carries the command that produced
it — re-derive before quoting, they move with the parser.

## The problem

`VerifierRule` is algebraic — 114 variants, no strings. `ParseError` is not: two
`Cow<'static, str>` carriers hold **512 of the parser's call sites**.

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
diagnostic this is down to a string. It is the opposite of opt-in — the default at 296 sites and
the easiest thing to reach for. `CLAUDE.md` is blunter: *"No silent erasure. Erasure is spelled
`as_dyn()` / `as_erased()`."* `Message` is silent erasure with no spelling at all.

**D5 — "Operand registration is structural: one exhaustive place per primitive."** The
diagnostic registry today is 512 scattered literals with no exhaustive match anywhere.

The cost is not theoretical. Every invented message this program deleted —
`expected valid shufflevector mask`, `a vector-of-pointers getelementptr base is not yet
supported`, `expected 'label'` at thirteen sites — was a `Cow` someone typed. **A `Cow` accepts
anything; a variant must be added, and adding one is visible in review.** Divergence 121 was
closed in a single sweep precisely because `VerifierRule` is an enum you can enumerate; there is
no equivalent lever for the parser today.

**And the erasure destroys structure, not just names.** The parser has an error-constructing
helper, `builder_err`, whose whole body is:

```rust
fn builder_err(&self, label: &str, e: IrError) -> ParseError {
    ParseError::Expected { expected: format!("valid {label}: {e}").into(), loc: … }
}
```

It is called **123 times**. Each call takes a fully structured `IrError` and flattens it into
text, so nothing downstream can `match` on why the builder refused — it can only parse the
rendering. That is the single largest cluster in the file and on its own it justifies the
change. Counting only direct `format!` sites misses it entirely; the helpers are where the mass
is.

**Worse, the labels duplicate an enum that already exists.** The 123 calls pass **87 distinct
literal labels** — `"add"`, `"alloca"`, `"call"`, `"shufflevector"` — and **54 of them are
variant names of `llvmkit_ir::Opcode` (73 variants)**. `ll_parser.rs` also carries two private
mnemonic enums whose 16 mnemonics are *all* among the 87, and exactly one call site uses one of
them (`builder_err(op.mnemonic(), e)`) while the rest hardcode the same strings. That is the
second-parallel-copy shape the porting rules treat as a defect in itself.

## Doctrine anchors

In the row form `docs/type-safety-vs-llvm.md` uses. The right-hand column is what this design
delivers; none of it exists today.

| Problem shape | Doctrine | Upstream LLVM C++ | llvmkit after this change |
| --- | --- | --- | --- |
| Invent a diagnostic upstream never emits | D3, D5 | `error(Loc, "any text you like")` — a `Twine` accepts anything | A `ParseErrorKind` variant must be added, and a drift test rejects text absent from the vendored `LLParser.cpp` |
| Ask why the builder refused a construct | D3 | n/a — upstream's parser calls IR constructors that assert | `BuilderRejected { context, source: IrError }` keeps the structured error instead of `format!`-ing it into 123 strings |
| Handle an error that has no source position | D1 | `SMDiagnostic` carries an *optional* `SMLoc`; the open-file failure uses the empty one | Unrepresentable: `loc: DiagLoc` is a field, not an `Option`, so a non-diagnostic cannot enter the type |
| Discover which entry points can fail how | Fallibility honesty | one `SMDiagnostic &Err` out-parameter for every failure kind | Every entry point returns `Result<T, ParseError>`, and each of the 17 can produce every variant of it |
| Match on a diagnostic rather than its rendering | D3, D6 | `Err.getMessage()` is a `std::string` | `match e.kind { … }`, exhaustive, with typed fields |

The last row is the one that pays for the rest: it is what let divergence 121 be closed in a
single sweep over `VerifierRule`, and what the parser has no equivalent of today.

## Measured surface

| what | count | command |
|---|---|---|
| `self.expected(…)` sites in `ll_parser.rs` | 163 | `grep -cE 'self\.expected\(' ll_parser.rs` |
| distinct `expected` literals, crate-wide | 105 | `grep -rhoE '\.expected\("[^"]*"' src/ \| sort -u \| wc -l` |
| `expected` sites using `format!` | **0** | `grep -oE 'self\.expected\(&?format!' ll_parser.rs \| wc -l` |
| distinct literal `message_at` texts | 100 | `grep -oE 'message_at\([a-z_]+, *"[^"]*"' ll_parser.rs \| grep -oE '"[^"]*"' \| sort -u \| wc -l` |
| `Message` sites using `format!` | 57 | `grep -cE 'message_at\([a-z_]+, *&?format!\|ParseError::Message *\{' ll_parser.rs` |
| corpus rows matching on text | 319 | `grep -c 'error=' parser_corpus_manifest.txt` |
| test assertions on rendered text | 216 | `grep -rc 'to_string()' tests/*.rs \| awk -F: '{s+=$2} END {print s}'` |

Target: **~205 nullary variants and ~57 with typed fields.** Not ~516 — that is the *upstream*
message count from the parity ledger, not ours. The `expected` half has **zero** `format!` uses,
so two thirds of the conversion is mechanical.

## Design

**One type. No sum, no wrapper, no optional location.**

```rust
/// A diagnostic at a source position. Every one has a location, so `loc` is not optional.
pub struct ParseError { pub kind: ParseErrorKind, pub loc: DiagLoc }

/// One variant per upstream diagnostic. No `Cow`, no free-form string.
pub enum ParseErrorKind { /* ~205 nullary, ~57 with typed fields */ }

pub type ParseResult<T> = Result<T, ParseError>;   // alias unchanged; the claim becomes true
```

`loc` moves out of the variants onto the struct, appearing **once** instead of ~205 times, and
the hand-maintained `loc() -> Option<DiagLoc>` accessor disappears — D1.

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
which is what `DiagLoc` is built on. `Lexer::new(src: &'src [u8])` already matches.

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
round-trips with a different `; ModuleID` line than upstream's. `grep -rniE
"moduleid|<string>|buffer identifier|module identifier" docs/divergences.md docs/future-work.md`
returns nothing, at `84a66ea`. Fixed by changing the constant, in the same commit that deletes
the file entry points; no ledger row is opened for a defect closed on arrival.

### Text carrier

Nullary variants carry their text through an exhaustive `message()`, as `VerifierRule` does — a
new variant without an arm is a compile error. Parameterised variants keep `thiserror`
interpolation, because upstream builds those mid-string
(`"invalid symbolic addrspace '" + AddrSpaceStr + "'"`), so the literal-at-head shape
`VerifierRule` uses cannot apply without changing the text, which is contractual.

**The drift test therefore covers the nullary variants exactly, and not the parameterised ones.**
That split is deliberate and, importantly, **structural rather than an exemption list**: a new
nullary variant is checked automatically, and you cannot opt out without adding a field — a
visible act in review. An enumerated exemption list would rot; this cannot.

### Field typing — store the thing, not its rendering

**D6: "modeled directly rather than flattened into weak runtime predicates."** A parameterised
variant's fields carry the *value*, not its rendering. Measuring what the 65 `format!` sites in
`ll_parser.rs` actually interpolate settles what each should become
(`grep -ohE 'format!\("[^"]*"' ll_parser.rs`, at `1034272`):

**Count the helpers, not just `format!`.** `builder_err` (113) and `builder_err_at` (10) build
their message internally, so a `format!` census misses 123 sites. Full picture:

| construction path | sites | becomes |
|---|---|---|
| `builder_err` / `builder_err_at` | **123** | `BuilderRejected { context, source: IrError }` |
| `expected` / `expected_at` | 177 | nullary variants |
| `message_at` / `message` | 167 | nullary variants, or fields below |
| direct `format!` | 65 | fields below |

and within the interpolating sites:

| interpolates | sites | becomes |
|---|---|---|
| `{e}` — **a nested error** | 24 direct, **123 via helper** | `source: IrError` |
| `{id}` / `{slot}` / `{index}` / `{next_id}` | 15 | `u32` / `u64` |
| `{kind}` / `{opcode}` / `{label}` / `{what}` / `{prefix}` | 6 | the enum it already is |
| `{result_ty}` / `{element_ty}` | 7 | a type handle where expressible, else owned text |
| `{name}` | 12 | owned text — genuinely arbitrary input |

**The 65 is an overcount, and Phase 5 must re-derive it.** Some `format!` sites are not
diagnostics at all: `format!("@{name}")` at `ll_parser.rs` :2140, :9027, :9057, :14688 and
`format!("%{id}")` in `LocalRef::display` build a *symbol display name* passed into
`check_valid_variable_type` / `check_resolved_global_type`. They are untouched by this refactor.
Filter them out before counting, or the ~19 prediction is measured against the wrong
denominator.

**The 24 nested-error sites are the most valuable part of this refactor, and they are worse than
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

The sites confirm it: they do not even render alike. `cannot create forward reference: {e}`
goes through `Message` (`ll_parser.rs:2155`) and `valid alias definition: {e}` through
`Expected` (`:7990`), so one renders with an `expected ` prefix and one without. A collapsed
variant would need a field to decide the *prefix*.

```rust
/// 54 sites, ONE shape — "the builder refused this instruction" — where the
/// opcode is genuinely data. Reuses `llvmkit_ir::Opcode` rather than minting a
/// parallel set: 54 of the 87 hand-typed labels are already Opcode names.
#[error("expected valid {opcode}: {source}")]
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

Output is byte-identical, structure intact, and the underlying `IrError` is finally
`match`-able. The cost of obeying the rule is **~35 variants instead of 1**, and `BuilderContext`
is deleted — it was the collapsed identity wearing an enum. **`ll_parser.rs`'s two private
mnemonic enums still go**: they are a third copy of the opcode naming, and one of them is used
at a single call site while 87 others hardcode its strings.

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

Owned text is therefore **~19 fields, not 57** — and `Box<str>` rather than `String`: 16 bytes
against 24, immutable, and it says the payload is fixed. `Cow<'static, str>` cannot help:
input-derived text is never `'static`, so the `Cow` could only ever be its `Owned` arm — a
`String` with ceremony, which is why the present code reaches for `format!`. Borrowing properly
would mean `Cow<'a, str>` and a lifetime on `ParseError`, which is lifetime-free today,
`Clone + Eq + Hash`, and crosses the public API. That lifetime would infect every signature
touching an error to save one allocation on a path that runs once, at failure, after which the
parse ends. **Rejected: the cost is structural, the saving is not.**

This gives Phase 5 its quality signal. **Count how many fields land on owned text.** The
measurement above predicts ~19; materially more means the stringly-typing was moved rather than
removed, and that is the point to stop and record why.

### One source of truth

A declarative macro (`macro_rules!`, no proc-macro needed) declares the enum, the exhaustive
`message()`, and the `ALL_NULLARY` slice together, so the list cannot drift from the type:

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
`tokError(…)` literal from the vendored `LLParser.cpp`; compare against our current literals in
both directions; print the counts. This measures how many invented messages exist **today**.
If that number is near zero, the case rests entirely on preventing future ones — decide on the
measurement, not on the argument above.

**Phase 1 — narrow the surface. Independent of everything else, and shippable alone.** Delete
the seven entry points in the table above, delete `Io` / `BrandInUse` / `BrandRetired`, delete
`module_name_for`, `branded_module` and `impl From<std::io::Error> for ParseError`, and change
`parse_dynamic`'s module name from `"asm"` to `"<string>"`. **`loc()` can then return `DiagLoc`
rather than `Option<DiagLoc>` — the D1 fix lands here**, before any variant work. No
`ParseErrorKind` yet; `Expected` and `Message` are untouched, so the diff is small and the
existing suite is the whole oracle.

One call site moves — `parser_corpus.rs:159`, from `parse_assembly_file_with_config` to a read
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
breaking change to a `#[non_exhaustive]` public enum and gets its own `CHANGELOG.md` line.

**`DataLayout::parse` has the same disease, and Phase 1 fixes it too.** `set_data_layout`
(`ll_parser.rs:3067`) matches on the returned `IrError` with a two-arm `match` whose second arm
is a catch-all rendering *differently* from the first — so one failure prints two ways depending
on which arm catches it. `DataLayout::parse` returns `IrResult<Self>` and constructs exactly one
variant (`grep -oE "IrError::[A-Za-z]+" data_layout.rs | sort -u` → `IrError::InvalidDataLayout`,
3 sites, at `84a66ea`). Narrowing its return type to that one error deletes the catch-all and
collapses the parser's `match` to a single `.map_err`. Three over-declarations found in one
session by asking the same question of each `IrResult`; there may be more, and Phase 1's
last step is to ask it of every `IrResult` the parser consumes.

**Phase 2 — the shape, no site changes.** Introduce `ParseError`/`ParseErrorKind` with
`Expected` and `Message` retained as transitional kind variants. The `self.expected(…)` and
`self.message_at(…)` helpers absorb the change, so most call sites do not move.

**Phase 3 — `builder_err`, and do it before the bulk.** 123 sites, one helper, the largest single
win: a structured `IrError` stops being flattened into text. Land `BuilderContext` reusing
`llvmkit_ir::Opcode`, convert both helpers, and delete `ll_parser.rs`'s two private mnemonic
enums. Those 123 sites stop constructing `Expected`, which shrinks Phase 4 before it starts.
**This phase is worth doing even if everything after it is abandoned.**

**Phase 4 — the `expected` half.** 105 literals, zero `format!`, mechanical. Delete `Expected`
once it reaches zero uses. Split commits by parser routine, not by variant — a 500-site diff is
unreviewable as one commit.

**Phase 5 — the `message` half.** 100 literals become nullary variants; the rest become variants
with typed fields per the table above. **No `Cow` survives.** Real defects surface here: a field
the message interpolates that the site does not cleanly have is a bug the string form was hiding.
Delete `Message`.

**Phase 6 — widen the corpus oracle** to match kind-and-fields rather than substring, closing the
`contains`-not-equality hole recorded in `docs/future-work.md`.

## Testing

**Behaviour preservation.** 319 corpus `error=` rows and 216 test `to_string()` assertions are
the oracle. **Zero re-blesses are expected. Any re-bless is a finding** — it means the old text
was wrong, and it gets reported as a defect rather than absorbed.

**Drift, both directions**, mirroring `attribute_td_drift.rs`, which drives from the vendored
`Attributes.td` rather than from a Rust list:

- **ours → upstream** — every nullary variant's text appears verbatim in the vendored
  `LLParser.cpp`. Catches invented messages.
- **upstream → ours** — every `error`/`tokError` literal upstream is some variant's text, or is
  named in an explicit `NOT_YET_PORTED` list. Catches missing ones.
- **the list itself** — `NOT_YET_PORTED` is checked for stale entries, exactly as
  `not_yet_modeled_list_has_no_stale_entries` does today. A recorded gap that has silently closed
  is its own defect.

Under **D11** all three are llvmkit-specific — upstream has no error enum — so each must say it
has no upstream counterpart rather than cite a source, and carry an `UPSTREAM.md` row marked as
such.

## Risks, and the case against

- **Phase 5 is where this can stall**, and the field-typing table is how you will know. The
  measurement predicts ~19 owned-text fields of 65 interpolations; materially more means the
  stringly-typing was moved rather than removed. Count them, and if the number runs away, stop
  after Phase 3 and record why rather than pushing through.
- **The 7 type renderings are the genuinely uncertain ones.** A `TypeId` needs a module to
  render, and an error can outlive the borrow that produced it. If they cannot be typed, they
  fall back to owned text and the ~19 grows. Settle those first in Phase 5 — they decide whether
  the phase is worth starting.
- **This closes no ledger entry directly.** It competes with ~11 remaining tractable entries. Its
  value is making a *class* of defect unspellable and mechanically checkable.
- **Phase 1 removes seven public functions**, which is the only user-visible break in the plan.
  Pre-1.0 and each replacement is one line, but it is a real break and belongs in `CHANGELOG.md`
  as one. The alternative — keeping them and wrapping the error in a `ParseFailure` sum — was
  considered and rejected above: it keeps the coupling and adds a level.
- **`ParseError` and `ParseErrorKind` are one variant apart from being the same type.** If
  `~205 + ~57` variants make the struct wrapper feel like overhead, the check is whether any
  variant would want a `loc` that is not the parser's current position. None does today, which
  is what makes the split safe rather than merely tidy.

## Not in scope

`LexError` is already algebraic. `VerifierRule` is the model being copied, not changed. The lexer
error-**retention** model (divergences 101 and 32) touches the same file and must not be
interleaved with this.

**`Module::branded`'s over-declaration is in scope, and is part of Phase 1** — see there for
why. It is named here only because the obvious reading of "delete `branded_module`" is that the
problem left with it, and it did not.
