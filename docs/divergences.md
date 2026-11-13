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

Entry prose cites **by symbol, never by line number** — upstream and our own
tree alike (repo law: an upstream coordinate rots when the vendored tree moves,
and one into our own files rots by the next refactor).
`crates/llvmkit-ir/tests/upstream_registry_drift.rs::no_live_doc_page_cites_a_line_number`
keeps it so. Its evidence-block counter used to open on any line containing
`<details`, so this preamble's prose mention of the block opened a block
nothing closed and the rest of the file went unchecked; it now opens only on a
real `<details>` tag at the start of a line, which
`upstream_registry_drift.rs::a_prose_mention_of_details_does_not_silence_the_rest_of_a_page`
pins.

The `Correction from verification` and `<details>` blocks are **deliberately
exempt**, and that is not a backlog item. They are dated snapshots of one
verification pass, so their coordinates are the *record of what was checked*,
not a claim about where something lives; rewriting them would be falsifying the
record. Treat those coordinates as valid only against the vendored 22.1.4 tree
and against the commit the block was written at, and re-derive the symbol before
quoting one anywhere else. The guard exempts both: a `<details>` block until its
`</details>`, and a `Correction from verification` paragraph, which is one
bullet line starting `- **Correction from verification`. Do not "clean up"
an evidence block to make a test pass.

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
two lists stay **separate**: `operand_ids()` is what the crate-internal
consumers walk, and they assume an operand is a first-class SSA value.

**Consequence:** `User::operand_count` on a conditional `br` answers 1 where
upstream answers 3, and `User::operand(i)` indices do not match
`Use::getOperandNo()`. Nothing in the tree reads those indices for a
terminator today.

**Fix:** merge the two, and re-check the index-keyed consumers plus every
site that assumes `operand(i)` is first-class. Upstream's true order is
`br` = `[cond, else, then]`, `switch` = `[cond, default, (val, dest)*]`,
`invoke` = `[args…, normal, unwind, callee]`.

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
registered against the wrapper would collapse to a single use.

**The use list is fixed.** Every edge is added, removed and moved through
`Context`'s use-list doors in `llvm_context.rs` (`add_use`, `remove_use`,
`prepend_moved_uses`, `use_list_of`), which read the wrapper through
`use_edge_target` to its global, and `Value`'s use-list queries read the same
way. So a global has one list, as upstream's has: a global field, an
instruction operand, a constant-expression operand, a metadata node and a
debug record all land on it, and a wrapper's own list stays empty.
`uselistorder ptr @f` covers a function's calls, `ptr @f` operands,
initializers and constant expressions
(`crates/llvmkit-asmparser/tests/parser_use_list.rs::a_function_called_and_named_by_constants_orders_every_use`),
and the Verifier's port of `F.hasAddressTaken` (`function_address_is_taken`)
sees a call that passes an intrinsic as an argument
(`crates/llvmkit-ir/tests/verifier_basic.rs::intrinsic_declaration_used_as_non_callee_operand_is_rejected`).
A cell may still hold the wrapper, so a reader asking "is this operand that
global" compares through `use_edge_target`.

**Still open — the handles' narrowing does not read through the wrapper.**
Upstream's `dyn_cast<GlobalVariable>(LI->getPointerOperand())` on
`load i32, ptr @g` answers `@g`. llvmkit's `GlobalVariable::try_from` of that
operand — the interned wrapper — answers
`IrError::ValueCategoryMismatch { expected: GlobalVariable, got: Constant }`
(found 2026-10-06 while asserting capability routes at the capability-plan
integration). The analyses each unwrap by hand (`global_value_or_self` in
`pointer_analysis.rs`, `global_value_name_and_type` in `eh_personalities.rs`,
the `GlobalValueRef` arms in `constant_fold.rs` and `constant_folding.rs`);
the public `TryFrom<Value>` impls of `GlobalVariable`, `GlobalAlias`,
`GlobalIfunc` and `FunctionValue` match only the global's own kind. Fix: those
impls unwrap a `GlobalValueRef` whose referent is their kind, as D3 says the
wrapper *is* the global. The same narrowing, the other way round, refused a
parse: `Constant::try_from` of a global variable fails, so the parser's
`landingpad` clause check, its port of `isa<Constant>`, refused `catch ptr @g`
as `clause argument must be a constant`. That check now asks about a global's
own value category too
(`crates/llvmkit-asmparser/tests/parser_use_list.rs::a_directive_on_an_invoked_function_parses`);
`Constant::try_from` itself still matches only a `Constant` kind.

### D4 — Two uses by the same user are indistinguishable

**Severity:** wrong-output (narrow)
**Where:** `crates/llvmkit-ir/src/value.rs` — `ValueUse::Instruction`

**LLVM:** a `Use` is identified by `(user, operand number)`, and
`AsmWriter::predictValueUseListOrder` breaks ties between two uses by the same
user on `Use::getOperandNo()`.

**llvmkit:** `ValueUse::Instruction(user)` records only the user, so
`add i32 %x, %x` produces two identical edges, and `ValueUse::Constant` is the
same. For those the tie-break has no counterpart; the comparator treats them
as equal and the stable sort leaves their relative order alone. A
`ValueUse::GlobalField` edge names its cell, so `ValueUse::operand_number`
answers for it and the tie-break between one function's personality, prefix
and prologue is upstream's
(`crates/llvmkit-asmparser/tests/parser_use_list.rs::function_operand_uses_survive_a_reversal_round_trip`).

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

**Guarded by:** nothing that can see it. The `parser_use_list.rs` round trips
assert that print, re-parse, print gives the same text, and the printer prints
two orders of this kind alike, so they pass whichever order the parse built.
`verify-uselistorder`'s own check compares each value's use list, `(user,
operand number)` by `(user, operand number)`, which needs a view of a use list
with operand numbers: the index this fix adds.

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

**Lookups that must resolve the groups themselves.** With no merge, every
reader of a function-index attribute has to consult the group numbers too, as
`FunctionValue::function_string_attribute` does. On 2026-09-19 (Task 24 fix
round 3) the three `CallBase::hasFnAttr` copies and the two
`Function::hasFnAttribute` copies (`speculation.rs::callee_is_speculatable`,
`assumptions.rs::enclosing_function_has_attribute`) that did not were
replaced by one port that does (`instr_types::call_site_has_fn_attr`, renamed
`call_site_has_function_attribute` in Task 1b of the crash-proofing plan,
`FunctionValue::has_fn_attribute`). One reader still skips the groups. It
reads a *payload*, so the fix is a port of `getFnAttribute`, not of
`hasFnAttr`:

- `value_tracking.rs::function_vscale_range` (`llvm::getVScaleRange`, via
  `F->getFnAttribute(Attribute::VScaleRange)`) reads the stored list only.

It falls back to upstream's own answer for a missing attribute (no range), so
the direction is conservative: an analysis learns less, never something
false. A second such reader, `speculation.rs::call_site_memory_effects`, was
replaced in Task 1b's second fix round of the crash-proofing plan by
`CallBaseParts::memory_effects` in `instr_types.rs`, which reads each
`memory(...)` through `merged_function_memory_effects`: the last named group
carrying one, else the inline one, as `validateEndOfModule`'s `FnAttrs.merge(B)`
leaves it
(`crates/llvmkit-asmparser/tests/speculation.rs::a_memory_attribute_in_an_attribute_group_counts`).
Found with
`rg -n "\.attributes\.borrow\(\)|function_attrs\(\)" crates/llvmkit-ir/src/`
on the commit that adds this paragraph (17 hits), reading each: besides these
two, the hits read a parameter or return index, which a `#N` group cannot
carry, resolve the groups themselves (`function_string_attribute`, the
verifier's `function_attrs_with_groups`), or are the printer.

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
| `upgrade_debug_info` | **selects nothing.** llvmkit's `AutoUpgrade` port is partial (inventory entry 19) and `llvm::UpgradeDebugInfo` is not among the ported call sites, so there is no `UpgradeDebugInfo` call for the flag to guard. |

**Consequence:** a caller that sets `upgrade_debug_info: false` (what `llvm-as`
and `opt -disable-upgrade-debug-info` do) gets the same module as one that
leaves it `true`. Nothing is silently *wrong* — llvmkit never upgrades debug
info — but the setting is a promise about a future behaviour rather than a
live switch, and the rustdoc says so.

**Also:** the standalone `parse_type*` / `parse_constant_value*` family and
`parse_summary_index_assembly*` correctly take no `*_with_config` twin: their
upstream counterparts do not either. (An earlier revision of this note flagged
the file-path convenience forms, `parse_file_branded` and `parse_file_dynamic`,
as the same kind of correct exception. Both are gone now — the parser reads no
files at all — so there is no longer a file-path form to except.)

**Fix:** lands with the `llvm::UpgradeDebugInfo` port. The flag already
reaches `Parser::parse_module_with_config`; only the guarded call is missing.

*(A second entry — "No `ParserConfig`: `AllowIncompleteIR`, the DataLayout
callback and the UpgradeDebugInfo flag are unmodelled" — sat in the model-gap
band recording that none of this existed. `ParserConfig` has since landed;
that entry was retired into this one on 2026-08-29 rather than left to be
closed twice.)*

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
`eraseMetadataIf`, and outside a pass context an `InstructionView` cannot be
erased from its block (`Instruction::erase_from_parent` takes the linear
`Instruction<Attached>` handle; `FnPatch::erase` takes a view's
`NonTerminator` but needs a pass's mutation context). Closing this is
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

## Numeric predicates

### D17 — Four zero-width predicates answer where upstream asserts

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/ap_int.rs` — `ApInt::is_max_signed_value`,
`is_min_signed_value`, `is_power_of_2`, `is_negated_power_of_2`

**LLVM:** `llvm/include/llvm/ADT/APInt.h::isMaxSignedValue` and
`isMinSignedValue` each assert a non-zero width, guarding a real
`1 << (BitWidth - 1)` shift that would otherwise shift by `-1`.
`isNegatedPowerOf2`'s assert is its first statement, unconditional, guarding
the `BitWidth - 1` bit index its body reaches through `isNonNegative()`.
`isPowerOf2`'s assert sits inside the `isSingleWord()` branch and guards
nothing mechanical — `isPowerOf2_64(U.VAL)` is `has_single_bit` over the
stored word (`llvm/include/llvm/Support/MathExtras.h`) and never reads
`BitWidth`; that assert is purely semantic, declining the question at width 0.
In a release build (`NDEBUG`), three of the four are undefined behaviour at
width 0 — the signed extremes shift by an out-of-range amount, and
`isNegatedPowerOf2` indexes an out-of-range bit. `isPowerOf2` is not: at
width 0 it computes `has_single_bit(0) == false`, a fully defined answer.

**llvmkit:** computes no such shift and answers instead — `true`, `true`,
`false`, `false`. `is_max_signed_value` and `is_min_signed_value` fall through
their `bit_width == 0 ||` short-circuit; `is_power_of_2` is
`popcount() == 1`, which is `false` when no bits are set; `is_negated_power_of_2`
is `is_negative() && ...`, and `is_negative` itself already guards
`bit_width != 0` at the exact point where upstream's index would go out of
range.

**Why:** llvmkit takes no runtime panics in production paths, so it must
define what upstream leaves undefined. The two signed-extreme answers match
upstream's own unasserted *unsigned* pair — `isMaxValue`/`isMinValue` are
`isAllOnes`/`isZero`, both `true` at width 0 — because a 0-bit domain holds
exactly one value, which is both its maximum and its minimum. The two
power-of-two answers are `false` because a 0-bit value has no bits set, and
`popcount() == 1` already states that on its own.

**Fix:** none owed. llvmkit is strictly more total than upstream here — it
answers every input upstream's assert would instead reject or leave
undefined — and no input makes any of the four answers wrong. This entry
records the difference; it does not schedule work.

---

## Type constructors

### D18 — An array constant of a non-array element is refused where upstream asserts

**Severity:** wrong-message, against a release build of upstream; an
assertions build aborts instead.
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` — the `Token::LSquare`
arm of the value parser, which ports `LLParser::parseValID`'s `lsquare` arm.

**LLVM:** the `lsquare` arm refuses a first element that is not
`isFirstClassType` with `invalid array element type: <ty>` at the first
element, then calls `ArrayType::get(Elts[0]->getType(), Elts.size())`, whose
first statement asserts `isValidElementType`, and only then checks each element
against element 0. `Type::isFirstClassType` (`lib/IR/Type.cpp`) is false only
for function types, `void` and opaque structs, so `x86_amx` and `token` pass the
guard while `ArrayType::isValidElementType` refuses both — `[x86_amx undef,
x86_amx undef]` and `[token none, token none]` reach the assert. An assertions
build aborts there. A release build makes `[2 x x86_amx]`, the agreement loop
passes, and `convertValIDToValue`'s `t_Constant` arm reports `constant
expression type mismatch: got type '[2 x x86_amx]' but expected '…'` at the
constant, because no demanded type can be an array that no `.ll` spells
(derived from the source; no assertions-off LLVM was run).

**llvmkit:** refuses where the assert fires — after the first-class guard,
before the agreement loop — with the guard's own diagnostic at the same
anchor: `invalid array element type: x86_amx` at the first element.
`ModuleRef::array_type` refuses the element with
`IrError::InvalidTypeComponent`, and the arm reports that refusal as the
guard's message. So `[x86_amx undef, i32 1]` is refused for its element type
too, where a release build of upstream reports the loop's `array element #1 is
not of type 'x86_amx`.

**Why:** llvmkit's array constructor cannot build `[2 x x86_amx]`, a type that
no parser reads back (Doctrine D10, `README.md`), and llvmkit takes no crash in
place of an assert.
The arm must refuse, and the guard's message at the guard's anchor is the
nearest diagnostic upstream has for an element no array may hold.

**Fix:** none owed — this is hardening of an assert, as D17 is. The entry
records the message, which no upstream build prints for this input.

**Guarded by:**
`crates/llvmkit-asmparser/tests/type_validity.rs::an_array_constant_of_a_non_array_element_is_refused_at_its_first_element`.

### D19 — The struct constructors refuse an element `StructType::get` accepts

**Severity:** model-gap — chosen by reading the table above: no grade fits an
API call, since the grades name *input*. It is not **rejects-valid**: no `.ll`
text LLVM accepts is refused, because `LLParser::parseStructBody` refuses the
same elements. The public constructor answers differently from its LLVM
counterpart, which is what **model-gap** names.
**Where:** `crates/llvmkit-ir/src/module.rs` — `struct_element_slots`, behind
`Module::struct_type` / `packed_struct_type` (and the same two on
`ModuleView`), `Module::set_struct_body` and `Module::set_struct_body_dyn`.

**LLVM:** `StructType::get` (`lib/IR/Type.cpp`) interns the body through
`StructType::setBody`, which is `cantFail(setBodyOrError(...))`;
`StructType::setBodyOrError` runs `StructType::checkBody`, which refuses a
recursive body and nothing else. `StructType::isValidElementType` — `void`,
`label`, `metadata`, a function type and `token` refused — is called only by
`LLParser::parseStructBody`, at its two element sites (`invalid element type
for struct`). The C API forwards (`LLVMStructTypeInContext` to
`StructType::get`, `LLVMStructSetBody` to `setBody`), so a module built through
either can hold `{ label }`, print it, and not read it back.

**llvmkit:** refuses such an element at construction with
`IrError::InvalidTypeComponent { position: TypePosition::StructElement { index
}, kind }` (`invalid element type for struct (element N): <kind>`), after
refusing a type from another module with `IrError::ForeignType`. `x86_amx` is a
struct element, as upstream's predicate says. The parser checks each element
first, at its own location, so its diagnostics are unchanged. The intrinsic
demangler follows the same rule: a literal-struct overload of such an element
(`sl_isVoids`) is a signature mismatch, and a named-struct overload (`s_<name>s`)
names a struct the module must already hold rather than creating one.

**Why:** a type no parser — llvmkit's or LLVM's — reads back is what Doctrine
D10 forbids building. `lib/IR/Verifier.cpp`'s header states the rule ("It
should be illegal to put a label into any other type (like a structure)"), and
MLIR's LLVM dialect enforces it at construction:
`LLVMStructType::verifyInvariants` (`mlir/lib/Dialect/LLVMIR/IR/LLVMTypes.cpp`)
refuses every element `LLVMStructType::isValidElementType` refuses, the same
five kinds.

**Fix:** none — deliberate. Revisit if upstream adds the check to
`StructType::get`; an LLVM patch proposing it is being drafted (the
type-constructor plan's Task 9).

**Guarded by:** `crates/llvmkit-ir/tests/type_components.rs::a_struct_refuses_an_invalid_element`,
`crates/llvmkit-ir/tests/cross_module_handles.rs::the_struct_constructors_reject_an_element_from_another_module`,
`crates/llvmkit-ir/tests/type_components.rs::an_intrinsic_name_cannot_spell_an_invalid_struct`,
`crates/llvmkit-ir/tests/type_components.rs::demangling_a_struct_name_creates_no_struct`,
and the agreement law
`crates/llvmkit-asmparser/tests/type_validity.rs::the_builder_and_the_parser_agree_on_every_derived_type`,
whose struct rows left its list of cells allowed to disagree with this change
(the list was deleted once the function-type rows left it too).

<details>
<summary>Evidence (2026-10-18, vendored llvmorg-22.1.4)</summary>

- `rg -n "StructType::isValidElementType" llvm/lib llvm/include` → the
  definition in `lib/IR/Type.cpp` and two calls in `lib/AsmParser/LLParser.cpp`
  (`parseStructBody`); no call on the `StructType::get` / `setBody` path.
- `StructType::checkBody` walks the element worklist and returns an error only
  when it meets the struct itself (`identified structure type '…' is
  recursive`).
- `rg -n "LLVMStructTypeInContext|LLVMStructSetBody" -A6 llvm/lib/IR/Core.cpp`
  → `StructType::get(*unwrap(C), Tys, Packed != 0)` and
  `unwrap<StructType>(StructTy)->setBody(Tys, Packed != 0)`, unchecked.

</details>

### D20 — A typed pointer is no IR type, so no derived type holds one

**Severity:** model-gap — no `.ll` text reaches it: the parser lowers a typed
pointer's spelling (`i32*`) to an opaque `ptr`, and no textual syntax builds a
`TypedPointerType`.
**Where:** `crates/llvmkit-ir/src/typed_pointer_type.rs` (`TypedPointerType`,
`TypedPointee`, `TypedPointerElement`), `crates/llvmkit-ir/src/llvm_context.rs`
(the typed-pointer table beside the type arena).

**LLVM:** `TypedPointerType` is a `Type` subclass (`TypedPointerTyID`) that the
DXIL and SPIR-V backends build in their analyses
(`lib/Target/DirectX/DirectXIRPasses/PointerTypeAnalysis.cpp`,
`lib/Target/SPIRV/SPIRVGlobalRegistry.cpp`, among others), and derived types may
contain one: `VectorType::isValidElementType` admits `TypedPointerTyID`, and
`ArrayType`, `StructType` and `FunctionType` do not exclude it. Its header says
"It is not legal to use this type, or derived types containing this type, in
LLVM IR", and `Value::Value`'s `checkType` asserts "Cannot have values with
typed pointer types" — both rules unchecked in a release build.

**llvmkit:** a typed pointer is not an IR type at all. `TypedPointerType` does
not implement `IrType` and has no route to `Type`, and it is interned in a
table of its own, so no `Type`, no value and no vector, array, struct or
function type can hold one; `TypedPointerType::of` builds one from a valid
pointee (a typed pointer included) and cannot fail, and
`Module::typed_pointer_type` checks a runtime `Type` pointee
(`TypedPointerType::isValidElementType`, `IrError::InvalidTypeComponent`) after
its module (`IrError::ForeignType`). It prints as `TypePrinting::print` prints
one, `typedptr(<pointee>, <addrspace>)`.

**Why:** llvmkit has no backends (code generation is out of scope), and the
upstream header calls these types illegal in IR. The old model was a `Type`
variant: it printed `i32*`, which re-parses as an opaque `ptr` — a different
type — and `AnyTypeEnum::from` of one built a `PointerType` over the
typed-pointer slot, whose `address_space` `expect`s a `Pointer` payload and
would have panicked (derived by reading `derived_types.rs` at `8e8292c`, not
run).

**Fix:** none — deliberate. Reopen with the first DXIL or SPIR-V analysis that
needs typed derived types, as an analysis-side type family rather than IR
types.

**Guarded by:** `crates/llvmkit-ir/tests/type_components.rs::upstream_typed_pointer_prints`,
`crates/llvmkit-ir/tests/type_components.rs::a_typed_pointer_nests_and_keeps_its_address_space`,
`crates/llvmkit-ir/tests/type_components.rs::an_erased_typed_pointer_refuses_an_invalid_pointee`,
`crates/llvmkit-ir/tests/cross_module_handles.rs::the_typed_pointer_constructor_rejects_a_pointee_from_another_module`,
and `crates/llvmkit-ir/tests/compile_fail/typed_pointer_is_not_an_ir_type.rs`
(a typed pointer as a function parameter fails on `IrType`'s own message).

<details>
<summary>Evidence (2026-10-29, vendored llvmorg-22.1.4, tree over `8e8292c`)</summary>

- `rg -n "not legal to use this type" llvm/include/llvm/IR/TypedPointerType.h`
  → the class comment; `rg -n "Cannot have values with typed pointer types"
  llvm/lib/IR/Value.cpp` → `checkType`'s assert.
- `rg -n -A6 "^bool VectorType::isValidElementType" llvm/lib/IR/Type.cpp` →
  `ElemTy->getTypeID() == TypedPointerTyID` among the admitted kinds.
- `rg -l "TypedPointerType::get\(" llvm/lib/Target` → five files under
  `DirectX/` and `SPIRV/`.
- rust-analyzer findReferences before the change: `TypeData::TypedPointer` 20,
  `TypeKind::TypedPointer` 6, `TypeKindLabel::TypedPointer` 4,
  `Type::is_typed_pointer` 2, `Module::typed_pointer_type` 3 (each count
  including the declaration); every arm was deleted with its variant.

</details>

### D21 — A malformed target type is refused on every request, not only the first

**Severity:** model-gap — API only: the parser stops at the first refusal, so
no `.ll` text asks twice.
**Where:** `crates/llvmkit-ir/src/module.rs` — `ModuleRef::target_ext_type`,
behind `Module::target_ext_type` / `ModuleView::target_ext_type`.

**LLVM:** `TargetExtType::getOrError` (`lib/IR/Type.cpp`) interns the type
first and runs `TargetExtType::checkParams` only when the lookup inserted it;
on a hit — "The target type was found. Just return it." — it answers the
interned type unchecked. So a second `getOrError` for the same malformed
`target("aarch64.svcount", i32)` succeeds where the first failed, and the
malformed type stays in the context.

**llvmkit:** checks the counts (`TargetExtParameterRule::broken_by`, the
branches of `checkParams` in order) before interning, so a malformed target
type never enters the arena and every request is refused with
`IrError::TargetExtTypeParameters`.

**Why:** a type no parser reads back is what D10 forbids holding; refusing
before interning is the only order that keeps none in the arena.

**Fix:** none — deliberate.

**Guarded by:** `crates/llvmkit-ir/tests/type_components.rs::a_target_type_checks_its_parameters_when_built`
and `crates/llvmkit-ir/tests/type_components.rs::an_intrinsic_name_cannot_spell_an_invalid_target_type`
(the intrinsic demangler applies the same rule).

### D22 — A target type `getTargetTypeInfo` can only assert on has no layout

**Severity:** model-gap — such a type parses, in llvmkit and in LLVM, because
`TargetExtType::checkParams` looks at no parameter beyond three names' counts.
What differs is the answer to a query about it.
**Where:** `crates/llvmkit-ir/src/data_layout.rs` — `target_ext_layout_type`,
read by `Type::is_sized`, `DataLayout`'s size and alignment queries, and
`TargetExtType::has_property`.

**LLVM:** `getTargetTypeInfo` (`lib/IR/Type.cpp`) builds the layout type and the
properties together, so `hasProperty`, `getLayoutType` and `isSized` all pass
through it. It asserts on five inputs: a `spirv.Type` without exactly three
integer parameters; a `spirv.Padding` or `dx.Padding` without its size
(`TargetExtType::getIntParameter`'s index assert); a `riscv.vector.tuple` whose
first type parameter is no scalable vector (`cast<ScalableVectorType>`) or whose
count is zero (`ScalableVectorType::get`'s `MinNumElts > 0`); and a `spirv.Type`
whose alignment `IntegerType::get` refuses as wider than `MAX_INT_BITS`. An
assertions build aborts on the first query. What a release build answers
depends on the arm, and some arms read past the parameter array.

**llvmkit:** `target_ext_layout_type` answers `None` for all five, so the type
is unsized, `DataLayout` sizes it as 0 bits with an alignment of 1, and
`has_property` answers `false` for every property.

**Why:** a query that reads a malformed type must not abort or read out of
bounds (Doctrine D10), and each of these layout types is one llvmkit cannot
build. `checkParams` cannot refuse them instead: `llvm-as` accepts their text,
so a refusal at construction would make `.ll` that LLVM reads unreadable here.

**Fix:** none — this is hardening of an assert, as D17 and D18 are.

**Guarded by:** `crates/llvmkit-ir/tests/type_components.rs::a_target_type_upstream_asserts_on_has_no_layout`,
`crates/llvmkit-ir/tests/type_components.rs::a_zero_count_riscv_tuple_has_no_layout`.

### D23 — A struct a schema holds cannot be renamed

**Severity:** model-gap, by D19's reading of the table: the grades name *input*
and this is an API call. No `.ll` text is involved, since `LLParser` never
renames a struct it has named.
**Where:** `crates/llvmkit-ir/src/module.rs` — `Module::set_struct_name`, on the
`schema_held` mark `Context::hold_schema_struct` sets. Its doors are
`ModuleRef::mint_struct_of`, `SchemaValue::try_new`, `IrBuilder::extract_field` and
both `try_from_function`s in `crates/llvmkit-ir/src/function_signature.rs`.

**LLVM:** `StructType::setName` (`lib/IR/Type.cpp`) renames any identified
struct. LLVM has no schemas, so nothing rests on the name.

**llvmkit:** refuses with `IrError::SchemaStructNameNotSettable` once a schema
has minted the struct, or a whole check has confirmed a value, a typed signature
or a field read against it, along with every struct it holds by value. A check
that fails holds nothing, at any depth: `FieldLayout::matches_type` only reads,
and each door marks after its own last check has passed.

**Why:** a `StructOf` or a `SchemaValue` proves its value is `%S`, and the
builds that take no `?` rest on that proof (Doctrine D10). A rename moved the
struct out from under the name: `rect.build` then wrote an `%Old` into a `%Point`
field, reaching `fold_field_insert`'s `unreachable!` through `ConstantFolder` and
an invalid `insertvalue` through `NoFolder`. Checking at the panic would leave
the proof false.

**A read-only check marks too, and that overrides Doctrine D8 here.** D8 says a
read-only route does not mutate, and this mark does change what a later
`set_struct_name` answers. It stands because a proof made at `ReadOnly` reaches a
mutable build by two routes, so an unmarked struct could be renamed under it:
- A schema's value is a `FieldArg` at any capability, and `IrBuilder::read_field`
  and `write_field` take a `SchemaValue` at any capability, so a `ReadOnly`
  `PointValue` from `Module::as_view` goes straight into `rect.build`.
- `with_typed_signature` answers with a `TypedFunctionId`, which carries no
  capability, so a check on a verified module reaches a mutable view through
  `Module::unverify`.
The mark is no IR: it prints nowhere, and a verified module stays verified.

**Fix:** none owed. Removing it needs a proof that does not rest on the name.

**Guarded by:** `crates/llvmkit-ir/tests/derived_struct_schema.rs::renaming_a_schema_struct_is_refused_with_constant_folder`,
`crates/llvmkit-ir/tests/derived_struct_schema.rs::renaming_a_schema_struct_is_refused_with_no_folder`,
`crates/llvmkit-ir/tests/derived_struct_schema.rs::a_nested_schema_struct_keeps_its_name`,
`crates/llvmkit-ir/tests/derived_struct_schema.rs::a_confirmed_struct_keeps_its_name_and_so_do_its_fields`,
`crates/llvmkit-ir/tests/derived_struct_schema.rs::a_struct_a_schema_value_checked_keeps_its_name`,
`crates/llvmkit-ir/tests/derived_struct_schema.rs::a_struct_a_typed_signature_checked_keeps_its_name`,
`crates/llvmkit-ir/tests/derived_struct_schema.rs::a_struct_no_schema_holds_can_be_renamed`,
`crates/llvmkit-ir/tests/derived_struct_schema.rs::a_read_only_proof_reaches_a_build_so_its_check_holds`,
`crates/llvmkit-ir/tests/struct_schema.rs::a_refused_check_holds_no_struct`,
`crates/llvmkit-ir/tests/struct_schema.rs::a_refused_signature_holds_no_struct`,
`crates/llvmkit-ir/tests/struct_schema.rs::a_passing_signature_holds_every_struct_it_names`,
`crates/llvmkit-ir/tests/struct_schema.rs::a_checked_field_read_holds_the_field_alone`.

### D24 — A reader of a size that upstream cannot take on a scalable one declines

**Severity:** model-gap, by D19's reading of the table: the query answers, and
upstream's stops. No build of upstream answers: the `TypeSize` conversion below
is a `reportFatalInternalError`, not an `assert`.
**Where:** `crates/llvmkit-ir/src/constant_folding.rs` — `gep_index_for_offset` and
`read_struct_data_from_global`; `crates/llvmkit-ir/src/value_tracking.rs` —
`gep_known_bits_from_values`; `crates/llvmkit-ir/src/constant_fold.rs` —
`global_pointer_alignment`; `crates/llvmkit-ir/src/pointer_analysis.rs` —
`constant_data_array_info`.

**LLVM:** `TypeSize::getFixedValue` asserts on a scalable size, and the implicit
`uint64_t` conversion (`TypeSize::operator ScalarTy`) reports a fatal error. Each
of these routines reads a size one of those two ways, and each can meet a scalable
one. `DataLayout::getGEPIndexForOffset` compares an offset with
`getSizeInBytes`, on a constant struct of scalable vectors.
`computeKnownBitsFromOperator` assigns `getElementOffset` to a `uint64_t`, on a
field of such a struct inside an array, which `Verifier::visitGetElementPtrInst`
does not refuse because the source is not itself a struct.
`DataLayout::getPreferredAlign` compares `getTypeSizeInBits` with 128, and
`getConstantDataArrayInfo` reads a null initializer's size with `getFixedValue`,
both on a scalable global, which the verifier refuses but the parser reads.
`ReadDataFromGlobal` reads `getTypeAllocSize` on a scalable field of a struct
whose layout size is fixed, which `{ i32, [0 x <vscale x 4 x i32>] }` has, past a
load that crosses into it; that struct is unsized (`StructType::isSized` refuses a
field that holds a scalable vector), so `ConstantFoldLoadFromConst`'s own
`getTypeAllocSize` of it asserts `isSized` before `ReadDataFromGlobal` is reached.

**llvmkit:** each takes the decline its routine already has: no index, unknown
bits, no alignment, no slice, no bytes. The unsized struct's case is D43's: its
size queries answer `None`, and no load from it is folded.

**Why:** a query must not abort (Doctrine D10).

**Fix:** none owed — this is hardening of a fatal error, as D17 and D18 are of
asserts.

**Guarded by:**
`crates/llvmkit-ir/tests/constant_folding_analysis.rs::a_load_at_an_offset_into_a_scalable_struct_is_not_folded`,
`crates/llvmkit-asmparser/tests/value_tracking_gep.rs::a_scalable_field_offset_is_unknown`,
`crates/llvmkit-asmparser/tests/pointer_analysis.rs::a_scalable_global_has_no_fixed_size_to_read`.

### D25 — A forward `dso_local_equivalent` that resolves to another address space is refused where upstream asserts

**Severity:** wrong-message, against a release build of upstream; an assertions
build aborts instead.
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` —
`resolve_one_forward_ref_dso_local_equivalent`.

**LLVM:** `LLParser::parseValID`'s `kw_dso_local_equivalent` arm makes its forward
reference a `GlobalVariable` in `DataLayout::getDefaultGlobalsAddressSpace`, and
`validateEndOfModule` replaces it with `DSOLocalEquivalent::get(GV)`, typed by the
function's own address space. When `G` and `P` differ the two types differ, and
`Value::doRAUW` asserts that they agree. A release build carries on.

**llvmkit:** `replace_all_uses_with` refuses the mismatch, so the module is refused
with `expected valid forward dso_local_equivalent: type mismatch: expected 'ptr
addrspace(1)', got 'ptr'`, anchored at the token the parse stopped on.

**Why:** a type mismatch on a use is invalid IR, and a replace that cannot be done
must not abort (Doctrine D10).

**Fix:** none owed — this is hardening of an assert, as D17 and D18 are. The text
is llvmkit's, because upstream's assert has none a user would see.

**Guarded by:** `crates/llvmkit-asmparser/tests/parser_constants.rs::a_forward_dso_local_equivalent_is_in_the_default_globals_space`.

### D30 — A name or string that is not valid UTF-8 is refused

**Severity:** rejects-valid
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs`, at each `std::str::from_utf8`
(`rg -n "from_utf8" crates/llvmkit-asmparser/src/ll_parser.rs`): `parse_string_constant`;
`current_str_payload` and its callers (`rg -n "current_str_payload\(" crates/llvmkit-asmparser/src/ll_parser.rs`);
`current_label_str`; `parse_comdat_definition`, `parse_global_or_function` and
`parse_optional_function_suffix`; `parse_named_metadata`; `parse_named_metadata_attachment`
and `skip_trailing_metadata`; `parse_addr_space_paren`; `parse_specialized_metadata_body`.
Gap **G9** in `docs/fixture-coverage.md`.

**LLVM:** a quoted name, a string constant, a quoted label and an escaped metadata name keep
whatever bytes `UnEscapeLexed` makes of them (`LLLexer::LexVar`, `LexQuote`, `LexDollar`,
`LexExclaim`); only a NUL in a name is refused. `LLParser` holds them in `std::string`, and
`printLLVMNameWithoutPrefix` and `printEscapedString` print the bytes back escaped.

**llvmkit:** the parser decodes each such token with `std::str::from_utf8`, and the IR
holds names and strings as `String`, so a byte sequence that is not UTF-8 is refused. Probed
with `target/release/examples/parse_file.exe` on 2026-11-05, over `ae636cd` plus the change
later committed as `f2fcd22`, which touches none of these routines:

| input | llvmkit answers |
|---|---|
| `@"\FF" = global i32 0` | `expected global identifier`, at the name |
| `$"\FF" = comdat any` | `expected valid UTF-8 comdat name` |
| `@g = global i32 0, section "\FF"` | `expected UTF-8 string constant`, at the string |
| a block labelled `"\FF":` | `expected basic-block label` |

The rest is read, not run. Every other `parse_string_constant` caller refuses with
`expected UTF-8 string constant`: metadata strings, `partition`, `gc`, `source_filename`,
the target triple and data layout, `module asm`, string attributes, operand-bundle tags,
inline asm, `syncscope`. `current_str_payload` answers a non-UTF-8 `@` or `%` name as
absent, so each caller reports its own `expected ...`. A named-metadata name refuses with
`expected valid UTF-8 metadata name`, and an attachment name with `expected valid UTF-8
metadata attachment name`. Two sites refuse what upstream refuses too, in other words: a
symbolic address space (upstream `invalid symbolic addrspace '...'`) and a specialized
metadata field name (upstream `invalid field '...'`). Three answer as upstream does, since
no non-UTF-8 name matches there either: `parse_optional_code_model`, `parse_named_metadata`'s
`DIExpression` / `DIArgList` lookup, and `parse_md_node_after_bang`'s `metadata type`.

Before this entry, over `ae636cd`, `rg -n -i "utf"` over `docs/divergences.md`,
`docs/future-work.md` and `docs/fixture-coverage.md` found only G9's row and its two fixture
rows, and `non-UTF`, `byte string` and `arbitrary bytes` found nothing more.

**Why:** names and strings are `String` through the IR, the printer and the public API; a
byte string is a model change across all three, which needs its own plan.

**Fix:** hold names and string payloads as bytes from the lexer through the IR, and print
them as `printLLVMNameWithoutPrefix` and `printEscapedString` do.

**Guarded by:** nothing. `test/Assembler/difile-escaped-chars.ll` and `named-metadata.ll`
are the fixtures it blocks (`docs/fixture-coverage.md`, G9).

### D31 — `computeKnownBits` knows nothing of an alias

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/value_tracking.rs` — `compute_known_bits_inner`, its
`ValueKindData::GlobalAlias` arm.

**LLVM:** `computeKnownBits`, below its depth cutoff: "A weak GlobalAlias is totally
unknown. A non-weak GlobalAlias has the bits of its aliasee." —
`if (!GA->isInterposable()) computeKnownBits(GA->getAliasee(), Known, Q, Depth + 1); return;`.

**llvmkit:** the arm answers `KnownBits::unknown` for every alias, interposable or not.
`GlobalValue::isInterposable` is ported (`constant_fold.rs::global_is_interposable`, in
`ae636cd`), so the arm has what it needs. Read, not run.

**Why:** found porting `isInterposable`, whose readers upstream this is one of; left to the
next plan's Task 19. Before this entry, `rg -n -i "computeKnownBits.*alias|alias arm" docs`
printed nothing.

**Fix:** port the arm in upstream's position: after the depth cutoff, before the operator walk.

**Guarded by:** nothing. The first test that would show it: a known-bits query on
`ptrtoint (ptr @a to i64)` with `@a = alias i8, inttoptr (i64 4096 to ptr)`, whose low 12
bits upstream answers zero (`derived`), and the `g1` function of
`test/Transforms/InstCombine/constant-fold-alias.ll`, which reads an alias's alignment.

### D32 — `ConstantFoldBinaryOpOperands` has no `SymbolicallyEvaluateBinop`

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/constant_folding.rs` —
`constant_fold_binary_op_operands_trusting_same_module` (its `let _ = dl`).

**LLVM:** `ConstantFoldBinaryOpOperands` (`lib/Analysis/ConstantFolding.cpp`) first asks
`SymbolicallyEvaluateBinop` when either operand is a `ConstantExpr`: an `and` whose known
bits already decide it answers one operand or a constant, and `(&GV+C1) - (&GV+C2)` answers
`C1-C2` through two `IsConstantOffsetFromGlobal` calls. Then `ConstantExpr::get` for a
desirable binary operator, and only then `ConstantFoldBinaryInstruction`.

**llvmkit:** `ConstantFoldBinaryInstruction` first, then `constant_expr` for a supported
opcode, with no symbolic step and the data layout unused. So the order is not upstream's, and
neither fold happens. Read, not run.

**Why:** `IsConstantOffsetFromGlobal` was ported whole in `ae636cd`, but its two callers in
folding were not: this routine and `simplifyRelativeLoad` (`docs/future-work.md`). Left to the
next plan's Task 19. Before this entry,
`rg -n "SymbolicallyEvaluateBinop|symbolically_evaluate_binop" docs` printed nothing.

**Fix:** port `ConstantFoldBinaryOpOperands` arm for arm, with `SymbolicallyEvaluateBinop`.

**Guarded by:** nothing. The first tests that would show it:
`ptrtoaddr_sub_consts_offset` and `ptrtoaddr_sub_consts_offset_addrsize` in
`test/Transforms/InstSimplify/ptrtoaddr.ll`, which answer `42`, and which
`crates/llvmkit-asmparser/tests/constant_fold_loads.rs::a_cast_of_a_constant_expression_folds_as_ptrtoaddr_ll_does`
leaves out, its doc says why; then the `f2` function of
`test/Transforms/InstCombine/constant-fold-alias.ll`, the `and` arm.

### D33 — A forward-named personality, aliasee or resolver joins its target's use list last

**Severity:** wrong-output (narrow)
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` — `parse_personality_fn`'s
`ParsedPersonalityFn::ForwardName` and `resolve_deferred_personality_fns`;
`parse_alias_or_ifunc`'s `forward_target` and `resolve_deferred_alias_targets`.

**LLVM:** a name not yet defined is read through `getGlobalVal`, which makes a placeholder
(`createGlobalFwdRef`), and the field is set over it at once; defining the name RAUWs the
placeholder, which prepends its uses, reversed, to the definition's list.
`predictValueUseListOrder`'s `GetsReversed` is written for that reversal.

**llvmkit:** a forward-named personality, aliasee or ifunc resolver is not set over a
placeholder. The parser keeps the name, sets a null or nothing, and sets the field at end of
module, so the use joins its target's list after every use made by then, in the order the
deferrals resolve. A count agrees; the order does not, so a `uselistorder` on the target
names its uses in another order than upstream's (`derived`: for `@a1` and `@a2`, aliases of
an `@f` defined after both, `@f`'s list reads `[a2, a1]` here and `[a1, a2]` upstream). A
global's initializer and an instruction operand do go through a placeholder.

**Why:** the deferrals predate use-list order (see `docs/future-work.md`, "Parser — deferred
alias/ifunc targets"). Found in `f2fcd22`, porting `parseFunctionHeader`'s setter order,
whose personality arm defers. Before this entry, `rg -n -i "deferred_personality|personality.*forward|forward.*personality" docs`
printed nothing.

**Fix:** set each field over `getGlobalVal`'s placeholder, as an initializer already is, and
let the placeholder's RAUW put the use in place.

**Guarded by:** nothing.

### D34 — An operand change that makes a `ConstantDataSequential` is taken as in place

**Severity:** wrong-output, latent: no input reaches it today (derived, see below).
**Where:** `crates/llvmkit-ir/src/constants.rs` — `aggregate_with_replaced_operand` and
`replace_constant_operand`.

**LLVM:** `ConstantArray::handleOperandChangeImpl` and `ConstantVector::handleOperandChangeImpl`
ask `getImpl` before `replaceOperandsInPlace`. When every new operand is a `ConstantInt` or a
`ConstantFP` of a compatible type, `getImpl` answers a `ConstantDataArray` or
`ConstantDataVector` from `getSequenceIfElementsMatch`. That is a different object, so
`handleOperandChange` runs `replaceAllUsesWith` and `destroyConstant`. A
`ConstantDataSequential` has no operands and no use list (`Value::hasUseList`).

**llvmkit:** a `ConstantDataSequential` is the same stored list as a `ConstantArray` or
`ConstantVector`, told apart by `Constant::kind`. So `getImpl`'s answer is a freshly interned
list whose operands are the old ones with `From` replaced, and `replace_constant_operand`'s
in-place test takes it as `replaceOperandsInPlace`'s update: the new list's edges take the old
ones' places on its elements' lists, and its users keep their order instead of being moved by
`replaceAllUsesWith`.

**Why unobservable today (derived from the code, not run):** only an element's or a user's
place on a use list differs. The elements are `ConstantInt`s or `ConstantFP`s, which are
`ConstantData` and have no use list a `uselistorder` line can name, and the list itself is
`ConstantData` by class (`Constant::is_constant_data`), so its own users have none to print
either. Reaching the case at all needs a forward reference inside an array or vector resolved
to a `ConstantInt` or `ConstantFP`; the parser resolves one inside a constant only to a global
or a `blockaddress`, so only `ForwardRefValue::replace_all_uses_with` called by hand reaches it.

**Fix:** Task 5 of the crash-proofing plan stores a `ConstantDataSequential` as bytes. That
answer then has no operands, so the in-place test's operand-count comparison sends it down the
replacement path, as it already sends a `ConstantAggregateZero` or an `undef`.

**Guarded by:** nothing yet. Found in Task 1 of the crash-proofing plan
(`task-1-report.md`, Concern 4), porting the `AllSame` arms.

### D37 — An `insertvalue` into a zero array of 2^32 elements or more declines

**Severity:** model-gap, and hardening where upstream asserts or answers a constant of the wrong
type.
**Where:** `crates/llvmkit-ir/src/constant_fold.rs` —
`constant_fold_insert_value_instruction`, through `aggregate_elements_for_rebuild` and
`aggregate_elements`, which declines a zero array whose length does not fit `unsigned`.

**LLVM:** `ConstantFoldInsertValueInstruction` (`ConstantFold.cpp`) counts the aggregate's
elements as `unsigned NumElts = cast<ArrayType>(...)->getNumElements()`, so for an array of 2^32
elements or more it walks only the low 32 bits' worth and hands `ConstantArray::get` a list
shorter than the type. `ConstantArray::getImpl` (`Constants.cpp`) answers that list in one of
three ways:
- an empty list, or one that is all `poison`, all `undef` or all null, gets the full type's
  constant, which drops an insertion at an index past the short count;
- a list of `ConstantInt`s or `ConstantFP`s gets `getSequenceIfElementsMatch`'s
  `ConstantDataArray` of the list's own length, a constant of the wrong type;
- any other list reaches the `ConstantArray` constructor's
  `V.size() == T->getNumElements()` assert.

All of this is derived from the routines and was not run.

**llvmkit:** `aggregate_elements` refuses to walk a zero whose narrowed count differs from its
length, so the fold answers `None` in all three cases. In the first case it answers `None`
where upstream answers a constant, which is right only when no insertion was dropped.

**Why:** the walk would have to build a list of 2^32 elements or more to answer as the type
says, or answer as upstream's narrowed count says, which is either wrong or an assert. Before
this entry,
`rg -n "aggregate_elements|getAggregateElement|aggregate_element\b|2\^32|4294967304|insertvalue|InsertValue" docs/divergences.md docs/future-work.md`
printed only unrelated lines: an `insertvalue` built through `NoFolder` in D23, an evidence
block on the three index-type walks, and a fixture list in `future-work.md`.
Fix round 3 of Task 1 of the crash-proofing plan ported `ConstantFoldExtractValueInstruction`'s
one-element read. It answered an `extractvalue` of the same arrays through the same decline, and
upstream answers it, so this is what remains of that decline.

**Fix:** none owed for the second and third cases, which are hardening. The first stays
declined: the cap on materialised elements (`MAX_EXPANDED_LANES`, D40) is far below 2^32, so a
zero that long declines past it whatever its count. The cap also declines a zero, `undef` or
`poison` array of 65537 to 2^32 − 1 elements, which upstream folds; that is D40's, not this
entry's.

**Guarded by:** nothing. The `extractvalue` side is pinned by
`crates/llvmkit-ir/tests/constant_aggregate_zero.rs::extractvalue_reads_one_element_of_a_long_array`.

### D38 — An intrinsic declared with no `memory(...)` reads its table's effects

**Severity:** model-gap (narrower than upstream: a call is said to touch what the intrinsic
table says, where upstream says it may touch anything).
**Where:** `crates/llvmkit-ir/src/instr_types.rs` — `CallBaseParts::memory_effects`, its
`Fn->getMemoryEffects()` half.

**LLVM:** `CallBase::getMemoryEffects` (`Instructions.cpp`) reads `Fn->getMemoryEffects()`,
which is the callee's `memory(...)` attribute or `MemoryEffects::unknown()`
(`AttributeSetNode::getMemoryEffects`, `Attributes.cpp`). `Function::Function` (`Function.cpp`)
gives an intrinsic its table attributes (`setAttributes(Intrinsic::getAttributes(...))`), but
`LLParser::parseFunctionHeader` then calls `Fn->setAttributes(PAL)` with the attributes the
`.ll` wrote, so a parsed `declare void @llvm.memcpy...(...)` with no attributes has no
`memory(...)`, and its calls have unknown effects. (Read from the three sites, not run.)

**llvmkit:** a callee that is an intrinsic and carries no `memory(...)` of its own (inline or
through an `#N` group) answers `IntrinsicId::memory_effects`, the effects its table gives.

**Why:** the fallback predates this entry (it was `speculation.rs::call_site_memory_effects`'s
last arm). Removing it moves every analysis that asks a hand-written intrinsic declaration
what it touches: dead-code elimination, `mayHaveNonDefUseDependency` and the assumption scan
among them. llvmkit's own tests and fixtures write many intrinsic declarations bare. Before this
entry, `rg -n -i "intrinsic table|descriptor.*memory|bare .declare|getMemoryEffects" docs/divergences.md docs/future-work.md`
printed only D9's bullet on `call_site_memory_effects`, which was about attribute groups.

**Fix:** drop the fallback, which makes a bare declaration's calls unknown as upstream's are, and
re-bless what moves. Each moved answer is then a fixture that should carry the intrinsic's
attributes, as `llvm-dis` output does.

**Guarded by:** nothing pins the fallback's answer on its own.

### D39 — A fixed splat of a value with a use list is one use of it, not one per lane

**Severity:** wrong-output (narrow): a use count and a `uselistorder` shuffle.
**Where:** `crates/llvmkit-ir/src/constant.rs` — `ConstantData::Splat` and its arm of
`ConstantData::for_each_operand`; `crates/llvmkit-ir/src/llvm_context.rs` —
`Context::intern_constant_splat`.

**LLVM:** a fixed vector whose lanes are all one value that is no `ConstantInt` or `ConstantFP`
of a `ConstantDataSequential` element type — `<2 x ptr> <ptr @g, ptr @g>`, or `splat (ptr @g)`
— is a `ConstantVector` (`ConstantVector::getImpl`, `getSplat`'s `get(Elts)`), and each lane is
an operand, so `@g` has one use per lane. A `ConstantDataVector` has no operands, and a
scalable splat's `insertelement` uses the value once.

**llvmkit:** every splat is stored as its one element (`ConstantData::Splat`, so that
`<4294967295 x ptr> splat (ptr @g)` costs one constant), and it registers one use of that
element. A scalable splat's operand is `getSplat`'s `insertelement`, which uses the element, as
upstream's does, so only a fixed splat is in question here. For the `ConstantVector` case only,
`@g`'s use list therefore holds one edge from the splat where upstream's holds one per lane, which changes `num_uses` and the `uselistorder`
directive the printer predicts for `@g`.

An operand change that makes a `ConstantVector` list's lanes all one value (a placeholder
resolved to the other lane) updates it in place, as `replaceOperandsInPlace` does, so its users
keep their order; the splat keeps one edge on the element's list, at the place of the list's
first (`constants.rs::replace_constant_operand`, through `take_edge_place`).

**Why:** an edge per lane would make the use list as long as the lane count, which is the
allocation the storage exists to avoid. Before this entry, `rg -n -i "splat" docs/divergences.md`
(run on `efbfa8a` with the uncommitted splat storage applied, the tree the commit that adds this entry starts from) printed no entry on use counts.

**Fix:** none planned that keeps the storage bounded. A use edge carrying a lane multiplicity
would let the count and the order be upstream's.

**Guarded by:** `crates/llvmkit-ir/tests/constant_splat.rs::a_constant_vector_splat_is_one_use_of_its_element`
pins the one edge, so a change to the storage's use registration fails there.

### D40 — A fold declines to build more than 65536 lanes out of a splat, a zero, `undef` or `poison`

**Severity:** wrong-output (narrow): a fold upstream performs is left as the instruction or
constant expression, so the printed IR differs. Hardening.
**Where:** `crates/llvmkit-ir/src/constant_fold.rs` — `MAX_EXPANDED_LANES`, `may_walk_lanes`,
`lane_walk` (the `LaneWalk` witness), and their uses in `aggregate_elements`,
`fixed_vector_elements_for_rebuild`, `aggregate_elements_for_rebuild`,
`constant_fold_select_instruction` and `constant_matches_negative_zero_fp_pattern`;
`crates/llvmkit-ir/src/constant_folding.rs` — the lane-changing arm of
`fold_bitcast_with_layout`, and `fold_vector_to_scalar_bitcast`;
`crates/llvmkit-ir/src/constant.rs` — `Constant::has_uniform_lanes` and `Constant::lanes_to_ask`,
read by `value_tracking.rs` (`constant_int_predicate_matches`, `shift_amount_known_in_range`,
`contains_undefined_element`, `contains_constant_expression`,
`expression_shuffle_mask_has_poison_lane`) and `vector_utils.rs`
(`constant_mask_lanes_satisfy`, `possibly_demanded_elements_in_mask`).

**LLVM:** a fixed vector stores every lane (`ConstantDataVector`, `ConstantVector`), so a fold
that walks the lanes (`ConstantFoldInsertElementInstruction`, the per-lane loops of
`ConstantFoldBinaryInstruction`, `ConstantFoldCompareInstruction`,
`ConstantFoldCastInstruction` and `ConstantFoldUnaryInstruction`, `ConstantFoldSelectInstruction`'s
`ConstantVector` loop, and `FoldBitCast`'s lane-changing loops in `Analysis/ConstantFolding.cpp`)
costs what the operand already cost to build.

**llvmkit:** a splat is stored as its one element (`ConstantData::Splat`), and a zero, `undef`
and `poison` store no lanes either, so `<4294967295 x i32> splat (i32 7)` is a few bytes of
text and one constant. A fold that would build its answer lane by lane out of such an operand
declines past `MAX_EXPANDED_LANES` (65536) lanes: `insertelement (<4294967295 x i32> splat (i32
7), i32 1, i64 0)` stays that expression, where upstream builds 4294967295 lanes; a lane-changing
`bitcast` of a long splat answers the `bitcast` expression, upstream's own answer for a lane it
cannot merge. A fold one of whose operands stores its lanes (an element list) walks them, and
builds an answer as long, at any length, since that costs what was stored: `add` of a stored
70000-lane list and a long splat is folded, as upstream folds it. An elementwise fold (unary,
binary, compare, same-lane cast) whose every operand stores one value for all its lanes (a splat,
a zero, `undef`, `poison`) folds lane 0 once and splats the answer, at any length: every lane of
upstream's loop folds the same inputs alike, and `ConstantVector::get` makes the repeated list
that splat, so `add (<4294967295 x i32> undef, splat (i32 1))` is `undef`, as upstream answers. The same cap holds for an
array that stores no elements: `ConstantFoldInsertValueInstruction` rebuilds every element of
the aggregate it inserts into, so `insertvalue` into `[70000 x i32] zeroinitializer` (or an
`undef` or `poison` array past 65536 elements) keeps the instruction, where upstream folds it. A
struct's type stores its fields, so a zero struct of any width is walked and folded. An answer longer than its
operand stores is held to the cap: a `bitcast` that splits a short list's lanes (`<2 x i65536>`
to `<131072 x i1>`), or a scalar (`i131072` to `<131072 x i1>`, through the one-lane vector
`FoldBitCast` wraps it in), into more than 65536 lanes answers the `bitcast` expression too.

Folds that need only some lanes read only those, and answer as upstream does at any length:
`extractelement` reads one lane through `getAggregateElement`, `shufflevector` one source lane
per mask lane (and, for a uniform mask — `splat`, `zeroinitializer`, `undef`, `poison` — one
source lane in all, answering the splat upstream's loop builds lane by lane,
`constant_fold_shuffle_vector_with_mask_constant`), and `select` reads its condition's lanes one at a time, as upstream's loop does,
so a lane that is no `ConstantInt` (and whose two arms differ) ends the walk there. A splat
condition past the cap is not walked at all: the fold goes on to the checks after the loop, so
where upstream's loop would have kept every lane (each lane's arms agreeing) the answer is the
`V1 == V2` check's or none. Analyses that ask every lane, or any
lane, one question (`cst_pred_ty`'s walk, `containsConstantExpression`, the mask tests of
`VectorUtils`, the shift-amount range check of `canCreateUndefOrPoison`) ask a vector stored in
one slot once (`Constant::lanes_to_ask`), which answers as the walk would.

The walks were found by `rg -n "aggregate_elements\(|\.aggregate_element\(|for (lane|i|index|element|idx) in 0\.\.|const_vector\(\(0\.\." crates/llvmkit-ir/src`
on `efbfa8a` with the uncommitted splat storage applied (the tree the commit that adds this entry
starts from), then by inline reviews (code-review skill, high), the second of which found the
vector-to-scalar `bitcast` (`fold_vector_to_scalar_bitcast` now holds to the cap too). A search
is not a proof, so the reads are pinned instead: `crates/llvmkit-ir/tests/lane_read_drift.rs`
lists every function in `crates/llvmkit-ir/src` that reads a constant's lanes through
`aggregate_element` or `extract_lane`, or repeat-fills per lane in the files that build
constants, how many times, and why each is bounded, and fails on one it does not list. A walk
spelled any other way escapes it; the scan narrows the search, it does not end it. Costs that are
proportional to a vector's lane count in upstream too (a demanded-lanes `APInt` one bit per lane,
an instruction's mask) are outside D40.

Two costs found to stay proportional to the lane count, as upstream's are: the printed form of a
splat that is a `ConstantVector` (`writeConstantInternal` writes every lane), and a
`shufflevector` instruction's mask, which stores one entry per lane
(`shufflevector_mask_from_constant`, upstream `ShuffleVectorInst::ShuffleMask`), so
`<4294967295 x i32> zeroinitializer` or `splat (i32 0)` as a mask still costs its lanes. The
second is a model-gap of the instruction, not of the folds, and is recorded in
`docs/future-work.md`.

**Why:** without the cap a few bytes of `.ll` make a fold allocate tens of gigabytes. Before this
entry, `rg -n -i "MAX_EXPANDED|expanded lanes|walk.*lanes" docs/divergences.md docs/future-work.md`
(run on `efbfa8a` with the uncommitted splat storage applied, the tree the commit that adds this entry starts from) printed nothing.

**Fix:** none owed; the cap is the hardening. A fold whose answer is periodic (a lane-merging
`bitcast` of a splat is a splat) could answer upstream's constant without the walk.

**Guarded by:** `crates/llvmkit-asmparser/tests/long_splat_folds.rs`, whose tests each fail
(by exhausting memory, or by a one-minute deadline) if their fold walks a long splat's lanes.

### D41 — A summary id costs one entry, where upstream sizes a vector by it

**Severity:** hardening. No answer differs against an upstream that can allocate what it asks
for; where it cannot, upstream fails to allocate and llvmkit parses.
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` — the `numbered_value_infos` field,
written by `add_global_value_to_index` and read by `parse_gv_reference`.

**LLVM:** `LLParser::addGlobalValueToIndex` stores the `ValueInfo` of `^N = gv: ...` at index
`N` of `NumberedValueInfos`, a `std::vector`, resizing it to `N + 1` when `N` is past its end
("to make test simplification easier"); `LLParser::parseGVReference` reads index `N` back. So a
summary naming `^4000000000` asks for 4000000001 slots, 64 GB at 16 bytes a `ValueInfo`.

**llvmkit:** the ids are keys of a `HashMap<u32, Guid>`, so an id costs one entry whatever its
value. The three answers upstream gives are kept: a defined id resolves to its GUID, an id never
defined is a forward reference, and a second definition of an id replaces the first (upstream's
`NumberedValueInfos[ID] = VI`). The `expected summary id to fit in a machine word` error the
vector's index conversion needed is gone with it: a `u32` key needs no conversion.

**Why:** a 78-byte summary naming `^4000000000` asked for 4,000,000,001 slots of
`Option<Guid>`, 16 bytes each (a `u64` with no niche), 64 GB, before parsing anything after it.
Before this entry, `rg -n -i 'NumberedValueInfos|numbered_value_infos|summary id'
docs/divergences.md docs/future-work.md` (run on `46362565`) printed nothing.

**Fix:** none owed; the map is the hardening.

**Guarded by:**
`crates/llvmkit-asmparser/tests/resource_bounds.rs::a_summary_id_near_the_top_of_the_range_costs_one_entry`,
which parses ids near `u32::MAX`, referenced before and after their definitions, inside a
one-minute deadline.

### D42 — A type size too large for 64 bits saturates, where upstream wraps

**Severity:** model-gap, on types no target can hold: `DataLayout` answers `u64::MAX` for a
size that does not fit in a `u64` in the unit asked for, where upstream answers that size
modulo 2^64. A type of 2^61 bytes or more has no bit size in a `u64`, so from there its bit
sizes saturate while its byte sizes stay exact until they too pass 2^64.
**Where:** `crates/llvmkit-ir/src/data_layout.rs` — `align_to`, `align_to_power_of_two`,
`type_store_size_inner` (which reads an aggregate's store size in bytes), and the size walks'
`TypeSize::saturating_scaled` / `TypeSize::saturating_add`
(`crates/llvmkit-ir/src/type_size.rs`).

**LLVM:** `TypeSize`'s `operator*` and `operator+`, `StructLayout::StructLayout`'s
`StructSize += ...` and `llvm::alignTo` are `uint64_t` arithmetic, which wraps. A struct of
`4 * 2^64` bytes (`%t64` of `%tK = type { %t(K-1), %t(K-1) }` over `{ i8, i16 }`) has an alloc
size of 0.

**llvmkit:** every size computation saturates: a product, a sum, and the round-up to an
alignment. Upstream's `getTypeStoreSize` divides the bit size by 8; llvmkit reads a struct's or
an array's store size in bytes instead (the same value whenever the bit size fits), because
dividing a saturated bit count would answer 2^61 - 1 bytes for any larger type. `u64::MAX` bytes is no size a type can have in memory, so the answer reads as "too
large to represent", never as a plausible small size; a wrapped size would let such a type pass
for a small one (an `alloca` of 0 bytes, an offset that fits). Products and sums already
saturated (`saturating_scaled` documents it); the round-up added `align - 1` unchecked, which
panicked in a debug build once a struct holding one struct type twice became sized (it was
taken for a cycle until `struct_is_sized` ported `SCDB_IsSized`), and now saturates too.

**Why:** Doctrine D10 (no undefined behaviour, no silent wrong answer), and no panic on a legal
query. A fallible size query (`Option<TypeSize>`) was weighed and not taken: every size query
upstream is infallible, every caller would grow an arm for a case no real type reaches, and the
saturated value already cannot be mistaken for a real size.

**Fix:** none owed; saturation is the hardening.

**Guarded by:**
`crates/llvmkit-asmparser/tests/resource_bounds.rs::a_struct_shared_sixty_four_levels_deep_parses_verifies_and_prints`,
which asserts the `u64::MAX` alloc and store sizes of a `4 * 2^64`-byte struct, and
`resource_bounds.rs::a_size_past_sixty_four_bits_saturates_without_shrinking_its_bytes`, which
asserts exact 2^62-byte alloc and store sizes and a saturated bit size for a 2^62-byte struct.

### D43 — A size query cannot be asked of an unsized type, where upstream asserts

**Severity:** model-gap, on queries upstream does not allow. `DataLayout`'s size and alignment
queries take a `SizedType`, a proof that the type is sized, so asking one of an unsized type
does not compile; the proof itself is refused at run time (`IrError::UnsizedType`).
`DataLayout::struct_layout` answers `None` wherever `StructLayout::StructLayout` asserts: an
opaque body; an unsized field (`Type::is_sized` false: for example an opaque struct or a struct
holding one, a target extension type with no layout, or a struct mixing a scalable vector with
other fields); and a field whose nonzero size cannot be added to the struct's nonzero size so far
because one is scalable and the other fixed (`TypeSize::operator+=`'s "Incompatible types"), as
in `{ <vscale x 4 x i32>, i8 }`. A struct that mixes a scalable vector only with fields of size
zero is unsized but has a layout, as upstream's does, so `struct_layout(s).is_some()` is no proof
that `s` is sized. (Commit `f54f4825`'s message says `None` "exactly when the struct is
unsized", and commit `a5fecaaa` wrote here that every struct mixing a scalable vector with sized
fields has a layout; both are wrong, and this paragraph is the correction. At `a5fecaaa`,
`{ <vscale x 4 x i32>, i8 }` was laid out with a scalable offset and a fixed 32-byte size.)
**Where:** `crates/llvmkit-ir/src/derived_types.rs` — `SizedType`, minted by
`TryFrom<Type>` (which asks `Type::is_sized`) or from an integer, float or pointer type, sized by
construction, and the crate-private `erase_sized`; `crates/llvmkit-ir/src/data_layout.rs` — the
eight queries that take `impl Into<SizedType>` (`rg -n -U 'pub fn \w+<[^>]*>\(\s*&self,\s*ty: impl Into<SizedType' crates/llvmkit-ir/src/data_layout.rs`,
eight at `a5fecaaa`:
`type_size_in_bits`, `type_store_size`, `type_store_size_in_bits`,
`type_size_equals_store_size`, `type_alloc_size`, `type_alloc_size_in_bits`, `abi_type_align`,
`pref_type_align`), `value_or_abi_type_align` (which takes any type and
asks for the proof only when its alignment is unset, as upstream reads the type only then),
`reached_unsized`, and `struct_layout` / `struct_layout_of`;
`crates/llvmkit-ir/src/ir_builder.rs` — `IrBuilder::sized`.

**LLVM:** `DataLayout::getTypeSizeInBits` (`DataLayout.h`) and `DataLayout::getAlignment`
(`DataLayout.cpp`) assert `Ty->isSized()`, and every other size query reaches one of them;
`StructLayout::StructLayout` asserts `!ST->isOpaque()`. A release build skips the asserts and
answers numbers for a type that has no size: an opaque struct laid out with no elements.

**llvmkit:** the precondition is in the signature. A query's answer is a plain `TypeSize`,
`Align` or `bool`, since a sized type always has one. Inside a sized type every type its walk
reaches is sized (a sized struct's fields, a sized array's element, a sized target extension
type's layout type), so the walks' arms for unsized types are `reached_unsized`, an
`unreachable!` that names that closure. The crate's own callers that start from an arbitrary type
take the proof through `erase_sized` and decline on `None` where upstream would assert (their
sites: `rg -n 'erase_sized\(' crates/llvmkit-ir/src`). `struct_layout` lays out only a struct
every field of which is sized, so no layout in the `DataLayout`'s struct-layout map reaches an
opaque struct, and none goes stale when one gets a body. The `IrBuilder` fills an omitted
`alloca`, `load` or `store` alignment only through a `SizedType`, and refuses an unsized type with
`IrError::UnsizedType`. Earlier, the queries answered `None` for an unsized type, and before
that they answered an empty layout's numbers, and `struct_layout_inner` reached an
`unreachable!` on an opaque struct, a panic on a public query.

**Why:** no runtime panic in a production path (CLAUDE.md), no number that is not a size, and
no `Option` on a query whose answer always exists once its precondition holds: readers of the
empty layout's numbers folded loads to the wrong constant and filled alignments that went wrong
once the opaque struct got a body.

**Fix:** none owed; this is hardening of an assert, as D17 and D18 are.

**Guarded by:**
`crates/llvmkit-ir/tests/data_layout_round_trip.rs::struct_layout_of_an_opaque_field_is_refused_and_not_cached`
(no proof, no layout, and the real ones once the body is set),
`crates/llvmkit-ir/tests/type_components.rs::a_target_type_upstream_asserts_on_has_no_layout`
(a target extension type with no layout has no proof),
`crates/llvmkit-ir/tests/data_layout_round_trip.rs::every_sized_type_answers_every_size_query`
(every type with a proof answers every query, so `reached_unsized` stays dead; it panicked when
`is_sized` was made to accept a target extension type with no layout),
`crates/llvmkit-ir/tests/data_layout_round_trip.rs::a_struct_mixing_scalable_and_fixed_sizes_has_no_layout`
(no layout where `TypeSize::operator+=` asserts, a scalable one where a zero-sized field passes it),
`crates/llvmkit-ir/tests/constant_folding_analysis.rs::a_load_from_a_constant_of_unsized_type_is_not_folded`
(the two folds that read the empty layout's numbers),
`crates/llvmkit-ir/tests/builder_alloca.rs::alloca_load_and_store_refuse_to_guess_an_unsized_alignment`,
and `crates/llvmkit-asmparser/tests/resource_bounds.rs::a_shared_struct_over_an_opaque_field_is_refused_at_once`.

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

*lexer* — crates/llvmkit-asmparser/src/ll_lexer.rs (`lex_uint`, `LexError::IdOverflow`, `LexError::IntegerOverflow64`)

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

### 138. `getOrInsertIntrinsicDeclarationImpl` refuses a same-typed holder that lacks the intrinsic's identity

**Severity:** rejects-valid (derived by reading; no test exhibits it — a
hypothesis until one does)
**Where:** `crates/llvmkit-ir/src/module.rs` —
`ModuleCore::get_or_insert_intrinsic_declaration`, inside its
`F->getFunctionType() == FT` arm.

- **LLVM:** `getOrInsertIntrinsicDeclarationImpl` (`IR/Intrinsics.cpp`)
  returns the function holding the name when `F->getFunctionType() == FT`. The
  type is the only condition: `F`'s `IntID` is derived from its name, which is
  the intrinsic's, so upstream's `F` here is always the intrinsic.
- **llvmkit:** a holder of the right type whose stored identity is not the
  requested descriptor is refused with `IrError::IntrinsicSignatureMismatch`.
  The stored identity is recomputed from name *and* signature
  (`update_after_name_change`), so a holder with the right name and type
  differs from the descriptor in one of three ways, and the guard refuses all
  three:

  | holder | what `IntrinsicCallBuilder::build` would do with it |
  |---|---|
  | no identity, because the name's overload suffix does not demangle, or demangles to another type | `IntrinsicInst::from_call` → `descriptor_for_callee`, whose by-name fallback fails the same way: `build` emits its call, then refuses — an error after a mutation |
  | no identity only because `intrinsic_descriptor_from_signature`'s `mangled_name() != name` round trip failed | the fallback, which skips that round trip, finds one: `build` succeeds |
  | a different stored identity | `from_call` reads it: `build` succeeds, with that id |

  The guard is kept for the first row only; for the other two it refuses a
  request upstream answers with `F` and that llvmkit's builder could also
  serve. The two sibling guards it shipped with — a definition refused, and
  the mismatched type refused instead of renamed — are gone: the arm returns a
  definition and the `.invalid` arm is ported.
- **Found:** 2026-10-04, the rename port's review (I1); the guard predates the
  port (`git show 458e9ce:crates/llvmkit-ir/src/module.rs` carries it).
  `rg -n "getOrInsertIntrinsicDeclarationImpl|\.invalid" docs/divergences.md
  docs/future-work.md` found only an unrelated parser-sweep entry before this
  one.
- **Fix:** closes with entry 136 — a name-only identity makes the holder the
  intrinsic, as upstream — or with `IntrinsicCallBuilder::build` checking its
  callee before it emits.

### 139. The verifier requires an intrinsic declaration to carry the intrinsic's attributes

**Severity:** rejects-valid (exhibited by a probe; evidence below)
**Where:** `crates/llvmkit-ir/src/verifier.rs` —
`Verifier::verify_intrinsic_function`, the
`declaration_attributes(..).is_subset_of(..)` check and the attribute-group
check beside it.

- **LLVM:** the `Verifier` never consults `Intrinsic::getAttributes`
  (`rg -n "Intrinsic::getAttributes" llvm/lib/IR/Verifier.cpp` is empty);
  `visitFunction` passes `F.isIntrinsic()` to `verifyFunctionAttrs` only to
  allow `immarg`. A function gets an intrinsic's attributes from `Function`'s
  constructor when it is created under the name with a valid signature;
  `setName` adds none and `setAttributes` may remove them, and the module
  verifies either way.
- **llvmkit:** refuses with `InvalidOperation { message: "intrinsic
  declaration modifier" }` unless the declaration carries every attribute of
  `IntrinsicDescriptor::declaration_attributes`. A plain `declare void
  @plain()` renamed to `llvm.debugtrap` fails `verify`; the declaration
  `get_or_insert_intrinsic_declaration_by_name` makes, which carries
  `nounwind`, passes. Five tests exercise the check — four its refusal, one
  its acceptance of an extra attribute — and until this entry their doc
  comments and `UPSTREAM.md` rows called them a `mirror` of `visitFunction`;
  they are relabelled llvmkit-specific:
  `verifier.rs::tests::intrinsic_declaration_missing_generated_attrs_is_rejected`
  and `::intrinsic_declaration_extra_attr_group_is_rejected`,
  `tests/verifier_basic.rs::intrinsic_declaration_missing_generated_function_attrs_is_rejected`
  and `::intrinsic_declaration_missing_generated_argument_attr_is_rejected`,
  `tests/intrinsics_generated.rs::verifier_accepts_extra_valid_intrinsic_declaration_attribute`.
  The list is the `crates/llvmkit-ir` rows that
  ``git show bac8293:UPSTREAM.md | rg -F 'Verifier.cpp::visitFunction` intrinsic declaration'``
  returns; the same search also returns five `llvmkit-asmparser` rows, which
  pin the parser's own intrinsic-declaration refusals rather than this check.
- **Found:** 2026-10-04, the rename port's review (M8): renaming made a
  natural trigger for a check already reachable through `set_attributes`.
  `rg -n "declaration_attributes|is_subset_of|intrinsic declaration modifier"
  docs/divergences.md docs/future-work.md` found nothing before this entry.
- **Fix:** delete the check and the four tests' assertions of it, with an
  upstream `.ll` fixture of an intrinsic declaration stripped of its
  attributes as the regression test.

<details><summary>Probe (2026-10-04, branch <code>rename-fix</code> on <code>bac8293</code>)</summary>

A temporary test, deleted after the run, built `declare void @plain()` with
`add_function_dyn`, renamed it `llvm.debugtrap`, printed the module and ran
`verify_borrowed`, then did the same for a module whose declaration came from
`get_or_insert_intrinsic_declaration_by_name("llvm.debugtrap")`. Output:

```text
PROBE renamed text:
; ModuleID = 'm'

declare void @llvm.debugtrap()

PROBE renamed verify: Err(InvalidOperation { message: "intrinsic declaration modifier" })
PROBE control text:
; ModuleID = 'control'

declare void @llvm.debugtrap() nounwind

PROBE control verify: Ok(())
```

</details>

## Accepts invalid input

llvmkit accepts IR that LLVM rejects, so a malformed module survives into the rest of the pipeline.

### 141. `Verifier::visitGlobalValue` and `visitGlobalAlias` are unported, and so is the `visitMDNode` call in `visitGlobalIFunc`'s attachment loop

**Severity:** accepts-invalid (derived by reading; no fixture exhibits it — a
hypothesis until one does)
**Where:** `crates/llvmkit-ir/src/verifier.rs` — `Verifier::run`,
`Verifier::visit_global_variable`, `Verifier::visit_global_ifunc`,
`Verifier::visit_function`.

- **LLVM:** `Verifier::visitGlobalVariable`, `visitGlobalAlias`,
  `visitGlobalIFunc` and `visitFunction` each call `visitGlobalValue(GV)`,
  whose `Check`s every global value meets — "Global is external, but doesn't
  have external or weak linkage!", the `!associated` metadata shape, the
  `dllimport` / `dso_local` rules, "Global is referenced in a different
  module!" and the rest. `Verifier::verify` also walks `M.aliases()` through
  `visitGlobalAlias` ("Alias should have private, internal, linkonce, weak,
  linkonce_odr, weak_odr, external, or available_externally linkage!", the
  aliasee walk). And `visitGlobalIFunc`'s attachment loop calls
  `visitMDNode(*I.second, AreDebugLocsAllowed::No)` on every attachment after
  its `!dbg` / `!prof` checks.
- **llvmkit:** no `visit_global_value` exists, so none of its `Check`s runs for
  any global; `Verifier::run` walks globals, ifuncs, the module flags and
  functions, but not aliases; and `visit_global_ifunc` ports the `!dbg` /
  `!prof` checks, the linkage check and the four resolver checks, but not the
  `visitMDNode` call. A module that fails only one of these verifies.
- **The aliasee walk includes the interposable-alias check.** `visitAliaseeSubExpr`,
  which `visitGlobalAlias` runs on each aliasee, checks `Check(!GA2->isInterposable(),
  "Alias cannot point to an interposable alias", &GA)` when the aliasee is itself an
  alias, beside "Alias must point to a definition" and "Aliases cannot form a cycle".
  So `@test3_c = alias i32, ptr @test3_b` over `@test3_b = weak alias i32, ptr @test3_a`
  verifies here (read, not run). The predicate is ported
  (`constant_fold.rs::global_is_interposable`, behind `GlobalValue::is_interposable`, in
  `ae636cd`), so the check waits only on the walk. Named 2026-11-05: before then,
  `rg -n -i "interposable alias" docs` printed nothing. The first test that would show it
  is `test/Verifier/alias.ll`, whose `CHECK: Alias cannot point to an interposable alias`
  names `@test3_c`.
- **Found:** 2026-10-04, porting `Verifier::visitGlobalIFunc`'s resolver and
  attachment checks (capability-typestate Task 4, fix round 2), whose first
  statement is `visitGlobalValue(GI)`.
  `git grep -n -E "visitGlobalValue|visit_global_value|visitGlobalAlias|visit_global_alias" 2eff2e7 -- crates/llvmkit-ir/src/verifier.rs docs`
  returned nothing (exit 1) before this entry. The `visitMDNode` half is the
  metadata walk the `visitDIExpression` entry under *Model gaps* already names
  as missing; this entry records the ifunc call site of it.
- **Why not in the same change:** `visitGlobalValue` is a routine of its own,
  shared by four visitors and reaching the metadata, linkage and module
  provenance subsystems; porting it, and `visitGlobalAlias` with it, wants its
  own arm table and its own `test/Verifier` fixtures. `visitMDNode` is the whole
  metadata-verification subsystem.
- **Fix:** port `visitGlobalValue` whole and call it first from each of the four
  visitors; port `visitGlobalAlias` and walk `M.aliases()` in `Verifier::run`
  in upstream's order; then call the ported `visitMDNode` from the ifunc
  attachment loop.

### 132. `Verifier::visitIntrinsicCall`'s signature split, its `MetadataAsValue` walk and all but one arm of its `switch` are unported — **NARROWED (W9)**

*verifier — call family* — crates/llvmkit-ir/src/verifier.rs (`check_intrinsic_call`)

Found 2026-08-27 while porting `Verifier::visitCallBrInst` (former entry 128):
`test/Verifier/callbr.ll`'s four `llvm.callbr.landingpad` functions could not
be ported with the rest of the fixture, because the routine that answers them
did not exist here.

**Narrowed 2026-08-29 (W9).** Three of those four now run, from
`crates/llvmkit-asmparser/tests/parser_calls.rs::upstream_callbr_landingpad_fixture_messages_match`.
`check_intrinsic_call` carries the preamble's `Intrinsic functions should never
be defined!`, the `const x86_amx is not allowed in argument!` half of its
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
- **`test/Verifier/x86_amx9.ll` is not ported**, though the `Check` it pins
  now exists. Its argument is `x86_amx bitcast (<256 x i32> … to x86_amx)`. The
  blocker recorded here — the parser answering `invalid bitcast constant
  expression` — is gone since `7efef7f`: `validate_constant_expr_data`'s
  `bitcast` arm now asks `cast_is_valid`, whose non-pointer arm compares
  `primitive_size_in_bits` (8192 for both types), and `valid_bitcast_constant`,
  which refused `x86_amx`, is deleted. Whether the fixture now parses and
  reaches the verifier has not been run.
  `verifier_basic.rs::const_x86_amx_intrinsic_argument_is_rejected` still builds
  an `undef` of that type instead, and its doc comment still gives the old
  reason.
- **`visitMetadataAsValue` on every `MetadataAsValue` argument.** The other half
  of the argument walk, and a whole routine of its own — it is what rejects an
  `MDNode` local to another function.
  `rg -n "visit_metadata_as_value|MetadataAsValue" crates/llvmkit-ir/src/verifier.rs`
  returns only comment lines in `check_intrinsic_call` quoting upstream's loop
  and naming the half as unported.
- **Every `switch (ID)` arm except `Intrinsic::callbr_landingpad`** — `llvm.assume`'s
  operand bundles, the `experimental.gc.*` family, the constrained-FP family,
  `Intrinsic::ucmp` / `Intrinsic::scmp`, and the rest. Each needs the
  `test/Verifier` fixture that pins it. For `scmp` / `ucmp` that is
  `test/Verifier/intrinsic-cmp.ll`, none of whose four `CHECK` messages
  llvmkit has: at `c541e21`,
  `rg -n "same number of elements|at least 2 bits wide|ucmp/scmp" crates/llvmkit-ir/src crates/llvmkit-asmparser/src`
  finds nothing.
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

*verifier* — crates/llvmkit-ir/src/verifier.rs (no counterpart routine; the module header's "Coverage gaps (deferred)" list says the two routines "have no counterpart here")

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
  first two are `verifier.rs`'s module header naming what has no counterpart, the
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
  @llvm.wasm.ref.null.exn()` therefore parses here and does not upstream. For
  the same reason the intrinsic name mangler (`append_mangled_type`,
  `crates/llvmkit-ir/src/intrinsics.rs`) spells an `exnref` overload `exnref`
  and its demangler (`parse_mangled_type_prefix`) reads that back;
  `intrinsics.rs::tests::every_mangled_type_demangles_to_itself` pins the pair.
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

### 140. Names given at creation skip `setNameImpl`'s assertions

**Severity:** accepts-invalid, wrong-output (derived by reading; no test
exhibits it — a hypothesis until one does)
**Where:** the routes that name a value without `Value::set_name`:
`crates/llvmkit-ir/src/module.rs` (`ModuleCore::push_function`,
`install_global_variable`, `install_global_alias`, `install_global_ifunc`,
which call `set_global_value_name`), `crates/llvmkit-ir/src/function.rs`
(`FunctionValue::set_local_value_name`, which the builders and
`append_basic_block` call), and `crates/llvmkit-ir/src/instruction.rs`
(`create_detached_instruction`).

- **LLVM:** every constructor that takes a name runs `Value::setName`, so
  `Value::setNameImpl`'s assertions — no NUL byte in a name, no name on a
  `void` value — bind at creation as at a rename. An `IRBuilder` call that
  names a `void` instruction asserts.
- **llvmkit:** `Value::set_name` refuses both with
  `IrError::InvalidValueName`, as hardening of the assertions. Creation does
  not run it: a name with a NUL byte is stored as given — and printed as a
  quoted `\00` escape that `LLLexer` rejects ("NUL character is not allowed in
  names") — and a name a builder is given for a `void` instruction is dropped
  without a word.
- **Found:** 2026-10-04, the rename port's review (M5), when `set_name` began
  refusing. Before it, nothing in the IR crate inspected a name for NUL:
  `rg -n -F "'\0'" crates/llvmkit-ir/src` was empty at `bac8293`.
- **Fix:** run the refusal at each creation entry before anything is pushed —
  the global creators already return `IrResult` — and give the builders'
  dropped `void` name the same answer.

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

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_basic_block`'s `seen_non_phi` guard)

- **LLVM:** `LLParser` accepts a `phi` written after a non-phi instruction and lets `Verifier::visitPHINode` reject it with `PHI nodes not grouped at top of basic block!`.
- **llvmkit:** `parse_basic_block` tracks a `seen_non_phi` flag and rejects at parse time. The message is emitted through `self.expected(...)`, whose `ParseError::Expected` renders `#[error("expected {expected}")]`, so the user-visible string is the ungrammatical `expected phi must be grouped at the top of its basic block`. Same verdict, wrong layer, and a message upstream never prints.
- **Why:** Recorded, with an explicit "do not fix this by deleting the parser check": every phi llvmkit builds goes through `IrBuilder::make_phi_in_block` or `IrBuilder::append_phi_instruction`, and **both** end in `BasicBlock::insert_instruction_at_phi_head`, which places the phi at the block's phi head regardless of the insertion point. Drop the parse check and a misplaced phi is *silently hoisted* into a legal position, so llvmkit's own `VerifierRule::PhiNotAtTop` never fires — accepting invalid IR and quietly rewriting it, strictly worse than the current strictness.
- **Fix:** Add a non-hoisting insertion path for parsed phis so the instruction lands where it was written, then delete the parse-time check and let `VerifierRule::PhiNotAtTop` deliver upstream's verdict and wording. That is entangled with llvmkit's head-phi design — block parameters are the leading run of head-phis, which is what `basic_block::block_parameter_phis` reads and `IrBuilder::append_block_with_params` writes — so it wants deciding alongside that model rather than as a parser patch.
- **Narrowed, and a duplicate merged.** Two halves used to be recorded here. The *message-text* half — `VerifierRule::PhiNotAtTop` rendering `PHI nodes not grouped at top of block`, dropping upstream's `basic` and its `!` — is closed: the rule now carries `PHI nodes not grouped at top of basic block!` in `IrError::VerifierFailure`'s `message`, asserted by `crates/llvmkit-ir/src/verifier.rs::phi_not_at_top` against the vendored fixture's own `CHECK` text. Only the *wrong-layer* half above is left. This entry also absorbed a second entry that recorded the same divergence in the same terms (former **35**, "A misplaced `phi` is rejected at parse time, not by the verifier"); nothing distinguished the two, and the duplicate was deleted rather than left to be closed twice.
- **What is blocked by it:** `test/Verifier/PhiGrouping.ll` is vendored at `crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/PhiGrouping.ll` and cannot be driven through the parser. `crates/llvmkit-asmparser/tests/parser_function_body.rs::upstream_phi_grouping_fixture_is_rejected_at_parse_time` asserts *this* entry's parse diagnostic instead, so the day the entry closes the test fails and the real port replaces it.

<details><summary>Verification evidence — re-read 2026-09-01 (divergence W15)</summary>

llvmkit: `crates/llvmkit-asmparser/src/ll_parser.rs` — `let mut seen_non_phi = false;` opens `parse_basic_block`'s instruction loop and the guard reads `if matches!(opcode, Opcode::Phi) { if seen_non_phi { return Err(self.expected("phi must be grouped at the top of its basic block")); } } else { seen_non_phi = true; }`. Message rendering: `crates/llvmkit-asmparser/src/parse_error.rs`, `#[error("expected {expected}")]` on `ParseError::Expected`. Pinned by `crates/llvmkit-asmparser/tests/parser_errors.rs::phi_after_non_phi_is_a_parse_error`. The hoisting claim, re-read rather than re-probed: `crates/llvmkit-ir/src/ir_builder.rs` has exactly two phi insertion paths, `IrBuilder::make_phi_in_block` and `IrBuilder::append_phi_instruction`, and each ends `bb.insert_instruction_at_phi_head(id);` under a comment saying placement ignores the builder's cursor; `rg -n 'insert_instruction_at_phi_head' crates/` lists those two call sites and the definition. Upstream: `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp` — `LLParser::parseBasicBlock` has no ordering state in its instruction loop, and `LLParser::parsePHI` rejects only `phi node must have first class type` plus bracket-syntax errors. `orig_cpp/.../llvm/lib/IR/Verifier.cpp` — `Verifier::visitPHINode` holds the check: `Check(&PN == &PN.getParent()->front() || isa<PHINode>(--BasicBlock::iterator(&PN)), "PHI nodes not grouped at top of basic block!", &PN, PN.getParent());`. llvmkit's counterpart rule is `VerifierRule::PhiNotAtTop` in `crates/llvmkit-ir/src/verifier.rs`.

</details>

### 32. Attribute-group reference error is fatal here, non-fatal upstream

*parser (attribute groups)* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_fn_attribute_value_pairs`'s `Token::AttrGrpId` arm), pinned by crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs::attribute_group_diagnostics_match_upstream_text

- **LLVM:** `LLParser::parseUnnamedAttrGrp` reports `cannot have an attribute group reference in an attribute group` non-fatally — it keeps parsing and accumulates further diagnostics.
- **llvmkit:** llvmkit reports the first and stops. The message text matches; the recovery does not.
- **Superseded, 2026-08-16 (W14a):** the second half of this entry — that a misspelled keyword is intercepted by llvmkit's lexer, so `unterminated attribute group` had to be triggered with a type keyword, an integer and end-of-file — is closed. A misspelled keyword now arrives as `Token::Error` and is a fourth trigger in `attribute_group_diagnostics_match_upstream_text`. The evidence block below predates that and says otherwise. (Separately: no upstream `.ll` pins `unterminated attribute group` at all, so "upstream's trigger" was itself a wrong premise.)
- **Why:** Recorded — "the same choice already recorded for the position diagnostics", i.e. llvmkit's parser is fail-fast by design where upstream accumulates.
- **Fix:** Either accept it as a documented global policy (then say so once, in `AGENTS.md`, rather than per-test) or give `ParseError` an accumulating mode so multi-diagnostic fixtures can be ported whole.
- **Correction from verification:** The divergence is real and still present, but one phrase is imprecise: upstream does NOT "accumulate further diagnostics" in the sense of retaining a list. `LLLexer::ErrorInfo` (LLLexer.h) holds exactly one `SMDiagnostic &Error` plus an `ErrorPriority`, and `LLLexer::Error` is `if (Priority < ErrorInfo.Priority) return; ErrorInfo.Error = ...`. Every `LLParser::error` goes through `Lex.ParseError` at `ErrorPriority::Parser`, so an equal-priority later error is NOT less-than and therefore OVERWRITES the earlier one. `HaveError` is only a bool that forces the eventual `return true`. Corrected statement: `LLParser::parseFnAttributeValuePairs` reports `cannot have an attribute group reference in an attribute group` non-fatally (`HaveError |= error(...)`, then `Lex.Lex(); continue;`) and keeps parsing the rest of the group, so the message the user finally sees is the LAST parser diagnostic the group produced, not this one. llvmkit returns `Err` at the first `#N` and stops, so it always reports THIS message. The consequence is sharper than "the recovery does not match" — it is an observable message-text divergence on inputs with a second fault: for `attributes #0 = { #1 i32 }` upstream ends at `unterminated attribute group` (the `Attr == Attribute::None` + `InAttrGrp` arm overwrites), while llvmkit reports `cannot have an attribute group reference in an attribute group` (confirmed empirically). Two lesser notes: (a) the test actually spans lines 899-955, not 899-935 — the cited range stops mid-test after the fifth assert; (b) the doc's "upstream's fixture spells it with a misspelled keyword" is not backed by a fixture I could locate — `test/Assembler/invalid-attrgrp.ll` is the only attrgrp fixture and it pins only `expected attribute group id`; no `.ll` under `test/Assembler` pins `unterminated attribute group` or the attrgrp-reference message at all. The *mechanism* described is nonetheless correct.

<details><summary>Verification evidence</summary>

UPSTREAM (C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp, `LLParser::parseFnAttributeValuePairs`): the `Token == lltok::AttrGrpID` arm reads `if (InAttrGrp) { HaveError |= error(Lex.getLoc(), "cannot have an attribute group reference in an attribute group"); } else { FwdRefAttrGrps.push_back(...); } Lex.Lex(); continue;` — non-fatal, loop continues. Contrast the fatal sibling four lines later: `if (Attr == Attribute::None) { if (!InAttrGrp) break; return error(Lex.getLoc(), "unterminated attribute group"); }`. The function ends `return HaveError;`, and `LLParser::parseUnnamedAttrGrp` only propagates that. UPSTREAM error retention (LLLexer.h lines ~35-46 and LLLexer.cpp `LLLexer::Error`): `enum class ErrorPriority { None, Parser, Lexer }`; `struct ErrorInfo { ErrorPriority Priority; SMDiagnostic &Error; }` — a single diagnostic slot. `void LLLexer::Error(LocTy, const Twine &Msg, ErrorPriority Priority) { if (Priority < ErrorInfo.Priority) return; ErrorInfo.Error = SM.GetMessage(...); ErrorInfo.Priority = Priority; }`. `LLParser.h:218` is `bool error(LocTy L, const Twine &Msg) { return Lex.ParseError(L, Msg); }`, and `LLLexer.h:99` shows `ParseError` calls `Error(ErrorLoc, Msg, ErrorPriority::Parser)`. Parser-vs-Parser is not `<`, so later parser errors overwrite — last-wins, not accumulate. LLVMKIT (C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:9463-9467), inside `parse_fn_attribute_value_pairs`: Token::AttrGrpId(_) if context == AttrListContext::AttributeGroup => { return Err(self.message( "cannot have an attribute group reference in an attribute group", )); } A bare `return Err` — fatal, before any further token is consumed. Message text matches upstream exactly; the control flow does not. TEST (C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:899-955): `attribute_group_diagnostics_match_upstream_text`. Its doc comment at 899-907 states the divergence in the project's own words ("is non-fatal upstream (it keeps parsing and accumulates); llvmkit reports the first and stops"). The cited assert is at 928-931. `cargo +1.96.0 test --release -p llvmkit-asmparser --test parser_attribute_matrix attribute_group_diagnostics_match_upstream_text` passes (1 passed), so the pinned behavior is current, not stale. EMPIRICAL (temporary probe test, since removed): llvmkit returns `cannot have an attribute group reference in an attribute group` for all of `attributes #0 = { #1 i32 }`, `{ #1 #2 }`, and `{ #1 nounwind }` — proving it stops at the first `#N` regardless of what follows. Upstream's loop would reach `i32` (lexed `lltok::Type`, `tokenToAttribute` -> `Attribute::None`, `InAttrGrp` true) and overwrite with `unterminated attribute group`. LEXER SUB-CLAIM CONFIRMED. Upstream LLLexer.cpp `LexIdentifier` ends `// Finally, if this isn't known, return an error. CurPtr = TokStart+1; return lltok::Error;` — a silent token kind carrying no message, which flows into the `Attribute::None` arm and yields `unterminated attribute group`. llvmkit instead fails in the lexer: ll_lexer.rs:128-131 defines `UnknownTokenReason::UnknownKeyword` with `#[error("unknown keyword '{word}'")]`, and the doc comment at ll_lexer.rs:95-103 says outright "**This has no upstream counterpart, deliberately.** `LLLexer` returns a bare `lltok::Error` at every one of these sites and records no message". Probe confirms: `attributes #0 = { nounwindd }` yields `unknown keyword 'nounwindd'`, never reaching `parse_fn_attribute_value_pairs`. That is exactly why the test substitutes a type keyword (`i32`), an integer (`42`), and EOF as triggers.

</details>

### 34. `ChecksumKindField` quotes a `Lex.getStrVal()` llvmkit does not keep — **NARROWED (W15)**

*metadata parser* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_metadata_field_value`, `check_metadata_field_value`)

- **Closed half (W14a):** a word matching no keyword now lexes as
  `Token::Error` and reaches the field parser, so `emissionKind: Bogus` /
  `nameTableKind: Bogus` / `!DIFixedPointType(kind: Bogus)` answer
  `expected emission kind` / `expected nameTable kind` /
  `expected fixed-point kind`, and the sibling `DW_*` families answer their
  own `expected …` the same way.
- **Closed half (W15):** the *acceptance* rule. `parse_metadata_field_value`
  dispatches on the field's declared kind for the twelve keyword families the
  way upstream's typed overloads do — `if (Lex.getKind() == lltok::APSInt)
  return parseMDField(…, MDUnsignedField &); if (Lex.getKind() != lltok::X)
  return tokError("expected <family>");` — so `null`, a string, a
  `!`-reference and a keyword from a *sibling* family are all the family's
  `expected …`, and `metadata_keyword_field_max` carries each family's
  `MDUnsignedField` `Max` so a raw encoding past it is
  `value for '<name>' too large, limit is <max>`. Five upstream fixtures pass
  as a result, all corpus rows:
  `test/Assembler/invalid-generic-debug-node-tag-wrong-type.ll`,
  `test/Assembler/invalid-generic-debug-node-tag-overflow.ll`,
  `test/Assembler/invalid-dicompileunit-emissionkind-bad.ll`,
  `test/Assembler/invalid-dicompileunit-language-overflow.ll` and the
  `invalid_dw_lang_2` / `invalid_dw_lname_2` parts of
  `test/Assembler/dicompileunit-invalid-language.ll`. The parser's
  `name_table_kind` table was also wrong while it was there — it read
  `Apple => 2, None => 3` where `DICompileUnit::DebugNameTableKind` is
  `GNU = 1, None = 2, Apple = 3` — unobservable only because nothing read the
  number until the `Max` check did.
- **Still open — the one residual.** `ChecksumKindField` is the single
  overload that is not an `MDUnsignedField`: it reports
  `invalid checksum kind '<Lex.getStrVal()>'` even when the *token kind* is
  wrong, so on any other token it quotes whatever the previous token left in
  `StrVal`. llvmkit's lexer keeps no such carry-over and cannot reproduce that
  string; `checksumkind: 1` answers `expected metadata field value` instead
  of `invalid checksum kind '…'`. The *verdict* matches — both reject — and no
  upstream `.ll` pins the text: `rg --no-ignore -l "invalid checksum kind"
  orig_cpp/…/llvm/test/Assembler` returns nothing. Pinned from llvmkit's side
  by `crates/llvmkit-asmparser/tests/parser_debug_metadata.rs::a_kind_family_rejects_every_token_but_an_integer_and_its_own_keyword`.

### 37. Intrinsic auto-declaration happens at the call site, not in `validateEndOfModule` — **NARROWED (W15)**

*parser — intrinsics* — crates/llvmkit-asmparser/src/ll_parser.rs (`resolve_direct_callee`, `resolve_forward_ref_globals`)

**Status (W15).** The two halves this entry was opened for are closed.

- The `intrinsic can only be used as callee` guard no longer fires at
  reference time. It is `resolve_forward_ref_globals`'s `llvm.` branch now —
  upstream's `for (Use &U : make_early_inc_range(Info.first->uses())) { auto
  *CB = dyn_cast<CallBase>(U.getUser()); if (!CB || !CB->isCallee(&U)) return
  error(Info.second, "intrinsic can only be used as callee"); }`, at step 7 of
  `validateEndOfModule`, so an undefined comdat now preempts it as upstream's
  order says it should. `test/Assembler/implicit-intrinsic-declaration-invalid2.ll`
  still passes, from the new site.
- The **rejects-valid** half is closed with it: an address-taken reference to
  a *declared* intrinsic parses, because the `declare` retires the forward
  reference before the sweep runs. `Verifier::visitFunction`'s guard answers
  instead, in upstream's words — `Invalid user of intrinsic instruction!` —
  over a port of `Function::hasAddressTaken` at the flag combination that
  caller passes, `IgnoreARCAttachedCall` included, so a
  `clang.arc.attachedcall` bundle operand reaches
  `Verifier::verifyAttachedCallBundle` rather than being refused first.
  `test/Verifier/intrinsic-addr-taken.ll` is vendored and driven
  (`crates/llvmkit-asmparser/tests/parser_intrinsics.rs::upstream_intrinsic_addr_taken_fixture_messages_match`,
  partial in one named place: its `@llvm.my.custom.intrinsic` declaration is
  gap **G3**), and all thirteen calls of `test/Verifier/operand-bundles.ll`
  now assert upstream's own `CHECK` text where seven used to assert this
  entry's parse rejection.

**Still open, and the reason the entry survives:** upstream auto-declares an
intrinsic *in that sweep*, from the call site's `FunctionType` via
`Intrinsic::getIntrinsicSignature` and `getOrInsertDeclaration`. llvmkit does
it at the call site itself, in `resolve_direct_callee`'s
`IntrinsicNameResolution::Known` arm, so no `llvm.`-prefixed name ever reaches
`global_forward_ref` from callee position and the sweep's declaration arm has
nothing to build. The two diagnostics that arm owns are therefore raised
elsewhere and worded differently: `unknown intrinsic '<name>'` becomes
`expected unknown intrinsic` at the call site, and `invalid intrinsic
signature` becomes `expected intrinsic signature mismatch`. Both are blocked
fixtures — `test/Assembler/implicit-intrinsic-declaration-invalid.ll` and
`-invalid3.ll`, gaps **G1** and **G3** in `docs/fixture-coverage.md`.

- **Why not now:** moving auto-declaration into the sweep means a call to an
  undeclared `llvm.` name must mint `getGlobalVal`'s untyped placeholder and
  leave the mangling suffix undecided until end of module, which is the same
  reshape `AutoUpgrade`'s unported call sites want (`docs/future-work.md`).
  Doing half of it would put the declaration in one place and the upgrade in
  another.

### 142. The verifier stops at its first failure, where `llvm::verifyModule` reports each failing entity

**Severity:** wrong-message (the verdict — broken or not — is the same; the
diagnostics are not: upstream prints one per failed `Check`, llvmkit returns
the first)
**Where:** `crates/llvmkit-ir/src/verifier.rs` — `Verifier::run`, which
propagates the first `Err` of every `visit_*` it calls with `?`, surfaced by
`Module::verify` / `Module::verify_borrowed` as one
`IrError::VerifierFailure`.

- **LLVM:** `llvm::verifyModule` (`lib/IR/Verifier.cpp`) runs
  `Verifier::verify(const Function &)` on every function and then
  `Verifier::verify()` on the module, OR-ing the results. A failed `Check` /
  `CheckDI` calls `Verifier::CheckFailed`, which writes the message to the
  stream and sets `Broken`; the macro then `return`s from that one visitor, and
  the walk goes on. So `llvm-as` prints a diagnostic for every failing entity,
  in the walk's order, and a `test/Verifier` fixture's `CHECK` lines pin all of
  them against one verification of the unmodified module.
- **llvmkit:** `Verifier::run` returns at the first failure, by design ("Stops
  on first error to keep `IrError` single-shot", its doc), so a module with
  several failures reports one.
- **Which one — the walk order differs too:** `llvm::verifyModule` verifies
  every function before it calls `Verifier::verify()`. That routine, in
  order, collects the `experimental.deoptimize` declarations, runs
  `verifyFrameRecoverIndices`, walks the global variables, aliases, ifuncs,
  named metadata and comdats, then checks the module flags, idents, command
  lines and errno-TBAA metadata, the compile units, and the deoptimize
  calling conventions. `Verifier::run` walks the global variables, the
  ifuncs and the module flags first and the functions last. So a module with a failing function and a
  failing global variable or ifunc reports the global's failure, where
  `llvm-as`'s first diagnostic is the function's. Entry 141 lists what the walk
  visits, not its order. Neither ifunc port below meets this: every function
  in both fixtures is valid.
- **What it forces:** a port of a fixture with several failing entities cannot
  compare upstream's output in one verification. The ifunc ports verify one
  ifunc per module — the fixture with every other ifunc line removed — and
  compare only the first `CHECK` against the fixture as written:
  `crates/llvmkit-asmparser/tests/parser_module_level.rs::upstream_ifunc_verifier_fixture_messages_match`
  (`test/Verifier/ifunc.ll`) and `::upstream_ifunc_opaque_verifier_fixture_messages_match`
  (`test/Verifier/ifunc-opaque.ll`). Earlier ports split their fixtures for
  the same reason and say so in their docs —
  `parser_calls.rs::upstream_callbr_label_constraint_fixture_messages_match`,
  `run_operand_bundle_fixture` (the kcfi and ptrauth operand-bundle ports),
  `upstream_musttail_invalid_fixture_messages_match`,
  `upstream_tailcc_musttail_fixture_messages_match`,
  `upstream_verifier_operand_bundles_fixture_messages_match` and
  `parser_eh_funclet.rs::upstream_invalid_eh_fixture_messages_match`
  (`git grep -n -i -E "accumulat|first failure" acf4502 -- crates/llvmkit-asmparser/tests`,
  read hit by hit) — and
  `parser_metadata.rs::upstream_invalid_range_metadata_fixture_messages_match`
  splits `test/Verifier/range-1.ll` per function without saying why.
- **Found:** 2026-10-04, by the second re-review of capability-typestate Task
  4, on the ifunc ports. Unrecorded before this entry, at `acf4502`:
  `rg -n -i "single-shot|first failure|first error|accumulat|stops at the first|CheckFailed|first finding|one finding|multi-error|every failure" docs/divergences.md`
  hits entries D15 (the parser), 101 (the lexer), 32 (the parser's own
  fail-fast choice) and 130, which cites `Verifier::CheckFailed` for how a
  value is rendered, not for whether the walk stops; `rg -n -i "verif.{0,80}(first|single|one at a time|stops)|(first|single|stops).{0,80}verif" docs/divergences.md docs/future-work.md docs/fixture-coverage.md`
  and `rg -n -i "verifyModule|verify_borrowed|Verifier::run" docs/divergences.md docs/future-work.md docs/fixture-coverage.md`
  find nothing about the verifier's stopping rule (the second search's
  `future-work.md` hit is the missing `test/Verifier` manifest; the third's are
  entry 141, then numbered 138, and `BrokenDebugInfo`). The walk order was found by the same
  review; `git grep -n -i -E "verifyModule|function walk|global-value walk|walk order|functions last|functions first" acf4502 -- docs/divergences.md docs/future-work.md docs/fixture-coverage.md`
  hits only the `BrokenDebugInfo` line.
- **Fix:** a collecting verifier — `Module::verify` answering every failure,
  in upstream's walk order, functions first — after which the per-ifunc and per-case ports
  become single comparisons of the unmodified fixture. That is a change to the
  verifier's result type and to every caller of it, so it is its own design
  step, not part of the ifunc port.

## Different printed bytes

The parser/printer contract is that printed output matches `AsmWriter.cpp` byte for byte and re-parses.

### 43. DWARF enumerations and `DIExpression` operands are stored as spellings, so numeric forms never normalise

*IR model* — crates/llvmkit-asmparser/src/ll_token.rs (the nine `Token::Dwarf*` variants), crates/llvmkit-asmparser/src/ll_parser.rs (`parse_metadata_field_value`'s enum arms), crates/llvmkit-ir/src/asm_writer.rs (`fmt_specialized_metadata_node`'s `MetadataFieldValue::Integer` and `MetadataFieldValue::Enum` arms)

- **LLVM:** `LLParser::parseDIExpressionBody` and the DWARF `MDField` readers convert each keyword to its integer encoding via `dwarf::getOperationEncoding` / `getAttributeEncoding` and store a `uint64_t`; the printer maps the encoding back to a name, so `!DIExpression(15)` prints as the `DW_OP_*` that 15 encodes and a numeric `DW_ATE`/`DW_TAG` field likewise comes back named.
- **llvmkit:** Every `DW_TAG_*`, `DW_ATE_*`, `DW_VIRTUALITY_*`, `DW_LANG_*`, `DW_LNAME_*`, `DW_CC_*`, `DW_OP_*`, `DW_MACINFO_*` and `DW_APPLE_ENUM_KIND_*` word is lexed into its own token carrying the full keyword text and collapses into `MetadataFieldValue::Enum(String)`; `DwarfExpressionOperand::Operation` keeps the source spelling. The AsmWriter writes `Enum(s)` verbatim and `Integer(v)` as the number, so a numerically-written operand or field prints back as a number where `llvm-dis` prints a name. Spellings are now *validated* against the same tables (W11: `invalid DWARF op '...'`, `invalid DWARF attribute encoding '...'`), so only the normalisation half is left.
- **Why:** Recorded: the typed form is the several-hundred-constant `Dwarf.def` family, which is generation work for `llvmkit-tablegen`'s sibling arm rather than a hand-typed enum, and it is deferred to the debug-info/metadata round-trip milestone where a consumer that needs the values exists. The entry also records that the parity plan had marked this closed at W11 from the plan's intent rather than from the tree, and that the false "closed" was worse than an open item.
- **Fix:** Generate the `Dwarf.def` tables (both directions) through `llvmkit-tablegen`, store the encoding rather than the name at parse time, and consult the reverse table at print time so a numeric input normalises to its keyword. The reverse tables are the half that closes the observable divergence; the forward tables already exist as `llvmkit_ir::dwarf::*` for W11's validation. The `DIExpression` half additionally needs `DIExpression::isValid()` modelled, because upstream only prints names when it holds and prints raw numbers otherwise — that consequence, and the IR-API construction hole behind it, is entry 67; do not restate it here.
- **Correction from verification:** Substantially accurate and still present; three corrections. (1) The illustrative example is wrong: 15 is DW_OP_const8s, which is not in DIExpression::isValid's accept switch, so writeDIExpression falls to its `else` branch and upstream prints `15` too. A correct example is `!DIExpression(6)` -> upstream `!DIExpression(DW_OP_deref)`, llvmkit `!DIExpression(6)`. The DWARF *field* half of the claim needs no such caveat: `!DIBasicType(tag: 15, ...)` prints `tag: 15` in llvmkit and `tag: DW_TAG_pointer_type` upstream. (2) The claim understates one case — writeDIExpression also special-cases DW_OP_LLVM_convert to print Op.getArg(1) through AttributeEncodingString, so `!DIExpression(DW_OP_LLVM_convert, 16, 5)` loses `DW_ATE_signed` in llvmkit even though the operation itself was written as a keyword. (3) The "Where" note and the code's own comments (metadata.rs:2094-2096, ll_parser.rs:5435-5436) say the Dwarf.def tables are unmodelled; that premise is now stale. crates/llvmkit-ir/src/dwarf.rs:621-675 generates BOTH directions (tag/tag_string, operation_encoding/operation_encoding_string, attribute_encoding_string, ...). The reverse lookups exist and are simply unused outside crates/llvmkit-asmparser/tests/dwarf_def_drift.rs — asm_writer.rs contains zero `dwarf::` references. Closing this divergence is now a matter of calling the existing encoding->name functions from the printer (and converting keyword->encoding at parse time), not of building tables.

<details><summary>Verification evidence</summary>

Cited lines all check out exactly. crates/llvmkit-asmparser/src/ll_token.rs:88-104 has the nine Token::Dwarf* variants, each holding &'src str keyword text. crates/llvmkit-asmparser/src/ll_parser.rs:5865-5881 collapses all nine (plus ChecksumKind/EmissionKind/NameTableKind/FixedPointKind) into MetadataFieldValue::Enum(value) carrying the source spelling. crates/llvmkit-asmparser/src/ll_parser.rs:5652-5666: the W11 `keyword` validator matches only the Enum arm and returns Ok(()) for everything else, so a numeric value passes through unconverted. crates/llvmkit-ir/src/asm_writer.rs:3477-3478 (Operation(name) => write_str(name), Literal(value) => write!("{value}")), :3491 (Integer(v) => write!("{v}")), :3497 (Enum(s) => write_str(s)); a grep for `dwarf::` across asm_writer.rs returns nothing. crates/llvmkit-ir/src/metadata.rs:2100-2107 defines DwarfExpressionOperand::Operation(String). Upstream lib/IR/AsmWriter.cpp MDFieldPrinter::printTag calls dwarf::TagString(N->getTag()) and falls back to the raw number only when empty; printDwarfEnum does the same via its Stringifier; writeDIExpression prints OperationEncodingString when N->isValid(). Upstream lib/AsmParser/LLParser.cpp parseMDField(..., DwarfTagField&) routes an APSInt token to MDUnsignedField and a keyword through dwarf::getTag, storing an unsigned either way. Empirical probe (temporary test, since deleted, run on +1.96.0 --release, parsing then re-printing): `!DIBasicType(tag: 15, name: "x")` printed back as `tag: 15`; `encoding: 4` printed back as `encoding: 4` (upstream DW_ATE_float, Dwarf.def HANDLE_DW_ATE(0x04, float)); `!DIExpression(6)` printed back as `!DIExpression(6)` where DW_OP_deref (0x6) IS in DIExpression::isValid's accept list so llvm-dis prints the name; `!DIExpression(DW_OP_LLVM_convert, 16, 5)` printed back with a bare `5`. Keyword-written forms round-trip correctly, so the divergence is exactly the normalisation half, as claimed. Unrelated side-finding from the same probe: unreferenced `!N = !DIExpression(...)` nodes are dropped entirely from format!("{module}") — they printed only once a named metadata node referenced them, whereas llvm-dis emits every numbered node.

</details>

### 45. DCE keeps calls and allocations upstream deletes

*passes* — crates/llvmkit-ir/src/dce.rs (`is_trivially_dead`); crates/llvmkit-ir/src/value.rs (`Value::has_uses`)

- **LLVM:** `wouldInstructionBeTriviallyDead` (`lib/Transforms/Utils/Local.cpp`) deletes unused `willReturn`+readnone calls, removable allocation-function calls, `free(null)`, and lifetime-only allocas; `Value::use_empty` ignores debug-record uses, and upstream salvages debug info instead of being blocked by it.
- **llvmkit:** `is_trivially_dead` returns `false` for every `Call` (and every `VaArg`/pad/atomic), so `DcePass` leaves all of the above in place; the module after DCE differs from upstream's. The record also notes `Value::has_uses` counts debug-record uses upstream ignores, which keeps further instructions alive.
- **Why:** Recorded: porting these needs faithful allocation-function and attribute modelling to avoid *over*-removal, which would be a miscompile if wrong, so the current DCE is deliberately conservative-but-safe.
- **Fix:** Model allocation functions and the relevant call attributes (`willReturn`, memory effects, the allocator family) first, then port `wouldInstructionBeTriviallyDead`'s call/alloca arms against them; separately, give `has_uses` a structural-uses-only sibling for the DCE query and port the debug-info salvage path rather than counting debug records as uses.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/dce.rs (is_trivially_dead, lines 48-75; cited 49-79 is minor line drift): the match arm `Some(Store | Fence | AtomicCmpXchg | AtomicRmw | Call | VaArg | LandingPad | CleanupPad | CatchPad) => false` makes every Call unconditionally not-dead — no attribute, willReturn, mayHaveSideEffects, allocation, free, or intrinsic inspection anywhere in the function. crates/llvmkit-ir/src/value.rs:570 is exactly `pub fn has_uses(self) -> bool { !self.data().use_list.borrow().is_empty() }`, and debug records do push into that list: instruction.rs:1535 and :1549 call `add_use(ValueUse::DebugRecord { .. })` via `record.for_each_value`, while `users()` (value.rs:553-559) filters DebugRecord/Metadata back out — so has_uses counts uses users() does not. Upstream Local.cpp::wouldInstructionBeTriviallyDead returns true for isRemovableAlloc(CB, TLI), any call with !mayHaveSideEffects (the willReturn+readnone case), stacksave/launder_invariant_group/allow_runtime_check/allow_ubsan_check, lifetime intrinsics whose alloca is used only by lifetime intrinsics, trivially-true assume, non-strict constrained FP, getFreedOperand on a null/undef constant (free(null)), and non-volatile loads from constant globals; isInstructionTriviallyDead gates on I->use_empty(). DebugProgramInstruction.h:296-298 confirms DbgVariableRecord reaches its value through DebugValueUser/ValueAsMetadata rather than a Use, so use_empty() is blind to debug records upstream. DCE.cpp::eliminateDeadCode calls salvageDebugInfo(*I) and salvageKnowledge(I) before erasing; `grep -rn salvage crates/llvmkit-ir/src/` returns zero hits. dce.rs is unmodified in the working tree (git status clean for it), so this is the committed state on dev. Two refinements: "lifetime-only allocas" is outcome-accurate but upstream states the rule on the lifetime intrinsic, after which the alloca falls out as an ordinary dead instruction; and the Load arm hides an unclaimed second gap — `load.is_unordered()` rejects an ordered atomic load from a constant global that upstream would delete. Blast radius is wider than the two cited sites: is_trivially_dead is also the predicate at inst_simplify.rs:51,62 and pass_context.rs:4009.

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

### 56. ppc_fp128 component pair stored mirrored from upstream

*IR model / ApFloat* — crates/llvmkit-ir/src/ap_float.rs (`ppc_words`); crates/llvmkit-ir/tests/ap_float_ppc_word_order.rs (module doc); crates/llvmkit-ir/tests/ap_float_upstream_predicates.rs::get_zero; crates/llvmkit-asmparser/tests/parser_hex_float_word_order.rs (module doc)

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
- **llvmkit:** the printer walks `metadata_store().nodes()` and numbers *every* non-`MDString` node in arena order. Before W13d that was invisible: the parser only interned nodes the text named, so every node was reachable. `UpgradeModuleFlags` breaks that — it replaces a flag tuple with a freshly interned one, and the superseded tuple stays in the arena and still prints. `test/Bitcode/upgrade-module-flag.ll` therefore prints its five upgraded flags plus three orphaned pre-upgrade tuples, where `llvm-dis` prints six nodes numbered `!0`–`!5`. The output still re-parses (a dead `!N = ...` definition is legal), so only the byte-for-byte half of the contract is broken. **W10 added a second producer**: `UpgradeNVVMAnnotations` rebuilds every `!nvvm.annotations` entry it did not fully consume, and `MetadataStore::push_tuple` never uniques (`intern_string` does — tuples do not), so a rebuilt entry with *identical contents* gets a fresh slot and both print. `parser_auto_upgrade.rs::a_repeated_nvvm_annotation_entry_is_visited_once` and `::malformed_nvvm_annotations_are_preserved_rather_than_upgraded` assert the fresh numbers and say why.
- **Why:** the fix is a real `SlotTracker` port, not a patch: upstream's numbering is *encounter* order over a specific traversal, so switching to reachability also changes which number every surviving node gets. That is a workstream of its own, and doing it inside an AutoUpgrade stage would have re-pinned every metadata-bearing expected output at the same time.
- **Fix:** port `SlotTracker::processModule` / `processFunction`'s metadata pre-pass and drive `metadata_slot_map` from it, then re-bless the metadata numbering in the corpus in the same commit. `crates/llvmkit-asmparser/tests/parser_auto_upgrade.rs` asserts on flag *contents* rather than `!N` numbering precisely because of this, and says so.

### 104. The metadata no-slot fallback prints an arena index, and a detached argument cannot be built — **NARROWED**

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
- **A detached `Argument` is not constructible**, so
  `unittests/IR/AsmWriterTest.cpp`'s `DebugPrintDetachedArgument` is ported
  with an *attached* unnamed value instead. The substitution is provably
  output-identical — `Value::print` sends an `Argument` to `printAsOperand`,
  which never calls `incorporateFunction`, so the local slot is -1 either way —
  and the substitution is stated in the test. `DebugPrintDetachedInstruction`
  is unported. It needs a `BinaryOperator` with no parent block, which this
  entry used to say llvmkit cannot hold; it can — `Instruction::detach_from_parent`
  returns an `Instruction<Detached>` that still has `set_metadata` and
  `to_erased` — so the recorded blocker does not hold. Whether such an
  instruction prints upstream's `<badref> = add i32 poison, poison, !<empty`
  has not been checked.
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

### 143. Every named struct in the type arena prints, not only the ones the module reaches

*printer* — crates/llvmkit-ir/src/asm_writer.rs (the named-struct type-identity block over `Module::iter_named_struct_ids`)

- **LLVM:** `AssemblyWriter::printTypeIdentities` prints the identified structs `TypePrinting::incorporateTypes` collected, and that runs `TypeFinder::run(M, false)`: a walk over global value types and initializers, aliases and aliasees, ifunc value types, and each function's type, attributes, arguments, instruction types, non-instruction operands and attached metadata, then named metadata. An identified struct the walk never reaches is never printed; the ones it reaches print in discovery order, and an unnamed one is numbered by that same order (`Type2Number`).
- **llvmkit:** the printer writes one line per named struct in the module's type arena, in creation order. A type that was interned and never used still prints. Observed 2026-10-06 on the working tree over `6c1efac`: `NestValue::build` — the `IrStruct` derive's builder — interns `%Nest` before it lifts its field operand, so when the lift refuses a value from another module nothing uses `%Nest`, yet the home module's printed text gains `%Nest = type { %Pair }`. `crates/llvmkit-ir/tests/cross_module_handles.rs::a_struct_schema_operand_rejects_a_value_from_another_module` interns `%Nest` before its no-mutation snapshot for that reason and says so. The output still re-parses (an unused type definition is legal), so only the byte-for-byte half of the contract is broken. Whether creation order and discovery order differ for any parsed fixture was not measured.
- **Why:** found while writing a regression test for an unrelated fix; a `TypeFinder` port is a change of its own and may re-pin expected printer output.
- **Fix:** port `TypeFinder::run` and drive the identity block — and the unnamed-struct numbering behind `anonymous_identified_struct_number` — from its result, re-blessing any expected output it moves in the same commit.

## Model gaps

A public query answers differently from its LLVM counterpart, or a structure LLVM has is missing.

**Triaged 2026-08-29.** Every entry in this band carries a verdict in its
heading. `**WILL NOT CLOSE**` marks a difference that follows from the project
charter (no code generation, no target backends, no linking) or from a settled
design decision, and each such entry states which — those are not pending work
and should not be re-triaged. `**NARROWED**` and `**FIXED**` mean what they do
elsewhere in this file. An entry with no marker is pending work whose
description was checked and stands. Three entries were deleted in that pass:
two the tree had already closed (`flags.ll`'s vector trunc functions, and
`dbg-record-invalid-5.ll`, both now carried by the corpus manifest), and one
that duplicated **D11** and **D13** — so the band is no longer a mix of open
defects and dead rows.

### 62. The phi known-bits arm recurses at `depth + 1` where upstream recurses at a fixed deep depth — **ONE DUPLICATE MERGED (W11); NARROWED (2026-08-29); NARROWED AGAIN (divergence W14)**

> **Divergence W14: the two portable halves are ported; only the cache-blocked half is
> left.** The global guard in `compute_known_bits_inner` now reads
> `depth >= query.max_depth()` and sits **after** the constant fast path, which
> is where upstream's `if (Depth == MaxAnalysisRecursionDepth) return;` sits
> relative to its `match(V, m_APInt(C))` arm ("All recursive calls that increase
> depth must come after this"). `phi_known_bits` now carries upstream's
> `Depth < MaxAnalysisRecursionDepth - 1` gate on the intersection loop.
>
> What remains is the *recursion depth under an incoming value*, and it is the
> one piece the entry always said was blocked: upstream recurses at the fixed
> `MaxAnalysisRecursionDepth - 1`, llvmkit at `depth + 1`, because
> `compute_known_bits_inner` memoizes on `(slot, query)` with no depth
> component — a fixed-deep entry would cache the weak answer and hand it to a
> later shallow query. It becomes portable when, and only when, the known-bits
> cache is depth-keyed; that is the trigger, not a backlog row.
>
> The entry's original witness was **false** and was struck at the 2026-08-29
> triage: upstream proves the same 60 leading zeros on `@test_udiv_neg`. No
> fixture in the tree exhibits the surviving difference, and none was written
> for it — exhibiting it needs a phi at shallow depth whose incoming value is an
> expression chain more than one operator deep.

Former entry **52**, "Phi known-bits recurses at `depth + 1` instead of
upstream's fixed deep depth", was this entry written a second time: same
routine (`phi_known_bits`), same coordinates, same rationale, same fix.
Deleted at W11 rather than left to be closed twice.

*analysis* — crates/llvmkit-ir/src/value_tracking.rs (`phi_known_bits`)

- **LLVM:** `computeKnownBitsFromOperator`'s `Instruction::PHI` arm gates its incoming-value intersection on `Depth < MaxAnalysisRecursionDepth - 1` and then recurses at that fixed depth, capping the search under an incoming value at one level so it does not spin around loops.
- **llvmkit:** the gate is ported; the recursion is at `depth + 1`, so llvmkit can prove strictly more about a shallow phi than upstream does.
- **Why:** `compute_known_bits_inner` memoizes on `(slot, query)` with no depth component, so entering an incoming value at a fixed deep depth would cache the weak answer computed there and hand it to a later shallow query. llvmkit also terminates by a different mechanism — the `stack` set rejects re-entering a value mid-computation — so the cap is not load-bearing for termination here.
- **Fix:** Leave it until the known-bits cache becomes depth-keyed. At that point add the depth component to the memo key and adopt upstream's fixed-depth recursion.
- **Divergence-W14 evidence for the two halves that closed:** `crates/llvmkit-asmparser/tests/value_tracking_depth.rs::an_operator_at_the_recursion_limit_is_not_walked` was watched failing on the pre-fix guard (it read 24 known leading zeros where upstream reads 0) and passes after; its sibling `a_constant_at_the_recursion_limit_still_reports_its_value` passes on both sides and says so in its own doc comment — it pins the *coupling* between the comparison and the fast path's placement, not a bug that was caught.

### 65. Two min/max matcher arms decline matches upstream accepts, rather than minting a constant — **WILL NOT CLOSE**

> **Triage 2026-08-29: settled design decision, not pending work.** Both arms
> would have to *mint a constant*, which is a module mutation an analysis has
> no business performing — the rule that also leaves
> `getFlippedStrictnessPredicateAndConstant` unported. Verified still present
> (`not_value` at `select_pattern.rs`, `look_through_cast_arm` in the same
> file), and both sites already say so in their own rustdoc. This entry closes
> only if the project decides a constant-minting analysis is acceptable, or
> gives these matchers a mutation-capable variant taking a module token — at
> which point all three land together. It is recorded so the weaker answers are
> not read as an oversight; do not re-triage it.

*analysis* — crates/llvmkit-ir/src/select_pattern.rs (`not_value`, `look_through_cast_arm`)

- **LLVM:** `getNotValue`'s second arm folds `~C` for a constant operand via `ConstantInt::get(V->getType(), ~*C)`, and `lookThroughCastConst` builds a casted constant with `ConstantExpr::getTrunc` / `ConstantFoldCastOperand` and checks it round-trips. Both let `matchSelectPattern` recognise shapes llvmkit's ports do not.
- **llvmkit:** `not_value` reports only the `xor X, -1` form, and `look_through_cast_arm` ports only the two arrangements that need no new value. A `not` written as a folded constant, and the cast arrangement needing a materialised constant, are not recognised — each forgoes a match rather than inventing one, so the select-pattern result is weaker than upstream's.
- **Why:** Recorded at both sites and in the backlog: minting a constant is a module mutation, which an analysis has no business performing. The same ground is why `getFlippedStrictnessPredicateAndConstant` is left unported entirely rather than split into a caller-builds-the-constant variant, which the entry argues would be a different function.
- **Fix:** Only closable by deciding that a constant-minting analysis is acceptable, or by giving these matchers a mutation-capable variant taking a module token — at which point `getFlippedStrictnessPredicateAndConstant` becomes portable too and the three should land together. Until then the divergence is the price of the no-mutation rule, and the sites already say so.

<details><summary>Verification evidence</summary>

Claim is accurate and the divergence is unchanged in the working tree (`git status` shows C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/select_pattern.rs clean; last touch was 15e4d87, a casing refactor, not a behavior change). 1) llvmkit `not_value`, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/select_pattern.rs:1210-1228 — the doc comment states outright that upstream's second arm "mints `ConstantInt::get(V->getType(), ~*C)`... so this reports only the `xor X, -1` form", and the body confirms it: it requires `InstructionKindData::Xor` with an all-ones operand (either order) and returns `None` otherwise. No constant arm. Upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ValueTracking.cpp, `getNotValue` (line ~8513) has exactly two arms: `m_Not(m_Value(NotV))` then `if (match(V, m_APInt(C))) return ConstantInt::get(V->getType(), ~(*C));`. The effect reaches `matchSelectPattern`: `not_value` feeds three call sites at select_pattern.rs:811-812, :828-829 (the port of `matchMinMax`'s two "look through not" blocks, ValueTracking.cpp:8546 and :8558) and :1029-1030 (the `lines_up` helper in the `matchMinMaxOfMinMax` port). A `not` spelled as a folded constant — e.g. `(X >s 5) ? ~X : -6` — matches upstream and returns `SPF_SMIN`, and returns no flavor in llvmkit. 2) llvmkit `look_through_cast_arm`, same file :1449-1493 — comment says the `lookThroughCastConst` arm "is not ported; the two shapes that need no new value are", and the body implements only (a) both arms the same cast from the same source type, and (b) `trunc` of a value the compare already widened via `sext`/`zext`. A constant `second` operand falls through to the trunc path and fails, so it is effectively declined. Single caller is select_pattern.rs:471, inside the cast-looking path of `match_select_pattern`. Upstream `lookThroughCast` (ValueTracking.cpp:9030) has three arms, and its middle one is `auto *C = dyn_cast<Constant>(V2); if (C) return lookThroughCastConst(CmpI, SrcTy, C, CastOp);`. `lookThroughCastConst` (:8935-9012) builds a new constant with `ConstantExpr::getTrunc` (ZExt/SExt cases) or `ConstantFoldCastOperand` (Trunc/FPTrunc/FPExt/FPToUI/FPToSI/UIToFP/SIToFP), then round-trips it: `CastedBack = ConstantFoldCastOperand(*CastOp, CastedTo, C->getType(), DL); if (CastedBack && CastedBack != C) return nullptr;` — precisely the "checks it round-trips" the claim describes. 3) The gap is a recorded deliberate choice, not an oversight: C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:832-837, "Two arms are deliberately narrower, both because they would need to mint a constant... Each forgoes a match rather than inventing one." That matches the claim's characterization. One nit, in llvmkit's comment rather than in the claim: select_pattern.rs:1452 calls `lookThroughCastConst` "Upstream's third arm", but it is the second arm in code order and item 2 of 3 in upstream's own doc comment (the trunc/`m_ZExtOrSExt` case is third). The claim's own wording does not repeat this, so it needs no correction.

</details>

### 66. Phi `remove_incoming` never self-erases an emptied phi — **WILL NOT CLOSE**

> **Triage 2026-08-29: settled design decision, not pending work.** Erasure in
> llvmkit goes through `Instruction::erase_from_parent`, which *consumes* the
> linear lifecycle handle so use-after-erase is a compile error; a `Copy`
> opcode handle cannot express that consumption, so self-erasure here would
> hand the caller a live handle to an erased instruction. The auto-erase
> behaviour does ship where it can be sound — inside the `ReshapeCfg` pass
> surface, whose edge edits RAUW an emptied phi with poison and erase it,
> mirroring `removePredecessor`. What is left is authoring ergonomics, not
> soundness: the verifier already flags the leftover through `check_phi`'s
> count guard. The `Fix:` bullet below records the two shapes a future
> ergonomic improvement could take; neither is a defect to schedule.

*IR model / Instructions* — crates/llvmkit-ir/src/instructions.rs (`phi_remove_incoming`, and the public doc on `PhiInst::remove_incoming`), crates/llvmkit-ir/src/phi_raw_tests/remove_incoming.rs (module doc)

- **LLVM:** `PHINode::removeIncomingValue(unsigned Idx, bool DeletePHIIfEmpty = true)` (`lib/IR/Instructions.cpp`) defaults to `true`: after `swap_remove`ing the entry, if the phi has no operands left it is `replaceAllUsesWith(PoisonValue)`d and `eraseFromParent()`ed. Callers that drop a predecessor rely on that.
- **llvmkit:** `phi_remove_incoming` (the shared body of all four phi handles' `remove_incoming`) mirrors the `swap_remove` exactly but stops there. Removing the last incoming leaves a live phi with zero incomings — a node that prints as `%p = phi i32` with no `[ … ]` pairs. `LLParser::parsePHI` **does** re-read that (it breaks cleanly out of its loop on the missing `[` and builds a zero-incoming PHINode); what rejects it is the *Verifier*, and only in a reachable block that has predecessors — a zero-incoming phi in an unreachable block is legal upstream, and llvmkit pins both halves. The caller is expected to finish the job.
- **Why:** Recorded on `instructions.rs::phi_remove_incoming`: erasure in llvmkit goes through `Instruction::erase_from_parent`, which *consumes* the linear lifecycle handle so use-after-erase is a compile error; a `Copy` opcode handle cannot express that consumption, so self-erasure here would hand the caller a live handle to an erased instruction. The auto-erase behaviour does ship where it can be sound — inside the `ReshapeCfg` pass surface, whose edge edits RAUW an emptied phi with poison and erase it, mirroring `removePredecessor`.
- **Fix:** Either (a) give the erased phi surface a linear, consuming variant — `remove_incoming_or_erase(self, …) -> Either<Value, ErasedPhi>` taking the phi by value so the handle cannot outlive the erase; or (b) return a `PhiEmptied` marker the caller must destructure, making the leftover unignorable. (b) is cheaper and preserves the `Copy` handle for the common case. The verifier already flags the leftover through `check_phi`'s count guard — a block whose other predecessors survive now has more predecessors than the emptied phi has entries — so the gap is authoring ergonomics, not soundness.
- **Correction from verification:** The core divergence is accurate and unchanged. One supporting detail is wrong: the claim (and llvmkit's own comments) say a bracket-less phi is something "LLParser::parsePHI cannot re-read". LLParser::parsePHI actually accepts it — on the first loop iteration `if (Lex.getKind() != lltok::lsquare) break;` exits cleanly and it builds a PHINode with zero incomings, returning InstNormal. The rejection is the Verifier's ("PHI node entries do not match predecessors"), not the parser's, and only for a reachable block that has predecessors; a zero-incoming phi in an unreachable block is legal upstream and llvmkit pins both halves (zero_incoming_phi.rs:298 rejected-in-reachable, :339 accepted-in-unreachable). llvmkit's own in-tree comments at zero_incoming_phi.rs:162 and :269 ("the shape LLVM's LL parser rejects") carry the same imprecision and should be reworded to blame the verifier. Everything else in the claim — the swap_remove-only body, the missing DeletePHIIfEmpty half, the four shared call sites, the caller-finishes-the-job contract, and all three cited line numbers — is correct.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/instructions.rs:1430 `fn phi_remove_incoming` — body is bounds-check, `incoming.swap_remove(slot)`, remove one `ValueUse::Instruction(phi_id)` from the removed value's use_list, return the Value. No zero-count branch, no erase. Its doc states outright: "The `DeletePHIIfEmpty` half of upstream's contract is deliberately **not** mirrored", because `Instruction::erase_from_parent` consumes a linear handle and a `Copy` opcode handle cannot express that consumption. Four call sites at instructions.rs:1648/1894/2106/2247 confirm "all four phi handles". Public doc at instructions.rs:1637: "Unlike upstream's default `DeletePHIIfEmpty = true`, removing the last incoming leaves the (now unprintable) phi in place". Upstream orig_cpp/.../llvm/lib/IR/Instructions.cpp:152-156 in PHINode::removeIncomingValue: `if (getNumOperands() == 0 && DeletePHIIfEmpty) { replaceAllUsesWith(PoisonValue::get(getType())); eraseFromParent(); }`; the default is declared in Instructions.h:2791-2792 as `bool DeletePHIIfEmpty = true`. Still-present proof: crates/llvmkit-ir/src/phi_raw_tests/remove_incoming.rs (final single-incoming case) removes the only incoming, asserts `incoming_count() == 0`, rediscovers the phi in block "join", and asserts `still_there` with the message "an emptied phi must not be self-erased"; the module doc at lines 13-20 names it a deliberate divergence and points at the ReshapeCfg edge edits (zero_incoming_phi.rs:142/238 assert no bracket-less `= phi` is printed after those) as where auto-erase does ship. Parser check: orig_cpp/.../llvm/lib/AsmParser/LLParser.cpp:8314-8355 shows parsePHI breaking out on a non-lsquare first token and constructing a zero-incoming PHINode rather than erroring.

</details>

### 67. `DIExpression::isValid()` is ported, but no verifier walk calls it — **NARROWED (2026-08-29); NARROWED AGAIN (divergence W14)**

> **Divergence W14: the model and printer halves are closed.**
> `llvmkit_ir::metadata::expression_is_valid` ports `DIExpression::isValid`
> element for element, `expression_operand_size` ports
> `DIExpression::ExprOperand::getSize`, and `DwarfExpressionOperand::element` /
> `expression_elements` recover upstream's `uint64_t` element array from the
> stored spellings through the drift-locked `crate::dwarf` tables. The
> AsmWriter now carries `writeDIExpression`'s branch on `isValid()`: a valid
> expression prints operation names, an invalid one prints the raw elements.
> The two stale in-code comments (`ll_parser.rs`'s `parse_di_expression_body`
> and `metadata.rs`'s `DwarfExpressionOperand`), which both claimed the
> `Dwarf.def` tables were unmodelled and that an unrecognised `DW_OP` round-trips
> rather than being rejected, are corrected in place.
>
> Two residuals, both narrower than what was here:
>
> 1. **Nothing calls it.** Upstream's `Verifier::visitDIExpression` is
>    `CheckDI(N.isValid(), "invalid expression", &N)`, and llvmkit's verifier
>    has no metadata-node walk at all to hang that on
>    (`grep -n "MetadataKind::Specialized" crates/llvmkit-ir/src/verifier.rs`
>    returns one hit, inside the module-flag `append` check). So a
>    `DIExpression` **built through the IR API** can still hold an invalid
>    operand list and print straight back out — it just now prints upstream's
>    raw-number form when it does.
> 2. **An unresolvable spelling has no raw form.** `Operation(String)` is a
>    public tuple variant, so the IR API can store a `DW_OP` spelling no
>    `Dwarf.def` table carries. `expression_elements` answers `None` for it and
>    the writer falls back to printing the spelling, because there is no
>    `uint64_t` to print. The parser cannot produce one.
>
> The *parse* path is not a gap and never was, and the reverse-normalisation
> half — a numerically written element printing back as a number — is entry
> **43**, not this one.

*IR model / metadata, verifier* — crates/llvmkit-ir/src/metadata.rs (`expression_is_valid`), crates/llvmkit-ir/src/asm_writer.rs (the `writeDIExpression` branch), crates/llvmkit-ir/src/verifier.rs (no metadata walk)

- **LLVM:** `Verifier::visitDIExpression` is `CheckDI(N.isValid(), "invalid expression", &N)`, reached from the metadata-node walk `Verifier::visitMDNode` drives.
- **llvmkit:** `expression_is_valid` exists and the printer branches on it, but no verifier path asks it, so an invalid expression built through the IR API verifies.
- **Why:** llvmkit's verifier has no `visitMDNode` equivalent; adding one is the whole metadata-verification subsystem, not this check.
- **Fix:** Port `Verifier::visitMDNode`'s dispatch far enough to reach the specialized-node visitors, then add `visitDIExpression`. `expression_is_valid` is already the predicate it needs.
- **Divergence-W14 fixture:** `crates/llvmkit-ir/tests/di_expression.rs::is_valid` is `TEST_F(DIExpressionTest, isValid)` from `llvm/unittests/IR/MetadataTest.cpp`, ported verbatim, and it passes.

### 69. Shuffle mask transforms model the IR alphabet only — **WILL NOT CLOSE**

> **Triage 2026-08-29: charter, not pending work.** The wider alphabet the
> transforms would need (`SM_SentinelZero == -2`) exists only for SelectionDAG
> and the X86 backend, and **code generation and target backends are out of
> scope**. Verified still present and verified *unrepresentable*, not merely
> unspelled: `ShuffleMaskElem::from_encoded` maps every negative to `Poison`,
> so `{-2,-2,-3,-3}` cannot be built at all. The only live action is
> bookkeeping — the three upstream `VectorUtilsTest.cpp` assertions with no
> llvmkit spelling are listed in `tests/vector_utils_masks.rs` and should carry
> `UPSTREAM.md` rows marked "no llvmkit spelling" so the coverage gap reads as
> deliberate. Do not re-triage the transforms themselves.

*analysis / VectorUtils* — crates/llvmkit-ir/src/vector_utils.rs (module header, and `widen_shuffle_mask_elements`'s doc)

- **LLVM:** `widenShuffleMaskElts`, `narrowShuffleMaskElts` and friends read mask elements as raw `int`s, so the same functions serve both the IR alphabet `{lane, poison}` and the wider one SelectionDAG and the X86 backend use, where `SM_SentinelZero` is `-2`. The widening rule is "negatives must be *equal* across a widened group", which distinguishes `-1` from `-2`.
- **llvmkit:** The transforms take `&[ShuffleMaskElem]`, which has only `Lane(n)` and `Poison`, so the equal-negatives rule collapses to "all poison". Three upstream assertions in `VectorUtilsTest.cpp` have no llvmkit spelling as a result.
- **Why:** Recorded in the module header as permanent, not pending: "code generation and target backends are out of scope", so no mask llvmkit can hold distinguishes the two negatives and the difference is unobservable in-tree. `widen_shuffle_mask_elements` repeats it at its site, and `tests/vector_utils_masks.rs` records the three unportable assertions.
- **Fix:** No fix warranted — the narrowing follows from the project charter, and the recorded reason is verified: `ShuffleMaskElem` genuinely has no sentinel-zero variant. The only action is to keep the three unportable upstream assertions visible: they are already listed in `tests/vector_utils_masks.rs`, and they should carry `UPSTREAM.md` rows marked "no llvmkit spelling" so the coverage gap reads as deliberate rather than missing.
- **Correction from verification:** Accurate as described; only the second line citation is stale. The site note is on `widen_shuffle_mask_elements` (crates/llvmkit-ir/src/vector_utils.rs:851-860), not :208-213 — that range is now inside `is_splat_value`'s doc comment. The module-header citation (:34-43) is exact. Two further precisions: the alphabet restriction is enforced at the type level *and* at decode time — `ShuffleMaskElem::from_encoded` (crates/llvmkit-ir/src/instr_types.rs:2607) maps every negative to `Poison`, so `{-2,-2,-3,-3}` is not merely unspellable but unrepresentable; and the two widen fixtures are re-asserted a second time through `getShuffleMaskWithWidestElts` upstream, so "three assertions" counts distinct fixtures rather than distinct EXPECT lines (llvmkit's own test header says the same and flags the duplication).

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/instr_types.rs:2575-2613 — `pub enum ShuffleMaskElem { Lane(u32), Poison }`, with `from_encoded(value: i64)` collapsing any negative (`u32::try_from` Err) to `Poison`. crates/llvmkit-ir/src/vector_utils.rs — all five transforms (`narrow_shuffle_mask_elements` :804, `widen_shuffle_mask_elements` :861, `widen_shuffle_mask_elements_in_pairs` :926, `scale_shuffle_mask_elements` :966, `shuffle_mask_with_widest_elements` :999) take `&[ShuffleMaskElem]`; the widen negative arm is `ShuffleMaskElem::Poison => { if !slice.iter().all(|e| *e == ShuffleMaskElem::Poison) { return None } scaled.push(ShuffleMaskElem::Poison) }` (:890-895), i.e. "all poison". Module header :34-43 states the narrowing verbatim; widen's doc :851-860 names `SM_SentinelZero` and the two absent fixtures. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/VectorUtils.cpp, `llvm::widenShuffleMaskElts(int Scale, ArrayRef<int>, SmallVectorImpl<int>&)` — `if (SliceFront < 0) { if (!all_equal(MaskSlice)) return false; ScaledMask.push_back(SliceFront); }`, propagating the sentinel value itself. orig_cpp/.../llvm/unittests/Analysis/VectorUtilsTest.cpp — `narrowShuffleMaskElts(1, {3,2,0,-2}, …)` expecting `{3,2,0,-2}`; `EXPECT_FALSE(widenShuffleMaskElts(2, {-1,-2,-1,-1}, …))`; `EXPECT_TRUE(widenShuffleMaskElts(2, {-2,-2,-3,-3}, …))`; plus the same two masks re-run through `getShuffleMaskWithWidestElts`. crates/llvmkit-ir/tests/vector_utils_masks.rs:10-35 records exactly those three as having no llvmkit spelling, and its `mask()` helper routes through `from_encoded`, so a sentinel fixture would silently collapse to poison rather than test anything.

</details>

### 76. Unported APInt/APFloat upstream tests — **NARROWED (2026-08-29); NARROWED AGAIN (divergence W14)**

> **Divergence W14: the three wrong headers and the undocumented gap are closed.**
> `TEST(APIntTest, clmul)` is ported
> (`ap_int_upstream_ops.rs::carryless_mul`, `UPSTREAM.md` row marked `port`).
> `ap_int_upstream.rs`'s header no longer calls GCD, clmul and the rotate family
> "APIs llvmkit does not have"; `ap_float_upstream_predicates.rs`'s no longer
> says `FPClassTest` is unmodelled (it is `llvmkit_ir::fp_class::FpClassTest`,
> and the `classify()` assertions live beside it in `fp_class.rs`);
> `ap_float_from_string.rs`'s no longer cites a `docs/future-work.md` entry that
> did not exist, no longer counts `makeNaN` among the unported fixtures, and the
> backlog entry it claimed has now been written.
>
> What is left is what the 2026-08-29 triage already found still true, and every
> row of it carries a recorded reason in `docs/future-work.md`: the
> `Float8*` / `Float6*` / `Float4E2M1FN` / `FloatTF32` semantics and their
> `APFloatTest.cpp` rows (`ApFloatSemantics` has seven variants);
> `TEST(APIntTest, SolveQuadraticEquationWrap)` and the `tc*` primitives;
> `nearestLogBase2`'s final `APInt(UINT32_MAX, 0)` row, dropped in place; and
> the two `APFloat` string fixtures `fromHexadecimalString` /
> `fromStringSpecials`, which `ap_float_from_string.rs` covers with
> llvmkit-chosen boundary spellings rather than verbatim.

*ADT ports* — crates/llvmkit-ir/tests/{ap_int_upstream_ops.rs, ap_float_upstream_predicates.rs, ap_float_from_string.rs}

- **LLVM:** `llvm/unittests/ADT/APFloatTest.cpp` covers `Float8*`, `Float6*`, `Float4E2M1FN` and `FloatTF32` semantics and the two string-parsing fixtures; `APIntTest.cpp` covers `SolveQuadraticEquationWrap` and the `tc*` word-level primitives.
- **llvmkit:** those are not ported.
- **Why:** each is recorded in `docs/future-work.md` with its reason — the semantics are genuine model additions, the `tc*` family is an internal API llvmkit deliberately does not expose, `SolveQuadraticEquationWrap` belongs with a SCEV port, and the two string fixtures are ordinary pending work.
- **Fix:** the two `APFloat` string fixtures are the cheap half and need only be written down; the semantics are the expensive half and each is independent.

### 78. An already-defined `!DIAssignID !N` attachment survives here; on a reading of `LLParser.cpp` it is dropped upstream — **NARROWED (2026-08-29); FIXTURES ADDED (divergence W14)**

> **The recorded RAUW half will not close — structural, and nothing observable
> is lost.** llvmkit resolves metadata forward references by reserve-then-fill
> on a stable `MetadataId` (`resolve_md_slot` → `metadata_reserve`,
> `define_md_slot` → `metadata_set`), so there are no temporary MDNodes and no
> `replaceAllUsesWith` anywhere in the parser. Upstream needs the DIAssignID
> special case *only* because `Instruction::setMetadata(MD_DIAssignID, …)`
> maintains `LLVMContextImpl::AssignmentIDToInstrs` via
> `updateDIAssignIDMapping`, a side map a temporary RAUW cannot fix —
> `LLParser.h` says so in as many words. llvmkit has no `AssignmentIDToInstrs`
> equivalent
> (`grep -rn "AssignmentIDToInstrs\|assignment_id_to_instr\|assignment_tracking" crates/`
> returns only prose — the module doc of `tests/di_assign_id_attachment.rs`
> saying llvmkit models no such map), so the machinery has nothing to protect. The gap becomes
> real only if llvmkit ports assignment tracking; that is the trigger.
>
> **The residual is an opposite-direction difference, and its upstream half is
> still a *reading*.** `parseInstructionMetadata` pushes to
> `TempDIAssignIDAttachments[N]` *unconditionally* when `MDK == MD_DIAssignID`
> and never calls `Inst.setMetadata` on that path, while the drain in
> `parseStandaloneMetadata` runs only inside the `ForwardRefMDNodes.find` hit.
> On that reading, an attachment whose `!N` was **already defined earlier in the
> file** is pushed, never drained, and silently dropped by `llvm-as`. **No
> `llvm-as` has been run against it.** llvmkit has no `MD_DIAssignID` branch at
> all — `skip_trailing_metadata` calls `inst.set_metadata(..)` for every kind —
> so it attaches the node in both orderings.
>
> **Divergence W14:** both orderings are now pinned by fixtures
> (`crates/llvmkit-asmparser/tests/di_assign_id_attachment.rs`), which assert
> **llvmkit's** answer only and say in their own doc comments that the upstream
> half is unexecuted. That closes the "unexercised in the tree in either
> arrangement" half of the old `Fix:` line. What it does not do is settle which
> way to resolve the difference: this file's precedent (entry 92,
> `riscv_vls_cc`) is that an upstream bug is reproduced deliberately, and that
> decision needs a real `llvm-as` run first.

*metadata parser* — crates/llvmkit-asmparser/src/ll_parser.rs (`skip_trailing_metadata`, the only attachment path; the `missing 'distinct', required for !DIAssignID()` guard is the only DIAssignID site)

- **LLVM:** on the reading above, `llvm-as` keeps a `!DIAssignID` instruction attachment only in the forward-referenced ordering.
- **llvmkit:** keeps it in both.
- **Why:** llvmkit's forward-reference model does not need the special case, so no branch was ever written; the difference in the already-defined ordering is a consequence, not a decision.
- **Fix:** run `llvm-as | llvm-dis` on the already-defined ordering and confirm or refute the reading. If it is confirmed, decide between reproducing the drop (this file's precedent for an upstream bug) and keeping llvmkit's attachment as a documented improvement, then update the fixture that pins the chosen answer.

### 80. `attr_kind_for_keyword` is a hand-written table, not generated from `Attributes.td` — **NARROWED (W13)**

> **Triage 2026-08-29: pending work, description verified and unchanged.**
> `attr_kind_for_keyword` is still a transcribed `match`, `crates/llvmkit-asmparser`
> still has no `build.rs`, and the only TableGen generation in the workspace is
> `llvmkit-ir/build.rs` → `llvmkit-tablegen`, whose sole root input is
> `llvm/IR/Intrinsics.td`. Scope note the entry understates: upstream emits its
> attribute *lexer* keywords from the same `Attributes.inc` via
> `KEYWORD(DISPLAY_NAME)`, so `ll_lexer/keywords.rs` is the second hand-written
> half of the same generated table.
>
> **Correction from W13, and the narrowing that follows.** The triage's scope
> note is right that `ll_lexer/keywords.rs` is a second hand-written half, and
> wrong to leave the impression that it is a second *unguarded* one. It is not:
> `attribute_td_drift.rs::parser_accepts` builds real `.ll` text
> (`attributes #0 = { <spelling> }`, a `define` parameter, a `define` return)
> and runs `parse_dynamic` on it, so every probe goes through the lexer's
> keyword table and the parser's `attr_kind_for_keyword` **end to end**. A
> keyword missing from `keywords.rs` fails to lex and fails the probe exactly
> as a missing parser arm does. Read the eight tests, not the file name:
>
> ```
> grep -c '^#\[test\]' crates/llvmkit-asmparser/tests/attribute_td_drift.rs
> ```
>
> Between them they pin the *set* (`no_unmodeled_attribute_is_silently_missing`
> against an empty `NOT_YET_MODELED`), the *positions* each attribute declares
> (`every_attribute_declares_a_position`), the *print order*
> (`a_function_attribute_list_prints_in_attributes_td_enum_order`, W11's
> addition), the reverse direction
> (`a_lexed_keyword_with_no_td_def_does_not_print_as_itself`), and the typed
> readers for the string-valued kinds. A wrong keyword→`AttrKind` mapping
> prints as the wrong attribute and fails the order test.
>
> **So the residual is a maintainability property, not a behavioural one:** the
> two tables are transcribed rather than generated, and the guard proves they
> currently agree with `Attributes.td` without removing the transcription. The
> obstacle to generation is recorded in `attribute_td_drift.rs`'s own module
> header and is real — the generated artifact would have to be keyed by
> `Keyword`, and `Keyword` is a hand-written enum over a deliberate subset, so
> emitting the table means emitting part of a 700-variant enum. A design that
> generates a `&[(&str, AttrKind)]` table and keys it off the lexer's existing
> string→`Keyword` map would sidestep that; nobody has costed it. **Not
> scheduled** — it buys no behaviour the guard does not already hold.

*parser — attributes* — crates/llvmkit-asmparser/src/ll_parser.rs (`attr_kind_for_keyword`), crates/llvmkit-ir/src/attributes.rs, crates/llvmkit-asmparser/tests/attribute_td_drift.rs, crates/llvmkit-asmparser/tablegen/llvm-22.1.4/include/llvm/IR/Attributes.td

- **LLVM:** `LLParser::tokenToAttribute` is generated from `llvm/IR/Attributes.td`, so a new attribute cannot be silently missing.
- **llvmkit:** The keyword→`AttrKind` mapping is a hand list. `attribute_td_drift.rs` stands in for generation and has caught five separate holes, but the table itself is still transcribed.
- **Why:** Recorded as carried out of W5: "Given how much that guard has caught, generating the table would be strictly better — carry it." Also note the guard's own reader had four bugs; a measurement taken with a broken reader looks authoritative and is not.
- **Fix:** Extend `llvmkit-tablegen` to emit the keyword→kind table from the vendored `Attributes.td` and keep the drift test as the cross-check (five-step method, steps 1 and 5).

<details><summary>Verification evidence</summary>

Claim #85 is REAL and STILL PRESENT; the description is accurate on every material point. llvmkit (hand-written, confirmed today): - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:9172 — `fn attr_kind_for_keyword(keyword: Keyword) -> Option<AttrKind>` is a literal transcribed `match` of ~90 `Keyword::X => AttrKind::Y` arms terminating in `_ => return None`. Lines 9216-9220 even carry a comment describing the arms added later as "upstream's `tokenToAttribute`" that "was missing them, so each parsed as 'not an attribute'". - No `build.rs` exists in crates/llvmkit-asmparser (dir listing: Cargo.toml, LICENSE, README.md, examples, src, tablegen, tests). Nothing generates this table. - The only TableGen generation in the workspace is llvmkit-ir/build.rs -> llvmkit-tablegen, whose sole root input is `const ROOT_TD: &str = "llvm/IR/Intrinsics.td"` (crates/llvmkit-tablegen/src/lib.rs:64). Intrinsics only; `Attributes.td` is never fed to the generator. - `AttrKind` is itself a hand-written enum at crates/llvmkit-ir/src/attributes.rs:285. Upstream (generated, confirmed in the vendored tree): - orig_cpp/.../llvm/lib/AsmParser/LLParser.cpp:1545 `static Attribute::AttrKind tokenToAttribute(lltok::Kind Kind)` whose switch body is `#define GET_ATTR_NAMES` / `#define ATTRIBUTE_ENUM(ENUM_NAME, DISPLAY_NAME) case lltok::kw_##DISPLAY_NAME: return Attribute::ENUM_NAME;` / `#include "llvm/IR/Attributes.inc"`, with `default: return Attribute::None`. Called from LLParser.cpp:1736 and :2026. The drift guard exists exactly as described: - crates/llvmkit-asmparser/tests/attribute_td_drift.rs `include_str!`s ../tablegen/llvm-22.1.4/include/llvm/IR/Attributes.td (present, 20191 bytes, tracked). `NOT_YET_MODELED` is `&[]`. Its module header states generation was deliberately rejected ("llvmkit deliberately models a subset ... would mean generating part of the 700-variant `Keyword` enum") and that the test "gives the same guarantee without that cost". - The "five holes" figure is consistent with the file's own doc comments, which record: Milestone 0's ~21 missing keywords; multi-line `def`s misread (dereferenceable_or_null, speculative_load_hardening, nocreateundeforpoison); the `split_once(':')` spacing bug dropping hot / disable_sanitizer_instrumentation / allockind; anonymous `def : CompatRuleStrAttr` defs inventing an attribute named `isEqual`; and positionless defs passing the probe vacuously (now guarded by `every_attribute_declares_a_position`). Scope note (the claim is if anything understated, not wrong): the divergence covers the lexer as well as the parser. Upstream's LLLexer.cpp:701 emits its attribute keywords from the same `Attributes.inc` via `KEYWORD(DISPLAY_NAME)`, whereas llvmkit's crates/llvmkit-asmparser/src/ll_lexer/keywords.rs is a hand-written byte-string match (e.g. line 334 `b"nocreateundeforpoison" => kw(Nocreateundeforpoison)`, line 395 `b"zeroext" => kw(Zeroext)`). So both generated halves upstream are hand-transcribed here. Working-tree caveat: ll_parser.rs is modified vs HEAD (173+/165- per `git diff --stat`), but that diff is line-ending/reflow churn in this region; the hand-written table is present in the working tree as read.

</details>

### 82. Three copies of the aggregate index walk — **BEHAVIOURAL HALF FIXED (2026-08-29); NARROWED to the consolidation**

> **Triage 2026-08-29: the "all three currently agree" line was false, and the
> disagreement was a rejects-valid defect. Fixed in this commit.**
> `verifier.rs::walk_aggregate_path` narrowed the array length with
> `u32::try_from(n).unwrap_or(u32::MAX)` before comparing, while
> `indexed_aggregate_type` and `walk_aggregate_for_builder` both widen the
> index to `u64` and compare against the true length — which is what upstream
> does, since `ExtractValueInst::getIndexedType`'s `if (Index >=
> AT->getNumElements())` promotes an `unsigned` index to a `uint64_t` count.
> Probed: `extractvalue [4294967296 x i8] %a, 4294967295` parsed and built,
> then failed verification with `Invalid ExtractValueInst operands! (index
> 4294967295 >= 4294967295)` — a wrong count as well as a wrong verdict.
> Reachable straight from `.ll` text, since an index-list entry is a `uint32`
> and an array length is a `uint64`. The walk now compares at 64 bits and
> carries a `u64` count.
>
> Nothing had pinned the three walks against each other — that gap was the
> reason they could drift, and it is closed by
> `builder_aggregate_vector.rs::the_three_aggregate_index_walks_agree_at_the_u32_boundary`,
> which drives the same boundary through all three.
>
> **Residual: the consolidation itself**, which is why this entry stays open.
> The law is a guard, not a fix; three implementations of one routine are still
> three, and the next diagnostic added to one of them can diverge again.

*IR model* — crates/llvmkit-ir/src/instructions.rs (`indexed_aggregate_type`), crates/llvmkit-ir/src/ir_builder.rs (`walk_aggregate_for_builder`), crates/llvmkit-ir/src/verifier.rs (`walk_aggregate_path`)

- **LLVM:** `ExtractValueInst::getIndexedType` is one routine, used by the parser, the builder and the verifier alike.
- **llvmkit:** Three implementations: the public port `llvmkit_ir::indexed_aggregate_type` (added W9c for `invalid indices for {extract,insert}value`), `ir_builder.rs::walk_aggregate_for_builder`, and `verifier.rs::walk_aggregate_path` (which additionally distinguishes *why* the walk failed via `AggWalkErr`). They agreed on every input as of 2026-08-29 — but only after the fix recorded above, and only because a test now says they must.
- **Why:** Recorded in docs/future-work.md (W9c): not urgent, but "a predicate with three implementations is one diagnostic away from having three behaviours". That prediction came true; the entry's own "not urgent" and "all three currently agree" were what let it. Consolidation is not a pure deletion because the verifier needs a richer return type than `Option`.
- **Fix:** Give the public port a result type that carries the failure reason, then delete the two private near-copies and re-point their callers.
- **Correction from verification (2026-08-29, superseded by the fix above):** the closing sub-claim "All three currently agree" had a reachable counterexample. `verifier.rs::walk_aggregate_path` narrowed the array length with `u32::try_from(*n).unwrap_or(u32::MAX)` before comparing, while `indexed_aggregate_type` and `walk_aggregate_for_builder` both widen the index to `u64` and compare against the true `u64` length. For an array longer than `u32::MAX` indexed at exactly `u32::MAX` — e.g. `extractvalue [4294967296 x i8] %a, 4294967295`, which the `.ll` grammar can spell since index-list entries are uint32 — the public port and the builder accepted and the verifier reported `OutOfRange` with a wrong `count`. That half is fixed; the consolidation is not.

<details><summary>Verification evidence</summary>

Upstream, C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Instructions.cpp — `ExtractValueInst::getIndexedType(Type *Agg, ArrayRef<unsigned> Idxs)` is a single ~20-line loop (array: `Index >= AT->getNumElements()` -> nullptr; struct: `Index >= ST->getNumElements()` -> nullptr; anything else -> nullptr). Its callers confirm the "one routine, three consumers" half of the claim: the parser calls it in `LLParser::parseExtractValue` / `parseInsertValue` (LLParser.cpp), the builder path calls it from the `ExtractValueInst` constructor via `checkGEPType(getIndexedType(...))` in include/llvm/IR/Instructions.h plus the `InsertValueInst::init` assert in Instructions.cpp, and the verifier calls it in `Verifier::visitExtractValueInst` / `visitInsertValueInst` (Verifier.cpp). llvmkit, all three copies read in the working tree (none touched by the uncommitted diff — `git diff -U0` on instructions.rs/ir_builder.rs shows no hunk mentioning `aggregate` or `indexed`): - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/instructions.rs:4011 `pub fn indexed_aggregate_type` — walks `AnyTypeEnum`, array arm `u64::from(index) >= array.len()`, struct arm `structure.field_type(...)` (derived_types.rs:1113, which returns `None` for an opaque body). Its own rustdoc at instructions.rs:4008 admits "Two private near-copies of this walk exist, in `ir_builder.rs` and `verifier.rs`". Only callers are the parser: ll_parser.rs:12655 and :12722, producing "invalid indices for extractvalue"/"...insertvalue". - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/ir_builder.rs:10528 `fn walk_aggregate_for_builder` — same walk over `TypeData`, returns `IrError::AggregateIndexOutOfRange` / `IrError::TypeMismatch`; called at ir_builder.rs:3423 (`extract_value_dyn`), :3490 (`insert_value_dyn`), :3520. - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/verifier.rs:3525 `fn walk_aggregate_path` with `enum AggWalkErr { NotAggregate, OutOfRange }` at :3520; called at verifier.rs:1537 (`check_extract_value`) and :1577 (`check_insert_value`). Not a fourth copy: `constant_fold.rs::constant_fold_extract_value_instruction` / `..._insert_value_instruction` walk constant *elements*, mirroring upstream's `ConstantFoldExtractValueInstruction`, not the type walk. The ledger entry is still open and unedited at C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:76-94, including the "Not urgent: all three currently agree" line that my refinement above corrects.

</details>

### 86. `AllocationType` models the four keyword values, not upstream's OR-able set — **WILL NOT CLOSE (until bitcode)**

> **Triage 2026-08-29: no observable divergence through any surface llvmkit
> has.** An ORed value cannot arrive through `.ll` text in either direction:
> `LLParser::parseAllocType` accepts only the four keywords and llvmkit's
> `parse_alloc_type` matches it arm-for-arm, and
> `AssemblyWriter::printFunctionSummary`'s `AllocTypeName` lambda
> `llvm_unreachable`s on anything but 0/1/2/4. The only producer is bitcode,
> which llvmkit does not have. Verified still present (`AllocationType` has
> exactly four variants). Revisit **with the bitcode reader**, at which point
> `AllocationType` becomes a masked newtype rather than an enum — that is the
> trigger, not a backlog row. One thing to fix whenever this is next touched:
> the doc comment on `AllocationType` states the power-of-two OR rationale
> while the type offers no OR and cannot hold the result.

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

### 120. `Verifier::visitGetElementPtrInst`'s result-element-type check has nothing to compare — **WILL NOT CLOSE**

> **Triage 2026-08-29: settled design decision, and in llvmkit's favour.** A
> stored-but-stale `ResultElementType` is a state upstream can reach and
> llvmkit cannot represent: `GepInstData` has exactly four fields
> (`source_ty`, `ptr`, `indices`, `flags`) with no stored result element type,
> and the null half of upstream's field is spelled instead as a rejection at
> construction. The address-space `Check` is vacuous for the same reason. Both
> are recorded so the missing `Check`s are not read as oversights. The check
> comes back if a future change ever stores a result element type; that is the
> trigger, not a backlog row.

*verifier / IR model* — crates/llvmkit-ir/src/verifier.rs (`check_gep`); crates/llvmkit-ir/src/instr_types.rs (`GepInstData`)

- **LLVM:** `GetElementPtrInst` stores `ResultElementType`, initialised to `getIndexedType(PointeeType, IdxList)` — which may be null — and `Verifier::visitGetElementPtrInst` asks `Check(PtrTy && GEP.getResultElementType() == ElTy, "GEP is not of right type for indices!")`, re-deriving `ElTy` from the source type and the index list and comparing the two.
- **llvmkit:** `GepInstData` stores `source_ty`, the operand slots and the no-wrap flags, and nothing else. There is no stored result element type, so there is nothing for a re-derived `ElTy` to disagree with, and the `PtrTy` half is all that is portable — `check_gep` ports that half as `VerifierRule::GepNonPointerResult`. The null half of upstream's field is spelled instead as a rejection at construction: `IrBuilder::gep_inner` and `IrBuilder::gep_erased` both return `IrError::GepInvalidIndices` when `getIndexedType` walks off the source type, which is why the Verifier check has nothing left to catch.
- **Why:** Structural, and in llvmkit's favour: a stored-but-stale `ResultElementType` is a state upstream can reach and llvmkit cannot represent. Recorded so the missing `Check` is not read as an oversight.
- **Fix:** None wanted. If a future change ever stores a result element type (say, to speed an analysis), the check comes back with it.

- **And vacuous rather than missing, in the same routine:** `Check(GEP.getAddressSpace() == PtrTy->getAddressSpace(), "GEP address space doesn't match type")`. `GetElementPtrInst::getAddressSpace` is the *pointer operand's* address space, and its own comment says "this is always the same as the pointer operand's address space". In llvmkit both `gep_inner` and `gep_erased` derive the result type from the base operand by `getGEPReturnType`, and `Instruction::replace_all_uses_with` refuses a replacement whose type slot differs, so the two address spaces are the same interned slot by construction. Recorded as vacuous, not fixed.

<details><summary>Verification evidence (2026-08-21)</summary>

Upstream read at the vendored tag `llvmorg-22.1.4` (the repo commit does not pin `orig_cpp/`, which is gitignored): `llvm/lib/IR/Verifier.cpp::Verifier::visitGetElementPtrInst` — the eight statements, in order, are the base `isa<PointerType>` check, `isSized`, the struct-scalable `Check`, the `all_of` integer-index `Check`, `ElTy = getIndexedType(...)` plus `Check(ElTy, ...)`, `PtrTy = dyn_cast<PointerType>(GEP.getType()->getScalarType())` plus the two-conjunct `Check`, the vector block, and the trailing address-space `Check`. Every one of those is emitted by `check_gep` except the second conjunct of the `PtrTy` `Check` and the address-space `Check`; the struct-scalable one was unported when this entry was first written and was ported in the fix round that narrowed it (`VerifierRule::GepScalableStructSource`, locked by `verifier_basic.rs::verify_gep_into_scalable_struct_fails`). `llvm/include/llvm/IR/Instructions.h` — `GetElementPtrInst`'s constructor initialiser list is `SourceElementType(PointeeType), ResultElementType(getIndexedType(PointeeType, IdxList))`, and `getAddressSpace()` forwards to `getPointerAddressSpace()`, which is `getPointerOperandType()->getPointerAddressSpace()`, under the comment quoted above. llvmkit side, at this commit: `crates/llvmkit-ir/src/instr_types.rs::GepInstData` has exactly four fields (`source_ty`, `ptr`, `indices`, `flags`); `crates/llvmkit-ir/src/ir_builder.rs::gep_return_type` derives the result type from the base operand's type alone; `crates/llvmkit-ir/src/instruction.rs::replace_all_uses_with` rejects a replacement with `IrError::TypeMismatch` when `new_value.ty != self.ty`, and no public operand setter exists (`grep -n 'pub fn set_operand\|pub fn replace_operand' crates/llvmkit-ir/src/*.rs` returns nothing), so an operand cannot be swapped for one in another address space after construction.

</details>

### 134. `CallBase::getCalledFunction` is ported without its function-type check

**Severity:** wrong-output, rejects-valid, accepts-invalid (derived by reading;
no fixture exhibits any of the four — a hypothesis until one does)
**Where:** four sites that each spell `getCalledFunction` as "the callee value
is a function": `crates/llvmkit-ir/src/verifier.rs` (the
`Direct call cannot have a ptrauth bundle` check, and `visitCallBrInst`'s
`Callbr: indirect function / invalid signature`),
`crates/llvmkit-ir/src/value_tracking.rs` (the `returned` lookup's
`getCalledFunction()` arm), `crates/llvmkit-ir/src/speculation.rs`
(`isSafeToSpeculativelyExecute`'s `Call` arm, before `callee_is_speculatable`).

- **LLVM:** `CallBase::getCalledFunction` (`IR/InstrTypes.h`) is
  `if (auto *F = dyn_cast_or_null<Function>(getCalledOperand())) if
  (F->getValueType() == getFunctionType()) return F; return nullptr;` — a call
  whose own function type differs from the callee's is *not* direct.
- **llvmkit:** every site checks only that the callee is a function, and two
  of the verifier comments state the plain `dyn_cast` as upstream's
  definition. On a call through a mismatched signature llvmkit therefore
  rejects a `ptrauth` bundle upstream allows, accepts a `callbr` upstream
  rejects, reads a `returned` parameter upstream ignores, and calls a
  `speculatable` callee hoistable where upstream says no.
- **Found:** 2026-09-19, Task 24 fix round 3, while porting
  `Function::isSpeculatable`. `rg -n "getCalledFunction" crates/llvmkit-ir/src/`
  names three of the sites (the `returned` arm in `value_tracking.rs`, the two
  verifier checks) plus two doc comments that decide nothing
  (`CallInst::classify_callee`, `IrBuilder::indirect_callbr_with_config`); the
  speculation site does not spell the name and was found by reading
  `isSafeToSpeculativelyExecute`'s `Call` arm against its port.
- **Fix:** one crate port of `getCalledFunction` (callee is a function *and*
  its signature slot equals the call's `fn_ty`), called from all four sites,
  with a mismatched-signature fixture per consequence. The port exists since
  Task 1b of the crash-proofing plan: `crates/llvmkit-ir/src/instr_types.rs`'s
  `called_function`. `CallBaseParts`'s `has_return_attribute`,
  `has_parameter_attribute`, `range` and `return_no_fp_class` (the ports of
  `CallBase::hasRetAttr`, `paramHasAttr`, `getRange` and `getRetNoFPClass`)
  call it. The type check is pinned through `has_return_attribute` by
  `crates/llvmkit-asmparser/tests/speculation.rs::can_create_poison_reads_every_annotation_and_arm`'s
  `call i64 @h(i32 %x)` row. The four sites above do not call it yet, and this
  entry stays open for them.

### 136. A renamed function keeps its intrinsic identity only when its signature matches the name

**Severity:** model-gap (exhibited by
`crates/llvmkit-ir/tests/global_value_names.rs::a_rename_onto_an_intrinsic_name_with_another_signature_stores_no_identity`,
which pins llvmkit's answer and flips when this closes)
**Where:** `crates/llvmkit-ir/src/function.rs` —
`FunctionValue::update_after_name_change`, the port of
`Function::updateAfterNameChange` that `Value::set_name` runs after renaming
a function.

- **LLVM:** `Function::updateAfterNameChange` (`IR/Function.cpp`) sets
  `IntID = Intrinsic::lookupIntrinsicID(Name)` for any `llvm.`-prefixed name —
  from the name alone. A function renamed to `llvm.trap` with the wrong
  signature answers `getIntrinsicID() == Intrinsic::trap`, and the verifier
  then judges the signature. `lookupIntrinsicID` also accepts a prefix match
  for an overloaded intrinsic, so the `<name>.invalid` that
  `getOrInsertIntrinsicDeclarationImpl` gives a stale declaration of one
  (`llvm.memcpy.p0.p0.i64.invalid`) still answers `Intrinsic::memcpy`.
- **llvmkit:** the stored identity (`IntrinsicFunctionData`) carries the
  overload types the signature determines, so it can only be built from a
  matching signature: `update_after_name_change` stores it when
  `intrinsic_descriptor_from_signature(name, signature)` succeeds and stores
  nothing otherwise — a mismatched signature, or an overload suffix that does
  not demangle, as `.invalid` does not. A function renamed to an intrinsic's
  name with a mismatched signature answers `intrinsic_id() == None` where
  upstream answers the id. The verifier still rejects that function by name
  (`Verifier::verify_intrinsic_function` resolves every `llvm.`-prefixed name
  and compares its signature), so the difference shows in `intrinsic_id`,
  `intrinsic_descriptor` and the queries built on them, not in a verdict.
  `is_intrinsic` reads the `llvm.` prefix, as upstream's `isIntrinsic` does,
  and is not one of them.
- **Found:** 2026-10-04, porting `Value::setName`'s `GlobalValue` arm —
  `updateAfterNameChange` is the first llvmkit route that names an existing
  function after an intrinsic; creation refuses such names (entry 137) and the
  parser checks the signature before it declares one. The rename port's review
  fix added the test above and the `.invalid` arm, a second route here.
- **Fix:** an identity that can hold an id without overload types — the
  name-only half of `IntID` — beside the descriptor, or a decision recorded
  here that a mismatched intrinsic name is refused at rename as at creation.

### 137. Creating a global value refuses a taken name, and a function refuses an `llvm.` name

**Severity:** model-gap (derived by reading; no test exhibits the upstream
answer — a hypothesis until one does)
**Where:** `crates/llvmkit-ir/src/module.rs` — `Module::declare_function` and
`ModuleCore::add_function_checked` (`DuplicateFunctionName`,
`reject_reserved_intrinsic_name`), and `ModuleCore::install_global_variable`,
`install_global_alias`, `install_global_ifunc` (`DuplicateGlobalName`).

- **LLVM:** a function, global variable, alias or ifunc enters its module's
  list as it is constructed (`getFunctionList().push_back(this)` in
  `Function`'s constructor, `Module::insertGlobalVariable`, `insertAlias`,
  `insertIFunc`), and `SymbolTableListTraits::addNodeToList` then runs
  `ValueSymbolTable::reinsertValue` (`IR/SymbolTableListTraitsImpl.h`,
  `IR/ValueSymbolTable.cpp`), which inserts the name or, under a name
  another global value holds, takes the uniqued name
  `ValueSymbolTable::makeUniqueName` gives it (`name.1`). The `GlobalValue`
  constructor's own `setName(Name)` (`IR/GlobalValue.h`) runs before the value
  has a parent, so `getSymTab` hands it no table and only the raw name is
  stored. `Function::Create` accepts an `llvm.`-prefixed name: `Function`'s
  constructor records `HasLLVMReservedName`, `Value::setName` sets `IntID`, and
  the verifier judges the declaration.
- **llvmkit:** each creation path checks `ModuleCore::named_value` first and
  returns `DuplicateFunctionName` / `DuplicateGlobalName` instead of
  uniquing; a function whose name resolves to a known or unknown intrinsic is
  refused with `ReservedIntrinsicName` / `UnknownIntrinsic` — intrinsic
  declarations come only from `get_or_insert_intrinsic_declaration`. Renaming
  does neither: `set_name` uniques a clash and accepts an `llvm.` name, as
  upstream does, so the two entry points answer the same request differently.
  The `.ll` parser is unaffected: `LLParser` reports a redefinition before it
  would create a second global.
- **Found:** 2026-10-04, porting `Value::setName`'s `GlobalValue` arm, which
  made creation and renaming share one table.
  `rg -n "ReservedIntrinsicName|DuplicateGlobalName|DuplicateFunctionName|makeUniqueName"
  docs/divergences.md docs/future-work.md docs/fixture-coverage.md` returned
  nothing before this entry. Corrected the same day (the port's review, M1):
  this entry first credited the uniquing to the constructor's `setName`.
- **Fix:** drop the pre-check and let creation unique the clash through
  `set_global_value_name` — the table step `reinsertValue` takes for the
  module's table, whose `MaxNameSize` is -1 — reading the final name back off
  the value, as upstream's list insertion does — a breaking change for any
  caller that relies on the refusal — or record the refusal as deliberate
  hardening.

### 144. The parser's forward-reference placeholder is a bare constant, not a `GlobalVariable`

**Severity:** model-gap
**Where:** `crates/llvmkit-ir/src/constant.rs` — `ConstantData::ForwardRefPlaceholder`,
answered by `Constant::kind` as `ConstantKind::ForwardReference`

- **LLVM:** a global referenced before its definition, and a `blockaddress` or
  `dso_local_equivalent` the parser must defer, are each held by a placeholder
  `GlobalVariable` — `createGlobalFwdRef` builds it, and the two deferrals in
  `LLParser::parseValID` do too — so `isa<GlobalValue>` answers `true` for it
  until the real value replaces it.
- **llvmkit:** the placeholder is `ConstantData::ForwardRefPlaceholder`, a
  constant with no global behind it — no name, no linkage, no `GlobalValue`
  handle to narrow to. `Constant::kind` therefore answers
  `ConstantKind::ForwardReference` rather than `GlobalValue`, and
  `is_constant_data` answers `false`, as upstream does for its placeholder. The
  difference is observable only while a parse is in flight — every placeholder
  is replaced before a parse succeeds — or through the public
  `Module::forward_ref_value_placeholder`.
- **Found:** 2026-10-10, writing `Constant::kind` (capability plan Task 6.1).
  `git grep -nE "ForwardRefPlaceholder|forward.reference placeholder|createGlobalFwdRef" 1109ee0 -- docs/divergences.md docs/future-work.md`
  (the commit before this entry) matches only D2's passing mention of the
  `no_cfi` deferral. The same task
  corrected `is_constant_data`'s doc comment, which said upstream's stand-in is
  "a `GlobalValue` or instruction".
- **Fix:** mint the placeholder as a global (a `GlobalVariable` with no
  initializer, removed when replaced), so `kind()` answers `GlobalValue` as
  upstream does — or keep the bare constant as deliberate and say so here.

## Coverage, tooling and provenance

Nothing changes for a well-formed module; these are gaps in what is measured, guarded or recorded.

### 92. Bare riscv_vls_cc swallows the following token (upstream bug, reproduced deliberately)

*parser (calling conventions)* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_riscv_vls_calling_conv`), pinned by crates/llvmkit-asmparser/tests/calling_conv_drift.rs::a_bare_riscv_vls_cc_swallows_the_next_token

- **LLVM:** `parseOptionalCallingConv`'s `kw_riscv_vls_cc` arm consumes its own keyword and, finding no `(`, `break`s to the switch tail — which consumes a second token. So `declare riscv_vls_cc void @f()` loses its return type. This is upstream's actual behaviour, not its intent.
- **llvmkit:** Reproduced exactly and pinned: `declare riscv_vls_cc void @f()` is an error, and `declare riscv_vls_cc void void @f()` parses at the default ABI_VLEN of 128.
- **Why:** Recorded, and the reasoning is explicit: unreachable from printed IR (`printCallingConv` always writes the parameterised form), reproduced because "the contract is upstream's behaviour", noted in `docs/future-work.md`, and "if upstream fixes it, this test is what says so".
- **Fix:** No action while tracking LLVM 22.1.4 — this is the parity-correct answer. Re-check on the next vendored-tree bump; the test is designed to fail if upstream drops the fallthrough.

<details><summary>Verification evidence</summary>

Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp, LLParser::parseOptionalCallingConv — the kw_riscv_vls_cc arm alone calls Lex.Lex() for itself, sets CC = RISCV_VLSCall_128, and on `if (!EatIfPresent(lltok::lparen)) break;` falls to the switch's common tail `Lex.Lex(); return false;`, consuming a second token. Every other arm reaches that tail unconsumed. llvmkit: crates/llvmkit-asmparser/src/ll_parser.rs:13168 dispatches RiscvVlsCc to parse_riscv_vls_calling_conv (13200-13206) with an early return that skips the shared bump at 13180; inside, after expect_keyword and a failed eat_punct(LParen) it does `self.bump()?` with the comment "The upstream double-`Lex.Lex()` described above" and returns RISCV_VLS_CALL_128. Test: crates/llvmkit-asmparser/tests/calling_conv_drift.rs lines 172-195 (a_bare_riscv_vls_cc_swallows_the_next_token) asserts `declare riscv_vls_cc void @f()` is an error and `declare riscv_vls_cc void void @f()` parses and prints containing riscv_vls_cc(128) — the cited line range is exact. Ran `cargo +1.96.0 test --release -p llvmkit-asmparser --test calling_conv_drift`: 6 passed, 0 failed, that test included. Also recorded at docs/future-work.md:180-188; landed in commit 4cfeea6 (LLParser parity W6). Note this is a divergence from upstream's intent, not its behaviour — llvmkit is bug-for-bug identical by design.

</details>

### 96. `test/Assembler/thinlto-vtable-summary2.ll` needs a module-summary *analysis*, so it is unported — **NARROWED**

*tests / fixtures* — crates/llvmkit-asmparser/tests/parser_summary.rs

- **LLVM:** the fixture's `RUN` line is `opt %s -S -module-summary | FileCheck %s`, which *generates* a summary index from the module's `!type` metadata; the input carries no `^N` block at all (`grep -cE "^\s*\^"` over it returns 0), and the expected `^2 = gv: (name: "_ZTS1A"` / `^6 = typeidCompatibleVTable: (name: "_ZTS1A")` are that pass's output.
- **llvmkit:** unported. llvmkit has the summary *model* and its parser but no module-summary analysis to produce one, so there is nothing to run the fixture against. Every other `test/Assembler` summary fixture is ported in `parser_summary.rs`.
- **Why:** recorded in `docs/future-work.md` with the same rationale — this is the analysis, not the parser.
- **Fix:** N/A until a module-summary analysis exists. A `CHECK` block that is a pipeline's output is not `AsmWriter`'s, so porting it before then would be porting `opt`.
- **Narrowed 2026-08-29 (wave 12).** This entry also claimed `invalid-name.ll` and `invalid-name2.ll` were unportable, and that was **wrong at the time it was re-verified**: both were checked in verbatim, NUL bytes and all, by the lexer-parity wave (commit `3a6d379`). `parser_corpus_manifest.txt` drives them at `status=reject | error=NUL character is not allowed in names`, `docs/fixture-coverage.md` classifies both `ported`, and `diff <(od -c <checked-in>) <(od -c <vendored>)` is empty for each. The entry's evidence block asserted the opposite from a repo-wide grep that returned nothing — the `-a` trap this repo's own `claims-and-counts` skill records, on the two fixtures it names as the example.

### 97. The parity ledger's `covered` column proves a message *exists*, never that a path reaches it — **NARROWED**

*measurement / ledger* — `~/.claude/plans/llparser-parity-ledger.md`, `~/.claude/plans/llparser-tools/ledger_v2.py` (both outside the repository)

- **LLVM:** n/a — measurement infrastructure for the LLParser parity program, which is complete.
- **llvmkit:** `ledger_v2.py` harvests string literals out of the parser sources and ticks an upstream message when its text is found. Text existing is not a path reaching it. W7 is the standing example: `redefinition of global '@x'` had a variant, a `Display` and a unit test, and never fired for a global.
- **Why it cannot be fixed in the tool.** A source-text search cannot decide reachability, and neither can the obvious refinements. Skipping comments and `#[cfg(test)]` modules — the two false-positive classes W14d and its verification probed for — would not have caught W7, because that message *was* constructed, just never for the kind that mattered. Nor would "the message is asserted by some test": W7's had one. The only oracle for "a path reaches this diagnostic" is an input that reaches it, which is `parser_corpus_manifest.txt`'s job and not the ledger's.
- **Where the reachability oracle actually lives, since wave 12.** Every `status=reject` corpus row asserts that llvmkit rejects the fixture *with* upstream's message, and — for every row whose own fixture pins a column — at upstream's caret. That is a per-message reachability proof for the messages the vendored tree can adjudicate, and it caught two anchor defects the text-only form could not see (see `docs/future-work.md`'s caret-anchor item).
- **What is still blind:** a message with no vendored fixture. Nothing proves such a message is reachable, in the ledger or anywhere else; the ledger's tick for it means only that the text is in the sources.
- **Do not quote the ledger's scoreboard.** Its header figure was measured with the pre-W14d extractor and disagrees with the tool at HEAD. Regenerate before using any count: `python ledger_v2.py <orig_cpp/llvm-project-llvmorg-22.1.4> <workspace root, NOT src> <ledger path>` — passing `src` yields `present=0` silently.

### 98. `UPSTREAM.md` provenance debt: the frozen backlog — **NARROWED**

*tests / provenance* — UPSTREAM.md, crates/llvmkit-ir/tests/fixtures/upstream_provenance_debt.txt

- **LLVM:** n/a — D11 house law: every `#[test]` cites its upstream source and gets an `UPSTREAM.md` row in the same commit.
- **llvmkit:** a residue of tests carry no row. The debt is inherited from the type-safety and pass-API programs and sits mostly in `llvmkit-ir` rather than in the parser crates, whose waves add rows per commit.
- **No figure is recorded here, and none in `UPSTREAM.md`'s header either.** This entry carried one, then a recount of it, then a correction to the recount, and the header carried three at once that did not agree with each other. The list is now checked in, so the population is the file: `crates/llvmkit-ir/tests/fixtures/upstream_provenance_debt.txt`, whose non-comment lines are exactly the unrowed tests at the commit you are reading.
- **Mechanically checked, both directions.** `crates/llvmkit-ir/tests/upstream_registry_drift.rs` fails if a row names a file absent from the tree, or a test its cited file does not define (2026-08-22, eleven such rows repaired); and since wave 12 (2026-08-29) `every_test_carries_a_registry_row_or_a_line_in_the_frozen_debt_list` fails if a `#[test]` has neither a row nor a debt line, **and** if a debt line no longer names an unrowed test. The backlog can therefore shrink but not grow, and cannot rot in either direction. Both halves were watched failing before the list was frozen.
- **Residual, and it is the whole backlog:** the listed tests still have no provenance. Clearing a line means naming a real upstream source, or saying in the row itself that the test is llvmkit-specific and why — the ratchet stops the debt growing, it does not pay it.
- **Still unchecked:** the upstream citation in the second column. A row may name a source that does not cover what the test asserts; that is `docs/fixture-coverage.md`'s phantom-citation finding, and it is a review judgement rather than a mechanical one.

### 133. The index-carrying parser names its module `<string>` by inference, not by port

*parser (entry points)* — `crates/llvmkit-asmparser/src/parser.rs`, `parse_assembly_with_index_and_config`

- **LLVM:** there is no string-taking index entry point to copy an identifier from. `parseAssemblyWithIndex` takes a `MemoryBufferRef` and uses whatever identifier the caller put on the buffer; the file-taking wrappers use the filename. The only two hardcoded identifiers in `Parser.cpp` are `parseAssemblyString`'s `MemoryBufferRef F(AsmString, "<string>")` and the matching one in `parseSummaryIndexAssemblyString`.
- **llvmkit:** `parse_assembly_with_index_and_config` takes bytes rather than a buffer-plus-identifier, so it must supply a name, and it supplies `"<string>"` — upstream's convention for a buffer with no name of its own, applied to an entry point upstream never gives that convention to.
- **Why:** the alternative is a `parse_assembly_with_index_and_name` twin so the caller supplies the identifier the way `MemoryBufferRef` does. That is the shape Task 5 of the `ParseError` surface-narrowing program chose for the plain closure family (`parse_assembly_with_name`), and it would make this a port rather than an inference. It was not done here because the index family has no such twin today and adding one is a wider API change than the divergence it closes.
- **Fix:** either add the `_with_name` twin and let this default fall out of it, or leave the inference and keep this row. Not urgent: the identifier is observable only through the `; ModuleID` comment, and choosing `"<string>"` makes that line agree with upstream's for the analogous string-parse path even though upstream has no such path here.

<details><summary>Verification evidence</summary>

Derived at `8d764d0`.

- Upstream's hardcoded identifiers: `grep -n '"<string>"' orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/Parser.cpp` — two hits, in `parseAssemblyString` and `parseSummaryIndexAssemblyString`; neither is on an index-with-buffer path.
- `parseAssemblyWithIndex`'s identifier source: it constructs from `F.getBufferIdentifier()`, so the name is the caller's, not a constant.
- llvmkit's site and its own rustdoc, which already records the inference in-place: `grep -n 'inference rather than' crates/llvmkit-asmparser/src/parser.rs`.
- This row exists because that rustdoc is the *only* record. An audit of behavioural differences reads this file, not the source comments.

</details>

## Checked and found already closed

The entries below did not survive verification. They are kept so nobody
re-derives them; each was a recorded belief that the tree no longer supports.

### A parsed module's summary index is not attached to the module, so `Display` never emits `^N`

*IR model* — crates/llvmkit-asmparser/src/ll_parser.rs (`ParsedModule::summary_index`, filled at both of `Parser::parse_module_with_config`'s returns); crates/llvmkit-ir/src/module_summary_index.rs (the model and its `Display`)

- **Was claimed:** The index comes back separately in `ParsedModule::summary_index`, is not attached to the `Module`, and `format!("{module}")` therefore never emits `^N`. Reproducing `llvm-dis` is two calls: print the module, then print the index.
- **Verdict:** NOT A DIVERGENCE — the llvmkit-side facts are true, but the upstream half of the claim is false, so this is parity, not a gap. Accurate as written: `ParsedModule::summary_index` (crates/llvmkit-asmparser/src/ll_parser.rs, the `ParsedModule` struct) returns the index separately, it is not attached to `Module`, `format!("{module}")` never emits `^N`, and reproducing `llvm-dis` is two calls. Wrong as written: "`llvm-dis` hands `AssemblyWriter` both the module and its index, and `printModuleSummaryIndex` runs after `printModule`, so one print emits the module followed by its `^N` entries." Upstream does exactly what llvmkit does. `AssemblyWriter` has two disjoint constructors — one takes `const Module *` and leaves `TheIndex` null, the other takes `const ModuleSummaryIndex *` and leaves `TheModule` null — and is never handed both. `Module::print` calls only `W.printModule(this)`; `printModuleSummaryIndex` is reached only from `ModuleSummaryIndex::print`, which builds its own second `SlotTracker` and its own second `AssemblyWriter`, and opens with `assert(TheIndex)`. `llvm-dis` makes two consecutive calls on the same stream (`M->print(...)` then `Index->print(...)`), as does `llvm-as`. `llvm::Module` owns no index at all (Module.h has only a forward declaration), and the parser entry point returns the pair `ParsedModuleAndIndex { Mod, Index }` — the same split shape llvmkit's `ParsedModule` mirrors. So the correct statement is: llvmkit's two-call print and separately-returned index MATCH upstream LLVM 22.1.4. The `docs/future-work.md` entry "Summary index — printing a module and its index is two calls" should be rewritten (it was, 2026-10-11) — its premise sentence ("`llvm-dis` prints a module and its summary index together because `AssemblyWriter` is handed both") is factually incorrect, and its closing suggestion that "attaching the index to the module" is an open design question is backwards: attaching it would be the divergence from upstream, not the fix.

<details><summary>Verification evidence</summary>

llvmkit side, confirmed present: crates/llvmkit-asmparser/src/ll_parser.rs:419-422 defines `pub struct ParsedModule<'ctx, B: ModuleBrand> { pub slot_mapping: SlotMapping<'ctx, B>, pub summary_index: Option<ModuleSummaryIndex> }`, filled by `self.summary_index.take()` at :1428 and :1482. crates/llvmkit-ir/src/module.rs:4685-4687 — `impl<B, S> Display for Module<B, S>` body is `crate::asm_writer::fmt_module(f, &self.core)`, with no index parameter; a grep for `ModuleSummaryIndex` across crates/llvmkit-ir hits only asm_writer.rs and module_summary_index.rs, never module.rs, so `ModuleCore` cannot hold one. Upstream, which refutes the claim: orig_cpp/.../llvm/tools/llvm-dis/llvm-dis.cpp:264-272 is `if (!DontPrint) { if (M) { M->removeDebugIntrinsicDeclarations(); M->print(Out->os(), Annotator.get(), false); } if (Index) Index->print(Out->os()); }` — two separate print calls, not one. lib/IR/AsmWriter.cpp:5077-5084 `Module::print` constructs `SlotTracker SlotTable(this)` and `AssemblyWriter W(OS, SlotTable, this, AAW, IsForDebug, ShouldPreserveUseListOrder)` then calls only `W.printModule(this)`. lib/IR/AsmWriter.cpp:5437-5442 `ModuleSummaryIndex::print` constructs a distinct `SlotTracker SlotTable(this)` and `AssemblyWriter W(OS, SlotTable, this, IsForDebug)` then calls `W.printModuleSummaryIndex()`. The two ctors at AsmWriter.cpp:2966 and :2982 initialize `TheModule` and `TheIndex` respectively and never both; `printModuleSummaryIndex` begins `assert(TheIndex)` (AsmWriter.cpp:3185-3186). include/llvm/IR/Module.h:51 has only `class ModuleSummaryIndex;` forward-declared. include/llvm/AsmParser/Parser.h:69-73 defines `ParsedModuleAndIndex` holding `std::unique_ptr<Module>` plus `std::unique_ptr<ModuleSummaryIndex> Index` — the split-pair return llvmkit mirrors. tools/llvm-as/llvm-as.cpp:156 likewise calls `Index->print(errs())` on its own. Source of the bad premise: docs/future-work.md:15-29, "Summary index — printing a module and its index is two calls (found 2026-08-15, LLParser parity W10)".

</details>

### ifunc is missing metadata attachments and three global properties

*IR model / parser — aliases and ifuncs* — crates/llvmkit-ir/src/global_ifunc.rs, crates/llvmkit-ir/src/global_alias.rs (the shape to mirror), crates/llvmkit-asmparser/src/ll_parser.rs (`parse_alias_or_ifunc`)

- **Was claimed:** `GlobalIfuncBuilder` has no `thread_local_mode` / `unnamed_addr` / `dll_storage_class` setters (the sibling `global_alias.rs` does), and ifunc metadata attachments are not stored.
- **Verdict:** CLOSED — the divergence no longer exists. All four items the claim calls missing are present in the tree today, and the parser and printer both wire them up. In `crates/llvmkit-ir/src/global_ifunc.rs`: - `GlobalIfuncBuilder::dll_storage_class`, `::thread_local_mode` and `::unnamed_addr` — all three `#[must_use]` setters exist, with a doc comment citing `parseAliasOrIFunc` as the reason ifunc carries them. - `GlobalIfuncData` stores `dll_storage_class`, `thread_local_mode`, `unnamed_addr` as `Cell`s plus `metadata: RefCell<MetadataAttachmentSet<StoredBrand>>`; `GlobalIfuncBuilder::into_data` threads all four through. - `GlobalIfunc` has the matching accessors/mutators, including `metadata()`, `metadata_stored()` and `set_metadata()`. The shape now matches `global_alias.rs` field-for-field; both files are structurally parallel (513 vs 518 lines, same field list on the data struct and the builder). Two secondary details in the claim's framing are also worth correcting: - ifunc/alias are NOT symmetric on metadata upstream, so "the same set GlobalAlias carries" is imprecise. `parseAliasOrIFunc`'s property loop guards metadata with `} else if (!IsAlias && Lex.getKind() == lltok::MetadataVar)` — only an **ifunc** may carry attachments; an alias with `!dbg !0` is still the "unknown alias or ifunc property!" error. llvmkit reproduces exactly this in `ll_parser.rs::parse_alias_or_ifunc`'s property loop, including the bang in the message. - The three global properties are stored but deliberately NOT printed for an ifunc. Upstream `AssemblyWriter::printIFunc` emits linkage, `printDSOLocation`, `printVisibility`, then goes straight to `"ifunc "` — it never prints DLL storage class, thread-local, or unnamed_addr, even though `parseAliasOrIFunc` applies all of them to `GV` in both branches. `fmt_ifunc` (asm_writer.rs) mirrors that omission, and an in-source comment in `ll_parser.rs::parse_alias_or_ifunc` records the choice. Closed by commit e02815d (2026-08-14) "feat(ir,asmparser)!: ifunc properties, global redefinition, named-metadata operands (LLParser parity W7, part 5)", which added exactly these six accessor/setter pairs to global_ifunc.rs (+71 lines) plus parser support (+73 lines) and tests in parser_module_level.rs and parser_debug_metadata.rs.

<details><summary>Verification evidence</summary>

Read all 518 lines of C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/global_ifunc.rs and all 513 lines of C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/global_alias.rs side by side. The ifunc builder's three setters are at lines 402, 408, 414 (alias's at 397, 403, 409 — same shape); `GlobalIfuncData.metadata` is at line 37 and `GlobalIfunc::set_metadata` at line 239, identical to the alias at lines 34 and 236. So the claim's "GlobalIfuncBuilder has no thread_local_mode / unnamed_addr / dll_storage_class setters" and "ifunc metadata attachments are not stored" are both directly contradicted by the file at the cited path. Read C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs lines 7126-7230 (`parse_alias_or_ifunc`): the property loop pushes into `ifunc_metadata` on `!is_alias && Token::MetadataVar` (7136-7141), the ifunc branch chains `.dll_storage_class(...).thread_local_mode(...).unnamed_addr(...)` onto `ifunc_builder` (7200-7202), and applies the attachments via `i_view.set_metadata(...)` in a loop at 7211-7213. Confirmed against upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp `LLParser::parseAliasOrIFunc`: it sets `setThreadLocalMode` / `setVisibility` / `setDLLStorageClass` / `setUnnamedAddr` on `GV` after both the alias and ifunc `create` calls, and its attribute loop reads `} else if (!IsAlias && Lex.getKind() == lltok::MetadataVar) { parseGlobalObjectMetadataAttachment(*GI) }` with the fallback `tokError("unknown alias or ifunc property!")` — llvmkit matches all three. Confirmed the printer contract against `AssemblyWriter::printIFunc` in orig_cpp/.../llvm/lib/IR/AsmWriter.cpp: linkage → printDSOLocation → printVisibility → `"ifunc "` → value type → resolver → partition → `GI->getAllMetadata(MDs)` / `printMetadataAttachments(MDs, ", ")`. llvmkit's `fmt_ifunc` in crates/llvmkit-ir/src/asm_writer.rs (line 3778 onward) emits that same sequence, ending with `fmt_metadata_attachments(f, &i.metadata_stored(), ..., ", ")`. Git history settles the timing: `git log -- crates/llvmkit-ir/src/global_ifunc.rs` shows e02815d (2026-08-14) as the most recent commit, and `git show e02815d` confirms it introduced the `dll_storage_class` / `thread_local_mode` / `unnamed_addr` accessors and builder setters. The file mtime (Aug 14) is newer than global_alias.rs (Aug 9), consistent with the ifunc side being brought up to the alias's shape after the claim was recorded.

</details>

### The summary index is not attached to the module, so `format!("{module}")` never prints `^N`

*printer / API surface* — crates/llvmkit-ir/src/module_summary_index.rs, crates/llvmkit-ir/src/asm_writer.rs, crates/llvmkit-asmparser/src/ll_parser.rs (`ParsedModule::summary_index`)

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
- **A row whose reason says a thing is *blocked* is the most dangerous kind.**
  Five of the ten wrong premises found in the 2026-08-29 `## Model gaps` triage
  were blocker sentences written before the blocker was removed and never
  revisited, and one of them had propagated into four files including
  production source. Re-derive a blocker before scheduling — or not
  scheduling — anything on it, and when you remove one, run the census and fix
  every copy in the same commit.
