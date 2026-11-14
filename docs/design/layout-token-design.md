# Layout Token — Permission Separated From Data — Design

Date: 2026-11-14. Target: `crates/llvmkit-ir` (`module.rs`, `ir_builder.rs`,
`instruction.rs`, `basic_block.rs`, `function.rs`, `pass_context.rs`,
`pass_manager.rs`, `ssa_builder.rs`), `crates/llvmkit-asmparser` (`ll_parser.rs`).
Status: **approved 2026-11-14, in progress.** The decisions taken in review
are recorded in §8 and override the text above them where the two differ.
It supersedes the unscheduled "layout session" entry in `docs/future-work.md`
(*A placement witness proves* was placed, *not* is placed).

## 1. The problem

llvmkit mutates a module through `&Module<B, Unverified>` and interior
mutability. No borrow ends when a structural edit happens, so anything that
*remembers* where it is in the layout can be invalidated behind its back.
The review of `864d2c03` found three concrete cases:

1. **A builder positioned before an anchor.** The anchor can be erased,
   moved or split off after positioning. At `864d2c03` every build in the
   `PositionedBefore` state therefore returns `IrResult` and refuses with
   `BlockError::AnchorNotInBlock`.
2. **A builder positioned at the end of a block.** A second builder can be
   positioned on the same block, from an id (`position_at_end_dyn`) or a
   saved `InsertPoint`. If it terminates the block first, the first builder's
   *infallible* `ret_void` / `unreachable` / `fence` / field read-write
   appends after the terminator. At `864d2c03` nothing refuses this: the
   premise "nothing about the end of a block goes stale", under which those
   six entries were made infallible, was false.
3. **A placement witness.** `PlacedInstruction` proves an instruction *was*
   in a block. The five entries that take one re-read the block and refuse
   a stale witness with `IrError::InstructionHasNoParent`.

All three have the same root cause: the right to change the layout is not
something the type system can see.

## 2. Prior art

- **GhostCell** (Yanovski, Dang, Jung, Dreyer, 2021) separates the
  *permission* to access a structure from the *data*. A `GhostToken<'id>` is
  the permission and a `GhostCell<'id, T>` the data. Holding `&token` allows
  reads and `&mut token` allows writes, so the borrow checker enforces
  exclusive mutation even though the data is freely aliased.
- **`qcell`** (`QCell`, `TCell`, `TLCell`, `LCell`) and **`tokenlock`** are
  variants of the same idea: owner tokens identified by an integer id, a
  marker type, or a lifetime.
- **Cranelift** gets the same exclusivity from `&mut Function` plus a
  `FuncCursor` positioned in its `Layout`.

What llvmkit takes from these is the **token**, not the cell. GhostCell's cell
needs `unsafe` to hand out `&mut T` from a shared reference. llvmkit's storage
is already `RefCell`-based and safe, so a token that is *pure permission* (no
data) gives the compile-time exclusivity with `#![forbid(unsafe_code)]`
intact.

## 3. The design

### 3.1 The token

```rust
/// The permission to change a module's layout: which instructions are in
/// which block, in what order, and which blocks are in which function.
/// One per module; not `Copy`, not `Clone`.
pub struct LayoutToken<B: ModuleBrand> { module: ModuleId, _brand: Invariant<B> }
```

- **Every structural edit takes `&mut LayoutToken<B>`.** That covers
  instruction insertion, erasure, detaching and moving; block appending,
  splitting, splicing and moving; and every builder.
- **Reads take nothing.** Views, analyses, the verifier and the printer stay
  on `&Module`, as today.

### 3.2 Why the token is separate from `Module`

A view holds `&'ctx ModuleCore` (`ModuleRef`), minted from `&Module`. A token
stored inside `Module` could only be borrowed `&mut` through `&mut Module`,
which conflicts with every live view and linear block handle. The Cranelift
shape (`&mut Function`) has the same conflict. So the token is its own owned
value, and the brand `B` ties it to its module.

### 3.3 Minting

A module is created together with its token, returned as a named struct
(CLAUDE.md forbids tuple returns):

```rust
pub struct NewModule<B: ModuleBrand> { pub module: Module<B, Unverified>, pub layout: LayoutToken<B> }

let NewModule { module: m, layout: mut layout } = module_new!("m")?;
```

`Module::branded`, `Module::dynamic`, `module_new!` and the parser's owned
entries (`parse_dynamic`, `parse_into`) return a `NewModule`. Typestate
transitions (`verify`) move only the module. The token is unaffected and can
be used again once the module is `Unverified` again.

### 3.4 Uniqueness, by brand

- **`B: UniqueBrand`** (named brands): the registry admits one live module
  per brand, so a `LayoutToken<B>` is that module's token by type. Every
  edit through it is infallible, with no run-time check.
- **`DynBrand`:** all dynamic modules share one type, so the token's
  `ModuleId` is the backstop (D7). It is compared **once, when an editor is
  opened**, for example in `IrBuilder::try_new(&m, &mut layout) ->
  IrResult<_>`, the `try_` convention `UniqueBrand` already uses. After that,
  builds are infallible.
- **Re-issued brands:** the token holds the brand registration together with
  the module, so a named brand is not re-issued while either is alive. See
  open question 2.

### 3.5 Builders

```rust
let b = IrBuilder::new(&m, &mut layout).position_at_end(entry);
let c = IrBuilder::new(&m, &mut layout);   // E0499: `layout` is already borrowed
b.ret_void();                              // infallible: no one else could edit the block
```

- **At-end builds stay infallible, and now honestly so.** While the builder
  holds the token `&mut`, nothing else can terminate its block.
- **Before-anchor builds become infallible again.** The anchor cannot be
  erased, moved or split off while the builder lives. The `IrResult` added
  to every `PositionedBefore` build in `864d2c03` is removed.
- **Positioning stays fallible.** `position_before` re-checks the anchor
  against the live layout once, at positioning time. A witness minted
  earlier (`&layout`) cannot be passed while the builder holds `&mut layout`,
  so positioning takes the view and checks it itself.
- **Restoring a stored `InsertPoint` stays fallible.** A snapshot is
  `'static` data saved under an earlier borrow, so `restore_insert_point`
  keeps its run-time refusals.
- **Upstream's style still works.** One builder, repositioned many times, is
  how `IRBuilder` is used upstream. What no longer compiles is two builders
  over one module alive at once.

### 3.6 Witnesses

```rust
let placed = view.placed(&layout)?;    // shared borrow of the token
ret.erase_from_parent(&mut layout);    // E0502 while `placed` is alive
```

`PlacedInstruction<'t, 'ctx, B>` borrows the token shared for `'t`, so it
proves *is placed* for as long as it lives. The five entries stop
re-checking, and the `InstructionHasNoParent` refusal at those entries goes
away.

### 3.7 Passes and the sandbox

- **The driver takes the token:**
  `run_function_pass(pass, verified, &mut layout, f, &mut analyses)`.
- **Mutating contexts lend it scoped to the function.** `FnPatch` and
  `FnReshape` hand out a function-scoped permission, `FnLayout<'f, B>`, built
  from the `&mut LayoutToken`. Exclusivity is compile-time: while a pass's
  builder lives, nothing else edits.
- **Function membership stays a run-time check, and the token does not change
  that.** Blocks, instructions and operands arrive as ids, which are `'static`
  data, so "this id names a block of *this* function" can only be checked
  when the id is admitted. That check is the full sandbox you chose, and it
  is built on top of this design, not replaced by it.

### 3.8 Parser and SSA builder

`ll_parser` and `SsaBuilder` borrow the token for their whole run, as they
already hold one builder each.

## 4. What changes, measured

All counts are at `864d2c03` from the workspace root, using
`rg -F --glob '*.rs' -c "<pattern>" <paths> | awk -F: '{s+=$2} END{print s}'`.
`<paths>` is `crates/*/src` for the "src" column and
`crates/*/tests crates/*/examples` for the "tests + examples" column.

| Call site | src | tests + examples |
|---|---|---|
| `IrBuilder::(new\|new_for\|at_end)` or `.builder()` (regex, `rg` without `-F`) | 130 | 775 |
| `append_basic_block(` | 157 | 884 |
| `erase_from_parent(` | 7 | 8 |
| `detach_from_parent(` | 1 | 8 |
| `.split_at(` / `.split_before(` | 3 / 0 | 11 / 7 |
| `run_function_pass(` | 8 | 67 |

Every row is a signature change: each call gains a `&mut layout` argument.
Not yet measured, and the implementation's first step: how many tests hold
two builders over one module at once. Those are the sites that need
restructuring rather than a mechanical edit.

**Run-time refusals this removes**
- `BlockError::AnchorNotInBlock` from builds. It stays only on
  `restore_insert_point`.
- The at-end-after-terminator hole (case 2), with no check needed.
- `IrError::InstructionHasNoParent` at the five witness entries.
- `at_end_infallible` and its dead arm, since both states are infallible.

**Breaking for callers**
- Module creation returns `NewModule`.
- Structural edits and builder constructors take the token.
- Witnesses borrow it.
- `PositionedBefore` builds lose their `IrResult` again.

## 5. Alternatives considered

| Alternative | Why not |
|---|---|
| `&mut Module` as the permission (Cranelift's `&mut Function`) | Views and linear block handles borrow `&Module`; every one would have to become an id first. |
| A run-time builder lease (the block records its holder; `RefCell`-like) | Exclusivity at run time only, plus a `Drop` impl. Every insertion path, instruction moves included, must honour the lease, which makes holes easy. |
| A closure-scoped token (`GhostToken::new(\|t\| ..)`) | CLAUDE.md: no way to make a module is closure-scoped, deliberately, so modules move into structs, `Vec`s and threads. |
| Per-function tokens (two builders in two functions at once) | More types, for a pattern upstream does not need. It can be added later as a split of `LayoutToken` (open question 1). |
| An interim run-time check on at-end builds | Rejected by the user in favour of this design: the token makes it obsolete. |

## 6. Open questions for review

1. **One token per module, or splittable per function?** The proposal is one
   per module. Splitting (`layout.split_function(f)`) could come later.
2. **Brand claim lifetime.** Should the token share the module's
   `BrandGuard`, so the brand is released when both are dropped, or should
   it be valid only while its module lives? Read `module.rs`'s registry
   before deciding.
3. **`create_orphan`.** An orphan block is in no function's layout. Should
   creating one need the token? The proposal is no; inserting it into a
   function does.

## 7. Plan

1. **Audit.** Measure the concurrent-builder sites (§4) and settle the open
   questions.
2. **Add the token.** Introduce `LayoutToken` and `NewModule`, thread the
   token through the primitives (`basic_block.rs`, `function.rs`,
   `instruction.rs`), and then through the builder.
3. **Compile-fail fixtures.** Add one each for a second builder, an edit
   while a witness lives, and an erase while a before-anchor builder lives.
4. **Remove the refusals** listed in §4 and their tests. Each removed test
   becomes a compile-fail fixture proving the case no longer compiles.
5. **Passes.** Lend `FnLayout` from the driver, then build the full sandbox
   (run-time membership checks) on top.
6. **Migrate** the parser, `SsaBuilder`, tests and examples, then update docs:
   `type-safety-vs-llvm.md`, `inkwell-migration.md`, and the `future-work.md`
   entry, which this note closes.

## 8. Decisions (2026-11-14)

Checked against a real consumer, [Mergen](https://github.com/NaC-L/Mergen).
Each lifter owns one module and one function, and keeps one long-lived
`IRBuilder` (`lifter/core/LifterClass.hpp`). It always creates blocks inside
the function.

1. **One token per module** (open question 1). No per-function split.
2. **The token borrows its module**: `LayoutToken<'m, B>` holds
   `&'m Module<B, Unverified>`.
   - **Minting:** a module hands its token out with `m.layout_token()`. This
     refuses only while another token over the module is alive, which a flag
     in the module core tracks and the token's `Drop` clears.
   - **What this replaces:** §3.3's `NewModule` and §3.4's brand check.
     - The token names exactly one module, so builders and edits take only
       `&mut LayoutToken`, with no `try_` forms for `DynBrand`.
     - It cannot outlive its module, so open question 2 (brand reuse) does
       not arise.
     - `verify(self)` cannot run while a token is alive, because the token
       borrows the module.
   - **Exclusivity:** the one run-time refusal is at acquisition. After
     that, exclusivity is compile-time.
3. **Appending a block, or creating an orphan, needs no token** (open
   question 3). Neither can invalidate a live builder's position or a
   witness. Only edits that can are keyed: inserting, appending, erasing,
   detaching or moving an instruction, and splitting or splicing a block.
   Moving a block within its function's list
   (`Function::move_basic_block_to_end`) changes no block's contents, so it
   is not keyed either.
4. **The builder's `'m`** (today an unused `PhantomData<&'m Module>`)
   becomes the token borrow. As built, the builder keeps only the borrow
   (`_key: PhantomData<&'m mut ()>`), since the token carries no data the
   builder needs. Public constructors take `&'m mut LayoutToken<'ctx, B>`,
   and crate-private unkeyed ones serve library code that holds the
   permission further up. The builder's type parameters are unchanged.
5. **Pass contexts hold no token.** The driver owns the module, so no user
   token can be alive while a pass runs. Inside the pass, `builder_at` /
   `builder_before` borrow the context `&mut`, so no context edit runs while
   a pass's builder lives, and a module pass's function cursor lends one
   mutator at a time (decision 7).
6. **Witnesses (§3.6) are deferred.** The five witness entries need
   `&mut LayoutToken`, so a witness that borrows the token shared cannot be
   passed to them. They keep their run-time re-check, which stays correct.
   Folding the token into the witness is a later step.
7. **Review of `24540903`.**
   - **One function mutator at a time.** `ModRewrite::patch_functions` /
     `reshape_functions` are lending cursors. `step` lends one mutator
     borrowing the cursor, which borrows the rewrite context `&mut`. Before
     this, two mutators over one function could be alive at once, and one's
     edit could invalidate the other's builder. That route was the last that
     could make a live builder's anchor stale, so the run-time stale-anchor
     check is gone and before-anchor builds are infallible where nothing
     else can refuse.
   - **`builder_before`.** `FnPatch` / `FnReshape::builder_before(anchor)` is
     how a pass starts a builder at one of its own instructions; before it,
     a pass could only restore an `InsertPoint` saved outside.
   - **`IrBuilder::at_end` / `BasicBlock::builder` return `IrResult`.** They
     refuse a block of another module than the token's
     (`ForeignLayoutToken`). Before this design they took the module from
     the block; taking it from the token without a check would have written
     into the wrong arena.
   - **Remaining gap.** `position_at_end(bb)` is still infallible and does not
     compare `bb`'s module with the builder's. It is one of the counted F1
     boundaries (`docs/leftover-api-rewrite-work.md` §6, Task 26), whose fix
     is a `Result` per site. Under a shared brand, a builder over one module
     positioned at another module's block is outside the token's guarantee
     until that task lands.
