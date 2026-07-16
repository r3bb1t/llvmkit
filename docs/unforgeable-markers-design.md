# Unforgeable markers — design

**Status:** approved design, 2026-07-16. Cycle 1.5 of the "No silent erasure" program.

## Context

A typed handle's marker is a claim about its runtime type: an `IntValue<'ctx, i32, B>`
asserts "this value's IR type is `i32`". The crate's guarantees rest on that claim being
true — `build_int_add(a, b)` emits `add i32` because the handles say `i32`.

**Today the claim is forgeable.** `IntValue::from_value_unchecked` (`value.rs:1093`,
`:1612`, `:1964` — three of the six are `pub(crate)`) attaches any marker to any value
without consulting its type. There are **~125 call sites**. A census classified them:

| Bucket | Count | |
|---|---|---|
| (A) provable by construction | ~104 | proof is real but **implicit** — 1-2 lines above the wrap |
| (B) genuinely unproven | ~11 | fixed in Cycle 1 (`bf57e17`) |
| (C) unclear / non-local | ~8 | proof spans files |

Cycle 1 closed bucket (B) and added runtime re-checks at six seams. This cycle attacks
the root: bucket (A)'s proofs are **sound but unstated**, and the forging door stays open
to every module in the crate.

**Why this matters beyond tidiness.** `docs/type-safety-vs-llvm.md` §4 claimed:

> "a custom folder cannot forge a wrong *width* either — the hook's signature pins it,
> and the builder accepts typed fold results with no runtime re-check."

Both halves were false, and the honest repair (commit `7d04cec`) **weakened the doc to
match the code**. The user's direction is the opposite: *fix without relaxing guarantees* —
make the code strong enough that the strong claim is true. Breaking changes and large
migrations are acceptable.

**Intended outcome:** a marker cannot be attached to a value whose type does not match it,
from anywhere in the crate; the ~100 implicit proofs become structural; and the strong
claim in §4 becomes true rather than documented-around.

## Decisions (locked before design)

1. **Runtime checks stay.** Cycle 1's six seams keep their checks as defense in depth.
   They are what catches `IntDyn` (whose marker names no width — inherent, not a weakness)
   and any future construction bug. The resulting doc claim is *stronger* than the original:
   "a folder cannot forge a wrong width — the constructor is unforgeable — **and** the
   builder re-checks anyway."
2. **Scope: the root fix only** (census rows 1-4). The other known gaps stay queued:
   `OperandWidthMismatch`'s two-shape overload, the 6 pre-existing
   `#[allow(clippy::type_complexity)]` on dev, literal-widening (task #72), and the strict
   cut (Cycle 2).
3. **No `TypeProof` object.** An `IntValue<W>` / `IntType<W>` already *is* the proof;
   boxing its `TypeId` adds indirection and a `debug_assert`. Rejected in favour of the
   typed-append family below.
4. **Zero user-facing change.** `from_value_unchecked` is `pub(crate)`; users cannot call
   it today and will not see this work. Public API and printed IR are unchanged.

## Architecture

**One idea:** the function that *attaches* a marker must also be the function that *chose
the type*. Then soundness is structural — there is no gap between the decision and the
claim for a reader (or a bug) to slip into.

```rust
// ir_builder.rs — the only places a marker is ever attached.

/// Append `kind` at `like`'s type; wrap the result as width-`W`.
/// Sound by construction: the instruction is created AT `like.ty()`, and
/// `like: IntValue<'ctx, W, B>` is W-typed, so the result is W-typed.
fn append_int_like<W: IntWidth>(
    &self, like: IntValue<'ctx, W, B>, kind: InstructionKindData, name: impl AsRef<str>,
) -> IntValue<'ctx, W, B>;

/// Append `kind` at `ty`; wrap as width-`W`.
/// Sound: `IntType<'ctx, W, B>` is W-typed by construction (`W::ir_type()` or a checked narrow).
fn append_int_at<W: IntWidth>(
    &self, ty: IntType<'ctx, W, B>, kind: InstructionKindData, name: impl AsRef<str>,
) -> IntValue<'ctx, W, B>;
```

Call sites lose the assertion entirely:

```rust
// before (ir_builder.rs:1523-1524)
let inst = self.append_instruction(lhs.ty().as_type().id(), kind_ctor(payload), name);
Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))

// after
Ok(self.append_int_like(lhs, kind_ctor(payload), name))
```

### The constructor family

Derived from the census's patterns, not invented:

| Constructor | Replaces (census pattern) | ~sites | Proof |
|---|---|---|---|
| `append_int_like<W>` / `append_fp_like<K>` | 1: result type taken from operand | 8 | appended at `like.ty()`; `like` is W-typed |
| `append_int_at<W>` / `append_fp_at<K>` | 2: result type from a typed dest param; 4: fixed `i1`; 5: load family (`W::ir_type`) | ~37 | appended at `ty`; `IntType<W>` is W-typed |
| `append_ptr` | 6: pointer-result (alloca/GEP) | 2 | `PointerValue` asserts only "is a pointer" |
| `IntValue::from_constant` (marker-preserving) | 12: `ConstantIntValue<W>` → `IntValue<W>` | 6 | marker is *carried*, not re-attached |
| (erased sites) | 9: `IntDyn`/`FloatDyn`/`ElemDyn` | ~14 | **not vacuous — see below** |

**Correction to the census on the erased markers.** The census called the
`IntDyn`/`FloatDyn` sites "vacuous (the marker asserts nothing)". That is true only of the
*width*: `IntDyn::static_bits()` is `None`, so a width re-check cannot fire. But
`IntValue<'ctx, IntDyn, B>` still claims **"is an integer, of some width"** — its
`TryFrom<Value>` (`value.rs:1621`) checks `matches!(ty.data(), TypeData::Integer { .. })`.
Wrapping a *pointer* as `IntValue<IntDyn>` is still a forged claim.

So these sites do **not** get a proof-free constructor. They route through the same
`append_int_like` / `append_int_at` family (which proves integer-ness structurally, since
the type came from a typed operand or an `IntType<IntDyn>`), and the width simply is not
part of what `IntDyn` claims. Do not introduce a `from_dyn` that skips proof.

Buckets already sound stay as they are: pattern 3/10 (fold results, now via
`Type::require_match`), pattern 11 (the five capability tokens), pattern 8 (`SelectNarrow`).

### The seal, and its honest limit

Goal: after migration, `from_value_unchecked` is unreachable outside the constructor family.

**Rust's floor is `pub(crate)`.** There is no `pub(in crate::ir_builder)` from `value.rs` —
the path in `pub(in ..)` must be an *ancestor* of the item's module, and `crate::ir_builder`
is not an ancestor of `crate::value`. So a *hard* seal requires **co-locating** the wrap
with its constructors (e.g. the constructors become associated fns in `value.rs` taking
`&IRBuilder`, letting `from_value_unchecked` drop to `pub(super)`).

**This is the one open implementation question.** Two acceptable outcomes:

- **Hard seal** — co-location is clean ⇒ `from_value_unchecked` becomes private to
  `value.rs`; forging outside the family is `E0624`, a compile error.
- **Audited seal** — co-location is contorted (the `IRBuilder` generics are heavy:
  `<'m,'ctx,B,F,S,R>`) ⇒ `from_value_unchecked` stays `pub(crate)`, but its callers drop
  from ~100 to ~8, each with a local total proof, and the definition's doc names the family
  as its only legitimate callers.

The implementer **must report which was achieved**, and must not describe an audited seal
as a compile-time guarantee. The audited outcome is still a large win; misreporting it is
the failure mode this whole program exists to prevent.

## Testing

- **Non-disruption is the primary signal.** This is a refactor of *how* a marker is
  attached, not *which* marker: every existing test must pass unchanged, and printed IR
  must stay byte-identical (guard: `asm_writer_basic`, `data_layout_round_trip`,
  `parser_corpus`, the four `*_example` suites which byte-lock output).
- **The seal, if hard:** a compile-fail fixture proving an in-crate module cannot call
  `from_value_unchecked` (`E0624`). If only the audited seal lands, **no fixture** — do not
  fake a compile-fail that proves something weaker than it appears.
- **The hostile-folder tests** (`hostile_native_typed_override_wrong_width_rejected_at_static_width`,
  `external_narrow_override_wrong_width_rejected_by_accept_folded_int`) must keep passing:
  the Cycle 1 runtime checks stay, so the in-crate hostile folder — if it can still be
  written — is still caught. If the hard seal makes that folder *unwritable*, the test must
  be retired with its `UPSTREAM.md` row, not deleted silently.
- Per-slice: the 5 gates. Baseline: **125 binaries ok, trybuild `2 of 78`** (the two
  environmental fixtures only, never re-blessed).

## Docs (part of the work, not an afterthought)

- `docs/type-safety-vs-llvm.md` §4 + its summary row: state the now-true strong claim.
  **Only after the seal actually lands, and only as strong as what landed.**
- `docs/future-work.md`: the "TypeId-carrying witness" entry (added `f0affa7`) describes
  the gap this cycle closes — retire or rewrite it. An open follow-up describing finished
  work is exactly the rot this program deletes.
- `CHANGELOG.md`: internal refactor; note the `IntDyn` residual honestly.

## Residuals (deliberate, stated)

- **`IntDyn` / `FloatDyn` stay vacuous.** The marker names no width/kind; that is what
  erasure means. `from_dyn` names this rather than hiding it.
- **Parsed `.ll` still narrows** at the boundary — correct, unavoidable.
- **~13 `CallInst`/`PhiInst` result accessors** (census pattern 13) have proofs spanning
  three files. Report rather than force; they may need their own slice.
- **Bucket (C), ~8 sites** (arena-lookup operand reads, `ssa_builder` `use_*_var`) rest on
  whole-construction invariants. Out of scope; leave and document.

## Risks

- **Ergonomic backlash inside the builder.** If threading `IntType<W>` through the append
  family proves noisy, the honest fix is a better constructor split — **not** restoring the
  unchecked wrap.
- **Blast radius ~125 sites.** Mechanical, but large. Slice by census pattern so each slice
  is reviewable, and keep every slice green.
- **Disk.** `target/` reached 25G and exhausted the 456G disk mid-Cycle-1, producing
  `E0786`/`STATUS_STACK_BUFFER_OVERRUN` (rustc aborting on its own truncated artifacts —
  not a code fault). Use `CARGO_INCREMENTAL=0`, and `cargo clean` when `df` drops under
  ~10G.
