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

### D9 — Attribute groups are never merged, and the alignment move is half-ported

**Severity:** wrong-output, model-gap
**Where:** `crates/llvmkit-ir/src/function.rs` — `function_attr_groups`;
`crates/llvmkit-asmparser/src/ll_parser.rs` — `parse_optional_function_suffix`

**LLVM:** `validateEndOfModule`'s first step merges every referenced
`#N` into the object's own attribute set, for five object kinds — `Function`,
`CallInst`, `InvokeInst`, `CallBrInst`, `GlobalVariable`. In the `Function`
arm only, an alignment that arrived as an *attribute* is moved to the
alignment field and removed. Upstream then discards the parsed numbering:
`AsmWriter` re-derives `#N` from `SlotTracker`'s dedup, so `attributes #7` on
input can print as `#0`.

**llvmkit:** no merge. Group ids are kept on the object and resolved lazily on
lookup and at print time, so the input numbering round-trips. The alignment
move exists only for *inline* attributes — mirroring upstream's other copy of
the same hack in `parseFunctionHeader` — so `define void @f() #0` with
`attributes #0 = { align 8 }` leaves `align` as a plain function attribute
instead of setting the field.

**Also:** an undefined `#N` is silently ignored by upstream (the
`NumberedAttrBuilders` lookup simply misses). llvmkit likewise never errors,
but then prints a dangling `#N` with no `attributes #N = { … }` line —
output that does not re-parse.

**Fix:** this is the item W7 blocked on, and it has to land with the
group-*forming* half of the printer, because the merge decides what survives
into the printed group. Brings `globalvariable-attributes.ll`'s `@g1`–`@g4`
and `test/Bitcode/attributes.ll` with it.

## Parser

### D6 — Self-typed aliasee does not parse

**Severity:** rejects-valid
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` — `parse_constant_expr`
**Also recorded in:** `future-work.md` (W7)

**LLVM:** `parseAliasOrIFunc` accepts an aliasee written as a leading
cast with no type of its own — `@b = alias i1, getelementptr ([4 x i1], ptr @a, i64 0, i64 2)`,
and the `bitcast` / `addrspacecast` / `inttoptr` spellings.

**llvmkit:** rejects with `expected type`. `parse_constant_expr` takes a
`result_ty`, and there is no entry point for a self-typing constant
expression.

**Consequence:** blocks `test/Assembler/uselistorder.ll` and makes upstream's
`invalid aliasee` unreachable, since it is only reached through this route.

**Fix:** W4's type-agnostic `ValID` refactor applied one level down — same
root cause, same shape.

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
| `allow_incomplete_ir` | **partly implemented** — see D12. |
| `upgrade_debug_info` | **selects nothing.** llvmkit has no `AutoUpgrade` port (see the inventory entry on `lib/IR/AutoUpgrade.cpp`), so there is no `UpgradeDebugInfo` call for the flag to guard. |

**Consequence:** a caller that sets `upgrade_debug_info: false` (what `llvm-as`
and `opt -disable-upgrade-debug-info` do) gets the same module as one that
leaves it `true`. Nothing is silently *wrong* — llvmkit never upgrades — but
the setting is a promise about a future behaviour rather than a live switch,
and the rustdoc says so.

**Fix:** lands with `AutoUpgrade`. The flag already reaches
`Parser::parse_module_with_config`; only the guarded call is missing.

### D12 — `-allow-incomplete-ir` covers value references, not disagreeing call sites

**Severity:** wrong-output, wrong-message
**Where:** `crates/llvmkit-asmparser/src/ll_parser.rs` —
`resolve_forward_ref_globals`, `validate_forward_function_decls`,
`parse_direct_callee`
**Pinned by:** `crates/llvmkit-asmparser/tests/parser_facade.rs::incomplete_ir_declarations`
against the verbatim `test/Assembler/incomplete-ir-declarations.ll`

**LLVM:** `getGlobalVal` mints one placeholder for *every* forward `@`
reference, callee or not, and parks it in a single sorted
`std::map ForwardRefVals`. `validateEndOfModule` then sweeps that one map:
under `AllowIncompleteIR` it synthesises a `Function` at the type
`GetCommonFunctionType` reports — the one function type every use calls the
value at — or an `i8` `GlobalVariable` when the uses disagree, are not calls,
or do not exist.

**llvmkit:** the forward references are split across two maps.
`forward_ref_globals` holds ordinary value references and is upstream's
`ForwardRefVals`; a *direct callee* instead goes through `parse_direct_callee`,
which builds a real `declare` at the **first** call site's signature and
records only the name in `forward_function_decls`.

Two consequences, both reproduced by the ported fixture:

1. **`@fn2` keeps its first signature.** Its three call sites disagree, so
   upstream emits `@fn2 = external global i8`; llvmkit already built
   `declare void @fn2(i32)` and prints that. Undoing it needs a
   function-removal and a function-RAUW that `llvmkit-ir` does not have —
   `replace_all_uses_with` exists on `ForwardRefValue` and `Instruction`, not
   on `FunctionValue`, and nothing removes a function from a module.
2. **A different leftover is reported when the option is off.** Upstream reports
   `ForwardRefVals.begin()`, which for a `std::map` is the lexicographically
   first name — `use of undefined value '@fn1'` for that fixture. llvmkit
   checks `forward_ref_globals` first and `forward_function_decls` after, so
   it reports `use of undefined value '@g1'`; and when the callee map *is* the
   one that fires, it says `undefined global` where upstream says
   `undefined value`, because the two maps carry different `SymbolKind`s.

The other two `AllowIncompleteIR` tolerances are unimplemented rather than
partial: `dropUnknownMetadataReferences` (see D13) and the relaxed
`InstsWithTBAATag` assertion, which has no counterpart at all because
`UpgradeTBAANode` is not ported.

**Fix:** route a direct callee through the same placeholder path
`getGlobalVal` uses, collapsing `forward_function_decls` into
`forward_ref_globals`. That closes both consequences at once and is the same
refactor several other rows want.

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

### 1. `u0x…` and >64-bit literals are rejected wherever a `uint64` is wanted

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:2244 (`parse_uint64`), :2216 (`parse_uint32`)

- **LLVM:** `LLParser::parseUInt64` accepts any `lltok::APSInt` whose value is unsigned and reads it with `APSInt::getLimitedValue()`, which saturates at `UINT64_MAX`. `u0x10` is an unsigned APSInt (`LLLexer::LexDigitOrNegative`'s `[us]0x` form) and is accepted at every `parseUInt64` call site; a literal wider than 64 bits saturates rather than failing.
- **llvmkit:** `Parser::parse_uint64` matches only `Token::IntegerLit` with `sign: Pos, base: Dec` and calls `digits.parse::<u64>()`, so `u0x10` answers `expected integer` and an over-wide decimal literal is rejected outright instead of saturating. `parse_uint32` has the same narrow shape, but its saturation is unobservable because the `0xFFFFFFFF + 1` range check rejects an oversized value either way (what `align-param-attr-error2.ll` pins).
- **Why:** Recorded: it is a W5-owned routine with 25 call sites, no `test/Assembler` fixture reaches either case, and the honest fix reads the token through `parse_int_literal` — the APSInt token model — which changes where the diagnostic's span comes from. Deliberately not smuggled into the W10 summary-index wave.
- **Fix:** Route both through `parse_int_literal` + `ParsedApsInt`, accepting any unsigned APSInt token (decimal, `u0x`, `0x`) and reproducing `getLimitedValue`'s saturation instead of failing; then re-check every one of the 25 call sites' diagnostic spans, since the span now comes from the APSInt token rather than the digit run.
- **Correction from verification:** Substantively accurate and still present; one sub-detail is wrong. The `[us]0x[0-9A-Fa-f]+` form is lexed by `LLLexer::LexIdentifier`, not `LLLexer::LexDigitOrNegative` (the latter only handles `[-]?[0-9]+`, plain `0x…` FP forms, and labels). Corrected statement: "`LLParser::parseUInt64` accepts any `lltok::APSInt` whose value is unsigned and reads it with `APSInt::getLimitedValue()`, which saturates at `UINT64_MAX`. `u0x10` is an unsigned APSInt (`LLLexer::LexIdentifier`'s `[us]0x` form) and is accepted at every `parseUInt64` call site; a literal wider than 64 bits saturates rather than failing." Everything else in the claim checks out verbatim, including the parenthetical that `parse_uint32`'s missing saturation is unobservable. Two additions worth recording: (a) the divergence is reachable from ordinary `.ll`, not just summary syntax — `align u0x8` (`parse_optional_alignment_value`, ll_parser.rs:2383) and `dereferenceable(u0x10)` both parse upstream and answer `expected integer` in llvmkit; and for an over-wide decimal such as `align 99999999999999999999999` upstream saturates to `UINT64_MAX` and then fails with `alignment is not a power of two`, so the observable diagnostic differs, not merely the internal value; (b) llvmkit's own comment inside `parse_uint32` (ll_parser.rs:2229-2231) asserts the saturating semantics — "`getLimitedValue(0xFFFFFFFFULL + 1)` saturates rather than failing, so a literal too wide even for 64 bits still reaches the range check below" — immediately above the `return Err(...)` that does the opposite, so the comment documents behavior the code does not implement.

<details><summary>Verification evidence</summary>

Upstream, C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp `LLParser::parseUInt64` — the whole body is `if (Lex.getKind() != lltok::APSInt || Lex.getAPSIntVal().isSigned()) return tokError("expected integer"); Val = Lex.getAPSIntVal().getLimitedValue();`. The only gate is token-kind plus signedness; the base/spelling is never inspected, and `getLimitedValue()` (default limit `UINT64_MAX`) saturates instead of failing. `parseUInt32` is identical with `getLimitedValue(0xFFFFFFFFULL+1)` followed by `if (Val64 != unsigned(Val64)) return tokError("expected 32-bit integer (too large)")` — which is exactly why over-wide is unobservable for the 32-bit form. Upstream lexer, same tree, lib/AsmParser/LLLexer.cpp `LLLexer::LexIdentifier` (the block commented "Check for [us]0x[0-9A-Fa-f]+ which are Hexadecimal constant generated by the CFE"): builds `APInt Tmp(bits, HexStr, 16)`, truncates to active bits, then `APSIntVal = APSInt(Tmp, TokStart[0] == 'u'); return lltok::APSInt;`. So `u0x10` is `lltok::APSInt` with `isSigned() == false` — it passes both of `parseUInt64`'s gates. `LexDigitOrNegative` is a separate function further down that handles labels/decimals/`0x…` FP and reaches `APSIntVal = APSInt(StringRef(...))`; the `[us]0x` case is not there. llvmkit, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:2244 `parse_uint64` — the match arm is `Token::IntegerLit(IntLit { sign: Sign::Pos, base: NumBase::Dec, digits })` calling `digits.parse::<u64>().ok()`, with `_ => None` and `None => Err(self.expected("integer"))`. Both halves of the claim follow directly: a non-`Dec` base falls to `_`, and a >64-bit decimal makes `parse::<u64>()` return `Err`. `parse_uint32` at :2216 has the same `sign: Pos, base: Dec` shape. llvmkit lexer, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_lexer.rs:1073 `classify_hex_apsint` — `b'u' => NumBase::HexUnsigned`, emitting `IntLit { sign: Sign::Pos, base: HexUnsigned, digits }`. So `u0x10` provably cannot match `parse_uint64`'s `NumBase::Dec` arm. Corroborating that llvmkit knows the distinction and applies it selectively: ll_parser.rs:3320 `parse_summary_flag` matches `base: NumBase::Dec | NumBase::HexUnsigned` and its doc comment states "an `u0x…` one is not [signed] — so the check is `APSInt::isSigned`". That is the shape `parse_uint64`/`parse_uint32` are missing, and it is pinned by a test at crates/llvmkit-asmparser/tests/parser_summary.rs:315 (`("u0x2", true)`). No such coverage exists for the uint64/uint32 path. Reachability from plain `.ll`: upstream `parseOptionalAlignment` (LLParser.cpp) and the `dereferenceable(N)` parser both call `parseUInt64` directly; llvmkit's counterpart `parse_optional_alignment_value` (ll_parser.rs:2379-2389) calls `self.parse_uint64()`. There are 22 `parseUInt64` call sites upstream and 20 `parse_uint64` call sites in llvmkit, all funnelling through these two functions, so the divergence applies uniformly.

</details>

### 3. A call's signature is checked against a later `declare`/`define`, which upstream leaves to the Verifier

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:10474, :10698 (the two rejections); `parse_direct_callee` around :13257

- **LLVM:** `LLParser::getGlobalVal` mints an untyped placeholder — a `ptr`-typed `GlobalVariable` — for a forward-referenced callee, so `LLParser::parseFunctionHeader` compares only `FwdFn->getType() != PFT`, which after opaque pointers is nothing but the address space. The signature is never compared at the definition site; a call whose arguments disagree with the eventual definition parses, and `Verifier::visitCallBase` rejects it with `Call parameter type does not match function signature!`.
- **llvmkit:** `parse_direct_callee`'s forward-reference arm calls `Module::add_function_dyn` with the *call site's* signature, so the placeholder is a real `Function` with a real `FunctionType` and cannot be re-typed later. `declare`/`define` therefore reject the reuse with two texts upstream never prints: `forward function declaration with matching signature` and `forward function definition with matching signature`.
- **Why:** Recorded, and the check is load-bearing rather than cosmetic: dropping it would leave a call wired to a function whose type it does not match, because llvmkit has no way to give an existing `Function` a different `FunctionType`.
- **Fix:** Apply the shape W2 gave value forward references — an untyped placeholder plus RAUW at the definition — to the callee position, so `parseFunctionHeader` can mint a fresh `Function` with the definition's type and re-point every pending call. That also unblocks the three per-site texts W2.5 carried (`invalid forward reference to function '<n>' with wrong type: expected 'T' but was 'U'`, pinned by the already-vendored `test/Assembler/opaque-ptr-invalid-forward-ref.ll`; `type of definition and forward reference of '@N' disagree`; and the global/alias twins), all of which compare types at the definition site where llvmkit still resolves in one end-of-module sweep.
- **Correction from verification:** The divergence is real and still present, and the llvmkit half of the description is exactly right. One clause about upstream is wrong, and correcting it makes the divergence WORSE, not smaller. Wrong clause: "`Verifier::visitCallBase` rejects it with `Call parameter type does not match function signature!`". It does not. `visitCallBase` compares each argument against the *call's own* `FunctionType` (`FTy = Call.getFunctionType()`), which the parser built from those very arguments, so that check can never fire for this program — and `LLParser::parseCall` would already have rejected a genuine arg/type disagreement with "argument is not of expected type" beforehand. The only check comparing the callee's declared type to the call site's type is `Check(Callee->getValueType() == FTy, "Intrinsic called with incompatible signature")` at Verifier.cpp:3860, and it is guarded by `if (IsIntrinsic)`. A non-intrinsic call whose signature disagrees with its callee's `declare`/`define` is simply valid LLVM IR after opaque pointers. Corrected statement: upstream `LLParser::getGlobalVal` mints an untyped placeholder — an i8 `GlobalVariable` with `ExternalWeakLinkage`, whose type is just `ptr addrspace(N)` (`createGlobalFwdRef`, LLParser.cpp:1762) — for a forward-referenced callee, so `parseFunctionHeader` compares only `FwdFn->getType() != PFT`, which after opaque pointers is nothing but the address space; it then RAUWs the placeholder with the real `Function`. The signature is never compared at the definition site, and it is never compared afterwards either: the mismatched call parses AND verifies, because for a non-intrinsic callee no Verifier check relates the call site's `FunctionType` to the callee's. llvmkit's `resolve_direct_callee` instead calls `Module::add_function_dyn` with the call site's signature, so the placeholder is a real `Function` with a real `FunctionType` that cannot be re-typed later, and `declare`/`define` reject the reuse with two texts upstream never prints. llvmkit therefore rejects valid LLVM IR outright, rather than merely moving an error from the Verifier to the parser. Two smaller location fixes: (a) the rejection lines are 10474 (declare) and 10698 (define) as cited, but the guard is `existing.signature() != fn_ty || <body non-empty>` — the second disjunct is the redefinition half; (b) the placeholder is minted in `resolve_direct_callee` (ll_parser.rs:13290, NonIntrinsic arm at :13362-13372), not `parse_direct_callee_ref` (:13263), which only classifies the callee token.

<details><summary>Verification evidence</summary>

Read (llvmkit): C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs. - :10473-10479 — `if existing.signature() != fn_ty || existing.basic_blocks().len() != 0 { return Err(... "forward function declaration with matching signature" ...) }`, where `existing` comes from `self.module.function_dyn(&name)` at :10469-10472. - :10697-10703 — the same shape for `define`: `"forward function definition with matching signature"`. - :13362-13372 — `resolve_direct_callee`'s NonIntrinsic forward-reference arm: `self.module.add_function_dyn(&name, parsed_fn_ty, Linkage::External)` then `self.forward_function_decls.entry(name).or_insert(loc)` — the placeholder is created with `parsed_fn_ty`, the *call site's* signature. - :9007-9024 — `check_function_redefinition` returns `Ok(())` early when `forward_function_decls.contains_key(name)`, so the call-forward-referenced case falls through to the signature check rather than the "invalid redefinition of function" arm. - The parser's own comment at :13303-13310 states the opposite policy for the resolved case ("The verifier — not the parser — owns the eventual call-vs-declaration check"), so llvmkit is internally inconsistent: a call to an *already declared* function with a mismatched signature is accepted, while the same mismatch discovered in the other order is a parse error. Read (upstream, orig_cpp/llvm-project-llvmorg-22.1.4/llvm/): - lib/AsmParser/LLParser.cpp:1762-1769 `createGlobalFwdRef` — `new GlobalVariable(*M, Type::getInt8Ty(...), false, GlobalValue::ExternalWeakLinkage, ...)`, comment: "The used global type does not matter. We will later RAUW it". - LLParser.cpp:1785-1817 `getGlobalVal(Name, Ty, Loc)` — on miss, `ForwardRefVals[Name] = {FwdVal, Loc}`; the callee is looked up as a bare `ptr` (parseCall/parseCallBr do `convertValIDToValue(PointerType::getUnqual(Context), CalleeID, ...)`, e.g. :8078). - LLParser.cpp:6860-6905 `parseFunctionHeader` — the only forward-ref test is `if (FwdFn->getType() != PFT)` with `PFT = PointerType::get(Context, AddrSpace)`; then :6947-6949 `FwdFn->replaceAllUsesWith(Fn); FwdFn->eraseFromParent();`. No `FT` comparison anywhere. - lib/IR/Verifier.cpp:3832-3861 `visitCallBase` — `FTy = Call.getFunctionType()`; the arg loop at :3846-3848 compares operands to `FTy`'s own params; `grep -n "getValueType() == FTy"` over Verifier.cpp returns exactly one hit, :3860, inside `if (IsIntrinsic)`. Ran (empirical, cargo +1.96.0 build --release -p llvmkit-asmparser --example parse_file): - `define void @caller() { call void @callee(i32 1) ret void }` + `declare void @callee()` → exit 1, `6:14: expected forward function declaration with matching signature`. - same caller + `define void @callee() { ret void }` → exit 1, `6:13: expected forward function definition with matching signature`. - control, same caller + `declare void @callee(i32)` → exit 0, round-trips cleanly — confirming the rejection is specifically the signature disagreement, not the reuse. Also: docs/future-work.md:108-134 already records this as a known open item, and repeats the same incorrect Verifier claim at :124-125; that doc should be corrected alongside.

</details>

### 4. A self-typed aliasee constant expression does not parse

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:7099-7130 (`parse_alias_or_ifunc`), :8529 (`parse_constant_expr`, which takes `result_ty`)

- **LLVM:** `LLParser::parseAliasOrIFunc` branches on the aliasee's first token: `bitcast`, `getelementptr`, `addrspacecast` and `inttoptr` go through a bare `parseValID` (its comment: the bitcast dest type is not present, it is implied by the dest type), and anything else goes through `parseGlobalTypeAndValue`, which is TYPE VALUE. The result must be `ValID::t_Constant` or the diagnostic is `invalid aliasee`, and the pointer check then runs on the aliasee *value's* type, taking the address space from it.
- **llvmkit:** `parse_alias_or_ifunc` always reads TYPE then VALUE (`parse_type` followed by `parse_alias_target`), so `@a = alias i32, bitcast (ptr @g to ptr)` and the `getelementptr` / `addrspacecast` / `inttoptr` spellings do not parse at all. `invalid aliasee` is unreachable, since it only fires on the branch llvmkit does not have.
- **Why:** Recorded. An attempt was made and reverted: the blocker is that `Parser::parse_constant_expr` takes a `result_ty` and llvmkit has no entry point for a constant expression that types itself — every constexpr arm is reached with the demanded type already in hand. This is W4's type-agnostic `ValID` refactor applied one level down, judged wave-sized rather than an alias fix.
- **Fix:** Give constant expressions a self-typing entry point (the constexpr analogue of W4's type-agnostic `ValID`), then branch `parse_alias_or_ifunc` on the first token as upstream does, run the pointer check on the parsed aliasee's own type, take the address space from it, and report `invalid aliasee` when the result is not a constant.

<details><summary>Verification evidence</summary>

Confirmed on all three legs — upstream, llvmkit source, and an empirical parse. 1. Upstream, C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp, `LLParser::parseAliasOrIFunc`, reads exactly as claimed: `if (Lex.getKind() != lltok::kw_bitcast && Lex.getKind() != lltok::kw_getelementptr && Lex.getKind() != lltok::kw_addrspacecast && Lex.getKind() != lltok::kw_inttoptr) { if (parseGlobalTypeAndValue(Aliasee)) return true; } else { /* The bitcast dest type is not present, it is implied by the dest type. */ ValID ID; if (parseValID(ID, nullptr)) return true; if (ID.Kind != ValID::t_Constant) return error(AliaseeLoc, "invalid aliasee"); Aliasee = ID.ConstantVal; }` followed by `Type *AliaseeType = Aliasee->getType(); auto *PTy = dyn_cast<PointerType>(AliaseeType); if (!PTy) return error(AliaseeLoc, "An alias or ifunc must have pointer type"); unsigned AddrSpace = PTy->getAddressSpace();` — the pointer check and address space do come off the aliasee *value's* type. 2. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:7099-7117 (`parse_alias_or_ifunc`) has no such branch: `let value_type = self.parse_type(false)?;` then the comma, then unconditionally `let target_ty = self.parse_type(false)?;`, the pointer check on `target_ty`, then `self.parse_alias_target(target_ty)` (:7234) which is `self.parse_constant(target_ty)`. Identical at HEAD (`git show HEAD:...` lines 7076-7081), so this is not a working-tree artifact. `parse_constant_expr` at :8529 indeed takes `result_ty: Type<'ctx, B>`, so a constant expression can only be reached once a type has already been consumed. Grep for `invalid aliasee` across crates/ finds exactly one hit — a comment at ll_parser.rs:7102 — no diagnostic emits it, confirming that message is unreachable. 3. Empirical probe (temporary test, since removed), release build on the pin: `@a = alias i32, bitcast (ptr @g to ptr)` -> ERR "expected type" `@b = alias i1, getelementptr ([4 x i1], ptr @g, i64 0, i64 2)` -> ERR "expected type" `@c = alias i32, addrspacecast (ptr @g to ptr addrspace(1))` -> ERR "expected type" `@d = alias i32, inttoptr (i64 42 to ptr)` -> ERR "expected type" `@e = alias i32, ptr @g` -> OK; `@f = alias i32, ptr bitcast (ptr @g to ptr)` -> OK (folds to `ptr @g`); `@h = alias i32, i32 3` -> ERR "An alias or ifunc must have pointer type". Two additions the claim does not mention, both supporting it. (a) The consequence is already visible in the tree: crates/llvmkit-asmparser/tests/fixtures/upstream/uselistorder/uselistorder.ll is a byte-verbatim copy of upstream test/Assembler/uselistorder.ll whose line 7 is `@b = alias i1, getelementptr ([4 x i1], ptr @a, i64 0, i64 2)`, and `the_upstream_uselistorder_fixture_parses_clean` in crates/llvmkit-asmparser/tests/parser_use_list.rs currently FAILS with `Expected { expected: "type", span 203..216 }` — bytes 203-216 of the fixture are the `getelementptr` token. (That fixture directory is untracked in-progress W12 work, so it is not a pre-existing red at HEAD; it is a fresh demonstration of this exact gap.) (b) The constant-expression machinery itself is present and works — only the type-less aliasee spelling is missing — so the gap is the missing first-token branch, not missing constexpr support.

</details>

### 6. A numeric-only or mixed `DIFlags` / `DISPFlags` term is rejected, and written flags are never canonicalised

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:5879-5903 (the `DiFlag`/`DiSpFlag` arm), :5668-5685 (per-term validation), :5768-5769 (field kinds); crates/llvmkit-ir/src/asm_writer.rs:3491, :3497

- **LLVM:** `LLParser::parseMDField(DIFlagField&)` and its `DISPFlagField` twin loop `do { parseFlag } while (EatIfPresent(lltok::bar))`, and `parseFlag` accepts either an unsigned `lltok::APSInt` (read with `parseUInt32`) or a `lltok::DIFlag`, OR-ing the terms into one `DINode::DIFlags` / `DISubprogram::DISPFlags` bitfield. `MDFieldPrinter::printDIFlags` / `printDISPFlags` then re-emit it through `DINode::splitFlags` + `getFlagString`, so output is canonical: bit order from the table, composite spellings recovered, duplicates collapsed, and any unnamed remainder printed as a trailing number.
- **llvmkit:** The `Token::DiFlag | Token::DiSpFlag` arm keeps the `|`-joined **source text** as `MetadataFieldValue::Enum(String)` (each term validated against `dwarf::di_flag` / `disp_flag`), and after a `|` it accepts only another flag token — `expected debug info flag after '|'`. So `spFlags: DISPFlagDefinition | 4096` and `flags: 4 | DIFlagPublic` are rejected though upstream accepts them, and a purely numeric `flags: 3` is stored as `Integer(3)` and printed back as `3` where `llvm-dis` prints `DIFlagPublic`. Written order, duplicates and alias spellings all round-trip verbatim instead of being canonicalised.
- **Why:** Recorded reason covers only the storage half: modelling `DINode::DIFlags` / `DISubprogram::DISPFlags` as bitflags is deferred to the debug-info/metadata milestone, on the ground that the bitflag type is worth its keep once something reads it, and that the joined text matches `printDIFlags`'s `ListSeparator(" | ")`. That separator claim is true but narrower than the entry implies — the numeric term and the `splitFlags` canonicalisation are **unrecorded**.
- **Fix:** Model both as `u32` bitflag types with `getFlag`/`getFlagString`/`splitFlags` ports (the tables already exist as `llvmkit_ir::dwarf::di_flag` / `disp_flag`), accept an unsigned integer term anywhere in the `|` chain as upstream's `parseFlag` does, store the OR, and print through the `splitFlags` + trailing-`Extra` shape. Land it with the DWARF-encoding storage fix below — both are the same normalisation milestone.
- **Correction from verification:** The body of the claim is accurate; the TITLE is wrong on one half. A numeric-only term is NOT rejected — `flags: 3` parses fine, is stored as `MetadataFieldValue::Integer(3)` and printed back verbatim as `3` (llvm-dis prints `DIFlagPublic`). Only MIXED terms are rejected, in both orders and with two different messages: `flags: DIFlagPublic | 4` and `spFlags: DISPFlagDefinition | 4096` give `expected debug info flag after '|'`, while `flags: 4 | DIFlagPublic` gives `expected ')' here` (the integer parses as the whole value and the field loop then demands `,` or `)`). Corrected title: "A MIXED `DIFlags`/`DISPFlags` term is rejected, a numeric-only one is kept as an unconverted integer, and written flags are never canonicalised." One additional divergence not in the claim: because the `DiFlags`/`DispFlags` validator returns early for any non-`Enum` value, no unsigned/range check runs, so `flags: -1` is accepted, where upstream's `!Lex.getAPSIntVal().isSigned()` guard falls through to `expected debug info flag`.

<details><summary>Verification evidence</summary>

llvmkit source: crates/llvmkit-asmparser/src/ll_parser.rs:5890-5904 — the `Token::DiFlag(s) | Token::DiSpFlag(s)` arm builds a `MetadataFieldValue::Enum(String)` by pushing `" | "` plus the next term's source text, and after a `Token::Bar` matches only `Token::DiFlag|Token::DiSpFlag`, else `Err(self.expected("debug info flag after '|'"))`; there is no `Token::IntegerLit` case in that loop. :5668-5685 — the `flags` closure splits the stored string on `'|'` and checks each term against `dwarf::di_flag`/`disp_flag`, but opens with `let MetadataFieldValue::Enum(spelling) = value else { return Ok(()) }`, so an `Integer` bypasses all checking. :5768-5769 — `MetadataFieldKind::DiFlags`/`DispFlags` dispatch to that closure. :5397-5406 — after `parse_metadata_field_value` the field loop only accepts `,` then `)`. crates/llvmkit-ir/src/asm_writer.rs:3491,3497 — `Integer(v) => write!(f, "{v}")` and `Enum(s) => f.write_str(s)`, i.e. verbatim echo; and `dwarf::di_flag_string`/`disp_flag_string` (crates/llvmkit-ir/src/dwarf.rs:668-669) have zero call sites in the whole workspace, so no canonicalisation path exists. Upstream: orig_cpp/.../llvm/lib/AsmParser/LLParser.cpp `parseMDField(LocTy, StringRef, DIFlagField&)` and the `DISPFlagField&` twin — `parseFlag` accepts `lltok::APSInt` when `!Lex.getAPSIntVal().isSigned()` (via `parseUInt32`) or `lltok::DIFlag`/`lltok::DISPFlag`, and the `do { ... } while (EatIfPresent(lltok::bar))` loop ORs terms into one bitfield; lib/IR/AsmWriter.cpp `MDFieldPrinter::printDIFlags`/`printDISPFlags` re-emit through `splitFlags` + `getFlagString` with `ListSeparator(" | ")`, printing the unnamed remainder as a trailing number; lib/IR/DebugInfoMetadata.cpp `DINode::splitFlags` collapses the accessibility triple (comment: emit "DIFlagPublic" and not "DIFlagPrivate | DIFlagProtected") and the FlagPtrToMemberRep triple, then walks HANDLE_DI_FLAG in table order. Empirical: a temporary test run under `cargo +1.96.0 test --release -p llvmkit-asmparser` (file since deleted) printed: `flags: 3` -> OK, re-printed `flags: 3`; `flags: DIFlagPublic | 4` -> ERR "expected debug info flag after '|'"; `flags: 4 | DIFlagPublic` -> ERR "expected ')' here"; `spFlags: DISPFlagDefinition | 4096` -> ERR "expected debug info flag after '|'"; `flags: DIFlagProtected | DIFlagPrivate` -> OK, re-printed verbatim (upstream would print `DIFlagPublic`); `flags: DIFlagStaticMember | DIFlagPublic` -> OK, written order preserved (upstream reorders); `flags: -1` -> OK, re-printed `-1`. The gap is also recorded as deliberate-but-open at docs/future-work.md:1219-1233 ("`DIFlags` / `DISPFlags` are not bitflags").

</details>

### 7. Attribute-group `align = N` / `alignstack = N` rejected where upstream asserts

*parser (LLParser)* — crates/llvmkit-asmparser/src/ll_parser.rs:9487-9496 (align), crates/llvmkit-asmparser/src/ll_parser.rs:9503-9512 (alignstack)

- **LLVM:** Inside an attribute group the grammar is `align = N`, read with `parseUInt32` and given no validation at all; upstream then constructs `Align(Value)` / `MaybeAlign(unsigned)`, whose rejections of zero and non-powers-of-two are C++ `assert`s. In a release `llvm-as` those asserts are compiled out, so the value is simply accepted.
- **llvmkit:** `parse_optional_attrs` calls `self.check_alignment_value(value, value_loc)` in the attribute-group arm, reusing `parseOptionalAlignment`'s wording, and returns a parse error for the two values upstream would assert on. llvmkit forbids runtime panics in production paths, so there is no way to reproduce the assert.
- **Why:** Recorded inline at `ll_parser.rs:9479-9486` and in `docs/future-work.md` as "a deliberate divergence in *diagnostic presence*, never in accept/reject". That framing is slightly optimistic: against an assertions-disabled upstream this is an accept/reject difference, not just a message difference.
- **Fix:** Decide which upstream is the oracle. If assertions-enabled LLVM is the contract, keep the check and restate the recorded reason as "matches an assertions-enabled `llvm-as`, diverges from a release one". If a release `llvm-as` is the contract, drop `check_alignment_value` in the `in_attr_group()` arm, store the raw `u32`, and let `Verifier` reject the bad alignment — which is where upstream's non-assert rejection actually lives.
- **Correction from verification:** The divergence is REAL and STILL PRESENT, but the description is wrong in three places and incomplete in a fourth. Corrected statement: In `parse_fn_attribute_value_pairs` (NOT `parse_optional_attrs` — no such function exists in the file), the attribute-group forms `align = N` and `alignstack = N` reject values that upstream accepts in a release build: 1. `align = N` (ll_parser.rs:9480-9503): the `in_attr_group()` branch calls `self.check_alignment_value(value, value_loc)?` at line 9494, reusing `parseOptionalAlignment`'s wording. This rejects `0` and any non-power-of-two with "alignment is not a power of two". Upstream's `parseEnumAttribute` case `Attribute::Alignment` reads `parseUInt32(Value)` and constructs `Align(Value)`, whose `assert(Value > 0)` / `assert(isPowerOf2_64(Value))` are compiled out under NDEBUG — a release `llvm-as` accepts `align = 3` and silently rounds it to 2 (`Log2_64(3) == 1`). Claim accurate here. - Note the `huge alignments are not supported yet` branch of `check_alignment_value` is unreachable from this arm: `parse_uint32` caps at u32::MAX (4294967295) < `1 << 32`. Only the power-of-two check is live. 2. `alignstack = N` (ll_parser.rs:9504-9527) does NOT call `check_alignment_value` and does NOT reuse `parseOptionalAlignment`'s wording. It uses an inline check with `parseOptionalStackAlignment`'s wording. Critically, the claim's "returns a parse error for the two values upstream would assert on" is WRONG for alignstack: `value == 0` hits `continue` (no attribute added), which matches upstream exactly — `MaybeAlign(uint64_t)`'s assert is `(Value == 0 || isPowerOf2_64(Value))`, so zero is explicitly well-defined and yields nullopt, and `addStackAlignmentAttr` returns early. Only the non-power-of-two case diverges (llvmkit errors with "stack alignment is not a power of two"; upstream asserts in debug, silently rounds in release). 3. Cited line ranges are slightly off for alignstack: claim says 9503-9512, but the arm opens at 9504, the zero-skip is at 9512-9514, and the actual rejection is at 9516-9520. The align citation (9487-9496) does contain the check at 9494. (Working-tree numbers; at HEAD the same code sits at 9486 and 9510 — the file has uncommitted edits, but none touch this logic.) 4. Missed, opposite-direction case: upstream `AttrBuilder::addStackAlignmentAttr` (Attributes.cpp:2193) also asserts `*Align <= 0x100`. llvmkit has no such cap, so `attributes #0 = { alignstack = 512 }` is ACCEPTED by llvmkit and asserts in an upstream debug build. The divergence therefore runs both ways, not just llvmkit-stricter. Secondary finding: the in-code comment at ll_parser.rs:9484-9488 asserts this is "recorded in `docs/future-work.md` as a deliberate divergence in *diagnostic presence*, never in accept/reject." Both halves are false. (a) No such record exists — grep of docs/future-work.md for `Align(Value)`, `MaybeAlign`, `diagnostic presence`, `align = `, `alignstack = ` returns nothing; the only align-adjacent entries are the W5 printer/hoisting note at lines 200-229. (b) "never in accept/reject" is untrue against a release upstream, which accepts `align = 3` while llvmkit errors. This is exactly the class of stale self-recorded premise the repo has been burned by before. Test coverage: the only fixture touching this is crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:936-941, which pins `alignstack = 0` producing "attribute group has no attributes" (the group ends up empty). Nothing pins the `align = 0` / `align = 3` / `alignstack = 3` rejections.

<details><summary>Verification evidence</summary>

crates/llvmkit-asmparser/src/ll_parser.rs:9437 — enclosing fn is `parse_fn_attribute_value_pairs`; grep for `fn parse_optional_attrs` returns nothing. crates/llvmkit-asmparser/src/ll_parser.rs:9489-9496 — align group branch: `self.expect_punct(PunctKind::Equal, "'=' here")?; let value_loc = self.loc(); let value = u64::from(self.parse_uint32()?); self.check_alignment_value(value, value_loc)?;` crates/llvmkit-asmparser/src/ll_parser.rs:2397-2404 — `check_alignment_value`: `if !value.is_power_of_two() { return Err(... "alignment is not a power of two") } if value > (1u64 << 32) { ... "huge alignments are not supported yet" }` — the second branch is unreachable from a `parse_uint32` source. crates/llvmkit-asmparser/src/ll_parser.rs:9504-9520 — alignstack group branch: inline `if value == 0 { continue; }` then `if !value.is_power_of_two() { return Err(self.message_at(value_loc, "stack alignment is not a power of two")) }` — no `check_alignment_value` call. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp:1563-1590 — `parseEnumAttribute`: `case Attribute::Alignment` does `parseUInt32(Value); Alignment = Align(Value);` and `case Attribute::StackAlignment` does `parseUInt32(Alignment); B.addStackAlignmentAttr(Alignment);` — no validation in either. orig_cpp/.../llvm/include/llvm/Support/Alignment.h:68-73 — `explicit Align(uint64_t Value) { assert(Value > 0 ...); assert(llvm::isPowerOf2_64(Value) ...); ShiftValue = Log2_64(Value); }` orig_cpp/.../llvm/include/llvm/Support/Alignment.h:122-127 — `explicit MaybeAlign(uint64_t Value) { assert((Value == 0 || llvm::isPowerOf2_64(Value)) ...); if (Value) emplace(Value); }` — zero is legal and yields nullopt. orig_cpp/.../llvm/lib/IR/Attributes.cpp:2185-2200 — `addAlignmentAttr`/`addStackAlignmentAttr` both early-return on a nullopt MaybeAlign; the latter also `assert(*Align <= 0x100)`, which llvmkit does not mirror. orig_cpp/.../llvm/include/llvm/IR/Attributes.h:1225-1238 — the `unsigned` overloads forward through `MaybeAlign(Align)`. docs/future-work.md — grep for `Align(Value)`, `MaybeAlign`, `diagnostic presence`, `align = `, `alignstack = ` finds no record of this divergence, contradicting the in-code comment at ll_parser.rs:9484-9488. crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:936-941 — only `alignstack = 0` is pinned, expecting "attribute group has no attributes". git show HEAD:crates/llvmkit-asmparser/src/ll_parser.rs — same code at lines 9486 and 9510, so this is committed, not a working-tree artifact.

</details>

### 8. Verifier rejects a zero-incoming phi in a reachable block

*verifier* — crates/llvmkit-ir/src/verifier.rs:2962-2978

- **LLVM:** `Verifier::visitPHINode` checks only that the incoming count equals the predecessor count. A phi with zero incomings in a block with zero predecessors passes on `0 == 0`, so LLVM's verifier accepts it.
- **llvmkit:** Before delegating to the shared `check_phi_incoming` core, the verifier gates on reachability and fails with `VerifierRule::PhiEmptyInReachableBlock` ("phi in a block reachable from entry has no incoming values") for any empty phi in a block reachable from entry. llvmkit rejects IR LLVM accepts.
- **Why:** Recorded inline as "Defense in depth (stricter than upstream)": such a phi prints as `%p = phi i32` with no `[ … ]` pairs, which `LLParser::parsePHI` refuses to read, so accepting it would produce un-round-trippable output. The reachability gate is there because an unreachable block may legitimately have no predecessors.
- **Fix:** Keep the rule — the round-trip contract is stronger than upstream's verifier here — but make the divergence explicit rather than a comment: give `VerifierRule::PhiEmptyInReachableBlock` a doc line saying it has no `Verifier.cpp` counterpart, add the `UPSTREAM.md` row, and confirm no ported `test/Verifier` fixture relies on the case passing. If strict parity is ever wanted, demote it to a warning behind a verifier-strictness flag.
- **Correction from verification:** The divergence is real and still present, but the claim misattributes the upstream check. `Verifier::visitPHINode` does NOT check incoming count vs. predecessor count — it checks only three things (phis grouped at block top, result type is not token-like, each incoming value's type equals the result type) and then explicitly defers with the comment "All other PHI node constraints are checked in the visitBasicBlock method." The count guard `Check(PN.getNumIncomingValues() == Preds.size(), "PHINode should have one entry for each predecessor of its parent basic block!")` lives in `Verifier::visitBasicBlock`. (llvmkit's own source comment at verifier.rs:2967 repeats the same misattribution, calling it "the same gap as LLVM's `visitPHINode`".) The substance of the claim survives that correction: LLVM's only length guard on a phi is `numIncoming == numPreds`, so a zero-incoming phi in a zero-predecessor block passes on `0 == 0` and LLVM's verifier accepts it, while llvmkit rejects it with `VerifierRule::PhiEmptyInReachableBlock`. One scoping refinement the claim omits: llvmkit computes `reachable` via `cx.dom_tree.is_reachable_from_entry(bb)` (verifier.rs:1130), and a reachable non-entry block necessarily has at least one predecessor. So the observable accept/reject split is confined to the **entry block** — the only block that is both reachable and predecessor-free. For a reachable block that does have predecessors, both verifiers reject a zero-incoming phi; they merely differ in which rule fires (llvmkit's `PhiEmptyInReachableBlock` runs before the delegation, pre-empting the `PhiPredecessorMismatch` that would otherwise report 0 incomings vs. N predecessors — a diagnostic-level divergence, not an accept/reject one). Finally, this is a deliberate, self-documented divergence rather than an oversight: the comment block opens "Defense in depth (stricter than upstream)" and justifies it on round-trip grounds — an empty phi prints as `%p = phi i32` with no `[ … ]` pairs, which `LLParser::parsePHI` rejects, so accepting it would produce un-reparseable output.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/verifier.rs:2962-2978 — the cited range is present verbatim and unmodified (verifier.rs is not in the working-tree diff; last touched by commit 85c3357). `check_phi` takes a `reachable: bool` parameter (line 2915) and runs, before delegating to `check_phi_incoming` at line 2981: `if reachable && incoming.is_empty() { return Err(self.fail(f, bb, VerifierRule::PhiEmptyInReachableBlock, "phi in a block reachable from entry has no incoming values".into())); }`. verifier.rs:1130 — the caller: `let reachable = cx.dom_tree.is_reachable_from_entry(bb);` then `self.check_phi(f, bb, inst, p, cx.predecessors, reachable)`. dominator_tree.rs:189-194 — `is_reachable_from_entry` is set membership in `self.reachable`, which includes the entry block. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Verifier.cpp, `Verifier::visitPHINode` (~line 3806) — full body read: grouping check, `!PN.getType()->isTokenLikeTy()`, per-incoming type equality, then `// All other PHI node constraints are checked in the visitBasicBlock method.` No count check, and no non-empty requirement anywhere. Same file, `Verifier::visitBasicBlock` (~line 3381-3389) — the real count guard, reached only under `if (isa<PHINode>(BB.front()))`: `Check(PN.getNumIncomingValues() == Preds.size(), "PHINode should have one entry for each predecessor of its parent basic block!", &PN);`. With zero incomings and zero predecessors this is `0 == 0`, so nothing in LLVM's verifier rejects the IR. crates/llvmkit-ir/src/error.rs:234 and :487 — `VerifierRule::PhiEmptyInReachableBlock` variant and its message are live. crates/llvmkit-ir/src/phi_raw_tests/zero_incoming_phi.rs:297-330 — `zero_incoming_phi_in_reachable_block_is_rejected` asserts the rule fires; :338 `zero_incoming_phi_in_unreachable_block_is_accepted` pins the reachability gate. Notably the "rejected" test uses a non-entry block with one predecessor, a case LLVM also rejects, so llvmkit's own test does not cover the genuinely divergent entry-block case. A grep of verifier.rs for entry-block-specific phi rules found none, confirming nothing else would independently reject an entry-block phi in either implementation.

</details>

### 15. A forward-referenced function is a typed `Function`, so a later definition cannot change its signature

*parser — forward references* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_direct_callee`, `parse_declare`/`parse_define` reuse path); crates/llvmkit-ir/src/module.rs (`add_function_dyn`)

- **LLVM:** `LLParser::getGlobalVal` mints an *untyped* placeholder (a `ptr`-typed `GlobalVariable`), so `parseFunctionHeader` compares only `FwdFn->getType() != PFT` — after opaque pointers, nothing but the address space. A call whose arguments disagree with the eventual definition is accepted by the parser and rejected by the Verifier (`Call parameter type does not match function signature!`).
- **llvmkit:** `parse_direct_callee`'s forward-reference arm calls `Module::add_function_dyn` with the *call site's* signature, so the placeholder is a real `Function` with a real `FunctionType`. `declare`/`define` then reject a signature mismatch with two invented texts — `forward function declaration with matching signature` / `forward function definition with matching signature` — neither text nor rule upstream's.
- **Why:** Recorded in docs/future-work.md (W8): the check is load-bearing for the representation, not just the diagnostic — dropping it would leave a call wired to a function whose type it does not match. It also blocks the three per-site forward-reference texts W2.5 carried.
- **Fix:** Apply W2's value-forward-reference shape (untyped placeholder + RAUW at definition) to the *callee* position so `parseFunctionHeader` can create a fresh `Function` and re-point the call. That unblocks `invalid forward reference to function '<n>' with wrong type: expected 'T' but was 'U'` (fixture `opaque-ptr-invalid-forward-ref.ll` is vendored and waiting), `type of definition and forward reference of '@N' disagree`, and the global/alias twins — plus the callee-normalization sweep and the retirement of `forward_function_decls`, both carried from W2.
- **Correction from verification:** Substantively accurate; two citation refinements. (1) The symbol is `resolve_direct_callee` (the token peek lives in `parse_direct_callee_ref`), not `parse_direct_callee`. (2) llvmkit is not missing untyped-placeholder machinery in general — `global_forward_ref` / `forward_ref_value_placeholder` / `resolve_global_forward_ref` (ll_parser.rs:1714-1810) mirror `createGlobalFwdRef` + RAUW and serve the ordinary global-operand paths; it is specifically the `call`/`invoke`/`callbr` callee position that bypasses them for `add_function_dyn`. Related unclaimed extra: a *numbered* forward-referenced callee (`call void @0()` before `define void @0()`) is not even forward-referenced — the `ParsedDirectCallee::Id` arm returns `UndefinedSymbol` outright, where upstream's `getGlobalVal(unsigned ID, ...)` mints a placeholder.

<details><summary>Verification evidence</summary>

llvmkit: C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs — `resolve_direct_callee`'s NonIntrinsic forward-reference arm (~line 13362) calls `self.module.add_function_dyn(&name, parsed_fn_ty, Linkage::External)` with the call site's signature and records the name in `forward_function_decls`; `add_function_dyn` (crates/llvmkit-ir/src/module.rs:4052) forwards to `declare_function::<Dyn>`, so the placeholder is a real Function with a real FunctionType. The reuse path in `parse_declare` (10473-10479) guards `existing.signature() != fn_ty || existing.basic_blocks().len() != 0` and errors with the literal text "forward function declaration with matching signature"; `parse_define` (10697-10703) guards `existing.signature() != fn_ty || existing.basic_blocks().any(|bb| !bb.is_empty())` and errors with "forward function definition with matching signature". `check_function_redefinition` (9011) exempts names in `forward_function_decls`, so those two branches are the ones that fire. Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp — `createGlobalFwdRef` (1762) returns `new GlobalVariable(*M, Type::getInt8Ty(...), false, ExternalWeakLinkage, ...)` with the comment "The used global type does not matter. We will later RAUW it"; `getGlobalVal` (1788, 1819) returns it; `parseFunctionHeader` (6860-6905) tests only `FwdFn->getType() != PFT` and then unconditionally `Function::Create`s a fresh function (6907) and RAUWs/erases the placeholder (6947-6950). Upstream's texts are "invalid forward reference to function '...' with wrong type: expected ... but was ..." and "type of definition and forward reference of '@N' disagree"; neither llvmkit text occurs upstream. The Verifier is the real gate: orig_cpp/.../llvm/lib/IR/Verifier.cpp:3847 "Call parameter type does not match function signature!". Corroboration: docs/future-work.md:112-145 records this exact gap, and crates/llvmkit-asmparser/tests/fixtures/upstream/opaque-ptr-invalid-forward-ref.ll is vendored but referenced only from that doc (no manifest registration), so it does not run.

</details>

### 16. A self-typed aliasee does not parse; `invalid aliasee` is unreachable

*parser — constants / aliases* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_alias_or_ifunc`, `parse_constant_expr`)

- **LLVM:** `LLParser::parseAliasOrIFunc` branches on the aliasee's first token: `bitcast`, `getelementptr`, `addrspacecast` and `inttoptr` go through a bare `parseValID` ("the bitcast dest type is not present, it is implied by the dest type"); everything else goes through `parseGlobalTypeAndValue`. A non-`t_Constant` result is `invalid aliasee`, and the pointer check plus the address space are taken from the aliasee *value's* type.
- **llvmkit:** `@a = alias i32, bitcast (ptr @g to ptr)` does not parse, nor do the `getelementptr`/`addrspacecast`/`inttoptr` spellings. `invalid aliasee` is therefore unreachable, and the alias address space is not derived from the aliasee.
- **Why:** Recorded in docs/future-work.md (W7): attempted and reverted. The blocker is that `Parser::parse_constant_expr` takes a `result_ty` and llvmkit has no entry point for a constant expression that types itself — every constexpr arm is reached with the demanded type already in hand. That is W4's type-agnostic ValID refactor one level down, i.e. wave-sized, so it was deliberately not smuggled into W7.
- **Fix:** Give `parse_constant_expr` a self-typing entry point (the cast/GEP arms deriving their result type from the written destination type), then add the four-keyword branch in `parse_alias_or_ifunc`, the `t_Constant` check with `invalid aliasee`, the value-typed pointer check, and the aliasee-derived address space. Expect the refactor to surface value bugs, not just missing diagnostics — that is what W4's stages A–C did.
- **Correction from verification:** Still present, but the last clause is wrong. Accurate statement: llvmkit's `parse_alias_or_ifunc` unconditionally calls `parse_type(false)` for the aliasee instead of branching on the first token, so the four bare-`parseValID` aliasee spellings upstream accepts — `bitcast (ptr @g to ptr)`, `getelementptr (...)`, `addrspacecast (...)`, `inttoptr (...)` — all fail with `expected type`, for both `alias` and `ifunc`. Consequently upstream's `invalid aliasee` diagnostic has no counterpart anywhere in llvmkit (the string appears only in a comment). However, "the alias address space is not derived from the aliasee" is FALSE: `GlobalAliasBuilder::new` computes `address_space = pointer_address_space(aliasee.ty()).unwrap_or(0)` and `GlobalIFuncBuilder::new` does the same from the resolver's type — the same source as upstream's `PTy->getAddressSpace()`. That sub-claim should be dropped.

<details><summary>Verification evidence</summary>

Upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp, `LLParser::parseAliasOrIFunc`: the aliasee read tests `Lex.getKind() != lltok::kw_bitcast && != kw_getelementptr && != kw_addrspacecast && != kw_inttoptr` to choose `parseGlobalTypeAndValue`, else bare `parseValID` with the comment "The bitcast dest type is not present, it is implied by the dest type", then `if (ID.Kind != ValID::t_Constant) return error(AliaseeLoc, "invalid aliasee");` and `unsigned AddrSpace = PTy->getAddressSpace();`. llvmkit C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:7099-7113 has no branch — `let target_ty = self.parse_type(false)?;` runs unconditionally; `parse_type` (line 6315) has no `Token::Instruction` arm and its fallback at line 6384 returns `Expected { expected: "type" }`; keywords.rs:39,92,95,96,113 lex all four words as `Token::Instruction`. Grep for "invalid aliasee" across crates/ hits only the comment at ll_parser.rs:7102. A temporary probe test (crates/llvmkit-asmparser/tests/zz_claim68_probe.rs, run under `cargo +1.96.0 test --release`, since deleted) printed: bitcast/getelementptr/addrspacecast/inttoptr -> "ERR: expected type" (including the ifunc spelling), while `ptr getelementptr ([4 x i1], ptr @a, i64 0, i64 2)` and `ptr addrspace(1) @g` parsed and round-tripped. Address space: C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/global_alias.rs:358 `let address_space = pointer_address_space(aliasee.ty()).unwrap_or(0);` (accessor at :111) and global_ifunc.rs:361 the same for the resolver. Also noted: the untracked WIP fixture crates/llvmkit-asmparser/tests/fixtures/upstream/uselistorder/uselistorder.ll line 7 uses the bare-`getelementptr` aliasee, and the working-tree test parser_use_list.rs:358 `the_upstream_uselistorder_fixture_parses_clean` asserts it parses — it cannot pass against the current parser.

</details>

### 18. `parse_uint64` / `parse_uint32` are narrower than `parseUInt64`

*parser — literals* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_uint64`, `parse_uint32`)

- **LLVM:** `LLParser::parseUInt64` accepts any `lltok::APSInt` whose value is unsigned and takes `APSInt::getLimitedValue()`, which **saturates** at `UINT64_MAX`.
- **llvmkit:** `parse_uint64` accepts only a positive **decimal** literal and fails outright when the digits do not fit. So `u0x10` answers `expected integer` where upstream accepts it, and an over-wide literal is rejected where upstream saturates. `parse_uint32` has the same shape (its saturation is unobservable — the range check rejects either way, which is what `align-param-attr-error2.ll` pins).
- **Why:** Recorded in docs/future-work.md (W10): it is a W5-owned routine with 25 call sites, no `test/Assembler` fixture reaches either case, and the honest fix reads the token through `parse_int_literal` (the APSInt token model), which changes where the diagnostic's span comes from — so it was not smuggled into the summary-index wave.
- **Fix:** Route both through `parse_int_literal`/`ParsedApsInt`, accept any unsigned APSInt spelling, and saturate rather than fail on over-wide values; re-check the span of every one of the 25 call sites' diagnostics after the change.
- **Correction from verification:** Substantively accurate and still present; only the supporting citation is wrong. Corrected statement: `LLParser::parseUInt64` accepts any `lltok::APSInt` whose APSInt is unsigned and reads it with the saturating `APSInt::getLimitedValue()`; llvmkit's `parse_uint64` matches only `IntLit { sign: Sign::Pos, base: NumBase::Dec }` and returns `expected integer` when `digits.parse::<u64>()` fails. Two observable divergences confirmed by running the parser: (1) `align u0x8` -> llvmkit `expected integer`, upstream accepts it as align 8, since `LLLexer::lexIdentifier` builds `u0x...` as `APSInt(Tmp, /*isUnsigned=*/true)`; (2) `align 18446744073709551616` -> llvmkit `expected integer`, upstream saturates to UINT64_MAX and then fails later in `parseOptionalAlignment` with `alignment is not a power of two`. `parse_uint32` shares the guard and likewise rejects `addrspace(u0x1)` and `alignstack=u0x8`, both of which upstream accepts; its saturation is indeed unobservable because the >u64 and >u32 paths emit the same `expected 32-bit integer (too large)`. The one factual error: `align-param-attr-error2.ll` is `ptr align ()` and pins parseUInt64's missing-value `expected integer`, NOT parseUInt32's 32-bit range check. What actually pins the range check is llvmkit's own test at crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:882-895 (`attributes #0 = { align = 4294967296 }` -> `expected 32-bit integer (too large)`). Note also that the signed forms (negative decimal, `s0x...`) are rejected by both, so only the unsigned-hex and over-wide-decimal cases diverge.

<details><summary>Verification evidence</summary>

crates/llvmkit-asmparser/src/ll_parser.rs:2216 (`parse_uint32`) and :2244 (`parse_uint64`) both match only `Token::IntegerLit(IntLit { sign: Sign::Pos, base: NumBase::Dec, digits })` and error `expected integer` when `digits.parse::<u64>()` fails — no HexUnsigned arm, no saturation. Upstream orig_cpp/.../llvm/lib/AsmParser/LLParser.cpp:1889-1908 shows both routines gate only on `Lex.getKind() != lltok::APSInt || getAPSIntVal().isSigned()` and then call `getLimitedValue()`; APInt.h:476 defines `getLimitedValue` as `ugt(Limit) ? Limit : getZExtValue()` (saturating); LLLexer.cpp:1062 builds `[us]0x...` as `APSIntVal = APSInt(Tmp, TokStart[0] == 'u')`, so `u0x...` is unsigned and passes upstream's guard; APSInt.cpp:21-38 makes every non-negative decimal unsigned at arbitrary width. llvmkit's own `parse_summary_flag` (ll_parser.rs:3316) ports the identical upstream guard (`LLParser::parseFlag`, LLParser.cpp:10027) and does include `NumBase::Dec | NumBase::HexUnsigned` — proving the token model can express the accepting set that parse_uint64/parse_uint32 omit. Empirically, with `cargo +1.96.0 build --release -p llvmkit-asmparser --example parse_file` and probe files: `align 8` and `align 4294967296` round-trip fine, while `align u0x8`, `align 18446744073709551616`, `addrspace(u0x1)`, and `attributes #0 = { alignstack=u0x8 }` all fail with `expected integer` (exit 1). Upstream fixture orig_cpp/.../llvm/test/Assembler/align-param-attr-error2.ll reads `define void @missing_value(ptr align () %ptr)` with `CHECK: error: expected integer`, i.e. it pins parseUInt64's empty-parens case, not a 32-bit range check.

</details>

### 19. AutoUpgrade does not exist — legacy-but-valid modules are not upgraded — **PARTLY FIXED (W13d)**

**Status 2026-08-16 (W13d).** `crates/llvmkit-ir/src/auto_upgrade.rs` now exists
under upstream's `lib/IR` layering and carries three of the nine call sites:
`UpgradeTBAANode` (fed by the parser's new `insts_with_tbaa_tag`, upstream's
`InstsWithTBAATag`), `UpgradeModuleFlags` and `UpgradeSectionAttributes`, each
wired at its own position in `Parser::parse_module_with_config`'s end-of-module
block. The **count is nine, verified again** — see the correction below. The six
that remain (`UpgradeIntrinsicFunction`, `UpgradeIntrinsicCall`,
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

### 20. The function-header attribute list is gated behind a hand-maintained lookahead

*parser — attributes* — crates/llvmkit-asmparser/src/ll_parser.rs (`keyword_starts_attribute`, `parse_optional_function_suffix`)

- **LLVM:** `LLParser::parseFunctionHeader` enters `parseFnAttributeValuePairs` unconditionally and lets `tokenToAttribute`'s fall-through end the loop. There is no lookahead.
- **llvmkit:** `Parser::keyword_starts_attribute` is a second copy of the loop's arm list. It was already wrong once: `uwtable`, `allocsize`, `vscale_range`, `allockind`, `nofpclass`, `dereferenceable`, `captures`, `range`, `initializes` and the six type attributes were all missing, so `define void @f() uwtable {` — plain clang output — was never even attempted and failed as `expected '{' to open function body`.
- **Why:** Recorded in docs/future-work.md (W5): the structural fix is to delete the lookahead, but that needs `parse_optional_function_suffix`'s `align`-is-not-an-attribute carve-out to survive, which was why W5 stopped at re-syncing the table.
- **Fix:** Enter the attribute loop unconditionally and let its `_ => break` arm end it, preserving the `align` carve-out (upstream keeps `align` out of the group and moves it to the field). Delete `keyword_starts_attribute`.
- **Correction from verification:** Still present as a structural divergence, but the concrete failure the claim cites is fixed and the claim's "Where" is slightly off. Accurate today: llvmkit gates the function-header attribute list behind `Parser::is_attr_start` / `Parser::keyword_starts_attribute` (ll_parser.rs:9274-9310), a hand-maintained second copy of `parse_fn_attribute_value_pairs`'s arm list, called from `parse_optional_function_header_attrs` (9312) — which `parse_optional_function_suffix` (9363) calls, so the gate is not in `parse_optional_function_suffix` itself. Upstream `LLParser::parseFunctionHeader` (LLParser.cpp:6725) enters `parseFnAttributeValuePairs` unconditionally as one term of its `||` chain and lets `tokenToAttribute`'s fall-through end the loop; that half of the claim is exact. The missing keywords are no longer missing: `uwtable`, `allocsize`, `vscale_range`, `allockind`, `nofpclass`, `dereferenceable`, `dereferenceable_or_null`, `captures`, `range`, `initializes` and all six type attributes (`byval`, `byref`, `inalloca`, `sret`, `preallocated`, `elementtype`) are in the list at ll_parser.rs:9282-9301, landed in 8c5f0ab/809ff3e (W5), with `argument_carrying_attributes_parse_on_a_function_header` (parser_attribute_matrix.rs:791) pinning `define void @f() uwtable {`. Arm-by-arm the predicate is in sync with the loop today, and because it delegates to the same `attr_kind_for_keyword` / `legacy_memory_effects` tables the loop's default arm uses, no input I could construct behaves differently. The divergence is therefore a maintenance/structure hazard (a duplicated list that must be kept in sync by hand), not a current behavioral difference — which is how docs/future-work.md:236-256 already records it.

<details><summary>Verification evidence</summary>

orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp:6725 `LLParser::parseFunctionHeader` — `parseArgumentList(...) || parseOptionalUnnamedAddr(...) || parseOptionalProgramAddrSpace(...) || parseFnAttributeValuePairs(FuncAttrs, FwdRefAttrGrps, false, BuiltinLoc) || (EatIfPresent(lltok::kw_section) && ...) ...`, one unconditional `||` chain with no predicate; `parseFnAttributeValuePairs` (LLParser.cpp:1692) ends via `tokenToAttribute` returning `Attribute::None` (error in a group, `break` outside). crates/llvmkit-asmparser/src/ll_parser.rs:9274 `fn keyword_starts_attribute(keyword: Keyword) -> bool` — `attr_kind_for_keyword(..).is_some() || legacy_memory_effects(..).is_some()` plus a `matches!` over Align, Alignstack, Memory, Nofpclass, Uwtable, Dereferenceable, DereferenceableOrNull, Byval, Byref, Inalloca, Sret, Preallocated, Elementtype, Captures, Range, Initializes, Allocsize, VscaleRange, Allockind; ll_parser.rs:9304 `is_attr_start`; ll_parser.rs:9324 `while self.is_attr_start() {`; ll_parser.rs:9367 the suffix's first statement calls it. Cross-checked against every `Token::Kw(...)` arm of `parse_fn_attribute_value_pairs` (ll_parser.rs:9437-9683) — all covered. crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:791-806 parses `define void @f() {uwtable, uwtable(sync), allocsize(0), allocsize(0, 1), vscale_range(1, 16), allockind("alloc"), memory(read), alignstack(16), nocallback} { ret void }`; UPSTREAM.md:1116 is its provenance row. `git log -S"Keyword::Uwtable" -- crates/llvmkit-asmparser/src/ll_parser.rs` → 809ff3e, 8c5f0ab. docs/future-work.md:236-256 records the remaining structural item.

</details>

### 101. Six `LLLexer::LexError` sites are non-fatal upstream and fatal (or absent) here

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
- **Fix:** needs the error-*retention* model upstream has and llvmkit does
  not: a single recorded diagnostic with a priority, consulted only if the
  parse fails. That is the same missing machinery entry 32 needs, and the two
  should land together. Reproducing the truncation without it would trade a
  wrong rejection for a silent wrong value, which is worse. Until then the two
  unconstructed variants should either gain their sites or be deleted — a
  public variant nothing produces is a claim the tree does not honour.

### 107. `invoke %named.struct @f(…)` does not parse — the return type is eaten as a result name

*parser — instruction dispatch* — crates/llvmkit-asmparser/src/ll_parser.rs (`Parser::parse_lhs_before_invoke`)

Found 2026-08-20 while fixing the `catchswitch` dispatch (0.0.4 funclet
parity); a direct consequence of the split instruction dispatch recorded in
[`future-work.md`](future-work.md).

- **LLVM:** `LLParser::parseBasicBlock` strips the optional `%name =` **before**
  `parseInstruction` runs, and `LLParser::parseInvoke` then reads the return
  type with `parseType`. It never looks for a result name after the opcode, so
  a `%`-sigil token in return-type position is unambiguously a named type.
- **llvmkit:** `parse_lhs_before_invoke` bumps `invoke` and *then* runs
  `parse_lhs_assignment`, so `%struct.S` in return-type position is read as a
  result name. Probe: `invoke %struct.S @f() to label %ok unwind label %lpad`
  answers `expected '=' after local SSA name`, anchored on `@f`.
- **Why:** llvmkit dispatches terminators before the result name is consumed,
  so `invoke` needs a helper that re-derives the name after the opcode. The
  helper cannot tell a return type from a result name.
- **Fix:** falls out for free from the dispatch hoist in
  [`future-work.md`](future-work.md) — move `parse_lhs_assignment` above the
  terminator `match` and delete `parse_lhs_before_invoke`. Not fixable in
  isolation without duplicating the lookahead.

## Accepts invalid input

llvmkit accepts IR that LLVM rejects, so a malformed module survives into the rest of the pipeline.

### 125. `Verifier::visitCallBase`'s operand-bundle loop is unported, so no bundle rule is enforced

*verifier — call family* — crates/llvmkit-ir/src/verifier.rs (`check_call`, `check_invoke`, `check_callbr`)

Found 2026-08-21 while porting `LLParser::parseCall`'s construction tail: the
change made operand bundles reach an *indirect* call for the first time, and
two vendored fixtures exist whose whole point is the rules that then do not
fire. Disclosed until now only in those tests' rustdoc.

- **LLVM:** `Verifier::visitCallBase` walks `Call.getOperandBundleAt(i)` and switches on the tag, rejecting a duplicate of each at-most-once tag (`deopt`, `gc-transition`, `funclet`, `cfguardtarget`, `ptrauth`, `kcfi`, `preallocated`, `gc-live`, `clang.arc.attachedcall`) and checking each one's operand count and operand types; after the loop it rejects a `ptrauth` bundle on a direct call.
- **llvmkit:** the loop has no counterpart. `check_call`, `check_invoke` and `check_callbr` never read `c.attrs.operand_bundles_slice()` — **no** rule from that loop is enforced, in any category. Read that as a blanket statement rather than a list to check against: it stays true as upstream grows the switch, which a list would not.
- **Cost, exactly:** the two fixtures vendored under `tests/fixtures/upstream/LLParser-parseCall/` for the erased-callee `call` work — `test/Verifier/kcfi-operand-bundles.ll` and `test/Verifier/ptrauth-operand-bundles.ll` — are `RUN: not opt -passes=verify`, so their verdict half cannot be ported. Between them their `CHECK:` lines pin six diagnostics (`grep -h '^; CHECK: ' <the two fixtures> | sed 's/^; CHECK: //' | sort -u`): `Direct call cannot have a ptrauth bundle`, `Kcfi bundle operand must be an i32 constant`, `Multiple kcfi operand bundles`, `Multiple ptrauth operand bundles`, `Ptrauth bundle discriminator operand must be an i64`, `Ptrauth bundle key operand must be an i32 constant`. llvmkit's `parser_calls.rs` tests over those files therefore assert the `CHECK-NEXT` instruction text only — which is `AsmWriter` output, and is what the erased-callee fix made correct.
- **Why:** the verifier's call chapter grew from `Verifier::visitCallBase`'s type and attribute checks; operand bundles were a *parser* and *printer* feature until this change and never had a verifier arm at all.
- **Fix:** port the loop whole — it is one `for` over the bundle list with one `if`/`else if` chain, and `CallAttributeData::operand_bundles_slice()` already carries the data on `call`, `invoke` and `callbr`. `verifyAttachedCallBundle`, which the `clang.arc.attachedcall` arm calls, is a second routine and can follow. The `funclet` arm's `isa<FuncletPadInst>` check overlaps entry 112, which is about `visitIntrinsicCall`'s *missing*-funclet rule rather than a malformed one; they are separate arms and neither subsumes the other.

**Live, self-checking:** `crates/llvmkit-asmparser/tests/parser_calls.rs::call_operand_bundle_rules_are_not_diagnosed` parses both vendored fixtures, calls `Module::verify_borrowed` and asserts `is_ok()` on each. It is green in the gate, so this entry's "llvmkit accepts them" half is pinned by a test rather than quoted from a probe, and it fails the moment any of the six rules lands.

### 127. `resolveFunctionType` builds a **vararg** call-site type for a short-syntax `musttail` forwarding call

*parser — call family* — crates/llvmkit-asmparser/src/ll_parser.rs (`Parser::parse_call`, its `resolveFunctionType` arm)

Found 2026-08-22 in the documentation pass over the divergence-closing branch,
walking `LLParser::resolveFunctionType` arm for arm. Pre-existing — the same
argument is threaded at `f07f817` — and unrecorded until now.

- **LLVM:** `LLParser::resolveFunctionType` builds the call-site signature from the *argument* types with the variadic bit hardcoded off: `FuncTy = FunctionType::get(RetType, ParamTypes, false);`. A trailing `...` in a `musttail` argument list is consumed by `parseParameterList` and contributes no `ParamInfo`, so it never reaches `ParamTypes` and never sets the bit. `AsmWriter` then prints `TypePrinter.print(FTy->isVarArg() ? FTy : RetTy, Out)` — the short form — and `verifyMustTailCall` rejects the module, because `CallerTy->isVarArg() == CalleeTy->isVarArg()` fails against the vararg caller.
- **llvmkit:** `parse_call` passes its own `var_args` flag — set by the trailing `...` — into `function_type_with_variadic`, so the call-site type keeps the variadic bit. `parse_invoke` and `parse_callbr` do **not**: both bind `let var_args = false;` and reject a forwarding ellipsis outright, which is upstream's shape. Only the `call` arm diverges.
- **Consequence:** accepts-invalid, plus different printed bytes on the same input. Probed at this commit with `target/release/examples/parse_file.exe` on `declare void @f(i32, ...)` / `define void @g(i32 %a, ...)` / `musttail call void @f(i32 %a, ...)`: llvmkit parses, verifies and prints `musttail call void (i32, ...) @f(i32 %a, ...)`, where upstream's `llvm-as` rejects the module at verification. Reachable only from the **short** syntax — with an explicit function type (`musttail call void (i32, ...) @f(...)`) the `AnyTypeEnum::Function` arm is taken and `resolveFunctionType` is not involved, which is the form every in-tree musttail test and every vendored fixture uses (`rg --no-ignore -n -- "musttail call.*\.\.\." orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/` returns only explicit-function-type forms).
- **Why:** llvmkit has no `verifyMustTailCall`, so keeping the variadic bit is what makes the printed form re-parse. Passing `false` here in isolation is **not** the fix: the call-site type would become `void (i32)`, the printer would drop the `...` as upstream's does, and re-parsing that output would then hit `parse_call`'s own `expected '...' at end of argument list for musttail call in varargs function`. It would trade an accepts-invalid for a round-trip break.
- **Fix:** both halves in one change — hardcode `false` the way `resolveFunctionType` does, *and* port `Verifier::verifyMustTailCall`'s `isVarArg` agreement `Check`, so llvmkit rejects the input at verification instead of printing something upstream never produces. Neither half stands alone.

### 21. An inline-asm call's per-operand `elementtype` rules are not verified

*verifier* — crates/llvmkit-ir/src/verifier.rs:2775-2779 (call arm), :3230-3240 (callbr arm); attributes reach the instruction via crates/llvmkit-asmparser/src/ll_parser.rs:12972 and crates/llvmkit-ir/src/instr_types.rs:2215 (`arg_attrs()`)

- **LLVM:** `Verifier::verifyInlineAsmCall` walks `IA->ParseConstraints()` and, for each constraint with an argument, checks three things: an indirect constraint's operand has pointer type (`Operand for indirect constraint must have pointer type`), an indirect constraint's operand carries an `elementtype` attribute (`Operand for indirect constraint must have elementtype attribute`), and a non-indirect constraint's operand does **not** (`Elementtype attribute can only be applied for indirect constraints`).
- **llvmkit:** llvmkit's verifier implements only the label half of the same routine — `label_constraint_count() != 0` for `call`, `!= indirect_dests.len()` for `callbr`. The three `elementtype` checks are absent, so `call void asm sideeffect "", "=*m"(ptr %p)` with no `elementtype` (and the inverse, `elementtype` on a direct constraint) verifies clean here and is rejected by upstream.
- **Why:** Recorded reason: "the call surface cannot spell per-operand `elementtype` attributes yet". **That premise is stale** — the parser reads per-argument attribute lists into `CallAttributeData::arg_attrs`, `Keyword::Elementtype` is one of the type attributes it accepts, and the AsmWriter prints them back. Nothing blocks the check today. [[verify-recorded-premises]]
- **Fix:** In both verifier arms, iterate `InlineAsm::parse_constraints()` alongside `attrs.arg_attrs()` exactly as upstream walks `ArgNo`, skipping label and no-argument constraints, and emit the three messages as new `VerifierRule` variants. Port the fixtures from `test/Verifier/inline-asm-*.ll` in the same commit.

<details><summary>Verification evidence</summary>

Upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Verifier.cpp, Verifier::verifyInlineAsmCall (~line 2799): loops ParseConstraints(), skips labels (counting them) and !CI.hasArg(); for CI.isIndirect it Checks Arg->getType()->isPointerTy() ("Operand for indirect constraint must have pointer type") and Call.getParamElementType(ArgNo) ("Operand for indirect constraint must have elementtype attribute"); the else arm Checks !Call.paramHasAttr(ArgNo, Attribute::ElementType) ("Elementtype attribute can only be applied for indirect constraints"). The label-count comparison (LabelNo == 0 for call, == CallBr->getNumIndirectDests() for callbr) is the tail of the same function. llvmkit C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/verifier.rs implements only that tail: line 2778 `if inline_asm.label_constraint_count() != 0` in check_call, line 3238 `if inline_asm.label_constraint_count() != d.indirect_dests.len()` in the callbr arm. Lines 2786-2787 carry an explicit in-code deferral: "// Full indirect-constraint / elementtype parity is deferred: the // current call surface cannot spell per-operand elementtype attrs." Grep of the entire crates/ tree for "indirect constraint" and "Elementtype attribute can only" returns zero hits, and verifier.rs contains no `AttrKind::` reference anywhere, so it never inspects call parameter attributes at all — nothing else in the verifier could reject these programs. `git status --porcelain crates/llvmkit-ir/src/verifier.rs` is empty, so this is committed state. Plumbing cited by the claim confirmed: crates/llvmkit-asmparser/src/ll_parser.rs line ~9613 parses `elementtype(T)` into AttrKind::ElementType; line 12974 (claim said 12972 — two lines off, not substantive) does `let one_arg_attrs = self.parse_optional_param_attrs()?;` per argument, fed into CallAttributeData::new(...) at line 13002; crates/llvmkit-ir/src/instr_types.rs:2215 `pub fn arg_attrs(&self) -> &[AttributeStorage]` exposes them (asm_writer.rs:2080 already prints from it). crates/llvmkit-ir/src/inline_asm.rs:240 `constraint_info()` and the `is_indirect` field of ConstraintInfo (line 353) supply the constraint half. Extra finding (not part of the claim, but stale premise): the recorded rationale for the deferral — in-code at verifier.rs:2786-2787 and in docs/future-work.md:1283-1285, "the call surface cannot spell per-operand elementtype attributes yet" — is no longer true. The parser stores per-arg attributes, and ir_builder.rs:9513, :9710, :9817 all expose `call_attributes(CallAttributeData)` on the call/invoke/callbr builders. The only missing piece is a `has_arg()` equivalent on ConstraintInfo (upstream's `Type == isInput || (Type == isOutput && isIndirect)`), which inline_asm.rs does not define.

</details>

### 22. A non-uniform scalable-vector constant is constructible through the IR builder and prints text neither LLVM nor llvmkit's own parser can read

*IR model* — crates/llvmkit-ir/src/constants.rs:957-983 (`const_vector`), crates/llvmkit-ir/src/asm_writer.rs:1272-1278 (`prints_as_splat`) and :1301 (`fmt_aggregate_constant`)

- **LLVM:** `ConstantVector::get` takes a fixed element count, so LLVM has no element-list constant form for a scalable vector at all; a scalable constant can only be a splat, and `AsmWriter`'s `splat (…)` shorthand is the only spelling. There is consequently no upstream rule against a non-uniform scalable vector — the constant cannot be built.
- **llvmkit:** `VectorType::const_vector` skips its element-count check for scalable types (`if !self.is_scalable() && n != expected`) and requires nothing of the lanes, so `<vscale x 4 x i32> <i32 7, i32 8, i32 7, i32 7>` builds. (The same text does **not** parse — see the correction below.) `prints_as_splat` collapses a *uniform* scalable vector to `splat (…)`, but a non-uniform one falls through `fmt_aggregate_constant` to the element-list fallback and prints text LLVM would reject — output that also quietly asserts `vscale == 1`.
- **Why:** Recorded, and explicitly left as a policy decision rather than an oversight: requiring uniformity was tried and reverted because two deliberate tests take the permissive behaviour as their *premise* — `scalable_i1_non_splat_divrem_does_not_use_scalar_i1_shortcuts` and `scalable_vector_fsub_negative_zero_pattern_controls_undef_fold` in `crates/llvmkit-ir/tests/constant_fold.rs` build non-uniform scalable vectors to check that the folder declines. The entry says: decide before changing anything. Printing it losslessly was judged better than collapsing it to a `splat (…)` it is not.
- **Fix:** Decide the representation policy first. If uniformity becomes required, add the lane-agreement check to `const_vector` (and the parser's aggregate path), and rewrite the two `constant_fold.rs` tests whose premise it removes — they become unnecessary rather than merely red. If it stays permissive, the printer needs a spelling that is at least not *invalid* IR, or the case must be made unreachable from the parser alone. Either way, delete the stale note on `constant_fold::vector_splat_constant`, which still says the splat collapse covers only integer and floating-point elements so a uniform scalable vector of pointers or `undef` prints as an element list; `prints_as_splat` returns `true` for every scalable element category.
- **Correction from verification (2026-08-21: the title and the `llvmkit:` bullet above were corrected in place from this block — "and parses" was false, and the `.ll` parser is not an entry point to the bad constant; re-probed at that date, `@g = global <vscale x 4 x i32> <i32 7, i32 8, i32 7, i32 7>` still fails with `constant expression type mismatch`):** What holds (verified by running it): - `VectorType::const_vector` skips the element-count check for scalable types (`crates/llvmkit-ir/src/constants.rs:977`, `if !self.is_scalable() && n != expected`) and requires nothing of the lanes. Building `<vscale x 4 x i32>` from `[7, 8, 7, 7]` succeeds and prints `<vscale x 4 x i32> <i32 7, i32 8, i32 7, i32 7>`. - `prints_as_splat` (`asm_writer.rs:1272-1278`) returns true unconditionally for `ScalableVector`, but only fires behind `aggregate_splat_id`, so a non-uniform vector falls through to the element-list fallback at `asm_writer.rs:1301`/`:1319-1342`. Cited line numbers are accurate. - Upstream confirmed: `ConstantVector::get` (orig_cpp/.../lib/IR/Constants.cpp:1444) always builds `FixedVectorType::get(V.front()->getType(), V.size())`, so LLVM has no element-list scalable constant and hence no rule against a non-uniform one. `ConstantVector::getSplat(ElementCount, …)` is the scalable path. What is wrong: - "and parses" is false. `@g = global <vscale x 4 x i32> <i32 7, i32 8, i32 7, i32 7>` fails with `constant expression type mismatch: got type '<4 x i32>' but expected '<vscale x 4 x i32>'`, as does the same constant in a function body. The parser's `<…>` arm builds a FIXED vector (`ll_parser.rs:7560-7563`, `self.module.vector_type(element_ty, len)`), mirroring upstream's `ConstantVector::get` → `FixedVectorType::get`, and `checked_constant_type` (`ll_parser.rs:7940-7956`) then compares types exactly — a faithful port of `convertValIDToValue`'s `t_Constant` arm (LLParser.cpp:6610-6614). llvmkit's parser matches upstream here. - `docs/future-work.md:429` states this text "parses today". That recorded premise is stale/wrong and should be corrected — it is likely where the claim came from. Two things the claim understates: - The lane count is unchecked entirely, not just "not required to be uniform": `<vscale x 4 x i32>` accepts a 2-element list and prints `<vscale x 4 x i32> <i32 7, i32 8>`. So the malformed shape is wider than a same-length non-uniform list. - Because the parser rejects the printed text, this is a genuine print/parse round-trip break in llvmkit's own contract, not merely "text LLVM would reject". The "quietly asserts vscale == 1" characterization is a fair reading of the semantics and matches the reasoning already recorded in the codebase's own comment at asm_writer.rs:1310-1318.

<details><summary>Verification evidence</summary>

1. crates/llvmkit-ir/src/constants.rs:955-985 (`VectorType::const_vector`) — the guard reads `if !self.is_scalable() && n != expected { return Err(OperandWidthMismatch) }`, so for a scalable type neither the count nor lane uniformity is checked. 2. crates/llvmkit-ir/src/asm_writer.rs:1272-1278 (`prints_as_splat`) returns `true` for `TypeData::ScalableVector`, but is only reached via `aggregate_splat_id` at :1301, which returns `None` when lanes differ; the element-list fallback at :1319-1342 then emits `<`…`>`. The in-tree comment at :1310-1318 explicitly acknowledges this prints invalid IR and names `const_vector`'s permissiveness as the real cause. 3. Ran a temporary probe under `cargo +1.96.0 test --release -p llvmkit-ir` (since deleted). Output: NONUNIFORM (4 lanes): <vscale x 4 x i32> <i32 7, i32 8, i32 7, i32 7> NONUNIFORM (2 lanes for vscale x 4): <vscale x 4 x i32> <i32 7, i32 8> UNIFORM: <vscale x 4 x i32> splat (i32 7) FIXED WRONG COUNT: Err(OperandWidthMismatch { lhs: 4, rhs: 2 }) 4. Ran a temporary probe under `cargo +1.96.0 test --release -p llvmkit-asmparser` (since deleted) calling `parser::parse_dynamic`. Output: GLOBAL RESULT: Err(Message { message: "constant expression type mismatch: got type '<4 x i32>' but expected '<vscale x 4 x i32>'" }) BODY RESULT: Err(same message) SPLAT RESULT: Ok(… "ret <vscale x 4 x i32> splat (i32 7)") This disproves the "and parses" half of the claim. 5. crates/llvmkit-asmparser/src/ll_parser.rs:7560-7563 builds the constant as a fixed vector; :7940-7956 (`checked_constant_type`) rejects any type mismatch; :8010 routes `ValId::Constant` through it. 6. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Constants.cpp:1444-1449 — `ConstantVector::get` always constructs `FixedVectorType::get(V.front()->getType(), V.size())`. 7. orig_cpp/.../llvm/lib/AsmParser/LLParser.cpp:6610-6614 — `convertValIDToValue`'s `t_Constant` arm emits exactly the "constant expression type mismatch" error llvmkit reproduces. 8. orig_cpp/.../llvm/lib/IR/AsmWriter.cpp:1775-1777 — `splat (` shorthand gated on `isa<ConstantInt> || isa<ConstantFP>`, the restriction `is_int_or_fp_splat_value` mirrors. 9. crates/llvmkit-ir/tests/constant_fold.rs:395-417 (`scalable_i1_non_splat_divrem_does_not_use_scalar_i1_shortcuts`) constructs a non-uniform scalable vector via `const_vector` and is part of the passing suite — live proof the permissive behavior is still in effect and depended upon. 10. docs/future-work.md:424-441 records this as an open, deliberately-unresolved policy question ("Decide before changing anything here"), and at :429 makes the incorrect "parses today" assertion.

</details>

### 23. `swifterror` use-site dataflow rules are not enforced

*verifier* — crates/llvmkit-ir/src/verifier.rs:2402-2435 (the alloca-level checks; no use-site walk exists)

- **LLVM:** `Verifier::visitAllocaInst` and the `swifterror` use checks in `Verifier` restrict a swifterror value's flow: it may appear only in specific positions (a `swifterror` call argument, a `load`/`store` of the pointer itself, and so on), and any other use is an error.
- **llvmkit:** llvmkit verifies the parse-level constraints only — a swifterror alloca must have pointer type and must not be an array allocation. A swifterror value used in a position upstream rejects verifies clean here.
- **Why:** Recorded under the 2026-07-06 upstream-parity follow-ups as deliberately deferred; no reason beyond scope is given for the deferral.
- **Fix:** Add a use-site walk keyed on the swifterror alloca/argument: enumerate the legal positions upstream allows, reject everything else as new `VerifierRule` variants, and port the fixtures from `test/Verifier/swifterror*.ll` with their `UPSTREAM.md` rows.
- **Correction from verification:** Substantively accurate; two refinements. (1) The two checks llvmkit does have are verifier-level, not "parse-level" — they live in `Verifier::check_alloca` (crates/llvmkit-ir/src/verifier.rs:2402-2434), a faithful mirror of the first two `Check`s in upstream `visitAllocaInst`. The parser (`LLParser::parseAlloc` mirror at crates/llvmkit-asmparser/src/ll_parser.rs:11811-11849) only eats the `swifterror` keyword and forwards it to `AllocaBuilder::swifterror`; it enforces nothing swifterror-specific. (2) The missing surface is wider than the use-site walk alone. llvmkit omits every swifterror rule except those two: `verifySwiftErrorValue` (the users() walk), `verifySwiftErrorCall` (callsite argument must carry the `swifterror` attribute), the `visitCallBase` "swifterror argument for call has mismatched alloca / should come from an alloca or parameter / mismatched parameter" checks, the per-argument `verifySwiftErrorValue` call in `visitFunction`, and "Cannot have multiple 'swifterror' parameters!". The capability is not the blocker: llvmkit has real use lists (`ValueData::use_list`, `operand_users()`, `Value::users()` — used by asm_writer.rs, demanded_bits.rs, assumptions.rs), so the walk is expressible today.

<details><summary>Verification evidence</summary>

Upstream C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Verifier.cpp: `verifySwiftErrorValue` (users() walk rejecting anything but load/store/call/invoke, requiring the value be the second store operand) and `verifySwiftErrorCall`; `visitAllocaInst` calls `verifySwiftErrorValue(&AI)` right after the pointer-type and array-allocation Checks; `visitFunction` calls it for each `Attribute::SwiftError` parameter; `visitCallBase` adds the mismatched-alloca/parameter checks; `verifyFunctionAttrs` has "Cannot have multiple 'swifterror' parameters!". llvmkit C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/verifier.rs: a case-insensitive grep for swifterror/SwiftError over the whole file returns only lines 2402-2431 — the alloca pointer-type and array-allocation checks, both emitting `VerifierRule::SwiftErrorAlloca`; no use-site walk, no callsite check, no argument-attribute check. Repo-wide grep over crates/llvmkit-ir/src finds swifterror only in asm_writer.rs (printing), attributes.rs (attribute enum + param-only position), instructions.rs / instr_types.rs (the flag), ir_builder.rs (`AllocaBuilder::swifterror`), error.rs (`SwiftErrorAlloca`). verifier.rs's own module header (lines 19-27) lists "Per-function attribute coherence rules" as out of scope. Empirical: a temporary integration test (since deleted) parsed the misuse shapes from orig_cpp/.../llvm/test/Verifier/swifterror.ll through `parse_assembly` + `verify_borrowed` on `cargo +1.96.0 test --release -p llvmkit-asmparser`. Results: gep on a `ptr swifterror` argument, gep on a swifterror alloca, a swifterror alloca used as a store's *value* operand, a plain alloca passed as `ptr swifterror` to a call, a swifterror alloca passed to a call with no swifterror attribute, and a declaration with two `swifterror` parameters all reported PARSED + VERIFIED CLEAN; upstream rejects every one. The control case `alloca swifterror i128` was correctly rejected ("swifterror alloca must have pointer type, got i128"), confirming the probe reached the verifier.

</details>

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

### 112. No funclet-token rule: an intrinsic call inside an EH funclet verifies

*verifier* — crates/llvmkit-ir/src/verifier.rs (the funclet-pad blanket arm at the `CleanupPad | CatchPad | CatchReturn | CleanupReturn | CatchSwitch => Ok(())` match; `check_intrinsic_call`)

Found 2026-08-20 while porting `test/Verifier/operand-bundles-wineh.ll` for the
`catchswitch` work (0.0.4 funclet parity). Disclosed until now only in that
test's rustdoc and in its `mirror (partial)` row in `UPSTREAM.md`.

- **LLVM:** `Verifier::visitIntrinsicCall` rejects an intrinsic call that sits inside an EH funclet and carries no `"funclet"` operand bundle, with `Missing funclet token on intrinsic call`. The fixture is `test/Verifier/operand-bundles-wineh.ll`: `RUN: not opt -passes=verify`, and that diagnostic is its one `CHECK`.
- **llvmkit:** accepts it. Every funclet-pad opcode answers `Ok(())` unconditionally, and `check_intrinsic_call` inspects only signature identity and `immarg` positions — nothing walks a call's enclosing funclet or its operand bundles. Probe: `%2 = call ptr @llvm.objc.retain(ptr null)` inside a `catchpad` funclet verifies clean.
- **Why:** not recorded. The funclet opcodes reached the model and the printer before the verifier had an EH chapter at all.
- **Fix:** port `Verifier::visitIntrinsicCall`'s funclet arm together with the pad colouring it depends on — `Verifier::visitFuncletPadInst`, `Verifier::visitEHPadPredecessors` and `Verifier::verifySiblingFuncletUnwinds`. Until then `parser_eh_funclet.rs::catchswitch_numbered_result` stays `mirror (partial)` and asserts the parse half only.

<details><summary>Verification evidence (verified 2026-08-20; re-anchored to a live test 2026-08-20, fix round 4)</summary>

**Live, self-checking:** `crates/llvmkit-asmparser/tests/parser_eh_funclet.rs::wineh_missing_funclet_token_is_not_diagnosed` parses the vendored `crates/llvmkit-asmparser/tests/fixtures/upstream/Verifier/operand-bundles-wineh.ll` — byte-identical to upstream — calls `Module::verify_borrowed` and asserts `is_ok()`. It is green in the gate, so this entry's `verify_borrowed() == Ok(())` half is now pinned by a test rather than quoted from a probe, and it fails the moment the funclet-token rule lands.

**How it was first found, and what that evidence was worth:** a temporary integration test (`crates/llvmkit-asmparser/tests/zz_probe_verify.rs`) printed `PROBE-RESULT: operand-bundles-wineh VERIFIES (upstream rejects)` and was deleted before the commit — so between then and fix round 4 this entry cited an artifact that no longer existed. The same probe reported that the sibling positive fixture `test/Verifier/preallocated-valid.ll` also verifies; that half *is* still live, through `parser_eh_funclet.rs::catchswitch_in_preallocated_teardown`, which runs `verify_borrowed` on it.

**Upstream side, read at 22.1.4:** the fixture's one `CHECK` directive is `; CHECK: Missing funclet token on intrinsic call`, and it is matched against `not opt -passes=verify` output — so what it pins is the Verifier diagnostic, nothing about the printer.

</details>

## Different diagnostic text

Same verdict, different wording. Upstream's text is contractual, including its own inconsistencies.

### 26. A misplaced `phi` is rejected by the parser, with a message upstream never prints

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:11125-11133 (the `seen_non_phi` guard)

- **LLVM:** `LLParser` accepts a `phi` written after a non-phi instruction and lets `Verifier::visitPHINode` reject it with `PHI nodes not grouped at top of basic block!`.
- **llvmkit:** `parse_basic_block` tracks a `seen_non_phi` flag and rejects at parse time with `phi must be grouped at the top of its basic block`. Same verdict, wrong layer, and a message upstream never prints.
- **Why:** Recorded, with an explicit "do not fix this by deleting the parser check": every phi llvmkit builds goes through `IrBuilder::make_phi_in_block` → `BasicBlock::insert_instruction_at_phi_head`, which places the phi at the block's phi head regardless of the insertion point. Drop the parse check and a misplaced phi is *silently hoisted* into a legal position, so llvmkit's own `VerifierRule::PhiNotAtTop` never fires — accepting invalid IR and quietly rewriting it, strictly worse than the current strictness.
- **Fix:** Add a non-hoisting insertion path for parsed phis so the instruction lands where it was written, then delete the parse-time check and let `VerifierRule::PhiNotAtTop` deliver upstream's verdict and wording. That is entangled with llvmkit's head-phi design — block parameters are operandless head-phis per `IrBuilder::append_block_with_params`, and `insert_instruction_at_phi_head` is the only phi insertion path today — so it wants deciding alongside that model rather than as a parser patch.
- **Correction from verification:** Accurate, with two refinements. (1) The guard now spans lines 11125-11134 (the claim cited 11125-11133; the `else { seen_non_phi = true; }` arm closes at 11134). (2) The message is emitted via `self.expected(...)`, whose ParseError variant renders as `#[error("expected {expected}")]`, so the actual user-visible string is the ungrammatical `expected phi must be grouped at the top of its basic block`, not the bare production the claim quotes. Additional context strengthening the claim: llvmkit already carries the same rule in its verifier (crates/llvmkit-ir/src/verifier.rs:1036-1051, VerifierRule::PhiNotAtTop), so the parse-time guard is strictly redundant with the correct layer, and it prevents a misplaced phi from ever reaching that rule. Worth noting the guard is deliberate, not an oversight: the in-source comment and the test doc both state the rationale (the auto-hoisting phi builders would silently reorder a misplaced phi into valid position, laundering ill-formed .ll into valid IR).

<details><summary>Verification evidence</summary>

llvmkit: C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs — `let mut seen_non_phi = false;` at 10956 (set at 11005, 11013, 11094, 11133) and the guard at 11125-11134: `if matches!(opcode, Opcode::Phi) { if seen_non_phi { return Err(self.expected("phi must be grouped at the top of its basic block")); } } else { seen_non_phi = true; }`. Confirmed also in HEAD (2ac3e3a) via `git show HEAD:...` at lines 11121-11125, so it is not a working-tree-only artifact. Message rendering: crates/llvmkit-asmparser/src/parse_error.rs:171-175 (`#[error("expected {expected}")]` on `ParseError::Expected`). Pinned by test `phi_after_non_phi_is_a_parse_error` at crates/llvmkit-asmparser/tests/parser_errors.rs:77-105, asserting the message contains "phi must be grouped at the top". Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp — `LLParser::parseBasicBlock` (declared at 7050) has no ordering state in its instruction loop, and `LLParser::parsePHI` (8314) rejects only `phi node must have first class type` plus bracket-syntax errors; nothing about placement. orig_cpp/.../llvm/lib/IR/Verifier.cpp:3808-3815 — `Verifier::visitPHINode` holds the check: `Check(&PN == &PN.getParent()->front() || isa<PHINode>(--BasicBlock::iterator(&PN)), "PHI nodes not grouped at top of basic block!", &PN, PN.getParent());`. llvmkit's counterpart verifier rule: crates/llvmkit-ir/src/verifier.rs:1035-1051 and crates/llvmkit-ir/src/error.rs:474.

</details>

### 27. An indirect `callbr` is rejected at parse time rather than by the verifier

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:14295-14301

- **LLVM:** `LLParser::parseCallBr` accepts an indirect callee; `Verifier::visitCallBrInst` is what requires a direct callee for a non-inline-asm callbr (`Callbr: indirect function / invalid signature`).
- **llvmkit:** `parse_callbr` rejects a non-inline-asm callbr whose callee is not a direct function with `expected direct function callee for callbr`. The verdict matches upstream's — the IR is invalid either way — but the layer and the wording do not.
- **Why:** Recorded under the 2026-07-06 upstream-parity follow-ups, which state the trade explicitly: llvmkit reaches the same verdict, and a stricter port would accept it at parse and reject it in the verifier. Noted alongside indirect *invoke*, which is valid IR and is now supported.
- **Fix:** Give the callbr builder an indirect-callee form (the indirect-invoke work is the template), let the parse succeed, and move the rejection into the verifier as a `VerifierRule` carrying upstream's wording — the callbr arm of the verifier already exists at verifier.rs:3230 for the label-count check.

<details><summary>Verification evidence</summary>

Claim is accurate and unchanged. (1) crates/llvmkit-asmparser/src/ll_parser.rs, parse_callbr's `match callee` arm `ParsedCallee::Indirect(_)` (arm ~14293-14301) returns `Err(self.expected("direct function callee for callbr"))` at line 14300 — exactly the cited range and wording. (2) The arm is reachable: resolve_direct_callee (ll_parser.rs:13290) yields ParsedCallee::Indirect for a pointer-typed callee, and `invoke` with an indirect callee round-trips (test invoke_indirect_callee_round_trips), so the rejection is callbr-specific, not a general limitation. (3) The check is absent from llvmkit's verifier: crates/llvmkit-ir/src/verifier.rs:3202 `check_callbr` ports only the default/indirect-destination membership checks and the inline-asm label-constraint check, and its own doc says "Constructive subset"; the string "Callbr: indirect function / invalid signature" appears in crates/ only as a comment (ll_parser.rs:14297) and in a fixture comment, never as a verifier diagnostic. (4) Root cause: the builder cannot express it — crates/llvmkit-ir/src/ir_builder.rs:8407 `callbr_with_config` takes `callee: FunctionValue<'ctx, R2, B>` and :8488 `inline_asm_callbr_with_config` takes an `InlineAsm`; there is no indirect callbr entry point. (5) Upstream confirms the opposite layering: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp:8024 `LLParser::parseCallBr` resolves the callee via `convertValIDToValue(PointerType::getUnqual(Context), CalleeID, Callee, &PFS)` into a bare `Value *Callee` and passes it to `CallBrInst::Create(Ty, Callee, DefaultDest, IndirectDests, Args, BundleList)` with no directness check, while orig_cpp/.../llvm/lib/IR/Verifier.cpp:3492 `Verifier::visitCallBrInst` does `if (!CBI.isInlineAsm()) Check(CBI.getCalledFunction(), "Callbr: indirect function / invalid signature");` — null for a ptr operand. (6) The divergence is deliberately pinned: test callbr_indirect_callee_rejected in crates/llvmkit-asmparser/tests/parser_calls.rs:648-664 asserts the exact message, and its fixture crates/llvmkit-asmparser/tests/fixtures/upstream/LLParser-parseCall/callbr_indirect_callee_rejected.ll labels itself a "llvmkit-specific STRICTNESS lock" documenting that upstream parses it and rejects it in the verifier.

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

### 35. A misplaced `phi` is rejected at parse time, not by the verifier

*parser / IR insertion model* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_basic_block`); crates/llvmkit-ir/src/basic_block.rs (`insert_instruction_at_phi_head`); crates/llvmkit-ir/src/ir_builder.rs (`make_phi_in_block`, `append_block_with_params`)

- **LLVM:** `LLParser` accepts a `phi` written after a non-phi instruction and lets `Verifier::visitPHINode` reject it with `PHI nodes not grouped at top of basic block!`.
- **llvmkit:** `ll_parser.rs::parse_basic_block` rejects it at parse time with `phi must be grouped at the top of its basic block` — a message upstream never prints. Same verdict, wrong layer.
- **Why:** Recorded in docs/future-work.md, and explicitly corrected in the plan (W1 item, `- [x]` with "CORRECTED 2026-08-08 — do not remove"): every phi goes through `IrBuilder::make_phi_in_block` → `BasicBlock::insert_instruction_at_phi_head`, so deleting the parse check makes llvmkit *silently hoist* the phi and its own `VerifierRule::PhiNotAtTop` never fires — accepting invalid IR and rewriting it, strictly worse than the current strictness.
- **Fix:** Add a non-hoisting insertion path for parsed phis so the instruction lands where it was written, then delete the parse-time check and let `VerifierRule::PhiNotAtTop` fire. Entangled with the head-phi/block-parameter model (block parameters are operandless head-phis), so it wants deciding alongside that model rather than as a parser patch.
- **Correction from verification:** Accurate as written; two refinements. (1) The claim is fully confirmed: ll_parser.rs::parse_basic_block (lines 11125-11134) tracks a `seen_non_phi` flag and returns `phi must be grouped at the top of its basic block` when a phi follows a non-phi, while upstream LLParser::parseBasicBlock (LLParser.cpp) inserts every instruction with `Inst->insertInto(BB, BB->end())` and has no ordering check at all, leaving it to Verifier::visitPHINode's `PHI nodes not grouped at top of basic block!`. (2) Worth adding: llvmkit DOES implement the verifier rule (verifier.rs, VerifierRule::PhiNotAtTop), but its rendered text is `PHI nodes not grouped at top of block` (error.rs) -- it drops upstream's word `basic` and the trailing `!`. So the entry understates the divergence slightly: it is both a wrong-layer divergence AND a message-text divergence in the verifier rule that does exist. Also note the recorded rationale for keeping the parser check holds up on inspection: every phi the parser builds routes through IrBuilder::{make_phi_in_block, append_phi_instruction}, both of which call BasicBlock::insert_instruction_at_phi_head unconditionally, so removing the parse-time check would silently hoist a misplaced phi past the verifier rather than reject it.

<details><summary>Verification evidence</summary>

crates/llvmkit-asmparser/src/ll_parser.rs:11125-11134 -- `if matches!(opcode, Opcode::Phi) { if seen_non_phi { return Err(self.expected("phi must be grouped at the top of its basic block")); } } else { seen_non_phi = true; }`; the working-tree diff on that file does not touch this region. Pinned by crates/llvmkit-asmparser/tests/parser_errors.rs:86 `phi_after_non_phi_is_a_parse_error`, which asserts the message text. crates/llvmkit-ir/src/basic_block.rs:863 `insert_instruction_at_phi_head` inserts at the first non-phi position unconditionally; crates/llvmkit-ir/src/ir_builder.rs:931 `make_phi_in_block` and ir_builder.rs:9045 `append_phi_instruction` both call it "regardless of the builder's cursor"; ir_builder.rs:993 `append_block_with_params` exists as cited; ll_parser.rs:12747 `parse_phi` builds every phi through those builder entry points. crates/llvmkit-ir/src/phi_raw_tests/typestate.rs:129 `build_phi_inserts_at_phi_head_not_cursor` demonstrates the hoist and then asserts `m.verify()` succeeds. crates/llvmkit-ir/src/verifier.rs:1036-1050 implements the PhiNotAtTop scan; crates/llvmkit-ir/src/error.rs:474 renders it as "PHI nodes not grouped at top of block". Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp:7050-7158 (parseBasicBlock, `Inst->insertInto(BB, BB->end())`, no ordering check), LLParser.cpp:8314-8356 (parsePHI, only the "phi node must have first class type" error), and lib/IR/Verifier.cpp:3808-3815 (visitPHINode's `Check(&PN == &PN.getParent()->front() || isa<PHINode>(--BasicBlock::iterator(&PN)), "PHI nodes not grouped at top of basic block!", &PN, PN.getParent());`).

</details>

### 36. Global forward references resolve in one end-of-module sweep, not per definition site

*parser — forward references* — crates/llvmkit-asmparser/src/ll_parser.rs:6991-6999 (`forward_ref_globals` guard), :1711-1731 (end-of-module leftovers)

- **LLVM:** `LLParser::getGlobalVal` plus the definition sites compare types where the definition is written, producing `forward reference and definition of global have different types` (at the type location), the alias twin, and `type of definition and forward reference of '@N' disagree`.
- **llvmkit:** A reference to an unknown `@name`/`@N` mints a `ForwardRefValue` at the demanded pointer type and one end-of-module sweep retires them. Same verdicts overall, but upstream's per-site texts are unreachable.
- **Why:** Recorded as a W2.5 correction and carried: "resolution is a single end-of-module sweep, not per-definition-site — same verdicts, but upstream's per-site texts stay unreachable". Closing it shares a root cause with the typed-forward-referenced-function item.
- **Fix:** Move resolution to the definition sites (global/declare/define/alias), comparing the placeholder's demanded type against the definition's at the type location — which also needs the untyped-callee placeholder above for the function twins.
- **Correction from verification:** Substantially accurate and still present, with one wording correction. The sweep design is exactly as described: `global_forward_ref` (ll_parser.rs:1765) mints a placeholder at the demanded pointer type, and `resolve_forward_ref_globals` (:1714), called once at :1475, retires every entry; neither `parse_alias_or_ifunc`'s tail (:7151-7231) nor `parse_declare`'s function tail (:10461-10592) retires a forward ref or compares types at the definition. But "upstream's per-site texts are unreachable" is wrong for one of the three: `forward reference and definition of global have different types` IS implemented, at :1748, emitted from the sweep. What is unreachable is upstream's ANCHOR for it — llvmkit reports at `entry.loc` (where the reference was written), upstream at `TyLoc` (the definition's type). The other two texts are genuinely absent from `crates/`: the alias twin (`...of alias have different types`) and `type of definition and forward reference of '@N' disagree` exist nowhere in llvmkit source (the latter appears only as a backlog note in docs/future-work.md:140). The numbered-function case falls through to the global text from the same sweep; the function-header path instead carries an unrelated llvmkit-only rule, `forward function declaration with matching signature` (:10474-10479), which compares the whole FunctionType where upstream compares only the address space (`FwdFn->getType() != PFT`). Also unclaimed but real: upstream ERASES the map entry at the definition site (`ForwardRefVals.erase(I)`), so a second definition of a forward-referenced name gets `redefinition of global '@g'`; llvmkit's guard at :6991-7000 only calls `.contains_key`, never removes, so the redefinition check stays suppressed for every later definition of that name and the collision surfaces from the builder as `expected valid global definition: ...` — the exact message the guard's own comment says it was added to avoid.

<details><summary>Verification evidence</summary>

crates/llvmkit-asmparser/src/ll_parser.rs — :1765-1809 `global_forward_ref` mints `forward_ref_value_placeholder(ty)` into `forward_ref_globals`/`forward_ref_global_ids` (call sites :8172, :8194, :8223, :8238); :1475 calls `resolve_forward_ref_globals` in the end-of-module tail; :1714-1740 sweeps both maps; :1742-1759 `resolve_global_forward_ref` holds the single `entry.placeholder.ty() != target.ty()` check emitting "forward reference and definition of global have different types" at `entry.loc`; :6991-7000 the `forward_ref_globals.contains_key` guard (reads, never removes); :7151-7231 alias/ifunc definition tail has no forward-ref handling; :10461-10592 function-header tail has none either, only `forward function declaration with matching signature` at :10474-10479. Repo-wide Grep excluding orig_cpp for "alias have different types|type of definition and forward reference|disagree: expected" returns zero hits in crates/. orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp — parseGlobal (read via sed 1370-1460) does `ForwardRefVals.erase(I)` / else `redefinition of global '@'+Name`, then `if (GVal) { if (GVal->getAddressSpace() != AddrSpace) return error(TyLoc, "forward reference and definition of global have different types"); GVal->replaceAllUsesWith(GV); GVal->eraseFromParent(); }`; parseAliasOrIFunc has the same shape at ExplicitTypeLoc with "...of alias have different types"; parseFunctionHeader has `if (FwdFn->getType() != PFT) return error(NameLoc, "type of definition and forward reference of '@N' disagree: expected ... but was ...")` plus `ForwardRefValIDs.erase(I)`. docs/future-work.md:134-145 already records this as an open item with a matching rationale.

</details>

### 37. `intrinsic can only be used as callee` fires at reference time, not at end of module

*parser — intrinsics* — crates/llvmkit-asmparser/src/ll_parser.rs:8158, :8209

- **LLVM:** Upstream auto-declares `llvm.`-prefixed leftovers from call-site function types in `validateEndOfModule`, and the `intrinsic can only be used as callee` rejection happens there, in that ordered sequence.
- **llvmkit:** The message is emitted at the point of reference (two sites), so a construct upstream would only reject at end of module is rejected earlier and the end-of-module error ordering differs.
- **Why:** Recorded as a W2 carried item ("`intrinsic can only be used as callee` still fires at reference time"). Error *ordering* in `validateEndOfModule` is itself part of parity, which is why it routes to W13.
- **Fix:** Fold into W13's `validateEndOfModule` 1:1 sequence: defer the check to the intrinsic auto-declaration step so it fires in upstream's order relative to blockaddress leftovers, dso_local_equivalent resolution, undefined types/comdats and `@` leftovers.
- **Correction from verification:** Accurate, and understated. Two corrections/refinements: (1) The ordering point is right but the mechanism is stronger than "rejected earlier": llvmkit has NO end-of-module `llvm.*` handling at all. `Parser::parse_module`'s end-of-module sequence (crates/llvmkit-asmparser/src/ll_parser.rs:1457-1480) contains no counterpart to upstream's `ForwardRefVals` auto-declaration loop; the guard was relocated wholesale into the two reference-time sites. So the rejection does not merely fire earlier within `validateEndOfModule` — it fires during `parseTopLevelEntities`, ahead of every end-of-module check. (2) The claim misses a second, larger consequence: because the guard is the *first* statement in each function, running before the `self.module.global(&name)` / `function_dyn(&name)` lookups, llvmkit also rejects address-taken references to an intrinsic that IS declared in the module. Upstream's `getGlobalVal` resolves those to the existing `Function` with no `ForwardRefVals` entry, so LLParser accepts them; the rejection comes from the Verifier with a different message, "Invalid user of intrinsic instruction!" (Verifier.cpp:3293, fixture test/Verifier/intrinsic-addr-taken.ll). llvmkit turns a verifier diagnostic into a parse error and renames it. Relatedly, llvmkit's own verifier (crates/llvmkit-ir/src/verifier.rs:965) emits "intrinsic can only be used as callee" where upstream emits "Invalid user of intrinsic instruction!" — a third site the claim does not list.

<details><summary>Verification evidence — three probes re-run 2026-08-21, all still as recorded</summary>

llvmkit source, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs — both cited lines are live and at the exact offsets given. `resolve_global_name_as_value` (:8148) and `resolve_global_name_as_constant` (:8199) each open with `if !matches!(resolve_intrinsic_name(&name), IntrinsicNameResolution::NonIntrinsic) { return Err(ParseError::Message { message: "intrinsic can only be used as callee", ... }) }` at :8153-8161 and :8204-8212, before any module lookup. `IntrinsicId::resolve_name` (crates/llvmkit-ir/src/intrinsics.rs:323) returns NonIntrinsic only for names not starting with "llvm.", so the guard covers every `llvm.`-prefixed name, known or unknown. Callers are `convert_val_id_to_value` (:7972) and `convert_val_id_to_constant` (:8035), both parse-time. Upstream, C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLParser.cpp — the one occurrence of the message is line 341, inside `LLParser::validateEndOfModule`, in the `for (const auto &[Name, Info] : make_early_inc_range(ForwardRefVals))` loop (:328), reached only for names never declared, and ordered after the ForwardRefBlockAddresses guard (:271), the dso_local_equivalent resolution (:302-311), the undefined-type checks (:313-321) and the undefined-comdat check (:323). Fixture test/Assembler/implicit-intrinsic-declaration-invalid2.ll pins that message for an undeclared `@llvm.umax`. Empirical probe (temporary test in crates/llvmkit-asmparser/tests/, run with `cargo +1.96.0 test --release -p llvmkit-asmparser`, since deleted): - `declare i32 @llvm.umax.i32(i32, i32)` + `@g1 = global ptr @llvm.umax.i32` (verbatim test/Verifier/intrinsic-addr-taken.ll) -> llvmkit PARSER errors "intrinsic can only be used as callee"; upstream parses this and the Verifier says "Invalid user of intrinsic instruction!". - `@c = global i32 0, comdat($nope)` alone -> llvmkit errors "use of undefined comdat '$nope'" (so the check exists and works). - `@c = global i32 0, comdat($nope)` followed by `@g = global ptr @llvm.umax` -> llvmkit errors "intrinsic can only be used as callee", preempting the comdat error. Upstream's order (LLParser.cpp:323 before :328) reports the comdat error instead. This is the ordering divergence, demonstrated. - `call i8 @llvm.umax.i8(...)` still parses OK, confirming the callee path routes elsewhere and only non-callee references hit the guard.

</details>

### 38. `validateEndOfModule` is not a 1:1 port, and its error order is not pinned

*parser — end of module* — crates/llvmkit-asmparser/src/ll_parser.rs:1711 (`validate_end_of_module` region), :4786-4798 (comdat guard), :4756 (undefined types)

- **LLVM:** `LLParser::validateEndOfModule` runs a fixed sequence: attribute-group merge + alignment-attr→field move, blockaddress leftovers, `dso_local_equivalent` resolution, undefined numbered/named types, undefined comdats, intrinsic auto-declaration and `@` leftovers, undefined metadata, metadata cycle resolution, the TBAA hook, then SlotMapping steal semantics. Which error fires first is itself observable.
- **llvmkit:** The pieces exist but were landed wave by wave (W2.5 did intrinsic auto-declaration and `@` leftovers; W3 the undefined types; W2.6 comdats), and the sequence was landed before anyone checked it against upstream's — see the Status bullet for where that stands now. The attribute-group merge does not exist. Of upstream's *two* alignment-attribute→field moves, `parseFunctionHeader`'s is ported (in `parse_optional_function_suffix`, with `check_attribute_position` carrying the matching `Alignment` "hack" exemption); the one inside `validateEndOfModule`, which pulls `align` out of an attribute *group* after the merge, is not.
- **Why:** Recorded as W13's opening item, with the ordering explicitly called "part of parity". Its group-merge half is the blocker under the printer's missing attribute-group forming.
- **Fix:** Port the routine as one ordered sequence, add the attr-group merge + `align`-to-field move, and pin the order with negative fixtures that trip two rules at once. Also covers `getIntrinsicSignature` mangling-suffix cases (`llvm.umax` on `i32` declares `llvm.umax.i32`) and the `InstsWithTBAATag` hook W11 was to leave behind.
- **Status (W13a, W13b):** the *sequence* is now upstream's, step by step, and the `dso_local_equivalent` step exists (see D7). The initializer deferral that made step 3 re-mint references after step 4 had run is gone (see D8), so `@g = global ptr blockaddress(@never_defined, %entry)` on its own is rejected rather than printing `<forward reference>`. **Still open:** the attribute-group merge, the `validateEndOfModule` half of the `align` move, the intrinsic auto-declaration loop (entry **37**, which is wider than this bullet — llvmkit's `intrinsic can only be used as callee` fires at reference time and rejects an address-taken reference to a *declared* intrinsic that upstream's parser accepts), metadata-cycle resolution, the TBAA hook and `Slots` steal semantics.

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
> recorded in **D12**, not here.

> **Evidence block removed 2026-08-20 (fix round 3).** It recorded a single verification pass taken before W13a and was superseded by the `Status (W13a, W13b)` paragraph above: its central finding, "11 calls in an order that does not match upstream's", is no longer true, and its llvmkit coordinate for the sequence (`ll_parser.rs:1457-1480`) had drifted into metadata-slot code. The upstream half it cited (`LLParser::validateEndOfModule`, and `parseValID`'s blockaddress leftovers) still holds and is named by symbol in the bullets above.

### 105. A re-used numbered label reports `redefinition of label '%N'` where upstream reports `label expected to be numbered 'N' or greater`

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs — `PerFunctionState::define_basic_block`, `BlockHeader::Numbered` arm

- **LLVM:** `LLParser::PerFunctionState::defineBB`'s numbered branch runs `P.checkValueID(Loc, "label", "", NumberedVals.getNext(), NameID)` *first*. A re-used id is necessarily below `NumberedVals.getNext()`, so `checkValueID` is what fires, with `label expected to be numbered '<next>' or greater`.
- **llvmkit:** a `defined_numbered_blocks` membership test raises `ParseError::Redefinition { kind: SymbolKind::Block, .. }` before `check_value_id` runs, pre-empting upstream's message. `SymbolKind::Block` renders as `label`, so the text a user sees is `redefinition of label '%1'` — verified by probe on `define void @f() {\n1:\n  br label %1\n1:\n  ret void\n}`, which upstream answers with `label expected to be numbered '2' or greater`. The same pre-emptive check also sits in `get_basic_block_numbered`, the `getBB(unsigned ID, LocTy)` mirror.
- **Why:** pre-existing. Carried through unchanged when `defineBB` was unified into one routine for the `printBasicBlock` parity commit, deliberately, so that no diagnostic and no diagnostic *order* moved in a commit whose subject was printed bytes. Keeping the two guards and their order is what makes that diff reviewable as a printing change; it is not an endorsement of them.
- **Fix:** delete the pre-emptive check from both sites and let `check_value_id` speak. Re-bless whatever pins `Redefinition` for a block id first. Sequence it with the already-recorded `unable to create block numbered '<N>'` entry, which rewrites the other guard in the same function.

### 109. Input ending after `%x =` reports `expected instruction opcode`, not upstream's end-of-file message

*parser — instruction dispatch* — crates/llvmkit-asmparser/src/ll_parser.rs (`Parser::parse_basic_block_instructions`, the `Token::Eof` guard)

Found 2026-08-20 while fixing the `catchswitch` dispatch (0.0.4 funclet
parity).

- **LLVM:** the Eof guard is `LLParser::parseInstruction`'s first statement —
  `if (Token == lltok::Eof) return tokError("found end of file when expecting
  more instructions");` — and `parseBasicBlock` has already consumed the
  optional `%name =` by the time it runs. A file ending after `%x =` therefore
  reports the end-of-file message.
- **llvmkit:** the same guard runs at the top of the instruction loop, *before*
  the shared `parse_lhs_assignment`. `%x =` is not `Eof`, so the guard passes,
  the LHS is consumed, and the opcode `match` answers
  `expected instruction opcode`. Probe: a file whose last line is `  %x =`
  reports `3:7: expected instruction opcode`.
- **Why:** llvmkit folded `parseInstruction`'s prologue into the loop header,
  where it sits one step earlier than upstream's.
- **Fix:** move the guard to just after `parse_lhs_assignment`, which is where
  the dispatch hoist in [`future-work.md`](future-work.md) puts the dispatch
  anyway. Doing it alone would also be correct.

### 110. Instruction diagnostics anchor at the opcode where upstream anchors at the result name

*parser — instruction dispatch* — crates/llvmkit-asmparser/src/ll_parser.rs (`Parser::parse_basic_block_instructions`, `result_loc`)

Found 2026-08-20 while fixing the `catchswitch` dispatch (0.0.4 funclet
parity).

- **LLVM:** `LLParser::parseBasicBlock` takes `LocTy NameLoc = Lex.getLoc();`
  *before* stripping the result name, and hands it to
  `PerFunctionState::setInstName`. Every diagnostic that routine raises points
  at the `%name` token.
- **llvmkit:** `result_loc` is taken *after* `parse_lhs_assignment`, so it
  points at the opcode. Probe: a function with two `%x = add …` lines reports
  `4:8: multiple definition of local value named 'x'` — column 8 is `add`,
  where upstream's `NameLoc` is column 3, the `%x`. The same shift applies to
  `instructions returning void cannot have a name`, `check_value_id`'s message
  and `instruction forward referenced with type '…'`.
- **Why:** the split dispatch (see [`future-work.md`](future-work.md)) forces
  `result_loc` to be taken in the post-LHS path, and it was taken at the point
  of use rather than before the name.
- **Fix:** take `result_loc` before `parse_lhs_assignment`. This changes the
  anchor column of four diagnostics parser-wide, so it wants its own
  diagnostic-span audit and lands with that hoist.

### 113. Every `parseTypeAndBasicBlock` site is rendered as a `label` keyword expectation

*parser — terminators* — crates/llvmkit-asmparser/src/ll_parser.rs (`expect_primitive(PrimitiveTy::Label, …)` in `parse_br`, `parse_switch`, `parse_indirectbr`, `parse_cleanupret`, `parse_catchret`, `parse_catchswitch`, `parse_invoke`, `parse_callbr`)

Found 2026-08-20 reviewing the `catchswitch` handler-list rewrite, which made
two of these reachable for the first time. Not previously recorded, at either
the `catchswitch` site or the parser-wide one behind it.

- **LLVM:** every block operand in a terminator goes through `LLParser::parseTypeAndBasicBlock`, which is `parseTypeAndValue` followed by `if (!isa<BasicBlock>(V)) return error(Loc, "expected a basic block")`. So the token that is *not* a `label` decides the message: a token that cannot begin a type gives `parseType`'s `expected type`, and a well-formed type-and-value that is not a block gives `expected a basic block` anchored at the start of the type.
- **llvmkit:** has no `parse_type_and_basic_block`. **13** sites call `expect_primitive(PrimitiveTy::Label, …)` directly, each with its own production string, so each answers a message upstream never emits — at a site where upstream emits one of its two. Same verdict (reject), same anchor token; different text. Seven render `expected 'label' **for** …`: `then-target`, `else-target`, `switch default`, `switch case destination`, `invoke normal destination`, `invoke unwind destination`, `callbr fallthrough destination`. Six render `expected 'label' **in** …`: `indirectbr destination`, `cleanupret unwind destination`, `catchret destination`, `catchswitch handler`, `catchswitch unwind destination`, `callbr indirect target`.
- **The two counts are not the same number, and that matters for the port.** llvmkit has 13 expectation sites; upstream has **15** `parseTypeAndBasicBlock` call sites across the same eight routines. The difference is `parseIndirectBr` and `parseCallBr`, each of which unrolls the first iteration of its destination list (`if (Lex.getKind() != lltok::rsquare) { parseTypeAndBasicBlock(…); while (EatIfPresent(comma)) parseTypeAndBasicBlock(…); }`) where llvmkit uses one loop. A 1:1 port has to reproduce the unrolled shape or prove it is unobservable, not just swap the message at 13 places.
- **Newly reachable at `b369431`:** `parse_catchswitch`'s `do … while (EatIfPresent(comma))` rewrite made `catchswitch within none []` and `[label %a,]` reach the handler-list expectation, where before the type bug rejected them earlier. Both now answer `expected 'label' in catchswitch handler`; upstream reaches `parseTypeAndBasicBlock` at the `]` and answers `expected type`.
- **Why:** the sites were written one terminator at a time, each spelling its own expectation, rather than through a shared port of `parseTypeAndBasicBlock`.
- **Fix:** port `LLParser::parseTypeAndBasicBlock` once — parse a type, parse a value of it, reject a non-block with `expected a basic block` at the type's start — and route llvmkit's 13 sites through it, checking the two unrolled loops above while you are there. Nothing in the tree pins the current messages (no test, no manifest `error=` row), so the change is message-only; but it is a parser refactor with its own verification, not a docs edit, which is why it is recorded here rather than done in the round that found it.

<details><summary>Verification evidence (verified 2026-08-20)</summary>

Probed with `target/release/examples/parse_file.exe` at `b369431`. `%c = catchswitch within none [] unwind to caller` -> `5:33: expected 'label' in catchswitch handler`, caret on the `]`. `[label %a,]` -> `5:42: expected 'label' in catchswitch handler`, caret on the `]`. `[i32 0]` -> `5:33: expected 'label' in catchswitch handler`, caret on `i32` — upstream parses `i32 0` successfully and answers `expected a basic block` at that same offset. Upstream side read from `lib/AsmParser/LLParser.cpp`: `parseCatchSwitch`'s handler loop is `do { if (parseTypeAndBasicBlock(DestBB, PFS)) return true; … } while (EatIfPresent(lltok::comma));`, and `parseTypeAndBasicBlock` is `parseTypeAndValue` + the `isa<BasicBlock>` guard. `grep -n "PrimitiveTy::Label," crates/llvmkit-asmparser/src/ll_parser.rs` returns 13 sites, in `parse_br` (x2), `parse_switch` (x2), `parse_indirectbr`, `parse_cleanupret`, `parse_catchret`, `parse_catchswitch` (x2), `parse_invoke` (x2) and `parse_callbr` (x2); `grep -rn "expected 'label\|expected a basic block" crates/*/tests/ docs/` returns nothing that pins either spelling.

Re-verified 2026-08-20 after the entry was first written, and it was wrong twice. (1) `grep -n parseTypeAndBasicBlock lib/AsmParser/LLParser.cpp` returns the definition plus **15** call sites (7606, 7608, 7626, 7642, 7681, 7686, 7748, 7750, 7872, 7894, 7922, 7938, 8044, 8053, 8058), not 13: `parseIndirectBr` and `parseCallBr` each unroll the first iteration of their destination list. The entry originally read "at all 13 of its sites", attributing llvmkit's count to upstream's routine. (2) The message template originally read `expected 'label' in <production>`, which covers only 6 of the 13; the other 7 spell it `for`. Probed at HEAD with `parse_file`: `br i1 %c, i32 0, label %b` -> `3:13: expected 'label' for then-target`; `switch i32 %x, i32 0 [ ]` -> `3:18: expected 'label' for switch default`; `indirectbr ptr %p, [ i32 0 ]` -> `3:24: expected 'label' in indirectbr destination`; `invoke void @g() to i32 0 unwind label %u` -> `4:23: expected 'label' for invoke normal destination`; `catchswitch within none [] unwind to caller` -> `5:33: expected 'label' in catchswitch handler`; `[label %a,]` -> `5:42:` same; `catchswitch within none [label %a] unwind i32 0` -> `5:50: expected 'label' in catchswitch unwind destination`; `cleanupret from %cp unwind i32 0` -> `6:30: expected 'label' in cleanupret unwind destination`.

</details>

### 114. A `zeroinitializer` of a target extension type reports `expected invalid type for null constant`

*parser — constants* — crates/llvmkit-asmparser/src/ll_parser.rs (`zero_initializer_constant`, the `AnyTypeEnum::TargetExt` arm)

Found 2026-08-20 in review of the operand-bundle work, which passed through this
arm while testing a target extension type as a metadata operand. Pre-existing
and unrecorded. It has the shape `docs/fixture-coverage.md` calls gap **G17** —
a complete upstream message routed through an `expected ...` wrapper.

- **LLVM:** `LLParser::convertValIDToValue`'s `case ValID::t_Zero:` is *two*
  guards, both raising the same bare message anchored at `ID.Loc` — the
  location of the `zeroinitializer` token itself, set as `LLParser::parseValID`'s
  first statement:

  ```cpp
  case ValID::t_Zero:
    // FIXME: LabelTy should not be a first-class type.
    if (!Ty->isFirstClassType() || Ty->isLabelTy())
      return error(ID.Loc, "invalid type for null constant");
    if (auto *TETy = dyn_cast<TargetExtType>(Ty))
      if (!TETy->hasProperty(TargetExtType::HasZeroInit))
        return error(ID.Loc, "invalid type for null constant");
    V = Constant::getNullValue(Ty);
    return false;
  ```

  This entry is about the second. Which `target(...)` names carry `HasZeroInit`
  is decided by `Type::getTargetTypeInfo` (`lib/IR/Type.cpp`), whose table
  `crates/llvmkit-ir/src/derived_types.rs` mirrors faithfully — the model is at
  parity, only the diagnostic and the anchor are not.
- **llvmkit:** the `TargetExt` arm of `zero_initializer_constant` maps
  `IrError::InvalidOperation`'s message into `ParseError::Expected`, whose
  rendering prefixes `expected `. So llvmkit prints
  `expected invalid type for null constant` where upstream prints the bare text.
- **The anchor half of this entry is CLOSED (2026-08-21).** It used to read
  "and anchors it at `self.loc()` — the lookahead token, reached after the
  failing value was consumed", and pointed at the (now deleted) entry on
  `convertValIDToValue`'s anchoring for the port. That port landed:
  `LLParser::ValID`'s `Loc` member is now carried by llvmkit's `ValId` and every
  arm of `convert_val_id_to_value` / `convert_val_id_to_constant` reports at it,
  `zero_initializer_constant` included. Probed at the closing commit:
  `2004-11-28-InvalidTypeCrash.ll` reports `5:39`, the `zeroinitializer` token,
  and `target-type-properties/zeroinit-error.ll` reports `3:48` — both the
  columns this entry derived for upstream. **Only the `expected ` wrapper
  survives**, and it is what the heading names.
- **Consequence:** both text and column differ from `llvm-as` on every
  `target(...)` `zeroinitializer` whose type lacks `HasZeroInit`. The verdict is
  the same. The wrapper is **not** confined to this arm — `zero_initializer_constant`'s
  `_` catch-all raises the same `ParseError::Expected` at the same location,
  where the opaque-`Struct` arm already raises `ParseError::Message`. **Which
  types reach that catch-all is stated in entry 116 and deliberately not
  restated here**: it was, and the two copies drifted into agreeing on a list
  that was wrong in both directions. `token` no longer reaches it at all: the
  arm `Constant::getNullValue`'s `case Type::TokenTyID` calls for is ported, and
  `parser_constants.rs::token_zeroinitializer_is_the_token_none_constant` pins
  it.
- **Why:** `Module::target_ext_none` answers `IrError::InvalidOperation` with a
  message that is already upstream's complete sentence, and the arm reuses
  `ParseError::Expected` to carry it rather than `ParseError::Message`, which
  adds no prefix.
- **Why it stayed hidden, and the wider hole:** two corpus rows pin this exact
  text — `test/Assembler/2004-11-28-InvalidTypeCrash.ll` and the
  `zeroinit-error.ll` part of `test/Assembler/target-type-properties.ll` — and
  both pass, but only one of them passes *because of* the oracle.
  `2004-11-28-InvalidTypeCrash.ll` is `@.FOO = internal global %struct.none
  zeroinitializer`, an opaque struct, so it takes the arm that already renders
  the bare text and would satisfy an equality oracle unchanged; its anchor is
  wrong all the same. Only `zeroinit-error.ll` is green on containment:
  `parser_corpus.rs` compares an `error=` pin with `rendered.contains(pin)`, so
  any wrapper that only *adds* text around upstream's message satisfies it, and
  neither row sets `loc=`, so the anchor is unchecked too. That is a property of
  the harness, not of these two rows; it is recorded in `docs/future-work.md`.
  A third pin is a unit test,
  `parser_constants.rs::target_ext_zeroinitializer_requires_zero_init_property`,
  which asserts the divergent `ParseError::Expected` variant by name.
- **Fix, in three parts:**
  - Raise `ParseError::Message` from `zero_initializer_constant`'s `TargetExt`
    arm rather than wrapping `IrError::InvalidOperation`'s sentence in
    `ParseError::Expected`.
  - Update `parser_constants.rs::target_ext_zeroinitializer_requires_zero_init_property`
    to expect `ParseError::Message`. The corpus rows assert on rendered text,
    not on the variant, so they need no change.
  - The anchor half is a separate port of upstream's `ValID::Loc`, not an arm
    edit: llvmkit's `ValId` is a bare enum with no location field, and neither
    `convert_val_id_to_value` nor `convert_val_id_to_constant` takes one, so
    there is no "span of the value token" in scope. Threading it fixes
    `2004-11-28-InvalidTypeCrash.ll`'s anchor at the same time, since that
    fixture fails upstream's *first* guard, also `error(ID.Loc, …)`.
  - Then tighten `zeroinit-error.ll`'s manifest row with a `loc=`;
    `2004-11-28-InvalidTypeCrash.ll` needs no row change.

<details><summary>Verification evidence (verified 2026-08-20)</summary>

Probed with `target/release/examples/parse_file.exe` built at `4da2ee3`.
On the vendored
`crates/llvmkit-asmparser/tests/fixtures/upstream/assembler-corpus/target-type-properties/zeroinit-error.ll`
llvmkit reports `5:3: expected invalid type for null constant`, caret on `ret`.
That file's *failing* line is line 3,
`%val2 = freeze target("unknown_target_type") zeroinitializer`, whose name
reaches `Type::getTargetTypeInfo`'s terminal `TargetTypeInfo(Type::getVoidTy(C))`
and carries no properties. Line 2's `spirv.DeviceEvent` reaches the same
routine's generic `Name.starts_with("spirv.")` branch, which grants
`HasZeroInit`, and is the fixture's positive control — line 2 in isolation
round-trips, exit 0. llvmkit's `5:3` anchor is the `ret` token after line 3's
instruction, line 4 being the `; CHECK-ZEROINIT` comment, so the skew is two
lines, not three. Upstream anchors at 3:48 — **derived, not observed**:
`llvm-as` is not runnable here and the fixture's `FileCheck` line
(`error: invalid type for null constant`) pins message text only;
`LLParser::parseValID` sets `ID.Loc = Lex.getLoc()` as its first statement,
`SMDiagnostic::print` (`lib/Support/SourceMgr.cpp`) emits `ColumnNo + 1` over a
0-based offset, and `awk 'NR==3{print index($0,"zeroinitializer")}'` on the
fixture gives 48. Its sibling
`crates/llvmkit-asmparser/tests/fixtures/upstream/assembler-corpus/2004-11-28-InvalidTypeCrash.ll`
reports `6:1: invalid type for null constant` — the bare text, from a
different arm of the same routine, which is why this entry is scoped to the
`TargetExt` arm; its anchor is off as well. Upstream read from
`lib/AsmParser/LLParser.cpp`: `convertValIDToValue`'s `t_Zero` case is
`if (auto *TETy = dyn_cast<TargetExtType>(Ty)) if
(!TETy->hasProperty(TargetExtType::HasZeroInit)) return error(ID.Loc, "invalid
type for null constant");`. Harness read from
`crates/llvmkit-asmparser/tests/parser_corpus.rs`: the `error=` assertion is
`rendered.contains(pin)`; the manifest rows for both fixtures carry
`error=invalid type for null constant` and no `loc=`.

</details>

### 116. `zero_initializer_constant`'s fallback arm invents a message, and the constant path drops upstream's first guard

*parser — constants* — crates/llvmkit-asmparser/src/ll_parser.rs
(`zero_initializer_constant`, `convert_val_id_to_constant`)

Found 2026-08-20 while checking entry 114's claim that the defect was confined
to the `TargetExt` arm. It is not.

- **LLVM:** `LLParser::convertValIDToValue`'s `case ValID::t_Zero:` is *two*
  guards. It opens `if (!Ty->isFirstClassType() || Ty->isLabelTy()) return
  error(ID.Loc, "invalid type for null constant");` and only then tests
  `TargetExtType::HasZeroInit` with the same bare message.
  `LLParser::parseConstantValue` routes `t_Zero` through the same routine with
  `PFS = nullptr`, so the first guard runs on the constant path too.
- **llvmkit, defect 1:** `zero_initializer_constant`'s `_` catch-all is
  `Err(self.expected_at(loc, "zeroinitializer for a zeroable type"))` — the same
  `ParseError::Expected` shape at the same location, carrying a production
  string upstream never emits. **This is the one place the reachable set is
  written down** (entry 114 points here rather than restating it): on the
  *value* path `check_undef_like_type` runs first, so the catch-all sees
  `metadata`, `x86_amx` and `exnref`; on the *constant* path, which
  skips that guard, `label` reaches it too. `token` used to be in that set and
  is not any more — the `case Type::TokenTyID` arm of `Constant::getNullValue`
  is ported, so it never reaches the catch-all. `void` and a function type never do
  — both are refused earlier, `void` at the type position and a function type
  by `functions are not values, refer to them as pointers`. Probed at this
  commit with `target/release/examples/parse_file.exe` on
  `@g = global label zeroinitializer` (catch-all),
  `@g = global void () zeroinitializer` (`functions are not values…`) and
  `@g = global void zeroinitializer` (`void type only allowed for function
  results`).
- **llvmkit, defect 2:** `convert_val_id_to_constant`'s `ValId::Zero` arm does
  not call `check_undef_like_type`, while its own `Undef` and `Poison` siblings
  do, and the value path does for `Zero`. So a global initializer skips
  upstream's first guard: `@g = global label zeroinitializer` reports
  `1:19: expected zeroinitializer for a zeroable type` where upstream reports
  `invalid type for null constant`.
- **Not affected:** the opaque-`Struct` arm already raises `ParseError::Message`
  with the bare text, which is why `2004-11-28-InvalidTypeCrash.ll` renders
  exactly and is the one arm entry 114's original scope sentence was true about.
- **Hardening, not a gap:** `metadata` and `x86_amx` `zeroinitializer` pass
  upstream's first-class guard and fall to `Constant::getNullValue`'s
  `default: llvm_unreachable("Cannot create a null constant of that type!")`, so
  upstream traps rather than accepting. llvmkit rejecting them is safe and must
  not be "ported" back.
- **Fix:** raise `ParseError::Message` from the `_` arm with upstream's text
  where upstream reaches its first guard, and call
  `check_undef_like_type(ty, "null")` from `convert_val_id_to_constant`'s
  `ValId::Zero` arm as the value path already does. The anchor half is entry
  114's `ValID::Loc` port.

<details><summary>Verification evidence (verified 2026-08-20)</summary>

Upstream read from `lib/AsmParser/LLParser.cpp`: `convertValIDToValue`'s
`t_Zero` case, both guards, verbatim as quoted in entry 114; `parseConstantValue`
dispatches `ValID::t_Zero` into
`convertValIDToValue(Ty, ID, V, /*PFS=*/nullptr)` in the same `case` group as
`t_Constant`. `lib/IR/Type.cpp::Type::isFirstClassType` returns false only for
`FunctionTyID`, `VoidTyID` and an opaque `StructTyID`;
`lib/IR/Constants.cpp::Constant::getNullValue` has no `MetadataTyID` or
`X86_AMXTyID` case and ends in
`default: llvm_unreachable("Cannot create a null constant of that type!")`.
llvmkit read from `crates/llvmkit-asmparser/src/ll_parser.rs`: the `_` arm at
:8347, the opaque-`Struct` `ParseError::Message` at :8323-8328, the value path's
`check_undef_like_type(ty, "null")` at :8475-8477, and the constant path's bare
`ValId::Zero => self.zero_initializer_constant(ty)` at :8546. Probed with
`target/release/examples/parse_file.exe` at this commit:
`@g = global label zeroinitializer` -> `2:1: expected
zeroinitializer for a zeroable type`;
`crates/llvmkit-asmparser/tests/fixtures/upstream/assembler-corpus/2004-11-28-InvalidTypeCrash.ll`
-> `6:1: invalid type for null constant`. A third probe stood here,
`%v = freeze token zeroinitializer`, and was removed rather than re-blessed when
the `case Type::TokenTyID` arm landed: that input parses now.

</details>

### 118. A `#dbg_*` record's value operand skips `parseValueAsMetadata`'s guard and `parseMetadata`'s `TypeMsg`

*parser — debug records* — crates/llvmkit-asmparser/src/ll_parser.rs
(`parse_debug_metadata_operand`)

Found 2026-08-21 in the close-out of the `DIArgList` port, which added
`parseMetadata`'s `DIArgList` dispatch to `parse_metadata_value_operand` and
left this fourth site — a second, hand-rolled copy of the same routine —
carrying neither of the two things `parseMetadata`'s fall-through supplies.

- **LLVM:** `LLParser::parseDebugRecord` parses its value field with
  `parseMetadata(ValLocMD, &PFS)` — the whole routine, not a re-implementation.
  Its non-`!` fall-through is
  `return parseValueAsMetadata(MD, "expected metadata operand", PFS);`, and
  `LLParser::parseValueAsMetadata` is `parseType(Ty, TypeMsg, Loc)`, then
  `if (Ty->isMetadataTy()) return error(Loc, "invalid metadata-value-metadata
  roundtrip");`, then `parseValue`. So a record operand gets upstream's
  `expected metadata operand` when the type will not parse, and the roundtrip
  guard — anchored at the *type* — when it parses as `metadata`.
- **llvmkit:** `parse_debug_metadata_operand` is written out rather than
  delegating: a `DIArgList` arm, then an `Exclaim | MetadataVar` arm, then a
  bare `parse_type` + `parse_value` tail. That tail has no `TypeMsg` and no
  `isMetadataTy` guard, so it reports `parse_type`'s own `expected type`, and
  it runs on into `parse_value` and blames the *value* for a `metadata` inner
  type.
- **Consequence:** two diagnostics differ, in text and in anchor, on input both
  sides reject. `#dbg_value(metadata %a, !5, !DIExpression(), !4)` reports
  `3:27: '%a' defined with type 'i32' but expected 'metadata'`, caret on `%a`,
  where upstream reports `invalid metadata-value-metadata roundtrip` at the
  inner `metadata` keyword. `#dbg_value(42, ...)` reports `3:16: expected type`
  where upstream reports `expected metadata operand`.
- **The asymmetry is inside llvmkit and visible without upstream.** The same
  shape spelled as a call argument goes through `parse_value_as_metadata` and
  answers correctly: `call void @llvm.dbg.value(metadata metadata %a, ...)`
  reports `3:38: invalid metadata-value-metadata roundtrip`, caret on the inner
  `metadata`. Operand bundles, exception-argument lists and `parse_di_arg_list`
  were all routed through that single port in the operand-bundle work; the
  record operand is the one caller left holding its own copy.
- **Fix:** delete the hand-rolled tail and delegate to
  `parse_metadata_value_operand` — which, since the `DIArgList` port, already
  is `parseMetadata` including its `DIArgList` arm — keeping only the wrapping
  into `DebugMetadataOperand`. That is a `parseDebugRecord` change with its own
  diagnostic re-blessing, not a one-line guard insertion, which is why it is
  recorded here rather than done in the round that found it.

<details><summary>Verification evidence (verified 2026-08-21)</summary>

Upstream read from `lib/AsmParser/LLParser.cpp`: `parseDebugRecord`'s value
field is `Metadata *ValLocMD; if (parseMetadata(ValLocMD, &PFS)) return true;`,
and `parseValueAsMetadata` is the five-statement routine quoted above, with
`TypeMsg` reaching `parseType` and the `isMetadataTy` error taking `Loc` from
`parseType`'s out-parameter. llvmkit read from
`crates/llvmkit-asmparser/src/ll_parser.rs::parse_debug_metadata_operand`: the
routine is a `peek_is_di_arg_list` arm, an `Exclaim | MetadataVar` arm, and
then exactly three statements — `let ty = self.parse_type(false)?;`,
`let value = self.parse_value(state, ty)?;`,
`Ok(DebugMetadataOperand::Value(value.id()))`. Nothing between the two calls,
so no `is_metadata()` test, and `parse_type`'s only argument is the
allow-void flag, so no `TypeMsg` is threaded either. Probed with
`target/release/examples/parse_file.exe` rebuilt at this commit's parent, whose
parser source this commit does not touch: the record shape gives
`3:27: '%a' defined with type 'i32' but expected 'metadata'`; the same shape as
a `call` argument gives `3:38: invalid metadata-value-metadata roundtrip`; and
`#dbg_value(42, ...)` gives `3:16: expected type`.

</details>

### 121. Verifier diagnostics are house-worded, not `Verifier::CheckFailed`'s strings

*verifier* — crates/llvmkit-ir/src/verifier.rs; crates/llvmkit-ir/src/error.rs (`VerifierRule`, `IrError::VerifierFailure`)

- **LLVM:** `Verifier`'s `Check(cond, "…", V)` macro hands its literal to `CheckFailed`, which prints that string verbatim ahead of the offending value. The literal *is* the diagnostic: `llvm/test/Verifier/*.ll` `CHECK` lines match it, so it is contractual the same way a parser diagnostic is.
- **llvmkit:** a verifier failure is `IrError::VerifierFailure { rule, function, block, message }`. `rule` is a `VerifierRule` whose `Display` is a house label written in the enum's own register (lower-case, no trailing `!`, named after the invariant rather than the sentence), and `message` is a `format!` written at the check site, usually naming the offending type or operand index. Neither reproduces upstream's literal. Four pairs from `check_gep` alone, upstream first: `GEP base pointer is not a vector or a vector of pointers` / `getelementptr base operand has type {} (expected pointer)`; `GEP into unsized type!` / `getelementptr source element type {} is unsized`; `GEP indexes must be integers` / `getelementptr index #{slot} has type {} (expected integer)`; `Invalid indices for GEP pointer type!` / `getelementptr indices do not index into source type {}`. The newer GEP rules were written to the same convention, so the divergence is the convention, not any one rule.
- **Why:** The rule enum, not the string, is llvmkit's diagnostic API — a caller matches `VerifierRule::…` and the text is for humans — so the strings were written for that surface rather than copied. Nothing enforces the convention and nothing measures the drift *across* the verifier: there is no `test/Verifier` counterpart to the manifest `parser_corpus.rs` drives, so the wording is compared against `Verifier.cpp` only where a hand-written test happens to do it.
- **The `!range` rules are the counterexample, and they answer the register question.** `crates/llvmkit-asmparser/tests/parser_metadata.rs::upstream_invalid_range_metadata_fixture_messages_match` `include_str!`s the vendored `tests/fixtures/upstream/Verifier/range-1.ll`, cuts out each of its functions with its `!range` node, runs `Module::verify_borrowed` over each, and `assert_eq!`s the result against upstream's own `Check` literal for that case (`Ranges are only for loads, calls and invokes!`, `It should have at least one range!`, `Intervals are overlapping`, …), read off `IrError::VerifierFailure`'s **`message`** field. So the divergence is not universal, a `test/Verifier` fixture *can* be ported by message text where the rule already carries upstream's string, and where the literal lives is settled: in `message`, not in `rule`'s `Display`.
- **Consequence:** the accept/reject verdict is unaffected — this is text only. But porting a `test/Verifier/*.ll` fixture by its `CHECK` lines works only for a rule already written to upstream's literal; elsewhere the `CHECK` line will not match and the port has to assert the `VerifierRule` instead and say so.
- **Fix:** One sweep, not a per-rule patch: give every `VerifierRule` its upstream `Check` literal in `message` (the enum doc comments already name most of them), keep the `format!` detail as a suffix rather than a replacement, and then drive the `test/Verifier` fixtures by text the way `parser_corpus.rs` drives `test/Assembler` — which is also the gate whose absence let this entry state a negative that one in-tree test already falsified.
- **Not covered here:** the parser's diagnostics, which *are* upstream's literals and are pinned as such; and the individual entries in this section that record a *parser* message differing from upstream's. `VerifierRule::PhiEmptyInReachableBlock` is also out of scope, but for a reason this entry is the wrong place to state — entry 8 works out when it pre-empts an upstream `Check` and when there is none to pre-empt. Read it there rather than trusting a summary here.

<details><summary>Verification evidence (2026-08-21)</summary>

Upstream read at the vendored tag `llvmorg-22.1.4`; the repo commit does not pin `orig_cpp/`, which is gitignored. `llvm/lib/IR/Verifier.cpp` — the `Check` macro expands to `CheckFailed(__VA_ARGS__); return;`, and `Verifier::visitGetElementPtrInst` carries the four literals quoted above. llvmkit at this commit: `crates/llvmkit-ir/src/error.rs` — `IrError::VerifierFailure`'s `message` field is documented "Human-readable description mirroring `Verifier::CheckFailed`", and `VerifierRule`'s `Display` arm for each GEP rule renders the house label (`"getelementptr base is not a pointer"`, `"getelementptr source element type is unsized"`, `"getelementptr index operand is not an integer"`, `"getelementptr indices are invalid for the source type"`); `crates/llvmkit-ir/src/verifier.rs::check_gep` carries the four `format!` strings quoted above. Scope check before opening this entry: `grep -niE "verifier.*(wording|reworded|message text|diagnostic text|Check string)" docs/divergences.md docs/future-work.md` found no class-level entry, and the two entries that mention verifier wording (the `PhiNotAtTop` text and the `callbr` "carrying upstream's wording" fix sketch) are per-rule remarks inside entries about a different divergence. The claim here is deliberately not quantified over every rule: four pairs were read and quoted, and the sentence says the convention diverges, not that every rule does.

**Correction, 2026-08-22.** That scope check searched `docs/` and never `crates/`, and the entry then asserted an absence over the tree it had not looked at: it said no `test/Verifier` fixture is driven through llvmkit's verifier by message text. `grep -rln 'fixtures/upstream/Verifier' crates/` finds the `range-1.ll` test named in the bullet above, which does exactly that and passes. Both the Why and the Consequence are rewritten; the `!range` rules are the in-tree counterexample.

</details>

## Different printed bytes

The parser/printer contract is that printed output matches `AsmWriter.cpp` byte for byte and re-parses.

### 24. An attribute list prints in insertion order, not `AttributeImpl::cmp`'s order — **NARROWED**

*IR model / Attributes* — crates/llvmkit-ir/src/attributes.rs (`AttributeStorage::add_stored`), crates/llvmkit-ir/src/asm_writer.rs (`fmt_attribute_set`)

- **LLVM:** `addAttributeImpl` inserts at `lower_bound(Attrs, Kind, AttributeComparator())`, so an `AttrBuilder`'s vector is always sorted, and `AttributeSetNode::get` sorts again with `llvm::sort` before uniquing. The order is `AttributeImpl::cmp`'s: enum-kinded attributes first, by `AttrKind` enum value, then string attributes by key. `AssemblyWriter` prints that order, so `declare void @f() "k"="1" "j"="2"` comes back as `"j"="2" "k"="1"`.
- **llvmkit:** `AttributeStorage` keeps one `Vec<AttributeStored>` per `AttrIndex` and `add_stored` pushes to the end; `fmt_attribute_set` prints it as stored. Source order therefore survives into the output, and a list written out of upstream's sort order prints out of it too.
- **Why:** the entry's other half — `add` de-duplicating by full structural equality, so `align 4 align 8` kept both — was the accepts-invalid behaviour and is closed: `add_stored` is now the port of `addAttributeImpl`'s `std::swap` branch, keyed by `AttrKind` for enum attributes and by key for string ones, and the redundant `AttributeStorage::set` is gone. The ordering half is left because it is a *different* change: it needs `AttributeComparator` and `AttributeImpl::cmp` ported as their own routines, and re-blessing every printed attribute list in the corpus that happens to be written out of order.
- **Fix:** port `AttributeComparator` and insert at its `lower_bound` in `add_stored`, then re-run the byte-lock and corpus gates.
- **Evidence (2026-08-21):** `crates/llvmkit-asmparser/tests/parser_modifiers.rs::an_attribute_list_holds_one_attribute_per_kind` asserts `declare void @f() "k"="1" "j"="2"` prints its two string attributes in *source* order, which is the divergence: `AttributeImpl::cmp`'s string arm is `getKindAsString().compare(AI.getKindAsString())`, so upstream would print `"j"` first. The closed half is asserted by the same test.

### 126. The comdat block prints every comdat in the module, in declaration order

*printer — module header* — crates/llvmkit-ir/src/asm_writer.rs (`fmt_module_with_options`, comdat loop)

Found 2026-08-21 while porting the blank lines of the very same loop
(`printModule`'s `if (!Comdats.empty()) Out << '
';` and its
`if (C != Comdats.back()) Out << '
';` separator, both of which were fixed in
that commit). Which comdats the loop walks was not.

- **LLVM:** the loop iterates `AssemblyWriter::Comdats`, a
  `SetVector<const Comdat *>` the `AssemblyWriter` constructor fills by walking
  `TheModule->global_objects()` and inserting `GO->getComdat()` for each object
  that has one. `Module::global_objects` is `concat<GlobalObject>(functions(),
  globals())`. So the printed set is *first-use* order over functions and then
  globals, and a comdat that no global object references is never printed at
  all.
- **llvmkit:** the loop iterates `ModuleCore::iter_comdats`, which is the
  module's own comdat table — declaration order, unreferenced entries
  included.
- **Consequence:** printed bytes differ from `llvm-dis` on any module whose
  comdat declaration order differs from its first-use order, and on any module
  carrying an unreferenced comdat. llvmkit's output still re-parses; the
  difference is that `llvm-as | llvm-dis` *loses* an unreferenced comdat and
  llvmkit keeps it, so a round-trip through the two tools disagrees.
- **Why it is recorded rather than fixed:** closing it changes which comdats a
  printed module contains and in what order, so every checked-in
  `.expected.ll` and byte-lock carrying more than one comdat moves with it.
  That is a re-blessing pass of its own, and it is a different question from
  the whitespace parity the commit that found it was fixing. Nothing about it
  is blocked — `GlobalVariable::comdat` and `FunctionValue::comdat` both
  exist.
- **Fix:** build the printed sequence the `AssemblyWriter` constructor's way:
  one pass over `iter_functions` then `iter_globals`, pushing each
  `comdat()` into an insertion-ordered set, and drive the loop from that
  instead of from `iter_comdats`.

<details><summary>Verification evidence (2026-08-21)</summary>

Upstream read at the vendored tag `llvmorg-22.1.4`. `llvm/lib/IR/AsmWriter.cpp` — the `AssemblyWriter` constructor's body is `if (!TheModule) return; for (const GlobalObject &GO : TheModule->global_objects()) if (const Comdat *C = GO.getComdat()) Comdats.insert(C);`, and `Comdats` is declared `SetVector<const Comdat *>` on the class. `printModule`'s comdat block is `if (!Comdats.empty()) Out << '
'; for (const Comdat *C : Comdats) { printComdat(C); if (C != Comdats.back()) Out << '
'; }`. `llvm/lib/IR/Module.cpp` — `iterator_range<Module::global_object_iterator> Module::global_objects() { return concat<GlobalObject>(functions(), globals()); }`.

llvmkit probed at the commit that fixed the blank lines, with `target/release/examples/parse_file.exe` rebuilt at that commit, on this input:

```llvm
$a = comdat any
$b = comdat largest
$orphan = comdat exactmatch
@g = global i32 0, comdat($a)
define void @f() comdat($b) {
  ret void
}
```

llvmkit prints `$a`, `$b`, `$orphan`, in that order. Upstream's set is `{$b, $a}` — `@f` is a function and functions come first in `global_objects()`, and `$orphan` is referenced by no global object — so `llvm-dis` would print `$b` then `$a` and drop `$orphan`. The blank-line placement around the block matches upstream at this commit; only the membership and order do not. `llvm-as`/`llvm-dis` are not runnable in this environment, so upstream's side of this probe is derived from the two routines above rather than executed.

</details>

### 40. Function attributes are never hoisted into an attribute group

*printer* — crates/llvmkit-ir/src/asm_writer.rs:3311-3320 (input-carried groups only), :2115 / :2525 / :3070 (inline header printing)

- **LLVM:** `SlotTracker::CreateAttributeSetSlot` mints one slot per distinct function `AttributeSet` and `AssemblyWriter::writeAllAttributeGroups` emits them at the end of the module, so a function header prints as `define void @f23() #13` with `attributes #13 = { alignstack=4 }` below.
- **llvmkit:** `asm_writer.rs` prints function attributes inline on the header (`define void @f() alignstack(4)`) and emits an `attributes #N = { … }` block only for groups the *input* already carried, read straight out of `Module`'s attribute-group table. Output is bulkier than upstream's and diverges byte-for-byte from `llvm-dis` for any module with function attributes — which the parser/printer contract says should not happen.
- **Why:** Recorded, with the sequencing reason: land it with the `validateEndOfModule` group-merge work (W13) rather than alone, because the merge decides which attributes survive into the printed group, so doing the writer first would pin output the merge then changes. A named consequence: `test/Bitcode/attributes.ll` — the fixture pinning the `InAttrGrp` spelling of all four attribute kinds that have one — cannot be ported as a round-trip; W5 ported the parse half only.
- **Fix:** Port `SlotTracker::CreateAttributeSetSlot` (one slot per distinct function `AttributeSet`, assigned during the module pre-pass) and `AssemblyWriter::writeAllAttributeGroups`, switch the function-header printer to emit `#N`, and merge the input's own groups into the same slot space. Sequence it after the `validateEndOfModule` group merge, then port `test/Bitcode/attributes.ll` as the round-trip it is.
- **Correction from verification:** Accurate as written, with two refinements. (1) The cited line pointers are slightly off: :2115 / :2525 / :3070 are the input-carried group-reference loops (`for group in ... { write!(f, " #{group}") }`), while the *inline* function-attribute printing is the adjacent `fmt_attribute_set(..., AttrIndex::Function, true, ...)` calls at :2108 (call), :2518 (invoke/callbr), and :3073 (function header). (2) The divergence is broader than the claim states: upstream `SlotTracker::processFunction` also mints attribute-group slots for every `CallBase`'s function attributes, so llvmkit's inline printing diverges at call sites as well, not only on function headers. Additionally, llvmkit never emits the `; Function Attrs: ...` comment line that `AssemblyWriter::printFunction` writes above any function carrying fn attrs — a related printer gap the claim does not mention.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/asm_writer.rs:3070-3073 — `fmt_function` writes ` #{group}` only for `func.function_attr_groups()` (input-carried) and then prints the function AttributeSet inline via `fmt_attribute_set(f, &attrs, AttrIndex::Function, true, func.module())`. asm_writer.rs:3311-3341 — the `attributes #N = { ... }` emitter iterates `m.attribute_groups()` and nothing else. crates/llvmkit-ir/src/module.rs:2364 — `attribute_groups()` clones a `RefCell<Vec<(u32, AttributeStorage)>>` populated only by `set_attribute_group`, whose sole non-test caller is crates/llvmkit-asmparser/src/ll_parser.rs:9168 (the parser). A repo-wide grep for `CreateAttributeSetSlot|attribute_set_slot|AttributeSetSlot` over crates/ returns no matches — there is no print-time slot minting. Same inline shape at call sites: asm_writer.rs:2108-2117 and :2518-2527. Upstream, orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/AsmWriter.cpp: `SlotTracker::processModule` calls `CreateAttributeSetSlot(F.getAttributes().getFnAttrs())` per function (~:1128) and `processFunction` does the same per `CallBase` (~:1171); `AssemblyWriter::printFunction` prints `if (Attrs.hasFnAttrs()) Out << " #" << Machine.getAttributeGroupSlot(Attrs.getFnAttrs());` (~:4197); `printModule` calls `writeAllAttributeGroups()` (~:3169), defined at :5010, which emits `attributes #N = { ... }` from `asMap`. llvmkit's own tests confirm the gap: crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:148-164 (`inline_alignstack_parses_and_round_trips`) ports only the parse half of test/Bitcode/attributes.ll's @f23 because "llvmkit's printer emits function attributes inline on the header and never forms an attribute group"; docs/future-work.md:200-232 records the identical entry.

</details>

### 41. `align` inside an attribute group is not moved onto the function

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:1470-1485 (the end-of-module sweep; no attribute-group merge among the `validate_*` calls); crates/llvmkit-ir/src/asm_writer.rs:3311

- **LLVM:** `LLParser::validateEndOfModule` pulls `Alignment` out of a function's merged attribute set (`FnAttrs.getAlignment()` → `Fn->setAlignment(*A)` → `FnAttrs.removeAttribute(Attribute::Alignment)`), so `attributes #0 = { align = 8 }` re-prints as `define void @f() align 8` with the attribute gone from the group.
- **llvmkit:** No group-merge step runs at end of module, so the `align` entry stays inside the printed group and never reaches the function's alignment field. The written text round-trips instead of being normalised the way `llvm-as | llvm-dis` normalises it.
- **Why:** Recorded as part of the attribute-group entry, noted as having "no visible effect yet" only because the writer half is also missing; it is scheduled with the W13 `validateEndOfModule` group-merge work.
- **Fix:** Add the group-merge step to llvmkit's end-of-module sweep: for each function, merge its referenced groups into one attribute set, move `Alignment` to the function's alignment field and drop it from the set, then let the (new) group writer print what survives. Do this before the writer-side hoisting so the printed group is pinned once.

<details><summary>Verification evidence</summary>

See above.

</details>

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

### 57. No attribute-group printer, so half of two upstream fixtures is unreachable

*printer (AsmWriter)* — crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:148-164; crates/llvmkit-asmparser/tests/parser_module_level.rs:26-29

- **LLVM:** `test/Bitcode/attributes.ll`'s `@f23` writes `define void @f23() alignstack(4)` inline and CHECKs `attributes #13 = { alignstack=4 }`, pinning both halves of the group spelling at once. `test/Assembler/global-variable-attributes.ll`'s `@g1`–`@g4` likewise need the trailing global attribute list plus the group printer.
- **llvmkit:** llvmkit's printer emits function attributes inline on the header and never forms an attribute group, so the CHECK lines cannot be produced from that input. Only the parse half of `@f23` is asserted; `@g1`–`@g4` are not ported. The group spelling itself is covered only from group-carrying *input*.
- **Why:** Recorded — "The printer gap is recorded in `docs/future-work.md`", and the global half is tagged W7 work.
- **Fix:** Port `AssemblyWriter::printModule`'s attribute-set collection: number distinct function attribute sets, print `attributes #N = { … }` blocks at module end, and emit `#N` on the headers. Then port `@f23` whole and add `@g1`–`@g4` with the trailing global attribute list.
- **Correction from verification:** Substantively accurate; two refinements. (1) The fixture is `test/Assembler/globalvariable-attributes.ll`, not `global-variable-attributes.ll` — the hyphenated name in the claim does not exist upstream (llvmkit itself miscites it at ll_parser.rs:15502 and parser_module_level.rs:660, while parser_module_level.rs:18 spells it correctly). (2) For `@g1`-`@g4` the gap is not printer-only: llvmkit's global parser has no equivalent of upstream's `parseFnAttributeValuePairs(Attrs, FwdRefAttrGrps, false, BuiltinLoc)` call that `LLParser::parseGlobal` makes after its property loop, so the trailing `"key" = "value"` list and `#0` reference are not accepted as input either — the property loop falls through to `unknown global variable property!`. Also worth stating precisely: llvmkit does have an `attributes #N = { … }` emitter, but it is a pass-through of groups the *input* carried, never a synthesis from attribute sets, so the divergence is "never forms a group", not "no group printer exists at all".

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/asm_writer.rs:3070-3073 — fmt_function emits ` #{group}` only for groups the function already referenced (func.function_attr_groups()), then prints remaining fn attrs inline via fmt_attribute_set(f, &attrs, AttrIndex::Function, true, func.module()). asm_writer.rs:3310-3344 — the `attributes #{slot} = {` block is driven by m.attribute_groups(); grep shows the only producer of that table is ll_parser.rs:9168 (`self.module.set_attribute_group(id, storage)`) parsing a literal `attributes #N = { … }` block, plus one test at constant_folding_analysis.rs:769 — nothing mirrors SlotTracker::CreateAttributeSetSlot. asm_writer.rs:3596-3723 — fmt_global prints section/partition/code_model/sanitizer/comdat/align/metadata and stops; no attribute list, no ` #N`. ll_parser.rs:6931-6978 — the global property loop's else arm returns `unknown global variable property!` with no attribute parsing after it. Upstream lib/IR/AsmWriter.cpp: processModule calls CreateAttributeSetSlot for each global's getAttributes() (~line 1097) and each function's getAttributes().getFnAttrs() (~1128-1130); printFunction emits `Out << " #" << Machine.getAttributeGroupSlot(Attrs.getFnAttrs())` (~4202) with no inline path; printGlobal emits the same for globals (~3986-3987); writeAllAttributeGroups (~5010) prints `attributes #N = { … }` using getAsString(true). lib/AsmParser/LLParser.cpp:1499-1502 — parseGlobal builds AttrBuilder Attrs + FwdRefAttrGrps via parseFnAttributeValuePairs after its property loop. Fixtures: test/Bitcode/attributes.ll:137-138 (`define void @f23() alignstack(4)` / `; CHECK: define void @f23() #13`) and :598 (`; CHECK: attributes #13 = { alignstack=4 }`); test/Assembler/globalvariable-attributes.ll CHECKs `@g1 = global i32 7 #0` through `attributes #3 = { … }`. Cited llvmkit tests unchanged: parser_attribute_matrix.rs:148-164 (inline_alignstack_parses_and_round_trips, asserting only the parse half and saying so in its doc comment) and parser_module_level.rs:27-29 (`@g1`-`@g4` are not ported). docs/future-work.md:200-228 records the same gap under "Printer — function attributes are never hoisted into an attribute group".

</details>

### 58. Function/global attributes are never hoisted into an attribute group by the printer

*printer (AsmWriter)* — crates/llvmkit-ir/src/asm_writer.rs, crates/llvmkit-ir/src/global_variable.rs, crates/llvmkit-asmparser/src/ll_parser.rs

- **LLVM:** `SlotTracker::CreateAttributeSetSlot` assigns one `#N` slot per distinct `AttributeSet` and `AssemblyWriter::writeAllAttributeGroups` emits the groups at the end of the module, so a function header prints `define void @f23() #13` with `attributes #13 = { alignstack=4 }` below. `validateEndOfModule` additionally moves a group's `Alignment` to `Fn->setAlignment()`, so `attributes #0 = { align = 8 }` re-prints as `define void @f() align 8`.
- **llvmkit:** `asm_writer.rs` prints function attributes inline on the header and emits an `attributes #N = { … }` block only for groups the *input* already carried. A global's trailing attribute list is not printed at all, and the `align`-out-of-group move never happens.
- **Why:** Recorded in docs/future-work.md (W5/W7): the group-*forming* machinery has never existed, and the item is routed to W13 deliberately — `validateEndOfModule`'s merge decides which attributes survive into the printed group, so building the writer first would pin output the merge then changes.
- **Fix:** Port `CreateAttributeSetSlot` + `writeAllAttributeGroups` together with W13's `validateEndOfModule` attr-group merge and the alignment-attr→field move. Unblocks `globalvariable-attributes.ll`'s `@g1`–`@g4` and makes `test/Bitcode/attributes.ll` portable as a round-trip (its CHECK lines are group lines).
- **Correction from verification:** Accurate, with one refinement and one sharpening. REFINEMENT (globals): the claim says a global's trailing attribute list "is not printed at all". It is worse than that — it is not *parsed* at all. `Parser::parse_global` in C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs ends its `, property` loop at the "unknown global variable property!" arm (~line 6977) and goes straight to `global_builder(...)`; it never runs upstream's trailing `parseFnAttributeValuePairs` (LLParser.cpp `parseGlobal`, lines 1499-1507). Feeding llvmkit the upstream fixture orig_cpp/.../llvm/test/Assembler/globalvariable-attributes.ll gives a hard error at line 3 col 20: `expected top-level entity` on `"key"`. `@gv = global i32 0, align 4 #0` fails the same way. Correspondingly `GlobalVariable`/`GlobalVariableData` in C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/global_variable.rs has no attribute field (a case-insensitive grep for "attr" in that file returns nothing), and `fmt_global` (asm_writer.rs:3596-3723) ends at metadata attachments with no ` #N` counterpart to `printGlobal`'s AsmWriter.cpp:3985-3987. SHARPENING (functions): llvmkit not only fails to hoist — it also never *merges* a referenced group into the function's own attribute set (upstream `validateEndOfModule`, LLParser.cpp:212-238, does). The `#N` is kept verbatim as a raw `u32` in `FunctionData::function_attr_groups` (function.rs:106) and re-emitted as-is (asm_writer.rs:3070-3072), while inline attributes print separately on the same header (asm_writer.rs:3073). So a header written `declare i32 @llvm.bswap.i32(i32 %x) nounwind #0` re-prints with both halves, where upstream would collapse them into one printer-derived group. Group numbering is input-preserved rather than derived: the module's group table is populated only by the parser via `ModuleCore::set_attribute_group` (module.rs:2354, called from ll_parser.rs:9168), and there is no analogue of `SlotTracker::CreateAttributeSetSlot` / `AssemblyWriter::writeAllAttributeGroups` anywhere in the tree. Everything else in the claim holds verbatim, including the `align` consequence.

<details><summary>Verification evidence</summary>

Empirical round-trip through the shipped parser/printer (cargo +1.96.0 run --release -p llvmkit-asmparser --example parse_file): Input C:/Users/olegg/AppData/Local/Temp/.../scratchpad/t1.ll — define void @f23() alignstack(4) { ret void } define void @g() #0 { ret void } attributes #0 = { align = 8 } Output — define void @f23() alignstack(4) { <- upstream: `define void @f23() #N` + `attributes #N = { alignstack=4 }` define void @g() #0 { <- upstream: `define void @g() align 8`, no group at all attributes #0 = { align=8 } <- upstream: gone (Alignment moved to Fn->setAlignment) Input orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/globalvariable-attributes.ll — error: line 3 col 20 `expected top-level entity` on `@g1 = global i32 7 "key" = "value" ...` That fixture's CHECK lines are exactly the missing behavior: `@g1 = global i32 7 #0` … `attributes #0 = { "key"="value" "key2"="value2" }`, with `@g3 = global i32 2 #0` re-printing as `#2` — proof the numbering is printer-derived, not input-preserved. Upstream C++ read (C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/): - IR/AsmWriter.cpp:1128-1130 (`CreateAttributeSetSlot(F.getAttributes().getFnAttrs())`), 1096-1098 (same for globals), 1434-1439 (slot assignment), 4201-4202 (`printFunction` emits only ` #N`, never inline fn attrs), 3985-3987 (`printGlobal` emits ` #N`), 5010-5020 (`writeAllAttributeGroups`), 3166-3169 (called from `printModule`). - AsmParser/LLParser.cpp:212-238 (`validateEndOfModule` merges groups into the Function and moves `Alignment` to `Fn->setAlignment`, removing it from the set), 6834-6836 (same move for inline header attrs), 1499-1507 (`parseGlobal` accepts the trailing attribute list and stores it on the GV). llvmkit source read: - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/asm_writer.rs:3070-3073 (prints input-carried `#N` refs, then fn attributes inline), 3310-3344 (`attributes #N = {…}` block driven solely by `m.attribute_groups()`), 3596-3723 (`fmt_global`, no attribute output). - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/module.rs:1573, 2354-2366, 3110-3117 (group table written only by `set_attribute_group`). - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/function.rs:106, 666-682, 1593-1598 (groups held as raw ids, never merged). - C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:9160-9169 (only writer of the table), 6974-7059 (`parse_global` with no trailing attr parse). The tree's own tests pin the divergence and pass today on the pin (`cargo +1.96.0 test --release -p llvmkit-asmparser --test parser_attribute_matrix -- inline_alignstack attribute_group_equals_grammar_round_trips` → 2 passed): crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:148-164 states in its doc comment that "llvmkit's printer emits function attributes inline on the header and never forms an attribute group", and :720-724 asserts `attributes #0 = { align = 8 }` re-prints as `align=8` inside the group. The gap is also recorded at C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:200-234, deferred to the W13 `validateEndOfModule` group-merge work — and that ledger entry checks out against the tree rather than being stale.

</details>

### 99. Metadata is numbered by arena position, not reachability, so a node AutoUpgrade replaces still prints

*printer* — crates/llvmkit-ir/src/asm_writer.rs (the numbered-metadata loop and `metadata_slot_map`); exposed by crates/llvmkit-ir/src/auto_upgrade.rs

- **LLVM:** `SlotTracker::processModule` mints metadata slots by *walking* — named metadata operands, global and function attachments, instruction attachments, function-local metadata — so a node nothing references is never numbered and `writeAllMDNodes` never prints it. `MDNode::get` also uniques, so rebuilding a tuple with identical contents yields the same node.
- **llvmkit:** the printer walks `metadata_store().nodes()` and numbers *every* non-`MDString` node in arena order. Before W13d that was invisible: the parser only interned nodes the text named, so every node was reachable. `UpgradeModuleFlags` breaks that — it replaces a flag tuple with a freshly interned one, and the superseded tuple stays in the arena and still prints. `test/Bitcode/upgrade-module-flag.ll` therefore prints its five upgraded flags plus three orphaned pre-upgrade tuples, where `llvm-dis` prints six nodes numbered `!0`–`!5`. The output still re-parses (a dead `!N = ...` definition is legal), so only the byte-for-byte half of the contract is broken.
- **Why:** the fix is a real `SlotTracker` port, not a patch: upstream's numbering is *encounter* order over a specific traversal, so switching to reachability also changes which number every surviving node gets. That is a workstream of its own, and doing it inside an AutoUpgrade stage would have re-pinned every metadata-bearing expected output at the same time.
- **Fix:** port `SlotTracker::processModule` / `processFunction`'s metadata pre-pass and drive `metadata_slot_map` from it, then re-bless the metadata numbering in the corpus in the same commit. `crates/llvmkit-asmparser/tests/parser_auto_upgrade.rs` asserts on flag *contents* rather than `!N` numbering precisely because of this, and says so.

### 104. Where LLVM prints `<badref>` for an unnumbered value, llvmkit prints `%<unnumbered>` / `@<unnumbered>`

*printer* — crates/llvmkit-ir/src/asm_writer.rs — `fmt_operand_ref` (two arms), `fmt_global_value_ref`, `fmt_instruction`

- **LLVM:** `writeAsOperandInternal`'s value path ends `if (Slot != -1) Out << Prefix << Slot; else Out << "<badref>";` — where `Prefix` is `'@'` for a `GlobalValue` and `'%'` otherwise, and the failure spelling carries **no sigil at all** in either case. `AssemblyWriter::printInstruction`'s unnamed-result arm is the same shape: `if (SlotNum == -1) Out << "<badref> = "; else Out << '%' << SlotNum << " = ";`.
- **llvmkit:** four sites spell the failure with a sigil and a different word.
  - `fmt_operand_ref`'s `BasicBlock` arm and its `Argument`/`Instruction` arm — `%<unnumbered>`, against `writeAsOperandInternal`.
  - `fmt_global_value_ref` — `@<unnumbered>`, against the same routine's `Prefix = '@'` branch.
  - `fmt_instruction`'s unnamed-result arm — `%<unnumbered> = `, against `printInstruction`'s `<badref> = `.
- **Why:** Found while porting `AssemblyWriter::printBasicBlock`, whose own `<badref>:` twin was fixed in that commit because the routine was being rewritten; these were left rather than swept into a commit about block printing.
- **Corrected 2026-08-21 — this entry's own reachability claim was false, and it was hiding a round-trip break.** The claim was that "all four are unreachable while the `SlotTracker` covers what is being printed, which is every case the module printer produces", and that "a repo-wide grep for `<unnumbered>` finds no test, fixture or expected-output file" made the blast radius empty. The tracker did **not** cover every case the module printer produces: `fmt_metadata_operand` and `fmt_metadata_node` took no tracker parameter at all, so every unnamed local reached through a `ValueAsMetadata` printed `%<unnumbered>` — and that text is not valid IR. `declare void @f()` plus `%1 = add i32 %x, 1` / `call void @f() [ "foo"(metadata i32 %1) ]` printed `[ "foo"(metadata i32 %<unnumbered>) ]`, which re-parsed as `expected value token`. The grep came back empty because no fixture exercised the path — absence of a probe read as absence of a trigger. That hole is **fixed**: the tracker is threaded through `fmt_metadata_operand` / `fmt_metadata_node` / `fmt_specialized_metadata_node` / `fmt_metadata_attachments` the way upstream's `AsmWriterContext` carries `Machine`, present wherever `printFunction` has run `Machine.incorporateFunction(F)` and `None` at module scope where upstream's tracker has no local numbering either.
- **What survives is the spelling only, and this entry no longer asserts the sites are unreachable.** The four sites still emit `%<unnumbered>` / `@<unnumbered>` where upstream emits `<badref>`; what is not claimed any more is that nothing reaches them. Note also that `Value`'s own `Display` (`crates/llvmkit-ir/src/value.rs`) passes `None`, where upstream's `Value::print` builds a `SlotTracker` for the parent function — a separate gap in the same family, not covered by fixing the four strings.
- **Fix:** one string per site, **all four** — the first framing of this entry said "one string per arm" of `fmt_operand_ref` and would have left the global and instruction sites behind.
- **Upstream's fourth `<badref>` site is a different divergence, not this one.** `AsmWriter.cpp` writes `<badref>` in exactly four places: `writeAsOperandInternal`, `printInstruction`, `printBasicBlock` (already fixed) and `printNamedMDNode`, whose metadata arm is `int Slot = Machine.getMetadataSlot(Op); if (Slot == -1) Out << "<badref>"; else Out << '!' << Slot;`. llvmkit's counterpart does not print an `<unnumbered>` spelling at all — it falls back to the node's raw arena index, `write!(f, "!{}", id.index())` — so closing this entry does not touch it and a grep for `<unnumbered>` will never surface it. Noted here so the enumeration of upstream's `<badref>` sites is complete.
- **Out of scope, deliberately** (three further `<unnumbered>` sites with no upstream `<badref>` twin, listed so the next reader does not re-derive it): the function-signature argument printer in `fmt_function_with_use_lists`, whose upstream counterpart `AssemblyWriter::printArgument` has no failure spelling at all — it is `int Slot = Machine.getLocalSlot(Arg); assert(Slot != -1 && "expect argument in function here"); Out << " %" << Slot;`; the anonymous identified-struct number in the type-identity block, where `printTypeIdentities` writes `Out << '%' << I << " = type "` from a `NumberedTypes` **index** and so cannot fail; and the same struct case in `type.rs`'s `Display`.

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
- **Fix:** Either (a) give the erased phi surface a linear, consuming variant — `remove_incoming_or_erase(self, …) -> Either<Value, ErasedPhi>` taking the phi by value so the handle cannot outlive the erase; or (b) return a `PhiEmptied` marker the caller must destructure, making the leftover unignorable. (b) is cheaper and preserves the `Copy` handle for the common case. The verifier already flags the leftover via `PhiEmptyInReachableBlock`, so the gap is authoring ergonomics, not soundness.
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

### 74. opaque-ptr-invalid-forward-ref.ll vendored but wired to nothing

*parser corpus / forward references* — crates/llvmkit-asmparser/tests/fixtures/upstream/opaque-ptr-invalid-forward-ref.ll (unreferenced); docs/future-work.md:137-141

- **LLVM:** `test/Assembler/opaque-ptr-invalid-forward-ref.ll` CHECKs `invalid forward reference to function 'f' with wrong type: expected 'ptr' but was 'ptr addrspace(1)'` for `@a = alias void (), ptr addrspace(1) @f` against a `define void @f()`.
- **llvmkit:** The fixture file is checked in but referenced by **no test and no manifest entry** — it neither runs nor xfails. `docs/future-work.md` names it as "vendored and waiting" on three per-site forward-reference texts left over from W2.5.
- **Why:** Recorded, in `docs/future-work.md`: it needs `invalid forward reference to function '<n>' with wrong type: expected 'T' but was 'U'` plus `type of definition and forward reference of '@N' disagree` and the global/alias twins — all comparing types at the forward-reference site.
- **Fix:** Implement the per-site type comparison in the forward-reference resolution path so the three texts are produced verbatim, then add the fixture to `parser_corpus_manifest.txt` (or to a `parser_forward_refs.rs` case asserting its CHECK line). Until then, give it a manifest row with an explicit status rather than leaving it inert.

<details><summary>Verification evidence</summary>

CONFIRMED — real and still present on dev @ 2ac3e3a (working tree has no changes to any file involved). What I read: 1. Fixture exists, byte-identical to upstream. `C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/tests/fixtures/upstream/opaque-ptr-invalid-forward-ref.ll` is a verbatim copy of `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/Assembler/opaque-ptr-invalid-forward-ref.ll` (I diffed by reading both: same RUN line, same CHECK line, same `@a = alias void (), ptr addrspace(1) @f` + `define void @f()`). 2. Wired to nothing. `grep -rn "opaque-ptr-invalid-forward-ref" crates/` returns exactly zero hits outside the fixture file's own CHECK line. The only mention anywhere in the repo outside `orig_cpp/` is `docs/future-work.md:139`, and that cites the *upstream* path `test/Assembler/opaque-ptr-invalid-forward-ref.ll`, not the vendored copy. `crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt` is 14 lines total and contains no `opaque` entry (I read the whole file). No `UPSTREAM.md` row. No directory-walking harness exists that could pick it up implicitly — `parser_corpus.rs` drives only `include_str!("fixtures/parser_corpus_manifest.txt")`, and every other `fixtures/upstream/...` use in the tests is an explicit `include_bytes!`/`include_str!` of a named path. So it neither runs nor xfails, exactly as claimed. 3. Upstream text confirmed at its source. `lib/AsmParser/LLParser.cpp`, in `LLParser::parseFunctionHeader`: when `ForwardRefVals` holds the name and `FwdFn->getType() != PFT`, it errors at `FRVI->second.second` (the *forward reference's* location) with `"invalid forward reference to function '" + FunctionName + "' with wrong type: expected '" + getTypeString(PFT) + "' but was '" + getTypeString(FwdFn->getType()) + "'"`. That string appears nowhere else in `lib/`. 4. llvmkit cannot emit that text. `grep -rn "invalid forward reference to function" crates/` matches only the fixture's own CHECK line — the message is absent from `ll_parser.rs`, `parse_error.rs`, and every other source file. 5. docs citation is accurate. `docs/future-work.md:137-142` (section "Parser — a forward-referenced function is a *typed* `Function`…", found 2026-08-14, W8) says the fix "also unblocks the three per-site texts W2.5 carried", names this message first, and states its "fixture is vendored and waiting", alongside `type of definition and forward reference of '@N' disagree` and the global/alias twins — "all of which compare types at the *definition site*, where llvmkit still resolves in one end-of-module sweep." 6. Git history: the fixture landed in commit 13a6a54 ("fix(asmparser)!: one parseArgumentList for every argument list (LLParser parity W8, part 1)") and has never been referenced since. Material nuance worth carrying forward (does not contradict the claim, which only asserts the wiring gap): llvmkit does *not* silently accept the bad program. Running `cargo +1.96.0 run --release -p llvmkit-asmparser --example parse_file -- <fixture>` produces: ...opaque-ptr-invalid-forward-ref.ll:7:1: forward reference and definition of global have different types | define void @f() { | ^^^^^^ So the accept/reject verdict matches upstream; what diverges is the diagnostic *text*, the *routine*, and the *location*. llvmkit emits from `ll_parser.rs:1742-1759` (`resolve_global_forward_ref`, the end-of-module sweep), reusing upstream's `parseGlobal` message (`LLParser.cpp` global-variable definition path, `GVal->getAddressSpace() != AddrSpace`) as a generic catch-all for every `@` forward-ref type disagreement, and it points at the `define` keyword on line 7 rather than at the forward reference on line 5. No test anywhere pins that llvmkit message either (`grep "forward reference and definition of global"` over `crates/llvmkit-asmparser/tests/` → no matches), so the area is untested in both directions. Also note that wiring the fixture into `parser_corpus_manifest.txt` as `status=xfail-parse` would not close the gap — that harness only asserts that parsing fails, never the message text; a real fix needs a message-pinning test plus the per-site check in the function-header path.

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

### 77. `DIFlags` / `DISPFlags` are stored as joined source text, not bitfields

*metadata model* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_metadata_field_value`), crates/llvmkit-ir/src/metadata.rs

- **LLVM:** `DINode::DIFlags` and `DISubprogram::DISPFlags` are `uint32_t` bitfields with `getFlag`/`getFlagString`/`splitFlags`; `AsmWriter::printDIFlags` emits them with a `ListSeparator(" | ")`.
- **llvmkit:** The parsing half landed (the `|`-joined form is read, mirroring upstream's repeated `lltok::bar` loop) but the written disjunction is kept as one `Enum(String)` field value, so `DIFlagPublic | DIFlagPrototyped` round-trips as text and never becomes a set. Printed bytes agree today because the separator matches.
- **Why:** Recorded in docs/future-work.md: same milestone and same reason as the DWARF tables — the bitflag type is only worth its keep once something reads it.
- **Fix:** Introduce the two bitflag types with `split_flags`/`flag_string` ports and store the decoded set; printing then derives the `|` list instead of echoing source text.
- **Correction from verification:** The claim is accurate as to the model divergence, but its last sentence over-claims. "Printed bytes agree today because the separator matches" holds only for input that is already in upstream's canonical form. Upstream normalizes on the round-trip and llvmkit cannot, because it never builds the bitmask: (a) `DINode::splitFlags` emits a fixed order — accessibility first, then pointer-to-member representation, then `HANDLE_DI_FLAG` order — so `flags: DIFlagPrototyped | DIFlagPublic` prints back from LLVM as `DIFlagPublic | DIFlagPrototyped` while llvmkit echoes the source order; (b) `FlagPrivate=1`, `FlagProtected=2`, `FlagPublic=3`, so `DIFlagPrivate | DIFlagProtected` ORs to 3 and LLVM prints `DIFlagPublic`, while llvmkit prints both terms; (c) duplicates collapse under `|=` upstream and survive in llvmkit; (d) `printDIFlags` returns early on a zero mask, so `flags: 0` disappears from LLVM's output but is stored and reprinted by llvmkit as `Integer(0)`. A second, unrecorded consequence of the same root cause: upstream's grammar comment for `DIFlagField` is `::= DIFlagVector '|' DIFlagFwdDecl '|' uint32 '|' DIFlagPublic` and its `parseFlag` lambda accepts an unsigned `APSInt` as any term of the chain. llvmkit's `parse_metadata_field_value` dispatches on the leading token: `Token::IntegerLit` returns `MetadataFieldValue::Integer` with no `|` loop at all, and the `Token::DiFlag | Token::DiSpFlag` loop rejects anything but another flag token after a bar (`_ => return Err(self.expected("debug info flag after '|'"))`). So `flags: 4 | DIFlagPublic` and `flags: DIFlagPublic | 4` both fail to parse in llvmkit and both are legal upstream.

<details><summary>Verification evidence</summary>

crates/llvmkit-ir/src/metadata.rs:2001-2009 — `pub enum MetadataFieldValue<B>` has variants `Null | Bool | Integer(i128) | String | Enum(String) | Metadata | MetadataList`. There is no bitflags variant and no `DiFlags`/`DispFlags` value type anywhere in llvmkit-ir; grep for `DiFlags|DispFlags` across crates/llvmkit-ir/src returns only `MetadataFieldKind::DiFlags` / `::DispFlags` (metadata.rs:477,479, documented as "one or more `DIFlag*` names joined with `|`") and the field-table rows that use them. crates/llvmkit-ir/src/dwarf.rs:668-669 has only `decl_lookup!(di_flag, di_flag_string, DI_FLAGS, ...)` / `disp_flag` — name-to-u32 lookups, not a stored type. crates/llvmkit-asmparser/src/ll_parser.rs:5890-5904 (`parse_metadata_field_value`) — the `Token::DiFlag(s) | Token::DiSpFlag(s)` arm builds `let mut value = (*s).to_owned()`, loops `while matches!(self.peek(), Token::Bar)`, and does `value.push_str(" | "); value.push_str(&next);`, returning `MetadataFieldValue::Enum(value)`. The comment at 5882-5889 states this outright: "Kept here as the joined source text rather than a bitmask: modelling `DINode::DIFlags` / `DISubprogram::DISPFlags` as bitflags is deferred (see `docs/future-work.md`)". crates/llvmkit-ir/src/asm_writer.rs:3497 — `MetadataFieldValue::Enum(s) => f.write_str(s)?`, i.e. the stored text is written back verbatim; nothing re-derives an ordering or collapses bits. Symptom that pins the practical cost: ll_parser.rs:5615-5618, the `parseDISubprogram` definition guard, tests for the definition bit by `flags.split('|').any(|flag| flag.trim() == "DISPFlagDefinition")` on the `Enum` string (falling back to a real bit test `bits & definition_bit != 0` only when the field was written as an integer). Upstream reads `SPFlags & SPFlagDefinition`. Upstream side, all in orig_cpp/llvm-project-llvmorg-22.1.4/llvm: include/llvm/IR/DebugInfoMetadata.h:223-240 declares `enum DIFlags : uint32_t`, `FlagAccessibility = FlagPrivate | FlagProtected | FlagPublic`, `LLVM_MARK_AS_BITMASK_ENUM(FlagLargest)`, plus `getFlag` / `getFlagString` / `splitFlags`; the same file at 2315-2321 declares the `DISPFlags` counterparts. lib/AsmParser/LLParser.cpp:5163-5195 (`parseMDField(..., DIFlagField&)`) accumulates `Combined |= Val` over a `do { } while (EatIfPresent(lltok::bar))` and calls `Result.assign(Combined)` — a uint32, not text; 5203-5235 is the `DISPFlagField` twin. lib/IR/AsmWriter.cpp:2014-2031 `MDFieldPrinter::printDIFlags` returns early on `!Flags`, calls `DINode::splitFlags`, and joins with `ListSeparator FlagsFS(" | ")`; 2033-2055 `printDISPFlags` does the same but always prints the field, emitting `0` for an empty mask. lib/IR/DebugInfoMetadata.cpp `DINode::splitFlags` special-cases `FlagAccessibility` and `FlagPtrToMemberRep` first, then walks `HANDLE_DI_FLAG` order — which is where the canonical output ordering comes from. The vendored include/llvm/IR/DebugInfoFlags.def gives `HANDLE_DI_FLAG(1, Private)`, `(2, Protected)`, `(3, Public)`. The project's own ledger agrees and does not claim closure: docs/future-work.md:1219-1233 is still an open bullet, "`DIFlags` / `DISPFlags` are not bitflags", recording that only the parsing half landed 2026-08-07.

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

### 85. The per-operand `elementtype` half of `verifyInlineAsmCall` is unported

*verifier / call surface* — crates/llvmkit-ir/src/inline_asm.rs, crates/llvmkit-ir/src/verifier.rs, crates/llvmkit-ir/src/ir_builder.rs (call surface)

- **LLVM:** `Verifier::verifyInlineAsmCall` checks per-operand `elementtype` attributes against the constraint records, in addition to the label rules W4 ported.
- **llvmkit:** The label rules and all nine `InlineAsm::verify` messages are reachable, but the `elementtype` half is absent.
- **Why:** Recorded in docs/future-work.md (W4): the call surface cannot spell per-operand `elementtype` attributes yet, so the check has nothing to read. The `Flag` / `ConstraintCode` bit encodings from the same header are recorded as backend serialization and deliberately out of scope.
- **Fix:** Grow the call-building surface to carry per-operand attribute sets, then port the `elementtype` arm of `verifyInlineAsmCall`.
- **Correction from verification:** Accurate as stated — the divergence is real and still present — with one factual refinement to the *reason* llvmkit records for it. Confirmed: `Verifier::verifyInlineAsmCall`'s per-operand loop has three Check messages ("Operand for indirect constraint must have pointer type", "Operand for indirect constraint must have elementtype attribute", "Elementtype attribute can only be applied for indirect constraints"). None of the three strings exists anywhere in the llvmkit tree. Only the two label rules are ported (call arm and callbr arm), and llvmkit marks the rest deferred in an inline comment. All nine `InlineAsm::verify` messages are indeed present and reachable (`InlineAsmVerifyError` has exactly nine variants; `verify_inline_asm` is called from `ll_parser.rs:13391`), as the claim says. Refinement: the stated blocker — "the current call surface cannot spell per-operand elementtype attrs" (verifier.rs comment, echoed in docs/future-work.md) — is true only of the *typed builder* helper, not of the call surface generally. `IrBuilder::inline_asm_call` hardcodes `CallAttributeData::default()`, so that one path cannot attach arg attributes. But `CallInstData` already carries `attrs: CallAttributeData` with `arg_attrs: Box<[AttributeStorage]>`; `LLParser::parse_call` collects per-argument attributes via `parse_optional_param_attrs` into exactly that field; `elementtype` is a live `AttrKind` parsed by the lexer/parser (`ll_parser.rs:9613`, `attributes.rs:389`); and the AsmWriter prints per-arg attrs back out. That claim was wrong until the indirect/inline-asm call attribute loss was closed: an inline-asm call's argument attributes were parsed and then discarded by `IrBuilder::inline_asm_call`'s hardcoded `CallAttributeData::default()`, so `ptr elementtype(i32) %p` printed back as `ptr %p` (`test/Verifier/inline-asm-indirect-operand.ll`, fed verbatim). Since that fix the attribute does parse, store and re-print, and what remains unported is only the *verifier* half — llvmkit never reads `c.attrs.arg_attrs()` for the inline-asm case, which is what this entry is about. Pinned by `crates/llvmkit-asmparser/tests/parser_calls.rs::inline_asm_call_elementtype_argument_attribute_round_trips`. The elementtype half is blocked by the builder ergonomics only; the data model and parser already support it, so the deferral rationale on record understates what is available.

<details><summary>Verification evidence</summary>

Upstream, C:/Users/olegg/Desktop/llvmkit/orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/IR/Verifier.cpp, `Verifier::verifyInlineAsmCall`: loops `IA->ParseConstraints()`, skips `isLabel` (counting LabelNo) and `!CI.hasArg()`, then for `CI.isIndirect` checks pointer type and `Call.getParamElementType(ArgNo)`, else checks `!Call.paramHasAttr(ArgNo, Attribute::ElementType)`; the LabelNo comparison against callbr dests is the tail. llvmkit, C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/verifier.rs:2775-2788 (`check_call`): the InlineAsm arm only emits "Label constraints can only be used with callbr", followed by the literal comment "Full indirect-constraint / elementtype parity is deferred: the current call surface cannot spell per-operand elementtype attrs." Lines 3230-3246 (`check_callbr`) port only the "Number of label constraints does not match number of callbr dests" twin. A repo-wide grep for "indirect constraint" and "Elementtype attribute can only" returns zero hits; the only `elementtype` mention in verifier.rs is that deferral comment. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/inline_asm.rs: `ConstraintInfo` carries `is_indirect` (line 353) and `verify_inline_asm` (line 718) is the static `InlineAsm::verify` port with the nine `InlineAsmVerifyError` variants; it never inspects call arguments, matching its doc note that the caller checks label counts. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/instr_types.rs:2165-2172 and 2246-2298: `CallAttributeData { return_attrs, arg_attrs: Box<[AttributeStorage]>, ... }` is a field of `CallInstData`. C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/src/ll_parser.rs:12949-13004 (`parse_call`) fills `arg_attrs` from `parse_optional_param_attrs`; line 9613 maps the keyword to `AttrKind::ElementType`; C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/asm_writer.rs (~line 2080) prints `c.attrs.arg_attrs().get(idx)`. (A sentence citing `ir_builder.rs`'s `inline_asm_call` passing `CallAttributeData::default()` stood here, as the actual builder-only gap; that code was replaced by the erased-callee `call` construction on 2026-08-21 -- `inline_asm_call` now forwards to `IrBuilder::call_erased` and carries whatever `CallSiteConfig` it is given -- so the citation was deleted rather than re-pointed. Nothing else in this block was re-checked at that time, and it carries no date for the same reason.) Deferral also recorded at C:/Users/olegg/Desktop/llvmkit/docs/future-work.md:1276-1286.

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

### 106. `FunctionCfg`'s predecessor lists are in block order, not use-list order

*analysis* — crates/llvmkit-ir/src/cfg.rs — `FunctionCfg::new`

- **LLVM:** `llvm/IR/CFG.h`'s `PredIterator` walks `BB->user_begin()`, skips every user that is not a terminator `Instruction`, and yields `cast<Instruction>(*It)->getParent()`. Because `Use::addToList` head-inserts, `predecessors(BB)` reads newest-first — reverse creation order — and every consumer (`AssemblyWriter::printBasicBlock`, the verifier, the dominator-tree builder) sees that order. It is neither sorted nor deduplicated.
- **llvmkit:** `FunctionCfg::new` builds its predecessor map by iterating `function.basic_blocks()` and pushing each block onto its successors' lists, so `FunctionCfg::predecessors` answers in **block order**. The underlying use list *is* correct (`ValueData::add_use` head-inserts, and its rustdoc cites `Use::addToList` as the reason), so the two disagree.
- **Why:** the CFG snapshot was written as an adjacency recomputation rather than a use-list view. Found while porting `AssemblyWriter::printBasicBlock`'s `; preds = …` comment, which needs upstream's order: that port reads the block's use list directly and does **not** go through `FunctionCfg`, so the divergence is not observable in printed output.
- **Fix:** build the predecessor lists from each block's use list — `asm_writer.rs`'s `block_predecessors` helper is the routine — or make `FunctionCfg::predecessors` delegate to it. Check `crates/llvmkit-ir/tests/cfg_basic.rs`, `dominator_tree_basic.rs` and the verifier's own predecessor construction for order-dependent expectations first.

### 119. A scalable shuffle answers "nothing known" where LLVM propagates the demanded set

**Severity:** model-gap

*value tracking* — crates/llvmkit-ir/src/value_tracking.rs (`shuffle_source_demands`),
reached from `computeKnownBits`' and `computeKnownFPClass`' `ShuffleVector` arms

- **LLVM:** the file-static `getShuffleDemandedElts(const ShuffleVectorInst *, const APInt &, APInt &, APInt &)`
  in `lib/Analysis/ValueTracking.cpp` opens with
  `if (isa<ScalableVectorType>(Shuf->getType())) { assert(DemandedElts == APInt(1,1)); DemandedLHS = DemandedRHS = DemandedElts; return true; }`
  — a scalable shuffle succeeds, with both sources demanded, so `computeKnownBits`
  recurses into the operands.
- **llvmkit:** `shuffle_source_demands` answers `None` for a scalable operand
  (`let (source_width, false) = vector_shape(lhs)? else { return None; }`), so
  `shuffle_vector_known_bits` returns `KnownBits::unknown` and `shuffle_vector_fp_class`
  returns `KnownFpClass::unknown` without recursing. Conservative-safe, never wrong, but
  strictly less precise than LLVM.
- **Why:** written when no scalable `shufflevector` instruction could be constructed, so
  the branch was unreachable and the `false` pattern read as an assertion rather than a
  choice. Porting `ShuffleVectorInst::isValidOperands` made it reachable.
- **Fix:** reproduce the wrapper's scalable arm — return both sources fully demanded,
  reusing the incoming demanded set. That needs llvmkit's demanded-elements width for a
  scalable vector settled first: upstream's non-`DemandedElts` `computeKnownBits` entry
  point seeds `APInt(1, 1)` for anything that is not a `FixedVectorType`, which is what
  makes its `assert` hold, while `shuffle_source_demands` falls back to
  `ApInt::all_ones(mask.len())`. `unittests/Analysis/ValueTrackingTest.cpp` has no
  scalable-shuffle `computeKnownBits` case, so the fix would land with a recorded absence
  of upstream coverage rather than a ported fixture.

<details><summary>Verification evidence</summary>

Both halves read at commit 481e276 plus this change. Upstream: orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/Analysis/ValueTracking.cpp, `static bool getShuffleDemandedElts(const ShuffleVectorInst *Shuf, const APInt &DemandedElts, APInt &DemandedLHS, APInt &DemandedRHS)` — its first statement is the `isa<ScalableVectorType>(Shuf->getType())` arm quoted above, and only after it does the routine read `cast<FixedVectorType>(Shuf->getOperand(0)->getType())->getNumElements()` and delegate to the public `llvm::getShuffleDemandedElts`. The `APInt(1,1)` the assert expects is what `llvm::computeKnownBits(const Value *V, KnownBits &Known, const SimplifyQuery &Q, unsigned Depth)` seeds when the type is not a `FixedVectorType`. llvmkit: crates/llvmkit-ir/src/value_tracking.rs `shuffle_source_demands` computes `demanded` first, then destructures `vector_shape(lhs)` / `vector_shape(rhs)` with a `false` scalability pattern and returns `None` on a scalable operand; the guard reads the OPERAND type where upstream reads the RESULT type, which selects the same shuffles because `ShuffleVectorInst`'s `ArrayRef<int>` constructor takes the result's scalability from V1. Reachability: the two masks `isValidOperands`' scalable branch admits are all-`Lane(0)` and all-`Poison`, and both match `m_ZeroMask` (`Elem == 0 || Elem == PoisonMaskElem`), so `shuffle_vector_known_bits`' `splat_value` fast path is tried first — but `getSplatValue`'s shuffle arm then requires `m_InsertElt(..., m_ZeroInt())` on operand 0, which a function parameter is not. So `shufflevector <vscale x 4 x i32> %a, <vscale x 4 x i32> %b, <vscale x 4 x i32> zeroinitializer` with `%a`/`%b` as parameters — the shape `test/Bitcode/vscale-round-trip.ll::@non_const_shufflevector` writes — reaches `shuffle_source_demands` and gets `None`. No test in the tree exercises it; the absence is why this is recorded rather than pinned.

</details>

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

### 95. Two upstream diagnostics are unreachable upstream and are recorded, not invented

*parser — function header/body* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_argument_list`, `define_numbered_label`); ~/.claude/plans/llparser-parity-ledger.md

- **LLVM:** `argument can not have void type` sits behind `parseArgumentList`'s `AllowVoid = false`, so `parseType` already refuses a literal `void` with `void type only allowed for function results`; `unable to create block numbered '<N>'` sits behind `defineBB`, which runs `checkValueID` first, so a colliding numbered label has already failed.
- **llvmkit:** Neither message exists, matching upstream's reachable behaviour exactly.
- **Why:** Recorded in W8: "recorded rather than given invented triggers". Listed so the ledger's MISSING rows for these two are not treated as gaps.
- **Fix:** Mark both ledger rows `N/A(unreachable-upstream)` in the W14 ledger-v3 pass.
- **Correction from verification:** Still present as a code fact — neither message exists in llvmkit — but the claim's rationale is half wrong, so this is one accurate recording plus one genuine unclosed gap, not two faithful matches. ACCURATE: `argument can not have void type` (LLParser.cpp:3401) is genuinely dead. `parseArgumentList` calls `parseType(ArgTy)` and LLParser.h:453-455 defaults `AllowVoid = false`; `parseType`'s end-of-type `default:` arm (LLParser.cpp:3034) rejects every void with `void type only allowed for function results` before returning success. llvmkit matches upstream's reachable behaviour exactly here. INACCURATE: `unable to create block numbered '<N>'` (LLParser.cpp:3900) IS reachable upstream. `checkValueID` only rejects `ID < NextID` ("label expected to be numbered 'N' or greater"); it does nothing about an id at or above `NextID` that already carries a pending non-label forward reference in `ForwardRefValIDs`. Such an id passes `checkValueID`, then `getBB(NameID)` -> `getVal(ID, LabelTy)` -> `checkValidVariableType` errors `'%N' is not a basic block` and returns nullptr, so `defineBB` emits `unable to create block numbered '<N>'` — which overwrites the earlier message, because `LLLexer::Error` keeps the last message at equal `ErrorPriority::Parser`. Reproducer: `define void @f() {\nentry:\n %0 = add i32 %5, 1\n ret void\n5:\n ret void\n}` — after `%0`, `NumberedVals.getNext()` is 1 and `ForwardRefValIDs[5]` holds an `Argument(i32)`; the label `5:` passes checkValueID (5 >= 1) and fails in getBB. This is the same mechanism as the named twin, which llvmkit already implements and documents. So the correct statement is: `argument can not have void type` is unreachable and correctly omitted; `unable to create block numbered '<N>'` is reachable via a numbered forward-reference type collision and is a real missing diagnostic in llvmkit, together with the stale premise recorded in the test comment at parser_module_level.rs:1111-1114 and in the parity ledger.

<details><summary>Verification evidence</summary>

Upstream C++ (orig_cpp/llvm-project-llvmorg-22.1.4/llvm/): - lib/AsmParser/LLParser.cpp:3400-3401 — `if (ArgTy->isVoidTy()) return error(TypeLoc, "argument can not have void type");` sits after `parseType(ArgTy)`. - include/llvm/AsmParser/LLParser.h:453-455 — `bool parseType(Type *&Result, const Twine &Msg, bool AllowVoid = false);` and the 2-arg overload forwarding `AllowVoid`; `parseArgumentList` passes neither, so AllowVoid is false. - lib/AsmParser/LLParser.cpp:3034 — in parseType's suffix loop, `default:` arm: `if (!AllowVoid && Result->isVoidTy()) return error(TypeLoc, "void type only allowed for function results");` — applies to every parsed type before success. Confirms part A dead. - lib/AsmParser/LLParser.cpp:3359-3365 — `checkValueID` body is only `if (ID < NextID) return error(... "expected to be numbered '" + Prefix + Twine(NextID) + "' or greater");`. It does NOT check forward refs. This is what breaks the claim. - lib/AsmParser/LLParser.cpp:3888-3900 — `defineBB`: `checkValueID` then `BB = getBB(NameID, Loc); if (!BB) { P.error(Loc, "unable to create block numbered '" + Twine(NameID) + "'"); return nullptr; }`. - lib/AsmParser/LLParser.cpp:3776-3790 — `getVal(unsigned ID, ...)`: falls back to `ForwardRefValIDs.find(ID)`, and `if (Val) return P.checkValidVariableType(Loc, "%" + Twine(ID), Ty, Val);` — a forward ref of wrong type yields nullptr. - lib/AsmParser/LLParser.cpp:1771-1783 — `checkValidVariableType`: `if (Ty->isLabelTy()) error(Loc, "'" + Name + "' is not a basic block"); ... return nullptr;` - lib/AsmParser/LLLexer.cpp:37-42 — `void LLLexer::Error(LocTy, const Twine &Msg, ErrorPriority Priority) { if (Priority < ErrorInfo.Priority) return; ErrorInfo.Error = ...; }` with include/llvm/AsmParser/LLLexer.h:98-102 `ParseError` using `ErrorPriority::Parser` — equal priority overwrites, so defineBB's message is the one reported. - lib/AsmParser/LLLexer.cpp:1184-1190 — `5:` lexes to `lltok::LabelID`; LLParser.cpp:7050-7067 `parseBasicBlock` passes it as `NameID` to `defineBB`. llvmkit (C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-asmparser/): - src/ll_parser.rs:6578-6586 — doc comment on `parse_argument_list` recording the void message as unreachable; src/ll_parser.rs:6423 emits `void type only allowed for function results`. Part A matches. - src/ll_parser.rs:14687-14715 — `define_named_block` emits `unable to create block named '{name}'`, and its doc comment at 14689-14695 explicitly describes the getVal-error-then-overwrite mechanism — the exact mechanism the claim denies for the numbered twin. - src/ll_parser.rs:14729-14745 — `define_numbered_label` checks `defined_numbered_blocks`, then `check_value_id`, then delegates; src/ll_parser.rs:14747-14783 `define_numbered_block` checks only `local_numbered` (defined values) and never `forward_ref_numbered`. - src/ll_parser.rs:14609-14611 — `forward_ref_numbered: RefCell<BTreeMap<u32, ForwardRef>>` documented as mirroring `PerFunctionState::ForwardRefValIDs` — the map that would have to be consulted, and is not. - tests/parser_module_level.rs:1111-1114 — the stale premise verbatim: "The numbered twin ... is not tested: `defineBB` runs `checkValueID` first, so a numbered label that collides has already failed with `label expected to be numbered 'N' or greater`". - Repo-wide grep of crates/ for both message strings returns only those two doc/test comments — neither diagnostic is emitted by llvmkit today.

</details>

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
