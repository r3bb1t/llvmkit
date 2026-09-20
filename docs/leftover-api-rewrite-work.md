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
draft.** Nothing was fully closed, but four entries were wrong about their own
size or about what already exists, and each now says so inline: §2 (a `Detached`
handle *is* obtainable), §3 and §4 (two method names are already taken by
different things), §8 (five of seven readers were fixed by this program), §9
(`value_from_slot` has seven copies, not the two on record). The corrections all
run the same direction — the residue is smaller than the draft implied in two
places and larger in one — which is the reason to re-derive rather than trust
this file.

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
(checked, returns `Option`) or `Instruction::placed` on the `Attached`
typestate (total, because that state is only minted in a block). It is `Copy`,
from the default `#[derive(Branded)]` set.

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
the guarantee, and the type is a narrowing, not a proof.

---

## 2. There is no detached-creation primitive; the helper was deleted

`create_detached_instruction(module, ty, kind, name)` existed in `instruction.rs`
during Task 24 and was **deleted** before the commit because it was dead code and
`clippy -D warnings` refuses that. `rg -c 'create_detached_instruction' crates/`
returns no files at `02ae334`.

Its body is ~20 lines and is fully specified: `build_instruction_value(ty, None,
kind, name)`, then snapshot `kind.operand_ids()` followed by
`kind.block_operand_ids()`, `push_value`, and `add_use(ValueUse::Instruction(id))`
for each — exactly what `IrBuilder::append_instruction` does, minus the block
append and the symbol-table naming.

Three constructors sit on top of it and do not exist either:

- a detached `call` — port of the `InsertPosition`-less
  `CallInst::Create(FTy, Callee, Args, Bundles, Name)`;
- a detached `invoke` — port of
  `InvokeInst::Create(FTy, Callee, IfNormal, IfException, Args, Bundles, Name)`;
- a parentless block — port of `BasicBlock::Create(Context)`. The storage is
  already there: `BasicBlockData.parent` is `RefCell<Option<…>>`, and the block
  printer already has an "Orphan block" branch.

**What already works, so the gap is not mistaken for a bigger one:** a
`Detached` instruction *handle* is obtainable today —
`Instruction::detach_from_parent` returns `Instruction<'ctx, state::Detached, B>`,
and that is a port of `Instruction::removeFromParent`. What is missing is
*creation*: making an instruction that was never in a block at all. Verified with
`rg -n 'state::Detached, B>' crates/llvmkit-ir/src/instruction.rs`, whose only
producing site is `detach_from_parent`.

This is the foundation §3 needs, which is why it is listed first.

---

## 3. No call site can be copied with different operand bundles

Upstream's `CallBase::Create(CallBase *CB, ArrayRef<OperandBundleDef>, InsertPosition)`
re-creates a call, invoke or callbr with its bundles replaced. llvmkit has no
equivalent, and that blocks two whole-test ports.

The shape settled during the rewrite: a **sealed public `CallBase` trait** over
`CallInst` / `InvokeInst` / `CallBrInst`, with

```rust
fn with_operand_bundles(
    &self,
    module: &'ctx Module<B, Unverified>,
    bundles: …,
) -> IrResult<Instruction<'ctx, state::Detached, B>>;
```

**The name `with_operand_bundles` is already taken.** `instr_types.rs` has a
`pub(crate) fn with_operand_bundles(self, bundles: Box<[OperandBundleData]>) -> Self`
on the call attribute payload, reached from three `ir_builder.rs` sites. It is a
builder-side setter on `CallAttributeData`, not a copy constructor, and it is
*not* this item — but a grep for the name finds it and can read as "already
done". Either the trait method takes a different name or the payload one does.

Upstream's `llvm_unreachable` default arm is unrepresentable here, which is the
D1 improvement the sealed trait buys. The four upstream bodies copy: arguments,
called operand, function type, calling convention, tail-call kind (call only),
`SubclassOptionalData` (llvmkit spells this as the fast-math flags inside
`CallAttributeData`), attributes, debug location, name, and `NumIndirectDests`
for callbr. Bundle inputs take the checked door, `OperandBundleDef::into_stored`.

**What it unblocks:** `InstructionsTest.AlterCallBundles` and
`InstructionsTest.AlterInvokeBundles` port whole. They are currently recorded as
unportable in `docs/future-work.md` § *Operand bundles — three upstream unit
tests not ported*; closing this deletes those two entries from that section and
adds their `UPSTREAM.md` rows. The third, `AsmWriterTest.PrintNullOperandBundle`,
stays N/A by model — a stored input is an arena slot and is never null, so the
printer's `<null operand bundle!>` branch has no llvmkit state to reach.

---

## 4. Three payloads have no interior mutability, so the setters cannot exist

`CallInst::setTailCallKind` and `CallBase::setAttributes` have no llvmkit port,
and §3's copy needs both.

**A `set_attributes` does exist, on the wrong object.** `FunctionValue::set_attributes`
(`function.rs`) sets a *function's* attributes; the call-site port of
`CallBase::setAttributes` has no equivalent, and `rg -n 'fn set_tail_call_kind'
crates/` returns nothing. As with §3's name collision, a grep finds the
function-level one and can read as done.

- `CallInstData.tail_kind` is a plain `pub(crate) tail_kind: TailCallKind` field
  (`instr_types.rs`), and the attribute lists in the three call payloads are
  plain fields too. The fix-round-2 recommendation is a `RefCell` per payload **with
  the bundles moved out beside it**, so no public getter changes — `OperandBundleUse`
  borrows `&'ctx OperandBundleData`, which is exactly why the bundles cannot go
  inside the `RefCell`.
- `ValueData.debug_loc` is `pub(super) debug_loc: Option<DebugLoc>` (`value.rs`),
  read through a `debug_loc()` getter and never written after construction, so
  copying a debug location needs either a setter or a context helper.

Both setters take the `&Module<B, Unverified>` mutation token, per the rule that
mutation requires the unverified capability.

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
| `create_detached_instruction` is absent (§2) | `rg -c 'create_detached_instruction' crates/` → no files |
| `detach_from_parent` is the only `Detached` producer (§2) | `rg -n 'state::Detached, B>' crates/llvmkit-ir/src/instruction.rs` |
| `PlacedInstruction` is `Copy` (§1) | default `#[derive(Branded)]` set is `Clone, Copy, Debug, PartialEq, Eq, Hash` — `crates/llvmkit-macros/src/lib.rs`, `derive_branded` rustdoc |
| No `CallBase` trait; `with_operand_bundles` is taken (§3) | `rg -n 'trait CallBase' crates/` → no match; `rg -n 'with_operand_bundles' crates/` → `instr_types.rs` (`pub(crate)`) + 3 `ir_builder.rs` callers |
| No `set_tail_call_kind`; `set_attributes` is function-level (§4) | `rg -n 'fn set_tail_call_kind\|fn set_attributes' crates/` → `function.rs` only |
| `tail_kind` and `debug_loc` are plain fields (§4) | `rg -n 'tail_kind\|debug_loc' crates/llvmkit-ir/src/instr_types.rs crates/llvmkit-ir/src/value.rs` |
| `FnReshape::split_block_before` is absent (§9) | `rg -n 'fn split_' crates/llvmkit-ir/src/pass_context.rs` → `split_block` only |
| `value_from_slot` has **7** copies (§9) | `rg -n 'fn value_from_slot' crates/llvmkit-ir/src/` |
| Untracked specs = **15**, tracked design docs = **5** (§10) | `ls docs/superpowers/specs/ \| wc -l`; `ls docs/design/` |
| `docs/superpowers/` is gitignored (§10) | `git check-ignore -v docs/superpowers/specs/2026-09-20-placement-witness-design.md` |

Every row above was run with `rg` and direct file reads. **No LSP was available in the session
that wrote this** — `/reload-plugins` reported `0 plugin LSP servers` — so none of these is
backed by rust-analyzer's call graph. Each is an existence-or-count question, which text search
answers exactly; a claim about *reachability* (which §5 and §7 both turn on) is not, and is
marked where it appears.

Three things in this document come from the program's own `HANDOFF.md` rather
than from a command run here, and each is marked where it appears:

- the 21-commit span of Task 24 (§ frame) — re-derivable from `git log`;
- the `create_detached_instruction` body sketch (§2) — re-derivable from
  `IrBuilder::append_instruction`;
- the statement that a foreign `DynBrand` token unlocks a verified module (§5) —
  **not** re-derivable by reading alone; it needs the reproduction §5 asks for.

None of the three was re-derived for this document. The mutation check quoted in
§1, and the design shapes in §3 and §4, likewise come from the program's task
reports rather than from a run here; they describe work that was done, so they
are history rather than premises, but a session acting on §3 or §4 should still
read the payload definitions before trusting the recommended `RefCell` placement.
