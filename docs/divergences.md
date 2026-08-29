# Known divergences from LLVM

Places where llvmkit's **observable behaviour** differs from the vendored
reference tree (`orig_cpp/llvm-project-llvmorg-22.1.4/llvm/`). Each entry says
what LLVM does, what llvmkit does instead, why, and what closing it takes.

This file is for *behavioural* differences — input accepted or rejected
differently, different diagnostic text, different printed bytes, different
answers from a public query. It is **not** a backlog of unimplemented
features; that is [`future-work.md`](future-work.md), which also carries the
rationale for anything deliberately deferred. A spelling difference that
changes no behaviour (an enum where LLVM uses a sentinel, a `Result` where
LLVM uses an out-parameter) is house doctrine, not a divergence, and does not
belong here.

**Every entry must be verified against the tree before it is trusted — and
that includes its evidence.** This project has repeatedly found its own
recorded premises wrong; three failed on contact in a single session, and
several `Verification evidence` blocks have since been found stale in turn. No
ids are named here on purpose: entries are deleted when they are closed and
their ids are re-used, so a list of examples eventually points at entries it
never described. A `<details>` block and a `Correction from verification`
paragraph are *dated snapshots of one verification pass*, not standing proof:
their line numbers, counts and command output were true when written and are
re-checked by nothing. Most evidence blocks in this file carry no date at all,
and only a handful carry one on the `<summary>` line, where it can be seen
without opening the block; before 2026-08-20 none did. No count is given here
on purpose: the obvious way to derive one is to grep for the block's opening
marker, and this paragraph would then be counting its own quotation of it.
Treat a row **and its evidence** as a hypothesis with a citation. When you re-verify an entry, date
the block you are trusting or replacing — on its `<summary>` line.

Entry prose cites upstream **by symbol, never by line number** (repo law: line
numbers rot the moment the vendored tree moves). The `Correction from
verification` and `<details>` blocks predate that rule and still carry
`File.cpp:LINE` pointers.
Treat those coordinates as valid only against the vendored 22.1.4 tree, and
re-derive the symbol before quoting one. The sweep is recorded in
[`future-work.md`](future-work.md).

Severities:

| | |
|---|---|
| **accepts-invalid** | llvmkit accepts input LLVM rejects |
| **rejects-valid** | llvmkit rejects input LLVM accepts — the worst kind, a parser that cannot read LLVM's own output |
| **wrong-message** | same verdict, different diagnostic text |
| **wrong-output** | printed bytes differ |
| **model-gap** | a public query answers differently from its LLVM counterpart |

---

## Use lists

### D1 — A terminator's operand list excludes its successors

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/instruction.rs` —
`InstructionKindData::operand_ids` vs `block_operand_ids`

**LLVM:** `BranchInst`, `SwitchInst`, `IndirectBrInst`, `InvokeInst`,
`CallBrInst`, `CatchSwitchInst`, `CleanupReturnInst` and `CatchReturnInst`
hold their successor `BasicBlock`s as ordinary `Use` operands, interleaved
with the value operands in one array. (`PHINode` is the deliberate exception:
its incoming blocks live in a hung-off array reached by `block_begin` and are
*not* `Use`s.)

**llvmkit:** blocks *do* now carry use edges — W12 added
`block_operand_ids()` and registers it everywhere `operand_ids()` is
registered, so `uselistorder label %bb` and `uselistorder_bb` work. But the
two lists stay **separate**: `operand_ids()` is what 21 crate-internal
consumers walk, and they assume an operand is a first-class SSA value.

**Consequence:** `User::operand_count` on a conditional `br` answers 1 where
upstream answers 3, and `User::operand(i)` indices do not match
`Use::getOperandNo()`. Nothing in the tree reads those indices for a
terminator today.

**Fix:** merge the two, and re-check the four index-keyed consumers plus every
site that assumes `operand(i)` is first-class. Upstream's true order is
`br` = `[cond, else, then]`, `switch` = `[cond, default, (val, dest)*]`,
`invoke` = `[args…, normal, unwind, callee]`.

### D1a — Two block operands cannot be RAUW-rewritten

**Severity:** model-gap (latent — no caller today)
**Where:** `crates/llvmkit-ir/src/instr_types.rs` —
`CleanupReturnInstData::unwind_dest`, `CatchReturnInstData::target_bb`

Every other block-holding field is a `Cell` or lives inside a `RefCell`, so a
future block-RAUW could rewrite it. These two are plain fields with no
interior mutability. Nothing performs block RAUW today, so this is latent;
adding one without changing these two field types would silently skip them.

### D2 — Constants wrapping a global register no operand use

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/constant.rs` — `ConstantData::for_each_operand`

**LLVM:** `DSOLocalEquivalent`, `NoCFIValue` and every `ConstantExpr` naming a
global hold it as a `Use`.

**llvmkit:** `for_each_operand` yields operands for `Expr`, `Aggregate`,
`PtrAuth` and — since W12 — `BlockAddress`, whose `[Function, BasicBlock]`
operands are what let a block be used from outside its own function.
`GlobalValueRef`, `DsoLocalEquivalent`, `NoCfi`, `GepOffset`, `SymbolDelta`
and `SymbolDeltaPlus` still yield nothing.

**Why not fixed with the rest:** all six wrap a *global*, and yielding them
would register the edge against llvmkit's interned wrapper rather than the
global — see D3. That loses count instead of fixing it, so the two have to be
closed together.

**What it costs elsewhere:** upstream's `kw_no_cfi` arm can wrap a
forward-reference placeholder immediately, because `NoCFIValue` is a real
`User` and `handleOperandChange` re-interns it when the placeholder is RAUW'd.
llvmkit's `NoCfi` is not, so nothing would rewrite it — the parser keeps a
`pending_no_cfi` list and builds the wrapper after the `ForwardRefVals` sweep
instead. Same messages, same positions, one extra end-of-module step; it goes
away when D2/D3 close.

### D3 — `GlobalValueRef` is an interned wrapper LLVM does not have

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/llvm_context.rs` — `use_edge_target`,
`intern_constant_global_value_ref`

**LLVM:** a `GlobalValue` *is* a pointer-typed `Constant`. A field or operand
naming `@g` is a `Use` of `@g` itself, so three aliases of one global are
three uses.

**llvmkit:** a global object carries its *value* type, and a separate
**interned** `ConstantData::GlobalValueRef` stands for `@g` where a constant
is wanted. Because it is interned, every reference shares one node, so edges
registered against the wrapper collapse to a single use.

**Partly fixed (W12):** `use_edge_target` unwraps the wrapper for
`ValueUse::GlobalField` edges — alias, ifunc resolver, initializer,
personality, prefix, prologue — so those now count per-reference as upstream
does, and `rewrite_global_field_cell` re-interns the wrapper on RAUW.

**Still open:** the same unwrap is *not* applied to constant-expression
operands. `@a` referenced from `getelementptr (… ptr @a …)` still records the
use against the wrapper, so `@a`'s count is one regardless of how many
expressions name it. Closing it needs `constant_with_replaced_operand` to
match wrapper-held operands too, which is why it was not done with the
global-field half.

### D4 — Two uses by the same user are indistinguishable

**Severity:** wrong-output (narrow)
**Where:** `crates/llvmkit-ir/src/value.rs` — `ValueUse::Instruction`

**LLVM:** a `Use` is identified by `(user, operand number)`, and
`AsmWriter::predictValueUseListOrder` breaks ties between two uses by the same
user on `Use::getOperandNo()`.

**llvmkit:** `ValueUse::Instruction(user)` records only the user, so
`add i32 %x, %x` produces two identical edges. The tie-break has no
counterpart; the comparator treats them as equal and the stable sort leaves
their relative order alone.

**Consequence:** for a value referenced twice by one user, the emitted
`uselistorder` shuffle may differ from upstream's. Both are self-consistent —
llvmkit's re-parses to the order it printed — so this is a byte difference,
not a correctness one. No upstream fixture reaches it.

**Fix:** carry the operand index on `ValueUse::Instruction` and on
`ValueUse::Constant` — both record only the user today. Touches every
registration site, `deregister_operand_uses`' occurrence counting, and the
paths that replace a single operand without knowing its index
(`demanded_bits.rs`'s operand replacement holds a `Cell`, not a position).
Blocked behind D1: the index is only meaningful once the two operand lists
are one.

### D4a — Call-family operand order puts the callee first

**Severity:** model-gap (latent, feeds D4)
**Where:** `crates/llvmkit-ir/src/instruction.rs` — `operand_ids`, `Call` /
`Invoke` / `CallBr` arms

**LLVM:** `CallInst`, `InvokeInst` and `CallBrInst` all place the called
operand **last** — `setCalledOperand` writes the highest index, and
`InvokeInst::init` carries the comment "Set operands in order of their index
to match use-list-order prediction."

**llvmkit:** `operand_ids()` yields the callee **first**, then the arguments.

**Consequence:** none observable yet, because llvmkit has no operand-number
tie-break (D4). It becomes observable the moment D4 is closed.

### D5 — `num_uses` counts metadata and debug-record edges

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/value.rs` — `ValueData::use_list`

**LLVM:** a `ValueAsMetadata` creates no `Use`. Metadata references are
tracked through `ReplaceableMetadataImpl` instead, which is exactly why
`AsmWriter::orderModule` has to reach *through* `MetadataAsValue` wrappers and
debug records to find the constants behind them. `Value::getNumUses` counts
only `Use`s.

**llvmkit:** the use list also holds `ValueUse::Metadata` and
`ValueUse::DebugRecord` edges, because RAUW must reach them. `num_uses`,
`has_uses` and `has_one_use` therefore over-count relative to their LLVM
counterparts whenever metadata names the value.

**Partly fixed (W12):** everything phrased in terms of `Value::uses()` —
`sort_use_list` and the `predictUseListOrder` port — filters to the
operand-use subset via `ValueUse::user()`.

**Still open:** the public `num_uses` / `has_uses` / `has_one_use` accessors
are unfiltered, and one in-tree consumer already reads the wider count —
`demanded_bits.rs` gates on `value.num_uses() != 1` where upstream's
`hasOneUse` would pass on a value a metadata node or debug record
*additionally* names.

Demonstrating it needs a non-`ConstantData` value — an instruction, argument
or global — named by a `!{…}` node, a `DIArgList` or a `#dbg_value` record. A
`ConstantData` such as `i32 1` diverges for a second, separate and
deliberately documented reason: upstream's `getNumUses` returns 0 outright
because `ConstantData` carries no `UseList`, while llvmkit records uses for
those values too (`Value::has_use_list`).

**Fix:** decide whether the public accessors should mirror `getNumUses`
(filter) and audit their callers, or gain filtered twins.

---

## End of module

### D7 — `dso_local_equivalent` forward references are not deferred — **FIXED (W13b)**

`parse_dso_local_equivalent_constant` now mirrors the arm: it skips a referent
that is still a `ForwardRefVals` entry, parks the reference in
`forward_ref_dso_local_equivalent_ids` / `…_names`, and
`resolve_forward_ref_dso_local_equivalents` drains them at
`validateEndOfModule`'s position 3, ids before names, with
`unknown function '<name>' referenced by dso_local_equivalent` and
`expected a function, alias to function, or ifunc in dso_local_equivalent`.
The upstream quirk is reproduced: the numbered spelling really does print an
empty name, because `ValID::StrVal` is empty for a `t_GlobalID`.

### D8 — A `blockaddress` in a global initializer never reaches the leftover check — **FIXED (W13b)**

The initializer deferral is gone: `parse_global` parses its initializer inline
like `parseGlobal` does, so a forward `blockaddress` lands in
`deferred_block_addresses` (upstream's `ForwardRefBlockAddresses`) and is
retired either by the named function's `resolve_block_addresses_for_function`
or by `validateEndOfModule`'s guard. `test/Assembler/pr119818.ll` — a global
initializer naming a *numbered* label of a later function — parses as a
result, and `expected function name in blockaddress` now fires from position 2
for a global initializer as well as a function body.

**Left open by the same change:** llvmkit still keys
`deferred_block_addresses` as a flat list rather than upstream's
`ForwardRefBlockAddresses[Fn][Label]` map, so two references to the same
`(function, label)` pair mint two placeholders where upstream shares one. The
only observable consequence is that the
`type of blockaddress must be a pointer and not '…'` check, which upstream
runs only when it creates a *fresh* placeholder, runs on every reference here.

### D9 — Attribute groups are never merged, and the alignment move is half-ported — **ONE DUPLICATE MERGED (W11)**

**Severity:** wrong-output, model-gap
**Where:** `crates/llvmkit-ir/src/function.rs` — `function_attr_groups`;
`crates/llvmkit-asmparser/src/ll_parser.rs` — `parse_optional_function_suffix`,
and the end-of-module sweep

This is the **parser** half of the attribute-group gap. Its printer twin is
entry 58, which is where the group-*forming* side lives; neither entry
restates the other. Former entry **41** ("`align` inside an attribute group is
not moved onto the function") said only what this entry's alignment paragraph
already says, and was deleted at W11 rather than left to be closed twice.

**LLVM:** `validateEndOfModule`'s first step merges every referenced
`#N` into the object's own attribute set, for five object kinds — `Function`,
`CallInst`, `InvokeInst`, `CallBrInst`, `GlobalVariable`. In the `Function`
arm only, an alignment that arrived as an *attribute* is moved to the
alignment field and removed: `FnAttrs.getAlignment()` -> `Fn->setAlignment(*A)`
-> `FnAttrs.removeAttribute(Attribute::Alignment)`, so `attributes #0 = { align
= 8 }` re-prints as `define void @f() align 8` with the attribute gone from the
group. Upstream then discards the parsed numbering: `AsmWriter` re-derives
`#N` from `SlotTracker`'s dedup, so `attributes #7` on input can print as `#0`.

**llvmkit:** no merge. Group ids are kept on the object and resolved lazily on
lookup and at print time, so the input numbering round-trips. The alignment
move exists only for *inline* attributes — mirroring upstream's other copy of
the same hack in `parseFunctionHeader`, ported in `parse_optional_function_suffix`
with `check_attribute_position` carrying the matching `Alignment` exemption —
so `define void @f() #0` with `attributes #0 = { align 8 }` leaves `align` as a
plain function attribute instead of setting the field. The written text
round-trips where `llvm-as | llvm-dis` normalises it.

**Also:** an undefined `#N` is silently ignored by upstream (the
`NumberedAttrBuilders` lookup simply misses). llvmkit likewise never errors,
but then prints a dangling `#N` with no `attributes #N = { … }` line —
output that does not re-parse.

**What pins the current behaviour:**
`crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs::attribute_group_equals_grammar_round_trips`
asserts `attributes #0 = { align = 8 }` re-prints as `align=8` *inside* the
group, so it turns red the day this entry closes.

**Fix:** this is the item W7 blocked on, and it has to land with entry 58's
group-*forming* half of the printer, because the merge decides what survives
into the printed group. Brings `globalvariable-attributes.ll`'s `@g1`–`@g4`
and `test/Bitcode/attributes.ll` with it.

## Entry points and configuration

### D10 — `Can't read textual IR with a Context that discards named Values` is **N/A (structurally impossible)**

**Severity:** none — recorded so the parity ledger's `MISSING` count is not
chased through this row.
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` — `Parser::parse_module`
(the `LLParser::Run` mirror)

**LLVM:** `LLParser::Run` opens, right after the priming `Lex.Lex()`, with
`if (Context.shouldDiscardValueNames()) return error(Lex.getLoc(), "Can't read
textual IR with a Context that discards named Values");`. The flag is
`LLVMContext::setDiscardValueNames`, which a client sets to save memory on
IR it will never print.

**llvmkit:** there is no discard-names mode to check. `llvm_context.rs`'s
`Context` is a per-module type / constant interning pool with no configuration
flags at all, and `ModuleCore` stores names unconditionally. Every entry point
either takes a caller's `Module<B, Unverified>` or builds `Module::dynamic`;
no object on any of those paths carries the setting.

**Why it is not "missing":** the diagnostic has no reachable trigger, so
implementing it would mean *inventing* a mode purely to have somewhere to
report from. This is recorded rather than invented, in the same spirit as
`argument can not have void type` — a message that is dead upstream and
correspondingly absent here.

**What would change this:** a `discard_value_names` mode on `Module`, wanted on
its own merits. The guard is one `if` at the top of `parse_module` on the day
it exists.

### D11 — `ParserConfig` carries three settings and two of them select nothing yet

**Severity:** model-gap
**Where:** `crates/llvmkit-asmparser/src/parser.rs` — `ParserConfig`

**LLVM:** `LLParser::Run(bool UpgradeDebugInfo, DataLayoutCallbackTy)` plus the
`-allow-incomplete-ir` `cl::opt` are the three knobs a parse runs under.
`AllowIncompleteIR` is read at three places in `validateEndOfModule`;
`UpgradeDebugInfo` guards exactly one statement, `llvm::UpgradeDebugInfo(*M)`.

**llvmkit:** all three are modelled, and their defaults match. What each
selects:

| setting | status |
|---|---|
| `data_layout_callback` | **implemented.** `parse_target_definitions` holds the layout string tentative across the leading `target` / `source_filename` run and offers `(triple, tentative)` to the callback before `DataLayout::parse`. |
| `allow_incomplete_ir` | **partly implemented.** The declaration-synthesis half is a whole port of `validateEndOfModule`'s `GetCommonFunctionType` block, callee references included; the metadata half is not — see D13. |
| `upgrade_debug_info` | **selects nothing.** llvmkit has no `AutoUpgrade` port (see the inventory entry on `lib/IR/AutoUpgrade.cpp`), so there is no `UpgradeDebugInfo` call for the flag to guard. |

**Consequence:** a caller that sets `upgrade_debug_info: false` (what `llvm-as`
and `opt -disable-upgrade-debug-info` do) gets the same module as one that
leaves it `true`. Nothing is silently *wrong* — llvmkit never upgrades — but
the setting is a promise about a future behaviour rather than a live switch,
and the rustdoc says so.

**Fix:** lands with `AutoUpgrade`. The flag already reaches
`Parser::parse_module_with_config`; only the guarded call is missing.

### D13 — `dropUnknownMetadataReferences` has no counterpart

**Severity:** model-gap
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` — the `metadata_slots`
leftover loop in `Parser::parse_module_with_config`

**LLVM:** under `-allow-incomplete-ir`, `validateEndOfModule` calls
`dropUnknownMetadataReferences` before the `ForwardRefMDNodes` guard. It erases
every attachment whose node is still temporary from functions, instructions and
global variables; erases `DbgInfoIntrinsic` and
`llvm.experimental.noalias.scope.decl` calls that carry one
(`dropIntrinsicWithUnknownMetadataArgument`); and then drops the
`NumberedMetadata` / `ForwardRefMDNodes` entries whose only remaining reference
is the numbering itself. `test/Assembler/incomplete-ir-metadata.ll` is the
fixture.

**llvmkit:** the option is accepted and this step is skipped, so an undefined
`!N` is still `use of undefined metadata '!N'` with or without it.

**Why:** two things are missing. llvmkit resolves metadata forward references
by reserve-then-fill on a stable `MetadataId` (`resolve_md_slot` ->
`metadata_reserve`), so "is this node temporary?" has to be asked of the
parser's own `metadata_slots` table rather than of the node — answerable, but
different. The blocker is the second: `MetadataAttachmentSet` has `insert` and
`get` and no removal, none of the four attachment holders has an
`eraseMetadataIf`, and an `InstructionView` cannot be erased from its block
(erasure takes the linear `Instruction<Attached>` handle). Closing this is
`llvmkit-ir` surface work, not parser work.

**Fix:** add `MetadataAttachmentSet::retain` plus `erase_metadata_if` on
`FunctionValue` / `InstructionView` / `GlobalVariable`, and a view-level
instruction erase; then port the routine and
`test/Assembler/incomplete-ir-metadata.ll` with it. The negative half,
`test/Assembler/incomplete-ir-metadata-unsupported.ll`, already passes: it
expects `use of undefined metadata '!1'` even with the option on.

The third `AllowIncompleteIR` tolerance rides on this one and needs nothing of
its own: `validateEndOfModule`'s `InstsWithTBAATag` loop relaxes
`assert(MD && "UpgradeInstWithTBAATag should have a TBAA tag")` to a skip under
the option, and the only thing that can drop such a tag is
`dropUnknownMetadataReferences`. llvmkit does not port the assert — repo law
forbids a runtime panic in a production path — so the relaxation has no
observable behaviour to diverge on until this entry closes.

### D14 — a `declare` prints its parameter names

**Severity:** wrong-output
**Where:** `crates/llvmkit-ir/src/asm_writer.rs` — the function-header
parameter loop
**Pinned by:** `crates/llvmkit-ir/tests/builder_call.rs`
(`declare float @llvm.acos.f32(float %0)`) and, since this wave,
`crates/llvmkit-asmparser/tests/parser_facade.rs::incomplete_ir_declarations`

**LLVM:** `AssemblyWriter::printFunction` branches on `F->isDeclaration()`:
a declaration prints only parameter *types* and their attributes — "We're only
interested in the type here - don't print argument names" — while a definition
prints `printArgument`, names included.

**llvmkit:** there is no branch. The loop always prints a name, falling back to
the slot number when the parameter is unnamed, so `declare void @f(i32)` on
input prints as `declare void @f(i32 %0)`.

**Consequence:** printed bytes differ from `llvm-dis` for every declaration
with at least one parameter — including every intrinsic declaration a module
picks up. The output still re-parses to an equivalent module, and the corpus
did not catch it because its expected files are llvmkit's own output; it
surfaced only when an upstream `CHECK` line was compared directly.

**Fix:** port the `isDeclaration()` branch, then re-bless every expected
fixture and byte-lock that carries a parameterised `declare`. The two tests
above encode the current behaviour and have to move with it.

### D15 — `target` and `source_filename` are accepted after other entities

**Severity:** accepts-invalid
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` —
`Parser::parse_module_with_config`'s dispatch loop,
`parse_late_target_definition`

**LLVM:** `parseTargetDefinitions` consumes the *leading* run of `kw_target` /
`kw_source_filename` and stops at the first token that is neither.
`parseTopLevelEntities` then has no arm for either keyword, so a `target triple`
written after any other entity falls to `default:` and is
`expected top-level entity`.

**llvmkit:** the leading pre-pass exists as of this wave — it is what makes
`data_layout_callback` possible — but the dispatch loop still keeps its
`Target` / `SourceFilename` arms, so a late one is accepted.

**Consequence:** input LLVM rejects is accepted. A late `target datalayout` is
also validated and installed on the spot rather than tentatively, so it never
reaches the data-layout callback: a caller overriding the layout gets its
override replaced by a stray late directive, where upstream would have refused
the file.

**Fix:** delete the two arms. Held back from this wave because it changes the
verdict on inputs the corpus may contain, which is a re-blessing pass of its
own rather than a line deletion.

### D16 — `parseDIExpressionBodyAtBeginning` has no public entry point

**Severity:** model-gap
**Where:** `crates/llvmkit-asmparser/src/parser.rs` — the standalone-fragment
family

**LLVM:** `Parser.h` exposes `parseDIExpressionBodyAtBeginning(Asm, Read, Err,
M, Slots)` beside `parseType`, `parseTypeAtBeginning` and `parseConstantValue`.
It restores the slot mapping, parses one `!DIExpression(...)` body, and reports
the byte count consumed —
`unittests/AsmParser/AsmParserTest.cpp` exercises it directly.

**llvmkit:** `Parser::parse_di_expression_body` exists but is private, reached
only from the specialised-metadata production. There is no
`parse_di_expression_body_at_beginning` on the public facade and no
`parseStandaloneMetadata`-style wrapper, so the last `Parser.h` entry point is
unreachable from outside the crate.

**Consequence:** a caller cannot parse a debug expression against an existing
module's slot tables the way the type and constant fragments allow. Nothing
inside llvmkit needs it, which is why it was not noticed until the entry-point
surface was audited as a whole.

**Fix:** a wrapper of the same shape as `parse_type_at_beginning_with_slots` —
`restoreParsingState`, prime, record the start location, call the existing body
parser, report `End - Start`. The upstream unit test ports with it, including
its `expected '(' here` negative case.

---


---

# Inventory

Swept out of `docs/future-work.md`, the source comments, the test doc comments
and the parity program's own records by four independent readers, then **each
candidate re-verified against the tree by its own agent** — 102 candidates, 99
confirmed still present, 3 found already closed (kept at the end). One of the
99 described work that was in flight while the sweep ran and is not reproduced.

Every entry carries the verifier's evidence verbatim, folded into a
`<details>` block. Where verification corrected the original claim, the
correction is quoted in full rather than summarised — several corrected a line
number, a count, or the scope of the claim, and two found the stated *reason*
wrong while the divergence itself held.

llvmkit file/line references are indicative and rot; the symbol names do not.

Where an inventory entry overlaps the hand-written section above, the section
above is current: it reflects what wave 12 actually closed, while the sweep
read the tree as it stood when each entry was first recorded.

Entries numbered **above 98** were not part of that sweep: they are divergences
later waves found and appended to the section they belong to, carrying the wave
that found them instead of a verifier's evidence block.

## Rejects valid input

llvmkit refuses IR that LLVM accepts — the worst kind, a parser that cannot read LLVM's own output.

### 19. AutoUpgrade does not exist — legacy-but-valid modules are not upgraded — **PARTLY FIXED (W13d, W10)**

**Status 2026-08-29 (W10).** `UpgradeNVVMAnnotations` is ported —
`auto_upgrade::upgrade_nvvm_annotations`, with `upgrade_single_nvvm_annotation`,
`upgrade_nvvm_fn_vector_attr` and `is_xyz` under their own names — and wired at
its own position in `Parser::parse_module_with_config`'s end-of-module block,
between `upgrade_module_flags` and `upgrade_section_attributes`, exactly where
`validateEndOfModule` calls it. `test/CodeGen/NVPTX/upgrade-nvvm-annotations.ll`
now passes whole (all fourteen entries, every `CHECK`), as
`parser_auto_upgrade.rs::nvvm_annotations_become_function_attributes`.
**Four of the nine call sites are ported; five remain.**

Two of the three blockers `docs/future-work.md` recorded against this call site
were **wrong**, which is why it ported in one pass:
`CallingConv::PTX_KERNEL` has existed since the calling-convention table was
written (`crates/llvmkit-ir/src/calling_conv.rs`), and `addParamAttr` at a
computed index is `FunctionValue::add_attribute` with `AttrIndex::Param`. Only
`NamedMDNode::clearOperands` was genuinely absent; it is
`NamedMetadataNode::clear_operands` plus `Module::named_metadata_clear_operands`
now. One further primitive the record did not name was also needed —
`mdconst::dyn_extract_or_null<GlobalValue>`, now
`Module::metadata_constant_global_value`.

The `assert`/`cast<>` sites inside the three upstream routines that are
reachable from parseable-but-malformed input have no defined upstream answer.
Each is read here as *upgrade nothing and keep the entry*, and each is
enumerated and pinned by
`parser_auto_upgrade.rs::malformed_nvvm_annotations_are_preserved_rather_than_upgraded`.

**Status 2026-08-16 (W13d).** `crates/llvmkit-ir/src/auto_upgrade.rs` now exists
under upstream's `lib/IR` layering and carries three of the nine call sites:
`UpgradeTBAANode` (fed by the parser's new `insts_with_tbaa_tag`, upstream's
`InstsWithTBAATag`), `UpgradeModuleFlags` and `UpgradeSectionAttributes`, each
wired at its own position in `Parser::parse_module_with_config`'s end-of-module
block. The **count is nine, verified again** — see the correction below. The six
that remained then (`UpgradeIntrinsicFunction`, `UpgradeIntrinsicCall`,
`UpgradeCallsToIntrinsic`, `llvm::UpgradeDebugInfo`, `UpgradeNVVMAnnotations`,
`copyModuleAttrToFunctions`) each have a named blocker recorded in
`docs/future-work.md` — the intrinsic trio needs the descriptor check moved out
of `parse_declare` / `resolve_direct_callee`, `UpgradeDebugInfo` needs
`StripDebugInfo`, `copyModuleAttrToFunctions` needs a `Triple`. The lifetime
divergence and the mis-citing test named in the correction below are **still
present**, unchanged. The rest of this entry is the pre-W13d record.

*IR / parser — end of module* — crates/llvmkit-ir/src/ (new `auto_upgrade.rs`, absent today), crates/llvmkit-asmparser/src/ll_parser.rs (end-of-module call sites)

- **LLVM:** `llvm/lib/IR/AutoUpgrade.cpp` is called from the parser's end-of-module path at eight sites: `UpgradeIntrinsicFunction`/`UpgradeIntrinsicCall`/`UpgradeCallsToIntrinsic` (generic arms include the `llvm.lifetime.start/end` size-argument drop, dbg intrinsics → `#dbg_*` records, `llvm.experimental.vector.*` → `llvm.vector.*`), `UpgradeDebugInfo`, `UpgradeTBAANode`, `UpgradeModuleFlags`, `UpgradeNVVMAnnotations`, `UpgradeSectionAttributes`, `copyModuleAttrToFunctions`.
- **llvmkit:** There is no `auto_upgrade.rs` in `crates/llvmkit-ir/src/` and no upgrade call sites, so legacy spellings that upstream silently modernises are either rejected or preserved unchanged. The lifetime size-argument form is noted as hit by "every clang-21 module".
- **Why:** Recorded as a resolved decision (2026-08-07, revised): **staged** — the generic parser-reachable upgrades port in-program at W13; the target-specific intrinsic rewrite bodies (the bulk of AutoUpgrade.cpp's 6,646 lines) plus `UpgradeARCRuntime`, `UpgradeBitCastInst/Expr`, `UpgradeInlineAsmString`, `UpgradeDataLayoutString` become a named ROADMAP milestone. Ledger rows satisfiable only by target-specific legacy input get `N/A(autoupgrade-milestone)`.
- **Fix:** Create `auto_upgrade.rs` under upstream's layering (lib/IR), extract `upgradeIntrinsicFunction1`'s non-target arms mechanically and diff back, wire the eight call sites, and port the generic `autoupgrade-*.ll` fixture families with UPSTREAM.md rows.
- **Correction from verification:** Substantively accurate; one count correction and one strengthening. (1) Count: LLParser::validateEndOfModule contains NINE AutoUpgrade call sites, not eight — the claim's own list names nine symbols (UpgradeIntrinsicFunction, UpgradeIntrinsicCall, UpgradeTBAANode, UpgradeCallsToIntrinsic, llvm::UpgradeDebugInfo, UpgradeModuleFlags, UpgradeNVVMAnnotations, UpgradeSectionAttributes, copyModuleAttrToFunctions). (2) Strengthening: the legacy lifetime size-argument form is not merely un-upgraded — llvmkit affirmatively REJECTS it, and pins that rejection in a test that mis-cites upstream. crates/llvmkit-asmparser/tests/parser_intrinsics.rs::intrinsic_signature_mismatch_is_rejected asserts `call void @llvm.lifetime.start.p0(i32 4, ptr %p)` fails with "intrinsic signature mismatch", under a doc comment citing `Intrinsics.td::int_lifetime_start` — but upstream's parser accepts that text and rewrites it via upgradeIntrinsicFunction1/upgradeIntrinsicCall. So the divergence is doubly recorded in the tree: absent machinery plus a test that encodes the wrong behavior as upstream-mirroring. Also note upstream's lifetime call-side arm does more than drop the size operand: when the pointer does not strip to an alloca it ERASES the marker entirely. Everything else in the claim verified as written, including the three generic arms (lifetime size drop, dbg intrinsics -> DbgVariableRecords, llvm.experimental.vector.* -> llvm.vector.*) and the cited paths.

<details><summary>Verification evidence</summary>

llvmkit (absent): directory listing of crates/llvmkit-ir/src/ shows no auto_upgrade.rs (nor any equivalent under another name). Case-insensitive grep for "upgrade" across crates/ returns only incidental prose: `upgradeMemoryAttr` in ll_parser.rs:9446 (legacy readonly/writeonly intersection at parse time, unrelated to AutoUpgrade), two rustdoc uses in ir_builder.rs:3406/3469 meaning runtime->compile-time check promotion, and verifier.rs:689 / error.rs:442 comments that merely describe what the bitcode reader would have upgraded (ModuleFlagLinkerOptionsUnsupported). No upgrade machinery and no call sites. crates/llvmkit-asmparser/src/ll_parser.rs:1406-1487 is `Parser::parse_module`, llvmkit's LLParser::Run + validateEndOfModule; its end-of-module block is purely resolution/validation — validate_forward_ref_types, metadata-slot definedness, resolve_deferred_global_initializers, resolve_deferred_block_addresses, resolve_deferred_personality_fns, resolve_deferred_alias_targets, validate_deferred_intrinsic_attribute_checks, validate_forward_ref_comdats, resolve_forward_ref_globals, validate_forward_function_decls, validate_end_of_index — with zero upgrade steps. No .md in the repo mentions AutoUpgrade as a subsystem; the only hits are UPSTREAM.md rows borrowing the fixture filename test/Assembler/auto_upgrade_nvvm_intrinsics.ll as provenance for unrelated builder tests. Upstream (present): orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp includes llvm/IR/AutoUpgrade.h and calls, all inside validateEndOfModule: UpgradeIntrinsicFunction, UpgradeIntrinsicCall, UpgradeTBAANode, UpgradeCallsToIntrinsic, llvm::UpgradeDebugInfo (gated on Run's UpgradeDebugInfo parameter), UpgradeModuleFlags, UpgradeNVVMAnnotations, UpgradeSectionAttributes, copyModuleAttrToFunctions. orig_cpp/.../llvm/lib/IR/AutoUpgrade.cpp is 6646 lines and contains all three named generic arms: upgradeIntrinsicFunction1 case 'l' matches lifetime.start/lifetime.end with F->arg_size() == 2 and re-declares on getArg(0)->getType(); the matching upgradeIntrinsicCall arm rebuilds via Builder.CreateLifetimeStart/End when the stripped pointer is an alloca and otherwise CI->eraseFromParent(); Name.consume_front("experimental.vector.") handles the vector renames; createUnresolvedDbgVariableRecord builds Declare/Value/Assign records from dbg intrinsics, with additional arms for dbg.addr -> dbg.value + DW_OP_deref and the old extra-offset dbg.value. orig_cpp/.../llvm/include/llvm/IR/Intrinsics.td defines int_lifetime_start/int_lifetime_end as DefaultAttrsIntrinsic<[], [llvm_anyptr_ty], ...> — a single operand — confirming the two-operand form llvmkit rejects is precisely the legacy spelling upstream silently modernises.

</details>

### 101. Six `LLLexer::LexError` sites are non-fatal upstream and fatal here — **NARROWED (W9)**

*lexer* — crates/llvmkit-asmparser/src/ll_lexer.rs (`lex_uint`, `LexError::IntegerOverflow64` / `IntegerOverflow128`)

Found 2026-08-16 while auditing `LexError`'s call sites for W14a; not previously recorded.

- **LLVM:** `LLLexer::LexError` *records* a message and returns; it does not
  end the lex. Eleven of upstream's seventeen call sites then return
  `lltok::Error` anyway, but six do not:
  - `LLLexer::LexUIntID` — `invalid value number (too large)`, then
    `UIntVal = unsigned(Val); return Token;`. `@4294967296` becomes `@0`.
  - `LLLexer::LexDigitOrNegative`'s numeric-label arm — the same message, then
    `return lltok::LabelID`.
  - `LLLexer::atoull` and `LLLexer::HexIntToVal` —
    `constant bigger than 64 bits detected` on wraparound, then `0` is
    returned as the value.
  - `LLLexer::HexToIntPair` and `LLLexer::FP80HexToIntPair` —
    `constant bigger than 128 bits detected` when hexits remain, then the
    truncated pair is used.

  The recorded message carries `ErrorPriority::Lexer`, so if the parse fails
  later for any reason it is the message reported. If the parse *succeeds*,
  `LLParser::Run` returns false and the message is discarded — the module is
  built from the truncated value with no diagnostic at all.
- **llvmkit:** two different shapes, neither upstream's.
  - The two `LexUIntID`-style sites are `Err(LexError::IdOverflow)`, which
    aborts the parse. Same text, different fatality: llvmkit rejects input
    `llvm-as` accepts (silently and wrongly).
  - `IntegerOverflow128` is declared and never constructed, because llvmkit's
    lexer stores the numeric lexeme and lets the parser decode it at the
    destination width rather than accumulating and watching for wraparound.
    Where upstream truncates and records, llvmkit either parses exactly or
    reports a different, narrower error (`bitwidth for integer type out of
    range` for `iN`).
- **Correction, 2026-08-16 (W14b).** The `LexDigitOrNegative` numeric-label
  bullet above was wrong about llvmkit twice over. It was not one of the "two
  `LexUIntID`-style sites": llvmkit had **no numeric-label token at all** —
  no `Token::LabelId`, no `lltok::LabelID` counterpart — so `42:` and `"42":`
  were the same `Token::LabelStr` and the too-large check had nothing to run
  on. `4294967296:` therefore built a basic block *named* `4294967296` and
  printed it back as `"4294967296":`, where `llvm-as` reports
  `invalid value number (too large)`. The parser reconstructed the
  distinction downstream instead, re-testing the label text for all-digits and
  peeking at the source byte under the span to tell a quoted `"42":` from a
  bare one. W14b added `Token::LabelId(u32)`, which is what the numeric arm
  now returns; `parse_function_body` matches it directly and both hacks are
  gone. `IntegerOverflow64` gained its first construction site there too — the
  `atoull` message for a label wider than 64 bits — so only
  `IntegerOverflow128` remains unconstructed. The fatality half of the entry
  is unchanged and still open: both messages abort the parse here and are
  recorded-and-ignored upstream.
- **Why:** not a decision — nobody had compared the `LexError(...)` call sites
  against which of them also `return lltok::Error`. W14a's split of "the lexer
  writes a message" from "the lexer forms no token" is what made the two
  groups visible, and it is also what surfaced the two dead variants.
- **Narrowed 2026-08-29 (W9).** The unconstructed-variant half is closed:
  `LexError::IntegerOverflow128` is deleted. It had no construction site —
  `grep -rn -a 'LexError::IntegerOverflow128' crates/` returned exactly one
  line, the `span()` accessor's own match arm — and llvmkit performs neither
  `HexToIntPair`'s nor `FP80HexToIntPair`'s accumulate-and-detect-wraparound to
  give it one. Two claims found wrong while closing it, both in llvmkit's own
  tree rather than in this entry: `ll_lexer.rs`'s module comment still said
  `IntegerOverflow64` was unreachable, which W14b's own correction to this
  entry had already contradicted; and the same comment's "`LLLexer` produces an
  `lltok::Error` at twenty-one places … ten record a message" split never
  matched the enum it described, since a wording several call sites share is one
  variant here. Both are rewritten without the numerals.
- **Fix (the fatality half, unchanged):** needs the error-*retention* model
  upstream has and llvmkit does not: a single recorded diagnostic with a
  priority, consulted only if the parse fails. `LLLexer::Error` is
  `if (Priority < ErrorInfo.Priority) return; ErrorInfo.Error = SM.GetMessage(...)`,
  writing straight into the `SMDiagnostic &` the caller passed, and
  `LLParser::Run` never consults it — the report-or-discard decision is
  structural, in `parseAssembly`'s caller, which prints only when `Run` failed.
  llvmkit has no counterpart: `Lexer::next_token` returns
  `Result<Spanned<Token>, LexError>` and `ParseError` is a value returned by
  `?`, with no sink anywhere (`grep -rn -a 'record_error\|Vec<Diagnostic\|DiagnosticSink\|HaveError' crates/`
  finds only prose about upstream). That is the same missing machinery entry 32
  needs, and the two should land together. Reproducing the truncation without it
  would trade a wrong rejection for a silent wrong value, which is worse.

## Accepts invalid input

llvmkit accepts IR that LLVM rejects, so a malformed module survives into the rest of the pipeline.

### 132. `Verifier::visitIntrinsicCall`'s signature split, its `MetadataAsValue` walk and all but one arm of its `switch` are unported — **NARROWED (W9)**

*verifier — call family* — crates/llvmkit-ir/src/verifier.rs (`check_intrinsic_call`)

Found 2026-08-27 while porting `Verifier::visitCallBrInst` (former entry 128):
`test/Verifier/callbr.ll`'s four `llvm.callbr.landingpad` functions could not
be ported with the rest of the fixture, because the routine that answers them
did not exist here.

**Narrowed 2026-08-29 (W9).** Three of those four now run, from
`crates/llvmkit-asmparser/tests/parser_calls.rs::upstream_callbr_landingpad_fixture_messages_match`.
`check_intrinsic_call` carries the preamble's `Intrinsic functions should never
be defined!`, its `Intrinsic name not mangled correctly for type arguments!
Should be: …`, the `const x86_amx is not allowed in argument!` half of its
argument walk, and the `switch`'s `case Intrinsic::callbr_landingpad:` arm
whole. What is left is below.

- **The `matchIntrinsicSignature` split, and the name-mangling `Check` behind
  the same gate.** Upstream turns one `MatchIntrinsicTypesResult` into
  `Intrinsic has incorrect return type!` and `Intrinsic has incorrect argument
  type!`, then runs `Intrinsic::matchIntrinsicVarArg` for `Intrinsic was not
  defined with variable arguments!` / `Callsite was not defined with variable
  arguments!`, then `Check(TableRef.empty(), "Intrinsic has too few
  arguments!")`, then `Check(ExpectedName == IF->getName(), "Intrinsic name not
  mangled correctly for type arguments! Should be: " + ExpectedName)`.

  **The root blocker is not the message split, it is which calls llvmkit treats
  as intrinsic calls.** `intrinsics::descriptor_for_callee` derives the
  descriptor from the callee's *name* and then requires the resulting
  `FunctionType` to equal the callee's signature; when it does not it answers
  `None`, and `check_intrinsic_call` returns before any `Check` runs. Upstream
  has no such gate — `Function::getIntrinsicID()` is a name lookup, so a callee
  whose types disagree with its name is still an intrinsic call and reaches
  every one of the six messages above. So none of them can fire here, whatever
  `match_intrinsic_signature` is reshaped to say, and the mangling `Check` is
  deliberately *not* ported rather than ported dead — the site says so. Behind
  that, `intrinsics::match_intrinsic_signature` still collapses everything into
  one `Intrinsic called with incompatible signature`, and llvmkit's parser
  reports its own `expected intrinsic signature mismatch` before the verifier
  ever runs — which is what `docs/fixture-coverage.md`'s
  `autoupgrade-invalid-name-mangling.ll` row records against gap **G1**. It has
  no `MatchIntrinsicTypesResult`, no
  `DeferredIntrinsicMatchPair` list, and re-derives the whole function type and
  compares where upstream leaves a cursor for `matchIntrinsicVarArg` to
  consume. The builder calls it too, so a second copy shaped for the verifier
  would be the defect rather than the fix.

  `test/Verifier/callbr.ll`'s `@callbrpad_bad_type` is the fixture this blocks;
  `upstream_callbr_label_constraint_fixture_messages_match` says so at the site.
- **`test/Verifier/x86_amx9.ll` cannot be loaded**, though the `Check` it pins
  now exists. Its argument is `x86_amx bitcast (<256 x i32> … to x86_amx)`, and
  llvmkit's parser answers `invalid bitcast constant expression` for a
  `bitcast` to `x86_amx`, so the module never reaches the verifier.
  `verifier_basic.rs::const_x86_amx_intrinsic_argument_is_rejected` builds an
  `undef` of that type instead and says why.
- **`visitMetadataAsValue` on every `MetadataAsValue` argument.** The other half
  of the argument walk, and a whole routine of its own — it is what rejects an
  `MDNode` local to another function.
  `rg -n "visit_metadata_as_value|MetadataAsValue" crates/llvmkit-ir/src/verifier.rs`
  returns nothing.
- **Every `switch (ID)` arm except `Intrinsic::callbr_landingpad`** — `llvm.assume`'s
  operand bundles, the `experimental.gc.*` family, the constrained-FP family,
  and the rest. Each needs the `test/Verifier` fixture that pins it.
- **`define`-ing an intrinsic is a *parse* error here.** Upstream's `LLParser`
  accepts `define void @llvm.donothing() { ret void }` and leaves the verdict to
  `visitIntrinsicCall`'s `Intrinsic functions should never be defined!`, raised
  per call site — so a defined-but-uncalled intrinsic verifies clean upstream.
  llvmkit's `parse_define` refuses the name outright, in a message of its own
  (`expected intrinsic functions should never be defined`). The verifier rule is
  ported and reachable through the builder, which is where
  `verifier_basic.rs::defined_intrinsic_called_from_a_body_is_rejected` drives
  it; the parser half is the residue. A second llvmkit-invented copy of the same
  rule lived in `visit_function` as a lowercase `InvalidOperation` and was
  deleted with this narrowing — it shadowed the port and printed text upstream
  never emits.
- **Two checks are in the wrong routine.** llvmkit's collapsed signature check
  and its `immarg operand has non-immediate parameter` loop live in
  `check_intrinsic_call`; upstream has both in `visitCallBase`, the first
  *before* the `swifterror` loop and the second inside the same per-argument
  loop as it. `visitIntrinsicCall` is called after both. So on a module with two
  faults llvmkit can report the second where upstream reports the first. Found
  while porting the preamble, by reading where upstream raises each message
  rather than which message it raises.

### 23. `Verifier::verifyFunctionAttrs` is unported, so two attribute-level `swifterror` rules do not fire — **NARROWED**

*verifier* — crates/llvmkit-ir/src/verifier.rs (no counterpart routine; the module header lists "Per-function attribute coherence rules … are out of scope")

**Narrowed 2026-08-27.** The use-site half of this entry is closed:
`Verifier::verifySwiftErrorValue` and `Verifier::verifySwiftErrorCall` are
ported, `check_alloca` and `visit_function`'s parameter loop call the first
where `visitAllocaInst` and `visitFunction` do, and the three-`Check`
`swifterror` loop of `visitCallBase` runs from `check_call` and
`check_invoke`. `test/Verifier/swifterror.ll`'s four `define`s are driven by
`crates/llvmkit-asmparser/tests/parser_calls.rs::upstream_swifterror_fixture_messages_match`.
What is left is that fixture's last two lines, both `declare`s.

- **LLVM:** `Verifier::verifyFunctionAttrs` raises `Cannot have multiple
  'swifterror' parameters!` when two parameters carry the attribute, and
  `Verifier::verifyParameterAttrs` — which it calls — raises `Attribute
  '<name>' applied to incompatible type!` when one sits on a non-pointer.
  `declare void @a(ptr swifterror %a, ptr swifterror %b)` and
  `declare void @b(i32 swifterror %a)` are the fixture's two cases, and
  `test/Verifier/swifterror2.ll` (`declare swifterror void @c(ptr swifterror
  %a)` → `this attribute does not apply to return values`) is a third from the
  same routine.
- **llvmkit:** neither routine exists. `grep -rn -a -E "Cannot have
  multiple|applied to incompatible type" crates/llvmkit-ir/src/` matches one
  line and `grep -rn -a -E "verifyFunctionAttrs|verifyParameterAttrs"
  crates/llvmkit-ir/src/` matches two, and every one of the three is prose: the
  first two are `verifier.rs`'s module header naming what is out of scope, the
  third a comment in `constant_range_list.rs` naming `verifyParameterAttrs` as
  the place an empty `initializes` list is rejected. None of the three
  `declare`s is rejected.

  *(Both commands used to be quoted here as returning "nothing" and "exactly one
  line". Neither is true any more — the W6 re-scope that narrowed this entry
  added the module-header lines the searches now match. The conclusion is
  unchanged and the counts were re-derived at W9; the wording was not, which is
  why it is restated as a match count and a reading of what matched.)*
  [[verify-recorded-premises]]
- **Why:** `verifyFunctionAttrs` and `verifyParameterAttrs` between them cover
  every function and parameter attribute — `sret`, `byval`, `byref`,
  `inalloca`, `nest`, `returned`, `preallocated`, alignment, the memory-effect
  coherence rules — with dozens of arms. Porting them for the two `swifterror`
  `Check`s alone would have been a half-port of a routine whose other arms are
  missing for exactly the same reason.
- **Fix:** port `Verifier::verifyFunctionAttrs` and `Verifier::verifyParameterAttrs`
  whole, and drive `test/Verifier/swifterror.ll`'s two `declare`s and
  `test/Verifier/swifterror2.ll` from them. The entry is deliberately scoped to
  the routines rather than to `swifterror`: closing it for `swifterror` alone
  would leave the same gap under a different attribute name.

### 103. `exnref` is a type keyword llvmkit has and LLVM 22.1.4 does not

*lexer / IR model* — crates/llvmkit-asmparser/src/ll_lexer/keywords.rs (`classify_word`), crates/llvmkit-asmparser/src/ll_token.rs (`PrimitiveTy::WasmExnRef`), crates/llvmkit-ir/src/type.rs (`TypeData::WasmExnRef`), crates/llvmkit-tablegen/src/lib.rs (`CUSTOM_IIT_WASM_EXNREF`)

Found 2026-08-16 by W14b's mechanical diff of `LLLexer.cpp`'s tables; the one
spelling the two sides disagree on, and the reason the W14 inventory's earlier
diff missed it is that it never expanded `Attributes.inc` and read the whole
llvmkit-only remainder as attribute keywords.

- **LLVM:** there is no `exnref` in the `.ll` grammar.
  `LLLexer::LexIdentifier`'s `TYPEKEYWORD` table holds thirteen spellings and
  `exnref` is not among them, `Type::TypeID` has no case for it, and
  `AsmWriter` can never print it. The name exists only in TableGen:
  `include/llvm/CodeGen/ValueTypes.td` declares `def exnref : ValueType<0>`
  and `IntrinsicsWebAssembly.td` uses `llvm_exnref_ty` for the
  `int_wasm_ref_null_exn` / `table_*_exnref` family. That is itself
  inconsistent upstream — `Intrinsics.td` gives no `IIT_VT<exnref>`, so
  `LLVMType<exnref>`'s `IITs` filter yields the empty list and the type
  contributes nothing to the intrinsic's encoded signature. Upstream `.ll`
  spells these reference types as address-space pointers instead
  (`%externref = type ptr addrspace(10)` in `test/CodeGen/WebAssembly`).
- **llvmkit:** models the type. `llvmkit-tablegen` assigns
  `CUSTOM_IIT_WASM_EXNREF = 54`, immediately past upstream's `IIT_V4096 = 53`,
  so the WebAssembly intrinsic signatures come out well-formed instead of
  silently losing an operand type; `llvmkit-ir` carries `TypeData::WasmExnRef`
  with `Display` printing `exnref`; and the lexer matches `exnref` as a
  `TYPEKEYWORD` so that printed output re-parses. `declare exnref
  @llvm.wasm.ref.null.exn()` therefore parses here and does not upstream.
- **Why:** deliberate, and kept as a unit. Dropping the *keyword* alone would
  leave `llvmkit-ir` printing a type `llvmkit-asmparser` cannot read — a
  print-but-not-parse hole in the parser/printer contract, which is worse than
  the extension. Dropping the *type* means either remapping the WebAssembly
  exception intrinsics onto `externref`/`funcref` (wrong signatures) or
  reproducing upstream's dropped-operand encoding (wrong for a different
  reason). Neither is a lexer decision.
- **Fix:** if it is to be closed, it closes in `llvmkit-tablegen` and
  `llvmkit-ir` first — pick a representation for the WebAssembly exception
  reference that upstream also has (an address-space pointer, or a target
  extension type), then remove `PrimitiveTy::WasmExnRef` and the keyword. The
  keyword is the last thing to go, not the first.
- **Guarded by:** `crates/llvmkit-asmparser/tests/lexer_token_drift.rs`'s
  `NON_UPSTREAM_KEYWORDS`, which is the only permitted way to spell an
  llvmkit-only keyword; `the_extension_list_has_no_stale_entries` retires the
  entry automatically if a later LLVM adopts the spelling.
- **Assessed for removal 2026-08-29 (W9); kept.** The exit plan above was
  followed as far as picking the replacement representation, and there is none
  that is not worse. `include/llvm/CodeGen/ValueTypes.td` and
  `IntrinsicsWebAssembly.td` are the same spelling upstream uses, so there is no
  other name to adopt: `grep -rn -a -i 'exnref' orig_cpp/…/llvm/include/ orig_cpp/…/llvm/lib/`
  at `llvmorg-22.1.4` finds it only under `BinaryFormat/Wasm.h`,
  `CodeGen/ValueTypes.{td,cpp}`, the two `.td` files above, `lib/Object`,
  `lib/ObjectYAML` and `lib/Target/WebAssembly` — nothing in `lib/IR`,
  `lib/AsmParser` or `lib/Bitcode`, and nothing in `include/llvm/IR/Type.h`. So
  the type exists upstream at the MVT/MC layer and is simply unreachable from
  `Type`. Remapping the exception-reference onto `externref`/`funcref` gives the
  WebAssembly intrinsics the wrong signatures; a target extension type is a
  different type with different rules; and reproducing upstream's own
  encoding — `LLVMType<exnref>`'s `IITs` filter yields the empty list, because
  `Intrinsics.td` has `IIT_EXTERNREF` and `IIT_FUNCREF` and no `IIT_VT<exnref>`,
  so the type contributes *nothing* to the signature — means porting a latent
  upstream bug into `llvmkit-tablegen`. The keyword is the last thing to go and
  nothing has moved ahead of it, so this stays an extension. Recorded here so
  the next wave does not re-derive the same three dead ends.

## Different diagnostic text

Same verdict, different wording. Upstream's text is contractual, including its own inconsistencies.

### 130. `test/Verifier/AmbiguousPhi.ll` is answered by the builder, not the verifier — **NARROWED (W11)**

*IR builder / diagnostics* — crates/llvmkit-ir/src/instructions.rs (`PhiInst::add_incoming` and its two siblings), crates/llvmkit-ir/src/ir_builder.rs (`make_phi_in_block`)

Found 2026-08-27 while porting `test/Verifier` fixtures by message text (former
entry 121). Two halves were recorded; the second is closed.

- **Closed half (W11) — the block name.** The message used to name `%4`, an
  internal arena index from `LlvmContext::block_diag_name`'s
  `id.arena_index().to_string()` fallback. That routine is gone. Every phi
  diagnostic now goes through `asm_writer::block_slot_label`, which resolves the
  block's owning function and asks `SlotTracker::for_function` — the same
  routine the verifier's `slot_label` already used, and the same answer
  `Verifier::CheckFailed` gets from `WriteAsOperand` → `Machine.getLocalSlot`.
  The vendored fixture's implicit entry block is now named `%0`, matching both
  upstream and the `%0` the fixture's own `phi` operands are written with.
  Pinned by `crates/llvmkit-ir/src/phi_raw_tests/fmf.rs::ambiguous_phi_names_the_block_asm_writer_prints`
  (unnamed block → `SlotTracker` number, cross-checked against printed IR) and
  `::ambiguous_phi_names_a_named_block_by_its_name` (written name used
  verbatim). Those two are the gap that hid the defect: every prior test on
  this error matched only the `IrError` variant, never the rendered text.
- **LLVM:** a phi with two differing entries for the same predecessor parses
  cleanly and is rejected by `Verifier::visitBasicBlock` —
  `PHI node has multiple entries for the same basic block with different incoming
  values!`
- **llvmkit:** `add_incoming` refuses the second entry at the *builder* call site
  with `IrError::AmbiguousPhiIncoming`, which the parser surfaces as
  `expected valid phi.add_incoming: phi already has an entry for block %0 with a
  different value`. Same verdict, wrong layer, and a message upstream never
  prints — the `VerifierRule::AmbiguousPhi` that would print upstream's literal
  is unreachable from parsed text.
- **Why:** the layer choice is deliberate and documented on the error variant —
  "enforced at the edge-add call site rather than deferred to `Module::verify`'s
  `AmbiguousPhi` rule", citing `llvm/llvm-project#196954`.
- **What is blocked by it:** `test/Verifier/AmbiguousPhi.ll` is vendored at
  `crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/AmbiguousPhi.ll` and
  cannot be driven through the parser.
  `crates/llvmkit-asmparser/tests/parser_function_body.rs::upstream_ambiguous_phi_fixture_is_rejected_by_the_builder`
  asserts *this* entry's diagnostic, so the day the remaining half closes the
  test fails and the real port replaces it. The verifier rule's own text is
  asserted separately, against the same fixture's `CHECK` line, by
  `crates/llvmkit-ir/src/verifier.rs::ambiguous_phi_duplicate_predecessor`.
- **Fix:** the same shape as entry 26, and it wants the same decision: a builder
  that records what was written and lets `Module::verify` judge it, rather than
  refusing at the edge.

> **Evidence block removed 2026-08-29 (W11).** It was a 2026-08-27 snapshot of
> the closed half: it quoted `LlvmContext::block_diag_name`, which no longer
> exists, and the `%4` the fixture no longer produces. Its upstream half —
> `Verifier::visitBasicBlock` carries the `Check`, and `Verifier::CheckFailed`
> renders each value through `WriteAsOperand`/`Machine.getLocalSlot` — is
> restated by symbol in the bullets above.

### 26. A misplaced `phi` is rejected by the parser, with a message upstream never prints

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:11125-11133 (the `seen_non_phi` guard)

- **LLVM:** `LLParser` accepts a `phi` written after a non-phi instruction and lets `Verifier::visitPHINode` reject it with `PHI nodes not grouped at top of basic block!`.
- **llvmkit:** `parse_basic_block` tracks a `seen_non_phi` flag and rejects at parse time with `phi must be grouped at the top of its basic block`. Same verdict, wrong layer, and a message upstream never prints.
- **Why:** Recorded, with an explicit "do not fix this by deleting the parser check": every phi llvmkit builds goes through `IrBuilder::make_phi_in_block` → `BasicBlock::insert_instruction_at_phi_head`, which places the phi at the block's phi head regardless of the insertion point. Drop the parse check and a misplaced phi is *silently hoisted* into a legal position, so llvmkit's own `VerifierRule::PhiNotAtTop` never fires — accepting invalid IR and quietly rewriting it, strictly worse than the current strictness.
- **Fix:** Add a non-hoisting insertion path for parsed phis so the instruction lands where it was written, then delete the parse-time check and let `VerifierRule::PhiNotAtTop` deliver upstream's verdict and wording. That is entangled with llvmkit's head-phi design — block parameters are operandless head-phis per `IrBuilder::append_block_with_params`, and `insert_instruction_at_phi_head` is the only phi insertion path today — so it wants deciding alongside that model rather than as a parser patch.
- **Correction from verification:** Accurate, with two refinements. (1) The guard now spans lines 11125-11134 (the claim cited 11125-11133; the `else { seen_non_phi = true; }` arm closes at 11134). (2) The message is emitted via `self.expected(...)`, whose ParseError variant renders as `#[error("expected {expected}")]`, so the actual user-visible string is the ungrammatical `expected phi must be grouped at the top of its basic block`, not the bare production the claim quotes. Additional context strengthening the claim: llvmkit already carries the same rule in its verifier (crates/llvmkit-ir/src/verifier.rs:1036-1051, VerifierRule::PhiNotAtTop), so the parse-time guard is strictly redundant with the correct layer, and it prevents a misplaced phi from ever reaching that rule. Worth noting the guard is deliberate, not an oversight: the in-source comment and the test doc both state the rationale (the auto-hoisting phi builders would silently reorder a misplaced phi into valid position, laundering ill-formed .ll into valid IR).
- **Narrowed, and a duplicate merged.** Two halves used to be recorded here. The *message-text* half — `VerifierRule::PhiNotAtTop` rendering `PHI nodes not grouped at top of block`, dropping upstream's `basic` and its `!` — is closed: the rule now carries `PHI nodes not grouped at top of basic block!` in `IrError::VerifierFailure`'s `message`, asserted by `crates/llvmkit-ir/src/verifier.rs::phi_not_at_top` against the vendored fixture's own `CHECK` text. Only the *wrong-layer* half above is left. This entry also absorbed a second entry that recorded the same divergence in the same terms (former **35**, "A misplaced `phi` is rejected at parse time, not by the verifier"); nothing distinguished the two, and the duplicate was deleted rather than left to be closed twice.
- **What is blocked by it:** `test/Verifier/PhiGrouping.ll` is vendored at `crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/PhiGrouping.ll` and cannot be driven through the parser. `crates/llvmkit-asmparser/tests/parser_function_body.rs::upstream_phi_grouping_fixture_is_rejected_at_parse_time` asserts *this* entry's parse diagnostic instead, so the day the entry closes the test fails and the real port replaces it.

<details><summary>Verification evidence</summary>

llvmkit: C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs — `let mut seen_non_phi = false;` at 10956 (set at 11005, 11013, 11094, 11133) and the guard at 11125-11134: `if matches!(opcode, Opcode::Phi) { if seen_non_phi { return Err(self.expected("phi must be grouped at the top of its basic block")); } } else { seen_non_phi = true; }`. Confirmed also in HEAD (2ac3e3a) via `git show HEAD:...` at lines 11121-11125, so it is not a working-tree-only artifact. Message rendering: crates/llvmkit-asmparser/src/parse_error.rs:171-175 (`#[error("expected {expected}")]` on `ParseError::Expected`). Pinned by test `phi_after_non_phi_is_a_parse_error` at crates/llvmkit-asmparser/tests/parser_errors.rs:77-105, asserting the message contains "phi must be grouped at the top". Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp — `LLParser::parseBasicBlock` (declared at 7050) has no ordering state in its instruction loop, and `LLParser::parsePHI` (8314) rejects only `phi node must have first class type` plus bracket-syntax errors; nothing about placement. orig_cpp/.../llvm/lib/IR/Verifier.cpp:3808-3815 — `Verifier::visitPHINode` holds the check: `Check(&PN == &PN.getParent()->front() || isa<PHINode>(--BasicBlock::iterator(&PN)), "PHI nodes not grouped at top of basic block!", &PN, PN.getParent());`. llvmkit's counterpart verifier rule: crates/llvmkit-ir/src/verifier.rs:1035-1051 and crates/llvmkit-ir/src/error.rs:474.

</details>

### 32. Attribute-group reference error is fatal here, non-fatal upstream

*parser (attribute groups)* — crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:899-935

- **LLVM:** `LLParser::parseUnnamedAttrGrp` reports `cannot have an attribute group reference in an attribute group` non-fatally — it keeps parsing and accumulates further diagnostics.
- **llvmkit:** llvmkit reports the first and stops. The message text matches; the recovery does not.
- **Superseded, 2026-08-16 (W14a):** the second half of this entry — that a misspelled keyword is intercepted by llvmkit's lexer, so `unterminated attribute group` had to be triggered with a type keyword, an integer and end-of-file — is closed. A misspelled keyword now arrives as `Token::Error` and is a fourth trigger in `attribute_group_diagnostics_match_upstream_text`. The evidence block below predates that and says otherwise. (Separately: no upstream `.ll` pins `unterminated attribute group` at all, so "upstream's trigger" was itself a wrong premise.)
- **Why:** Recorded — "the same choice already recorded for the position diagnostics", i.e. llvmkit's parser is fail-fast by design where upstream accumulates.
- **Fix:** Either accept it as a documented global policy (then say so once, in `AGENTS.md`, rather than per-test) or give `ParseError` an accumulating mode so multi-diagnostic fixtures can be ported whole.
- **Correction from verification:** The divergence is real and still present, but one phrase is imprecise: upstream does NOT "accumulate further diagnostics" in the sense of retaining a list. `LLLexer::ErrorInfo` (LLLexer.h) holds exactly one `SMDiagnostic &Error` plus an `ErrorPriority`, and `LLLexer::Error` is `if (Priority < ErrorInfo.Priority) return; ErrorInfo.Error = ...`. Every `LLParser::error` goes through `Lex.ParseError` at `ErrorPriority::Parser`, so an equal-priority later error is NOT less-than and therefore OVERWRITES the earlier one. `HaveError` is only a bool that forces the eventual `return true`. Corrected statement: `LLParser::parseFnAttributeValuePairs` reports `cannot have an attribute group reference in an attribute group` non-fatally (`HaveError |= error(...)`, then `Lex.Lex(); continue;`) and keeps parsing the rest of the group, so the message the user finally sees is the LAST parser diagnostic the group produced, not this one. llvmkit returns `Err` at the first `#N` and stops, so it always reports THIS message. The consequence is sharper than "the recovery does not match" — it is an observable message-text divergence on inputs with a second fault: for `attributes #0 = { #1 i32 }` upstream ends at `unterminated attribute group` (the `Attr == Attribute::None` + `InAttrGrp` arm overwrites), while llvmkit reports `cannot have an attribute group reference in an attribute group` (confirmed empirically). Two lesser notes: (a) the test actually spans lines 899-955, not 899-935 — the cited range stops mid-test after the fifth assert; (b) the doc's "upstream's fixture spells it with a misspelled keyword" is not backed by a fixture I could locate — `test/Assembler/invalid-attrgrp.ll` is the only attrgrp fixture and it pins only `expected attribute group id`; no `.ll` under `test/Assembler` pins `unterminated attribute group` or the attrgrp-reference message at all. The *mechanism* described is nonetheless correct.

<details><summary>Verification evidence</summary>

UPSTREAM (C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp, `LLParser::parseFnAttributeValuePairs`): the `Token == lltok::AttrGrpID` arm reads `if (InAttrGrp) { HaveError |= error(Lex.getLoc(), "cannot have an attribute group reference in an attribute group"); } else { FwdRefAttrGrps.push_back(...); } Lex.Lex(); continue;` — non-fatal, loop continues. Contrast the fatal sibling four lines later: `if (Attr == Attribute::None) { if (!InAttrGrp) break; return error(Lex.getLoc(), "unterminated attribute group"); }`. The function ends `return HaveError;`, and `LLParser::parseUnnamedAttrGrp` only propagates that. UPSTREAM error retention (LLLexer.h lines ~35-46 and LLLexer.cpp `LLLexer::Error`): `enum class ErrorPriority { None, Parser, Lexer }`; `struct ErrorInfo { ErrorPriority Priority; SMDiagnostic &Error; }` — a single diagnostic slot. `void LLLexer::Error(LocTy, const Twine &Msg, ErrorPriority Priority) { if (Priority < ErrorInfo.Priority) return; ErrorInfo.Error = SM.GetMessage(...); ErrorInfo.Priority = Priority; }`. `LLParser.h:218` is `bool error(LocTy L, const Twine &Msg) { return Lex.ParseError(L, Msg); }`, and `LLLexer.h:99` shows `ParseError` calls `Error(ErrorLoc, Msg, ErrorPriority::Parser)`. Parser-vs-Parser is not `<`, so later parser errors overwrite — last-wins, not accumulate. LLVMKIT (C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:9463-9467), inside `parse_fn_attribute_value_pairs`: Token::AttrGrpId(_) if context == AttrListContext::AttributeGroup => { return Err(self.message( "cannot have an attribute group reference in an attribute group", )); } A bare `return Err` — fatal, before any further token is consumed. Message text matches upstream exactly; the control flow does not. TEST (C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:899-955): `attribute_group_diagnostics_match_upstream_text`. Its doc comment at 899-907 states the divergence in the project's own words ("is non-fatal upstream (it keeps parsing and accumulates); llvmkit reports the first and stops"). The cited assert is at 928-931. `cargo +1.96.0 test --release -p llvmkit-asmparser --test parser_attribute_matrix attribute_group_diagnostics_match_upstream_text` passes (1 passed), so the pinned behavior is current, not stale. EMPIRICAL (temporary probe test, since removed): llvmkit returns `cannot have an attribute group reference in an attribute group` for all of `attributes #0 = { #1 i32 }`, `{ #1 #2 }`, and `{ #1 nounwind }` — proving it stops at the first `#N` regardless of what follows. Upstream's loop would reach `i32` (lexed `lltok::Type`, `tokenToAttribute` -> `Attribute::None`, `InAttrGrp` true) and overwrite with `unterminated attribute group`. LEXER SUB-CLAIM CONFIRMED. Upstream LLLexer.cpp `LexIdentifier` ends `// Finally, if this isn't known, return an error. CurPtr = TokStart+1; return lltok::Error;` — a silent token kind carrying no message, which flows into the `Attribute::None` arm and yields `unterminated attribute group`. llvmkit instead fails in the lexer: ll_lexer.rs:128-131 defines `UnknownTokenReason::UnknownKeyword` with `#[error("unknown keyword '{word}'")]`, and the doc comment at ll_lexer.rs:95-103 says outright "**This has no upstream counterpart, deliberately.** `LLLexer` returns a bare `lltok::Error` at every one of these sites and records no message". Probe confirms: `attributes #0 = { nounwindd }` yields `unknown keyword 'nounwindd'`, never reaching `parse_fn_attribute_value_pairs`. That is exactly why the test substitutes a type keyword (`i32`), an integer (`42`), and EOF as triggers.

</details>

### 34. A metadata field accepts values its `parseMDField` overload rejects (plan divergence #2, residual)

**Severity:** accepts-invalid

*metadata parser* — crates/llvmkit-asmparser/src/ll_parser.rs (`check_metadata_field_value`)

- **Closed half (W14a):** the diagnostic half of this entry is gone. A word
  matching no keyword now lexes as `Token::Error` and reaches the field
  parser, so `emissionKind: Bogus` / `nameTableKind: Bogus` /
  `!DIFixedPointType(kind: Bogus)` answer `expected emission kind` /
  `expected nameTable kind` / `expected fixed-point kind`, and the sibling
  `DW_*` families answer their own `expected …` the same way. Pinned by
  `parser_debug_metadata.rs::exact_word_kind_families_reject_an_unknown_spelling`
  and `::dwarf_kind_families_reject_a_word_that_is_no_keyword`.
- **LLVM:** each `LLParser::parseMDField` overload is *typed*: the token must
  be `lltok::APSInt` or the one kind that overload wants. Anything else —
  `null`, a string, a metadata reference — is `expected <family>`, and an
  integer past the field's `Max` is `value for '<name>' too large, limit is
  <max>`.
- **llvmkit:** the value is parsed first and validated afterwards, and
  `check_metadata_field_value`'s `keyword` closure only rejects an `Enum`
  spelling its table does not carry (`_ => Ok(())` for everything else), with
  no `max` on the kind families. So `emissionKind: null`, `emissionKind: "x"`,
  `nameTableKind: null` and `emissionKind: 99` all parse and round-trip, where
  upstream rejects the first three with `expected emission kind` /
  `expected nameTable kind` and the fourth with `value for 'emissionKind' too
  large, limit is 3` (`EmissionKindField : MDUnsignedField(0,
  DICompileUnit::LastEmissionKind)`).
- **Why:** parse-then-validate is llvmkit's own shape for `PARSE_MD_FIELDS`,
  and narrowing what each field *accepts* changes round-tripping, not just
  wording. `expected_for_metadata_field_kind` (`ll_parser.rs`) already carries
  the per-family phrase for the no-value-at-all case; the acceptance rule is
  the part still outstanding.
- **Fix:** make `parse_metadata_field_value` typed the way upstream's
  overloads are — accept only `APSInt` plus the family's own token, and route
  the range check through `MDUnsignedField`'s `Max` for the kind families
  rather than leaving it unbounded.
- **Note on `ChecksumKindField`:** upstream's is the one overload that reports
  `invalid checksum kind '<Lex.getStrVal()>'` even when the *token kind* is
  wrong, so on an error token it quotes whatever the previous token left in
  `StrVal`. llvmkit carries no stale `StrVal` and cannot reproduce that
  string; it answers `expected metadata field value` instead.

<details><summary>Verification evidence</summary>

Built and ran C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/examples/parse_file.rs on `+1.96.0 --release` against hand-written fixtures: - `emissionKind: Bogus` -> `unknown keyword 'Bogus'` (upstream: `expected emission kind`) - `nameTableKind: Bogus` -> `unknown keyword 'Bogus'`; `!DIFixedPointType(kind: Bogus)` -> `unknown keyword 'Bogus'` - `emissionKind: DW_TAG_class_type` -> `invalid emission kind 'DW_TAG_class_type'` (upstream: `expected emission kind`) - `emissionKind: null`, `emissionKind: "x"`, `emissionKind: 99`, `nameTableKind: null` -> ALL parsed successfully and printed back verbatim. Source read: - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_lexer.rs lines 1060-1067 (`classify_prefixed` word lists) and 980-990 (the fallthrough `Err(LexError::UnknownToken { UnknownKeyword })`, with a comment noting it mirrors upstream's cursor rewind); lines 94-101 document the deliberate departure: upstream "returns a bare `lltok::Error` at every one of these sites and records no message". - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs lines 5652-5666 (`keyword` closure -> `ParseError::InvalidMetadataFieldValue`, `_ => Ok(())` for non-Enum), 5770-5773 (the three arms), 5865-5881 (`parse_metadata_field_value` token arms), 15388-15432 (the `emission_kind` / `name_table_kind` / `fixed_point_kind` tables). - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/parse_error.rs lines 299-309: `#[error("invalid {what} '{value}'")]`. Repo-wide grep finds zero producible `expected emission kind` / `expected nameTable kind` / `expected fixed-point kind` strings. - Upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp lines 5082-5135 (the three `parseMDField` overloads: APSInt fallback, then `Lex.getKind() != lltok::X` -> `tokError("expected ... kind")`) and 4798-4812 (`EmissionKindField`/`FixedPointKindField`/`NameTableKindField` Max bounds). - Upstream LLLexer.cpp `LexIdentifier`: identical word lists, then `// Finally, if this isn't known, return an error. CurPtr = TokStart+1; return lltok::Error;`. - Upstream lib/IR/DebugInfoMetadata.cpp lines 928-934, 1255-1272: the StringSwitch cases equal the lexer word lists, proving upstream's `invalid ... kind` arms are dead. Also in-tree: C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/parser_debug_metadata.rs lines 697-715 already records this divergence in a doc comment, but only for the unknown-spelling case, and its body is `let _ = parse_err(src);` — it asserts nothing about which error, so it would not catch items (3) or (4).

</details>

### 36. Global and alias forward references resolve in one end-of-module sweep, not per definition site — **NARROWED (W8)**

*parser — forward references* — crates/llvmkit-asmparser/src/ll_parser.rs — the `forward_ref_globals` guard in `parse_global`, `resolve_forward_ref_globals`

**Status 2026-08-28 (W8).** The **function** half is closed:
`claim_function_forward_ref` is a port of `parseFunctionHeader`'s
`if (!FunctionName.empty()) { … } else { … }` block, so a `declare` / `define`
now retires its own `ForwardRefVals` / `ForwardRefValIDs` entry, compares
`FwdFn->getType() != PFT` there, and emits both of upstream's per-site
texts — `invalid forward reference to function '<n>' with wrong type: expected
'T' but was 'U'` (pinned by `test/Assembler/opaque-ptr-invalid-forward-ref.ll`,
now a corpus row) and `type of definition and forward reference of '@N'
disagree`. The bullets below are the residual: `parse_global` and
`parse_alias_or_ifunc`.

- **LLVM:** `LLParser::parseGlobal` and `parseAliasOrIFunc` compare types where the definition is written, producing `forward reference and definition of global have different types` at the type location, and the alias twin.
- **llvmkit:** A reference to an unknown `@name`/`@N` mints a `ForwardRefValue` at the demanded pointer type and one end-of-module sweep retires it. The global text is emitted, but from the sweep and anchored at the *reference* rather than at the definition's type; the alias twin does not exist at all.
- **Why:** Recorded as a W2.5 correction and carried: "resolution is a single end-of-module sweep, not per-definition-site — same verdicts, but upstream's per-site texts stay unreachable".
- **Fix:** Move resolution into `parse_global` and `parse_alias_or_ifunc` the way `claim_function_forward_ref` moved it into the two function headers, comparing at the type location. Upstream also **erases** the map entry there; llvmkit's global guard only calls `contains_key`, so the redefinition check stays suppressed for every later definition of that name and the collision surfaces from the builder instead — that half is part of this entry too.
- **Correction from verification:** Substantially accurate and still present, with one wording correction. The sweep design is exactly as described: `global_forward_ref` (ll_parser.rs:1765) mints a placeholder at the demanded pointer type, and `resolve_forward_ref_globals` (:1714), called once at :1475, retires every entry; neither `parse_alias_or_ifunc`'s tail (:7151-7231) nor `parse_declare`'s function tail (:10461-10592) retires a forward ref or compares types at the definition. But "upstream's per-site texts are unreachable" is wrong for one of the three: `forward reference and definition of global have different types` IS implemented, at :1748, emitted from the sweep. What is unreachable is upstream's ANCHOR for it — llvmkit reports at `entry.loc` (where the reference was written), upstream at `TyLoc` (the definition's type). The other two texts are genuinely absent from `crates/`: the alias twin (`...of alias have different types`) and `type of definition and forward reference of '@N' disagree` exist nowhere in llvmkit source (the latter appears only as a backlog note in docs/future-work.md:140). The numbered-function case falls through to the global text from the same sweep; the function-header path instead carries an unrelated llvmkit-only rule, `forward function declaration with matching signature` (:10474-10479), which compares the whole FunctionType where upstream compares only the address space (`FwdFn->getType() != PFT`). Also unclaimed but real: upstream ERASES the map entry at the definition site (`ForwardRefVals.erase(I)`), so a second definition of a forward-referenced name gets `redefinition of global '@g'`; llvmkit's guard at :6991-7000 only calls `.contains_key`, never removes, so the redefinition check stays suppressed for every later definition of that name and the collision surfaces from the builder as `expected valid global definition: ...` — the exact message the guard's own comment says it was added to avoid.

<details><summary>Verification evidence</summary>

crates/llvmkit-asmparser/src/ll_parser.rs — :1765-1809 `global_forward_ref` mints `forward_ref_value_placeholder(ty)` into `forward_ref_globals`/`forward_ref_global_ids` (call sites :8172, :8194, :8223, :8238); :1475 calls `resolve_forward_ref_globals` in the end-of-module tail; :1714-1740 sweeps both maps; :1742-1759 `resolve_global_forward_ref` holds the single `entry.placeholder.ty() != target.ty()` check emitting "forward reference and definition of global have different types" at `entry.loc`; :6991-7000 the `forward_ref_globals.contains_key` guard (reads, never removes); :7151-7231 alias/ifunc definition tail has no forward-ref handling; :10461-10592 function-header tail has none either, only `forward function declaration with matching signature` at :10474-10479. Repo-wide Grep excluding orig_cpp for "alias have different types|type of definition and forward reference|disagree: expected" returns zero hits in crates/. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp — parseGlobal (read via sed 1370-1460) does `ForwardRefVals.erase(I)` / else `redefinition of global '@'+Name`, then `if (GVal) { if (GVal->getAddressSpace() != AddrSpace) return error(TyLoc, "forward reference and definition of global have different types"); GVal->replaceAllUsesWith(GV); GVal->eraseFromParent(); }`; parseAliasOrIFunc has the same shape at ExplicitTypeLoc with "...of alias have different types"; parseFunctionHeader has `if (FwdFn->getType() != PFT) return error(NameLoc, "type of definition and forward reference of '@N' disagree: expected ... but was ...")` plus `ForwardRefValIDs.erase(I)`. docs/future-work.md:134-145 already records this as an open item with a matching rationale.

</details>

### 37. `intrinsic can only be used as callee` fires at reference time, not at end of module

*parser — intrinsics* — crates/llvmkit-asmparser/src/ll_parser.rs:8158, :8209

- **LLVM:** Upstream auto-declares `llvm.`-prefixed leftovers from call-site function types in `validateEndOfModule`, and the `intrinsic can only be used as callee` rejection happens there, in that ordered sequence.
- **llvmkit:** The message is emitted at the point of reference (two sites), so a construct upstream would only reject at end of module is rejected earlier and the end-of-module error ordering differs.
- **Why:** Recorded as a W2 carried item ("`intrinsic can only be used as callee` still fires at reference time"). Error *ordering* in `validateEndOfModule` is itself part of parity, which is why it routes to W13.
- **Fix:** Fold into W13's `validateEndOfModule` 1:1 sequence: defer the check to the intrinsic auto-declaration step so it fires in upstream's order relative to blockaddress leftovers, dso_local_equivalent resolution, undefined types/comdats and `@` leftovers. The verifier half has to land with it: upstream's `Verifier::visitInstruction` exempts an `OB_clang_arc_attachedcall` bundle operand from `Cannot take the address of an intrinsic!` precisely so `verifyAttachedCallBundle` can judge it, so deferring the parse guard without that exemption trades a rejects-valid for an accepts-invalid.
- **Blocks a fixture, named here so the cost is visible:** `test/Verifier/operand-bundles.ll`'s `@f_clang_arc_attachedcall` writes `ptr @llvm.objc.retainAutoreleasedReturnValue` and `ptr @llvm.assume` as bundle operands. Seven of its thirteen calls therefore stop at this parse error, and `crates/llvmkit-asmparser/tests/parser_calls.rs::upstream_verifier_operand_bundles_fixture_messages_match` asserts *that* for them rather than the diagnostic upstream prints — so it turns red the day this entry closes, which is when the remaining `verifyAttachedCallBundle` coverage has to land.
- **Correction from verification:** Accurate, and understated. Two corrections/refinements: (1) The ordering point is right but the mechanism is stronger than "rejected earlier": llvmkit has NO end-of-module `llvm.*` handling at all. `Parser::parse_module`'s end-of-module sequence (crates/llvmkit-asmparser/src/ll_parser.rs:1457-1480) contains no counterpart to upstream's `ForwardRefVals` auto-declaration loop; the guard was relocated wholesale into the two reference-time sites. So the rejection does not merely fire earlier within `validateEndOfModule` — it fires during `parseTopLevelEntities`, ahead of every end-of-module check. (2) The claim misses a second, larger consequence: because the guard is the *first* statement in each function, running before the `self.module.global(&name)` / `function_dyn(&name)` lookups, llvmkit also rejects address-taken references to an intrinsic that IS declared in the module. Upstream's `getGlobalVal` resolves those to the existing `Function` with no `ForwardRefVals` entry, so LLParser accepts them; the rejection comes from the Verifier with a different message, "Invalid user of intrinsic instruction!" (Verifier.cpp:3293, fixture test/Verifier/intrinsic-addr-taken.ll). llvmkit turns a verifier diagnostic into a parse error and renames it. Relatedly, llvmkit's own verifier (crates/llvmkit-ir/src/verifier.rs:965) emits "intrinsic can only be used as callee" where upstream emits "Invalid user of intrinsic instruction!" — a third site the claim does not list.

<details><summary>Verification evidence — three probes re-run 2026-08-21, all still as recorded</summary>

llvmkit source, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs — both cited lines are live and at the exact offsets given. `resolve_global_name_as_value` (:8148) and `resolve_global_name_as_constant` (:8199) each open with `if !matches!(resolve_intrinsic_name(&name), IntrinsicNameResolution::NonIntrinsic) { return Err(ParseError::Message { message: "intrinsic can only be used as callee", ... }) }` at :8153-8161 and :8204-8212, before any module lookup. `IntrinsicId::resolve_name` (crates/llvmkit-ir/src/intrinsics.rs:323) returns NonIntrinsic only for names not starting with "llvm.", so the guard covers every `llvm.`-prefixed name, known or unknown. Callers are `convert_val_id_to_value` (:7972) and `convert_val_id_to_constant` (:8035), both parse-time. Upstream, C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp — the one occurrence of the message is line 341, inside `LLParser::validateEndOfModule`, in the `for (const auto &[Name, Info] : make_early_inc_range(ForwardRefVals))` loop (:328), reached only for names never declared, and ordered after the ForwardRefBlockAddresses guard (:271), the dso_local_equivalent resolution (:302-311), the undefined-type checks (:313-321) and the undefined-comdat check (:323). Fixture test/Assembler/implicit-intrinsic-declaration-invalid2.ll pins that message for an undeclared `@llvm.umax`. Empirical probe (temporary test in crates/llvmkit-asmparser/tests/, run with `cargo +1.96.0 test --release -p llvmkit-asmparser`, since deleted): - `declare i32 @llvm.umax.i32(i32, i32)` + `@g1 = global ptr @llvm.umax.i32` (verbatim test/Verifier/intrinsic-addr-taken.ll) -> llvmkit PARSER errors "intrinsic can only be used as callee"; upstream parses this and the Verifier says "Invalid user of intrinsic instruction!". - `@c = global i32 0, comdat($nope)` alone -> llvmkit errors "use of undefined comdat '$nope'" (so the check exists and works). - `@c = global i32 0, comdat($nope)` followed by `@g = global ptr @llvm.umax` -> llvmkit errors "intrinsic can only be used as callee", preempting the comdat error. Upstream's order (LLParser.cpp:323 before :328) reports the comdat error instead. This is the ordering divergence, demonstrated. - `call i8 @llvm.umax.i8(...)` still parses OK, confirming the callee path routes elsewhere and only non-callee references hit the guard.

</details>

### 38. `validateEndOfModule` is not a 1:1 port, and its error order is not pinned

*parser — end of module* — crates/llvmkit-asmparser/src/ll_parser.rs:1711 (`validate_end_of_module` region), :4786-4798 (comdat guard), :4756 (undefined types)

- **LLVM:** `LLParser::validateEndOfModule` runs a fixed sequence: attribute-group merge + alignment-attr→field move, blockaddress leftovers, `dso_local_equivalent` resolution, undefined numbered/named types, undefined comdats, intrinsic auto-declaration and `@` leftovers, undefined metadata, metadata cycle resolution, the TBAA hook, then SlotMapping steal semantics. Which error fires first is itself observable.
- **llvmkit:** The pieces exist but were landed wave by wave (W2.5 did intrinsic auto-declaration and `@` leftovers; W3 the undefined types; W2.6 comdats), and the sequence was landed before anyone checked it against upstream's — see the Status bullet for where that stands now. Step one — the attribute-group merge and the `validateEndOfModule` half of the alignment-attribute→field move — is **D9**, and is not restated here; this entry owns only the *sequence*.
- **Why:** Recorded as W13's opening item, with the ordering explicitly called "part of parity". Its group-merge half is the blocker under the printer's missing attribute-group forming.
- **Fix:** Port the routine as one ordered sequence, add the attr-group merge + `align`-to-field move, and pin the order with negative fixtures that trip two rules at once. Also covers `getIntrinsicSignature` mangling-suffix cases (`llvm.umax` on `i32` declares `llvm.umax.i32`) and the `InstsWithTBAATag` hook W11 was to leave behind.
- **Status (W13a, W13b):** the *sequence* is now upstream's, step by step, and the `dso_local_equivalent` step exists (see D7). The initializer deferral that made step 3 re-mint references after step 4 had run is gone (see D8), so `@g = global ptr blockaddress(@never_defined, %entry)` on its own is rejected rather than printing `<forward reference>`. **Still open:** step one (D9), the intrinsic auto-declaration loop (entry **37**, which is wider than this bullet — llvmkit's `intrinsic can only be used as callee` fires at reference time and rejects an address-taken reference to a *declared* intrinsic that upstream's parser accepts), metadata-cycle resolution, the TBAA hook and `Slots` steal semantics.

> **`Correction from verification` block removed 2026-08-21.** It was a
> snapshot taken before W13a/W13b, and each of its four empirical findings
> now behaves the other way. Re-probed at that date with
> `crates/llvmkit-asmparser/examples/parse_file.rs` built at this commit:
> `@x = external global %undefined.type` plus a leftover `blockaddress`
> reports `expected function name in blockaddress` — upstream's order, not the
> undefined type; an undefined callee plus `!named = !{!5}` reports the
> undefined value, not the metadata; `@g = global ptr
> blockaddress(@never_defined, %entry)` alone is rejected, with no
> `<forward reference>` reaching printed IR; and
> `@a = global ptr dso_local_equivalent @nosuch` reports
> `unknown function 'nosuch' referenced by dso_local_equivalent`, the message
> that block said existed nowhere in the tree. Its two surviving sub-clauses —
> the split alignment move, and the attribute-group merge still being absent —
> are folded into the `llvmkit:` bullet above. Its remaining structural note,
> that the steps are inlined in `parse_module` rather than in a
> `validate_end_of_module` routine, is no longer a divergence: they run there
> in upstream's order under a comment naming each upstream step. The
> `undefined global` / `undefined value` noun split it did not mention is
> gone: every `@`-reference, callee or not, now goes through the one
> `forward_ref_globals` map and its one `SymbolKind`.

> **Evidence block removed 2026-08-20 (fix round 3).** It recorded a single verification pass taken before W13a and was superseded by the `Status (W13a, W13b)` paragraph above: its central finding, "11 calls in an order that does not match upstream's", is no longer true, and its llvmkit coordinate for the sequence (`ll_parser.rs:1457-1480`) had drifted into metadata-slot code. The upstream half it cited (`LLParser::validateEndOfModule`, and `parseValID`'s blockaddress leftovers) still holds and is named by symbol in the bullets above.

## Different printed bytes

The parser/printer contract is that printed output matches `AsmWriter.cpp` byte for byte and re-parses.

### 43. DWARF enumerations and `DIExpression` operands are stored as spellings, so numeric forms never normalise

*IR model* — crates/llvmkit-asmparser/src/ll_token.rs (the nine `Token::Dwarf*` variants), crates/llvmkit-asmparser/src/ll_parser.rs:5865-5878 (`parse_metadata_field_value`'s enum arms), crates/llvmkit-ir/src/asm_writer.rs:3491, :3497

- **LLVM:** `LLParser::parseDIExpressionBody` and the DWARF `MDField` readers convert each keyword to its integer encoding via `dwarf::getOperationEncoding` / `getAttributeEncoding` and store a `uint64_t`; the printer maps the encoding back to a name, so `!DIExpression(15)` prints as the `DW_OP_*` that 15 encodes and a numeric `DW_ATE`/`DW_TAG` field likewise comes back named.
- **llvmkit:** Every `DW_TAG_*`, `DW_ATE_*`, `DW_VIRTUALITY_*`, `DW_LANG_*`, `DW_LNAME_*`, `DW_CC_*`, `DW_OP_*`, `DW_MACINFO_*` and `DW_APPLE_ENUM_KIND_*` word is lexed into its own token carrying the full keyword text and collapses into `MetadataFieldValue::Enum(String)`; `DwarfExpressionOperand::Operation` keeps the source spelling. The AsmWriter writes `Enum(s)` verbatim and `Integer(v)` as the number, so a numerically-written operand or field prints back as a number where `llvm-dis` prints a name. Spellings are now *validated* against the same tables (W11: `invalid DWARF op '...'`, `invalid DWARF attribute encoding '...'`), so only the normalisation half is left.
- **Why:** Recorded: the typed form is the several-hundred-constant `Dwarf.def` family, which is generation work for `llvmkit-tablegen`'s sibling arm rather than a hand-typed enum, and it is deferred to the debug-info/metadata round-trip milestone where a consumer that needs the values exists. The entry also records that the parity plan had marked this closed at W11 from the plan's intent rather than from the tree, and that the false "closed" was worse than an open item.
- **Fix:** Generate the `Dwarf.def` tables (both directions) through `llvmkit-tablegen`, store the encoding rather than the name at parse time, and consult the reverse table at print time so a numeric input normalises to its keyword. The reverse tables are the half that closes the observable divergence; the forward tables already exist as `llvmkit_ir::dwarf::*` for W11's validation. The `DIExpression` half additionally needs `DIExpression::isValid()` modelled, because upstream only prints names when it holds and prints raw numbers otherwise — that consequence, and the IR-API construction hole behind it, is entry 67; do not restate it here.
- **Correction from verification:** Substantially accurate and still present; three corrections. (1) The illustrative example is wrong: 15 is DW_OP_const8s, which is not in DIExpression::isValid's accept switch, so writeDIExpression falls to its `else` branch and upstream prints `15` too. A correct example is `!DIExpression(6)` -> upstream `!DIExpression(DW_OP_deref)`, llvmkit `!DIExpression(6)`. The DWARF *field* half of the claim needs no such caveat: `!DIBasicType(tag: 15, ...)` prints `tag: 15` in llvmkit and `tag: DW_TAG_pointer_type` upstream. (2) The claim understates one case — writeDIExpression also special-cases DW_OP_LLVM_convert to print Op.getArg(1) through AttributeEncodingString, so `!DIExpression(DW_OP_LLVM_convert, 16, 5)` loses `DW_ATE_signed` in llvmkit even though the operation itself was written as a keyword. (3) The "Where" note and the code's own comments (metadata.rs:2094-2096, ll_parser.rs:5435-5436) say the Dwarf.def tables are unmodelled; that premise is now stale. crates/llvmkit-ir/src/dwarf.rs:621-675 generates BOTH directions (tag/tag_string, operation_encoding/operation_encoding_string, attribute_encoding_string, ...). The reverse lookups exist and are simply unused outside crates/llvmkit-asmparser/tests/dwarf_def_drift.rs — asm_writer.rs contains zero `dwarf::` references. Closing this divergence is now a matter of calling the existing encoding->name functions from the printer (and converting keyword->encoding at parse time), not of building tables.

<details><summary>Verification evidence</summary>

Cited lines all check out exactly. crates/llvmkit-asmparser/src/ll_token.rs:88-104 has the nine Token::Dwarf* variants, each holding &'src str keyword text. crates/llvmkit-asmparser/src/ll_parser.rs:5865-5881 collapses all nine (plus ChecksumKind/EmissionKind/NameTableKind/FixedPointKind) into MetadataFieldValue::Enum(value) carrying the source spelling. crates/llvmkit-asmparser/src/ll_parser.rs:5652-5666: the W11 `keyword` validator matches only the Enum arm and returns Ok(()) for everything else, so a numeric value passes through unconverted. crates/llvmkit-ir/src/asm_writer.rs:3477-3478 (Operation(name) => write_str(name), Literal(value) => write!("{value}")), :3491 (Integer(v) => write!("{v}")), :3497 (Enum(s) => write_str(s)); a grep for `dwarf::` across asm_writer.rs returns nothing. crates/llvmkit-ir/src/metadata.rs:2100-2107 defines DwarfExpressionOperand::Operation(String). Upstream lib/IR/AsmWriter.cpp MDFieldPrinter::printTag calls dwarf::TagString(N->getTag()) and falls back to the raw number only when empty; printDwarfEnum does the same via its Stringifier; writeDIExpression prints OperationEncodingString when N->isValid(). Upstream lib/AsmParser/LLParser.cpp parseMDField(..., DwarfTagField&) routes an APSInt token to MDUnsignedField and a keyword through dwarf::getTag, storing an unsigned either way. Empirical probe (temporary test, since deleted, run on +1.96.0 --release, parsing then re-printing): `!DIBasicType(tag: 15, name: "x")` printed back as `tag: 15`; `encoding: 4` printed back as `encoding: 4` (upstream DW_ATE_float, Dwarf.def HANDLE_DW_ATE(0x04, float)); `!DIExpression(6)` printed back as `!DIExpression(6)` where DW_OP_deref (0x6) IS in DIExpression::isValid's accept list so llvm-dis prints the name; `!DIExpression(DW_OP_LLVM_convert, 16, 5)` printed back with a bare `5`. Keyword-written forms round-trip correctly, so the divergence is exactly the normalisation half, as claimed. Unrelated side-finding from the same probe: unreferenced `!N = !DIExpression(...)` nodes are dropped entirely from format!("{module}") — they printed only once a named metadata node referenced them, whereas llvm-dis emits every numbered node.

</details>

### 44. `ptrtoint`/`ptrtoaddr` mid-width folding diverges on `pointer_size != index_size` layouts

*constant folder* — crates/llvmkit-ir/src/constant_folding.rs:2432 (`fold_ptr_to_int_pair`)

- **LLVM:** In `isEliminableCastPair`'s case-11 *declined* sub-case (`MidSize < SrcSize && MidSize < DstSize`), `ConstantFoldCastOperand` falls to its switch path, where `PtrToInt` takes `DL.getAddressType` and `PtrToAddr` takes `DL.getIntPtrType` — the inverse of each other. On `p:128:128:128:64`, `ptrtoaddr(inttoptr(i128 x)):i128` folds to `x`.
- **llvmkit:** `fold_ptr_to_int_pair` always two-steps through the case-11 mid (`ptrtoint` to the pointer size, `ptrtoaddr` to the index/address size), so the same expression folds to `x mod 2^64` — the semantically correct address extraction, and a different constant from upstream's.
- **Why:** Recorded as a deliberate, reasoned divergence awaiting an explicit decision: llvmkit's value is arguably the more correct side, and matching upstream would mean introducing a wrong mask to copy an upstream quirk, on layouts x86 and the `bin_lift` consumer never use.
- **Fix:** Only if CHERI-like parity comes into scope: mirror the switch path's type choice (`getAddressType` for `PtrToInt`, `getIntPtrType` for `PtrToAddr`) in the declined case-11 sub-case, and port the upstream fixture that pins it so the quirk is documented as copied rather than invented.

<details><summary>Verification evidence</summary>

Confirmed real and unchanged. (1) llvmkit: crates/llvmkit-ir/src/constant_folding.rs:2432 `fold_ptr_to_int_pair`, lines 2462-2466, picks `mid_bits` as `PtrToInt => dl.pointer_size_in_bits(addr_space)`, `PtrToAddr => dl.index_size_in_bits(addr_space)`, then two-steps source -> mid_ty -> dest_ty via `fold_integer_cast_constant` (2467-2471). It is reached unconditionally from `constant_fold_cast_operand` (lines 307-311); `grep -rn "eliminable" --include=*.rs crates/` returns nothing, so llvmkit has no `isEliminableCastPair` accept/decline branch and never falls through to a different mid width. (2) Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ConstantFolding.cpp:1508-1510 in `ConstantFoldCastOperand`'s switch uses `MidTy = Opcode == Instruction::PtrToInt ? DL.getAddressType(CE->getType()) : DL.getIntPtrType(CE->getType())` — and DataLayout.h:679 defines `getAddressType(PtrTy)` as `getIndexType(PtrTy)` (index size) while DataLayout.cpp:976-984 defines `getIntPtrType(Type*)` via `getPointerTypeSizeInBits`. That is exactly the inverse of llvmkit's mapping, and also the inverse of upstream's own case 11 at Instructions.cpp:2965-2967 (`secondOp == PtrToAddr ? DL->getAddressSizeInBits(MidTy) : DL->getPointerTypeSizeInBits(MidTy)`), so upstream's two sites disagree and the declined sub-case (`MidSize < SrcSize && MidSize < DstSize`, Instructions.cpp:2972-2973) is where it becomes observable. (3) Empirically, with a temporary integration test run on `cargo +1.96.0 test --release -p llvmkit-ir` (since deleted; `git status` on the tests dir is clean): on `e-p:128:128:128:64`, `ptrtoaddr(inttoptr(i128 0x1_0000_0000_0000_0001)) to i128` folds in llvmkit to `ApInt{bit_width:128, words:[1]}` i.e. `x mod 2^64`, whereas upstream's case 11 declines (MidSize 64 < 128 and < 128) and the fallback's `getIntPtrType` = i128 makes `ConstantFoldIntegerCast` a no-op, yielding `x` = `words:[1,1]`. The `ptrtoint` half diverges too, in the opposite direction: `ptrtoint(inttoptr(i256 x)) to i256` gives llvmkit `x mod 2^128` (pointer size) versus upstream's fallback `getAddressType` = i64 giving `x mod 2^64`. (4) The scoping in the claim checks out: when case 11 accepts, llvmkit's two-step is value-equivalent to upstream's direct trunc/zext/bitcast (truncation is transitive; zext-then-cast is identical), and when `pointer_size == index_size` the two mid widths coincide, so the divergence is confined to declined case-11 on `pointer_size != index_size` layouts. (5) llvmkit already knows about the gap but does not pin it: the doc comment on `crates/llvmkit-ir/tests/constant_folding_analysis.rs:78-84` calls out "the (unimplemented in llvmkit) index-size-based switch-fallback in `ConstantFoldCastOperand`", and neither of the two `p:128:128:128:64` tests there (lines 86, 116) reaches a declined case-11.

</details>

### 45. DCE keeps calls and allocations upstream deletes

*passes* — crates/llvmkit-ir/src/dce.rs:49-79 (`is_trivially_dead`); crates/llvmkit-ir/src/value.rs:570 (`has_uses`)

- **LLVM:** `wouldInstructionBeTriviallyDead` (`lib/Transforms/Utils/Local.cpp`) deletes unused `willReturn`+readnone calls, removable allocation-function calls, `free(null)`, and lifetime-only allocas; `Value::use_empty` ignores debug-record uses, and upstream salvages debug info instead of being blocked by it.
- **llvmkit:** `is_trivially_dead` returns `false` for every `Call` (and every `VaArg`/pad/atomic), so `DcePass` leaves all of the above in place; the module after DCE differs from upstream's. The record also notes `Value::has_uses` counts debug-record uses upstream ignores, which keeps further instructions alive.
- **Why:** Recorded: porting these needs faithful allocation-function and attribute modelling to avoid *over*-removal, which would be a miscompile if wrong, so the current DCE is deliberately conservative-but-safe.
- **Fix:** Model allocation functions and the relevant call attributes (`willReturn`, memory effects, the allocator family) first, then port `wouldInstructionBeTriviallyDead`'s call/alloca arms against them; separately, give `has_uses` a structural-uses-only sibling for the DCE query and port the debug-info salvage path rather than counting debug records as uses.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/dce.rs (is_trivially_dead, lines 48-75; cited 49-79 is minor line drift): the match arm `Some(Store | Fence | AtomicCmpXchg | AtomicRmw | Call | VaArg | LandingPad | CleanupPad | CatchPad) => false` makes every Call unconditionally not-dead — no attribute, willReturn, mayHaveSideEffects, allocation, free, or intrinsic inspection anywhere in the function. crates/llvmkit-ir/src/value.rs:570 is exactly `pub fn has_uses(self) -> bool { !self.data().use_list.borrow().is_empty() }`, and debug records do push into that list: instruction.rs:1535 and :1549 call `add_use(ValueUse::DebugRecord { .. })` via `record.for_each_value`, while `users()` (value.rs:553-559) filters DebugRecord/Metadata back out — so has_uses counts uses users() does not. Upstream Local.cpp::wouldInstructionBeTriviallyDead returns true for isRemovableAlloc(CB, TLI), any call with !mayHaveSideEffects (the willReturn+readnone case), stacksave/launder_invariant_group/allow_runtime_check/allow_ubsan_check, lifetime intrinsics whose alloca is used only by lifetime intrinsics, trivially-true assume, non-strict constrained FP, getFreedOperand on a null/undef constant (free(null)), and non-volatile loads from constant globals; isInstructionTriviallyDead gates on I->use_empty(). DebugProgramInstruction.h:296-298 confirms DbgVariableRecord reaches its value through DebugValueUser/ValueAsMetadata rather than a Use, so use_empty() is blind to debug records upstream. DCE.cpp::eliminateDeadCode calls salvageDebugInfo(*I) and salvageKnowledge(I) before erasing; `grep -rn salvage crates/llvmkit-ir/src/` returns zero hits. dce.rs is unmodified in the working tree (git status clean for it), so this is the committed state on dev. Two refinements: "lifetime-only allocas" is outcome-accurate but upstream states the rule on the lifetime intrinsic, after which the alloca falls out as an ordinary dead instruction; and the Load arm hides an unclaimed second gap — `load.is_unordered()` rejects an ordered atomic load from a constant global that upstream would delete. Blast radius is wider than the two cited sites: is_trivially_dead is also the predicate at inst_simplify.rs:51,62 and pass_context.rs:4009.

</details>

### 46. InstSimplify folds inside unreachable blocks that upstream skips

*passes* — crates/llvmkit-ir/src/inst_simplify.rs:23-68 (`type Requires = ()`, the ungated worklist loop)

- **LLVM:** `InstSimplifyPass.cpp::runImpl` iterates reachable blocks only, gating each on `DT->isReachableFromEntry(&BB)`, so instructions in dead blocks are left exactly as written.
- **llvmkit:** `InstSimplifyPass::run` walks the whole function worklist with no reachability gate — its `Requires` is `()` and no dominator tree is consulted — so it folds in unreachable blocks and the printed function differs from upstream's in that dead code.
- **Why:** Recorded as a textual-only divergence in dead code; closing it needs reachability (a dominator tree) threaded into the pass. The entry also notes the knock-on test gap: no InstSimplify test covers unreachable-block behaviour, precisely because the skip is missing.
- **Fix:** Change the pass's `Requires` to prefetch `DominatorTree`, skip any block failing `is_reachable_from_entry`, and port the upstream fixture that pins the skip. The analysis is already available (`DominatorTree::is_reachable_from_entry` is what the phi verifier rule uses), so this is plumbing plus one test.
- **Correction from verification:** Accurate as written, with one refinement: because llvmkit gates on `!has_uses` and only erases after a successful fold, the divergence inside unreachable blocks is specifically folding-and-erasing-the-folded-instruction — upstream additionally skips its `isInstructionTriviallyDead` deletion arm there, which llvmkit never performs in that pass anyway. The core claim (no reachability gate, `Requires = ()`, no dominator tree consulted, dead-code text differs from upstream) is correct and unchanged in the tree.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/inst_simplify.rs: line 27 is `type Requires = ();`; the run body (lines 38-66) enters `cx.mutate()`, opens `patch.worklist()`, and loops `while let Some(inst) = scope.step()` with only a `!view.to_erased().has_uses()` skip — grep for "reachab" in the file returns no matches. The worklist seed is block-unfiltered: `FnPatch::worklist` (crates/llvmkit-ir/src/pass_context.rs:1002-1006) pushes every item from `body_instructions()`, which is `self.function.as_function().basic_blocks().flat_map(|b| b.instruction_ids())` (pass_context.rs:934-943) — all blocks, reachable or not. Upstream orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp `runImpl` lines 33-37 open each block with `if (!SQ.DT->isReachableFromEntry(&BB)) continue;` (comment: unreachable code can take strange forms, e.g. an instruction may have itself as an operand), and both entry points build `SimplifyQuery` with a real DominatorTree (lines 96-104, 125-130). llvmkit has the capability but does not wire it: `is_reachable_from_entry` exists at crates/llvmkit-ir/src/dominator_tree.rs:189 and is used by crates/llvmkit-ir/src/verifier.rs:1130. Still present today: `git status --porcelain -- crates/llvmkit-ir/src/inst_simplify.rs` is empty and the file's last touching commit is 857ff39 (a naming refactor). The gap is also self-recorded at docs/future-work.md:1729-1735 as "InstSimplify unreachable-block skip".

</details>

### 47. `SymbolicallyEvaluateGEP` sub-cases not ported — fewer folds than upstream

*constant folder* — crates/llvmkit-ir/src/constant_folding.rs (the GEP symbolic-evaluation path)

- **LLVM:** `SymbolicallyEvaluateGEP` (`ConstantFolding.cpp`) additionally normalises vector index widths via `CastGEPIndices`, preserves `in_range` through nested GEPs, folds a null- or `inttoptr`-nonzero-base GEP to an `inttoptr` (using `mustNotIntroduceIntToPtr` and `APInt::insertBits`), and infers `inbounds` for GEPs of globals from a dereferenceable-bytes query.
- **llvmkit:** None of the four sub-cases is ported, so llvmkit declines these folds; the constant expression survives into the printed module where upstream would have folded it away.
- **Why:** Recorded under the constant-folding parity cycle's known-remaining points, with the property that matters stated explicitly: each sub-case only ever *declines*, never mis-folds, so the divergence is weaker output rather than wrong output.
- **Fix:** Port the four sub-cases one at a time against `ConstantFoldingTest.cpp` and the matching `test/Transforms/InstSimplify` fixtures; the null/`inttoptr` arm needs `mustNotIntroduceIntToPtr` and `ApInt::insert_bits` first, and the inbounds inference needs a dereferenceable-bytes query that does not exist yet.
- **Correction from verification:** Still present, and the four-way gap is real — but the claim's framing is wrong in three places. (1) "None of the four sub-cases is ported, so llvmkit declines these folds; the constant expression survives into the printed module" is wrong for the fourth sub-case. Missing `inbounds` inference does NOT decline a fold. At `crates/llvmkit-ir/src/constant_folding.rs:1710-1718`, `symbolically_evaluate_gep` skips the inference and then still calls `build_canonical_i8_gep(ptr, &offset, nw)` with the merged flags. llvmkit produces the same canonical `getelementptr i8` upstream does; the printed difference is a missing `inbounds` keyword, not a surviving unfolded expression. Only sub-cases 1-3 actually `return Ok(None)` (lines 1657-1659, 1662-1667, 1706-1708), after which the caller at line 2116 falls back to `constant_fold_get_element_ptr`. (2) `CastGEPIndices` is not a vector-only normalisation. Upstream (`ConstantFolding.cpp:845-878`) rewrites *any* index whose scalar type differs from `DL.getIndexType(ResultTy)`'s scalar type (the gate at line 858 is `Ops[i]->getType()->getScalarType() != IntIdxScalarTy`); vector-ness only picks `IntIdxTy` vs `IntIdxScalarTy` as the target at line 861. llvmkit reproduces the scalar half inline — `constant_gep_offset` does `.sext_or_trunc(index_bits)` (constant_folding.rs:1592), matching upstream's signed cast — so a scalar `i16` index folds identically. The genuine residue is the vector-shaped case (a vector index on a scalar-pointer base, or a vector-of-pointers base), where upstream returns a width-normalised GEP and llvmkit returns `None`. (3) The recorded *reason* for the null/`inttoptr` sub-case is stale on both counts, and this is the substantive correction. The doc comment at constant_folding.rs:1632-1633 says llvmkit "models neither `DataLayout::mustNotIntroduceIntToPtr` nor `APInt::insertBits`". Both premises fail on contact: `ApInt::insert_bits` exists at `crates/llvmkit-ir/src/ap_int.rs:354`, and `mustNotIntroduceIntToPtr` is defined upstream (`DataLayout.h:450-452`) as exactly `hasUnstableRepresentation(AS) || hasExternalState(AS)` — llvmkit has both, at `data_layout.rs:520` and `data_layout.rs:525`, plus the type-level `getScalarType()`/address-space plumbing. Nothing infrastructural blocks this fold; it needs a two-line disjunction helper over queries already present. `docs/future-work.md:2056-2061` repeats the same stale justification and should be corrected. The `in_range` sub-case (2) is the one whose stated reason holds up: `ConstantExprInRange` (`constant.rs:132-171`) exposes only `new`/`start`/`end`/`bit_width`/`into_parts` — no `sext_or_trunc`, `subtract`, or `intersect_with`, which upstream needs at ConstantFolding.cpp:911-913 and 934-938. Net: the divergence is real and unfixed in all four sub-cases, the "never mis-folds, only declines or under-annotates" safety property is accurate, but one of the four is a flag-strength difference rather than a declined fold, and the null/`inttoptr` case is far closer to portable than the ledger records.

<details><summary>Verification evidence</summary>

Read `C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/constant_folding.rs:1600-1919`: `symbolically_evaluate_gep`'s doc comment enumerates the same four deferrals, and the body confirms them — early `return Ok(None)` on `in_range.is_some()` (1657), on a non-`Pointer` base type (1662), and on `is_null_or_inttoptr_nonzero_base(ptr)` (1706); the inbounds-inference site (1710-1711) is a bare comment with no code, and `build_canonical_i8_gep` still runs at 1718, so that path folds with weaker flags rather than declining. Caller at 2099-2116 falls back to `constant_fold_get_element_ptr` when the symbolic path returns `None`. Read upstream `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ConstantFolding.cpp:843-991`: `CastGEPIndices` (845-878) called at 890-892; `InRange->sextOrTrunc(BitWidth)` at 911-913 and the per-level `.subtract(Offset)`/`intersectWith` at 934-938; the `(Ptr->isNullValue() || BaseIntVal != 0) && !DL.mustNotIntroduceIntToPtr(...)` → `insertBits` → `getIntToPtr` block at 953-971; the `getPointerDereferenceableBytes` inbounds inference at 973-980. All four exist as described. Grepped the whole `crates/` tree for `cast_gep_indices`/`CastGEPIndices` — only two hits, both prose in the doc comment; no implementation. Same for a dereferenceable-bytes query — the only hit is the doc comment at 1640. Falsifying the recorded blockers: `grep "fn insert_bits"` hit `crates/llvmkit-ir/src/ap_int.rs:354` (read 345-365 — a working bit-copy loop, with an upstream-ported test at `tests/ap_int_upstream_bits.rs:406`). Read `orig_cpp/.../llvm/include/llvm/IR/DataLayout.h:450-452,479-482` for `mustNotIntroduceIntToPtr`'s definition, then read `crates/llvmkit-ir/src/data_layout.rs:510-527`, which carries `has_unstable_representation(addr_space)` and `has_external_state(addr_space)` — both components — each documented as mirroring its upstream counterpart. Read `crates/llvmkit-ir/src/constant.rs:130-171` for `ConstantExprInRange`: no range arithmetic beyond accessors, confirming the `in_range` blocker. `git log -- crates/llvmkit-ir/src/constant_folding.rs` shows the most recent touch is b9e5c97 (LLParser parity W2.1-2.2); nothing since has revisited this path, and the file is unmodified in the current working tree.

</details>

### 50. `possibly_demanded_elements_in_mask` answers exactly for a `zeroinitializer` mask

*analysis / VectorUtils* — crates/llvmkit-ir/src/vector_utils.rs:641 (definition), reason at crates/llvmkit-ir/src/vector_utils.rs:634-640

- **LLVM:** `llvm::possiblyDemandedEltsInMask` reaches its per-element loop only through `dyn_cast<ConstantVector>`. A `zeroinitializer` mask is a `ConstantAggregateZero`, not a `ConstantVector`, so the cast fails and upstream returns "every lane demanded" for a mask that demands none.
- **llvmkit:** llvmkit stores `ConstantVector` and `ConstantAggregateZero` as one element list, so the loop runs for a `zeroinitializer` mask and the function returns the exact all-zero `ApInt`.
- **Why:** Recorded at the call site: "Stronger than upstream, in a sound direction … Over-approximating fewer lanes is always safe for this query; the divergence can only make a caller more precise." It falls out of llvmkit's constant representation rather than being sought.
- **Fix:** No fix wanted for correctness — but the port is no longer faithful, so any upstream fixture asserting the weak answer will diverge. Either gate the element loop on the constant actually being a spelled-out `ConstantVector` (restoring bug-for-bug parity and losing precision), or keep it and add an `UPSTREAM.md` row plus a test naming the divergence so a future parity sweep does not silently "fix" it back.

<details><summary>Verification evidence</summary>

CONFIRMED — real, accurate, and still present in the committed tree. Upstream (C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/VectorUtils.cpp, `llvm::possiblyDemandedEltsInMask`) reads verbatim: APInt DemandedElts = APInt::getAllOnes(VWidth); if (auto *CV = dyn_cast<ConstantVector>(Mask)) for (unsigned i = 0; i < VWidth; i++) if (CV->getAggregateElement(i)->isNullValue()) DemandedElts.clearBit(i); return DemandedElts; The element loop is indeed gated on `dyn_cast<ConstantVector>` alone. That an all-zero i1 mask is not a `ConstantVector` is confirmed twice in llvm/lib/IR/Constants.cpp: `Constant::getNullValue` returns `ConstantAggregateZero::get(Ty)` for `FixedVectorTyID`, and `ConstantVector::getImpl` computes `isZero` over the element list and returns `ConstantAggregateZero::get(T)` before ever reaching `VectorConstants.getOrCreate`. So the dyn_cast fails and upstream returns all-ones for a mask that demands no lane. Scope refinement (does not contradict the claim, which already says "both spellings"): the collapse in `getImpl` means the written-out form `<4 x i1> <i1 0, i1 0, i1 0, i1 0>` is *also* a ConstantAggregateZero upstream, so the divergence covers every fully-zero mask, not only the literal `zeroinitializer` keyword. It does not leak wider than that — `ConstantDataSequential::isElementTypeCompatible` admits only i8/i16/i32/i64 and half/bfloat/float/double, so i1 never becomes a `ConstantDataVector`, and a mixed mask such as `<i1 0, i1 1>` stays a genuine `ConstantVector` where both implementations agree. All-undef/all-poison masks also fail upstream's dyn_cast, but llvmkit's `aggregate_element` hands back an undef/poison element whose `is_null_value()` is false, so those lanes stay demanded and the two agree there too. llvmkit side, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/vector_utils.rs:641-665: the gate is `if let ValueKindData::Constant(_) = &mask.data().kind` — any constant, not a ConstantVector-specific cast — then the loop clears each lane where `constant.aggregate_element(lane).is_some_and(Constant::is_null_value)`. The "one element list" premise checks out rather than being taken on trust. `ConstantData` (crates/llvmkit-ir/src/constant.rs:272-299) has no `ConstantAggregateZero`-equivalent variant; `Aggregate(Box<[ValueSlot]>)` covers "ConstantArray, ConstantStruct, or ConstantVector". `Constant::aggregate_element` (constant.rs:786-814) serves the `Aggregate` arm by handing the stored element back, and its own doc records the collapse ("ConstantAggregateZero and ConstantDataSequential both become an ordinary element list at construction"). The construction side confirms it: the parser's `zero_initializer_constant` (crates/llvmkit-asmparser/src/ll_parser.rs:7831-7844) materialises a fixed-vector `zeroinitializer` by pushing `len` zero elements into `t.const_vector(elements)`. So the loop runs for a zeroinitializer mask and returns the exact all-zero ApInt where upstream returns all-ones. Still present: `git status --porcelain crates/llvmkit-ir/src/vector_utils.rs` is empty (file clean, not among the working-tree modifications), last touched by commit 857ff39. Cited line numbers are exact: 634-640 is the "Stronger than upstream, in a sound direction" paragraph, 641 is the `pub fn` line. The soundness argument also holds — demanding fewer lanes is the conservative direction for this over-approximation, so the divergence can only make a caller more precise, never wrong.

</details>

### 52. Phi known-bits recurses at `depth + 1` instead of upstream's fixed deep depth

*analysis / ValueTracking* — crates/llvmkit-ir/src/value_tracking.rs:1726 (definition), reason at crates/llvmkit-ir/src/value_tracking.rs:1715-1725

- **LLVM:** The `Instruction::PHI` arm of `computeKnownBitsFromOperator` gates its intersection loop on `Depth < MaxAnalysisRecursionDepth - 1` and then recurses at the fixed depth `MaxAnalysisRecursionDepth - 1`, capping the search under any incoming value at one level so the walk does not "spin around in loops".
- **llvmkit:** `phi_known_bits` recurses at `depth + 1`, so a shallow phi gets a full remaining budget under each incoming and can prove strictly more bits than upstream.
- **Why:** Recorded inline: llvmkit already terminates by a different mechanism — the `stack` set rejects re-entering a value that is mid-computation — and `compute_known_bits_inner` memoizes on `(slot, query)` with no depth component, so entering an incoming at a fixed deep depth would cache the weak answer and hand it to a later shallow query of the same value.
- **Fix:** The recorded reason is sound and the fix is not the depth but the cache: add the depth (or a "budget remaining" bucket) to the memo key, or mark entries computed under a truncated budget as non-cacheable. With that in place the fixed `MaxAnalysisRecursionDepth - 1` recursion can be restored and the port becomes faithful.

<details><summary>Verification evidence</summary>

Verified on both sides; the code is committed (no working-tree modification to value_tracking.rs — it is not in the dirty list, and `git diff HEAD` on it is empty). llvmkit side — C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/value_tracking.rs: - `phi_known_bits` is defined at line 1726 and is live: dispatched from line 761, `InstructionKindData::Phi(data) => phi_known_bits(value, data, query, depth, stack)`. - The intersection loop (lines 1757-1778) has NO depth gate at all — the only early exit before it is `if !known.is_unknown() { return Ok(known); }` (line 1748) plus the empty-incoming check. The recursive call at lines 1763-1768 is `compute_known_bits_inner(value_from_slot(value, incoming_value.get()), query, depth + 1, stack)`. - The rationale the claim cites is verbatim in the rustdoc at lines 1715-1725: "Upstream gates the intersection loop on `Depth < MaxAnalysisRecursionDepth - 1` and then recurses at the fixed depth `MaxAnalysisRecursionDepth - 1` ... llvmkit recurses at `depth + 1` instead", justified by the `stack` re-entry guard (line 502) and by `compute_known_bits_inner` memoizing on `(slot, query)` with no depth component (cache key built at line 505, stored at line 531) — a fixed-deep entry would poison the cache for later shallow queries. - `MAX_ANALYSIS_RECURSION_DEPTH = 6` (line 46), matching upstream's constant. Upstream side — orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ValueTracking.cpp, `Instruction::PHI` arm of `computeKnownBitsFromOperator`: the loop is gated `if (Depth < MaxAnalysisRecursionDepth - 1 && Known.isUnknown())`, and the recursion is `computeKnownBits(IncValue, DemandedElts, Known2, RecQ, MaxAnalysisRecursionDepth - 1);` directly under the comment "Recurse, but cap the recursion to one level, because we don't want to waste time spinning around in loops." (plus a TODO about basing the limiter on incoming edge count). So the divergence is real in both halves the claim names: the missing `Depth < MaxAnalysisRecursionDepth - 1` gate and the `depth + 1` recursion in place of the fixed `MaxAnalysisRecursionDepth - 1`. Concretely, a phi reached at depth 0 gets 6 further levels under each incoming in llvmkit versus 1 upstream, and at depth >= 5 upstream skips the intersection entirely while llvmkit still runs it. Two nuances worth noting (neither contradicts the claim): (1) the "strictly more bits" phrasing is really "at least as many, sometimes more" — the llvmkit comment itself says "more precisely ... never less"; (2) llvmkit's general cap is `if depth > query.max_depth()` (line 499) whereas upstream is `if (Depth == MaxAnalysisRecursionDepth) return;` (before the operator walk in `computeKnownBits`), an independent off-by-one that lets llvmkit process one level deeper everywhere, not just at phis.

</details>

### 56. ppc_fp128 component pair stored mirrored from upstream

*IR model / ApFloat* — crates/llvmkit-ir/tests/ap_float_ppc_word_order.rs:1-20; crates/llvmkit-ir/tests/ap_float_upstream_predicates.rs:57-77; crates/llvmkit-asmparser/tests/parser_hex_float_word_order.rs:22-26

- **LLVM:** `DoubleAPFloat::bitcastToAPInt` builds `Data[] = {Floats[0]…, Floats[1]…}` then `APInt(128, 2, Data)`, and APInt word 0 is least significant — so the *leading* double lands in the **low** 64 bits. `TEST(APFloatTest, getZero)` therefore expects `PPCDoubleDouble` negative zero as `{0x8000000000000000, 0}`.
- **llvmkit:** `ppc_words` reads the **high** word as the leading double. `ap_float_upstream_predicates.rs::get_zero` pins the mirrored row `(PPC, true, [0, 0x8000_0000_0000_0000])` with an inline comment `// Upstream: {0x8000000000000000, 0} — mirrored here`. `ap_float_ppc_word_order.rs` pins the mirroring itself and states outright that `to_bits` "does **not** agree with upstream's `bitcastToAPInt` for this one semantics".
- **Why:** Recorded. The module doc says the mirroring is invisible to finite arithmetic (both components are summed) and visible only in three places: the zero/NaN/infinity category (decided by the leading component alone), the placement of a special value by the `qnan`/`inf`/`zero` constructors, and `to_bits`. The `.ll` reader and `AsmWriter` both compensate, so the textual form matches `LLLexer::HexToIntPair`.
- **Fix:** Swap the word order inside `ppc_words` / `ApFloat::from_bits`+`to_bits` for `PpcDoubleDouble` so word 0 holds the leading double, then remove the compensating transposition in the `.ll` reader and `AsmWriter` printer (`parser_hex_float_word_order.rs` records that the two mirrorings currently cancel), and restore the upstream row in `get_zero`. `ap_float_ppc_word_order.rs`'s helper `ppc(leading, residual)` inverts, and `zero_leading_double_is_zero` / `leading_double_dominates_the_value` must keep passing unchanged.

<details><summary>Verification evidence</summary>

Upstream orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Support/APFloat.cpp, DoubleAPFloat::bitcastToAPInt: `uint64_t Data[] = {Floats[0]...getRawData()[0], Floats[1]...getRawData()[0]}; return APInt(128, Data);` — leading double lands in Data[0] = APInt word 0 = low 64 bits. Confirmed by unittests/ADT/APFloatTest.cpp TEST(APFloatTest, getZero): `{&APFloat::PPCDoubleDouble(), true, true, {0x8000000000000000ULL, 0}, 2}`. llvmkit C:\Users\olegg\Desktop\llvmkit\crates\llvmkit-ir\src\ap_float.rs:1683 `ppc_words` returns `(words.get(1), words.first())` as `(high, low)` — i.e. word 1 is the leading double, the mirror image. Constructors agree: `ApFloat::zero`/`one`/`inf` for PpcDoubleDouble build `&[0, <leading>]` (ap_float.rs:324, :348). Word-order convention is otherwise identical to APInt: ApInt::from_words (ap_int.rs:119) is word-0-least-significant, and the IEEEquad row in the llvmkit test matches upstream verbatim ([0, 0x8000_0000_0000_0000]) — only the PPC row is transposed. All three cited pins are live: ap_float_upstream_predicates.rs:72-74 holds `// Upstream: {0x8000000000000000, 0} — mirrored here` above `(PPC, true, [0, 0x8000_0000_0000_0000])`; ap_float_ppc_word_order.rs:1-31 states to_bits "does **not** agree with upstream's `bitcastToAPInt` for this one semantics" and its helper builds `ApInt::from_words(128, &[residual, leading])`; parser_hex_float_word_order.rs:22-26 records ppc_fp128 "parsed correctly by accident" because the two mirrorings cancel. Two compensation sites keep the textual form correct: asm_writer.rs:1092-1102 prints the high word first for PpcFp128, and ll_parser.rs:14431-14440 deliberately routes FpLit::HexPpc128 through big-endian `parse_hex_apfloat` instead of `parse_hex_apfloat_pair`. Ran `cargo +1.96.0 test --release -p llvmkit-ir --test ap_float_ppc_word_order --test ap_float_upstream_predicates`: 3 passed and 20 passed, get_zero green with the mirrored expectation. Minor citation nit only: upstream 22.1.4 spells the constructor `APInt(128, Data)` (ArrayRef overload), not the older `APInt(128, 2, Data)`; word 0 is still least significant, so the substance is unchanged.

</details>

### 58. Attribute groups are never *formed* by the printer — **TWO DUPLICATES MERGED (W11)**

*printer (AsmWriter)* — crates/llvmkit-ir/src/asm_writer.rs (`fmt_function`, `fmt_global`, the `attributes #N = { … }` emitter), crates/llvmkit-ir/src/global_variable.rs, crates/llvmkit-asmparser/src/ll_parser.rs (`parse_global`)

This is the **printer** half of the attribute-group gap. Its parser twin is
**D9** (the `validateEndOfModule` group merge and the `align`-out-of-group
move); neither entry restates the other, and the two close together because
the merge decides what survives into the printed group.

**Two entries were deleted into this one at W11**, both saying the same thing
from a different angle and neither adding a defect the other lacked:

- former **40**, "Function attributes are never hoisted into an attribute
  group" — the function-header framing, plus two facts kept below (upstream
  mints a group slot per `CallBase` too, so *call sites* diverge as well; and
  llvmkit never emits the `; Function Attrs: …` comment
  `AssemblyWriter::printFunction` writes above a function carrying fn attrs);
- former **57**, "No attribute-group printer, so half of two upstream fixtures
  is unreachable" — the blocked-fixture framing, kept below with its test
  citations and with its own correction: the fixture is
  `test/Assembler/globalvariable-attributes.ll`, not the hyphenated
  `global-variable-attributes.ll`, which does not exist upstream.

- **LLVM:** `SlotTracker::CreateAttributeSetSlot` assigns one `#N` slot per
  distinct `AttributeSet` — `processModule` calls it for every function's
  `getAttributes().getFnAttrs()` and every global's `getAttributes()`, and
  `processFunction` calls it for every `CallBase`'s function attributes — and
  `AssemblyWriter::writeAllAttributeGroups` emits the groups at the end of the
  module. `printFunction` therefore writes `define void @f23() #13`, never an
  inline function attribute, with `attributes #13 = { alignstack=4 }` below and
  a `; Function Attrs: alignstack(4)` comment above; `printGlobal` emits the
  same `#N` for a global.
- **llvmkit:** `asm_writer.rs` prints function attributes inline on the header
  and at the call site, and emits an `attributes #N = { … }` block only for
  groups the *input* already carried — the module's group table is written
  only by `ModuleCore::set_attribute_group`, whose sole non-test caller is the
  parser. Numbering is therefore input-preserved, not printer-derived. A
  global's trailing attribute list is not printed at all, and is not *parsed*
  either: `parse_global`'s property loop ends at `unknown global variable
  property!` with no counterpart to upstream's trailing
  `parseFnAttributeValuePairs`, and `GlobalVariableData` has no attribute
  field. The `; Function Attrs:` comment is never emitted. Output is bulkier
  than upstream's and diverges byte-for-byte from `llvm-dis` for any module
  with function attributes — which the parser/printer contract says should not
  happen.
- **Why:** recorded in `docs/future-work.md` (W5/W7) and routed to W13
  deliberately: D9's merge decides which attributes survive into the printed
  group, so building the writer first would pin output the merge then changes.
- **Fix:** port `CreateAttributeSetSlot` + `writeAllAttributeGroups` together
  with D9's merge and alignment move, switch the function-header, call-site and
  global printers to `#N`, add the `; Function Attrs:` comment line, accept the
  trailing global attribute list in `parse_global`, and merge the input's own
  groups into the same slot space.
- **What is blocked by it:** `test/Bitcode/attributes.ll`'s `@f23` writes
  `define void @f23() alignstack(4)` inline and CHECKs
  `attributes #13 = { alignstack=4 }`, pinning both halves of the group
  spelling at once; only the parse half is asserted, by
  `crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs::inline_alignstack_parses_and_round_trips`,
  whose doc comment says so. `test/Assembler/globalvariable-attributes.ll`'s
  `@g1`–`@g4` need the trailing global attribute list *and* the group printer
  and are not ported at all
  (`crates/llvmkit-asmparser/tests/parser_module_level.rs`, whose
  `@g5`–`@g14` half is ported and whose doc comment names the gap).

### 99. Metadata is numbered by arena position, not reachability, so a node AutoUpgrade replaces still prints

*printer* — crates/llvmkit-ir/src/asm_writer.rs (the numbered-metadata loop and `metadata_slot_map`); exposed by crates/llvmkit-ir/src/auto_upgrade.rs

- **LLVM:** `SlotTracker::processModule` mints metadata slots by *walking* — named metadata operands, global and function attachments, instruction attachments, function-local metadata — so a node nothing references is never numbered and `writeAllMDNodes` never prints it. `MDNode::get` also uniques, so rebuilding a tuple with identical contents yields the same node.
- **llvmkit:** the printer walks `metadata_store().nodes()` and numbers *every* non-`MDString` node in arena order. Before W13d that was invisible: the parser only interned nodes the text named, so every node was reachable. `UpgradeModuleFlags` breaks that — it replaces a flag tuple with a freshly interned one, and the superseded tuple stays in the arena and still prints. `test/Bitcode/upgrade-module-flag.ll` therefore prints its five upgraded flags plus three orphaned pre-upgrade tuples, where `llvm-dis` prints six nodes numbered `!0`–`!5`. The output still re-parses (a dead `!N = ...` definition is legal), so only the byte-for-byte half of the contract is broken. **W10 added a second producer**: `UpgradeNVVMAnnotations` rebuilds every `!nvvm.annotations` entry it did not fully consume, and `MetadataStore::get_tuple_with_distinct` never uniques (`get_string` does — tuples do not), so a rebuilt entry with *identical contents* gets a fresh slot and both print. `parser_auto_upgrade.rs::a_repeated_nvvm_annotation_entry_is_visited_once` and `::malformed_nvvm_annotations_are_preserved_rather_than_upgraded` assert the fresh numbers and say why.
- **Why:** the fix is a real `SlotTracker` port, not a patch: upstream's numbering is *encounter* order over a specific traversal, so switching to reachability also changes which number every surviving node gets. That is a workstream of its own, and doing it inside an AutoUpgrade stage would have re-pinned every metadata-bearing expected output at the same time.
- **Fix:** port `SlotTracker::processModule` / `processFunction`'s metadata pre-pass and drive `metadata_slot_map` from it, then re-bless the metadata numbering in the corpus in the same commit. `crates/llvmkit-asmparser/tests/parser_auto_upgrade.rs` asserts on flag *contents* rather than `!N` numbering precisely because of this, and says so.

### 104. The metadata no-slot fallback prints an arena index, and detached IR cannot be built — **NARROWED**

**Severity:** wrong-output (unreachable today) / model-gap

*printer* — crates/llvmkit-ir/src/asm_writer.rs — `fmt_metadata_operand`; crates/llvmkit-ir/src/argument.rs, instruction.rs

This entry used to be about four `<unnumbered>` spellings in `asm_writer.rs`.
Those are `<badref>` now, and `asm_writer_basic.rs::an_argument_with_no_slot_prints_upstreams_badref`
pins the string. What survives is the metadata half and a model gap.

- **`AsmWriter.cpp` has two different spellings for a missing metadata slot and
  llvmkit has one, which is neither.** `printNamedMDNode` writes
  `int Slot = Machine.getMetadataSlot(Op); if (Slot == -1) Out << "<badref>";
  else Out << '!' << Slot;`, while `writeAsOperandInternal(raw_ostream &, const
  Metadata *, …)`'s `MDNode` arm writes the node's **pointer** —
  `Out << "<" << N << ">"`, under its own comment "Give the pointer value
  instead of \"badref\", since this comes up all the time when debugging."
  llvmkit routes both through one `fmt_metadata_operand`, whose fallback is
  `write!(f, "!{}", id.index())` — the node's raw arena index, which *looks*
  like a valid `!N` reference. Reproducing the second spelling is barred by the
  ban on pointer identity; splitting the routine to reproduce the first is not.
- **Unreachable by construction today**, which is why this is recorded rather
  than fixed in the round that found it: `fmt_metadata_operand` returns early
  for an `MDString` and for an inline node, and `metadata_slot_map` gives a slot
  to every *other* node in the store, so the fallback needs a `MetadataSlot`
  the store does not hold.
- **A detached `Argument` or `Instruction` is not constructible**, so
  `unittests/IR/AsmWriterTest.cpp`'s `DebugPrintDetachedArgument` and
  `DebugPrintDetachedInstruction` are ported with an *attached* unnamed value
  instead. For the argument the substitution is provably output-identical —
  `Value::print` sends an `Argument` to `printAsOperand`, which never calls
  `incorporateFunction`, so the local slot is -1 either way — and the
  substitution is stated in the test. `DebugPrintDetachedInstruction` is
  unported: it needs a `BinaryOperator` with no parent block, and llvmkit's
  builder has no such state.
- **Out of scope, deliberately** (three `%<unnumbered>` sites with no upstream
  `<badref>` twin, listed so the next reader does not re-derive it): the
  function-signature argument printer in `fmt_function_with_use_lists`, whose
  upstream counterpart `AssemblyWriter::printArgument` has no failure spelling
  at all — it is `int Slot = Machine.getLocalSlot(Arg); assert(Slot != -1 &&
  "expect argument in function here"); Out << " %" << Slot;`; the anonymous
  identified-struct number in the type-identity block, where
  `printTypeIdentities` writes `Out << '%' << I << " = type "` from a
  `NumberedTypes` **index** and so cannot fail; and the same struct case in
  `type.rs`'s `Display`.

## Model gaps

A public query answers differently from its LLVM counterpart, or a structure LLVM has is missing.

### 61. Plain `add`/`sub`/`div`/shift builders consult the wrong folder hook

*IR builder* — crates/llvmkit-ir/src/ir_builder.rs (`int_add` / `int_sub` and the div/shift emitters), crates/llvmkit-ir/src/ir_builder/folder.rs (the hook trait)

- **LLVM:** `IRBuilder::CreateAdd` funnels through `FoldNoWrapBinOp(.., false, false)`, and `CreateUDiv` and friends through `FoldExactBinOp(.., false)`, so a folder sees the no-wrap/exact hook even when no flag is set.
- **llvmkit:** `int_add` / `int_sub` and the div/shift siblings consult the plain `fold_int_bin_op` hook directly. Results are identical with the shipped folders; a third-party folder that overrides only the no-wrap or exact hooks observes the difference.
- **Why:** Recorded under the 2026-07-06 upstream-parity follow-ups, with the observability boundary stated: identical results with the shipped folders, observable only by third-party folders overriding just those hooks.
- **Fix:** Route the flagless emitters through the no-wrap / exact hooks with the flags set false, matching `FoldNoWrapBinOp` / `FoldExactBinOp`, and add a test folder that overrides only those hooks to witness the dispatch — the divergence is unobservable without one.
- **Correction from verification:** Still present, but the title's "shift" is over-broad and the scope is understated in two ways. Accurate statement: llvmkit's `int_add` and `int_sub` consult the plain `fold_int_bin_op` hook where upstream `CreateAdd`/`CreateSub` funnel through `FoldNoWrapBinOp(.., false, false)`; and `int_udiv`/`int_sdiv`/`int_lshr`/`int_ashr` consult the plain hook where upstream `CreateUDiv`/`CreateSDiv`/`CreateLShr`/`CreateAShr` funnel through `FoldExactBinOp(.., false)`. `int_shl` and `int_mul` do NOT diverge -- both route through `int_binop_flagged`, whose dispatch sends {Add, Sub, Mul, Shl} to `fold_int_bin_op_no_wrap` with empty `OverflowFlags`, matching upstream `CreateShl`/`CreateMul`. `int_urem`/`int_srem` correctly match upstream's plain `FoldBinOp`. Two extensions the entry omits: (1) the `_with_flags` variants (`int_udiv_with_flags`, `int_sdiv_with_flags`, `int_lshr_with_flags`, `int_ashr_with_flags`) also hit the plain hook whenever the exact bit is absent, so the divergence is not limited to the bare builders; (2) the erased path `int_binop_dyn_with_flags` calls `fold_bin_op_dyn` unconditionally -- even when nuw/nsw/exact ARE set -- so `int_binop_erased` (the .ll parser's entry point) and every `int_*_dyn` wrapper never reach `fold_no_wrap_bin_op_dyn` or `fold_exact_bin_op_dyn` at all, a strictly larger gap than the recorded one. Root cause for the exact half: `IrBuilderFolder::fold_exact_bin_op_dyn` deliberately drops upstream's `bool IsExact` parameter (documented in its own rustdoc), so calling it with IsExact=false is unspellable without a signature change; the add/sub half needs only a one-line switch to `fold_int_bin_op_no_wrap` with empty flags, exactly as mul/shl already do. The claim's "results are identical with the shipped folders" is verified correct.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/ir_builder.rs: `int_add` (line 1234) and `int_sub` (line 1258) each call `self.folder.fold_int_bin_op(BinaryOpcode::Add/Sub, lhs, rhs)?` inline. `int_udiv` (1304), `int_sdiv` (1327), `int_lshr` (1420), `int_ashr` (1443) delegate to `int_binop`, whose sole fold call is `self.folder.fold_int_bin_op(opcode, lhs, rhs)` (line 1837). `int_mul` (1280) and `int_shl` (1396) delegate to `int_binop_flagged`, whose dispatch at lines 1798-1809 reads: `if payload.is_exact { fold_int_bin_op_exact } else if matches!(opcode, Add|Sub|Mul|Shl) { fold_int_bin_op_no_wrap(.., OverflowFlags::from_parts(nuw, nsw)) } else { fold_int_bin_op }` -- so mul/shl reach the no-wrap hook with empty flags (matching upstream) while udiv/sdiv/lshr/ashr fall to the final else. `int_binop_dyn_with_flags` (line 1911) calls `self.folder.fold_bin_op_dyn(opcode, lhs, rhs)` before `opcode.accepted_flags(flags).apply(&mut payload)`, so the erased/parser path ignores flags for folding entirely. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/IR/IRBuilder.h: CreateAdd (1403-1406) and CreateSub (1420-1423) call `Folder.FoldNoWrapBinOp(Instruction::Add/Sub, LHS, RHS, HasNUW, HasNSW)` with `HasNUW = HasNSW = false` defaults; CreateUDiv (1454-1456), CreateSDiv (1467-1469), CreateLShr (1513-1515), CreateAShr (1532-1534) call `Folder.FoldExactBinOp(..., isExact)` with `isExact = false` default; CreateShl (1492-1495) and CreateMul (1437-1440) call FoldNoWrapBinOp; CreateURem/CreateSRem (1480-1490) call plain FoldBinOp. crates/llvmkit-ir/src/ir_builder/folder.rs lines 59-71: `fold_exact_bin_op_dyn` has no `is_exact` parameter, with a rustdoc note saying "The builder only ever calls this with exactness implied by the method (there is no non-exact caller), so unlike upstream's FoldExactBinOp(..., bool IsExact) there is no is_exact parameter to thread." Equivalence check: crates/llvmkit-ir/src/ir_builder/constant_folder.rs `fold_exact_binary` (577-587) calls `fold_binary_constants(opcode, lhs, rhs, ConstantExprFlags::none())` -- byte-identical to what `fold_bin_op_dyn` (35-42) does via `fold_binary`; `fold_no_wrap_bin_op_dyn` (53-66) with empty OverflowFlags yields `ConstantExprFlags::overflowing(false, false)`, which `canonical_constant_expr_flags` in crates/llvmkit-ir/src/constants.rs (1468-1470, applied at line 1091) maps to `ConstantExprFlags::None`. crates/llvmkit-ir/src/ir_builder/no_folder.rs: `impl IrBuilderFolder for NoFolder {}` -- every default body returns Ok(None). docs/future-work.md lines 1740-1745 records this as a known open item titled "Plain add/sub/div/shift hook dispatch", and crates/llvmkit-ir/tests/constant_folder_builder.rs tests `custom_folder_no_wrap_hook_receives_mul` / `..._shl` pin the mul/shl no-wrap routing as already-correct behavior.

</details>

### 62. The phi known-bits arm recurses deeper than upstream, and answers more precisely

*analysis* — crates/llvmkit-ir/src/value_tracking.rs:1715-1726 (the recorded decision on `phi_known_bits`)

- **LLVM:** `computeKnownBitsFromOperator`'s `Instruction::PHI` arm gates its incoming-value intersection on `Depth < MaxAnalysisRecursionDepth - 1` and then recurses at that fixed depth, capping the search under an incoming value at one level so it does not spin around loops.
- **llvmkit:** `phi_known_bits` recurses at `depth + 1`, so it can prove strictly more about a shallow phi than upstream does. `@test_udiv_neg` in the ported `recurrence-knownbits.ll` witnesses it: llvmkit proves 60 leading zeros where upstream proves none (the fixture's own claim, bit 2 unknown, is untouched).
- **Why:** Recorded and deliberate, with two reasons: llvmkit already terminates by a different mechanism — the `stack` set rejects re-entering a value mid-computation — and `compute_known_bits_inner` memoizes on `(slot, query)` with no depth component, so entering an incoming value at a fixed deep depth would cache the weak answer computed there and hand it to a later shallow query. The entry flags the trigger to revisit: if the cache ever becomes depth-keyed, the upstream cap becomes portable as-is.
- **Fix:** Leave it until the known-bits cache becomes depth-keyed. At that point add the depth component to the memo key, adopt upstream's `MaxAnalysisRecursionDepth - 1` gate and fixed-depth recursion, and re-run `recurrence-knownbits.ll` — `@test_udiv_neg`'s llvmkit-specific extra precision is the assertion that will move.
- **Correction from verification:** The code-level divergence is real and still present, but the witness sentence is wrong and should be dropped. Accurate half: upstream's `Instruction::PHI` arm in `computeKnownBitsFromOperator` (ValueTracking.cpp) guards the intersection with `if (Depth < MaxAnalysisRecursionDepth - 1 && Known.isUnknown())` and then calls `computeKnownBits(IncValue, DemandedElts, Known2, RecQ, MaxAnalysisRecursionDepth - 1)` — a fixed depth, not `Depth + 1`. llvmkit's `phi_known_bits` has no depth gate at all (only the `if !known.is_unknown() { return }` early exit) and recurses with `depth + 1`. So llvmkit can indeed prove strictly more about a shallow phi. That part stands, in the source and in the recorded rationale (memoization on `(slot, query)` with no depth component, `stack`-set termination). Wrong half: "`@test_udiv_neg` witnesses it: llvmkit proves 60 leading zeros where upstream proves none." Upstream proves the same 60 leading zeros there. Trace, with `MaxAnalysisRecursionDepth = 6`: `%iv` is reached at Depth 1, `matchSimpleRecurrence` matches `udiv` but bails on `if (BO->getOperand(0) != I) break;`, so the intersection loop runs (1 < 5). Incoming `%iv.next = udiv i64 9, %iv` is recursed at the fixed depth 5 — which still processes the operator; only its operands land at depth 6. Upstream's cutoff `if (Depth == MaxAnalysisRecursionDepth) return;` sits *after* the ConstantInt fast path, so at depth 6 the numerator `9` is still a known constant and only `%iv` comes back unknown. `KnownBits::udiv(9, unknown)` takes `MinDenom = 0`, hence `MaxRes = MaxNum = 9`, hence `Zero.setHighBits(60)`. Intersected with the constant-`2` incoming (62 leading zeros) that leaves exactly 60 leading zeros — the same answer llvmkit reaches via `depth + 1` (its `compute_for_udiv` is a line-for-line port of the same routine). The fixed-depth cap costs upstream nothing in this function because the incoming value is one operator deep. The cited test does not witness the divergence either: `value_tracking_recurrence.rs` asserts only that `%res` of `@test_udiv_neg` folds to `None`, which both sides agree on. I found no fixture in the tree that actually exhibits the divergence — it would need a phi at shallow depth whose incoming value is an expression chain more than one operator deep. Two smaller notes on the same code, unclaimed: llvmkit also drops upstream's `Depth < MaxAnalysisRecursionDepth - 1` gate outright rather than just relaxing the recursion depth; and the global guard `if depth > query.max_depth()` in `compute_known_bits_inner` differs from upstream's `Depth == MaxAnalysisRecursionDepth` in two ways — it admits one extra operator level (depth 6), and it sits *before* the constant fast path, so at depth 7 llvmkit returns unknown for a constant where upstream would return its exact value at any depth.

<details><summary>Verification evidence</summary>

C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/value_tracking.rs — the doc comment at 1715-1725 records the deliberate divergence verbatim, and the code at 1746-1779 confirms it: no depth gate, `compute_known_bits_inner(value_from_slot(value, incoming_value.get()), query, depth + 1, stack)`. The global guard at 499 is `if depth > query.max_depth()`, with `MAX_ANALYSIS_RECURSION_DEPTH: u32 = 6` at line 46. C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ValueTracking.cpp — the `Instruction::PHI` arm of `computeKnownBitsFromOperator` shows `if (Depth < MaxAnalysisRecursionDepth - 1 && Known.isUnknown())` and the recursion `computeKnownBits(IncValue, DemandedElts, Known2, RecQ, MaxAnalysisRecursionDepth - 1)`; the `Instruction::UDiv` arm recurses at `Depth + 1` and calls `KnownBits::udiv`; the entry-point guard `if (Depth == MaxAnalysisRecursionDepth) return;` in `computeKnownBits` sits after the ConstantInt/ConstantData handling, with the comment "All recursive calls that increase depth must come after this." C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/Analysis/ValueTracking.h — `constexpr unsigned MaxAnalysisRecursionDepth = 6;` C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Support/KnownBits.cpp — `KnownBits::udiv` computes `MaxRes = MinDenom.isZero() ? MaxNum : MaxNum.udiv(MinDenom)` then `Known.Zero.setHighBits(MaxRes.countLeadingZeros())`, which gives 60 for numerator 9 at i64. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/known_bits.rs `compute_for_udiv` is the identical port. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/fixtures/upstream/Analysis/ValueTracking/recurrence-knownbits.ll — `@test_udiv_neg` is `%iv = phi i64 [2, %entry], [%iv.next, %loop]` / `%iv.next = udiv i64 9, %iv`, CHECK keeps `and i64 [[IV]], 4`. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/value_tracking_recurrence.rs lists `("test_udiv_neg", None)` — it asserts only that `%res` is not a constant, never a leading-zero count. C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:1103-1114 — the origin of the claim, including the incorrect "llvmkit proves 60 leading zeros where upstream proves none" sentence at line 1111.

</details>

### 64. `returned_arg_operand` reads only the call site's argument attributes

*analysis* — crates/llvmkit-ir/src/value_tracking.rs:965-974 (`returned_arg_operand`), :976 (`returned_attr`); correct twin in crates/llvmkit-ir/src/pointer_analysis.rs:1316

- **LLVM:** `CallBase::getReturnedArgOperand` consults the *callee's* parameter attributes as well as the call site's, so `declare ptr @f(ptr returned)` makes a call that does not repeat `returned` still return its argument.
- **llvmkit:** `returned_arg_operand` in `value_tracking.rs` scans only the `arg_attrs` slice its caller hands it — the call site's — so the declaration-side `returned` is missed and the `returned` arm of `call_known_bits` is weaker than upstream's. `pointer_analysis.rs` ports both halves correctly; this is the same shortfall in a different function.
- **Why:** Recorded as a tranche-5 finding, with the sibling explicitly named: an upstream fixture caught the missing half in `pointer_analysis.rs`, and the `value_tracking.rs` twin was left as-is. No reason for leaving it is recorded.
- **Fix:** Give `returned_arg_operand` the callee value as well as the call-site attributes and check the callee's parameter attributes when the call site has none, mirroring what `pointer_analysis.rs` already does; take the upstream fixture that caught the pointer-analysis half in the same commit.

<details><summary>Verification evidence</summary>

Upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Instructions.cpp, CallBase::getArgOperandWithAttribute: checks `Attrs.hasAttrSomewhere(Kind, &Index)` (the call site's AttributeList) and then, if `getCalledFunction()` is non-null, `F->getAttributes().hasAttrSomewhere(Kind, &Index)` — i.e. the callee's parameter attributes. getReturnedArgOperand (include/llvm/IR/InstrTypes.h) is that call with Attribute::Returned, and ValueTracking.cpp's computeKnownBits call arm uses it (`if (const Value *RV = CB->getReturnedArgOperand())`, union + reset on conflict). llvmkit C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/value_tracking.rs:965-974: `returned_arg_operand(anchor, args, arg_attrs)` is a single `arg_attrs.iter().enumerate().find_map(...)` over the slice it is handed — no callee fallback. Those attrs are the call site's: value_tracking.rs:778-801 builds CallKnownBitsInputs from `data.attrs.arg_attrs()`, and CallAttributeData::arg_attrs (crates/llvmkit-ir/src/instr_types.rs:2167, 2215) is a plain Box<[AttributeStorage]> on the call instruction with no propagation from the callee at build or parse time. The struct does carry `callee_id`, but it is used only by intrinsic_semantic_for_callee (value_tracking.rs:953, 997-1002), never by the `returned` lookup. Helper returned_attr (:976-989) additionally probes AttrIndex::Param(0) then Param(idx), but only inside the call-site slice. Correct twin confirmed at crates/llvmkit-ir/src/pointer_analysis.rs:1315-1349: `returned_argument` scans the call site's arg_attrs, then `.or_else(...)` resolves the callee via value_from_slot, matches ValueKindData::Function, borrows `data.attributes`, and re-scans Param(index) for Returned; its doc comment names the `declare ptr @f(ptr returned)` case as the reason the fallback is load-bearing. File state: value_tracking.rs is clean in the working tree (git status shows no modification; last commit touching it is b9e5c97), so this is the committed code as of today. The same shortfall is independently recorded at docs/future-work.md:841-848, but the verdict here comes from reading the code.

</details>

### 65. Two min/max matcher arms decline matches upstream accepts, rather than minting a constant

*analysis* — crates/llvmkit-ir/src/select_pattern.rs:1210-1216 (`not_value`), :1449-1456 (`look_through_cast_arm`)

- **LLVM:** `getNotValue`'s second arm folds `~C` for a constant operand via `ConstantInt::get(V->getType(), ~*C)`, and `lookThroughCastConst` builds a casted constant with `ConstantExpr::getTrunc` / `ConstantFoldCastOperand` and checks it round-trips. Both let `matchSelectPattern` recognise shapes llvmkit's ports do not.
- **llvmkit:** `not_value` reports only the `xor X, -1` form, and `look_through_cast_arm` ports only the two arrangements that need no new value. A `not` written as a folded constant, and the cast arrangement needing a materialised constant, are not recognised — each forgoes a match rather than inventing one, so the select-pattern result is weaker than upstream's.
- **Why:** Recorded at both sites and in the backlog: minting a constant is a module mutation, which an analysis has no business performing. The same ground is why `getFlippedStrictnessPredicateAndConstant` is left unported entirely rather than split into a caller-builds-the-constant variant, which the entry argues would be a different function.
- **Fix:** Only closable by deciding that a constant-minting analysis is acceptable, or by giving these matchers a mutation-capable variant taking a module token — at which point `getFlippedStrictnessPredicateAndConstant` becomes portable too and the three should land together. Until then the divergence is the price of the no-mutation rule, and the sites already say so.

<details><summary>Verification evidence</summary>

Claim is accurate and the divergence is unchanged in the working tree (`git status` shows C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/select_pattern.rs clean; last touch was 15e4d87, a casing refactor, not a behavior change). 1) llvmkit `not_value`, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/select_pattern.rs:1210-1228 — the doc comment states outright that upstream's second arm "mints `ConstantInt::get(V->getType(), ~*C)`... so this reports only the `xor X, -1` form", and the body confirms it: it requires `InstructionKindData::Xor` with an all-ones operand (either order) and returns `None` otherwise. No constant arm. Upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ValueTracking.cpp, `getNotValue` (line ~8513) has exactly two arms: `m_Not(m_Value(NotV))` then `if (match(V, m_APInt(C))) return ConstantInt::get(V->getType(), ~(*C));`. The effect reaches `matchSelectPattern`: `not_value` feeds three call sites at select_pattern.rs:811-812, :828-829 (the port of `matchMinMax`'s two "look through not" blocks, ValueTracking.cpp:8546 and :8558) and :1029-1030 (the `lines_up` helper in the `matchMinMaxOfMinMax` port). A `not` spelled as a folded constant — e.g. `(X >s 5) ? ~X : -6` — matches upstream and returns `SPF_SMIN`, and returns no flavor in llvmkit. 2) llvmkit `look_through_cast_arm`, same file :1449-1493 — comment says the `lookThroughCastConst` arm "is not ported; the two shapes that need no new value are", and the body implements only (a) both arms the same cast from the same source type, and (b) `trunc` of a value the compare already widened via `sext`/`zext`. A constant `second` operand falls through to the trunc path and fails, so it is effectively declined. Single caller is select_pattern.rs:471, inside the cast-looking path of `match_select_pattern`. Upstream `lookThroughCast` (ValueTracking.cpp:9030) has three arms, and its middle one is `auto *C = dyn_cast<Constant>(V2); if (C) return lookThroughCastConst(CmpI, SrcTy, C, CastOp);`. `lookThroughCastConst` (:8935-9012) builds a new constant with `ConstantExpr::getTrunc` (ZExt/SExt cases) or `ConstantFoldCastOperand` (Trunc/FPTrunc/FPExt/FPToUI/FPToSI/UIToFP/SIToFP), then round-trips it: `CastedBack = ConstantFoldCastOperand(*CastOp, CastedTo, C->getType(), DL); if (CastedBack && CastedBack != C) return nullptr;` — precisely the "checks it round-trips" the claim describes. 3) The gap is a recorded deliberate choice, not an oversight: C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:832-837, "Two arms are deliberately narrower, both because they would need to mint a constant... Each forgoes a match rather than inventing one." That matches the claim's characterization. One nit, in llvmkit's comment rather than in the claim: select_pattern.rs:1452 calls `lookThroughCastConst` "Upstream's third arm", but it is the second arm in code order and item 2 of 3 in upstream's own doc comment (the trunc/`m_ZExtOrSExt` case is third). The claim's own wording does not repeat this, so it needs no correction.

</details>

### 66. Phi `remove_incoming` never self-erases an emptied phi

*IR model / Instructions* — crates/llvmkit-ir/src/instructions.rs:1430 (helper), crates/llvmkit-ir/src/instructions.rs:1637 (public `PhiInstDyn::remove_incoming` doc), crates/llvmkit-ir/src/phi_raw_tests/remove_incoming.rs:13

- **LLVM:** `PHINode::removeIncomingValue(unsigned Idx, bool DeletePHIIfEmpty = true)` (`lib/IR/Instructions.cpp`) defaults to `true`: after `swap_remove`ing the entry, if the phi has no operands left it is `replaceAllUsesWith(PoisonValue)`d and `eraseFromParent()`ed. Callers that drop a predecessor rely on that.
- **llvmkit:** `phi_remove_incoming` (the shared body of all four phi handles' `remove_incoming`) mirrors the `swap_remove` exactly but stops there. Removing the last incoming leaves a live phi with zero incomings — a node that prints as `%p = phi i32` with no `[ … ]` pairs, which `LLParser::parsePHI` cannot re-read. The caller is expected to finish the job.
- **Why:** Recorded at `crates/llvmkit-ir/src/instructions.rs:1417`: erasure in llvmkit goes through `Instruction::erase_from_parent`, which *consumes* the linear lifecycle handle so use-after-erase is a compile error; a `Copy` opcode handle cannot express that consumption, so self-erasure here would hand the caller a live handle to an erased instruction. The auto-erase behaviour does ship where it can be sound — inside the `ReshapeCfg` pass surface, whose edge edits RAUW an emptied phi with poison and erase it, mirroring `removePredecessor`.
- **Fix:** Either (a) give the erased phi surface a linear, consuming variant — `remove_incoming_or_erase(self, …) -> Either<Value, ErasedPhi>` taking the phi by value so the handle cannot outlive the erase; or (b) return a `PhiEmptied` marker the caller must destructure, making the leftover unignorable. (b) is cheaper and preserves the `Copy` handle for the common case. The verifier already flags the leftover through `check_phi`'s count guard — a block whose other predecessors survive now has more predecessors than the emptied phi has entries — so the gap is authoring ergonomics, not soundness.
- **Correction from verification:** The core divergence is accurate and unchanged. One supporting detail is wrong: the claim (and llvmkit's own comments) say a bracket-less phi is something "LLParser::parsePHI cannot re-read". LLParser::parsePHI actually accepts it — on the first loop iteration `if (Lex.getKind() != lltok::lsquare) break;` exits cleanly and it builds a PHINode with zero incomings, returning InstNormal. The rejection is the Verifier's ("PHI node entries do not match predecessors"), not the parser's, and only for a reachable block that has predecessors; a zero-incoming phi in an unreachable block is legal upstream and llvmkit pins both halves (zero_incoming_phi.rs:298 rejected-in-reachable, :339 accepted-in-unreachable). llvmkit's own in-tree comments at zero_incoming_phi.rs:162 and :269 ("the shape LLVM's LL parser rejects") carry the same imprecision and should be reworded to blame the verifier. Everything else in the claim — the swap_remove-only body, the missing DeletePHIIfEmpty half, the four shared call sites, the caller-finishes-the-job contract, and all three cited line numbers — is correct.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/instructions.rs:1430 `fn phi_remove_incoming` — body is bounds-check, `incoming.swap_remove(slot)`, remove one `ValueUse::Instruction(phi_id)` from the removed value's use_list, return the Value. No zero-count branch, no erase. Its doc states outright: "The `DeletePHIIfEmpty` half of upstream's contract is deliberately **not** mirrored", because `Instruction::erase_from_parent` consumes a linear handle and a `Copy` opcode handle cannot express that consumption. Four call sites at instructions.rs:1648/1894/2106/2247 confirm "all four phi handles". Public doc at instructions.rs:1637: "Unlike upstream's default `DeletePHIIfEmpty = true`, removing the last incoming leaves the (now unprintable) phi in place". Upstream orig_cpp/.../llvm/lib/IR/Instructions.cpp:152-156 in PHINode::removeIncomingValue: `if (getNumOperands() == 0 && DeletePHIIfEmpty) { replaceAllUsesWith(PoisonValue::get(getType())); eraseFromParent(); }`; the default is declared in Instructions.h:2791-2792 as `bool DeletePHIIfEmpty = true`. Still-present proof: crates/llvmkit-ir/src/phi_raw_tests/remove_incoming.rs (final single-incoming case) removes the only incoming, asserts `incoming_count() == 0`, rediscovers the phi in block "join", and asserts `still_there` with the message "an emptied phi must not be self-erased"; the module doc at lines 13-20 names it a deliberate divergence and points at the ReshapeCfg edge edits (zero_incoming_phi.rs:142/238 assert no bracket-less `= phi` is printed after those) as where auto-erase does ship. Parser check: orig_cpp/.../llvm/lib/AsmParser/LLParser.cpp:8314-8355 shows parsePHI breaking out on a non-lsquare first token and constructing a zero-incoming PHINode rather than erroring.

</details>

### 67. `DIExpression` elements stored as source spelling, not DWARF encodings

*IR model / metadata, parser* — crates/llvmkit-ir/src/metadata.rs:2100-2107 (model), crates/llvmkit-asmparser/src/ll_parser.rs:5440-5478 (parser), stale reasons at crates/llvmkit-asmparser/src/ll_parser.rs:5433-5439 and crates/llvmkit-ir/src/metadata.rs:2092-2099

- **LLVM:** `LLParser::parseDIExpressionBody` maps each `DW_OP_*` / `DW_ATE_*` through `dwarf::getOperationEncoding` / `getAttributeEncoding` and stores a `uint64_t` in `DIExpression::Elements`. Downstream, `DIExpression::isValid()` walks those encodings and checks each operation's operand count and shape; `AsmWriter::writeDIExpression` prints them back by name.
- **llvmkit:** `DwarfExpressionOperand::Operation(String)` keeps the written spelling. The parser does validate the spelling against `llvmkit_ir::dwarf::operation_encoding` / `attribute_encoding` and rejects an unknown one, so the *parse* path matches upstream — but the model itself carries no encodings, so nothing performs `DIExpression::isValid()`'s operand-arity checking, and a `DIExpression` built through the IR API (rather than parsed) can hold an arbitrary `Operation(String)` that prints straight back out.
- **Why:** Recorded at `metadata.rs:2092-2099` and `ll_parser.rs:5433-5439`: the `Dwarf.def` tables were unmodelled when this landed, and `writeDIExpression` prints a known op back by name regardless, so the written form is what round-trips. Note the two recorded texts are now stale in opposite directions — `ll_parser.rs:5438` claims an unrecognised op "round-trips rather than being rejected" and `metadata.rs:2098` claims it is "accepted here where upstream rejects it", but the code immediately below `ll_parser.rs:5450` rejects it by name, and `dwarf.rs` does model the tables.
- **Fix:** Two steps. First, correct the two stale comments — the parser no longer accepts unknown ops, and `dwarf.rs` is a drift-locked transcription of `Dwarf.def`, so the recorded premise no longer holds. Second, change `DwarfExpressionOperand::Operation(String)` to carry the resolved `u32` encoding alongside (or instead of) the spelling, and port `DIExpression::isValid()` on top of it so operand-arity errors are caught; the printer maps the encoding back through the existing `dwarf` table, exactly as `writeDIExpression` does.
- **Correction from verification:** Accurate as written; two refinements. (1) The "stale reasons" charge applies only to the two in-code doc comments (crates/llvmkit-asmparser/src/ll_parser.rs:5433-5439 and crates/llvmkit-ir/src/metadata.rs:2092-2099), both of which assert the Dwarf.def tables are unmodelled and that an unknown DW_OP round-trips rather than being rejected — both false. The backlog entry at docs/future-work.md:1198-1212 is NOT stale: it correctly records that half the gap closed in LLParser-parity W11 (validation on the way in) and that only storage remains. (2) The printer consequence is broader than the claim states. Upstream's AsmWriter::writeDIExpression branches on isValid(): a valid expression prints operation names and decodes DW_OP_LLVM_convert's second argument through dwarf::AttributeEncodingString, while an INVALID one falls through to printing the raw uint64_t elements. llvmkit's writer (crates/llvmkit-ir/src/asm_writer.rs:3471-3482) has no such branch and prints the stored spelling unconditionally, so an expression upstream would dump as bare numbers prints as names here. The reverse normalisation also diverges: a numeric source element such as !DIExpression(15) prints back as 15 in llvmkit where llvm-dis prints the operation name that value encodes.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/metadata.rs:2100-2107 defines `pub enum DwarfExpressionOperand { Operation(String), Literal(u64) }` — source spelling, no encoding, confirming the model half. crates/llvmkit-asmparser/src/ll_parser.rs:5455-5478 calls `llvmkit_ir::dwarf::operation_encoding(&name).is_none()` / `attribute_encoding(&name).is_none()` and returns ParseError::InvalidMetadataFieldValue on failure, then pushes `DwarfExpressionOperand::Operation(name)` — the encoding is looked up, tested, and thrown away; upstream LLParser.cpp `parseDIExpressionBody` (LLParser.cpp:6257-6297) performs the identical lookup via `dwarf::getOperationEncoding` / `getAttributeEncoding` but does `Elements.push_back(Op)`. Upstream Verifier.cpp:1813-1815 `visitDIExpression` is `CheckDI(N.isValid(), "invalid expression", &N)`, and DebugInfoMetadata.cpp:1725+ `DIExpression::isValid()` walks `expr_ops()` checking per-op operand counts, fragment-must-be-last, stack_value placement, and entry_value position. llvmkit has no equivalent: grepping `DiExpression|expression_operands|invalid expression` across crates/llvmkit-ir/src/ outside metadata.rs returns a single hit (asm_writer.rs:3573), and crates/llvmkit-ir/src/verifier.rs never mentions DIExpression. Nothing guards API construction: `Operation(String)` is a public tuple variant, `with_expression_operands` (metadata.rs:2198-2206) only `extend`s, `into_stored` (metadata.rs:2254-2256) is a plain move commented "Operands carry no metadata reference, so there is no tag to check", and asm_writer.rs:3477 emits `f.write_str(name)` verbatim. The staleness of the two cited comments is settled by crates/llvmkit-ir/src/dwarf.rs (675 lines of generated DW_* tables with `pub fn operation_encoding`/`attribute_encoding`, guarded by dwarf_def_drift.rs) and by the rejection code sitting 16 lines below ll_parser.rs:5439's claim that an unrecognised op "round-trips rather than being rejected".

</details>

### 68. `match_select_pattern` ignores fast-math flags written on the `select`

*analysis / SelectPattern* — crates/llvmkit-ir/src/select_pattern.rs:395 (definition), reason at crates/llvmkit-ir/src/select_pattern.rs:389-394

- **LLVM:** `llvm::matchSelectPattern` reads `SI->getFastMathFlags()` when the `select` is an `FPMathOperator`, and uses `nnan` / `nsz` from the select itself to admit float min/max idioms that the `fcmp`'s own flags do not justify.
- **llvmkit:** llvmkit's `select` instruction carries no fast-math flag word, so those flags cannot be consulted. Flags on the `fcmp` are read. Some float min/max patterns upstream recognises are declined here.
- **Why:** Recorded inline at `select_pattern.rs:389-394`: "llvmkit's `select` carries no flag word, so `nnan` / `nsz` written on the select cannot be consulted. Flags on the `fcmp` *are* read, which is where they normally sit. Some float patterns upstream accepts are therefore declined here — never the reverse."
- **Fix:** The blocker is the IR model, not the analysis: add a `FastMathFlags` field to `SelectInstData` (every other `FPMathOperator` in llvmkit already carries one), have the parser and `IrBuilder::select*` set it, print it in `AsmWriter`, and then read it here exactly as upstream does. Until then the divergence is also a round-trip gap — `select nnan float ...` loses its flags on reprint — which makes this higher-value than the analysis weakness alone suggests.
- **Correction from verification:** The behavioral divergence is real and still present, but the stated reason is stale and wrong. `match_select_pattern` does still discard the select's own fast-math flags — at C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/select_pattern.rs:419-427 it hands `FastMathFlags::empty()` to `match_decomposed_select_pattern` where upstream passes `isa<FPMathOperator>(SI) ? SI->getFastMathFlags() : FastMathFlags()`. But the premise "llvmkit's `select` instruction carries no fast-math flag word, so those flags cannot be consulted" is false today. `SelectInstData` has carried `fmf: Cell<FastMathFlags>` since commit 8b2e3de ("feat(ir,asmparser)!: fast-math flags on select, phi, fptrunc and fpext", confirmed an ancestor of HEAD): the parser stores them (`select_erased_with_fmf`), `SelectInst::fast_math_flags()` is public, and AsmWriter prints them. So this is no longer a data-model gap — it is a one-line omission in the analysis, fixable by replacing `FastMathFlags::empty()` with `select.fmf.get()`. The rest of the description holds: the fcmp's `nnan` is propagated (select_pattern.rs:449-453, mirroring `CmpI->hasNoNaNs()`), `nsz` can only reach the matcher from the select itself or the FPToSI/FPToUI cast path, and `nsz`-gated declines at select_pattern.rs:586 and :672 mean llvmkit rejects float min/max idioms upstream accepts — never the reverse. The doc comment at select_pattern.rs:389-394 asserting the missing flag word should be corrected along with the code.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/select_pattern.rs:395-427 — `match_select_pattern` is defined at line 395 as the claim says, and its tail call passes a literal `FastMathFlags::empty()` as the `fast_math_flags` argument; the select's `SelectInstData` is destructured at line 404 but only `cond`/`true_val`/`false_val` are read. Lines 389-394 hold the "Fast-math flags on the `select` are not read" doc paragraph, including the now-false justification. Compared against orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ValueTracking.cpp:9071-9090 (`llvm::matchSelectPattern` forwards `isa<FPMathOperator>(SI) ? SI->getFastMathFlags() : FastMathFlags()`) and :9092-9127 (`matchDecomposedSelectPattern`, whose `if (isa<FPMathOperator>(CmpI) && CmpI->hasNoNaNs()) FMF.setNoNaNs();` and FPToSI/FPToUI `FMF.setNoSignedZeros()` both have exact llvmkit counterparts at select_pattern.rs:449-453 and :480-482). Refuting the "no flag word" premise: crates/llvmkit-ir/src/instr_types.rs:2350-2368 shows `SelectInstData { cond, true_val, false_val, fmf: Cell<FastMathFlags> }` with a doc comment about LLParser applying the flags after `parseSelect`; crates/llvmkit-ir/src/instructions.rs:1055-1059 is the public `SelectInst::fast_math_flags()`; crates/llvmkit-ir/src/asm_writer.rs:3825-3831 prints them after the `select` opcode; crates/llvmkit-asmparser/src/ll_parser.rs:12293 parses them via `select_erased_with_fmf` (builder at crates/llvmkit-ir/src/ir_builder.rs:2171). `git merge-base --is-ancestor 8b2e3de HEAD` succeeded, and neither select_pattern.rs nor instr_types.rs appears in the working-tree modification list, so all of this is committed state at HEAD.

</details>

### 69. Shuffle mask transforms model the IR alphabet only

*analysis / VectorUtils* — crates/llvmkit-ir/src/vector_utils.rs:34-43 (module header), crates/llvmkit-ir/src/vector_utils.rs:208-213

- **LLVM:** `widenShuffleMaskElts`, `narrowShuffleMaskElts` and friends read mask elements as raw `int`s, so the same functions serve both the IR alphabet `{lane, poison}` and the wider one SelectionDAG and the X86 backend use, where `SM_SentinelZero` is `-2`. The widening rule is "negatives must be *equal* across a widened group", which distinguishes `-1` from `-2`.
- **llvmkit:** The transforms take `&[ShuffleMaskElem]`, which has only `Lane(n)` and `Poison`, so the equal-negatives rule collapses to "all poison". Three upstream assertions in `VectorUtilsTest.cpp` have no llvmkit spelling as a result.
- **Why:** Recorded in the module header as permanent, not pending: "code generation and target backends are out of scope", so no mask llvmkit can hold distinguishes the two negatives and the difference is unobservable in-tree. `widen_shuffle_mask_elements` repeats it at its site, and `tests/vector_utils_masks.rs` records the three unportable assertions.
- **Fix:** No fix warranted — the narrowing follows from the project charter, and the recorded reason is verified: `ShuffleMaskElem` genuinely has no sentinel-zero variant. The only action is to keep the three unportable upstream assertions visible: they are already listed in `tests/vector_utils_masks.rs`, and they should carry `UPSTREAM.md` rows marked "no llvmkit spelling" so the coverage gap reads as deliberate rather than missing.
- **Correction from verification:** Accurate as described; only the second line citation is stale. The site note is on `widen_shuffle_mask_elements` (crates/llvmkit-ir/src/vector_utils.rs:851-860), not :208-213 — that range is now inside `is_splat_value`'s doc comment. The module-header citation (:34-43) is exact. Two further precisions: the alphabet restriction is enforced at the type level *and* at decode time — `ShuffleMaskElem::from_encoded` (crates/llvmkit-ir/src/instr_types.rs:2607) maps every negative to `Poison`, so `{-2,-2,-3,-3}` is not merely unspellable but unrepresentable; and the two widen fixtures are re-asserted a second time through `getShuffleMaskWithWidestElts` upstream, so "three assertions" counts distinct fixtures rather than distinct EXPECT lines (llvmkit's own test header says the same and flags the duplication).

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/instr_types.rs:2575-2613 — `pub enum ShuffleMaskElem { Lane(u32), Poison }`, with `from_encoded(value: i64)` collapsing any negative (`u32::try_from` Err) to `Poison`. crates/llvmkit-ir/src/vector_utils.rs — all five transforms (`narrow_shuffle_mask_elements` :804, `widen_shuffle_mask_elements` :861, `widen_shuffle_mask_elements_in_pairs` :926, `scale_shuffle_mask_elements` :966, `shuffle_mask_with_widest_elements` :999) take `&[ShuffleMaskElem]`; the widen negative arm is `ShuffleMaskElem::Poison => { if !slice.iter().all(|e| *e == ShuffleMaskElem::Poison) { return None } scaled.push(ShuffleMaskElem::Poison) }` (:890-895), i.e. "all poison". Module header :34-43 states the narrowing verbatim; widen's doc :851-860 names `SM_SentinelZero` and the two absent fixtures. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/VectorUtils.cpp, `llvm::widenShuffleMaskElts(int Scale, ArrayRef<int>, SmallVectorImpl<int>&)` — `if (SliceFront < 0) { if (!all_equal(MaskSlice)) return false; ScaledMask.push_back(SliceFront); }`, propagating the sentinel value itself. orig_cpp/.../llvm/unittests/Analysis/VectorUtilsTest.cpp — `narrowShuffleMaskElts(1, {3,2,0,-2}, …)` expecting `{3,2,0,-2}`; `EXPECT_FALSE(widenShuffleMaskElts(2, {-1,-2,-1,-1}, …))`; `EXPECT_TRUE(widenShuffleMaskElts(2, {-2,-2,-3,-3}, …))`; plus the same two masks re-run through `getShuffleMaskWithWidestElts`. crates/llvmkit-ir/tests/vector_utils_masks.rs:10-35 records exactly those three as having no llvmkit spelling, and its `mask()` helper routes through `from_encoded`, so a sentinel fixture would silently collapse to poison rather than test anything.

</details>

### 70. `simplifyPHINode`'s undef blending not mirrored

*passes / InstSimplify* — crates/llvmkit-ir/src/inst_simplify.rs:78 (definition), reason at crates/llvmkit-ir/src/inst_simplify.rs:72-77

- **LLVM:** `llvm::simplifyPHINode` folds a phi whose incomings are one common value *blended with* `undef` — `[X, undef]` simplifies to `X` — in addition to tolerating self-references.
- **llvmkit:** `uniform_phi_value` ignores only self-referencing incomings; an `undef` incoming makes the phi mixed, so the fold is declined. llvmkit's InstSimplify pass simplifies strictly fewer phis.
- **Why:** Recorded inline at `inst_simplify.rs:75-77`: "Undef blending (upstream folds `[X, undef]` to `X`) is deliberately not mirrored here; it is documented as out of scope." The recorded reason names a scope decision but not what blocks it — llvmkit does model `undef` constants, so the blocker is not obviously representational.
- **Fix:** Verify the recorded premise before extending — `undef` is representable (`ConstantData::Undef`), so this looks closable rather than blocked. Extend `uniform_phi_value` to skip incomings that are `undef` alongside self-references, returning `None` only when two distinct non-undef, non-self values appear, and return the common value when at least one non-undef incoming exists. Port the matching `test/Transforms/InstSimplify/phi.ll` cases in the same commit.
- **Correction from verification:** Real and still present, but narrower than described. Correct statement: upstream `llvm::simplifyPHINode` (InstructionSimplify.cpp) skips self-referencing, `PoisonValue`, and `Q.isUndefValue` incomings, so `[X, undef]` and `[X, poison]` fold to `X` (guarded by `valueDominatesPHI`, plus `isGuaranteedNotToBePoison` when an undef input is present). llvmkit's `uniform_phi_value` (crates/llvmkit-ir/src/inst_simplify.rs:78) skips only self-references, so an undef or poison incoming makes the phi mixed and the fold is declined. However, the claim's blanket "an undef incoming makes the phi mixed, so the fold is declined" overstates the pass-level effect: `InstSimplifyPass::run` calls `constant_fold_instruction` first (inst_simplify.rs:49), and its `fold_phi` (crates/llvmkit-ir/src/constant_folding.rs:1354-1383) DOES skip undef and poison incomings, so `phi [i32 7, undef]` folds to `7` today, and an all-undef/poison phi folds to undef. The surviving gap is exactly the case where the common value is a non-constant: `phi [%x, undef]` / `phi [%x, poison]` with `%x` an instruction or argument — `fold_phi` bails at the first non-constant incoming and `uniform_phi_value` then sees a mixed phi. The claim also omits that poison blending is missing on the same path, and that llvmkit's constant-path blending carries neither of upstream's dominance / not-poison guards (moot there, since constants satisfy both).

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/inst_simplify.rs (working tree == HEAD; `git status --short` does not list this file): line 78 defines `fn uniform_phi_value`, its doc comment at 72-77 states verbatim that "Undef blending (upstream folds `[X, undef]` to `X`) is deliberately not mirrored here; it is documented as out of scope", and the loop at 86-95 `continue`s only on `value == self_value`, returning `None` on any second distinct incoming. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/InstructionSimplify.cpp, `simplifyPHINode`: the incoming loop skips `Incoming == PN`, sets `HasPoisonInput` and continues on `isa<PoisonValue>`, sets `HasUndefInput` and continues on `Q.isUndefValue(Incoming)`, and only then bails on `CommonValue && Incoming != CommonValue`; a null `CommonValue` yields `UndefValue::get`/`PoisonValue::get`, and a non-null one with `HasPoisonInput || HasUndefInput` is returned after `valueDominatesPHI` and (for undef) `isGuaranteedNotToBePoison`. crates/llvmkit-ir/src/constant_folding.rs:364-379 shows `constant_fold_instruction` dispatching `InstructionKindData::Phi` to `fold_phi`, and `fold_phi` at 1354-1383 `continue`s on `is_undef(constant) || is_poison(constant)` (returning `ty.undef()` when nothing else remains) but returns `Ok(None)` as soon as an incoming is not a constant — which is what confines the divergence to non-constant common values. No other phi-simplification path exists: grep for `uniform_phi_value`/`simplifyPHINode` outside orig_cpp hits only this pass, its tests in crates/llvmkit-ir/tests/scalar_cleanup_passes.rs, and planning docs.

</details>

### 71. getVScaleRange gap row cites a blocker that no longer exists

*ValueTracking parity ledger* — crates/llvmkit-ir/tests/value_tracking_parity.rs:495-498; contradicted by crates/llvmkit-asmparser/tests/attribute_td_drift.rs:34-38, crates/llvmkit-ir/src/attributes.rs:819-830, crates/llvmkit-asmparser/src/ll_parser.rs:9974-9981, crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:966-968

- **LLVM:** `llvm::getVScaleRange` (`ValueTracking.h`) reads the `vscale_range` function attribute's (min, max) pair and returns a `ConstantRange`.
- **llvmkit:** Listed in `VALUE_TRACKING_GAPS` as "blocked on the `vscale_range` attribute itself, which attribute_td_drift.rs lists as NOT_YET_MODELED: upstream reads a packed (min, max) pair and llvmkit's payload is a single u64, so porting it would mean inventing the second half." **Both halves of that reason are false today.** `attribute_td_drift.rs`'s `NOT_YET_MODELED` is `&[]` with a doc saying "**The list is empty**", and the payload is not a u64 — `Attribute::VScaleRange { min: u32, max: Option<u32> }` is a structured pair with an explicit note that `Option` is what keeps "unbounded" apart from "max defaults to min". `LLParser::parseVScaleRangeArguments` is ported as `parse_vscale_range_attribute`, and `vscale_range(1,16)` / `vscale_range(4,4)` round-trip in a passing test.
- **Why:** Unrecorded — the row was never revised after `vscale_range` landed. This is the same failure mode `vector_utils_parity.rs` documents for its own table ("An earlier revision of this table recorded eight of these as blocked on 'needs `Intrinsic::ID`', and that reason was wrong").
- **Fix:** Port `getVScaleRange` against `Attribute::VScaleRange { min, max }` — `max: None` maps to upstream's packed-`0` unbounded case — move the symbol from `VALUE_TRACKING_GAPS` to the modeled table (the surface-accounting assertion in `value_tracking_surface_is_accounted_for` keeps the counts honest), and port upstream's `ValueTrackingTest` vscale fixtures.
- **Correction from verification:** Accurate as stated, and still present at HEAD (dev @ 2ac3e3a). Two refinements: (1) The gap row's *listing* is correct — there is no port of `getVScaleRange` anywhere in the tree (no `vscale_range`-reading function exists in `llvmkit-ir/src/value_tracking.rs` or elsewhere; a repo-wide grep for `fn vscale_range|vscale_range_min|vscale_range_max|pub fn .*vscale` returns zero matches). Only the row's stated *reason* is false. The fix is to rewrite the reason, not to delete the row. (2) The same stale premise appears in two places the claim does not cite, and one of them is production source, not a test: - `crates/llvmkit-ir/src/value_tracking.rs:2558-2559` — the rustdoc on `is_known_to_be_a_power_of_two` says the `vscale` arm is omitted because "`vscale_range` is on `attribute_td_drift.rs`'s `NOT_YET_MODELED` list, so the attribute it reads does not exist here". Both clauses are false; the attribute is modeled and parsed. - `docs/future-work.md:748-753` — repeats the packed-`(min, max)`/`NOT_YET_MODELED`/"single-`u64` payload" reason and adds a third false clause, "so the parser cannot even produce a function carrying one", which `parser_attribute_matrix.rs` disproves by parsing, printing and re-parsing `vscale_range(1, 16)` and `vscale_range(4)`. Corrected reason for the row: `getVScaleRange` is unported but no longer blocked — it is a pending port. Everything it needs exists: `Attribute::VScaleRange { min: u32, max: Option<u32> }`, `ConstantRange`, and function-attribute lookup. What remains is the arithmetic body (bit-width poison check, `Option`-max handling, `ConstantRange` construction) plus its two call sites in `computeKnownBits` (the `Intrinsic::vscale` arm) and `getRangeForIntrinsic`. Side observation from the same grep, outside this claim's scope: `ROADMAP.md:837` still says the remaining unmodeled attribute keywords are "`NOT_YET_MODELED`, 42 today", which is also stale against the empty list.

<details><summary>Verification evidence</summary>

Read all four cited anchors plus upstream. 1. `crates/llvmkit-ir/tests/value_tracking_parity.rs:495-498` — the row is verbatim as quoted, inside `VALUE_TRACKING_GAPS`: `("getVScaleRange", "blocked on the \`vscale_range\` attribute itself, which attribute_td_drift.rs lists as NOT_YET_MODELED: upstream reads a packed (min, max) pair and llvmkit's payload is a single u64, so porting it would mean inventing the second half")`. 2. `crates/llvmkit-asmparser/tests/attribute_td_drift.rs:29-38` — `const NOT_YET_MODELED: &[&str] = &[];`, with the doc reading "**The list is empty** — every attribute `Attributes.td` declares is accepted in every position it declares". So `vscale_range` is not on that list; nothing is. First half of the reason is false. 3. `crates/llvmkit-ir/src/attributes.rs:819-830` — the payload is `VScaleRange { min: u32, max: Option<u32> }`, not a `u64`, with a doc explicitly noting that upstream reserves `0` for unbounded and defaults a missing max to *min*, "and only the `Option` keeps them apart". Second half of the reason is false: the pair is already modeled, nothing needs inventing. 4. `crates/llvmkit-asmparser/src/ll_parser.rs:9976-9995` — `parse_vscale_range_attribute`, documented as porting `LLParser::parseVScaleRangeArguments`, parses `min`, defaults a missing max to `min`, and maps `0` to `None` via `(max > 0).then_some(max)` — mirroring `addVScaleRangeAttr(Min, Max > 0 ? Max : std::nullopt)`. 5. `crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:961-979` — `the_argument_carrying_function_attributes_round_trip` drives `("vscale_range(1, 16)", "vscale_range(1,16)")` and `("vscale_range(4)", "vscale_range(4,4)")` through `parse_print_reparse`. 6. Upstream `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ValueTracking.cpp:1279-1296` — `ConstantRange llvm::getVScaleRange(const Function *F, unsigned BitWidth)` does `F->getFnAttribute(Attribute::VScaleRange)`, then `Attr.getVScaleRangeMin()` and `std::optional<unsigned> AttrMax = Attr.getVScaleRangeMax()`; absent attribute gives `[1, 0)`, min wider than `BitWidth` gives empty, absent/too-wide max gives `[Min, 0)`, else `[Min, Max+1)`. Note upstream's own accessor pair is `(unsigned, std::optional<unsigned>)` — structurally identical to llvmkit's `(u32, Option<u32>)`, so the claimed representational mismatch does not exist in either direction. Callers at lines 2230, 2253 (`Intrinsic::vscale`) and 10163. 7. Repo-wide grep for `NOT_YET_MODELED` surfaced the two uncited repetitions of the same stale premise: `crates/llvmkit-ir/src/value_tracking.rs:2559` and `docs/future-work.md:750`. Grep for any vscale-range accessor or port in `crates/` returned nothing, confirming the entry point itself is still genuinely unmodeled.

</details>

### 72. SqrtNszSignBit %A3/%A4 unported behind a nofpclass blocker that is closed

*ValueTracking / known-FP-class* — crates/llvmkit-asmparser/tests/known_fp_class.rs:547-607; contradicted by crates/llvmkit-asmparser/tests/parser_nofpclass.rs, crates/llvmkit-ir/src/known_fp_class.rs:150-153,651-674

- **LLVM:** `ComputeKnownFPClassTest.SqrtNszSignBit` (`llvm/unittests/Analysis/ValueTrackingTest.cpp`) declares `float nofpclass(nan) %arg.nnan` and adds two more blocks: `%A3 = call float @llvm.sqrt.f32(float %arg.nnan)` expecting `fcPosInf | fcPosNormal | fcZero | fcQNan` and `%A4` the `nsz` variant.
- **llvmkit:** Only `%A` and `%A2` are ported. The doc says the rest "need a `nofpclass(nan)` parameter attribute, which llvmkit does not model". **That is stale.** `nofpclass` is fully modeled: every component keyword round-trips in `parser_nofpclass.rs`, `Attribute::NoFpClass(mask)` prints in `NoFPClassName` order, and `known_fp_class.rs`'s `no_fp_class_of` explicitly ports "`CallBase::getRetNoFPClass` for a call, and `Argument::getNoFPClass` for a parameter" as the opening read of `KnownNotFromFlags`.
- **Why:** Unrecorded — the reason predates the `nofpclass` work and was not revisited. `docs/future-work.md` is cited as holding it, so the entry there is stale too.
- **Fix:** Restore the parameter `float nofpclass(nan) %arg.nnan` to the fixture IR and add the `%A3`/`%A4` blocks with upstream's two exact masks (`fcPosInf|fcPosNormal|fcZero|fcQNan` for both the with-flags and without-flags reads of `%A3`). Then drop the stale claim from the doc comment and from `docs/future-work.md`.

<details><summary>Verification evidence</summary>

Confirmed on every leg, including empirically. 1. Upstream (C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/unittests/Analysis/ValueTrackingTest.cpp, `TEST_F(ComputeKnownFPClassTest, SqrtNszSignBit)` at line 2074) parses `define float @test(float %arg, float nofpclass(nan) %arg.nnan)` and has four blocks. `%A3` expects `fcPosInf | fcPosNormal | fcZero | fcQNan` under both UseInstrInfo=true and false; `%A4` expects `fcPosInf | fcPosNormal | fcPosZero | fcQNan` with flags and `fcPosInf | fcPosNormal | fcZero | fcQNan` without. Sign bit `std::nullopt` throughout. 2. llvmkit's port (C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/known_fp_class.rs, `sqrt_nsz_is_hidden_from_a_query_that_ignores_instruction_flags` at line 562) parses only `%A` and `%A2` — no `%arg.nnan` parameter, no `%A3`/`%A4` anywhere in the file (grep for `arg.nnan` returns nothing; the only `A3` hits are an unrelated `sitofp` fixture at line 247). 3. The recorded reason is stale, exactly as claimed. The doc comment at lines 554-556 reads: "Its `%A3`/`%A4` blocks are **not** ported: they need a `nofpclass(nan)` parameter attribute, which llvmkit does not model". But `nofpclass` is modeled end to end today: - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/parser_nofpclass.rs — `every_nofpclass_component_round_trips`, `a_multi_class_mask_prints_in_upstream_order`, and `nofpclass_round_trips_on_parameters_and_returns`, whose fixture is literally `define nofpclass(nan) float @test(float nofpclass(nan inf) %nnan.ninf, float nofpclass(nan) %nnan, ...)` — i.e. the exact shape `SqrtNszSignBit` needs, on a `define` header. - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/known_fp_class.rs:651-674, `fn no_fp_class_of`, documented as porting "`CallBase::getRetNoFPClass` for a call, and `Argument::getNoFPClass` for a parameter"; its `Argument` arm calls `function_no_fp_class(value, *parent_fn, AttrIndex::Param(*slot))`, backed by `AttributeList::no_fp_class` at crates/llvmkit-ir/src/attributes.rs:1751. It is consumed at known_fp_class.rs:153 as the opening read of `ruled_out` (upstream's `KnownNotFromFlags`). - `FpClassTest::QUIET_NAN` exists (crates/llvmkit-ir/src/fp_class.rs:46), so upstream's `fcQNan` expectation is spellable. 4. Empirical proof: I wrote a throwaway integration test that parses upstream's exact four-block fixture and asserts upstream's exact four masks for `%A3`/`%A4` (both UseInstrInfo settings), then ran `cargo +1.96.0 test --release -p llvmkit-asmparser`. It passed on the first try — printed masks 866 (`fcPosInf|fcPosNormal|fcZero|fcQNan`) and 834 (`fcPosInf|fcPosNormal|fcPosZero|fcQNan`), sign bit `None` throughout. So the port is not merely unblocked, it is already correct; nothing needs implementing, only the two blocks need writing down. I deleted the probe file afterwards (`crates/llvmkit-asmparser/tests/zz_scratch_verify_claim50.rs`); other `zz_*` files in that directory are not mine. 5. Provenance of the staleness, from git: the "does not model" comment landed in 1aae954 ("feat(value-tracking): the floating-point select arm and the context arm", 2026-08-04); `no_fp_class_of` landed later the same day in its descendant 25925f3 ("feat(valuetracking): computeKnownFPClass's arithmetic arms, and the nofpclass they read"). The comment was simply never revisited. One addition to the claim's scope: the stale reason is recorded in three places, not one. Besides known_fp_class.rs:554-556, it appears at C:/Users/olegg/Desktop/llvmkit/UPSTREAM.md:2037 ("its `%A3`/`%A4` blocks need a `nofpclass` parameter attribute, which llvmkit does not model | partial port") and C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:710-711 ("`nofpclass` on a parameter or call return is still unmodeled, which is why two of `SqrtNszSignBit`'s four blocks are not ported"). All three need correcting when the two blocks are added.

</details>

### 73. flags.ll vector trunc functions unported behind a closed blocker

*parser (instruction modifiers)* — crates/llvmkit-asmparser/tests/parser_modifiers.rs:76-98; UPSTREAM.md:882-883; contradicted by crates/llvmkit-asmparser/src/ll_parser.rs:11600-11615 and crates/llvmkit-asmparser/tests/parser_vector_casts.rs:76-88

- **LLVM:** `test/Assembler/flags.ll` carries `@test_trunc_signed_vector`, `@test_trunc_unsigned_vector`, `@test_trunc_both_vector` and `@test_trunc_both_reversed_vector`, whose CHECK lines pin `trunc nuw nsw <2 x i64> %a to <2 x i32>` and the reversed spelling printing canonically.
- **llvmkit:** Only the scalar `@test_trunc_both` / `@test_trunc_both_reversed` are ported, with the reason "the upstream vector form needs vector int-cast support, which parse_int_cast lacks". **`parse_int_cast` has that support.** Its vector branch builds `IntCastFlags` carrying `nuw`/`nsw`/`nneg` and routes through `int_cast_erased`, and `parser_vector_casts.rs` already round-trips `%t = trunc nuw nsw <2 x i32> %x to <2 x i16>`.
- **Why:** Unrecorded — the reason survived the vector-cast work unchanged, and is mirrored verbatim into two `UPSTREAM.md` rows.
- **Fix:** Vendor the four vector functions into the two `fixtures/upstream/flags/*.ll` files as upstream spells them, extend the `assert_check_lines` lists, and correct the doc comments and both `UPSTREAM.md` rows.
- **Correction from verification:** Accurate, but understated. The four vector trunc functions from test/Assembler/flags.ll (@test_trunc_signed_vector, @test_trunc_unsigned_vector, @test_trunc_both_vector, @test_trunc_both_reversed_vector) are indeed unported behind a blocker reason that is false: parse_int_cast fully supports vector integer casts with nuw/nsw/nneg. I empirically confirmed all four parse, verify, and print byte-exact against their upstream CHECK lines today, including the reversed `trunc nsw nuw` canonicalizing to `trunc nuw nsw <2 x i64> %a to <2 x i32>`. Extension to the claim: the scalar single-flag forms @test_trunc_signed and @test_trunc_unsigned are ALSO unported (nothing in the tree matches `trunc nsw i64` or `trunc nuw i64`), so six of upstream's eight flags.ll trunc functions lack a ported counterpart, not four -- and those two were never covered by the stated blocker at all. Also, the stale blocker text appears in four places, not two: parser_modifiers.rs:77-78 and 88-90, plus the fixture headers tests/fixtures/upstream/flags/nuw_nsw_trunc_round_trips.ll:4-6 and nsw_nuw_reversed_trunc_round_trips.ll:4-6 ("llvmkit's parse_int_cast does not support vector integer casts yet"), plus the two UPSTREAM.md rows. The fix is a test/docs change, not a parser change.

<details><summary>Verification evidence</summary>

orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/flags.ll lines 242-289: eight trunc functions, four scalar and four vector, with CHECK lines pinning `trunc nsw/nuw/nuw nsw <2 x i64> %a to <2 x i32>`; RUN line is `llvm-as | llvm-dis | FileCheck`, so the CHECKs are AsmWriter output. crates/llvmkit-asmparser/tests/parser_modifiers.rs:76-98: only nuw_nsw_trunc_round_trips and nsw_nuw_reversed_trunc_round_trips exist, both scalar, both doc-commented "the upstream vector form needs vector int-cast support, which parse_int_cast lacks". UPSTREAM.md:882-883 repeats "(scalar; vector form blocked on vector int-cast support)". CONTRADICTED BY crates/llvmkit-asmparser/src/ll_parser.rs:11576-11615: parse_int_cast eats nuw/nsw in either order at lines 11586-11590 (before any type is parsed, so vector-agnostic like LLParser::parseCast), then lines 11606-11615 take an `if is_vector_type(src_ty) || is_vector_type(dst_ty)` branch building IntCastFlags::new().nuw()/.nsw()/.nneg() and routing through int_cast_erased. crates/llvmkit-asmparser/tests/parser_vector_casts.rs:82-93 already round-trips `%t = trunc nuw nsw <2 x i32> %x to <2 x i16>`. EMPIRICAL: I wrote a throwaway integration test with all four upstream vector functions verbatim and ran `cargo +1.96.0 test --release -p llvmkit-asmparser`; it passed first try, printing `%res = trunc nsw <2 x i64> %a to <2 x i32>`, `trunc nuw <2 x i64>...`, and `trunc nuw nsw <2 x i64>...` twice (the reversed one canonicalized), and module.verify() succeeded. Temp file deleted; tree unchanged. A grep for `trunc nsw`/`trunc nuw`/`test_trunc` across crates/, UPSTREAM.md and docs/ shows no coverage of the scalar single-flag forms either.

</details>

### 75. dbg-record-invalid-5.ll vendored with no reference and no recorded blocker

*parser corpus / debug records* — crates/llvmkit-asmparser/tests/fixtures/upstream/dbg-record-invalid/dbg-record-invalid-5.ll (unreferenced); sibling coverage at crates/llvmkit-asmparser/tests/parser_debug_metadata.rs:790-848

- **LLVM:** `test/Assembler/dbg-record-invalid-5.ll` tests that a basic block containing *only* a debug record is a parse error, CHECKing `expected instruction opcode` at the closing brace.
- **llvmkit:** The fixture is checked in and referenced **nowhere** — no test, no manifest row, no `UPSTREAM.md` row, and no entry in `docs/future-work.md`. Its siblings `-1/-2/-3/-4/-6/-7/-8` all have tests, and `-0` is exercised via a manifest copy (`corpus_dbg_record_after_terminator_invalid.ll`, `status=reject` with its message and span pinned).
- **Why:** Unrecorded. Nothing in the tree states why this one split was skipped while its seven siblings were ported.
- **Fix:** Add a `DBG_RECORD_INVALID_5` const beside the others in `parser_debug_metadata.rs` and assert `expected instruction opcode`; if llvmkit answers something else, that is the finding to record. Also fold the now-duplicate `upstream/dbg-record-invalid/dbg-record-invalid-0.ll` (unreferenced) into the manifest row that currently points at a private copy.
- **Correction from verification:** The coverage gap is real and still present, but the title's "no recorded blocker" is misleading: there is no blocker. Accurate statement: `crates/llvmkit-asmparser/tests/fixtures/upstream/dbg-record-invalid/dbg-record-invalid-5.ll` is vendored verbatim from upstream and git-tracked (committed in f68bee0, LLParser parity W11) but referenced nowhere in the tree — no `include_str!`, no `parser_corpus_manifest.txt` row, no `UPSTREAM.md` row, no `docs/future-work.md` entry — while siblings -1/-2/-3/-4/-6/-7/-8 are all tested in `parser_debug_metadata.rs` and -0 is exercised via the derived corpus copy `corpus_dbg_record_after_terminator_invalid.ll`. Crucially, llvmkit's parser already produces exactly upstream's message on this input (`expected instruction opcode`), so this is a pure test-coverage/provenance gap — a dead vendored fixture — not a behavioral divergence, and unlike sibling -4 it is not blocked on the W14 `Token::Error` re-layering. Minor addition: the vendored `dbg-record-invalid-0.ll` is also never `include_str!`'d by path (only its hand-derived corpus copy carries that content), so the directory holds two files no code path opens, though -0's content is covered.

<details><summary>Verification evidence</summary>

1) Read C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/fixtures/upstream/dbg-record-invalid/dbg-record-invalid-5.ll and C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/dbg-record-invalid-5.ll — byte-identical; upstream CHECK line is `<stdin>:[[@LINE+1]]:1: error: expected instruction opcode` on a block whose only content is a `#dbg_value(!DIArgList(...))` record. 2) Repo-wide grep for `dbg-record-invalid-5` returned zero matches; grep for `dbg-record-invalid` returned only UPSTREAM.md:2147-2152 (rows for -1, -3, -4, -2/-6, -7/-8, plus a DIArgList row), parser_debug_metadata.rs:768-781 `include_str!` constants for -1/-2/-3/-4/-6/-7/-8 with tests at lines 789-848, and parser_corpus_manifest.txt:6 (`corpus_dbg_record_after_terminator_invalid.ll | test/Assembler/dbg-record-invalid-0.ll | status=xfail-parse`). 3) `git ls-files` on the fixture dir shows all nine files tracked; `git log` shows -5 added in f68bee0. 4) No `read_dir`/directory-walking test exists in crates/llvmkit-asmparser/tests, so nothing picks the fixture up implicitly. 5) Behavior probe: a temporary test target (since deleted) ran the fixture through `Parser::new(...).parse_module()` under `cargo +1.96.0 test --release -p llvmkit-asmparser` and printed `PROBE55: ERROR = expected instruction opcode` — matching upstream exactly. 6) Contrast: parser_debug_metadata.rs:807-827 documents sibling -4's genuine blocker (W14 `Token::Error` re-layering) and asserts llvmkit's divergent `unknown keyword 'dbg_invalid'`; -5 requires no such caveat.

</details>

### 76. Unported APInt/APFloat upstream tests for unmodeled surface

*ADT ports* — crates/llvmkit-ir/tests/ap_int_upstream.rs:5-9,390-396; crates/llvmkit-ir/tests/ap_int_upstream_ops.rs:82-87; crates/llvmkit-ir/tests/ap_float_upstream_predicates.rs:5-10,205-206; crates/llvmkit-ir/tests/ap_float_from_string.rs:9-12

- **LLVM:** `llvm/unittests/ADT/APIntTest.cpp` covers `GCD`, `SolveQuadraticEquationWrap`, `clmul`, the rotate family and the `tc*` word-level primitives; `APFloatTest.cpp` covers `Float8*`, `Float6*`, `Float4E2M1FN` and `FloatTF32` semantics and asserts through `classify() -> FPClassTest`.
- **llvmkit:** Those tests are not ported. `ap_int_upstream.rs` states the APIs do not exist; `ap_float_upstream_predicates.rs` omits the unmodeled semantics and notes each `classify()` line as unmodeled rather than approximating it with the coarser `ApFloatCategory`. `TEST(APIntTest, nearestLogBase2)`'s final `APInt(UINT32_MAX, 0)` row is deliberately dropped (half a gigabyte to re-check an answer an adjacent row already checks). The three `APFloat` string-parsing fixtures are marked `llvmkit-specific subset` rather than claiming to be ports.
- **Why:** Recorded in every case, at the module-doc level, with the missing API or semantics named. `docs/future-work.md` holds the APFloat-string remainder.
- **Fix:** Lowest-cost first: add `FpClassTest`-grained `classify()` to `ApFloat` (the mask type already exists in `llvmkit-ir` for `nofpclass`/known-FP-class) and restore the omitted `classify()` assertions. Then port the three `APFloat` string fixtures verbatim. The `Float8*`/`Float6*`/`FloatTF32` semantics and the `APInt` primitives are genuine model additions, each independent.
- **Correction from verification:** A residue of unported ADT tests is still real, but the claim's specifics are substantially stale on both halves. STILL TRUE: - `ApFloatSemantics` (crates/llvmkit-ir/src/ap_float.rs:14-22) has exactly seven variants (IeeeHalf, Bfloat, IeeeSingle, IeeeDouble, IeeeQuad, X87DoubleExtended, PpcDoubleDouble). `Float8*`, `Float6*`, `Float4E2M1FN` and `FloatTF32` are unmodeled and their `APFloatTest.cpp` rows are unported; the header note at ap_float_upstream_predicates.rs:5-7 is accurate. - `TEST(APIntTest, SolveQuadraticEquationWrap)` and the `tc*` primitives (e.g. `tcDecrement`) are still unported, and docs/future-work.md:492,498 records each with a reason. - `TEST(APIntTest, nearestLogBase2)`'s final `APInt(UINT32_MAX, 0)` row is still deliberately dropped, documented in place at ap_int_upstream_ops.rs:82-87. - ap_float_from_string.rs:1-12 still marks itself `llvmkit-specific subset`, and all its UPSTREAM.md rows (1834-1841) carry that label rather than `port`. NOW FALSE — the claim names four APIs as unmodeled/unported that llvmkit has since gained: 1. GCD: `ApInt::greatest_common_divisor` exists (ap_int.rs:1161) and `TEST(APIntTest, GCD)` is ported verbatim at ap_int_upstream_ops.rs:719-721, with an UPSTREAM.md row marked `port` (line 1921). 2. Rotate family: `rotl`, `rotr`, `rotl_by`, `rotr_by` exist (ap_int.rs:835-869) and `TEST(APIntTest, Rotate)` is ported at ap_int_upstream_ops.rs:123-125 (UPSTREAM.md:1907, `port`). 3. clmul: `carryless_mul` / `carryless_mul_reversed` / `carryless_mul_high` exist (ap_int.rs:930/943/953), and `TEST(APIntTest, clmulr)` and `TEST(APIntTest, clmulh)` are ported (ap_int_upstream_ops.rs:868, 891). Only the plain `TEST(APIntTest, clmul)` (upstream APIntTest.cpp:3826) is still unported — and unlike the other deferrals it is NOT recorded in the future-work.md table, so it is an undocumented gap rather than a deliberate one. 4. classify()/FPClassTest: llvmkit DOES model it. `crates/llvmkit-ir/src/fp_class.rs` ports `llvm::FPClassTest` as `FpClassTest`, and `FpClassTest::of(&ApFloat)` (fp_class.rs:223-250) is the port of `APFloat::classify()`. The `classify()` assertions from `TEST(APFloatTest, isSignaling)` and `TEST(APFloatTest, isDenormal)` are ported as fp_class.rs unit tests at lines 1347 and 1369. Consequently three in-tree comments are now factually wrong and should be corrected: - ap_int_upstream.rs:5-9 still says GCD, clmul and the rotate family are "APIs llvmkit does not have". - ap_float_upstream_predicates.rs:8-10 and :205-206 still say `FPClassTest` is "a finer classification than llvmkit's ApFloatCategory ... which llvmkit does not model". - ap_float_from_string.rs:11-12 says the verbatim port of those fixtures "is recorded as remaining work in docs/future-work.md"; no such entry exists there (grep for fromHexadecimalString/fromStringSpecials/makeNaN returns nothing), and that document instead declares the whole ApFloat/ApInt audit closed as of 2026-08-01. That header also says "three fixtures", but one of the three — `TEST(APFloatTest, makeNaN)` — is now a genuine port (UPSTREAM.md:1851, ap_float_upstream_predicates.rs::make_nan), leaving two.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/tests/ap_int_upstream.rs:5-9 — header still lists GCD/clmul/rotate as APIs llvmkit lacks; crates/llvmkit-ir/src/ap_int.rs:835-869, 930-954, 1161 — rotl/rotr/rotl_by/rotr_by, carryless_mul/_reversed/_high, greatest_common_divisor all exist as public methods citing APIntOps; crates/llvmkit-ir/tests/ap_int_upstream_ops.rs:123, 719, 868, 891 — "Port of TEST(APIntTest, Rotate)/(GCD)/(clmulr)/(clmulh)"; UPSTREAM.md:1907, 1921, 1924, 1925 — those four rows labeled `port`. crates/llvmkit-ir/src/fp_class.rs:3, 28, 223-250 — FpClassTest ports llvm::FPClassTest and FpClassTest::of(&ApFloat) reproduces APFloat::classify's five arms; fp_class.rs:1342-1405 — the classify() assertions from isSignaling and isDenormal ported as tests. crates/llvmkit-ir/src/ap_float.rs:14-22 — ApFloatSemantics has only seven variants, confirming Float8*/Float6*/Float4/TF32 are unmodeled (a repo-wide grep for Float8E/Float6E/Float4E2M1FN/FloatTF32 hits only the test comment). orig_cpp/.../llvm/unittests/ADT/APIntTest.cpp:1565, 1655, 1754, 2812, 3424, 3826, 3854, 3882 — upstream Rotate, tcDecrement, nearestLogBase2, GCD, SolveQuadraticEquationWrap, clmul, clmulr, clmulh. docs/future-work.md:480-499 — the audit section is marked closed (2026-08-01) and its deferral table lists tcDecrement and SolveQuadraticEquationWrap but not clmul, GCD or rotate. crates/llvmkit-ir/tests/ap_int_upstream_ops.rs:82-87 and ap_float_from_string.rs:1-12 — the nearestLogBase2 drop and the llvmkit-specific-subset framing are verbatim as claimed. git log shows commit 1701b58 "feat(apint)!: finish the APIntTest sweep" is what closed the APInt half after the claim was recorded.

</details>

### 78. `TempDIAssignIDAttachments` RAUW machinery is absent

*metadata parser* — crates/llvmkit-asmparser/src/ll_parser.rs:5344-5350 (the only DIAssignID site)

- **LLVM:** `LLParser` collects instructions carrying a forward-referenced `!DIAssignID` attachment in `TempDIAssignIDAttachments` and RAUWs the temporary nodes when the real node is defined (`validateEndOfModule`).
- **llvmkit:** Only the `missing 'distinct', required for !DIAssignID()` rejection exists (landed W7 part 5); no DIAssignID-specific attachment RAUW appears in the parser — the generic metadata forward-reference map is all there is. Verified by grep: `DIAssignID` occurs in ll_parser.rs only around the distinct check.
- **Why:** Recorded as still open at W7 ("the `TempDIAssignIDAttachments` RAUW machinery … is metadata-layer work and belongs with W11"). W11's own completion notes do not record it landing, so the reason it is still open is **unrecorded** — treat the routing as a hypothesis, not a finding.
- **Fix:** First verify whether llvmkit's generic metadata forward-reference resolution already covers the attachment case; if not, add the pending-attachment map drained at end of module (law 8c: the map owns linear handles, draining consumes them).
- **Correction from verification:** Still present, with two corrections to the description. (1) Upstream's RAUW drain lives in `LLParser::parseStandaloneMetadata`, not `validateEndOfModule` — `TempDIAssignIDAttachments` has exactly three sites in LLVM 22.1.4 (LLParser.h declaration, push in `parseInstructionMetadata`, drain in `parseStandaloneMetadata`) and `validateEndOfModule` never names it. (2) The absence in llvmkit is structural, not an oversight: llvmkit resolves metadata forward references by reserve-then-fill on a stable `MetadataId` (`resolve_md_slot` -> `metadata_reserve`, `define_md_slot` -> `metadata_set`), so there are no temporary MDNodes and no `replaceAllUsesWith` anywhere in the parser. Upstream needs the DIAssignID special case only because `Instruction::setMetadata(MD_DIAssignID, ...)` maintains `LLVMContextImpl::AssignmentIDToInstrs` via `updateDIAssignIDMapping`, a side map that a temporary RAUW cannot fix (LLParser.h: "DIAssignID metadata does not support temporary RAUW so we cannot use the normal metadata forward reference resolution method"). llvmkit has no `AssignmentIDToInstrs` equivalent at all, so nothing observable is lost by the omission today; the gap becomes real only if llvmkit ports assignment tracking. Additional finding not in the claim: upstream pushes to `TempDIAssignIDAttachments[N]` unconditionally and drains only the forward-referenced entry, so a `!DIAssignID !N` attachment whose `!N` was already defined earlier in the file is silently dropped by upstream while llvmkit attaches it — a genuine behavioral divergence, but in the opposite direction from a missing feature.

<details><summary>Verification evidence</summary>

crates/llvmkit-asmparser/src/ll_parser.rs: lines 5344-5350 remain the only DIAssignID site (the `missing 'distinct', required for !DIAssignID()` guard in `parse_specialized_metadata_body`) — cited range still exact. The instruction-attachment loop is `skip_trailing_metadata` (lines 6089-6123, doc-commented "Mirrors the metadata-attachment loop in LLParser::parseInstructionMetadata"); it calls `inst.set_metadata(self.module, MetadataAttachmentKind::from_name(&name), id)` for every kind with no MD_DIAssignID branch and no deferral list. Forward refs: `resolve_md_slot` (line 1347) mints `module.metadata_reserve()`; `define_md_slot` (line 1363) later `metadata_set`s the same id — no temporary, no RAUW. crates/llvmkit-ir/src/instruction.rs:764-773: `set_metadata` is a plain `self.data().metadata.borrow_mut().insert(kind, id)`. Grep for `AssignmentIDToInstrs|assignment_id_to_instr|assignment_tracking` across crates/: no matches. Upstream: orig_cpp/.../llvm/include/llvm/AsmParser/LLParser.h:128 declares `DenseMap<MDNode*, SmallVector<Instruction*,2>> TempDIAssignIDAttachments` with the "does not support temporary RAUW" comment; LLParser.cpp:2383 pushes in `parseInstructionMetadata` (`if (MDK == LLVMContext::MD_DIAssignID) TempDIAssignIDAttachments[N].push_back(&Inst); else Inst.setMetadata(MDK, N);`); LLParser.cpp:1030-1038 drains inside `parseStandaloneMetadata` under `if (isa<DIAssignID>(Init))` before `ToReplace->replaceAllUsesWith(Init)`. lib/IR/Metadata.cpp:1757 shows why — `MD_DIAssignID` routes through `updateDIAssignIDMapping` into `getContext().pImpl->AssignmentIDToInstrs`, asserting `!Node->isTemporary()`. `LLParser::parseMDNodeID` returns the real node when the id is already in `NumberedMetadata`, which is what makes upstream's unconditional push drop already-defined-node attachments. No llvmkit fixture exercises a `!DIAssignID !N` instruction attachment (grep over crates/*/tests found only standalone `distinct !DIAssignID()` definitions in parser_debug_metadata.rs and the dbg-record-invalid fixtures).

</details>

### 79. No `ParserConfig`: `AllowIncompleteIR`, the DataLayout callback and the UpgradeDebugInfo flag are unmodelled

*parser — entry points* — crates/llvmkit-asmparser/src/parser.rs:209,:230,:252,:288

- **LLVM:** `parseAssembly*` takes a `DataLayoutCallbackTy` and an `UpgradeDebugInfo` flag, and `AllowIncompleteIR` (a `cl::opt`) enables auto-declaration of non-intrinsic undefineds, `dropUnknownMetadataReferences` and TBAA-drop tolerance.
- **llvmkit:** No `ParserConfig` / `DataLayoutCallback` / `allow_incomplete_ir` symbol exists anywhere in `crates/llvmkit-asmparser/src` (verified by grep); the entry points are `parse_assembly`, `parse_assembly_file`, `parse_assembly_with_index` (landed W10) and `parse_assembly_with_context`, all with fixed behaviour.
- **Why:** Recorded as W13 items with annex A7 giving the exact shape (`ParserConfig` with `*_with_config` twins, plain forms keeping today's defaults). Deferred because both options only become meaningful once the end-of-module machinery and AutoUpgrade exist.
- **Fix:** Add `ParserConfig` per annex A7 plus `*_with_config` twins, thread the DataLayout callback into the target-definitions phase (W3's restructure), and implement `AllowIncompleteIR`'s three tolerances faithfully, off by default.
- **Correction from verification:** Accurate as stated; one undercount worth fixing. The claim's "the entry points are parse_assembly, parse_assembly_file, parse_assembly_with_index and parse_assembly_with_context" lists only the four closure forms. crates/llvmkit-asmparser/src/parser.rs also exposes parse_into (:68), parse_branded (:99), parse_dynamic (:124), parse_file_branded (:139), parse_file_dynamic (:156), parse_summary_index_assembly (:267), parse_summary_index_assembly_file (:277), and the standalone parse_type* / parse_constant_value* family (:321-:365). None of them takes a config either, so the divergence is broader than the claim implies rather than narrower. Two additions worth recording: (1) llvmkit already acknowledges the DataLayout-callback half in a source comment at ll_parser.rs:1406-1411 ("We don't ship that callback path yet"), and llvmkit correspondingly does not model parseTargetDefinitions as a separate pre-pass — it dispatches `target` as an ordinary top-level entity; (2) the gap is undocumented — a repo-wide grep excluding orig_cpp finds no mention of ParserConfig / DataLayoutCallback / allow_incomplete_ir / UpgradeDebugInfo in any source file or any .md, including docs/future-work.md.

<details><summary>Verification evidence</summary>

llvmkit: crates/llvmkit-asmparser/src/parser.rs — cited lines are exact. :209 `pub fn parse_assembly<R, S, F>(src: S, f: F) -> ParseResult<R>`, :230 `parse_assembly_file(path, f)`, :252 `parse_assembly_with_index(src, f)`, :288 `parse_assembly_with_context(src, f)`. Each takes only source/path plus a closure and funnels into constructors that likewise carry no config: ll_parser.rs:1220 `pub fn new(src: &'src [u8], module: &'ctx Module<B, Unverified>) -> ParseResult<Self>`, :1262 `with_summary_index(src, module)`, :1274 `summary_index_only(src, module)`. A case-insensitive Grep for `ParserConfig|DataLayoutCallback|data_layout_callback|allow_incomplete_ir|AllowIncompleteIR|upgrade_debug_info|UpgradeDebugInfo` over C:/Users/olegg/Desktop/llvmkit with orig_cpp excluded returned zero matches (and a separate run over crates/ alone also returned zero). ll_parser.rs:1406-1411 states in-tree: "Upstream splits `parseTargetDefinitions` from `parseTopLevelEntities` because LLVM 22 wants a chance to apply a default DataLayout *before* anything that depends on it. We don't ship that callback path yet". Upstream (orig_cpp/llvm-project-llvmorg-22.1.4/llvm/): include/llvm/AsmParser/Parser.h:36 typedefs `DataLayoutCallbackTy`, defaulted into parseAssembly/parseAssemblyFile/parseAssemblyString (:92,:133,:174); :97 declares `parseAssemblyFileWithIndexNoUpgradeDebugInfo(...)`, whose only purpose is passing UpgradeDebugInfo=false (lib/AsmParser/Parser.cpp:133-137), and which is called by tools/llvm-as/llvm-as.cpp:125 and tools/opt/optdriver.cpp:554-555 under `-disable-upgrade-debug-info`. lib/AsmParser/LLParser.cpp:75-91 `LLParser::Run(bool UpgradeDebugInfo, DataLayoutCallbackTy DataLayoutCallback)` calls parseTargetDefinitions(DataLayoutCallback) then validateEndOfModule(UpgradeDebugInfo). LLParser.cpp:61-65 declares `static cl::opt<bool> AllowIncompleteIR("allow-incomplete-ir", cl::init(false), cl::Hidden, ...)`, consumed at exactly the three sites the claim names, inside validateEndOfModule: :373 `if (!AllowIncompleteIR) continue;` guarding the block at :376-403 that synthesizes declarations for non-intrinsic forward refs (a Function at the common call type via GetCommonFunctionType, else an i8 GlobalVariable); :416-417 `if (AllowIncompleteIR && !ForwardRefMDNodes.empty()) dropUnknownMetadataReferences();` (that method at :175-198 erases temporary MDNodes off functions, instructions and globals); and :430-439 the InstsWithTBAATag loop where `if (!AllowIncompleteIR) assert(MD && "UpgradeInstWithTBAATag should have a TBAA tag");` relaxes to tolerate a TBAA tag dropped by the incomplete-IR path. :447-448 `if (UpgradeDebugInfo) llvm::UpgradeDebugInfo(*M);`.

</details>

### 80. `attr_kind_for_keyword` is a hand-written table, not generated from `Attributes.td`

*parser — attributes* — crates/llvmkit-asmparser/src/ll_parser.rs (`attr_kind_for_keyword`), crates/llvmkit-ir/src/attributes.rs, crates/llvmkit-asmparser/tests/attribute_td_drift.rs, crates/llvmkit-asmparser/tablegen/llvm-22.1.4/Attributes.td

- **LLVM:** `LLParser::tokenToAttribute` is generated from `llvm/IR/Attributes.td`, so a new attribute cannot be silently missing.
- **llvmkit:** The keyword→`AttrKind` mapping is a hand list. `attribute_td_drift.rs` stands in for generation and has caught five separate holes, but the table itself is still transcribed.
- **Why:** Recorded as carried out of W5: "Given how much that guard has caught, generating the table would be strictly better — carry it." Also note the guard's own reader had four bugs; a measurement taken with a broken reader looks authoritative and is not.
- **Fix:** Extend `llvmkit-tablegen` to emit the keyword→kind table from the vendored `Attributes.td` and keep the drift test as the cross-check (five-step method, steps 1 and 5).

<details><summary>Verification evidence</summary>

Claim #85 is REAL and STILL PRESENT; the description is accurate on every material point. llvmkit (hand-written, confirmed today): - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:9172 — `fn attr_kind_for_keyword(keyword: Keyword) -> Option<AttrKind>` is a literal transcribed `match` of ~90 `Keyword::X => AttrKind::Y` arms terminating in `_ => return None`. Lines 9216-9220 even carry a comment describing the arms added later as "upstream's `tokenToAttribute`" that "was missing them, so each parsed as 'not an attribute'". - No `build.rs` exists in crates/llvmkit-asmparser (dir listing: Cargo.toml, LICENSE, README.md, examples, src, tablegen, tests). Nothing generates this table. - The only TableGen generation in the workspace is llvmkit-ir/build.rs -> llvmkit-tablegen, whose sole root input is `const ROOT_TD: &str = "llvm/IR/Intrinsics.td"` (crates/llvmkit-tablegen/src/lib.rs:64). Intrinsics only; `Attributes.td` is never fed to the generator. - `AttrKind` is itself a hand-written enum at crates/llvmkit-ir/src/attributes.rs:285. Upstream (generated, confirmed in the vendored tree): - orig_cpp/.../llvm/lib/AsmParser/LLParser.cpp:1545 `static Attribute::AttrKind tokenToAttribute(lltok::Kind Kind)` whose switch body is `#define GET_ATTR_NAMES` / `#define ATTRIBUTE_ENUM(ENUM_NAME, DISPLAY_NAME) case lltok::kw_##DISPLAY_NAME: return Attribute::ENUM_NAME;` / `#include "llvm/IR/Attributes.inc"`, with `default: return Attribute::None`. Called from LLParser.cpp:1736 and :2026. The drift guard exists exactly as described: - crates/llvmkit-asmparser/tests/attribute_td_drift.rs `include_str!`s ../tablegen/llvm-22.1.4/include/llvm/IR/Attributes.td (present, 20191 bytes, tracked). `NOT_YET_MODELED` is `&[]`. Its module header states generation was deliberately rejected ("llvmkit deliberately models a subset ... would mean generating part of the 700-variant `Keyword` enum") and that the test "gives the same guarantee without that cost". - The "five holes" figure is consistent with the file's own doc comments, which record: Milestone 0's ~21 missing keywords; multi-line `def`s misread (dereferenceable_or_null, speculative_load_hardening, nocreateundeforpoison); the `split_once(':')` spacing bug dropping hot / disable_sanitizer_instrumentation / allockind; anonymous `def : CompatRuleStrAttr` defs inventing an attribute named `isEqual`; and positionless defs passing the probe vacuously (now guarded by `every_attribute_declares_a_position`). Scope note (the claim is if anything understated, not wrong): the divergence covers the lexer as well as the parser. Upstream's LLLexer.cpp:701 emits its attribute keywords from the same `Attributes.inc` via `KEYWORD(DISPLAY_NAME)`, whereas llvmkit's crates/llvmkit-asmparser/src/ll_lexer/keywords.rs is a hand-written byte-string match (e.g. line 334 `b"nocreateundeforpoison" => kw(Nocreateundeforpoison)`, line 395 `b"zeroext" => kw(Zeroext)`). So both generated halves upstream are hand-transcribed here. Working-tree caveat: ll_parser.rs is modified vs HEAD (173+/165- per `git diff --stat`), but that diff is line-ending/reflow churn in this region; the hand-written table is present in the working tree as read.

</details>

### 81. `inrange` bounds have a second, parallel APSInt reader

*parser — constants* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_inrange_bound` and helpers)

- **LLVM:** `LLLexer` has one APSInt rule — the `[us]0x` active-bit truncation and the signed widening — and every consumer widens from it.
- **llvmkit:** `Parser::parse_inrange_bound` and its six helpers (`ParsedInRangeBound`, `inrange_bound_to_apint_words`, `signed_magnitude_to_apint_words`, `apsint_to_apint_words`, `hex_apsint_bit_width`, `decimal_digits_to_words`, `hex_digits_to_words`) implement the same rule that `parse_int_literal` implements since W5. Both are currently correct.
- **Why:** Recorded in docs/future-work.md (W5): two implementations of one lexer rule is the exact shape this program keeps finding bugs in (three private copies of the scalable-vector walk, none matching `Type::isScalableTy`). Not done in W5 because it sits on the GEP constant-expression path — routed to W9a, which did not take it.
- **Fix:** Collapse `parse_inrange_bound` onto `parse_int_literal` + `ParsedApsInt::extend_or_truncate`; the only real work is that `ConstantExprInRange::new` takes `Box<[u64]>` rather than an `ApInt`. Keep `parser_constants.rs::constant_expr_gep_inrange_signed_hex_active_bits_are_preserved` green.
- **Correction from verification:** The claim is accurate in substance but understates the scope and misplaces the root cause. Two refinements: (1) the parallel machinery is larger than "six helpers" — `sign_extend_apint_words`, `negate_apint_words`, `mask_apint_top_word`, `apint_active_bits`, `mul_add_words`, `low_u64`, plus `signed_apint_cmp`/`unsigned_apint_cmp`/`apint_sign_bit` (re-implementing APSInt `sge` for the emptiness check) exist only for this path, ~12 exclusive items total; (2) the divergence originates in llvmkit-ir, not the parser: `ConstantExprInRange` (crates/llvmkit-ir/src/constant.rs:132) stores `start: Box<[u64]>, end: Box<[u64]>, bit_width: u32` where upstream's `ConstantRange` holds two `APInt`s, so the parser cannot hand it a `ParsedApsInt`/`ApInt` and is forced to build words. Collapsing `parse_inrange_bound` alone would not remove the duplication — `ConstantExprInRange` must hold `ApInt` first. Also, upstream's lexer sets `APSIntVal` at two sites (LLLexer.cpp lexIdentifier's `[us]0x` path and the decimal `APSInt(StringRef)` path), not literally one rule, but both feed the single `APSIntVal` every consumer reads via `getAPSIntVal()`, so the claim's "one APSInt rule" characterization holds. The "both are currently correct" assessment is confirmed.

<details><summary>Verification evidence</summary>

crates/llvmkit-asmparser/src/ll_parser.rs: `parse_inrange_bound` (line 8702) matches `Token::IntegerLit` itself and calls `decimal_digits_to_words` (1061) / `hex_digits_to_words` (1075) / `hex_apsint_bit_width` (1092) — it never calls `parse_int_literal` (2277) or `ParsedApsInt::extend_or_truncate` (637). `hex_apsint_bit_width` restates the same `active_bits > 0 && active_bits < syntactic_bits` truncation that `parse_int_literal`'s HexSigned/HexUnsigned arm already implements against `ApInt`. `gep_constant_expr_flags` (8677) then calls `inrange_bound_to_apint_words` (929) -> `signed_magnitude_to_apint_words` (943) / `apsint_to_apint_words` (959), which redo the sign/zero-extend-or-truncate that `extend_or_truncate` performs via `sext_or_trunc`/`zext_or_trunc`. Grep confirms `mul_add_words`, `negate_apint_words`, `sign_extend_apint_words`, `apint_active_bits`, `mask_apint_top_word`, `low_u64` have no caller outside this cluster. Upstream orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp lines 4485-4531 (LLParser::parseValID, getelementptr arm): reads `InRangeStart = Lex.getAPSIntVal()` / `InRangeEnd = Lex.getAPSIntVal()`, then `InRangeStart = InRangeStart.extOrTrunc(IndexWidth); InRangeEnd = InRangeEnd.extOrTrunc(IndexWidth); if (InRangeStart.sge(InRangeEnd)) return error(...)` — no second reader. LLLexer.cpp:1062 `APSIntVal = APSInt(Tmp, TokStart[0] == 'u')` after `if (activeBits > 0 && activeBits < bits) Tmp = Tmp.trunc(activeBits)`, and LLLexer.cpp:1207 `APSIntVal = APSInt(StringRef(...))`. crates/llvmkit-ir/src/constant.rs:132-150 shows `ConstantExprInRange` holding `Box<[u64]>` rather than `ApInt`. Correctness of both readers is pinned by crates/llvmkit-asmparser/tests/fixtures/upstream/LLParser-parseValID/constant_expr_gep_inrange_hex_apsint.ll, ..._apint_trunc.ll, and ..._signed_hex_active_bits_invalid.ll (the last proving `s0x1` reads as -1).

</details>

### 82. Three copies of the aggregate index walk

*IR model* — crates/llvmkit-ir/src/instructions.rs (`indexed_aggregate_type`), crates/llvmkit-ir/src/ir_builder.rs (`walk_aggregate_for_builder`), crates/llvmkit-ir/src/verifier.rs (`walk_aggregate_path`)

- **LLVM:** `ExtractValueInst::getIndexedType` is one routine, used by the parser, the builder and the verifier alike.
- **llvmkit:** Three implementations: the public port `llvmkit_ir::indexed_aggregate_type` (added W9c for `invalid indices for {extract,insert}value`), `ir_builder.rs::walk_aggregate_for_builder`, and `verifier.rs::walk_aggregate_path` (which additionally distinguishes *why* the walk failed via `AggWalkErr`). All three currently agree.
- **Why:** Recorded in docs/future-work.md (W9c): not urgent, but "a predicate with three implementations is one diagnostic away from having three behaviours". Consolidation is not a pure deletion because the verifier needs a richer return type than `Option`.
- **Fix:** Give the public port a result type that carries the failure reason, then delete the two private near-copies and re-point their callers.
- **Correction from verification:** The divergence is real and still present as described, with one refinement: the closing sub-claim "All three currently agree" has a reachable counterexample. `verifier.rs::walk_aggregate_path` narrows the array length with `u32::try_from(*n).unwrap_or(u32::MAX)` before comparing, while `indexed_aggregate_type` and `walk_aggregate_for_builder` both widen the index to `u64` and compare against the true `u64` length. For an array longer than `u32::MAX` indexed at exactly `u32::MAX` — e.g. `extractvalue [4294967296 x i8] %a, 4294967295`, which the `.ll` grammar can spell since index-list entries are uint32 — the public port and the builder accept and the verifier reports `OutOfRange` with a wrong `count` (u32::MAX). So the parser/builder path constructs the instruction and the verifier then rejects it. The three walks agree on every practically-sized aggregate, but not by construction, which is exactly the failure mode the future-work entry predicts ("one diagnostic away from having three behaviours").

<details><summary>Verification evidence</summary>

Upstream, C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Instructions.cpp — `ExtractValueInst::getIndexedType(Type *Agg, ArrayRef<unsigned> Idxs)` is a single ~20-line loop (array: `Index >= AT->getNumElements()` -> nullptr; struct: `Index >= ST->getNumElements()` -> nullptr; anything else -> nullptr). Its callers confirm the "one routine, three consumers" half of the claim: the parser calls it in `LLParser::parseExtractValue` / `parseInsertValue` (LLParser.cpp), the builder path calls it from the `ExtractValueInst` constructor via `checkGEPType(getIndexedType(...))` in include/llvm/IR/Instructions.h plus the `InsertValueInst::init` assert in Instructions.cpp, and the verifier calls it in `Verifier::visitExtractValueInst` / `visitInsertValueInst` (Verifier.cpp). llvmkit, all three copies read in the working tree (none touched by the uncommitted diff — `git diff -U0` on instructions.rs/ir_builder.rs shows no hunk mentioning `aggregate` or `indexed`): - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/instructions.rs:4011 `pub fn indexed_aggregate_type` — walks `AnyTypeEnum`, array arm `u64::from(index) >= array.len()`, struct arm `structure.field_type(...)` (derived_types.rs:1113, which returns `None` for an opaque body). Its own rustdoc at instructions.rs:4008 admits "Two private near-copies of this walk exist, in `ir_builder.rs` and `verifier.rs`". Only callers are the parser: ll_parser.rs:12655 and :12722, producing "invalid indices for extractvalue"/"...insertvalue". - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/ir_builder.rs:10528 `fn walk_aggregate_for_builder` — same walk over `TypeData`, returns `IrError::AggregateIndexOutOfRange` / `IrError::TypeMismatch`; called at ir_builder.rs:3423 (`extract_value_dyn`), :3490 (`insert_value_dyn`), :3520. - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/verifier.rs:3525 `fn walk_aggregate_path` with `enum AggWalkErr { NotAggregate, OutOfRange }` at :3520; called at verifier.rs:1537 (`check_extract_value`) and :1577 (`check_insert_value`). Not a fourth copy: `constant_fold.rs::constant_fold_extract_value_instruction` / `..._insert_value_instruction` walk constant *elements*, mirroring upstream's `ConstantFoldExtractValueInstruction`, not the type walk. The ledger entry is still open and unedited at C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:76-94, including the "Not urgent: all three currently agree" line that my refinement above corrects.

</details>

### 83. Private `Type`-predicate duplicates were never consolidated onto the public ports

*IR model* — crates/llvmkit-asmparser/src/ll_parser.rs; crates/llvmkit-ir/src/{constants.rs, intrinsics.rs, ir_builder/constant_folder.rs, assumptions.rs, implied_conditions.rs}

- **LLVM:** `Type::isSized`, `Type::isScalableTy`, `CastInst::castIsValid` and the `isValid*Type` family are each one routine in `llvm/IR/Type.cpp` / `Instructions.cpp`.
- **llvmkit:** W4 (`a8b619c`) landed seven public `Type` predicates plus `cast_is_valid`, but private near-copies remain: `is_int_or_int_vector_type` and `is_ptr_or_ptr_vector_type` local in `ll_parser.rs`, plus duplicates recorded in `constants.rs`, `intrinsics.rs`, `ir_builder/constant_folder.rs`, `assumptions.rs`, `implied_conditions.rs`.
- **Why:** Recorded under "follow-ups deliberately not folded into parity commits". W4.5 had to fix the three `type_contains_scalable_vector` copies because they were all wrong (missing the target-extension arm, the cycle guard, or recursing where upstream does not) — the remaining copies were left because they were not load-bearing for a diagnostic at the time.
- **Fix:** Delete each private copy in favour of the public predicate, checking each against its upstream counterpart first — the W4 lesson is that a predicate llvmkit already has may be answering a different question.
- **Correction from verification:** Still present, but the upstream framing is wrong and the file list needs two corrections. The four upstream routines the claim names as the point of comparison are precisely the ones llvmkit did NOT duplicate. Each has exactly one definition in the tree and no private near-copies: Type::isSized -> Type::is_sized (type.rs:604); Type::isScalableTy -> Type::is_scalable (type.rs:615, with derived_types.rs:1006 being VectorType::isScalable, a genuinely different predicate that type.rs:608-614 explicitly distinguishes); CastInst::castIsValid -> cast_is_valid (instructions.rs:3883, one definition, one external caller at ll_parser.rs:8589); the isValid*Type family -> Type::is_valid_{struct,array,vector,pointer}_element (type.rs:712/725/732/747, all called from ll_parser.rs). W4's type_contains_scalable_vector consolidation genuinely landed - that identifier no longer appears anywhere in crates/. Corrected statement: the surviving duplication is entirely in the isIntOrIntVectorTy / isPtrOrPtrVectorTy / isFPOrFPVectorTy / getScalarType family, which upstream defines INLINE IN Type.h (lines 246, 250, 270), not in Type.cpp. W4 landed public ports as Type::is_int_or_int_vector (type.rs:686), Type::is_ptr_or_ptr_vector (type.rs:691), Type::is_vector (type.rs:535) and Type::scalar_type (type.rs:633), and then consolidated nothing onto them. Twelve private near-copies remain: ll_parser.rs:818 (is_int_or_int_vector_type), :870 (is_ptr_or_ptr_vector_type), :828 (is_fp_or_fp_vector_type, no public counterpart exists); constants.rs:2512, :2391, :2362; intrinsics.rs:2033 (is_integer_or_integer_vector), :2040 (is_float_or_float_vector), :2056 (is_vector), :2066 (scalar_type_data); ir_builder/constant_folder.rs:679; implied_conditions.rs:1200 (is_vector). Two refinements to the claim's cited file list. (1) assumptions.rs:1168 and implied_conditions.rs:1206 are the weakest entries: is_int_or_int_vector_of_width_one ports Type::isIntOrIntVectorTy(1), the bit-width overload, which llvmkit has no public port of at all - so those two duplicate each other, not an existing public port. (2) The list omits two further slot-level copies of Type::scalar_type: value_tracking.rs:1164 and verifier.rs:3488, both named scalar_type_id. Severity nuance the claim does not draw: constant_folder.rs:679 takes a Type<'_, B> view - the exact receiver type of the public method - so it is a pure shadow with no layering excuse, as are the parser's two (ll_parser.rs:6453 already calls ty.is_valid_pointer_element(), proving public Type methods are reachable from that file). Only constants.rs's copies have a real defense: they take &ModuleCore + TypeSlot, a layer below where a Type view is constructible, so consolidating them would not be a pure deletion.

<details><summary>Verification evidence</summary>

Decisive single fact: grep for callers of the public predicates `is_int_or_int_vector()` / `is_ptr_or_ptr_vector()` across crates/ returns exactly one file - crates/llvmkit-ir/src/instructions.rs (i.e. cast_is_valid itself). Every other consumer in the workspace calls a local copy. Files read: - crates/llvmkit-ir/src/type.rs:580-760 - the public ports: is_sized (604), is_scalable (615), scalar_type (633), is_int_or_int_vector (686, `self.scalar_type().is_integer()`), is_ptr_or_ptr_vector (691), is_valid_struct/array/vector/pointer_element (712/725/732/747), is_vector (535). Free slot-level helpers is_sized/is_scalable at 934/1011 are private to type.rs. - crates/llvmkit-asmparser/src/ll_parser.rs:818-883 - private `is_int_or_int_vector_type` (matches on AnyTypeEnum::Int / Vector-with-integer-element), `is_fp_or_fp_vector_type`, `is_ptr_or_ptr_vector_type`; called at :8566, :8764, :11370, :11535, :12160. Same file calls the public `ty.is_valid_pointer_element()` at :6453/:6921/:11817 and `llvmkit_ir::cast_is_valid` at :8589, so the public Type surface is demonstrably reachable there. - crates/llvmkit-ir/src/constants.rs:2362 (scalar_type_id), :2391 (is_ptr_or_ptr_vector), :2512 (is_int_or_int_vector) - all `(&ModuleCore, TypeSlot)`-shaped; ~10 call sites at :1903, :2090-2133, :2242, :2300-2305. - crates/llvmkit-ir/src/intrinsics.rs:2033-2069 - is_integer_or_integer_vector, is_float_or_float_vector, is_vector, scalar_type_data. - crates/llvmkit-ir/src/ir_builder/constant_folder.rs:679-690 - `fn is_int_or_int_vector<B: ModuleBrand>(ty: Type<'_, B>) -> bool`, same receiver type as the public method, called at :624. - crates/llvmkit-ir/src/assumptions.rs:1166-1175 and implied_conditions.rs:1199-1217 - both carry the rustdoc "Ports `Type::isIntOrIntVectorTy(1)`"; implied_conditions.rs:1200 also has a bare `is_vector`. - grep "type_contains_scalable_vector" across crates/ - zero hits, confirming that W4 consolidation did land. Upstream confirmed: - orig_cpp/.../llvm/include/llvm/IR/Type.h:246 `isIntOrIntVectorTy()`, :250 the `(unsigned BitWidth)` overload, :270 `isPtrOrPtrVectorTy()` - all inline in the header, not Type.cpp. - orig_cpp/.../llvm/lib/IR/Type.cpp:61/69 Type::isScalableTy, :263 isSizedDerivedType, :703/765/789/875 the four isValidElementType. - orig_cpp/.../llvm/lib/IR/Instructions.cpp:3312 CastInst::castIsValid. Context: docs/future-work.md:76-94 records the identical shape for the aggregate index walk ("one port, three callers"; "a predicate with three implementations is one diagnostic away from having three behaviours") and cites W4's type_contains_scalable_vector as the precedent - but there is no future-work entry covering the Type int/ptr/fp/scalar predicate family.

</details>

### 84. `ConstantRangeList` set operations are not ported

*IR model* — crates/llvmkit-ir/src/constant_range_list.rs

- **LLVM:** `llvm/IR/ConstantRangeList.h` provides `subtract`, `unionWith` and `intersectWith` alongside `insert`.
- **llvmkit:** Only `isOrderedRanges`, `getConstantRangeList`, `print` and `insert` (with its `int64_t` overload) are ported. Three of the six `unittests/IR/ConstantRangeListTest.cpp` cases (`Subtract`, `Union`, `Intersect`) are therefore unportable.
- **Why:** Recorded in docs/future-work.md (W5, decided 2026-08-12): no consumer anywhere in llvmkit — upstream's callers are Attributor- and `MemoryLocation`-style passes this tree has not ported — so porting would add public API with no in-tree user and no way to be sure it stays right.
- **Fix:** Land the three methods together with their first real caller, taking the three unit tests in the same commit. One detail to carry: `insert`'s no-op check uses `ConstantRange::contains` (unsigned) while everything else in the class compares signed; llvmkit reproduces the inconsistency, so read a `subtract` port against upstream's comment, not against `insert`.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/constant_range_list.rs (343 lines, read in full, unmodified in the working tree): the sole `impl ConstantRangeList` block has `new` (= getConstantRangeList), `is_ordered_ranges`, `ranges`, `is_empty`, `len`, `bit_width`, `insert`, `insert_signed`, plus `impl Display` for `print`. No `subtract`, `union_with`, or `intersect_with`. A crate-wide grep for `ConstantRangeList` across crates/llvmkit-ir/src shows the type only appears additionally in attributes.rs (the `Initializes` payload) and lib.rs:219 (re-export) — no set operations anywhere else. Upstream orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/IR/ConstantRangeList.h declares `subtract` (line 77), `unionWith` (81) and `intersectWith` (85), all three defined in lib/IR/ConstantRangeList.cpp (lines 86, 146, 197). unittests/IR/ConstantRangeListTest.cpp has exactly six TEST_F cases — Basics, getConstantRangeList, Insert, Subtract, Union, Intersect — and llvmkit's in-file `mod tests` ports only the first three (UPSTREAM.md rows 1130-1133 corroborate; the fourth test, display_matches_print, is self-declared llvmkit-specific). docs/future-work.md line 258 records this as a deliberate deferral ("ConstantRangeList - three set operations not ported (decided 2026-08-12, LLParser parity W5)"), reasoning that the three methods have no in-tree caller and instructing that they land with their first real caller together with the three tests. Sole nuance: the claim's enumeration understates the ported surface slightly — `rangesRef`/`empty`/`size`/`getBitWidth`/`operator==` are also ported (as ranges/is_empty/len/bit_width/PartialEq) — but what is missing is exactly the three set operations, as claimed.

</details>

### 86. `AllocationType` models the four keyword values, not upstream's OR-able set

*IR model — summary index* — crates/llvmkit-ir/src/module_summary_index.rs

- **LLVM:** `enum class AllocationType : uint8_t` has `None = 0`, `NotCold = 1`, `Cold = 2`, `Hot = 4`, `All = 7`, powers of two precisely so a context reaching an allocation more than one way can OR them; `AllocInfo::Versions` is a `SmallVector<uint8_t>` for that reason.
- **llvmkit:** The enum models exactly the four keywords `LLParser::parseAllocType` reads.
- **Why:** Recorded in docs/future-work.md (W10) as deliberate: `AssemblyWriter::printFunctionSummary`'s `AllocTypeName` lambda handles those four and `llvm_unreachable`s on anything else, so an ORed value can only come from bitcode, which llvmkit does not have. "The enum is the `.ll` surface, exactly."
- **Fix:** Nothing until llvmkit gains bitcode; revisit with the bitcode reader, at which point `AllocationType` becomes a masked newtype rather than an enum.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/module_summary_index.rs:501-571 — `pub enum AllocationType` has exactly four variants (None/NotCold/Cold/Hot), no `All`, no bitflags, and only a one-way `raw()` returning 0/1/2/4 plus `keyword()`; `MemoryInfoBlock::allocation_type: AllocationType` and `AllocationInfo::versions: Vec<AllocationType>` both use it, so an OR-ed value (e.g. NotCold|Cold == 3) is unrepresentable. Upstream orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/IR/ModuleSummaryIndex.h:397-403 confirms `enum class AllocationType : uint8_t { None=0, NotCold=1, Cold=2, Hot=4, All=7 }` with "This should always be set to the OR of all values", and :427-436 confirms `SmallVector<uint8_t> Versions` with the comment "Before cloning, index 0 may have more than one allocation type"; lib/Transforms/IPO/MemProfContextDisambiguation.cpp really ORs (`AllocTypes |= …` at :1234/:1465/:1494 and `AN.Versions[0] = (uint8_t)allocTypeToUse(AllocNode->AllocTypes)` at :2538), as does include/llvm/Analysis/MemoryProfileInfo.h:95. Scope qualifiers, also read: the gap cannot surface through the .ll text path — LLParser::parseAllocType (lib/AsmParser/LLParser.cpp) accepts only the four keywords and AsmWriter::printFunctionSummary's AllocTypeName lambda (lib/IR/AsmWriter.cpp) llvm_unreachable()s on anything but 0/1/2/4; llvmkit's crates/llvmkit-asmparser/src/ll_parser.rs:4648-4663 `parse_alloc_type` matches upstream arm-for-arm. Also, MIBInfo::AllocType is single-valued in practice (getMIBAllocType in lib/Analysis/MemoryProfileInfo.cpp:94 returns exactly one of Cold/Hot/NotCold), so AllocInfo::Versions is the field the divergence actually costs. Noted inconsistency: llvmkit's doc comment at module_summary_index.rs:503-504 states the power-of-two OR rationale while the type offers no OR and cannot hold the result.

</details>

### 87. No token-set drift test outside attributes, calling conventions and summary keywords — **FIXED (W14b)**

*lexer* — crates/llvmkit-asmparser/src/ll_lexer/keywords.rs, crates/llvmkit-asmparser/src/ll_token.rs, crates/llvmkit-asmparser/tests/

- **Closed, 2026-08-16 (W14b).** `crates/llvmkit-asmparser/tests/lexer_token_drift.rs`
  is the fifth drift test. `LLLexer.cpp` and `LLToken.h` are now vendored under
  `crates/llvmkit-asmparser/tablegen/llvm-22.1.4/`, and the test reads their
  macro tables directly: `KEYWORD` (plus the `Attributes.td` names
  `Attributes.inc` splices into it), `TYPEKEYWORD`, `INSTKEYWORD`, the nine
  `DWKEYWORD` families, `DBGRECORDTYPEKEYWORD`, the three prefix tails
  (`DIFlag`, `DISPFlag`, `CSK_`), the three exact-word tails (emission kind,
  name-table kind, fixed-point kind) and the thirteen payload-free punctuation
  arms of `LLLexer::LexToken` — each probed through the public
  `Lexer`, so a spelling routed to the *wrong* family fails too. Both set
  directions are closed as well, the backward one against an explicit
  `NON_UPSTREAM_KEYWORDS` list, and `every_lltok_kind_has_a_llvmkit_token`
  pins the `lltok::Kind` enum itself.
- **What the missing guard was hiding.** The correction bullet below concluded
  "every llvmkit-only word is an Attributes.td-generated attribute keyword".
  That is wrong, and it is wrong because the diff it describes never expanded
  `Attributes.inc`: it took the 510 llvmkit spellings, subtracted the 409 it
  read straight out of `LLLexer.cpp`, and called the remainder attributes
  without checking. Two corrections to those numbers. The 409 counts
  `DISPLAY_NAME`, the `ATTRIBUTE_ENUM` macro parameter — the bullet says so
  itself and then leaves it in the total — so the explicit tables hold 408
  spellings (329 `KEYWORD` + 13 `TYPEKEYWORD` + 66 `INSTKEYWORD`). That makes
  the unchecked remainder 102, not 101, and `Attributes.td` declares exactly
  101 enum-valued attributes. The odd one out is `exnref`, a `TYPEKEYWORD`
  LLVM 22.1.4 does not have — now divergence 103. Upstream's total is 509
  spellings against llvmkit's 510, and the new drift test asserts all five
  numbers separately so none can be arrived at by subtraction again.
- **Two further inventions the spelling diff could not see**, both closed by
  the same change: `Token::SpecializedMetadata`, a token with no `lltok`
  counterpart carrying a hand-written list of eighteen of the thirty-two `DI*`
  node names; and the missing `lltok::LabelID` (see entry 101).
- **LLVM:** `LLLexer.cpp`'s keyword and token tables define the accepted vocabulary; a keyword llvmkit does not know is silently mis-parsed rather than reported.
- **Note, 2026-08-16 (W14a):** the correction bullet below is still right that a missing keyword is *reported*, not silently mis-parsed — but its mechanism has changed. `lex_identifier`'s fallthrough is now `Ok(Token::Error)` with the same `TokStart+1` rewind, so the rejection comes from whichever parser production was running (`expected top-level entity`, `unterminated attribute group`, …) rather than from `unknown keyword '…'`. The failure modes drift would produce are unchanged.
- **llvmkit:** `attribute_td_drift.rs` and `calling_conv_drift.rs` cross-check two families and the summary keywords were verified by inventory; the instruction keywords and the misc `kw_` families have never been mechanically diffed against upstream.
- **Why:** Recorded as a W14 item. The program's own W6 lesson is the reason it matters: "a table nothing cross-checks is wrong" — 28 calling conventions parsed as `ccc` and printed back wrong, invisibly, because every test used a convention that already worked.
- **Fix:** Mechanically diff `LLLexer.cpp`'s tables against `keywords.rs` + `ll_token.rs` for every remaining family, and add a drift test wherever a vendorable source exists (note `orig_cpp/` is gitignored — anything a test reads must be vendored under `crates/llvmkit-asmparser/tablegen/llvm-22.1.4/`).
- **Correction from verification:** Core gap is REAL and still present: nothing in the tree mechanically diffs the lexer's instruction (`INSTKEYWORD`) or misc `KEYWORD` families against upstream. Three details in the claim are wrong or undercounted, though: (1) "two families" undercounts the existing guards. There are FOUR drift tests in `crates/llvmkit-asmparser/tests/`: `attribute_td_drift.rs` (Attributes.td), `calling_conv_drift.rs` (CallingConv.h, plus a parser-vs-printer round-trip half), `dwarf_def_drift.rs` (Dwarf.def + DebugInfoFlags.def), and `fixed_metadata_kinds_drift.rs` (FixedMetadataKinds.def). The last two do cover vocabulary the lexer's prefixed-word path (`DW_*`, `DIFlag*`) feeds, so the uncovered remainder is narrower than "everything but attributes/CCs/summary": what is unguarded is the `KEYWORD`/`INSTKEYWORD`/`TYPEKEYWORD` set and the hand-written exact-word families (EmissionKind, NameTableKind, FixedPointKind, CSK_*, dbg_*). (2) The consequence sentence is wrong. A keyword llvmkit does not know is NOT silently mis-parsed — it is reported. `Lexer::lex_identifier` (`crates/llvmkit-asmparser/src/ll_lexer.rs`) falls through to `Err(LexError::UnknownToken { reason: UnknownTokenReason::UnknownKeyword { word } })` after rewinding `self.pos = self.tok_start + 1`, exactly mirroring `LLLexer::LexIdentifier`'s tail (`CurPtr = TokStart+1; return lltok::Error;`). So a *missing* keyword makes llvmkit loudly reject IR LLVM accepts. The genuinely silent failure modes drift would produce are the other two directions: a spelling mapped to the wrong `Token` variant, or a keyword llvmkit still knows after upstream removed it (llvmkit accepts what LLVM rejects). (3) The claim implies unknown risk; the spelling set in fact agrees today. I ran the diff the missing test would run: 409 distinct spellings extracted from LLLexer.cpp's `KEYWORD(...)`/`INSTKEYWORD(...)`/`TYPEKEYWORD("...")` macro invocations vs. 510 `b"..."` literals in the production half of `keywords.rs`. Every upstream spelling is present (the only apparent miss, `DISPLAY_NAME`, is the `ATTRIBUTE_ENUM(ENUM_NAME, DISPLAY_NAME)` macro parameter at LLLexer.cpp's `#include` of Attributes.inc, not a keyword), and every llvmkit-only word is an Attributes.td-generated attribute keyword already covered by `attribute_td_drift.rs`. So this is a missing *guard against future drift*, not a live behavioral divergence in the accepted vocabulary — the diff checks spellings only, not token identity or payload. Structural cause worth recording alongside the finding: `crates/llvmkit-asmparser/tablegen/` vendors only Dwarf.def, Attributes.td, CallingConv.h, DebugInfoFlags.def and FixedMetadataKinds.def — no LLLexer.cpp — and `orig_cpp/` is gitignored (`.gitignore:2`), so a test reading it passes locally and fails CI. That is the same reason already recorded for the DI-field tables in `parser_debug_metadata.rs` and `docs/future-work.md`, but no equivalent note exists for the keyword tables.

<details><summary>Verification evidence</summary>

Files read: `C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_lexer/keywords.rs` — `classify_word` is a hand-written 510-arm byte-slice match; its header carries `TODO(tablegen): the attribute keyword list is hand-mirrored from ... Attributes.td`, and its `#[cfg(test)]` module (line 600+) spot-checks ~12 words. `crates/llvmkit-asmparser/src/ll_lexer.rs:940-990` — the unknown-word tail returning `LexError::UnknownToken`/`UnknownKeyword`, and `:95-115` documenting it against `LLLexer`'s bare `lltok::Error`. `crates/llvmkit-asmparser/src/ll_lexer_tests.rs:551-600` — mod `keywords_cat`, the only lexer keyword tests: hand-picked samples (`define declare global constant`; `add load store call ret br switch alloca`; nine flags/attrs), each citing KEYWORD/INSTKEYWORD by symbol but enumerating nothing. `crates/llvmkit-asmparser/src/ll_token.rs:462-467` — the `Keyword` enum doc; no `ALL` const, no spelling accessor, so no exhaustive in-tree enumeration exists either. Test-tree inventory: only four `*_drift.rs` files exist; `find crates/llvmkit-asmparser/tablegen -type f` returns five vendored inputs, none of them LLLexer.cpp; grep for `include_str!` across `crates/*/tests/` finds no test reading any AsmParser C++ source. Upstream read: `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLLexer.cpp` — the `KEYWORD`/`TYPEKEYWORD`/`INSTKEYWORD`/`DWKEYWORD` macro definitions and `LexIdentifier`'s `// Finally, if this isn't known, return an error. CurPtr = TokStart+1; return lltok::Error;`. Diff performed in the scratchpad: 330 KEYWORD + 66 INSTKEYWORD + 13 TYPEKEYWORD spellings (409 unique) vs. 510 production `b"..."` literals; `comm -23` yields only `DISPLAY_NAME`, `comm -13` yields only Attributes.td attribute keywords (align, allockind, byref, ... zeroext).

</details>

### 88. The corpus manifest covered 9 fixtures against 500 upstream `test/Assembler` files — **FIXED (W14c)**

*tests / corpus* — crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt, crates/llvmkit-asmparser/tests/parser_corpus.rs, crates/llvmkit-asmparser/tests/fixtures/upstream/

- **LLVM:** n/a — this is llvmkit's completeness proof, not an upstream behaviour. Upstream's `test/Assembler` holds exactly 500 `.ll` files: 257 `not llvm-as` negatives and 175 `llvm-as | llvm-dis` round-trips.
- **llvmkit:** closed in W14c. The manifest now carries a row per ported fixture, most `status=reject` rows pinning upstream's diagnostic through `error=` and many the reported span through `loc=` (`grep -c 'status=' crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt` and the same per field, re-derive rather than copy). `parser_corpus.rs`'s module doc documents every status plus the `error=` FileCheck-substring rule and the `loc=` span rule.
- **Why:** recorded as W14's "mass fixture port — the proof": classify every upstream fixture as `ported` / `blocked-model` / `N/A` with a one-line rationale. That classification shipped as [`fixture-coverage.md`](fixture-coverage.md), which holds the per-class and per-gap tallies and the command that derives them; no copy is kept here.
- **Residue:** none left in the manifest — the three `status=xfail-parse` rows were upstream negatives misfiled as llvmkit gaps and are `reject` rows now, with the duplicate row and its duplicate fixture file deleted. The live coverage index is `fixture-coverage.md`, not this entry.
- **Two standing traps, still true:** an upstream `CHECK` block is a *pipeline's* output — check the `RUN` line before treating a mismatch as a bug — and an `xfail` reason is a hypothesis; unblocking one has twice revealed an unrelated defect.
- **Correction from verification (2026-08-20, fix round 3):** every number this entry stated was falsified by the tree, including inside its own evidence block, which is why that block was deleted rather than repaired. "9 manifest entries" against 502; "116 lines" for a 270-line driver; a status vocabulary with no `reject`/`error=`/`loc=`; "124 fixtures on disk with 115 unmanaged" and a later correction of "238 fixture `.ll` files, 233 under `fixtures/upstream/`, unmanaged count 229" against **754** `.ll` files on disk with 747 under `upstream/` (`find crates/llvmkit-asmparser/tests/fixtures -name '*.ll' | wc -l`, and the same restricted to `.../fixtures/upstream`, re-derived at this commit and unchanged since `ea57b14`; the correction as first written said 749/742, which was this round's *base* `b369431` and was already stale when written — commit `4e27ae7` of the same round vendored the five `Verifier/` and `compatibility/` fixtures that make up the delta); and a cited path, `crates/llvmkit-asmparser/tests/parser_corpus_manifest.txt`, that does not exist — the file is one level down under `tests/fixtures/`, which the entry's own correction flagged while leaving the header uncorrected. Only "500 upstream `test/Assembler` files" survived. The `Fix:` line described work that had already shipped, so a reader planning the next corpus wave would have re-done W14's completeness proof.

### 120. `Verifier::visitGetElementPtrInst`'s result-element-type check has nothing to compare

*verifier / IR model* — crates/llvmkit-ir/src/verifier.rs (`check_gep`); crates/llvmkit-ir/src/instr_types.rs (`GepInstData`)

- **LLVM:** `GetElementPtrInst` stores `ResultElementType`, initialised to `getIndexedType(PointeeType, IdxList)` — which may be null — and `Verifier::visitGetElementPtrInst` asks `Check(PtrTy && GEP.getResultElementType() == ElTy, "GEP is not of right type for indices!")`, re-deriving `ElTy` from the source type and the index list and comparing the two.
- **llvmkit:** `GepInstData` stores `source_ty`, the operand slots and the no-wrap flags, and nothing else. There is no stored result element type, so there is nothing for a re-derived `ElTy` to disagree with, and the `PtrTy` half is all that is portable — `check_gep` ports that half as `VerifierRule::GepNonPointerResult`. The null half of upstream's field is spelled instead as a rejection at construction: `IrBuilder::gep_inner` and `IrBuilder::gep_erased` both return `IrError::GepInvalidIndices` when `getIndexedType` walks off the source type, which is why the Verifier check has nothing left to catch.
- **Why:** Structural, and in llvmkit's favour: a stored-but-stale `ResultElementType` is a state upstream can reach and llvmkit cannot represent. Recorded so the missing `Check` is not read as an oversight.
- **Fix:** None wanted. If a future change ever stores a result element type (say, to speed an analysis), the check comes back with it.

- **And vacuous rather than missing, in the same routine:** `Check(GEP.getAddressSpace() == PtrTy->getAddressSpace(), "GEP address space doesn't match type")`. `GetElementPtrInst::getAddressSpace` is the *pointer operand's* address space, and its own comment says "this is always the same as the pointer operand's address space". In llvmkit both `gep_inner` and `gep_erased` derive the result type from the base operand by `getGEPReturnType`, and `Instruction::replace_all_uses_with` refuses a replacement whose type slot differs, so the two address spaces are the same interned slot by construction. Recorded as vacuous, not fixed.

<details><summary>Verification evidence (2026-08-21)</summary>

Upstream read at the vendored tag `llvmorg-22.1.4` (the repo commit does not pin `orig_cpp/`, which is gitignored): `llvm/lib/IR/Verifier.cpp::Verifier::visitGetElementPtrInst` — the eight statements, in order, are the base `isa<PointerType>` check, `isSized`, the struct-scalable `Check`, the `all_of` integer-index `Check`, `ElTy = getIndexedType(...)` plus `Check(ElTy, ...)`, `PtrTy = dyn_cast<PointerType>(GEP.getType()->getScalarType())` plus the two-conjunct `Check`, the vector block, and the trailing address-space `Check`. Every one of those is emitted by `check_gep` except the second conjunct of the `PtrTy` `Check` and the address-space `Check`; the struct-scalable one was unported when this entry was first written and was ported in the fix round that narrowed it (`VerifierRule::GepScalableStructSource`, locked by `verifier_basic.rs::verify_gep_into_scalable_struct_fails`). `llvm/include/llvm/IR/Instructions.h` — `GetElementPtrInst`'s constructor initialiser list is `SourceElementType(PointeeType), ResultElementType(getIndexedType(PointeeType, IdxList))`, and `getAddressSpace()` forwards to `getPointerAddressSpace()`, which is `getPointerOperandType()->getPointerAddressSpace()`, under the comment quoted above. llvmkit side, at this commit: `crates/llvmkit-ir/src/instr_types.rs::GepInstData` has exactly four fields (`source_ty`, `ptr`, `indices`, `flags`); `crates/llvmkit-ir/src/ir_builder.rs::gep_return_type` derives the result type from the base operand's type alone; `crates/llvmkit-ir/src/instruction.rs::replace_all_uses_with` rejects a replacement with `IrError::TypeMismatch` when `new_value.ty != self.ty`, and no public operand setter exists (`grep -n 'pub fn set_operand\|pub fn replace_operand' crates/llvmkit-ir/src/*.rs` returns nothing), so an operand cannot be swapped for one in another address space after construction.

</details>

## Coverage, tooling and provenance

Nothing changes for a well-formed module; these are gaps in what is measured, guarded or recorded.

### 89. `parse_optional_refs` sorts stably where upstream's sort is unstable

*parser (LLParser) / module summary index* — crates/llvmkit-asmparser/src/ll_parser.rs:3726, reason at crates/llvmkit-asmparser/src/ll_parser.rs:3706-3711

- **LLVM:** `LLParser::parseOptionalRefs` sorts the parsed references by access specifier with `llvm::sort`, which is `std::sort` — unstable, and deliberately shuffled first under `EXPENSIVE_CHECKS`. Ties (references sharing an access class) therefore end up in an unspecified order, and `FunctionSummary::specialRefCounts` and the printer only rely on read-only/write-only sitting at the end.
- **llvmkit:** `parsed.sort_by_key(|(reference, _)| reference.value.access)` — Rust's stable sort — so ties keep source order. For a `refs:` list with several references of one access class, llvmkit's printed order is deterministic where upstream's is not.
- **Why:** Recorded inline at `ll_parser.rs:3706-3711`. No reason is given for preferring stability beyond it being what `sort_by_key` does; the note frames it as an observation rather than a decision.
- **Fix:** Low-stakes but worth resolving explicitly. Since upstream's order is *unspecified* rather than different, llvmkit's stable order is a legal refinement — so the fix is documentation, not code: restate the comment as "upstream leaves ties unspecified; llvmkit pins source order, which is one of the orders `llvm::sort` may produce", and add the `UPSTREAM.md` row. Only change to `sort_unstable_by_key` if a ported fixture actually depends on upstream's particular permutation, which none does.

<details><summary>Verification evidence</summary>

C:\Users\olegg\Desktop\llvmkit\crates\llvmkit-asmparser\src\ll_parser.rs:3726 is verbatim `parsed.sort_by_key(|(reference, _)| reference.value.access);` inside `parse_optional_refs`, with the rustdoc at 3706-3711 self-documenting the divergence (upstream unstable + EXPENSIVE_CHECKS shuffle vs. llvmkit stable); `slice::sort_by_key` is Rust's stable sort. Upstream `LLParser::parseOptionalRefs` at orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp:10442 builds a `std::vector<ValueContext>` then at 10468-10470 calls `llvm::sort(VContexts, [](const ValueContext &VC1, const ValueContext &VC2){ return VC1.VI.getAccessSpecifier() < VC2.VI.getAccessSpecifier(); })` with the comment citing `FunctionSummary::specialRefCounts()`. The comparator overload of `llvm::sort` (include/llvm/ADT/STLExtras.h:1652-1662) is unconditionally `std::sort` preceded by `detail::presortShuffle` under `#ifdef EXPENSIVE_CHECKS` — it does NOT take the `array_pod_sort` trivially-copyable fast path, which exists only on the comparator-less overload at 1634, so the shuffle really does apply here. Sort keys order identically at both ends, so this is purely a stability difference, not also an ordering one: upstream `ValueInfo::Flags` = `{HaveGV=1, ReadOnly=2, WriteOnly=4}` masked by `getAccessSpecifier()` (include/llvm/IR/ModuleSummaryIndex.h:220, 265-268) gives 0 < 2 < 4, and llvmkit's `AccessSpecifier { None, ReadOnly, WriteOnly }` with derived `Ord` (crates/llvmkit-ir/src/module_summary_index.rs:341-350) gives 0 < 1 < 2. Consequence as claimed: a `refs:` list with multiple references of one access class keeps source order in llvmkit and is unspecified upstream.

</details>

### 90. DIExpression oversized-element test asserts nothing, on a stale message claim

*parser (debug metadata)* — crates/llvmkit-asmparser/tests/parser_debug_metadata.rs:372-387 (stale doc + discarding assertion); contradicted by the same file at :985-988 and crates/llvmkit-asmparser/src/ll_parser.rs:5498

- **LLVM:** `test/Assembler/invalid-diexpression-large.ll` accepts an element of exactly `UINT64_MAX` (`CHECK-NOT: error:`) and rejects one above it with `element too large, limit is 18446744073709551615` from `LLParser::parseDIExpressionBody`.
- **llvmkit:** The test's doc says "Same logic as upstream, different diagnostic: … llvmkit reports the structured `Expected` error its parser uses throughout, so this asserts on the accept/reject behaviour rather than on message text", and the body discards the error entirely: `let _ = parse_err("…18446744073709551616…");`. **The message exists.** `ll_parser.rs` emits `format!("element too large, limit is {}", u64::MAX)`, and a *different* test in the same file already asserts it verbatim.
- **Why:** Unrecorded — the message landed later and this doc/body pair was not revisited; the divergence excuse outlived the divergence.
- **Fix:** Replace `let _ = parse_err(...)` with `assert_eq!(parse_err(...).to_string(), "element too large, limit is 18446744073709551615")` and delete the "different diagnostic" paragraph, so the port asserts the fixture's actual CHECK line.
- **Correction from verification:** Still present, with one wording fix: the test does not "assert nothing". `parse_err` (crates/llvmkit-asmparser/tests/parser_debug_metadata.rs:152-158) ends in `.expect_err("parse must fail")`, so `let _ = parse_err("...18446744073709551616...")` at :386 does assert the input is rejected — it discards only the error *value*, i.e. the message. Accurate statement of the divergence: the doc comment at :374-378 is stale. It claims "Same logic as upstream, different diagnostic: … llvmkit reports the structured `Expected` error its parser uses throughout, so this asserts on the accept/reject behaviour rather than on message text" — but llvmkit does emit upstream's exact message. `LLParser`-mirroring code in crates/llvmkit-asmparser/src/ll_parser.rs:5496-5500 returns `self.message(format!("element too large, limit is {}", u64::MAX))` on the `digits.parse::<u64>()` failure (no `Expected` error on that path), and `di_expression_validates_its_operands` in the *same* test file at :985-988 already asserts `parse_err("!0 = !DIExpression(18446744073709551616)\n").to_string() == "element too large, limit is 18446744073709551615"` verbatim. So the port of `test/Assembler/invalid-diexpression-large.ll` weakens its own reject half to a bare accept/reject check, and justifies it on a premise the codebase contradicts. Fix is a two-line change: replace :386 with an `assert_eq!` on the message and delete the "different diagnostic" paragraph.

<details><summary>Verification evidence</summary>

1) crates/llvmkit-asmparser/tests/parser_debug_metadata.rs:372-387 — read verbatim; doc comment carries the "different diagnostic … structured `Expected` error" claim and the body's reject half is `let _ = parse_err("!0 = !DIExpression(18446744073709551616)\n");`. 2) Same file :152-158 — `parse_err` is `...parse_module().expect_err("parse must fail")`, which is why the discarded line still asserts rejection (basis for the correction above). 3) Same file :975-989 — `di_expression_validates_its_operands` asserts the message verbatim as "element too large, limit is 18446744073709551615". 4) crates/llvmkit-asmparser/src/ll_parser.rs:5484-5503 — the non-`DW_OP_`/`DW_ATE_` arm parses the decimal literal and on overflow returns `self.message(format!("element too large, limit is {}", u64::MAX))`; the only `Expected` error there is the unrelated `self.expected("unsigned integer")` for a non-integer token. 5) orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/invalid-diexpression-large.ll — `CHECK-NOT: error:` over `!DIExpression(18446744073709551615)` and `CHECK: … error: element too large, limit is 18446744073709551615` over `!DIExpression(18446744073709551616)`; message matches llvmkit's byte-for-byte. 6) `git diff --stat` on the test file is empty and `git show HEAD:...ll_parser.rs | grep "element too large"` hits at HEAD line 5475, so this is committed state (test last touched by f68bee0), not an uncommitted working-tree artifact.

</details>

### 91. return-self-good.ll described as blocked while the manifest runs it as passing

*parser corpus / provenance ledger* — crates/llvmkit-asmparser/tests/parser_constants.rs:859-867; UPSTREAM.md:1170; contradicted by crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt

- **LLVM:** `test/Bitcode/blockaddress-addrspace.ll::return-self-good.ll` uses `target datalayout = "P2"` with `@take_self_prog_as` declaring no address space, so the program address space supplies it.
- **llvmkit:** Two records disagree. `parser_corpus_manifest.txt` lists `upstream/blockaddress-addrspace/return_self_good.ll … status=pass` (and `CHANGELOG.md` records it moving `xfail-parse` -> `pass`), while `parser_constants.rs` and its `UPSTREAM.md` row still say "that fixture is still blocked on the *program* address space … which is W3 work" and route around it by dropping the address spaces.
- **Why:** Unrecorded — the doc was not updated when the fixture started passing.
- **Fix:** Confirm the corpus entry really exercises the whole fixture, then rewrite `same_function_forward_blockaddress_resolves_by_name` to use `upstream/blockaddress-addrspace/return_self_good.ll` directly (or state plainly that the address-space-stripped shape isolates the `BlockAddressPFS` rule), and correct the `UPSTREAM.md` classification from `llvmkit-specific subset` to `port`.

<details><summary>Verification evidence</summary>

Confirmed on dev (working tree today). (1) crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt:12 lists `upstream/blockaddress-addrspace/return_self_good.ll | test/Bitcode/blockaddress-addrspace.ll::return-self-good.ll | status=pass`, and the checked-in fixture is the upstream text verbatim including `target datalayout = "P2"` and `define ptr addrspace(2) @take_self_prog_as()` with no function-level addrspace (matches orig_cpp/.../llvm/test/Bitcode/blockaddress-addrspace.ll:104-118). (2) parser_corpus.rs enforces status=pass as both parse and verify (parse_result.unwrap_or_else "should parse" plus verify_result.unwrap_or_else "should verify"), and I ran `cargo +1.96.0 test -p llvmkit-asmparser --release --test parser_corpus` -> parser_corpus_round_trips_checked_in_fixtures ok, so the fixture genuinely passes. (3) The parser has the capability: crates/llvmkit-asmparser/src/ll_parser.rs:2202-2207 `parse_optional_program_addr_space` ("Mirrors LLParser::parseOptionalProgramAddrSpace"), called from the define/declare paths at :10378 and :10679, with `"P" => layout.program_addr_space()` at :2172. CHANGELOG.md:991-994 records the entry moving xfail-parse -> pass. (4) Yet the two cited records are unchanged: parser_constants.rs:864-867 doc comment on `same_function_forward_blockaddress_resolves_by_name` still says "that fixture itself is still blocked on the *program* address space (`target datalayout = \"P2\"` reaching a function that declares none), which is W3 work", and UPSTREAM.md:1170 still carries "(that fixture is still blocked on the *program* address space, W3)" with classification `llvmkit-specific subset`. The fix is prose-only: the blockaddress-self/ fixtures still isolate distinct rules (self-reference by name, and the numbered spelling via PerFunctionState::getBB(unsigned), which no upstream .ll isolates), so the stale W3 rationale should be removed from both records rather than the tests deleted.

</details>

### 92. Bare riscv_vls_cc swallows the following token (upstream bug, reproduced deliberately)

*parser (calling conventions)* — crates/llvmkit-asmparser/tests/calling_conv_drift.rs:172-195

- **LLVM:** `parseOptionalCallingConv`'s `kw_riscv_vls_cc` arm consumes its own keyword and, finding no `(`, `break`s to the switch tail — which consumes a second token. So `declare riscv_vls_cc void @f()` loses its return type. This is upstream's actual behaviour, not its intent.
- **llvmkit:** Reproduced exactly and pinned: `declare riscv_vls_cc void @f()` is an error, and `declare riscv_vls_cc void void @f()` parses at the default ABI_VLEN of 128.
- **Why:** Recorded, and the reasoning is explicit: unreachable from printed IR (`printCallingConv` always writes the parameterised form), reproduced because "the contract is upstream's behaviour", noted in `docs/future-work.md`, and "if upstream fixes it, this test is what says so".
- **Fix:** No action while tracking LLVM 22.1.4 — this is the parity-correct answer. Re-check on the next vendored-tree bump; the test is designed to fail if upstream drops the fallthrough.

<details><summary>Verification evidence</summary>

Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp, LLParser::parseOptionalCallingConv — the kw_riscv_vls_cc arm alone calls Lex.Lex() for itself, sets CC = RISCV_VLSCall_128, and on `if (!EatIfPresent(lltok::lparen)) break;` falls to the switch's common tail `Lex.Lex(); return false;`, consuming a second token. Every other arm reaches that tail unconsumed. llvmkit: crates/llvmkit-asmparser/src/ll_parser.rs:13168 dispatches RiscvVlsCc to parse_riscv_vls_calling_conv (13200-13206) with an early return that skips the shared bump at 13180; inside, after expect_keyword and a failed eat_punct(LParen) it does `self.bump()?` with the comment "The upstream double-`Lex.Lex()` described above" and returns RISCV_VLS_CALL_128. Test: crates/llvmkit-asmparser/tests/calling_conv_drift.rs lines 172-195 (a_bare_riscv_vls_cc_swallows_the_next_token) asserts `declare riscv_vls_cc void @f()` is an error and `declare riscv_vls_cc void void @f()` parses and prints containing riscv_vls_cc(128) — the cited line range is exact. Ran `cargo +1.96.0 test --release -p llvmkit-asmparser --test calling_conv_drift`: 6 passed, 0 failed, that test included. Also recorded at docs/future-work.md:180-188; landed in commit 4cfeea6 (LLParser parity W6). Note this is a divergence from upstream's intent, not its behaviour — llvmkit is bug-for-bug identical by design.

</details>

### 94. `Can't read textual IR with a Context that discards named Values` is structurally unreachable

*parser — entry points* — crates/llvmkit-asmparser/src/parser.rs; ~/.claude/plans/llparser-parity-ledger.md

- **LLVM:** `LLParser::Run` refuses to parse when the `LLVMContext` has `shouldDiscardValueNames()` set.
- **llvmkit:** llvmkit has no discard-names mode at all, so the condition cannot arise and the message has no home.
- **Why:** Recorded as a W13 decision: classify as N/A-with-rationale in the ledger rather than inventing a trigger. Listed here so the final ledger's `MISSING = 0` target is not chased through this row.
- **Fix:** Mark the ledger row `N/A(structurally-impossible)` with the rationale, in the W14 ledger-v3 pass.

<details><summary>Verification evidence</summary>

Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp, LLParser::Run — right after the priming Lex.Lex(), `if (Context.shouldDiscardValueNames()) return error(Lex.getLoc(), "Can't read textual IR with a Context that discards named Values");`. The flag is declared in include/llvm/IR/LLVMContext.h as shouldDiscardValueNames() / setDiscardValueNames(bool). llvmkit: C:\Users\olegg\Desktop\llvmkit\crates\llvmkit-asmparser\src\ll_parser.rs line 1406, `pub fn parse_module`, doc-commented "Drive the parser to EOF. Mirrors `LLParser::Run`" — it goes straight from priming into the !M summary-only loop / top-level dispatch loop with no discard-names guard. Grep for "Can't read textual IR" over crates/ and docs/ returns zero hits; grep -riE "discard_?(value)?_?names|should_discard|strip_names|setDiscardValueNames" over crates/, docs/, AGENTS.md, README.md also returns zero hits. Structural half confirmed rather than assumed: C:\Users\olegg\Desktop\llvmkit\crates\llvmkit-ir\src\llvm_context.rs defines `pub(crate) struct Context` as a per-module type/constant interning pool whose module doc states "`Context` is `pub(crate)` — the public surface is on [`Module`]"; it holds no configuration flags (its only bool fields are `is_var_arg` and `packed` on type keys). ModuleCore in crates\llvmkit-ir\src\module.rs (~line 1530) stores `name` and the by-name tables unconditionally. The entry points in crates\llvmkit-asmparser\src\parser.rs (parse_into, parse_branded, parse_dynamic, parse_assembly*, parse_assembly_with_context) either take a caller's Module<B, Unverified> or construct Module::dynamic("asm") — no object carries a discard-names mode, so the condition cannot arise. Minor pointer nit (not a substantive inaccuracy): the claim's "Where" cites crates/llvmkit-asmparser/src/parser.rs, but the LLParser::Run mirror is Parser::parse_module in crates/llvmkit-asmparser/src/ll_parser.rs; parser.rs holds only the public wrapper functions. The ledger at ~/.claude/plans/llparser-parity-ledger.md line 26 still lists this under "`Run` — 1 messages, 1 missing" as unchecked, consistent with the tree.

</details>

### 95. `argument can not have void type` is unreachable upstream and is recorded, not invented — **NARROWED**

*parser — function header* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_argument_list`); ~/.claude/plans/llparser-parity-ledger.md

- **LLVM:** `argument can not have void type` sits behind `parseArgumentList`'s `AllowVoid = false`, so `parseType` already refuses a literal `void` with `void type only allowed for function results` before the guard is reached.
- **llvmkit:** The message does not exist, matching upstream's reachable behaviour exactly.
- **Why:** Recorded in W8: "recorded rather than given invented triggers". Listed so the ledger's MISSING row for it is not treated as a gap.
- **Fix:** Mark the ledger row `N/A(unreachable-upstream)`.
- **Narrowed:** this entry used to carry a second message, `unable to create block numbered '<N>'`, on the same rationale. That rationale was wrong — `checkValueID` guards `ID < NextID` only, and `getVal`'s `checkValidVariableType` is what a numbered label collides with — and the message is now emitted. So is the `ForwardRefVals` half of its named twin, which llvmkit had implemented for the symbol-table half only. Both are pinned by `parser_module_level.rs`, message and column.

### 96. `test/Assembler/thinlto-vtable-summary2.ll` and `invalid-name*.ll` are unportable fixtures

*tests / fixtures* — crates/llvmkit-asmparser/tests/parser_summary.rs, crates/llvmkit-asmparser/tests/fixtures/upstream/

- **LLVM:** `thinlto-vtable-summary2.ll` runs `opt %s -S -module-summary`, which *generates* a summary index from the module's type metadata; there is no `^N` block in the input. `invalid-name*.ll` are binary files with embedded NUL bytes pinning lexer name handling.
- **llvmkit:** Neither is ported. The other sixteen summary fixtures are ported in `parser_summary.rs`; the metadata negatives around `invalid-name*.ll` are ported.
- **Why:** Recorded in docs/future-work.md and in W7 part 6 respectively: the first is the module-summary *analysis*, not the parser, and llvmkit has neither; the second cannot be expressed as a Rust string literal, and it was recorded rather than skipped silently.
- **Fix:** Leave the first as N/A until a module-summary analysis exists. For the second, load the fixture from a checked-in binary file (`include_bytes!`) rather than a literal if the coverage is judged worth it; otherwise keep the N/A row with its rationale.
- **Correction from verification:** Substantially accurate and still present, with three wording corrections. (a) The `thinlto-vtable-summary2.ll` half is exact. Upstream's RUN line is `opt %s -S -module-summary | FileCheck %s`; the input contains zero `^` entries, and the expected `^2 = gv: (name: "_ZTS1A"` / `^6 = typeidCompatibleVTable: (name: "_ZTS1A")` are generated from the module's `!type` metadata. It is unported, and the 16-of-17 count is exact: `test/Assembler` has 17 summary fixtures, llvmkit ports 16, the delta is precisely this file. (b) The `invalid-name*.ll` half is accurate as to the fixtures (both are binary, both unported) but the claim understates llvmkit's coverage: the *behavior* they pin IS implemented and partly tested. `ll_lexer.rs::lex_quoted_name` (line 591) and `ll_lexer.rs::lex_quote`'s label path (line 574) both return `LexError::NulInName`, whose message (line 63) is upstream's verbatim "NUL character is not allowed in names", and `ll_lexer_tests.rs::nul_in_quoted_name_is_error` (line 254) covers the name case. What is genuinely missing is only the label-path test (`invalid-name2.ll`'s case) and any exercise of a *raw* NUL byte as opposed to the `\00` escape. (c) "unportable" is overstated. U+0000 is valid UTF-8, so `include_str!` would accept a fixture containing a raw NUL byte; nothing in Rust prevents checking these in. The barrier is tooling/reviewability, not expressiveness. (d) "the metadata negatives around `invalid-name*.ll` are ported" is loosely worded. What is ported is the `invalid-di*.ll` metadata family (alphabetical neighbors), per UPSTREAM.md lines 1330 and 1332. No llvmkit test cites `invalid-name.ll` or `invalid-name2.ll`.

<details><summary>Verification evidence</summary>

C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/thinlto-vtable-summary2.ll -- read in full: RUN lines are `opt %s -S -module-summary | FileCheck %s` and `opt %s -S -module-summary -o - | llvm-as -o - | llvm-dis -o - | FileCheck %s`; CHECKs are `^2 = gv: (name: "_ZTS1A"` and `^6 = typeidCompatibleVTable: (name: "_ZTS1A"`; `grep -cE "^\s*\^"` over the file returns 0, so the input has no summary block -- the index is generated from `!0 = !{i64 16, !"_ZTS1A"}` / `!1 = !{i64 16, !"_ZTSM1AFivE.virtual"}`. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/parser_summary.rs -- lines 44-66 hold exactly 16 `include_str!("fixtures/upstream/summary/...")` lines; `ls` of C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/fixtures/upstream/summary/ shows the same 16 files. Upstream `test/Assembler` holds 17 (the 16 plus thinlto-vtable-summary2.ll). Repo-wide grep for "vtable-summary2" excluding orig_cpp/target/.git hits only C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:47, which records the omission with the same rationale. `od -c` of orig_cpp/.../test/Assembler/invalid-name.ll shows `% " \0 " = s e x t i 1 6 0 t o i 3 2`; invalid-name2.ll shows `" \0 " :`. `file` reports "data" for both. Repo-wide grep for "invalid-name" excluding orig_cpp returns nothing -- neither fixture is checked in, and no test or UPSTREAM.md row cites them. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_lexer.rs -- line 63 `#[error("NUL character is not allowed in names")] NulInName { span: Span }`; line 574 rejects NUL in the quoted-label path (`lex_quote`); line 591 rejects NUL in the quoted-name path (`lex_quoted_name`), both after `escape::unescape`. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_lexer_tests.rs:254 `nul_in_quoted_name_is_error` asserts `LexError::NulInName` for `@"a\00b"` -- the name path only, via escape rather than a raw byte. C:/Users/olegg/Desktop/llvmkit/UPSTREAM.md:1330 and :1332 -- rows for parser_debug_metadata.rs citing `invalid-dilocation-field-bad.ll` and fourteen other `invalid-di*.ll` negatives; none cite invalid-name*.ll.

</details>

### 97. The ledger's `present` column is a string proxy and its Twine column undercounts by construction — **TOOL FIXED (W14d); the proxy itself remains**

*measurement / ledger* — ~/.claude/plans/llparser-parity-ledger.md, ~/.claude/plans/llparser-tools/ledger_v2.py

- **LLVM:** n/a — measurement infrastructure for the parity program. The denominator is 516 exact messages (`error` 221, `parseToken` 200, `tokError` 90, `checkValueID` 7, `parseValueAsMetadata` 2) plus 84 Twine templates and 11 lexer messages.
- **llvmkit:** `ledger_v2.py` globs every string literal under `crates/llvmkit-asmparser/src`, so `present` proves the text exists in the sources — never that a code path reaches it (W7's lesson: `redefinition of global '@x'` had a variant, a `Display` and a unit test and never fired). The Twine-fragment column matches fragments literally while llvmkit builds those texts with `format!`, so correctly-ported messages read unticked.
- **Why:** Recorded in both documents. Current standing: 411 present / 105 missing of 516 at the W11 boundary. W14's ledger v3 targets `MISSING = 0`, every row EXACT-with-a-citing-test or `N/A(rationale)`.
- **Fix:** Regenerate before quoting any count — `python ledger_v2.py <orig_cpp/llvm-project-llvmorg-22.1.4> <workspace root, NOT src> <ledger path>`; passing `src` yields `present=0` silently. Read `present` as a ceiling on parity, and treat only the 516 exact-literal column as a scoreboard.
- **W14d update — two extraction defects fixed, and the recorded history retired.** (1) `llvmkit_strings` no longer regexes `"..."`; it lexes Rust (line and nested block comments, raw/byte/C strings, char literals, lifetimes). The regex desynchronized permanently at the first lone `"` inside a `//` comment — `ll_parser.rs`'s `DIExpression` arm of `parse_named_metadata` has one — hiding every literal after it. (2) It now harvests every literal under `crates/llvmkit-ir/src`, not only `#[error("...")]`: the five `constant ptrauth …` checks carry their text in a plain `message:` field of a struct variant and were counted missing while llvmkit emits them. (3) A new `[~]` template column credits the 34 upstream literals llvmkit renders through one interpolated message — `{opcode} constexprs are no longer supported` (29), `expected scope value for {pad}` (3), `invalid type for {what} constant` (2) — which no literal search can match. Point (2) of the correction above is confirmed and now moot: with doc comments correctly skipped, the Twine column's spurious tick set is gone and it reads 0 of 84, all of them `format!`-built. **Standing after the fix: 516 exact — 466 covered (432 literal + 34 template), 50 missing, of which 4 are `N/A` and 46 are real gaps**, classified per message in the ledger. The proxy nature is unchanged: `covered` still proves only that the text exists, never that a path reaches it.
- **The program's recorded numbers were measured with the broken tool and should not be compared across this commit.** Re-measuring with the fixed extractor gives W11 (`2ac3e3a`) 412 present / 104 missing — not the recorded 411/105 — and W12 (`dfcb43d`) through W13d (`bd90449`) 427/89. The old tool reported 335 at those same W12/W13 commits, i.e. an apparent 76-message regression that never happened; it was the `//`-comment desync landing in W12.
- **Correction from verification:** Substantially accurate; two refinements, one of which makes the defect worse than claimed. (1) Scope imprecision: `ledger_v2.py` does not glob only `crates/llvmkit-asmparser/src`. `llvmkit_strings()` (ledger_v2.py:223-241) globs every string literal under `crates/llvmkit-asmparser/src/**/*.rs` AND every `#[error("...")]` attribute under `crates/llvmkit-ir/src/**/*.rs`, then synthesizes an `"expected " + s` variant of each. The ir-crate glob is deliberate (its comment: "some diagnostics originate as IrError text") and is load-bearing — two ticked messages, `value has no uses` and `value only has one use`, are present only via `llvmkit-ir/src/value.rs:244,247`, which is correct because `ll_parser.rs:4904 sort_use_list_order` renders the IrError with `e.to_string()`. (2) The Twine column is worse than "undercounts": it is simultaneously a false-negative and a false-positive machine. It ticks 3 of 84 fragments. One is the bare `'` character. The other two — `redefinition of global '@` and `use of undefined value '@` — are ticked ONLY because of doc comments quoting the C++ source (`parse_error.rs:66`, `:90`, `:460`); no llvmkit code path contains either literal, since both are built structurally via `#[error("redefinition of {kind} '{sigil}{id}'", sigil = .kind.sigil())]` at parse_error.rs:202. So the column's entire meaningful tick set is spurious. One qualification on the `present` column: the mechanism is unchanged, but the tree currently exhibits zero demonstrable false positives in it. I probed for three W7-shaped classes across all 426 ticked messages and found none: 0 ticked with every occurrence inside a `//` comment, 0 ticked with every occurrence inside a `#[cfg(test)]` module or `ll_lexer_tests.rs`, and 0 `ParseError` variants declared in parse_error.rs but never named elsewhere in the crate. The specific W7 instance is closed — `ParseError::Redefinition` is now constructed at 4 sites, 5 of them with `SymbolKind::Global`. The tool that failed to catch it is unchanged, so the proxy would not catch a recurrence. Separately: the checked-in ledger is stale. It records "516 exact — 411 present, 105 missing"; regenerating against the tree today gives 426 present / 90 missing.

<details><summary>Verification evidence</summary>

Ran `~/.claude/plans/llparser-tools/ledger_v2.py` against the current tree: output `exact=516 present=426 missing=90 / templates=84 lexer=11`, channels `error: 221, parseToken: 200, tokError: 90, checkValueID: 7, parseValueAsMetadata: 2` — the claim's denominator reproduces exactly. ledger_v2.py:223-241 (`llvmkit_strings`) is a bare `re.finditer(r'"((?:[^"\\]|\\.)*)"', text)` over every `.rs` under the asmparser src dir plus `#[error(...)]` under llvmkit-ir src, with zero reachability analysis — comments, doc comments, `#[cfg(test)]` modules (10 in the crate) and `ll_lexer_tests.rs` all feed the `present` set. ledger_v2.py:330-334 tests the Twine column with `frag in kit`, i.e. exact set membership of a whole string, so a fragment can never match a `format!` template or a substring. Twine undercount, measured: of the 81 unticked fragments, 47 are a literal substring of an existing llvmkit string literal. Cleanest case — upstream LLParser.cpp:3096-3099 `const char *Msg = "unexpected ellipsis in argument list for "; tokError(Twine(Msg) + "non-musttail call")`; llvmkit has the fully concatenated text at ll_parser.rs:12960, 12965, 14057, 14195, yet both fragments read `[ ]`. Parametric-variant cases: `wrong number of indexes, expected ` is `#[error("wrong number of indexes, expected {expected}")]` at llvmkit-ir/src/value.rs:251; the thirteen `invalid DWARF *` fragments collapse into `#[error("invalid {what} '{value}'")]` at parse_error.rs:304 with `what: "DWARF op" / "DWARF attribute encoding" / "DWARF enum kind code"` at ll_parser.rs:5456, 5468, 5782; `redefinition of comdat '$` is parse_error.rs:202 driven by `SymbolKind::Comdat => '$'` (parse_error.rs:97) and constructed at ll_parser.rs:4831. Twine false positives: `grep "use of undefined value '@"` over both crates returns exactly one hit, parse_error.rs:66 — a doc comment. `grep "redefinition of global '@"` returns parse_error.rs:90 and :460 (doc comments) plus :456 and :473 (test assertions, and those carry `'@foo'` so they match a different string). Upstream's real sites are LLParser.cpp:408 and :413. Ledger scoreboard read at ~/.claude/plans/llparser-parity-ledger.md:8-10 ("411 present, 105 missing"); its Twine section at :1237-1249 carries no explanatory prose and line :1247 shows `- [x] '` — the bare apostrophe ticked as present.

</details>

### 98. `UPSTREAM.md` provenance debt: tests with no row

*tests / provenance* — UPSTREAM.md, crates/llvmkit-ir/tests/, crates/llvmkit-ir/src/

- **LLVM:** n/a — D11 house law: every `#[test]` cites its upstream source and gets an `UPSTREAM.md` row in the same commit.
- **llvmkit:** a residue of tests carry no row. The debt is inherited from the type-safety and pass-API programs and sits in `llvmkit-ir` rather than in the parser crates, whose waves add rows per commit.
- **No figure is recorded here, and none in `UPSTREAM.md`'s header either.** This entry carried one, then a recount of it, then a correction to the recount, and the header carried three at once that did not agree with each other. The split also has no honest one-liner: matching rows to tests by their `path.rs::name` segment counts every whole-file *group* row's tests as unrowed. `UPSTREAM.md`'s header names the commands and says what a real audit costs; read it there.
- **Fix:** enforce the per-wave rule — a row in the same commit — and clear the backlog file by file. A missing row means missing *provenance*, never "no upstream counterpart", so each backfill has to name a real source or say explicitly that the test is llvmkit-specific.
- **What is now mechanically checked (2026-08-22):** `crates/llvmkit-ir/tests/upstream_registry_drift.rs` fails if a row names a file absent from the tree, or a test its cited file does not define. That closes the failure mode this entry's earlier prose kept having to scope around — rows naming a file the test had moved out of, which a name-only audit cannot see. Eleven such rows existed and are repaired. It does **not** check the upstream citation in the second column; that is `docs/fixture-coverage.md`'s phantom-citation finding.

## Checked and found already closed

The entries below did not survive verification. They are kept so nobody
re-derives them; each was a recorded belief that the tree no longer supports.

### A parsed module's summary index is not attached to the module, so `Display` never emits `^N`

*IR model* — crates/llvmkit-asmparser/src/ll_parser.rs:421 (`ParsedModule::summary_index`), :1428 and :1482 (where it is handed back); crates/llvmkit-ir/src/module_summary_index.rs (the model and its `Display`)

- **Was claimed:** The index comes back separately in `ParsedModule::summary_index`, is not attached to the `Module`, and `format!("{module}")` therefore never emits `^N`. Reproducing `llvm-dis` is two calls: print the module, then print the index.
- **Verdict:** NOT A DIVERGENCE — the llvmkit-side facts are true, but the upstream half of the claim is false, so this is parity, not a gap. Accurate as written: `ParsedModule::summary_index` (crates/llvmkit-asmparser/src/ll_parser.rs:421) returns the index separately, it is not attached to `Module`, `format!("{module}")` never emits `^N`, and reproducing `llvm-dis` is two calls. Wrong as written: "`llvm-dis` hands `AssemblyWriter` both the module and its index, and `printModuleSummaryIndex` runs after `printModule`, so one print emits the module followed by its `^N` entries." Upstream does exactly what llvmkit does. `AssemblyWriter` has two disjoint constructors — one takes `const Module *` and leaves `TheIndex` null, the other takes `const ModuleSummaryIndex *` and leaves `TheModule` null — and is never handed both. `Module::print` calls only `W.printModule(this)`; `printModuleSummaryIndex` is reached only from `ModuleSummaryIndex::print`, which builds its own second `SlotTracker` and its own second `AssemblyWriter`, and opens with `assert(TheIndex)`. `llvm-dis` makes two consecutive calls on the same stream (`M->print(...)` then `Index->print(...)`), as does `llvm-as`. `llvm::Module` owns no index at all (Module.h has only a forward declaration), and the parser entry point returns the pair `ParsedModuleAndIndex { Mod, Index }` — the same split shape llvmkit's `ParsedModule` mirrors. So the correct statement is: llvmkit's two-call print and separately-returned index MATCH upstream LLVM 22.1.4. The entry at docs/future-work.md:15-29 should be rewritten — its premise sentence ("`llvm-dis` prints a module and its summary index together because `AssemblyWriter` is handed both") is factually incorrect, and its closing suggestion that "attaching the index to the module" is an open design question is backwards: attaching it would be the divergence from upstream, not the fix.

<details><summary>Verification evidence</summary>

llvmkit side, confirmed present: crates/llvmkit-asmparser/src/ll_parser.rs:419-422 defines `pub struct ParsedModule<'ctx, B: ModuleBrand> { pub slot_mapping: SlotMapping<'ctx, B>, pub summary_index: Option<ModuleSummaryIndex> }`, filled by `self.summary_index.take()` at :1428 and :1482. crates/llvmkit-ir/src/module.rs:4685-4687 — `impl<B, S> Display for Module<B, S>` body is `crate::asm_writer::fmt_module(f, &self.core)`, with no index parameter; a grep for `ModuleSummaryIndex` across crates/llvmkit-ir hits only asm_writer.rs and module_summary_index.rs, never module.rs, so `ModuleCore` cannot hold one. Upstream, which refutes the claim: orig_cpp/.../llvm/tools/llvm-dis/llvm-dis.cpp:264-272 is `if (!DontPrint) { if (M) { M->removeDebugIntrinsicDeclarations(); M->print(Out->os(), Annotator.get(), false); } if (Index) Index->print(Out->os()); }` — two separate print calls, not one. lib/IR/AsmWriter.cpp:5077-5084 `Module::print` constructs `SlotTracker SlotTable(this)` and `AssemblyWriter W(OS, SlotTable, this, AAW, IsForDebug, ShouldPreserveUseListOrder)` then calls only `W.printModule(this)`. lib/IR/AsmWriter.cpp:5437-5442 `ModuleSummaryIndex::print` constructs a distinct `SlotTracker SlotTable(this)` and `AssemblyWriter W(OS, SlotTable, this, IsForDebug)` then calls `W.printModuleSummaryIndex()`. The two ctors at AsmWriter.cpp:2966 and :2982 initialize `TheModule` and `TheIndex` respectively and never both; `printModuleSummaryIndex` begins `assert(TheIndex)` (AsmWriter.cpp:3185-3186). include/llvm/IR/Module.h:51 has only `class ModuleSummaryIndex;` forward-declared. include/llvm/AsmParser/Parser.h:69-73 defines `ParsedModuleAndIndex` holding `std::unique_ptr<Module>` plus `std::unique_ptr<ModuleSummaryIndex> Index` — the split-pair return llvmkit mirrors. tools/llvm-as/llvm-as.cpp:156 likewise calls `Index->print(errs())` on its own. Source of the bad premise: docs/future-work.md:15-29, "Summary index — printing a module and its index is two calls (found 2026-08-15, LLParser parity W10)".

</details>

### ifunc is missing metadata attachments and three global properties

*IR model / parser — aliases and ifuncs* — crates/llvmkit-ir/src/global_ifunc.rs, crates/llvmkit-ir/src/global_alias.rs (the shape to mirror), crates/llvmkit-asmparser/src/ll_parser.rs (`parse_alias_or_ifunc`)

- **Was claimed:** `GlobalIfuncBuilder` has no `thread_local_mode` / `unnamed_addr` / `dll_storage_class` setters (the sibling `global_alias.rs` does), and ifunc metadata attachments are not stored.
- **Verdict:** CLOSED — the divergence no longer exists. All four items the claim calls missing are present in the tree today, and the parser and printer both wire them up. In `crates/llvmkit-ir/src/global_ifunc.rs`: - `GlobalIfuncBuilder::dll_storage_class` (line 402), `::thread_local_mode` (line 408), `::unnamed_addr` (line 414) — all three `#[must_use]` setters exist, with a doc comment citing `parseAliasOrIFunc` as the reason ifunc carries them. - `GlobalIfuncData` (lines 33-37) stores `dll_storage_class`, `thread_local_mode`, `unnamed_addr` as `Cell`s plus `metadata: RefCell<MetadataAttachmentSet<StoredBrand>>`; `into_data` (lines 473-486) threads all four through. - `GlobalIfunc` has the matching accessors/mutators (lines 192-248), including `metadata()`, `metadata_stored()` and `set_metadata()`. The shape now matches `global_alias.rs` field-for-field; both files are structurally parallel (513 vs 518 lines, same field list on the data struct and the builder). Two secondary details in the claim's framing are also worth correcting: - ifunc/alias are NOT symmetric on metadata upstream, so "the same set GlobalAlias carries" is imprecise. `parseAliasOrIFunc`'s property loop guards metadata with `} else if (!IsAlias && Lex.getKind() == lltok::MetadataVar)` — only an **ifunc** may carry attachments; an alias with `!dbg !0` is still the "unknown alias or ifunc property!" error. llvmkit reproduces exactly this at `ll_parser.rs:7136-7147`, including the bang in the message. - The three global properties are stored but deliberately NOT printed for an ifunc. Upstream `AssemblyWriter::printIFunc` emits linkage, `printDSOLocation`, `printVisibility`, then goes straight to `"ifunc "` — it never prints DLL storage class, thread-local, or unnamed_addr, even though `parseAliasOrIFunc` applies all of them to `GV` in both branches. `fmt_ifunc` (asm_writer.rs) mirrors that omission, and an in-source comment at ll_parser.rs:7197-7199 records the choice. Closed by commit e02815d (2026-08-14) "feat(ir,asmparser)!: ifunc properties, global redefinition, named-metadata operands (LLParser parity W7, part 5)", which added exactly these six accessor/setter pairs to global_ifunc.rs (+71 lines) plus parser support (+73 lines) and tests in parser_module_level.rs and parser_debug_metadata.rs.

<details><summary>Verification evidence</summary>

Read all 518 lines of C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/global_ifunc.rs and all 513 lines of C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/global_alias.rs side by side. The ifunc builder's three setters are at lines 402, 408, 414 (alias's at 397, 403, 409 — same shape); `GlobalIfuncData.metadata` is at line 37 and `GlobalIfunc::set_metadata` at line 239, identical to the alias at lines 34 and 236. So the claim's "GlobalIfuncBuilder has no thread_local_mode / unnamed_addr / dll_storage_class setters" and "ifunc metadata attachments are not stored" are both directly contradicted by the file at the cited path. Read C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs lines 7126-7230 (`parse_alias_or_ifunc`): the property loop pushes into `ifunc_metadata` on `!is_alias && Token::MetadataVar` (7136-7141), the ifunc branch chains `.dll_storage_class(...).thread_local_mode(...).unnamed_addr(...)` onto `ifunc_builder` (7200-7202), and applies the attachments via `i_view.set_metadata(...)` in a loop at 7211-7213. Confirmed against upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp `LLParser::parseAliasOrIFunc`: it sets `setThreadLocalMode` / `setVisibility` / `setDLLStorageClass` / `setUnnamedAddr` on `GV` after both the alias and ifunc `create` calls, and its attribute loop reads `} else if (!IsAlias && Lex.getKind() == lltok::MetadataVar) { parseGlobalObjectMetadataAttachment(*GI) }` with the fallback `tokError("unknown alias or ifunc property!")` — llvmkit matches all three. Confirmed the printer contract against `AssemblyWriter::printIFunc` in orig_cpp/.../llvm/lib/IR/AsmWriter.cpp: linkage → printDSOLocation → printVisibility → `"ifunc "` → value type → resolver → partition → `GI->getAllMetadata(MDs)` / `printMetadataAttachments(MDs, ", ")`. llvmkit's `fmt_ifunc` in crates/llvmkit-ir/src/asm_writer.rs (line 3778 onward) emits that same sequence, ending with `fmt_metadata_attachments(f, &i.metadata_stored(), ..., ", ")`. Git history settles the timing: `git log -- crates/llvmkit-ir/src/global_ifunc.rs` shows e02815d (2026-08-14) as the most recent commit, and `git show e02815d` confirms it introduced the `dll_storage_class` / `thread_local_mode` / `unnamed_addr` accessors and builder setters. The file mtime (Aug 14) is newer than global_alias.rs (Aug 9), consistent with the ifunc side being brought up to the alias's shape after the claim was recorded.

</details>

### The summary index is not attached to the module, so `format!("{module}")` never prints `^N`

*printer / API surface* — crates/llvmkit-ir/src/module_summary_index.rs, crates/llvmkit-ir/src/asm_writer.rs, crates/llvmkit-asmparser/src/parser.rs (`ParsedModule::summary_index`)

- **Was claimed:** The index comes back in `ParsedModule::summary_index`, is not attached to the `Module`, and a caller reproducing `llvm-dis` prints the module and then the index. `Display for ModuleSummaryIndex` reproduces `printModuleSummaryIndex` exactly, leading blank line included.
- **Verdict:** The llvmkit-side description is accurate, but it is NOT a divergence — it is exact parity, because the "Upstream does" half of the claim is factually wrong. Upstream does not hand one `AssemblyWriter` both a module and an index, and `printModuleSummaryIndex` is not run "after `printModule`" in one print call. The two are mutually exclusive by construction: `AssemblyWriter` has two distinct constructors, one taking `const Module *` (sets `TheModule`) and one taking `const ModuleSummaryIndex *` (sets `TheIndex`, and passes `TypePrinter(/*Module=*/nullptr)`), so a single writer never holds both. `Module::print` calls `W.printModule(this)` and nothing else; `ModuleSummaryIndex::print` builds its own separate `AssemblyWriter` and calls `W.printModuleSummaryIndex()`. `llvm-dis` itself makes two calls — `M->print(Out->os(), Annotator.get(), /*ShouldPreserveUseListOrder*/ false);` and then, separately, `if (Index) Index->print(Out->os());`. Likewise, upstream's parse-both interface returns `ParsedModuleAndIndex { std::unique_ptr<Module> Mod; std::unique_ptr<ModuleSummaryIndex> Index; }` — the index is a separate owned object, never attached to the `Module`. So llvmkit's shape (index in `ParsedModule::summary_index`, not on `Module`; `Display for Module` printing no `^N`; a separate `Display for ModuleSummaryIndex` carrying the leading blank line; caller prints module then index to reproduce `llvm-dis`) mirrors upstream one-for-one. Upstream's own `Module::print` also never emits `^N`. The correct statement is: "llvmkit splits module printing and summary-index printing exactly as upstream does, including the index's leading blank line and the two-call `llvm-dis` reproduction." This is the design already recorded in CLAUDE.md as intentional parity, not a gap.

<details><summary>Verification evidence</summary>

llvmkit side (all still as described): C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/module.rs:4679 — `impl<B: ModuleBrand, S> core::fmt::Display for Module<B, S>` whose body is only `crate::asm_writer::fmt_module(f, &self.core)`; a grep for "summary" across module.rs returns exactly one hit (line 1636, an unrelated doc comment), so `ModuleCore` has no index field. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/asm_writer.rs:3168 `fmt_module` -> :3175 `fmt_module_with_options`, which I read to its close at :3387 (`Ok(())` after the named-metadata loop) — it never emits `^N` or touches an index. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/asm_writer.rs:4416 `impl fmt::Display for ModuleSummaryIndex`, documented "Mirrors `AssemblyWriter::printModuleSummaryIndex`, including its leading blank line", with `writeln!(f)?;` at :4421 matching upstream's `Out << "\n";`. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:421 `pub summary_index: Option<ModuleSummaryIndex>` on `ParsedModule`; C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/parser.rs:252-260 `parse_assembly_with_index`. Upstream side (what refutes the claim's premise): C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/AsmWriter.cpp:2966 and :2982 — the two mutually exclusive `AssemblyWriter` constructors (`const Module *M` vs `const ModuleSummaryIndex *Index` with `TypePrinter(/*Module=*/nullptr)`); :5078-5083 `Module::print` ends in `W.printModule(this);` alone; :5438-5443 `ModuleSummaryIndex::print` constructs its own `SlotTracker`/`AssemblyWriter` and calls `W.printModuleSummaryIndex();`; :3185 `printModuleSummaryIndex` opens with `assert(TheIndex);` then `Out << "\n";`. C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/tools/llvm-dis/llvm-dis.cpp:265-271, under the comment "All that llvm-dis does is write the assembly to a file": `M->print(Out->os(), Annotator.get(), /* ShouldPreserveUseListOrder */ false);` followed by a separate `if (Index) Index->print(Out->os());`. C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/AsmParser/Parser.h:71-75 `struct ParsedModuleAndIndex { std::unique_ptr<Module> Mod; std::unique_ptr<ModuleSummaryIndex> Index; };`.

</details>

---

## How to use this file

- Adding a row: verify it first, cite the upstream symbol, and give a fix
  sketch concrete enough to act on.
- Closing a row: delete it, and say in the commit which fixture now passes.
- A row with no fixture behind it is a hypothesis; say so in the entry.
