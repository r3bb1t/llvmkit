# Leftover work from the handle-and-error API rewrite

What the handle-safety and error-surface rewrite left undone, and why each piece
stopped where it did. Every item below is a **known** gap — something the rewrite
reached, understood, and deliberately did not close — not a survey of unexplored
ground.

**This is not the general backlog.** `docs/future-work.md` is where
known-missing *features* live; `docs/divergences.md` is where observable
behavioural differences from the vendored tree live. This file is narrower: it
is the residue of one program, and every entry here exists because that
program's own changes created or exposed it. Entries graduate out of this file
when they are closed, not when they are re-filed.

Measured at **`02ae334`** — the tree that was `4da9711` plus the placement-witness
slice, which landed as `02ae334` while this file was being written. Every count
below names the command that produced it; the table at the end collects them.
Re-derive before quoting — the repo's docs are not checked by CI, and these
numbers move with the code.

**Each entry was re-checked against the tree on 2026-09-20, after the first
draft.** Nothing was fully closed then, but four entries were wrong about their
own size or about what already exists: §2 (a `Detached` handle *was*
obtainable), §3 and §4 (two method names were already taken by different
things), §8 (five of seven readers were fixed by this program), §9
(`value_from_slot` has seven copies, not the two on record). The corrections all
run the same direction — the residue is smaller than the draft implied in two
places and larger in one — which is the reason to re-derive rather than trust
this file. §2–§4 have since closed and collapsed into one note; §8 and §9 still
say so inline.

---

## What the rewrite changed, so the leftovers have a frame

Three things, across the `llvmkit-ir` public surface:

1. **A slot leaves a handle or id only through a checked door.** `ValueSlot`,
   `TypeSlot` and `MetadataSlot` became crate-private; `slot_in(owner)` compares
   modules and refuses another module's with a `Foreign*` error, and
   `slot_trusting_same_module()` is the narrow unchecked twin for reads that
   provably stay in one module. `BlockId` has no unchecked door at all — its one
   exception is the named `slot_unchecked_at_marked_boundary()`, counted by
   `tests/boundary_door_drift.rs`.
2. **An instruction in no block says so.** `InstructionData.parent` became
   `Cell<Option<ValueSlot>>`, `detach_from_parent` / `erase_from_parent` clear
   it (upstream's `Instruction::removeFromParent` sets `Parent = nullptr`), and
   the entries that need a block refuse one that has none. The slice `02ae334`
   goes further: a new `PlacedInstruction<'ctx, B>` witness makes "never in a
   block" unspellable at those entries (D1).
3. **Errors narrowed to what their operations can produce.** `BrandError`,
   `DataLayoutError`, `Blame`, per-site variants for four llvmkit-bug sites, and
   `#[non_exhaustive]` deleted workspace-wide so a downstream `match` is
   exhaustive.

The program is the SDD plan `docs/superpowers/plans/2026-09-06-error-surface-cleanup.md`
(gitignored — see §10). Its Task 24 landed as 21 commits, `17b293e` through
`4da9711`; the list is in that plan's `HANDOFF.md`.

---

## 1. The placement witness proves *was placed*, not *is placed*

**The hole the rewrite knowingly shipped.** `PlacedInstruction` pairs an
instruction with the block it was in *at the moment the witness was minted*:

```rust
pub struct PlacedInstruction<'ctx, B: ModuleBrand> {
    view:  InstructionView<'ctx, B>,
    block: BlockId<Dyn, B>,
}
```

There is no public constructor. It is minted by `InstructionView::placed`
(checked, returns `Option`), `Instruction::placed` on the `Attached` typestate
(total, because that state is only minted in a block), or
`BasicBlock::placed_instructions` / `BasicBlockView::placed_instructions`, which
need no check because the block is the one being walked (added after this
section was written, closing the third mint the design called for). It is
`Copy`, from the default `#[derive(Branded)]` set.

But llvmkit mutates through `&Module` with interior mutability, so nothing stops
a detach between the mint and the use:

```rust
let placed = view.placed().ok_or(IrError::InstructionHasNoParent)?;
some_instruction.detach_from_parent(&module)?;   // `placed` is now stale
builder.position_before(placed)?;                // must still refuse
```

So the five entries — `IrBuilder::position_before`, `Instruction::move_before` /
`move_after` / `insert_before` / `insert_after` — **re-read the current block**
rather than trusting the witness's, and return `IrError::InstructionHasNoParent`
for the stale case. That is now the variant's only reachable path at those
entries.

A mutation check proved the re-read is load-bearing: making `position_before`
trust the witness's stored block failed
`mutation_basic::position_before_refuses_an_anchor_in_no_block_without_mutating`.

**What closing it would take:** exclusive access to the function's layout for
the lifetime of the witness, so no detach can interleave. That is a different
design from the one shipped — it changes who may hold a `&Module` while a
witness is alive — and it was not attempted. Until then the runtime refusal is
the guarantee, and the type is a narrowing, not a proof. The sketch of that
design, its blast radius and its in-crate precedent are in
[`future-work.md`](future-work.md) under "A placement witness proves *was
placed*, not *is placed*"; it is recorded there, not scheduled.

---

## 2–4. Closed: detached creation, the call-site copy and its setters

These three sections recorded what stood between llvmkit and porting
`InstructionsTest.AlterCallBundles` and `AlterInvokeBundles` whole. Both now
port whole (`crates/llvmkit-ir/tests/builder_call.rs`), so the sections
graduated out; `CHANGELOG.md`'s *Added — call sites created in no block, and
copied with other bundles* has the shipped surface. What each one recorded,
and where it went:

- **§2, detached creation.** The primitive is `create_detached_instruction`
  (`instruction.rs`), over a `push_instruction` that `IrBuilder::append_instruction`
  now shares, so a detached and an attached instruction register their uses
  through one routine. The three constructors are `CallInst::create_detached`,
  `InvokeInst::create_detached` and `BasicBlock::create_orphan`.
- **§3, the copy.** The sealed `CallBase` trait and `with_operand_bundles` on
  all three call sites, as sketched — except for the result, which is a
  `DetachedCallSite`, the linear `Instruction<Detached>` beside the typed view,
  so a copy is read without narrowing an erased handle at run time. The name
  collision this section warned about is gone: the payload-side
  `with_operand_bundles` was deleted when the bundles left `CallAttributeData`.
- **§4, the setters.** `CallInst::set_tail_call_kind`, and `set_attributes` on
  all three. The interior mutability is a `Cell` per field, not the `RefCell`
  recommended here. A call's attribute list is interned in the module, as
  upstream's `AttributeList` is uniqued in the context, and the payload holds
  the slot, so every reader keeps a plain `&'ctx` borrow and no setter can meet
  an outstanding one. Ported over the old payload, `setAttributes` would also
  have overwritten the fast-math flags, which lived inside `CallAttributeData`
  until the same change moved them out. The debug location needed no setter:
  llvmkit's is the `!dbg` attachment, which `set_metadata` already writes —
  `ValueData.debug_loc` is a reserved slot no code fills (`rg -n "debug_loc:
  Some|\.debug_loc =" crates/` finds nothing).

One piece of §2 stays open and moved to `docs/future-work.md`: an orphan block
cannot join a function, because `BasicBlock::insertInto` is not ported.

---

## 5. A mutation token does not have to belong to the module it unlocks

**The largest open item, and it needs a design ruling before any code.**

Methods that require mutation capability take the token and then ignore it:

```rust
pub fn some_mutator(&self, _module: &'ctx Module<B, Unverified>, …) -> IrResult<()>
//                         ^ never read
```

The token proves *a* module is unverified. It does not prove it is *this* module.
Under two distinct brands the types differ, so D7 closes it — but every
`Module::dynamic` is `DynBrand`, and there the type check cannot apply.

The program's own statement of the consequence, **relayed, not re-derived here**:
another `DynBrand` module's token unlocks a verified module, and nothing refuses
it. Reproducing it would mean constructing a throwaway
`Module<DynBrand, Unverified>` and passing its token to a mutator reached from a
different module's handle. **Write that reproduction first** — the fix's shape
depends on whether the hole is reachable through a public path or only through a
crate-internal one.

Nothing pins it today:
`rg -n 'throwaway|foreign token|other module.*token|another module.*token' crates/*/tests/`
returns one hit, a doc comment in `return_marker_mismatch_diagnostic.rs` about an
unrelated throwaway module. So the 49 cross-module tests in
`cross_module_handles.rs` cover foreign *handles* and not the foreign *token*.

The family is larger than the program's ledger recorded. At `02ae334`:

```
rg -o '_module\w*\s*:\s*&[^,)]*' crates/ --no-filename | sort | uniq -c | sort -rn
     65 _module: &'ctx Module<B…
      8 _module_token: &'ctx Module<B…
      1 _module_token: &Module<B…
```

**74 sites, not 65.** The plan's Task 25 names 65, which is the `_module:`
spelling alone; the nine `_module_token:` sites are the same defect under a
different parameter name and must not be missed by a search written from the
smaller figure.

The ruling owed: whether the token becomes a checked argument (compare
`ModuleId`, return `ForeignValueId`), or whether the capability is restructured
so an unrelated token is unspellable. Task 25 stops at step 1 pending that
answer.

---

## 6. Twenty-seven infallible boundary sites still admit a foreign slot unchecked

```
rg -c 'boundary \(F1\)' crates/ | awk -F: '{s+=$2} END {print s}'   →  27
```

F1 marks an infallible entry or a value-producing builder that reads a caller's
slot without comparing modules, because its signature has no error channel to
report the refusal through. Each site is annotated, and
`tests/boundary_door_drift.rs` fails when a call sits under no marker — so the
set is bounded and enumerable, which is the property the rewrite bought.

Closing one means giving the entry a `Result`, which is a breaking signature
change per site. The program's Task 26 is done exactly when that count is zero.

---

## 7. Sixty-six read-only analyses cross modules by construction

```
rg -c 'boundary \(F2\)' crates/ | awk -F: '{s+=$2} END {print s}'   →  66
```

F2 is the read-only twin of F1: an analysis that reads a caller's slot without
checking, because analyses do not mutate and historically did not refuse.

This one is a **policy question, not a mechanical sweep**. Making 66 read-only
analyses fallible is a large ergonomic cost paid for a class of mistake that
cannot corrupt IR — it can only produce a wrong answer about the wrong module.
The rewrite left the decision to the user rather than assuming it.

Downstream of the answer: the two lookup panics in `llvm_context.rs` can be
proven dead once F2 is resolved, and not before.

---

## 8. Two attribute *payload* readers still ignore `#N` groups

**Mostly closed — read `docs/divergences.md` D9, not this section.** The
underlying model gap (attribute groups are never merged) predates this program
and belongs to the ledger. What this program did was narrow it, and the
narrowing is what makes the residue small enough to name:

- **Fixed here** (`097e405`, `62d43b2`, Task 24 fix round 3): the three
  `CallBase::hasFnAttr` copies and the two `Function::hasFnAttribute` copies —
  `speculation.rs::callee_is_speculatable` and
  `assumptions.rs::enclosing_function_has_attribute` — were replaced by one port
  each that does consult the groups (`instr_types::call_site_has_fn_attr`,
  `FunctionValue::has_fn_attribute`).
- **Still open**, per D9's own list, because each reads a *payload* and so needs
  a port of `getFnAttribute` rather than of `hasFnAttr`:
  `speculation.rs::call_site_memory_effects` (`CallBase::getMemoryEffects`) and
  `value_tracking.rs::function_vscale_range` (`llvm::getVScaleRange`). Both fall
  back to upstream's own missing-attribute answer, so an analysis learns less,
  never something false.

`docs/divergences.md` entry **134** (`CallBase::getCalledFunction` ported without
its function-type check, four sites) was recorded by this program and is
untouched; it is a ledger item, not API-rewrite residue, and is named here only
so a reader of this file does not conclude the rewrite closed it.

---

## 9. Parity follow-ups noticed in passing

Small, independent, each found while doing something else:

- **`FnReshape::split_block_before` is missing.** `BasicBlock::split_before`
  exists (`basic_block.rs`) and `FnReshape::split_block` exists
  (`pass_context.rs`), but there is no `split_block_before` wrapper, so a pass
  cannot reach the primitive: `rg -n 'fn split_' crates/llvmkit-ir/src/pass_context.rs`
  returns `split_block` only.

- **`value_from_slot` has seven copies, not two.** The program's handoff records
  "the duplicate in `assumptions.rs` / `implied_conditions.rs`", which
  undercounts it by five:

  ```
  rg -n 'fn value_from_slot' crates/llvmkit-ir/src/
    assumptions.rs · fp_predicate.rs · implied_conditions.rs · known_fp_class.rs
    select_pattern.rs · speculation.rs · value_tracking.rs
  ```

  Six are private; `value_tracking.rs`'s is `pub(crate)` and is the obvious home.
  A fix scoped from the recorded figure would leave five copies standing, which
  is why the number is written out here.

- **`BasicBlock::splice_into` carries an unverified "Mirrors" claim.** Its
  rustdoc still reads *"Mirrors `BasicBlock::splice` in `lib/IR/BasicBlock.cpp`"*.
  The citation has not been checked against the `.cpp` — either it holds and
  stays, or it does not and a divergence is recorded. **Still unread**; this is a
  premise, not a finding.

---

## 10. The rewrite's design rationale is untracked

`docs/superpowers/` is gitignored (`.gitignore:6`), and the specs that argued
these designs live there:

```
ls docs/superpowers/specs/ | wc -l   →  15
ls docs/design/                      →  5 files
```

Fifteen design specs — including `2026-09-20-placement-witness-design.md`, which
is the argument for §1 — exist only on the machine that wrote them. Five
tracked design docs sit in `docs/design/`.

That asymmetry is itself a leftover. A design that a future session must
re-derive from the code is a design that will be re-litigated, and the
placement-witness one in particular encodes a user ruling ("do the first
approach for type safety") that the code cannot state. Either the durable
reasoning moves into `docs/design/`, or these sections carry it — which is part
of why §1 is written out in full above rather than pointing at the spec.

---

## Where else to look

| Question | File |
|---|---|
| What feature is known-missing, and why was it deferred? | `docs/future-work.md` |
| Where does llvmkit's observable behaviour differ from LLVM? | `docs/divergences.md` |
| Which `test/Assembler` fixture is blocked, and on what? | `docs/fixture-coverage.md` |
| Which upstream test does a given llvmkit test port? | `UPSTREAM.md` |
| What does the handle model actually guarantee? | `CLAUDE.md` § *The handle model*, `README.md` (D1–D11 prose) |

---

## How every number here was derived

All at `02ae334`. None of these is checked by CI; re-run rather than quote.

| Figure | Command |
|---|---|
| F1 boundary markers = **27** (§6) | `rg -c 'boundary \(F1\)' crates/ \| awk -F: '{s+=$2} END {print s}'` |
| F2 boundary markers = **66** (§7) | `rg -c 'boundary \(F2\)' crates/ \| awk -F: '{s+=$2} END {print s}'` |
| Unused module tokens = **74** in three spellings (§5) | `rg -o '_module\w*\s*:\s*&[^,)]*' crates/ --no-filename \| sort \| uniq -c \| sort -rn` |
| No test pins the foreign token (§5) | `rg -n 'throwaway\|foreign token\|other module.*token\|another module.*token' crates/*/tests/` → 1 unrelated doc-comment hit |
| `PlacedInstruction` is `Copy` (§1) | default `#[derive(Branded)]` set is `Clone, Copy, Debug, PartialEq, Eq, Hash` — `crates/llvmkit-macros/src/lib.rs`, `derive_branded` rustdoc |
| `FnReshape::split_block_before` is absent (§9) | `rg -n 'fn split_' crates/llvmkit-ir/src/pass_context.rs` → `split_block` only |
| `value_from_slot` has **7** copies (§9) | `rg -n 'fn value_from_slot' crates/llvmkit-ir/src/` |
| Untracked specs = **15**, tracked design docs = **5** (§10) | `ls docs/superpowers/specs/ \| wc -l`; `ls docs/design/` |
| `docs/superpowers/` is gitignored (§10) | `git check-ignore -v docs/superpowers/specs/2026-09-20-placement-witness-design.md` |

Every row above was run with `rg` and direct file reads. **No LSP was available in the session
that wrote this** — `/reload-plugins` reported `0 plugin LSP servers` — so none of these is
backed by rust-analyzer's call graph. Each is an existence-or-count question, which text search
answers exactly; a claim about *reachability* (which §5 and §7 both turn on) is not, and is
marked where it appears.

Two things in this document come from the program's own `HANDOFF.md` rather
than from a command run here, and each is marked where it appears:

- the 21-commit span of Task 24 (§ frame) — re-derivable from `git log`;
- the statement that a foreign `DynBrand` token unlocks a verified module (§5) —
  **not** re-derivable by reading alone; it needs the reproduction §5 asks for.

Neither was re-derived for this document. The mutation check quoted in §1
likewise comes from the program's task reports rather than from a run here; it
describes work that was done, so it is history rather than a premise. (A third
item, the detached-creation body sketch, left with §2 when it closed — and the
`RefCell` placement §4 recommended was not what shipped, which is the case for
re-deriving a recorded design before building on it.)
