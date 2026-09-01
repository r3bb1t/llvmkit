# `ParseError` becomes an algebraic type

**Status:** design, approved for planning. Measured at `1034272`; every figure carries the
command that produced it. Re-derive before quoting — they move with the parser.

## The problem

`VerifierRule` is algebraic — 114 variants, no strings. `ParseError` is not: two
`Cow<'static, str>` carriers hold **512 of the parser's call sites**.

```rust
#[error("expected {expected}")]
Expected { expected: Cow<'static, str>, loc: DiagLoc },

#[error("{message}")]
Message { message: Cow<'static, str>, loc: DiagLoc },
```

This violates two doctrine rules.

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

```rust
/// A diagnostic at a source position. Every one has a location, so `loc` is not optional.
pub struct ParseError { pub kind: ParseErrorKind, pub loc: DiagLoc }

/// One variant per upstream diagnostic. No `Cow`, no free-form string.
pub enum ParseErrorKind { /* ~205 nullary, ~57 with typed fields */ }

/// Failures that are not diagnostics and have no source position.
pub enum ParseFailure {
    Parse(ParseError),
    Io { kind: std::io::ErrorKind, message: String },
    BrandInUse { brand: &'static str },
    BrandRetired { brand: &'static str },
}
```

`loc` moves out of the variants and onto the struct, appearing **once** instead of ~205 times,
and the hand-maintained `loc() -> Option<DiagLoc>` accessor disappears. `Io` and `BrandInUse`
are not diagnostics — they have no location, which is why that `Option` exists today — so they
move to `ParseFailure` rather than being given a fake one. This mirrors upstream, where
`LLParser::error` always takes a `LocTy` and I/O failure lives in the `MemoryBuffer` layer,
outside the parser's error type.

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
variant's fields carry the *value*, not its text:

```rust
ExpectedKeyword { keyword: TokenKind },                  // not String — the token is an enum
MetadataFieldValueTooLarge { value: u64, max: u64 },     // not String — they are numbers
InvalidSymbolicAddrSpace { name: String },               // String is right: arbitrary input text
```

`String` is correct **only** when the value is arbitrary text from the source; `&'static str`
when it is a compile-time constant; otherwise the typed value.

Note `Cow<'static, str>` is not an option for input-derived text: that text is never `'static`,
so the `Cow` could only ever be its `Owned` arm — a `String` with extra ceremony, which is why
the present code reaches for `format!`. Borrowing the source properly would mean
`Cow<'a, str>` and a lifetime on `ParseError`, which is currently lifetime-free, `Clone + Eq +
Hash`, and crossing the public API. That lifetime would infect every signature that touches an
error, to save one allocation on a path that runs once, at failure, after which the parse ends.
**Rejected: the cost is structural and the saving is not.**

This gives Phase 3 its quality signal. **Count how many of the 57 land on `String`.** A high
count means the stringly-typing was moved rather than removed, and that is the point to stop and
record why — not a reason to push through.

### One source of truth

A declarative macro (`macro_rules!`, no proc-macro needed) declares the enum, the exhaustive
`message()`, and the `ALL_NULLARY` slice together, so the list cannot drift from the type:

```rust
parse_error_kinds! {
    ExpectedTypeKeyword      => "expected type",
    InvalidSymbolicAddrSpace => "invalid symbolic addrspace" { name: String },
}
```

## Migration — four phases, each independently green

`Display` output must stay **byte-identical** throughout. That makes the refactor
behaviour-preserving, which makes the existing suite the oracle.

**Phase 0 — inventory, `#[ignore]`d. This is the go/no-go.** Extract every `error(…)` /
`tokError(…)` literal from the vendored `LLParser.cpp`; compare against our current literals in
both directions; print the counts. This measures how many invented messages exist **today**.
If that number is near zero, the case rests entirely on preventing future ones — decide on the
measurement, not on the argument above.

**Phase 1 — the shape, no site changes.** Introduce `ParseError`/`ParseErrorKind`/`ParseFailure`
with `Expected` and `Message` retained as transitional kind variants. The `self.expected(…)` and
`self.message_at(…)` helpers absorb the change, so most call sites do not move.

**Phase 2 — the `expected` half.** 105 literals, 163 sites, zero `format!`. Mechanical. Delete
`Expected` once it reaches zero uses. Split commits by parser routine, not by variant — a
512-site diff is unreviewable as one commit.

**Phase 3 — the `Message` half.** 100 literals become nullary variants; **57 become variants with
typed fields — no `Cow` survives.** This is where real defects surface: a field the message
interpolates that the site does not cleanly have is a bug the string form was hiding. Delete
`Message`.

**Phase 4 — widen the corpus oracle** to match kind-and-fields rather than substring, closing the
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

- **Phase 3 is where this can stall**, and the field-typing rule above is how you will know.
  If many of the 57 sites interpolate something with no typed form available, those variants
  grow a `String` and the stringly-typing has been *moved*, not removed. Count them; if it is
  most, stop after Phase 2 and record why rather than pushing through.
- **This closes no ledger entry directly.** It competes with ~11 remaining tractable entries. Its
  value is making a *class* of defect unspellable and mechanically checkable.
- **The two-level error type** (`ParseFailure` wrapping `ParseError`) is a visible change to
  every public entry point's signature. That is the price of not inventing fake locations.

## Not in scope

`LexError` is already algebraic. `VerifierRule` is the model being copied, not changed. The lexer
error-**retention** model (divergences 101 and 32) touches the same file and must not be
interleaved with this.
