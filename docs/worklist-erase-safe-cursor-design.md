# Erase-Safe Cursor + Mutation-Driven Worklist — Design

Date: 2026-07-10. Target: `crates/llvmkit-ir` (`worklist.rs` new, `pass_context.rs`,
`dce.rs`, `inst_simplify.rs`). Branch flow: `feature-N/*` → `dev`. This is the
Package 3 deferred perf item from `docs/pass-facing-type-safety.md` (items 1–2)
and `docs/future-work.md`, now designed in detail.

## Context

`DcePass` and `InstSimplifyPass` (`dce.rs`, `inst_simplify.rs`) both hand-roll the
same O(n²) loop: clone the block's instruction id list, scan for the first
instruction to change, mutate it, `return true`, and let `while iteration() {}`
**restart the entire scan** from the top. For N cascading changes that is N full
scans over N instructions — O(n²). The clone-and-restart exists only to survive
iterator invalidation under mutation and to reach the cascade fixpoint (erasing an
instruction can make its operands dead; folding one can make its users foldable).

The existing `iter::BlockCursor` does not solve this: it requires an `Unterminated`
block (so it cannot run over terminated function bodies), yields the raw
`Instruction<Attached>` handle rather than the pass-facing `NonTerminator`, and
does not id-liveness-check, so it is unsafe against cascade erases behind its back.

Goal: replace the restart-scan with an O(n) amortized **worklist** to fixpoint, plus
a general **erase-safe cursor** over terminated function bodies — as reusable
pass-authoring API (not a one-off DCE speedup) — while keeping the pass output
**byte-identical** on the existing `scalar_cleanup` corpus.

**Doctrine alignment** (from the pass-facing type-safety spec): *unrepresentable > witnessed > tested,
never trusted*. The worklist is kept consistent structurally (every mutation flows
through `FnPatch`, which maintains it), not by author discipline.

### Prior art (researched)

- **LLVM `InstructionWorklist`** (the InstCombine engine): a `SmallVector` +
  dedup map + deferred set. `eraseInstFromFunction` **removes the instruction from
  the worklist as part of erasing it** (no dangling), and mutation helpers push the
  cascade (`pushUsersToWorkList` on replacement, `handleUseCountDecrement` after a
  use drops). The *mutator*, not the pass, maintains worklist consistency.
- **MLIR `GreedyPatternRewriteDriver`**: a fully reusable driver that knows nothing
  about specific patterns; patterns must route every IR change through the
  `PatternRewriter`, and the driver listens to those notifications to keep its
  worklist consistent (erased ops pulled, created/modified ops re-added). Reusable
  driver + user bodies, coupled only by "all mutation flows through one channel."

**Key insight both designs share:** cascade direction is determined by the
*mutation*, not the *pass*. `erase` always means "my operands each lost a use →
maybe dead" (push operands); `replace_all_uses` always means "my users got a new
operand → maybe simplify" (push users). So the mutator's own methods can push the
correct cascade automatically — no per-pass knob, no `Action` enum, no way to
bypass — because we already have LLVM/MLIR's single mutation channel: `FnPatch`.

## Component 1 — `Worklist` (data structure), `worklist.rs`

A SetVector mirroring LLVM's `InstructionWorklist`:

- Backing store: `Vec<ValueId>` (LIFO stack) + `HashSet<ValueId>` (dedup).
- `push(id)` — no-op if already queued.
- `pop(&mut self, module) -> Option<NonTerminator>` — pops the next id, reconstructs
  its `NonTerminator`, and returns it; skips any id that no longer resolves to a
  non-terminator **instruction** (a cheap O(1) value-kind check, not an O(block)
  "is it still in its block" scan). `None` when drained. **`pop` releases the id
  from the dedup set** (not just the stack), so a later `push` can re-queue it —
  required for the cascade: an operand becomes dead only after its *last* user is
  erased, which may happen after that operand was already popped-and-found-live.
- `remove(id)` — pulls an id from both stack and set.
- `contains(id)`, `is_empty()`.

**Correctness against erased ids comes from remove-on-erase, not a liveness scan:**
`FnPatch::erase` calls `worklist.remove(id)` (Component 2), so an erased id never
surfaces from `pop`. The O(1) kind-check on `pop` is only a defensive guard against
a reused slot; a full "still in its block" scan would reintroduce O(n²) at seed
time and is deliberately avoided.

Stores bare `ValueId`s, so it is lifetime-free and unit-testable in isolation.
Drain order is LIFO but **irrelevant to output**: DCE's transitive-dead closure and
InstSimplify's fold-to-constant fixpoint are order-independent, so any drain order
yields the identical surviving instruction set — the basis of the byte-identical
guarantee.

## Component 2 — Mutation-driven integration on `FnPatch` (`pass_context.rs`)

`FnPatch` gains an opt-in worklist slot alongside the existing `dirty` cell:

```rust
worklist: RefCell<Option<Worklist>>,   // None (default) = today's behavior exactly
```

When a worklist is active, the mutator's **own** methods maintain it:

- `erase(x)` — after erasing, for each operand-defining instruction of `x`, `push`
  it (it lost a use → maybe dead); then `remove(x.id())` from the worklist.
  Operand ids come from the existing crate-internal `Instruction::operand_ids()`.
- `replace_all_uses(x, v)` — after RAUW, `push` each of `x`'s former users
  (`Value::users()`; they got a new operand → maybe simplify).

When `worklist` is `None`, both methods are byte-for-byte today's methods: no
overhead and no behavior change for non-worklist passes. Because every erase/RAUW
goes through `FnPatch`, a worklist pass cannot bypass maintenance (the MLIR
"one channel" property, enforced structurally by going through the mutator).

## Component 3 — Erase-safe function-body cursor (`pass_context.rs`)

```rust
impl FnPatch {
    /// Early-increment walk over every non-terminator of the function body, in
    /// program order. Snapshots each block's instruction ids up front and walks
    /// by index, so erasing the *yielded* instruction does not disturb the walk
    /// (its successor is already fixed — LLVM's `make_early_inc_range` idiom).
    /// Yields `NonTerminator` (so `erase` takes it directly); never yields a
    /// terminator. Cascades (erasing instructions *ahead* of the cursor) are the
    /// worklist's job, not the cursor's — a bare-cursor pass that needs them
    /// should drive a worklist instead.
    pub fn body_instructions(&self) -> impl Iterator<Item = NonTerminator<'ctx, B>> + '_;
}
```

Used to seed the worklist and independently usable for simple single-pass
transforms. Distinct from `iter::BlockCursor` (which is `Unterminated`-only and
yields `Instruction<Attached>`); that primitive is unchanged.

## Component 4 — `WorklistScope` driver handle (`worklist.rs`)

```rust
let wl = patch.worklist();          // activates + seeds all non-terminators
while let Some(inst) = wl.next() {  // liveness-safe pop
    // pass body mutates via `patch` directly; the mutation auto-cascades
}
// `wl` drop deactivates the worklist slot
```

`patch.worklist()` returns a `WorklistScope` borrowing `&patch`: on creation it
sets `patch.worklist = Some(Worklist::new())` and seeds it from
`body_instructions()`; `next()` pops the next live instruction; `Drop` restores
`patch.worklist = None`. The scope borrows `patch` shared, and the pass body calls
`patch.erase`/`patch.replace_all_uses` (also `&self`) — both shared borrows, so the
loop composes without a borrow conflict; `next()`'s pop and the mutators' pushes
touch the `RefCell` in disjoint, sequential borrows.

## Component 5 — Pass rewrites (`dce.rs`, `inst_simplify.rs`)

```rust
// DcePass::run
let patch = cx.mutate();
let wl = patch.worklist();
while let Some(inst) = wl.next() {
    if is_trivially_dead(&inst.as_view()) {
        patch.erase(inst);                       // auto-pushes operand-defs, self-removes
    }
}
Ok(patch.done())

// InstSimplifyPass::run
let patch = cx.mutate();
let dl = patch.function().module().data_layout().clone();
let wl = patch.worklist();
while let Some(inst) = wl.next() {
    let view = inst.as_view();
    if !view.to_erased().has_uses() { continue; }            // upstream !use_empty guard
    if let Some(c) = constant_fold_instruction(&view, &dl, None)? {
        patch.replace_all_uses(&view, c)?;                   // auto-pushes users
        if crate::dce::is_trivially_dead(&view) {
            patch.erase(inst);
        }
    }
}
Ok(patch.done())
```

`is_trivially_dead` and the fold logic are unchanged. The two passes cascade in
opposite directions (operands vs users) yet neither writes a `push` — the mutation
does it. Both drop the `instruction_ids()` clone-and-restart. The dirty-bit,
`done()`/floor reporting, and `PatchBody` rung are all unchanged, so
preservation/verification behavior is identical.

## Testing

- **Byte-identical corpus (primary regression lock):** the existing `scalar_cleanup`
  pipeline tests (`pipeline_basic.rs` / `single_pass_driver.rs`) must emit identical
  IR text before/after the rewrite — the guard that the worklist fixpoint equals the
  restart-scan fixpoint.
- **`Worklist` unit tests:** double-push → one entry (dedup); LIFO order; `remove`
  mid-drain; `pop` skipping an erased/dead id.
- **Cascade tests:** a dead chain `a→b→c` erased from a single seed (operand cascade
  reaches fixpoint in one drain, not N restarts); an InstSimplify chain where folding
  `a` re-queues its user `b` (user cascade); the InstSimplify ordered-atomic-load
  termination case (folded once, kept, not re-folded — the `!has_uses` guard).
- **Cursor test:** `body_instructions()` yields each non-terminator once, and
  erasing the *yielded* instruction mid-iteration does not disturb the walk (the
  early-increment property); it never yields the terminator.
- All five CI gates: `cargo fmt -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `RUSTDOCFLAGS=-D warnings cargo doc --no-deps
  --all-features -p llvmkit-ir`, `cargo test --workspace --all-features`, `cargo audit`.

## Sequencing

One branch `feature-10/worklist-cursor` off `dev`, landed in reviewable slices:

1. `Worklist` data structure + unit tests (`worklist.rs`, self-contained).
2. Erase-safe `body_instructions()` cursor on `FnPatch` + cursor test.
3. Mutation-driven integration: `worklist` slot on `FnPatch`, `erase`/`replace_all_uses`
   maintenance, `WorklistScope` + `patch.worklist()`, cascade unit tests.
4. Rewrite `DcePass` + `InstSimplifyPass`; byte-identical corpus check.

Each slice is independently green and committable; merge `--no-ff` to `dev`.

## Out of scope (considered, deferred)

- **Deferred/reverse-order worklist** (LLVM's `add` vs `push`, InstCombine's bottom-up
  seed): a visit-order optimization that does not change the fixpoint or the output;
  a single LIFO `push` suffices. Revisit only if a pass needs InstCombine-style
  ordering.
- **`handleUseCountDecrement` one-use refinement** (revisit only instructions with a
  single remaining use): an InstCombine micro-optimization; pushing all operand-defs
  is correct and simpler.
- **Generalizing `iter::BlockCursor`** to terminated blocks / `NonTerminator`:
  `body_instructions()` is the pass-facing cursor; the low-level `BlockCursor` keeps
  its `Unterminated`-construction niche.
- **Worklist for `ReshapeCfg`/`ModRewrite` passes:** these passes reshape the CFG or
  rewrite the module; the worklist here is scoped to in-block instruction churn
  (`PatchBody`). Extending it is future work with a consumer.
