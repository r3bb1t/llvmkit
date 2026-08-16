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

**Every entry must be verified against the tree before it is trusted.** This
project has repeatedly found its own recorded premises wrong — three failed on
contact in a single session — so treat a row as a hypothesis with a citation,
not as a fact.

Upstream is cited **by symbol, never by line number** (repo law: line numbers
rot the moment the vendored tree moves).

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

**Fix:** carry the operand index on `ValueUse::Instruction`. Touches every
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
are unfiltered. Ported optimisations that gate on `hasOneUse` can therefore
see a different answer from upstream on a value a debug record names.

**Fix:** decide whether the public accessors should mirror `getNumUses`
(filter) and audit their callers, or gain filtered twins.

---

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

---


---

# Inventory

The 99 entries below were swept out of `docs/future-work.md`, the source
comments, the test doc comments and the parity program's own records, then
each was **independently re-verified against the tree** — 3 of the 102
candidates turned out to be already closed or misdescribed and were dropped.
Where verification corrected the original claim, the correction is quoted
under the entry.

llvmkit file/line references are indicative and rot; the symbol names do not.

Where an inventory entry overlaps the hand-written section above, the section
above is current: it reflects what wave 12 actually closed, while the sweep
read the tree as it stood when each entry was first recorded.

## Rejects valid input

llvmkit refuses IR that LLVM accepts — the worst kind, a parser that cannot read LLVM's own output.

### 1. `u0x…` and >64-bit literals are rejected wherever a `uint64` is wanted

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:2244 (`parse_uint64`), :2216 (`parse_uint32`)

- **LLVM:** `LLParser::parseUInt64` accepts any `lltok::APSInt` whose value is unsigned and reads it with `APSInt::getLimitedValue()`, which saturates at `UINT64_MAX`. `u0x10` is an unsigned APSInt (`LLLexer::LexDigitOrNegative`'s `[us]0x` form) and is accepted at every `parseUInt64` call site; a literal wider than 64 bits saturates rather than failing.
- **llvmkit:** `Parser::parse_uint64` matches only `Token::IntegerLit` with `sign: Pos, base: Dec` and calls `digits.parse::<u64>()`, so `u0x10` answers `expected integer` and an over-wide decimal literal is rejected outright instead of saturating. `parse_uint32` has the same narrow shape, but its saturation is unobservable because the `0xFFFFFFFF + 1` range check rejects an oversized value either way (what `align-param-attr-error2.ll` pins).
- **Why:** Recorded: it is a W5-owned routine with 25 call sites, no `test/Assembler` fixture reaches either case, and the honest fix reads the token through `parse_int_literal` — the APSInt token model — which changes where the diagnostic's span comes from. Deliberately not smuggled into the W10 summary-index wave.
- **Correction from verification:** Substantively accurate and still present; one sub-detail is wrong. The `[us]0x[0-9A-Fa-f]+` form is lexed by `LLLexer::LexIdentifier`, not `LLLexer::LexDigitOrNegative` (the latter only handles `[-]?[0-9]+`, plain `0x…` FP forms, and labels).
- **Fix:** Route both through `parse_int_literal` + `ParsedApsInt`, accepting any unsigned APSInt token (decimal, `u0x`, `0x`) and reproducing `getLimitedValue`'s saturation instead of failing; then re-check every one of the 25 call sites' diagnostic spans, since the span now comes from the APSInt token rather than the digit run.

### 2. `%cs = catchswitch …` does not parse

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:11058 (bare arm), :11131-11205 (named-result table and its `_ =>` fall-through)

- **LLVM:** `catchswitch` produces a token value, so `LLParser::parseInstruction`'s `lltok::kw_catchswitch` case is reached from both the bare and the named-result spellings; `LLParser::parseCatchSwitch` binds the result normally.
- **llvmkit:** The bare form is dispatched (`Token::Instruction(Opcode::CatchSwitch)` arm of `parse_basic_block`), but the named-result opcode table has no `CatchSwitch` arm — the special-cased named terminators there are `call`, `invoke` and `callbr` only — so `%cs = catchswitch within none [label %h] unwind to caller` falls to the `_ =>` arm and answers `expected instruction opcode supported by this parser (got CatchSwitch)`. Valid IR that does not parse.
- **Why:** Recorded as a P0 in the W1 sense (valid IR that does not parse) rather than a missing message; found while testing `expected scope value for catchswitch`, which the bare form reaches. No reason for the omission is recorded — it reads as an oversight in the named-result table.
- **Correction from verification:** The claim is accurate in substance; two refinements. (1) Line numbers have drifted slightly in the current working tree: the bare-form arm is at ll_parser.rs:11060 (not 11058), and the named-result opcode table spans 11137-11198 with its `_ =>` fall-through at 11199-11207 (claim said 11131-11205).
- **Fix:** Add a `Opcode::CatchSwitch => self.parse_catchswitch(state, b_ref, &result_name)?` arm to the named-result table, mirroring the existing bare arm's `take_live_builder` + terminator return shape (catchswitch is a terminator, so it must `return Ok(())` after `bind_local` rather than continue the loop, like the `callbr` special case above it).

### 3. A call's signature is checked against a later `declare`/`define`, which upstream leaves to the Verifier

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:10474, :10698 (the two rejections); `parse_direct_callee` around :13257

- **LLVM:** `LLParser::getGlobalVal` mints an untyped placeholder — a `ptr`-typed `GlobalVariable` — for a forward-referenced callee, so `LLParser::parseFunctionHeader` compares only `FwdFn->getType() != PFT`, which after opaque pointers is nothing but the address space. The signature is never compared at the definition site; a call whose arguments disagree with the eventual definition parses, and `Verifier::visitCallBase` rejects it with `Call parameter type does not match function signature!`.
- **llvmkit:** `parse_direct_callee`'s forward-reference arm calls `Module::add_function_dyn` with the *call site's* signature, so the placeholder is a real `Function` with a real `FunctionType` and cannot be re-typed later. `declare`/`define` therefore reject the reuse with two texts upstream never prints: `forward function declaration with matching signature` and `forward function definition with matching signature`.
- **Why:** Recorded, and the check is load-bearing rather than cosmetic: dropping it would leave a call wired to a function whose type it does not match, because llvmkit has no way to give an existing `Function` a different `FunctionType`.
- **Correction from verification:** The divergence is real and still present, and the llvmkit half of the description is exactly right. One clause about upstream is wrong, and correcting it makes the divergence WORSE, not smaller. Wrong clause: "`Verifier::visitCallBase` rejects it with `Call parameter type does not match function signature!`". It does not.
- **Fix:** Apply the shape W2 gave value forward references — an untyped placeholder plus RAUW at the definition — to the callee position, so `parseFunctionHeader` can mint a fresh `Function` with the definition's type and re-point every pending call.

### 4. A self-typed aliasee constant expression does not parse

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:7099-7130 (`parse_alias_or_ifunc`), :8529 (`parse_constant_expr`, which takes `result_ty`)

- **LLVM:** `LLParser::parseAliasOrIFunc` branches on the aliasee's first token: `bitcast`, `getelementptr`, `addrspacecast` and `inttoptr` go through a bare `parseValID` (its comment: the bitcast dest type is not present, it is implied by the dest type), and anything else goes through `parseGlobalTypeAndValue`, which is TYPE VALUE. The result must be `ValID::t_Constant` or the diagnostic is `invalid aliasee`, and the pointer check then runs on the aliasee *value's* type, taking the address space from it.
- **llvmkit:** `parse_alias_or_ifunc` always reads TYPE then VALUE (`parse_type` followed by `parse_alias_target`), so `@a = alias i32, bitcast (ptr @g to ptr)` and the `getelementptr` / `addrspacecast` / `inttoptr` spellings do not parse at all. `invalid aliasee` is unreachable, since it only fires on the branch llvmkit does not have.
- **Why:** Recorded. An attempt was made and reverted: the blocker is that `Parser::parse_constant_expr` takes a `result_ty` and llvmkit has no entry point for a constant expression that types itself — every constexpr arm is reached with the demanded type already in hand.
- **Fix:** Give constant expressions a self-typing entry point (the constexpr analogue of W4's type-agnostic `ValID`), then branch `parse_alias_or_ifunc` on the first token as upstream does, run the pointer check on the parsed aliasee's own type, take the address space from it, and report `invalid aliasee` when the result is not a constant.

### 5. A vector-of-pointers GEP base, and vector GEP indices, are rejected after passing every upstream check

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:12134-12217 (`parse_gep`, conversion at :12200-12212); `crates/llvmkit-ir/src/ir_builder.rs` (`gep_with_flags`)

- **LLVM:** `LLParser::parseGetElementPtr` asks `dyn_cast<PointerType>(BaseType->getScalarType())`, so a `<N x ptr>` base is legal, and `GetElementPtrInst::getGEPReturnType`'s vector arm gives the result its vector shape. Vector indices are likewise legal and lane-matched.
- **llvmkit:** `parse_gep` reproduces all of upstream's checks (scalar-type pointer test, lane agreement, sized base element, scalable-struct rejection, `indexed_gep_type`) and only then fails at the builder, because `IrBuilder::gep_with_flags` takes a scalar `PointerValue` and `IntValue<IntDyn>` indices: `a vector-of-pointers getelementptr base is not yet supported` / `vector getelementptr indices are not yet supported`.
- **Why:** Recorded as an IR-model gap, not a parser one — the parser deliberately runs every upstream rule before the conversion so the recorded gap is all that is left. Also listed under the 2026-07-06 upstream-parity follow-ups and the ergonomics backlog.
- **Correction from verification:** The main divergence is REAL and still present, exactly as described. One sub-clause is wrong. Corrected statement: `LLParser::parseGetElementPtr` accepts a `<N x ptr>` base (`dyn_cast<PointerType>(BaseType->getScalarType())`) and vector indices, with `GetElementPtrInst::getGEPReturnType`'s vector arm giving the result its vector shape.
- **Fix:** Add an erased, vector-capable GEP builder beside `gep_with_flags` — the same third-tier `_erased` move the vector binops (`int_binop_erased`), compares (`int_cmp_erased`) and casts (`int_cast_erased`) already made — computing the lane-matched result type from base and index shapes, and route `parse_gep`'s conversion through it.

### 6. A numeric-only or mixed `DIFlags` / `DISPFlags` term is rejected, and written flags are never canonicalised

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:5879-5903 (the `DiFlag`/`DiSpFlag` arm), :5668-5685 (per-term validation), :5768-5769 (field kinds);

- **LLVM:** `LLParser::parseMDField(DIFlagField&)` and its `DISPFlagField` twin loop `do { parseFlag } while (EatIfPresent(lltok::bar))`, and `parseFlag` accepts either an unsigned `lltok::APSInt` (read with `parseUInt32`) or a `lltok::DIFlag`, OR-ing the terms into one `DINode::DIFlags` / `DISubprogram::DISPFlags` bitfield.
- **llvmkit:** The `Token::DiFlag | Token::DiSpFlag` arm keeps the `|`-joined **source text** as `MetadataFieldValue::Enum(String)` (each term validated against `dwarf::di_flag` / `disp_flag`), and after a `|` it accepts only another flag token — `expected debug info flag after '|'`. So `spFlags: DISPFlagDefinition | 4096` and `flags: 4 | DIFlagPublic` are rejected though upstream accepts them, and a purely numeric `flags: 3` is stored as `Integer(3)` and printed back as `3` where `llvm-dis` prints `DIFlagPublic`.
- **Why:** Recorded reason covers only the storage half: modelling `DINode::DIFlags` / `DISubprogram::DISPFlags` as bitflags is deferred to the debug-info/metadata milestone, on the ground that the bitflag type is worth its keep once something reads it, and that the joined text matches `printDIFlags`'s `ListSeparator(" | ")`.
- **Correction from verification:** The body of the claim is accurate; the TITLE is wrong on one half. A numeric-only term is NOT rejected — `flags: 3` parses fine, is stored as `MetadataFieldValue::Integer(3)` and printed back verbatim as `3` (llvm-dis prints `DIFlagPublic`).
- **Fix:** Model both as `u32` bitflag types with `getFlag`/`getFlagString`/`splitFlags` ports (the tables already exist as `llvmkit_ir::dwarf::di_flag` / `disp_flag`), accept an unsigned integer term anywhere in the `|` chain as upstream's `parseFlag` does, store the OR, and print through the `splitFlags` + trailing-`Extra` shape. Land it with the DWARF-encoding storage fix below — both are the same normalisation milestone.

### 7. Attribute-group `align = N` / `alignstack = N` rejected where upstream asserts

*parser (LLParser)* — crates/llvmkit-asmparser/src/ll_parser.rs:9487-9496 (align), crates/llvmkit-asmparser/src/ll_parser.rs:9503-9512 (alignstack)

- **LLVM:** Inside an attribute group the grammar is `align = N`, read with `parseUInt32` and given no validation at all; upstream then constructs `Align(Value)` / `MaybeAlign(unsigned)`, whose rejections of zero and non-powers-of-two are C++ `assert`s. In a release `llvm-as` those asserts are compiled out, so the value is simply accepted.
- **llvmkit:** `parse_optional_attrs` calls `self.check_alignment_value(value, value_loc)` in the attribute-group arm, reusing `parseOptionalAlignment`'s wording, and returns a parse error for the two values upstream would assert on. llvmkit forbids runtime panics in production paths, so there is no way to reproduce the assert.
- **Why:** Recorded inline at `ll_parser.rs:9479-9486` and in `docs/future-work.md` as "a deliberate divergence in *diagnostic presence*, never in accept/reject". That framing is slightly optimistic: against an assertions-disabled upstream this is an accept/reject difference, not just a message difference.
- **Correction from verification:** The divergence is REAL and STILL PRESENT, but the description is wrong in three places and incomplete in a fourth. Corrected statement: In `parse_fn_attribute_value_pairs` (NOT `parse_optional_attrs` — no such function exists in the file), the attribute-group forms `align = N` and `alignstack = N` reject values that upstream accepts in a …
- **Fix:** Decide which upstream is the oracle. If assertions-enabled LLVM is the contract, keep the check and restate the recorded reason as "matches an assertions-enabled `llvm-as`, diverges from a release one". If a release `llvm-as` is the contract, drop `check_alignment_value` in the `in_attr_group()` arm, store the raw `u32`, and let `Verifier` reject the bad alignment — which is where upstream's non-assert rejection actu …

### 8. Verifier rejects a zero-incoming phi in a reachable block

*verifier* — crates/llvmkit-ir/src/verifier.rs:2962-2978

- **LLVM:** `Verifier::visitPHINode` checks only that the incoming count equals the predecessor count. A phi with zero incomings in a block with zero predecessors passes on `0 == 0`, so LLVM's verifier accepts it.
- **llvmkit:** Before delegating to the shared `check_phi_incoming` core, the verifier gates on reachability and fails with `VerifierRule::PhiEmptyInReachableBlock` ("phi in a block reachable from entry has no incoming values") for any empty phi in a block reachable from entry. llvmkit rejects IR LLVM accepts.
- **Why:** Recorded inline as "Defense in depth (stricter than upstream)": such a phi prints as `%p = phi i32` with no `[ … ]` pairs, which `LLParser::parsePHI` refuses to read, so accepting it would produce un-round-trippable output. The reachability gate is there because an unreachable block may legitimately have no predecessors.
- **Correction from verification:** The divergence is real and still present, but the claim misattributes the upstream check. `Verifier::visitPHINode` does NOT check incoming count vs. predecessor count — it checks only three things (phis grouped at block top, result type is not token-like, each incoming value's type equals the result type) and then explicitly defers with t …
- **Fix:** Keep the rule — the round-trip contract is stronger than upstream's verifier here — but make the divergence explicit rather than a comment: give `VerifierRule::PhiEmptyInReachableBlock` a doc line saying it has no `Verifier.cpp` counterpart, add the `UPSTREAM.md` row, and confirm no ported `test/Verifier` fixture relies on the case passing.

### 9. Vector-of-pointers GEP base and vector GEP indices rejected at parse time

*parser (LLParser) / IrBuilder* — crates/llvmkit-asmparser/src/ll_parser.rs:12198-12210, builder signature at crates/llvmkit-ir/src/ir_builder.rs (`gep_with_flags`)

- **LLVM:** `LLParser::parseGetElementPtr` accepts a `<N x ptr>` base and `<N x iM>` indices; the three tail checks it runs (sized base element, no scalable-vector-containing struct source, valid index chain) all pass for these shapes, and `GetElementPtrInst` holds them.
- **llvmkit:** Every one of upstream's rules is evaluated first — deliberately, so their diagnostics stay reachable — and then the conversion to the builder's `PointerValue` / `IntValue<IntDyn>` fails, producing llvmkit-invented messages "a vector-of-pointers getelementptr base is not yet supported" and "vector getelementptr indices are not yet supported". Valid LLVM IR is rejected.
- **Why:** Recorded inline at `ll_parser.rs:12192-12197`: the shape is "upstream-valid but not yet expressible — `gep_with_flags` takes a scalar `PointerValue` and `IntValue<IntDyn>` indices", so the conversion sits after the checks and "the recorded IR gap is all that is left here. See `docs/future-work.md`."
- **Fix:** Add an erased GEP entry point on `IrBuilder` — `gep_erased(source_ty, base: Value, indices: &[Value], flags, name)` — that validates via the existing `indexed_gep_type` walk and admits vector bases/indices, following the `_erased` third tier the codebase already uses for `int_binop_erased`. The parser then calls that instead of `gep_with_flags`, and the two bespoke messages disappear.

### 10. `shufflevector` on scalable vectors rejected outright

*IrBuilder / parser* — crates/llvmkit-ir/src/ir_builder.rs:3646-3650, reached from crates/llvmkit-asmparser/src/ll_parser.rs:12594

- **LLVM:** `ShuffleVectorInst::isValidOperands` accepts scalable operands; the only extra rule is that a scalable shuffle's mask must be all-zeros or all-poison (`ShuffleVectorInst::isValidOperands` / `Verifier::visitShuffleVectorInst`). `shufflevector <vscale x 4 x i32> %a, <vscale x 4 x i32> poison, <vscale x 4 x i32> zeroinitializer` is the canonical scalable splat idiom and parses fine.
- **llvmkit:** `IrBuilder::shuffle_vector` returns `IrError::InvalidOperation { message: "shufflevector with scalable input is not yet supported" }` as soon as the operand vector is scalable. `LLParser`'s `parse_shufflevector` routes straight to it (via `builder_err`), so the parser rejects the idiom — even though `parse_shuffle_mask` already handles the scalable mask-type rule correctly just above.
- **Why:** Unrecorded — there is no explanatory comment, only the error string. This is the one item in this list carried solely by a message rather than by a recorded rationale, and its "not yet supported" phrasing hides a hard rejects-valid behind what reads like a TODO.
- **Fix:** Port `ShuffleVectorInst::isValidOperands`'s scalable branch: allow a scalable operand when the decoded mask is entirely `Poison` or entirely `Lane(0)`, and keep the rejection (with upstream's wording) otherwise. The mask decode already exists in `shufflevector_mask_from_constant`. Add the ported `test/Assembler/shufflevector.ll` scalable cases and an `UPSTREAM.md` row;

### 11. catchswitch with a named result does not parse

*parser (EH funclets)* — crates/llvmkit-asmparser/tests/parser_function_body.rs:1307-1314; docs/future-work.md:98-105

- **LLVM:** `catchswitch` produces a token value and may be written with an explicit result name: `%cs = catchswitch within none [label %h] unwind to caller` is valid IR.
- **llvmkit:** llvmkit dispatches only the bare form; its named-result table has no `CatchSwitch` arm, so the named spelling answers `expected instruction opcode supported by this parser (got CatchSwitch)`. A test comment routes around it by writing the terminator bare.
- **Why:** Recorded in `docs/future-work.md` as "Valid IR that does not parse, so a P0 in the W1 sense rather than a missing message", found while testing `expected scope value for catchswitch`.
- **Fix:** Add a `CatchSwitch` arm to the named-result dispatch table in `parse_instruction` (it already exists on the bare path), then port the upstream `catchswitch` fixtures that use `%cs =`.

### 12. `call addrspace(1) void @f()` does not parse (P0)

*parser — call family* — crates/llvmkit-asmparser/src/ll_parser.rs:12915 (parse_call), :14028 (parse_invoke), :14167 (parse_callbr)

- **LLVM:** `LLParser::parseCall`, `parseInvoke` and `parseCallBr` each run `parseOptionalProgramAddrSpace` between the return attributes and the callee type, and `convertValIDToValue` then compares the resolved callee against `ptr addrspace(N)`.
- **llvmkit:** `parse_optional_program_addr_space` exists (W8 wired it into `declare`/`define`) but none of the three call routines call it — they go straight from return attributes to `parse_type`, so the `addrspace` token is a syntax error. Verified at parse_call, parse_invoke and parse_callbr.
- **Why:** Recorded in docs/future-work.md as a W9 P0 carried out of the wave: "Parsing and discarding it would silently drop information — worse than the current honest failure", so the fix has to thread the address space into callee resolution rather than just consume the token.
- **Correction from verification:** Still present, but the description is wrong about callbr and understates the rest. WRONG: "parseCall, parseInvoke and parseCallBr each run parseOptionalProgramAddrSpace". Upstream `LLParser::parseCallBr` does NOT — `parseOptionalProgramAddrSpace` has exactly three callsites in LLParser.cpp (parseFunctionHeader, parseInvoke, parseCall), an …
- **Fix:** Call `parse_optional_program_addr_space` at each of the three sites and thread the result into `parse_direct_callee_ref` / callee type resolution so the callee's pointer type is compared against `ptr addrspace(N)`, as `convertValIDToValue` does.

### 13. `%cs = catchswitch ...` — the named-result form is not dispatched (P0)

*parser — instruction dispatch* — crates/llvmkit-asmparser/src/ll_parser.rs:11197 (missing arm), :13969 (`parse_catchswitch`)

- **LLVM:** `LLParser::parseInstruction` reaches `parseCatchSwitch` from both the bare and the named-result path; `catchswitch` produces a `token`-typed value and may legally be given a result name.
- **llvmkit:** The bare form is dispatched, but the named-result match has no `Opcode::CatchSwitch` arm and falls into the catch-all, answering `expected instruction opcode supported by this parser (got CatchSwitch)`. Verified: bare arm at ll_parser.rs:11058, catch-all at :11197; `parse_catchswitch` itself exists at :13969.
- **Why:** Recorded in docs/future-work.md as a W9c P0: found while testing `expected scope value for catchswitch` (which the bare form reaches) and recorded rather than rushed into the wave.
- **Correction from verification:** The claim is accurate in substance and severity. Two refinements: (1) Cited line numbers have drifted by 2 (the file is modified per git status). Current positions: bare-form terminator arm at ll_parser.rs:11060, catch-all `_ =>` at :11199, `parse_catchswitch` doc comment at :13969 / `fn` at :13974.
- **Fix:** Add the `Opcode::CatchSwitch` arm to the named-result dispatch, calling the existing `parse_catchswitch` and binding the result through `state.bind_local` exactly as the bare arm does. The parse routine already exists; this is a dispatch-table hole.

### 14. Metadata-typed operand-bundle inputs do not parse

*parser — call family* — crates/llvmkit-asmparser/src/ll_parser.rs:10278-10280

- **LLVM:** `LLParser::parseOptionalOperandBundles` reads each input as TYPE VALUE and branches on the type: a `metadata`-typed input goes through `parseMetadataAsValue`, not `parseValue`.
- **llvmkit:** The bundle-input loop reads `parse_type` then `parse_value` unconditionally, with no metadata arm, so `[ "tag"(metadata !0) ]` is rejected.
- **Why:** Recorded in the handoff as carried out of W9 ("Metadata-typed operand-bundle inputs (upstream routes them through `parseMetadataAsValue`)"). Same shape as the W11 P0 in `parseParameterList`, which was fixed; the bundle site was not.
- **Correction from verification:** The divergence is real but the claim's description and example are wrong. The structural part is accurate: crates/llvmkit-asmparser/src/ll_parser.rs:10277-10284 (parse_optional_operand_bundles) reads `parse_type` then `parse_value` unconditionally, with no `is_metadata()` arm — unlike the parameter-list loops at lines 12979, 14067, 14205, …
- **Fix:** Branch on `ty.is_metadata()` before `parse_value` and route to the same `parse_metadata_as_value` path W11 added for `metadata i32 %a` parameters.

### 15. A forward-referenced function is a typed `Function`, so a later definition cannot change its signature

*parser — forward references* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_direct_callee`, `parse_declare`/`parse_define` reuse path); crates/llvmkit-ir/src/module.rs (`add_function_dyn`)

- **LLVM:** `LLParser::getGlobalVal` mints an *untyped* placeholder (a `ptr`-typed `GlobalVariable`), so `parseFunctionHeader` compares only `FwdFn->getType() != PFT` — after opaque pointers, nothing but the address space. A call whose arguments disagree with the eventual definition is accepted by the parser and rejected by the Verifier (`Call parameter type does not match function signature!`).
- **llvmkit:** `parse_direct_callee`'s forward-reference arm calls `Module::add_function_dyn` with the *call site's* signature, so the placeholder is a real `Function` with a real `FunctionType`. `declare`/`define` then reject a signature mismatch with two invented texts — `forward function declaration with matching signature` / `forward function definition with matching signature` — neither text nor rule upstream's.
- **Why:** Recorded in docs/future-work.md (W8): the check is load-bearing for the representation, not just the diagnostic — dropping it would leave a call wired to a function whose type it does not match. It also blocks the three per-site forward-reference texts W2.5 carried.
- **Correction from verification:** Substantively accurate; two citation refinements. (1) The symbol is `resolve_direct_callee` (the token peek lives in `parse_direct_callee_ref`), not `parse_direct_callee`. (2) llvmkit is not missing untyped-placeholder machinery in general — `global_forward_ref` / `forward_ref_value_placeholder` / `resolve_global_forward_ref` (ll_parser.r …
- **Fix:** Apply W2's value-forward-reference shape (untyped placeholder + RAUW at definition) to the *callee* position so `parseFunctionHeader` can create a fresh `Function` and re-point the call. That unblocks `invalid forward reference to function '<n>' with wrong type: expected 'T' but was 'U'` (fixture `opaque-ptr-invalid-forward-ref.ll` is vendored and waiting), `type of definition and forward reference of '@N' disagree`, …

### 16. A self-typed aliasee does not parse; `invalid aliasee` is unreachable

*parser — constants / aliases* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_alias_or_ifunc`, `parse_constant_expr`)

- **LLVM:** `LLParser::parseAliasOrIFunc` branches on the aliasee's first token: `bitcast`, `getelementptr`, `addrspacecast` and `inttoptr` go through a bare `parseValID` ("the bitcast dest type is not present, it is implied by the dest type"); everything else goes through `parseGlobalTypeAndValue`. A non-`t_Constant` result is `invalid aliasee`, and the pointer check plus the address space are taken from the aliasee *value's* type.
- **llvmkit:** `@a = alias i32, bitcast (ptr @g to ptr)` does not parse, nor do the `getelementptr`/`addrspacecast`/`inttoptr` spellings. `invalid aliasee` is therefore unreachable, and the alias address space is not derived from the aliasee.
- **Why:** Recorded in docs/future-work.md (W7): attempted and reverted. The blocker is that `Parser::parse_constant_expr` takes a `result_ty` and llvmkit has no entry point for a constant expression that types itself — every constexpr arm is reached with the demanded type already in hand.
- **Correction from verification:** Still present, but the last clause is wrong. Accurate statement: llvmkit's `parse_alias_or_ifunc` unconditionally calls `parse_type(false)` for the aliasee instead of branching on the first token, so the four bare-`parseValID` aliasee spellings upstream accepts — `bitcast (ptr @g to ptr)`, `getelementptr (...)`, `addrspacecast (...)`, `in …
- **Fix:** Give `parse_constant_expr` a self-typing entry point (the cast/GEP arms deriving their result type from the written destination type), then add the four-keyword branch in `parse_alias_or_ifunc`, the `t_Constant` check with `invalid aliasee`, the value-typed pointer check, and the aliasee-derived address space. Expect the refactor to surface value bugs, not just missing diagnostics — that is what W4's stages A–C did.

### 17. GEP vector operands are not expressible in the IR model

*IR model / builder* — crates/llvmkit-ir/src/ir_builder.rs:5634 (`gep`), :5779 (`gep_with_flags`), :5797 (`gep_inner`); crates/llvmkit-asmparser/src/ll_parser.rs (`parse_getelementptr`)

- **LLVM:** `LLParser::parseGetElementPtr` accepts a vector-of-pointers base and vector indices, producing a vector-of-pointers result (`getelementptr_vec_*.ll` ×6 pin it).
- **llvmkit:** `IrBuilder::gep`/`gep_with_flags` take `P: IntoPointerValue` and `V: IntoIntValue<IntDyn>` and return `PointerValueId`, so neither a vector base nor a `<4 x i32>` index can be built. W9a got the *diagnostics* right for five GEP fixtures by moving the builder conversion after upstream's rules, but valid vector GEPs still do not parse.
- **Why:** Recorded in docs/future-work.md as a known IR gap and moved from W1 to W9a with the reason: "Not a parser-only fix". The builder surface has to grow the vector forms first.
- **Correction from verification:** Still present, but the title overstates the scope. Two refinements: (1) Scope: it is the GEP *instruction* path (builder + `parse_gep`) that cannot express vector operands, not "the IR model".
- **Fix:** Widen the GEP builder surface to vector-of-pointer bases and vector indices (an `_erased`/`_dyn` tier per the suffix vocabulary), then let the parser build rather than diagnose-and-stop. Upstream coverage: `getelementptr_vec_*.ll` ×6.

### 18. `parse_uint64` / `parse_uint32` are narrower than `parseUInt64`

*parser — literals* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_uint64`, `parse_uint32`)

- **LLVM:** `LLParser::parseUInt64` accepts any `lltok::APSInt` whose value is unsigned and takes `APSInt::getLimitedValue()`, which **saturates** at `UINT64_MAX`.
- **llvmkit:** `parse_uint64` accepts only a positive **decimal** literal and fails outright when the digits do not fit. So `u0x10` answers `expected integer` where upstream accepts it, and an over-wide literal is rejected where upstream saturates. `parse_uint32` has the same shape (its saturation is unobservable — the range check rejects either way, which is what `align-param-attr-error2.ll` pins).
- **Why:** Recorded in docs/future-work.md (W10): it is a W5-owned routine with 25 call sites, no `test/Assembler` fixture reaches either case, and the honest fix reads the token through `parse_int_literal` (the APSInt token model), which changes where the diagnostic's span comes from — so it was not smuggled into the summary-index wave.
- **Correction from verification:** Substantively accurate and still present; only the supporting citation is wrong. Corrected statement: `LLParser::parseUInt64` accepts any `lltok::APSInt` whose APSInt is unsigned and reads it with the saturating `APSInt::getLimitedValue()`;
- **Fix:** Route both through `parse_int_literal`/`ParsedApsInt`, accept any unsigned APSInt spelling, and saturate rather than fail on over-wide values; re-check the span of every one of the 25 call sites' diagnostics after the change.

### 19. AutoUpgrade does not exist — legacy-but-valid modules are not upgraded

*IR / parser — end of module* — crates/llvmkit-ir/src/ (new `auto_upgrade.rs`, absent today), crates/llvmkit-asmparser/src/ll_parser.rs (end-of-module call sites)

- **LLVM:** `llvm/lib/IR/AutoUpgrade.cpp` is called from the parser's end-of-module path at eight sites: `UpgradeIntrinsicFunction`/`UpgradeIntrinsicCall`/`UpgradeCallsToIntrinsic` (generic arms include the `llvm.lifetime.start/end` size-argument drop, dbg intrinsics → `#dbg_*` records, `llvm.experimental.vector.*` → `llvm.vector.*`), `UpgradeDebugInfo`, `UpgradeTBAANode`, `UpgradeModuleFlags`, `UpgradeNVVMAnnotations`, `UpgradeSectionAttributes`, `copyModuleAttrToFunctions`.
- **llvmkit:** There is no `auto_upgrade.rs` in `crates/llvmkit-ir/src/` and no upgrade call sites, so legacy spellings that upstream silently modernises are either rejected or preserved unchanged. The lifetime size-argument form is noted as hit by "every clang-21 module".
- **Why:** Recorded as a resolved decision (2026-08-07, revised): **staged** — the generic parser-reachable upgrades port in-program at W13; the target-specific intrinsic rewrite bodies (the bulk of AutoUpgrade.cpp's 6,646 lines) plus `UpgradeARCRuntime`, `UpgradeBitCastInst/Expr`, `UpgradeInlineAsmString`, `UpgradeDataLayoutString` become a named R …
- **Correction from verification:** Substantively accurate; one count correction and one strengthening. (1) Count: LLParser::validateEndOfModule contains NINE AutoUpgrade call sites, not eight — the claim's own list names nine symbols (UpgradeIntrinsicFunction, UpgradeIntrinsicCall, UpgradeTBAANode, UpgradeCallsToIntrinsic, llvm::UpgradeDebugInfo, UpgradeModuleFlags, Upgrad …
- **Fix:** Create `auto_upgrade.rs` under upstream's layering (lib/IR), extract `upgradeIntrinsicFunction1`'s non-target arms mechanically and diff back, wire the eight call sites, and port the generic `autoupgrade-*.ll` fixture families with UPSTREAM.md rows.

### 20. The function-header attribute list is gated behind a hand-maintained lookahead

*parser — attributes* — crates/llvmkit-asmparser/src/ll_parser.rs (`keyword_starts_attribute`, `parse_optional_function_suffix`)

- **LLVM:** `LLParser::parseFunctionHeader` enters `parseFnAttributeValuePairs` unconditionally and lets `tokenToAttribute`'s fall-through end the loop. There is no lookahead.
- **llvmkit:** `Parser::keyword_starts_attribute` is a second copy of the loop's arm list. It was already wrong once: `uwtable`, `allocsize`, `vscale_range`, `allockind`, `nofpclass`, `dereferenceable`, `captures`, `range`, `initializes` and the six type attributes were all missing, so `define void @f() uwtable {` — plain clang output — was never even attempted and failed as `expected '{' to open function body`.
- **Why:** Recorded in docs/future-work.md (W5): the structural fix is to delete the lookahead, but that needs `parse_optional_function_suffix`'s `align`-is-not-an-attribute carve-out to survive, which was why W5 stopped at re-syncing the table.
- **Correction from verification:** Still present as a structural divergence, but the concrete failure the claim cites is fixed and the claim's "Where" is slightly off. Accurate today: llvmkit gates the function-header attribute list behind `Parser::is_attr_start` / `Parser::keyword_starts_attribute` (ll_parser.rs:9274-9310), a hand-maintained second copy of `parse_fn_attri …
- **Fix:** Enter the attribute loop unconditionally and let its `_ => break` arm end it, preserving the `align` carve-out (upstream keeps `align` out of the group and moves it to the field). Delete `keyword_starts_attribute`.

## Accepts invalid input

llvmkit accepts IR that LLVM rejects, so a malformed module survives into the rest of the pipeline.

### 21. An inline-asm call's per-operand `elementtype` rules are not verified

*verifier* — crates/llvmkit-ir/src/verifier.rs:2775-2779 (call arm), :3230-3240 (callbr arm); attributes reach the instruction via crates/llvmkit-asmparser/src/ll_parser.rs:12972 and …

- **LLVM:** `Verifier::verifyInlineAsmCall` walks `IA->ParseConstraints()` and, for each constraint with an argument, checks three things: an indirect constraint's operand has pointer type (`Operand for indirect constraint must have pointer type`), an indirect constraint's operand carries an `elementtype` attribute (`Operand for indirect constraint must have elementtype attribute`), and a non-indirect constraint's operand does **not** (`Elementtype attribute can only be applied for indirect constraints`).
- **llvmkit:** llvmkit's verifier implements only the label half of the same routine — `label_constraint_count() != 0` for `call`, `!= indirect_dests.len()` for `callbr`. The three `elementtype` checks are absent, so `call void asm sideeffect "", "=*m"(ptr %p)` with no `elementtype` (and the inverse, `elementtype` on a direct constraint) verifies clean here and is rejected by upstream.
- **Why:** Recorded reason: "the call surface cannot spell per-operand `elementtype` attributes yet". **That premise is stale** — the parser reads per-argument attribute lists into `CallAttributeData::arg_attrs`, `Keyword::Elementtype` is one of the type attributes it accepts, and the AsmWriter prints them back. Nothing blocks the check today.
- **Fix:** In both verifier arms, iterate `InlineAsm::parse_constraints()` alongside `attrs.arg_attrs()` exactly as upstream walks `ArgNo`, skipping label and no-argument constraints, and emit the three messages as new `VerifierRule` variants. Port the fixtures from `test/Verifier/inline-asm-*.ll` in the same commit.

### 22. A non-uniform scalable-vector constant is constructible, parses, and prints text LLVM rejects

*IR model* — crates/llvmkit-ir/src/constants.rs:957-983 (`const_vector`), crates/llvmkit-ir/src/asm_writer.rs:1272-1278 (`prints_as_splat`) and :1301 (`fmt_aggregate_constant`)

- **LLVM:** `ConstantVector::get` takes a fixed element count, so LLVM has no element-list constant form for a scalable vector at all; a scalable constant can only be a splat, and `AsmWriter`'s `splat (…)` shorthand is the only spelling. There is consequently no upstream rule against a non-uniform scalable vector — the constant cannot be built.
- **llvmkit:** `VectorType::const_vector` skips its element-count check for scalable types (`if !self.is_scalable() && n != expected`) and requires nothing of the lanes, so `@g = global <vscale x 4 x i32> <i32 7, i32 8, i32 7, i32 7>` both builds and parses. `prints_as_splat` collapses a *uniform* scalable vector to `splat (…)`, but a non-uniform one falls through `fmt_aggregate_constant` to the element-list fallback and prints text LLVM would reject — output that also quietly asserts `vscale == 1`.
- **Why:** Recorded, and explicitly left as a policy decision rather than an oversight: requiring uniformity was tried and reverted because two deliberate tests take the permissive behaviour as their *premise* — `scalable_i1_non_splat_divrem_does_not_use_scalar_i1_shortcuts` and `scalable_vector_fsub_negative_zero_pattern_controls_undef_fold` in `cr …
- **Correction from verification:** The divergence is REAL and still present, but the claim's title and body are wrong on one of the three verbs: it does NOT parse. Corrected statement: "A non-uniform scalable-vector constant is constructible through the IR builder API and prints text LLVM rejects — text that llvmkit's own parser also rejects, so the output does not round-t …
- **Fix:** Decide the representation policy first. If uniformity becomes required, add the lane-agreement check to `const_vector` (and the parser's aggregate path), and rewrite the two `constant_fold.rs` tests whose premise it removes — they become unnecessary rather than merely red.

### 23. `swifterror` use-site dataflow rules are not enforced

*verifier* — crates/llvmkit-ir/src/verifier.rs:2402-2435 (the alloca-level checks; no use-site walk exists)

- **LLVM:** `Verifier::visitAllocaInst` and the `swifterror` use checks in `Verifier` restrict a swifterror value's flow: it may appear only in specific positions (a `swifterror` call argument, a `load`/`store` of the pointer itself, and so on), and any other use is an error.
- **llvmkit:** llvmkit verifies the parse-level constraints only — a swifterror alloca must have pointer type and must not be an array allocation. A swifterror value used in a position upstream rejects verifies clean here.
- **Why:** Recorded under the 2026-07-06 upstream-parity follow-ups as deliberately deferred; no reason beyond scope is given for the deferral.
- **Correction from verification:** Substantively accurate; two refinements. (1) The two checks llvmkit does have are verifier-level, not "parse-level" — they live in `Verifier::check_alloca` (crates/llvmkit-ir/src/verifier.rs:2402-2434), a faithful mirror of the first two `Check`s in upstream `visitAllocaInst`.
- **Fix:** Add a use-site walk keyed on the swifterror alloca/argument: enumerate the legal positions upstream allows, reject everything else as new `VerifierRule` variants, and port the fixtures from `test/Verifier/swifterror*.ll` with their `UPSTREAM.md` rows.

### 24. `AttributeStorage::add` permits two attributes of the same kind at one index

*IR model / Attributes* — crates/llvmkit-ir/src/attributes.rs:1698 (`add`), crates/llvmkit-ir/src/attributes.rs:1721 (`set`), recorded reason at crates/llvmkit-ir/src/attributes.rs:1713-1720

- **LLVM:** `AttrBuilder::add*` all route through `addAttributeImpl`, whose `std::swap(*It, Attr)` branch *replaces* an existing attribute of the same kind (or, for a string attribute, the same key). An `AttrBuilder` can never hold two attributes of one kind, so `align(4)` followed by `align(8)` leaves only `align(8)`.
- **llvmkit:** `AttributeStorage::add` de-duplicates only by full structural equality (`if !set.contains(&stored) { set.push(stored) }`), so `align(4)` and `align(8)` coexist at the same `AttrIndex`. The replacing semantics exist only on the separate `AttributeStorage::set`, which is not what the builder surface calls.
- **Why:** Recorded at `attributes.rs:1719`: "[`Self::add`] keeps its weaker structural de-duplication; the difference is observable and recorded in `docs/future-work.md`." No reason is given for keeping the weaker form beyond it predating `set`.
- **Fix:** Make `add` an alias of `set` (i.e. give `add_stored` the same-kind replacement scan `set` already has) and delete the structural-equality path; then audit callers that relied on accumulation — string attributes with distinct keys are unaffected because `set` keys on the string key. Guard with a test that adding `align(4)` then `align(8)` leaves exactly one `Alignment` attribute, mirroring `AttrBuilderTest`.

### 25. xfail-parse conflates upstream-negative with unsupported, leaving CHECK lines unpinned

*parser corpus driver* — crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt:8-14; crates/llvmkit-asmparser/tests/parser_corpus.rs:44-52,66-90;

- **LLVM:** `test/Assembler/2004-11-28-InvalidTypeCrash.ll` is a negative fixture whose CHECK is `invalid type for null constant`; `test/Bitcode/blockaddress-addrspace.ll::return-self-bad.ll`'s CHECK is `constant expression type mismatch`.
- **llvmkit:** Both sit under `# Upstream fixtures checked in now, but expected-failing` with `status=xfail-parse`, and the driver only asserts that parsing fails — never with which message. So neither CHECK line is pinned, and a *wrong* rejection would pass. `2004-11-28-InvalidTypeCrash.ll` carries no stated reason at all;
- **Why:** Partly recorded and now stale. The driver doc admits the conflation: "Manifest `status=xfail-*` entries are the explicit parser corpus allowlist for upstream-negative or not-yet-supported shapes" — one status for two very different situations.
- **Fix:** Split the status into `negative=<expected message>` (assert the fixture's CHECK line verbatim) and `xfail-parse` (genuinely unsupported, with a required reason field). Move both entries to `negative=` with their CHECK text, and delete the stale W4 comment.

## Different diagnostic text

Same verdict, different wording. Upstream's text is contractual, including its own inconsistencies.

### 26. A misplaced `phi` is rejected by the parser, with a message upstream never prints

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:11125-11133 (the `seen_non_phi` guard)

- **LLVM:** `LLParser` accepts a `phi` written after a non-phi instruction and lets `Verifier::visitPHINode` reject it with `PHI nodes not grouped at top of basic block!`.
- **llvmkit:** `parse_basic_block` tracks a `seen_non_phi` flag and rejects at parse time with `phi must be grouped at the top of its basic block`. Same verdict, wrong layer, and a message upstream never prints.
- **Why:** Recorded, with an explicit "do not fix this by deleting the parser check": every phi llvmkit builds goes through `IrBuilder::make_phi_in_block` → `BasicBlock::insert_instruction_at_phi_head`, which places the phi at the block's phi head regardless of the insertion point.
- **Correction from verification:** Accurate, with two refinements. (1) The guard now spans lines 11125-11134 (the claim cited 11125-11133; the `else { seen_non_phi = true; }` arm closes at 11134). (2) The message is emitted via `self.expected(...)`, whose ParseError variant renders as `#[error("expected {expected}")]`, so the actual user-visible string is the ungrammatical …
- **Fix:** Add a non-hoisting insertion path for parsed phis so the instruction lands where it was written, then delete the parse-time check and let `VerifierRule::PhiNotAtTop` deliver upstream's verdict and wording. That is entangled with llvmkit's head-phi design — block parameters are operandless head-phis per `IrBuilder::append_block_with_params`, and `insert_instruction_at_phi_head` is the only phi insertion path today — s …

### 27. An indirect `callbr` is rejected at parse time rather than by the verifier

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:14295-14301

- **LLVM:** `LLParser::parseCallBr` accepts an indirect callee; `Verifier::visitCallBrInst` is what requires a direct callee for a non-inline-asm callbr (`Callbr: indirect function / invalid signature`).
- **llvmkit:** `parse_callbr` rejects a non-inline-asm callbr whose callee is not a direct function with `expected direct function callee for callbr`. The verdict matches upstream's — the IR is invalid either way — but the layer and the wording do not.
- **Why:** Recorded under the 2026-07-06 upstream-parity follow-ups, which state the trade explicitly: llvmkit reaches the same verdict, and a stricter port would accept it at parse and reject it in the verifier. Noted alongside indirect *invoke*, which is valid IR and is now supported.
- **Fix:** Give the callbr builder an indirect-callee form (the indirect-invoke work is the template), let the parse succeed, and move the rejection into the verifier as a `VerifierRule` carrying upstream's wording — the callbr arm of the verifier already exists at verifier.rs:3230 for the label-count check.

### 28. Unknown debug-record type rejected by the lexer, not by parseDebugRecord

*lexer / parser (LLParser parity W14)* — crates/llvmkit-asmparser/tests/parser_debug_metadata.rs:807-827; UPSTREAM.md:2149

- **LLVM:** `test/Assembler/dbg-record-invalid-4.ll` CHECKs `expected debug record type here`. `LLLexer` hands `parseDebugRecord` a token it does not recognise (a silent `lltok::Error`), and `parseDebugRecord`'s opening check produces that message — the one lowercase label in an otherwise capital-`E` routine.
- **llvmkit:** The test asserts `"unknown keyword 'dbg_invalid'"`. llvmkit's lexer rejects the unknown keyword itself, so the parser never sees the token and the upstream message is unreachable.
- **Why:** Recorded, in the doc comment and in the `UPSTREAM.md` row: "Closing the gap is the W14 lexer re-layering (a `Token::Error` variant so an unknown word can reach the parser)". Deliberately pinned as-is so the test fails the moment the layering changes.
- **Correction from verification:** Substantially accurate and still present, with one wording fix. Real and unchanged: upstream's LLLexer::LexIdentifier falls through to a silent `lltok::Error` for an unrecognized word, LLLexer::LexHash returns bare `lltok::hash`, and LLParser::parseDebugRecord's opening check answers `expected debug record type here` (the one lowercase la …
- **Fix:** W14: add a `Token::Error` variant to `ll_lexer` so an unrecognised word becomes a silent error token instead of a lexer diagnostic, and move the rejection into each `parse*` routine. Then `an_unknown_debug_record_type_is_rejected` asserts `expected debug record type here`. This one change also unblocks the four items below.

### 29. Three of eight memory-attribute-errors.ll splits unported

*parser (attributes) / lexer* — crates/llvmkit-asmparser/tests/parser_modifiers.rs:257-294

- **LLVM:** `test/Assembler/memory-attribute-errors.ll` has eight splits; `memory(foo)`, `memory(other: read)` and `memory(argmem: foo)` each turn on a word matching no keyword, and upstream's `LLParser::parseMemoryAttr` reports the location/access-kind diagnostic.
- **llvmkit:** Only five splits are ported. The other three cannot be reached: llvmkit's lexer raises `unknown keyword '...'` before `parseMemoryAttr` runs. Doc: "Same rejection, wrong layer and wrong text".
- **Why:** Recorded — named as "the lexer-parity item recorded for the end of the parity program" (W14).
- **Fix:** Same `Token::Error` re-layering as above; then vendor the three remaining splits under `tests/fixtures/upstream/memory-attribute-errors/` and add them to the existing table in `memory_attribute_errors_match_upstream_text`.

### 30. invalid-inline-constraint.ll unported behind the same lexer blocker

*parser (calls / inline asm)* — crates/llvmkit-asmparser/tests/parser_calls.rs:72-78

- **LLVM:** `test/Assembler/invalid-inline-constraint.ll` pins `failed to parse constraints` from `LLParser::convertValIDToValue`; its body is deliberately corrupted past the call and upstream's lexer returns a silent `lltok::Error` there.
- **llvmkit:** Not ported. llvmkit's lexer raises `unknown keyword 'ounwi'` before the parser can report. The nine splits of the sibling `inline-asm-constraint-error.ll` are ported and do pass.
- **Why:** Recorded: "Same blocker as three splits of `memory-attribute-errors.ll`; see the lexer-parity item recorded for the end of the parity program."
- **Fix:** W14 `Token::Error`; then add the fixture as a tenth entry in `inline_asm_constraint_errors_match_upstream_text`'s `SPLITS` table.

### 31. captures(bogus) diagnostic unreachable

*parser (attributes)* — crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:496-507

- **LLVM:** `LLParser::parseCapturesAttr` answers `expected one of 'none', 'address', 'address_is_null', 'provenance' or 'read_provenance'` for any unrecognised component word, including `captures(bogus)`.
- **llvmkit:** `captures(bogus)` never reaches the arm — the lexer answers `unknown keyword 'bogus'`. The test substitutes `captures()`, which reaches the arm only because `)` is a token the parser sees.
- **Why:** Recorded inline: "Blocked on the same lexer re-layering as three splits of `memory-attribute-errors.ll`."
- **Fix:** W14 `Token::Error`; then add `parse_err("captures(bogus)")` alongside the existing `captures()` assertion.

### 32. Attribute-group reference error is fatal here, non-fatal upstream

*parser (attribute groups)* — crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:899-935

- **LLVM:** `LLParser::parseUnnamedAttrGrp` reports `cannot have an attribute group reference in an attribute group` non-fatally — it keeps parsing and accumulates further diagnostics.
- **llvmkit:** llvmkit reports the first and stops. The message text matches; the recovery does not. The same doc also notes that `unterminated attribute group`'s upstream trigger (a misspelled keyword) is intercepted by llvmkit's lexer, so the test substitutes a type keyword, an integer, and end-of-file.
- **Why:** Recorded — "the same choice already recorded for the position diagnostics", i.e. llvmkit's parser is fail-fast by design where upstream accumulates.
- **Correction from verification:** The divergence is real and still present, but one phrase is imprecise: upstream does NOT "accumulate further diagnostics" in the sense of retaining a list. `LLLexer::ErrorInfo` (LLLexer.h) holds exactly one `SMDiagnostic &Error` plus an `ErrorPriority`, and `LLLexer::Error` is `if (Priority < ErrorInfo.Priority) return;
- **Fix:** Either accept it as a documented global policy (then say so once, in `AGENTS.md`, rather than per-test) or give `ParseError` an accumulating mode so multi-diagnostic fixtures can be ported whole. The lexer half rides on the W14 `Token::Error` change.

### 33. Lexer reports unknown keywords itself instead of returning an error token

*lexer* — crates/llvmkit-asmparser/src/ll_lexer.rs, crates/llvmkit-asmparser/src/ll_token.rs:22, crates/llvmkit-asmparser/src/ll_lexer/keywords.rs, crates/llvmkit-asmparser/src/ll_ …

- **LLVM:** `LLLexer::LexIdentifier` (and its siblings) return a silent `lltok::Error` token on an unrecognised word; the *parser* then emits the message for the construct it was reading — `LLParser::parseMemoryAttr`, `parseOptionalCaptures`, `parseFnAttributeValuePairs`, `parseDIExpressionBody`, the debug-record parser, and every `parseMDField` kind overload.
- **llvmkit:** `ll_lexer.rs` returns `Err(LexError)` (e.g. `unknown keyword 'x'`) which propagates out immediately; `Token` has no `Error` variant, so the parser can never say what it wanted. Verified: `enum Token` at ll_token.rs:22 has no `Error` arm.
- **Why:** Recorded: this is a structural re-layering, not a wording fix, and it was deferred to W14 from W0 onwards. The handoff calls it "the single highest-leverage item left — roughly a third of the remaining ledger gap".
- **Correction from verification:** Substantively accurate; two details need refinement. (1) The upstream captures parser is `LLParser::parseCapturesAttr` (LLParser.cpp:3240), not `parseOptionalCaptures`. (2) "the parser can never say what it wanted" is true as the architecture but has two hand-rolled exceptions where llvmkit re-maps a `ParseError::Lex` into an `Expected`: …
- **Fix:** Add `Token::Error` (carrying the offending span/text), make the lexer emit it instead of `Err`, and let each parser routine's existing `_ => break`/default arm produce upstream's message. Then finish `parser_modifiers.rs::memory_attribute_errors_match_upstream_text` and port the queued fixtures: three `memory-attribute-errors.ll` splits (`memory(foo)`, `memory(other: read)`, `memory(argmem: foo)`), `invalid-inline-co …

### 34. Exact-word metadata kind families are rejected one layer too early (plan divergence #2)

*lexer / metadata parser* — crates/llvmkit-asmparser/src/ll_lexer/keywords.rs, crates/llvmkit-asmparser/src/ll_parser.rs (`parse_metadata_field_value`)

- **LLVM:** The `LLParser::parseMDField` overloads for the emission-kind, name-table-kind and fixed-point-kind fields receive an ordinary identifier/error token and reject it themselves with `expected emission kind` / `expected nameTable kind` / `expected fixed-point kind`.
- **llvmkit:** The lexer hard-rejects any spelling outside its keyword table in those positions, so llvmkit answers with its own `unknown keyword '...'` and upstream's three messages are unreachable.
- **Why:** Recorded: W11 states plainly that "Bullet 2 (divergence #2, the exact-word kind families) stays blocked" behind the `Token::Error` re-layering. Same root cause as the entry above, but it is tracked as its own named divergence with its own message set.
- **Correction from verification:** REAL and still present, but the description is imprecise in three ways and understates the impact in a fourth. Corrected statement: (1) Mechanism. Both lexers carry the *same* word lists — upstream `LLLexer::LexIdentifier` matches `NoDebug|FullDebug|LineTablesOnly|DebugDirectivesOnly`, `GNU|Apple|None|Default`, `Binary|Decimal|Rational` e …
- **Fix:** Falls out of `Token::Error`: pass the unknown spelling through as a plain identifier/error token and let the three field parsers reject it with upstream's text. Applies to any other exact-word family the lexer currently owns.

### 35. A misplaced `phi` is rejected at parse time, not by the verifier

*parser / IR insertion model* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_basic_block`); crates/llvmkit-ir/src/basic_block.rs (`insert_instruction_at_phi_head`);

- **LLVM:** `LLParser` accepts a `phi` written after a non-phi instruction and lets `Verifier::visitPHINode` reject it with `PHI nodes not grouped at top of basic block!`.
- **llvmkit:** `ll_parser.rs::parse_basic_block` rejects it at parse time with `phi must be grouped at the top of its basic block` — a message upstream never prints. Same verdict, wrong layer.
- **Why:** Recorded in docs/future-work.md, and explicitly corrected in the plan (W1 item, `- [x]` with "CORRECTED 2026-08-08 — do not remove"): every phi goes through `IrBuilder::make_phi_in_block` → `BasicBlock::insert_instruction_at_phi_head`, so deleting the parse check makes llvmkit *silently hoist* the phi and its own `VerifierRule::PhiNotAtTo …
- **Correction from verification:** Accurate as written; two refinements. (1) The claim is fully confirmed: ll_parser.rs::parse_basic_block (lines 11125-11134) tracks a `seen_non_phi` flag and returns `phi must be grouped at the top of its basic block` when a phi follows a non-phi, while upstream LLParser::parseBasicBlock (LLParser.cpp) inserts every instruction with `Inst- …
- **Fix:** Add a non-hoisting insertion path for parsed phis so the instruction lands where it was written, then delete the parse-time check and let `VerifierRule::PhiNotAtTop` fire. Entangled with the head-phi/block-parameter model (block parameters are operandless head-phis), so it wants deciding alongside that model rather than as a parser patch.

### 36. Global forward references resolve in one end-of-module sweep, not per definition site

*parser — forward references* — crates/llvmkit-asmparser/src/ll_parser.rs:6991-6999 (`forward_ref_globals` guard), :1711-1731 (end-of-module leftovers)

- **LLVM:** `LLParser::getGlobalVal` plus the definition sites compare types where the definition is written, producing `forward reference and definition of global have different types` (at the type location), the alias twin, and `type of definition and forward reference of '@N' disagree`.
- **llvmkit:** A reference to an unknown `@name`/`@N` mints a `ForwardRefValue` at the demanded pointer type and one end-of-module sweep retires them. Same verdicts overall, but upstream's per-site texts are unreachable.
- **Why:** Recorded as a W2.5 correction and carried: "resolution is a single end-of-module sweep, not per-definition-site — same verdicts, but upstream's per-site texts stay unreachable". Closing it shares a root cause with the typed-forward-referenced-function item.
- **Correction from verification:** Substantially accurate and still present, with one wording correction. The sweep design is exactly as described: `global_forward_ref` (ll_parser.rs:1765) mints a placeholder at the demanded pointer type, and `resolve_forward_ref_globals` (:1714), called once at :1475, retires every entry;
- **Fix:** Move resolution to the definition sites (global/declare/define/alias), comparing the placeholder's demanded type against the definition's at the type location — which also needs the untyped-callee placeholder above for the function twins.

### 37. `intrinsic can only be used as callee` fires at reference time, not at end of module

*parser — intrinsics* — crates/llvmkit-asmparser/src/ll_parser.rs:8158, :8209

- **LLVM:** Upstream auto-declares `llvm.`-prefixed leftovers from call-site function types in `validateEndOfModule`, and the `intrinsic can only be used as callee` rejection happens there, in that ordered sequence.
- **llvmkit:** The message is emitted at the point of reference (two sites), so a construct upstream would only reject at end of module is rejected earlier and the end-of-module error ordering differs.
- **Why:** Recorded as a W2 carried item ("`intrinsic can only be used as callee` still fires at reference time"). Error *ordering* in `validateEndOfModule` is itself part of parity, which is why it routes to W13.
- **Correction from verification:** Accurate, and understated. Two corrections/refinements: (1) The ordering point is right but the mechanism is stronger than "rejected earlier": llvmkit has NO end-of-module `llvm.*` handling at all.
- **Fix:** Fold into W13's `validateEndOfModule` 1:1 sequence: defer the check to the intrinsic auto-declaration step so it fires in upstream's order relative to blockaddress leftovers, dso_local_equivalent resolution, undefined types/comdats and `@` leftovers.

### 38. `validateEndOfModule` is not a 1:1 port, and its error order is not pinned

*parser — end of module* — crates/llvmkit-asmparser/src/ll_parser.rs:1711 (`validate_end_of_module` region), :4786-4798 (comdat guard), :4756 (undefined types)

- **LLVM:** `LLParser::validateEndOfModule` runs a fixed sequence: attribute-group merge + alignment-attr→field move, blockaddress leftovers, `dso_local_equivalent` resolution, undefined numbered/named types, undefined comdats, intrinsic auto-declaration and `@` leftovers, undefined metadata, metadata cycle resolution, the TBAA hook, then SlotMapping steal semantics. Which error fires first is itself observable.
- **llvmkit:** The pieces exist but were landed wave by wave (W2.5 did intrinsic auto-declaration and `@` leftovers; W3 the undefined types; W2.6 comdats), and the sequence has never been verified against upstream's. The attribute-group merge and the alignment-attr→field move do not exist at all.
- **Why:** Recorded as W13's opening item, with the ordering explicitly called "part of parity". Its group-merge half is the blocker under the printer's missing attribute-group forming.
- **Correction from verification:** Substantially accurate, with one sub-clause corrected and one under-statement. CORRECTION: "The attribute-group merge and the alignment-attr->field move do not exist at all" is half wrong.
- **Fix:** Port the routine as one ordered sequence, add the attr-group merge + `align`-to-field move, and pin the order with negative fixtures that trip two rules at once. Also covers `getIntrinsicSignature` mangling-suffix cases (`llvm.umax` on `i32` declares `llvm.umax.i32`) and the `InstsWithTBAATag` hook W11 was to leave behind.

### 39. `expected comdat type` is unimplemented pending upstream's error-priority question

*parser — comdats* — crates/llvmkit-asmparser/src/ll_parser.rs:4800-4823

- **LLVM:** `LLParser::parseComdat` calls `parseToken(lltok::kw_comdat, "expected comdat keyword")` *and then* `tokError("expected comdat type")` on the same failure, so which message a user sees depends on how `LLLexer`'s recorded-error priority ranks them.
- **llvmkit:** `parse_comdat_definition` expects the keyword with its own label and falls through to `unknown selection kind`; `expected comdat type` exists nowhere in the crate (verified by grep).
- **Why:** Recorded as deferred in W6 part 2 with the reason stated: the `LLLexer::ErrorPriority` question has to be answered before the message can be placed correctly.
- **Correction from verification:** The divergence is REAL and still present, but the claim's framing is wrong in two places. CORRECTED STATEMENT: Upstream `LLParser::parseComdat` (LLParser.cpp:889-890) is: if (parseToken(lltok::kw_comdat, "expected comdat keyword")) return tokError("expected comdat type");
- **Fix:** Read `LLLexer`'s error-priority handling in the `.cpp`, decide which of the two messages upstream actually surfaces, and place it accordingly. Worth checking the adjacent labels in the same routine at the same time (`'=' after comdat name`, `'comdat'`), which do not obviously match upstream's `expected '=' here` / `expected comdat keyword`.

## Different printed bytes

The parser/printer contract is that printed output matches `AsmWriter.cpp` byte for byte and re-parses.

### 40. Function attributes are never hoisted into an attribute group

*printer* — crates/llvmkit-ir/src/asm_writer.rs:3311-3320 (input-carried groups only), :2115 / :2525 / :3070 (inline header printing)

- **LLVM:** `SlotTracker::CreateAttributeSetSlot` mints one slot per distinct function `AttributeSet` and `AssemblyWriter::writeAllAttributeGroups` emits them at the end of the module, so a function header prints as `define void @f23() #13` with `attributes #13 = { alignstack=4 }` below.
- **llvmkit:** `asm_writer.rs` prints function attributes inline on the header (`define void @f() alignstack(4)`) and emits an `attributes #N = { … }` block only for groups the *input* already carried, read straight out of `Module`'s attribute-group table. Output is bulkier than upstream's and diverges byte-for-byte from `llvm-dis` for any module with function attributes — which the parser/printer contract says should not happen.
- **Why:** Recorded, with the sequencing reason: land it with the `validateEndOfModule` group-merge work (W13) rather than alone, because the merge decides which attributes survive into the printed group, so doing the writer first would pin output the merge then changes.
- **Correction from verification:** Accurate as written, with two refinements. (1) The cited line pointers are slightly off: :2115 / :2525 / :3070 are the input-carried group-reference loops (`for group in ...
- **Fix:** Port `SlotTracker::CreateAttributeSetSlot` (one slot per distinct function `AttributeSet`, assigned during the module pre-pass) and `AssemblyWriter::writeAllAttributeGroups`, switch the function-header printer to emit `#N`, and merge the input's own groups into the same slot space. Sequence it after the `validateEndOfModule` group merge, then port `test/Bitcode/attributes.ll` as the round-trip it is.

### 41. `align` inside an attribute group is not moved onto the function

*parser* — crates/llvmkit-asmparser/src/ll_parser.rs:1470-1485 (the end-of-module sweep; no attribute-group merge among the `validate_*` calls);

- **LLVM:** `LLParser::validateEndOfModule` pulls `Alignment` out of a function's merged attribute set (`FnAttrs.getAlignment()` → `Fn->setAlignment(*A)` → `FnAttrs.removeAttribute(Attribute::Alignment)`), so `attributes #0 = { align = 8 }` re-prints as `define void @f() align 8` with the attribute gone from the group.
- **llvmkit:** No group-merge step runs at end of module, so the `align` entry stays inside the printed group and never reaches the function's alignment field. The written text round-trips instead of being normalised the way `llvm-as | llvm-dis` normalises it.
- **Why:** Recorded as part of the attribute-group entry, noted as having "no visible effect yet" only because the writer half is also missing; it is scheduled with the W13 `validateEndOfModule` group-merge work.
- **Fix:** Add the group-merge step to llvmkit's end-of-module sweep: for each function, merge its referenced groups into one attribute set, move `Alignment` to the function's alignment field and drop it from the set, then let the (new) group writer print what survives. Do this before the writer-side hoisting so the printed group is pinned once.

### 42. An unnamed calling convention prints as `cc 11`, not `cc11`

*printer* — crates/llvmkit-ir/src/calling_conv.rs:326 (`Display`), :359-365 (`FromStr`'s `cc ` branch); locked by crates/llvmkit-asmparser/tests/calling_conv_drift.rs

- **LLVM:** `AssemblyWriter::printCallingConv`'s default arm is `Out << "cc" << cc`, so a convention with no mnemonic prints as `cc11` — which `LLLexer` then reads as a single unknown identifier. `llvm-as` therefore cannot re-parse `llvm-dis`'s own output for `HiPE`, `AVR_BUILTIN`, `MSP430_BUILTIN`, `WASM_EmscriptenInvoke`, `M68k_INTR` or the two ARM64EC thunks.
- **llvmkit:** `CallingConv`'s `Display` writes `cc {n}` with a space — the spelling `LLParser::parseOptionalCallingConv`'s `kw_cc` arm actually accepts. llvmkit's output round-trips through its own parser and remains valid input to `llvm-as`, at the cost of one byte of difference from `llvm-dis`.
- **Why:** Recorded, and deliberate: this is the one place the byte-for-byte printer rule is knowingly broken, and it is broken in the safe direction — reproducing upstream would mean emitting text neither parser can read back.
- **Correction from verification:** The byte-level divergence is real and still present, but the stated rationale for it is false — and the file's own doc comment plus `docs/future-work.md` record that false premise. Corrected statement: `AssemblyWriter::printCallingConv`'s default arm is `Out << "cc" << cc`, so an unnamed convention prints as `cc11`;
- **Fix:** Nothing to fix unless upstream fixes `printCallingConv`. If it ever does, change the one `write!(f, "cc {}", self.0)` and re-bless the drift lock; if byte-parity with today's `llvm-dis` is ever demanded instead, the round-trip corpus and the drift lock both have to be told the output is deliberately unreadable.

### 43. DWARF enumerations and `DIExpression` operands are stored as spellings, so numeric forms never normalise

*IR model* — crates/llvmkit-asmparser/src/ll_token.rs (the nine `Token::Dwarf*` variants), crates/llvmkit-asmparser/src/ll_parser.rs:5865-5878 (`parse_metadata_field_value`'s enum arm …

- **LLVM:** `LLParser::parseDIExpressionBody` and the DWARF `MDField` readers convert each keyword to its integer encoding via `dwarf::getOperationEncoding` / `getAttributeEncoding` and store a `uint64_t`; the printer maps the encoding back to a name, so `!DIExpression(15)` prints as the `DW_OP_*` that 15 encodes and a numeric `DW_ATE`/`DW_TAG` field likewise comes back named.
- **llvmkit:** Every `DW_TAG_*`, `DW_ATE_*`, `DW_VIRTUALITY_*`, `DW_LANG_*`, `DW_LNAME_*`, `DW_CC_*`, `DW_OP_*`, `DW_MACINFO_*` and `DW_APPLE_ENUM_KIND_*` word is lexed into its own token carrying the full keyword text and collapses into `MetadataFieldValue::Enum(String)`; `DwarfExpressionOperand::Operation` keeps the source spelling. The AsmWriter writes `Enum(s)` verbatim and `Integer(v)` as the number, so a numerically-written operand or field prints back as a number where `llvm-dis` prints a name.
- **Why:** Recorded: the typed form is the several-hundred-constant `Dwarf.def` family, which is generation work for `llvmkit-tablegen`'s sibling arm rather than a hand-typed enum, and it is deferred to the debug-info/metadata round-trip milestone where a consumer that needs the values exists.
- **Correction from verification:** Substantially accurate and still present; three corrections. (1) The illustrative example is wrong: 15 is DW_OP_const8s, which is not in DIExpression::isValid's accept switch, so writeDIExpression falls to its `else` branch and upstream prints `15` too.
- **Fix:** Generate the `Dwarf.def` tables (both directions) through `llvmkit-tablegen`, store the encoding rather than the name at parse time, and consult the reverse table at print time so a numeric input normalises to its keyword. The reverse tables are the half that closes the observable divergence; the forward tables already exist as `llvmkit_ir::dwarf::*` for W11's validation.

### 44. `ptrtoint`/`ptrtoaddr` mid-width folding diverges on `pointer_size != index_size` layouts

*constant folder* — crates/llvmkit-ir/src/constant_folding.rs:2432 (`fold_ptr_to_int_pair`)

- **LLVM:** In `isEliminableCastPair`'s case-11 *declined* sub-case (`MidSize < SrcSize && MidSize < DstSize`), `ConstantFoldCastOperand` falls to its switch path, where `PtrToInt` takes `DL.getAddressType` and `PtrToAddr` takes `DL.getIntPtrType` — the inverse of each other. On `p:128:128:128:64`, `ptrtoaddr(inttoptr(i128 x)):i128` folds to `x`.
- **llvmkit:** `fold_ptr_to_int_pair` always two-steps through the case-11 mid (`ptrtoint` to the pointer size, `ptrtoaddr` to the index/address size), so the same expression folds to `x mod 2^64` — the semantically correct address extraction, and a different constant from upstream's.
- **Why:** Recorded as a deliberate, reasoned divergence awaiting an explicit decision: llvmkit's value is arguably the more correct side, and matching upstream would mean introducing a wrong mask to copy an upstream quirk, on layouts x86 and the `bin_lift` consumer never use.
- **Fix:** Only if CHERI-like parity comes into scope: mirror the switch path's type choice (`getAddressType` for `PtrToInt`, `getIntPtrType` for `PtrToAddr`) in the declined case-11 sub-case, and port the upstream fixture that pins it so the quirk is documented as copied rather than invented.

### 45. DCE keeps calls and allocations upstream deletes

*passes* — crates/llvmkit-ir/src/dce.rs:49-79 (`is_trivially_dead`); crates/llvmkit-ir/src/value.rs:570 (`has_uses`)

- **LLVM:** `wouldInstructionBeTriviallyDead` (`lib/Transforms/Utils/Local.cpp`) deletes unused `willReturn`+readnone calls, removable allocation-function calls, `free(null)`, and lifetime-only allocas; `Value::use_empty` ignores debug-record uses, and upstream salvages debug info instead of being blocked by it.
- **llvmkit:** `is_trivially_dead` returns `false` for every `Call` (and every `VaArg`/pad/atomic), so `DcePass` leaves all of the above in place; the module after DCE differs from upstream's. The record also notes `Value::has_uses` counts debug-record uses upstream ignores, which keeps further instructions alive.
- **Why:** Recorded: porting these needs faithful allocation-function and attribute modelling to avoid *over*-removal, which would be a miscompile if wrong, so the current DCE is deliberately conservative-but-safe.
- **Fix:** Model allocation functions and the relevant call attributes (`willReturn`, memory effects, the allocator family) first, then port `wouldInstructionBeTriviallyDead`'s call/alloca arms against them; separately, give `has_uses` a structural-uses-only sibling for the DCE query and port the debug-info salvage path rather than counting debug records as uses.

### 46. InstSimplify folds inside unreachable blocks that upstream skips

*passes* — crates/llvmkit-ir/src/inst_simplify.rs:23-68 (`type Requires = ()`, the ungated worklist loop)

- **LLVM:** `InstSimplifyPass.cpp::runImpl` iterates reachable blocks only, gating each on `DT->isReachableFromEntry(&BB)`, so instructions in dead blocks are left exactly as written.
- **llvmkit:** `InstSimplifyPass::run` walks the whole function worklist with no reachability gate — its `Requires` is `()` and no dominator tree is consulted — so it folds in unreachable blocks and the printed function differs from upstream's in that dead code.
- **Why:** Recorded as a textual-only divergence in dead code; closing it needs reachability (a dominator tree) threaded into the pass. The entry also notes the knock-on test gap: no InstSimplify test covers unreachable-block behaviour, precisely because the skip is missing.
- **Correction from verification:** Accurate as written, with one refinement: because llvmkit gates on `!has_uses` and only erases after a successful fold, the divergence inside unreachable blocks is specifically folding-and-erasing-the-folded-instruction — upstream additionally skips its `isInstructionTriviallyDead` deletion arm there, which llvmkit never performs in that …
- **Fix:** Change the pass's `Requires` to prefetch `DominatorTree`, skip any block failing `is_reachable_from_entry`, and port the upstream fixture that pins the skip. The analysis is already available (`DominatorTree::is_reachable_from_entry` is what the phi verifier rule uses), so this is plumbing plus one test.

### 47. `SymbolicallyEvaluateGEP` sub-cases not ported — fewer folds than upstream

*constant folder* — crates/llvmkit-ir/src/constant_folding.rs (the GEP symbolic-evaluation path)

- **LLVM:** `SymbolicallyEvaluateGEP` (`ConstantFolding.cpp`) additionally normalises vector index widths via `CastGEPIndices`, preserves `in_range` through nested GEPs, folds a null- or `inttoptr`-nonzero-base GEP to an `inttoptr` (using `mustNotIntroduceIntToPtr` and `APInt::insertBits`), and infers `inbounds` for GEPs of globals from a dereferenceable-bytes query.
- **llvmkit:** None of the four sub-cases is ported, so llvmkit declines these folds; the constant expression survives into the printed module where upstream would have folded it away.
- **Why:** Recorded under the constant-folding parity cycle's known-remaining points, with the property that matters stated explicitly: each sub-case only ever *declines*, never mis-folds, so the divergence is weaker output rather than wrong output.
- **Correction from verification:** Still present, and the four-way gap is real — but the claim's framing is wrong in three places. (1) "None of the four sub-cases is ported, so llvmkit declines these folds; the constant expression survives into the printed module" is wrong for the fourth sub-case. Missing `inbounds` inference does NOT decline a fold.
- **Fix:** Port the four sub-cases one at a time against `ConstantFoldingTest.cpp` and the matching `test/Transforms/InstSimplify` fixtures; the null/`inttoptr` arm needs `mustNotIntroduceIntToPtr` and `ApInt::insert_bits` first, and the inbounds inference needs a dereferenceable-bytes query that does not exist yet.

### 48. `Value::num_uses` counts metadata and debug-record edges

*IR model / use lists* — crates/llvmkit-ir/src/value.rs:584 (definition), crates/llvmkit-ir/src/value.rs:215 (recorded reason), crates/llvmkit-ir/src/value.rs:166 (the filtered twin)

- **LLVM:** `Value::getNumUses` (`llvm/IR/Value.h`) counts entries on the intrusive `Use` list only. A `ValueAsMetadata` reference creates no `Use` at all — metadata references are tracked separately through `ReplaceableMetadataImpl` — which is precisely why `AsmWriter::orderModule` has to reach *through* `MetadataAsValue` wrappers and debug records to find the constants behind them.
- **llvmkit:** `ValueData::use_list` is a single `Vec<ValueUse>` that also carries `ValueUse::Metadata(..)` and `ValueUse::DebugRecord { .. }` edges (pushed at `module.rs:2590` and `instruction.rs:1535`/`:1549`). The public `Value::num_uses` returns `self.data().use_list.borrow().len()` with no filter, while its doc claims `Mirrors Value::getNumUses`. A value named by `!{i32 1}` or by a `#dbg_value` record therefore reports a use count strictly larger than LLVM's.
- **Why:** Recorded at `crates/llvmkit-ir/src/value.rs:215`: "llvmkit's use list is deliberately wider than upstream's: a metadata node or a debug record keeps a value alive and must be reached by RAUW, so each gets an edge." The wider list is deliberate;
- **Correction from verification:** Still present and substantially accurate, with three refinements. (1) Scope is wider than stated: `num_uses` is not the only unfiltered public counter. `Value::has_uses` (value.rs:570-572, `!use_list.borrow().is_empty()`, doc'd "Mirrors `Value::hasUses`") and `Value::has_one_use` (value.rs:578-580, `use_list.borrow().len() == 1`, doc'd "M …
- **Fix:** Make `num_uses` count only operand edges — `self.data().use_list.borrow().iter().filter(|e| e.is_operand_use()).count()` — and promote a separate public accessor (`num_references` / `all_uses`) for the wider set, since RAUW and liveness genuinely want it. Add a test that a value referenced only from a named-metadata node reports `num_uses() == 0`, matching `getNumUses`.

### 49. Numeric calling-convention fallback prints `cc 11`, not `cc11`

*printer (AsmWriter)* — crates/llvmkit-ir/src/calling_conv.rs:326 (the `write!`), reason at crates/llvmkit-ir/src/calling_conv.rs:312-319

- **LLVM:** `printCallingConv`'s default branch (`lib/IR/AsmWriter.cpp`) writes `Out << "cc" << cc`, producing `cc11` with no space, for any convention with no mnemonic (HiPE, M68k_INTR, the ARM64EC thunks, …).
- **llvmkit:** `impl Display for CallingConv` writes `write!(f, "cc {}", self.0)` — `cc 11`, with a space.
- **Why:** Recorded in the doc comment as a one-off deliberate byte-level divergence: `LLLexer` reads `cc11` as a single unknown identifier, so `llvm-as` cannot re-parse `llvm-dis`'s own output for a mnemonic-less convention.
- **Correction from verification:** The divergence itself is accurately described and still present: upstream's printCallingConv default branch writes `Out << "cc" << cc` (no space) while llvmkit's `Display for CallingConv` writes `write!(f, "cc {}", self.0)` at crates/llvmkit-ir/src/calling_conv.rs:326, and that Display is what asm_writer.rs uses for function headers, decl …
- **Fix:** This is the one place the project knowingly breaks the byte-for-byte contract with `AsmWriter.cpp`. If byte parity must win, emit `cc{n}` and accept that the output is not re-parseable (matching upstream's own bug), and add a round-trip-corpus exemption.

### 50. `possibly_demanded_elements_in_mask` answers exactly for a `zeroinitializer` mask

*analysis / VectorUtils* — crates/llvmkit-ir/src/vector_utils.rs:641 (definition), reason at crates/llvmkit-ir/src/vector_utils.rs:634-640

- **LLVM:** `llvm::possiblyDemandedEltsInMask` reaches its per-element loop only through `dyn_cast<ConstantVector>`. A `zeroinitializer` mask is a `ConstantAggregateZero`, not a `ConstantVector`, so the cast fails and upstream returns "every lane demanded" for a mask that demands none.
- **llvmkit:** llvmkit stores `ConstantVector` and `ConstantAggregateZero` as one element list, so the loop runs for a `zeroinitializer` mask and the function returns the exact all-zero `ApInt`.
- **Why:** Recorded at the call site: "Stronger than upstream, in a sound direction … Over-approximating fewer lanes is always safe for this query; the divergence can only make a caller more precise." It falls out of llvmkit's constant representation rather than being sought.
- **Fix:** No fix wanted for correctness — but the port is no longer faithful, so any upstream fixture asserting the weak answer will diverge. Either gate the element loop on the constant actually being a spelled-out `ConstantVector` (restoring bug-for-bug parity and losing precision), or keep it and add an `UPSTREAM.md` row plus a test naming the divergence so a future parity sweep does not silently "fix" it back.

### 51. Known-FP-class bitcast arm gets a fresh known-bits recursion budget

*analysis / KnownFPClass* — crates/llvmkit-ir/src/known_fp_class.rs:429-437

- **LLVM:** `computeKnownFPClass`'s bitcast-from-integer arm recurses into `computeKnownBits` at `Depth + 1` on the *shared* analysis budget, so a bitcast reached late in an FP walk hands known bits a query already at `MaxAnalysisRecursionDepth` — which learns nothing.
- **llvmkit:** The `depth` parameter is explicitly discarded (`let _ = depth;`) and `compute_known_bits(source, query)` is entered as a separate top-level query starting at zero. A deep bitcast chain is therefore answered more precisely than upstream answers it.
- **Why:** Recorded inline: "That is sound — known bits is correct at any depth, and depth only bounds discovery — but it is a divergence upward, and it costs compile time."
- **Fix:** Thread the budget: change the call to a depth-aware entry point (`compute_known_bits_at_depth(source, query, depth + 1)`) so the shared budget is honoured, matching upstream's discovery envelope and removing the compile-time cost. Keep the current behaviour only if a ported fixture demonstrably needs the extra precision — in which case record it in `UPSTREAM.md` rather than only in a source comment.

### 52. Phi known-bits recurses at `depth + 1` instead of upstream's fixed deep depth

*analysis / ValueTracking* — crates/llvmkit-ir/src/value_tracking.rs:1726 (definition), reason at crates/llvmkit-ir/src/value_tracking.rs:1715-1725

- **LLVM:** The `Instruction::PHI` arm of `computeKnownBitsFromOperator` gates its intersection loop on `Depth < MaxAnalysisRecursionDepth - 1` and then recurses at the fixed depth `MaxAnalysisRecursionDepth - 1`, capping the search under any incoming value at one level so the walk does not "spin around in loops".
- **llvmkit:** `phi_known_bits` recurses at `depth + 1`, so a shallow phi gets a full remaining budget under each incoming and can prove strictly more bits than upstream.
- **Why:** Recorded inline: llvmkit already terminates by a different mechanism — the `stack` set rejects re-entering a value that is mid-computation — and `compute_known_bits_inner` memoizes on `(slot, query)` with no depth component, so entering an incoming at a fixed deep depth would cache the weak answer and hand it to a later shallow query of t …
- **Fix:** The recorded reason is sound and the fix is not the depth but the cache: add the depth (or a "budget remaining" bucket) to the memo key, or mark entries computed under a truncated budget as non-cacheable. With that in place the fixed `MaxAnalysisRecursionDepth - 1` recursion can be restored and the port becomes faithful.

### 53. Shift-amount poison refinement with no upstream counterpart

*analysis / ValueTracking* — crates/llvmkit-ir/src/value_tracking.rs:5694-5730

- **LLVM:** `isGuaranteedNotToBeUndefOrPoison` reaches `canCreateUndefOrPoison`, whose `shiftAmountKnownInRange` helper is *syntactic* — it only recognises a constant shift amount. A `shl`/`lshr`/`ashr` by a non-constant amount is therefore classified "can create poison" and the operand walk is skipped, so upstream answers "not guaranteed".
- **llvmkit:** An extra arm re-examines shifts: if known bits prove the amount is in range and both operands are themselves guaranteed not to be undef-or-poison, the shift is reported well defined. llvmkit answers `true` where upstream answers `false`.
- **Why:** Labelled in-source as an "llvmkit refinement (no upstream counterpart)"; the reasoning given is that known bits can prove the amount in range even when the syntactic check cannot, and a shift with an in-range amount and well-defined operands provably is not poison.
- **Fix:** This is a genuine invention inside a port, and the project's own D11 says tests are ported not invented — the same should hold for analysis facts. Either move the refinement behind an explicit, separately named entry point (`is_known_not_poison_refined`) so `is_guaranteed_not_to_be_undef_or_poison` stays a faithful port, or delete it and carry the improvement upstream.

### 54. `uselistorder` prediction has no operand-number tie-break

*printer (AsmWriter) / use lists* — crates/llvmkit-ir/src/asm_writer.rs:511-525 (the sort), reason at crates/llvmkit-ir/src/asm_writer.rs:506-510

- **LLVM:** `predictValueUseListOrder`'s comparator, under `llvm::sort`, breaks a tie between two uses by the *same* user on the operand number (`LU.second < RU.second`), so `%x = add i32 %a, %a` yields a deterministic relative order for its two edges to `%a`.
- **llvmkit:** llvmkit's use list holds one indistinguishable `ValueUse::Instruction(user)` edge per reference with no operand index, so two uses by the same user compare equal; a stable sort leaves them in list order. The predicted `uselistorder` vector can therefore differ from LLVM's for any value used twice by one instruction.
- **Why:** Recorded inline at `asm_writer.rs:506-510`: the tie-break "has no llvmkit counterpart — its use list holds one indistinguishable edge per reference". Recorded in `docs/future-work.md`.
- **Fix:** Add the operand index to the operand-bearing use variants — `ValueUse::Instruction { user, operand: u32 }` and `ValueUse::Constant { user, operand: u32 }` — set at every `add_use` call site (the operand position is known there), then extend the comparator with upstream's `LU.second < RU.second` tie-break.

### 55. Non-uniform scalable vector constants print an unparseable element list

*printer (AsmWriter) / constant model* — crates/llvmkit-ir/src/asm_writer.rs:1310-1318 (printer fallback), crates/llvmkit-ir/src/asm_writer.rs:1272 (`prints_as_splat`), stale note at crates/llvmkit-ir/src/consta …

- **LLVM:** `ConstantVector::get` takes a fixed element count, so a scalable vector constant with a per-lane element list cannot be constructed in LLVM at all. Every scalable vector constant is either `zeroinitializer`, `poison`/`undef`, or a `splat (…)`, and `writeConstantInternal` has exactly those spellings.
- **llvmkit:** `VectorType::const_vector` accepts a scalable type with non-uniform elements (llvmkit represents a scalable splat as `min_len` equal elements, so the same storage admits unequal ones). When such a constant reaches `fmt_aggregate_constant` and `aggregate_splat_id` finds no common element, the printer falls through to the element-list branch and emits `<i32 1, i32 2>` for a scalable type — text LLVM has no form for and cannot re-parse.
- **Why:** Recorded inline at `asm_writer.rs:1310-1318`: printing losslessly was chosen over collapsing to a `splat (…)` of the first lane, "because claiming a splat the constant is not would corrupt silently where this merely fails to re-parse".
- **Correction from verification:** Substantially accurate; two refinements. (1) The claim implies the bad text is merely unparseable by LLVM. It is now also unparseable by llvmkit's own parser: `parse_dynamic` rejects `<vscale x 4 x i32> <i32 7, i32 8, i32 7, i32 7>` in both global and instruction-operand position with "constant expression type mismatch: got type '<4 x i32 …
- **Fix:** Close it at the constructor, as the comment says: make `VectorType::const_vector` reject a scalable type whose elements are not all identical, returning an `IrError` rather than an unprintable constant. Then update the two tests that depend on the lax behaviour (they should be asserting the rejection instead), and the printer's fallback branch becomes unreachable for scalable types — replace it with an `unreachable!` …

### 56. ppc_fp128 component pair stored mirrored from upstream

*IR model / ApFloat* — crates/llvmkit-ir/tests/ap_float_ppc_word_order.rs:1-20; crates/llvmkit-ir/tests/ap_float_upstream_predicates.rs:57-77;

- **LLVM:** `DoubleAPFloat::bitcastToAPInt` builds `Data[] = {Floats[0]…, Floats[1]…}` then `APInt(128, 2, Data)`, and APInt word 0 is least significant — so the *leading* double lands in the **low** 64 bits. `TEST(APFloatTest, getZero)` therefore expects `PPCDoubleDouble` negative zero as `{0x8000000000000000, 0}`.
- **llvmkit:** `ppc_words` reads the **high** word as the leading double. `ap_float_upstream_predicates.rs::get_zero` pins the mirrored row `(PPC, true, [0, 0x8000_0000_0000_0000])` with an inline comment `// Upstream: {0x8000000000000000, 0} — mirrored here`. `ap_float_ppc_word_order.rs` pins the mirroring itself and states outright that `to_bits` "does **not** agree with upstream's `bitcastToAPInt` for this one semantics".
- **Why:** Recorded. The module doc says the mirroring is invisible to finite arithmetic (both components are summed) and visible only in three places: the zero/NaN/infinity category (decided by the leading component alone), the placement of a special value by the `qnan`/`inf`/`zero` constructors, and `to_bits`.
- **Fix:** Swap the word order inside `ppc_words` / `ApFloat::from_bits`+`to_bits` for `PpcDoubleDouble` so word 0 holds the leading double, then remove the compensating transposition in the `.ll` reader and `AsmWriter` printer (`parser_hex_float_word_order.rs` records that the two mirrorings currently cancel), and restore the upstream row in `get_zero`.

### 57. No attribute-group printer, so half of two upstream fixtures is unreachable

*printer (AsmWriter)* — crates/llvmkit-asmparser/tests/parser_attribute_matrix.rs:148-164; crates/llvmkit-asmparser/tests/parser_module_level.rs:26-29

- **LLVM:** `test/Bitcode/attributes.ll`'s `@f23` writes `define void @f23() alignstack(4)` inline and CHECKs `attributes #13 = { alignstack=4 }`, pinning both halves of the group spelling at once. `test/Assembler/global-variable-attributes.ll`'s `@g1`–`@g4` likewise need the trailing global attribute list plus the group printer.
- **llvmkit:** llvmkit's printer emits function attributes inline on the header and never forms an attribute group, so the CHECK lines cannot be produced from that input. Only the parse half of `@f23` is asserted; `@g1`–`@g4` are not ported. The group spelling itself is covered only from group-carrying *input*.
- **Why:** Recorded — "The printer gap is recorded in `docs/future-work.md`", and the global half is tagged W7 work.
- **Correction from verification:** Substantively accurate; two refinements. (1) The fixture is `test/Assembler/globalvariable-attributes.ll`, not `global-variable-attributes.ll` — the hyphenated name in the claim does not exist upstream (llvmkit itself miscites it at ll_parser.rs:15502 and parser_module_level.rs:660, while parser_module_level.rs:18 spells it correctly).
- **Fix:** Port `AssemblyWriter::printModule`'s attribute-set collection: number distinct function attribute sets, print `attributes #N = { … }` blocks at module end, and emit `#N` on the headers. Then port `@f23` whole and add `@g1`–`@g4` with the trailing global attribute list.

### 58. Function/global attributes are never hoisted into an attribute group by the printer

*printer (AsmWriter)* — crates/llvmkit-ir/src/asm_writer.rs, crates/llvmkit-ir/src/global_variable.rs, crates/llvmkit-asmparser/src/ll_parser.rs

- **LLVM:** `SlotTracker::CreateAttributeSetSlot` assigns one `#N` slot per distinct `AttributeSet` and `AssemblyWriter::writeAllAttributeGroups` emits the groups at the end of the module, so a function header prints `define void @f23() #13` with `attributes #13 = { alignstack=4 }` below. `validateEndOfModule` additionally moves a group's `Alignment` to `Fn->setAlignment()`, so `attributes #0 = { align = 8 }` re-prints as `define void @f() align 8`.
- **llvmkit:** `asm_writer.rs` prints function attributes inline on the header and emits an `attributes #N = { … }` block only for groups the *input* already carried. A global's trailing attribute list is not printed at all, and the `align`-out-of-group move never happens.
- **Why:** Recorded in docs/future-work.md (W5/W7): the group-*forming* machinery has never existed, and the item is routed to W13 deliberately — `validateEndOfModule`'s merge decides which attributes survive into the printed group, so building the writer first would pin output the merge then changes.
- **Correction from verification:** Accurate, with one refinement and one sharpening. REFINEMENT (globals): the claim says a global's trailing attribute list "is not printed at all". It is worse than that — it is not *parsed* at all.
- **Fix:** Port `CreateAttributeSetSlot` + `writeAllAttributeGroups` together with W13's `validateEndOfModule` attr-group merge and the alignment-attr→field move. Unblocks `globalvariable-attributes.ll`'s `@g1`–`@g4` and makes `test/Bitcode/attributes.ll` portable as a round-trip (its CHECK lines are group lines).

### 59. DIExpression operands keep their source spelling instead of the DWARF encoding, so nothing normalises on print

*metadata model / printer* — crates/llvmkit-asmparser/src/ll_token.rs (`Token::Dwarf*` carrying full keyword text), crates/llvmkit-asmparser/src/ll_parser.rs (`parse_metadata_field_value`, `MetadataF …

- **LLVM:** `LLParser::parseDIExpressionBody` converts each operand through `dwarf::getOperationEncoding` / `getAttributeEncoding` and stores a `uint64_t`; `AsmWriter`'s DIExpression printer maps it back, so `!DIExpression(15)` prints as the `DW_OP_...` name that value encodes.
- **llvmkit:** `DwarfExpressionOperand::Operation` stores the written spelling. W11 added *validation* against the same tables (an unrecognised `DW_OP_*`/`DW_ATE_*` is now rejected with `invalid DWARF op '...'` / `invalid DWARF attribute encoding '...'`), but storage and normalisation are untouched: `!DIExpression(15)` round-trips as `15`.
- **Why:** Recorded in docs/future-work.md, deferred to the debug-info/metadata round-trip milestone (ROADMAP Milestone 12) — the typed form is the nine `Dwarf.def` families, several hundred constants, which is generation work for `llvmkit-tablegen` rather than a hand-typed enum.
- **Correction from verification:** The divergence is real and still present, but the claim's worked example is wrong and its "Where" list is slightly off. CORRECTED STATEMENT: DIExpression operands keep their source spelling instead of the DWARF encoding, so nothing normalises on print.
- **Fix:** Generate the nine DWARF families as typed tables, store encodings on the way in, and consult the reverse tables at print time so numeric spellings normalise to names. `element too large, limit is N` already landed; the remaining half is storage + printing.

### 60. `AsmParserContext` is populated by a line-scanning heuristic, not real spans

*parser — diagnostics/context* — crates/llvmkit-asmparser/src/parser.rs:294-301,:374; crates/llvmkit-asmparser/src/asm_parser_context.rs

- **LLVM:** Upstream records real token positions as it parses; the parse context of a construct is a captured location, not a reconstruction.
- **llvmkit:** `record_parser_context` greps the source bytes for `"define "` / `"@name("` / `"}"` to reconstruct which function a construct belongs to. Verified present at parser.rs:374, called from parser.rs:301.
- **Why:** Recorded in annex A14 as a fidelity divergence: "a heuristic that happens to agree on pretty input is a fidelity divergence, and this program's span-carrying diagnostics make the real spans free". Routed to W13.
- **Fix:** Capture spans at the parse sites and populate `AsmParserContext` from them; delete the byte-scanning reconstruction. Same commit should fix the stale `SlotMapping` doc comment that claims `metadata_nodes` is omitted (the field exists).

## Model gaps

A public query answers differently from its LLVM counterpart, or a structure LLVM has is missing.

### 61. Plain `add`/`sub`/`div`/shift builders consult the wrong folder hook

*IR builder* — crates/llvmkit-ir/src/ir_builder.rs (`int_add` / `int_sub` and the div/shift emitters), crates/llvmkit-ir/src/ir_builder/folder.rs (the hook trait)

- **LLVM:** `IRBuilder::CreateAdd` funnels through `FoldNoWrapBinOp(.., false, false)`, and `CreateUDiv` and friends through `FoldExactBinOp(.., false)`, so a folder sees the no-wrap/exact hook even when no flag is set.
- **llvmkit:** `int_add` / `int_sub` and the div/shift siblings consult the plain `fold_int_bin_op` hook directly. Results are identical with the shipped folders; a third-party folder that overrides only the no-wrap or exact hooks observes the difference.
- **Why:** Recorded under the 2026-07-06 upstream-parity follow-ups, with the observability boundary stated: identical results with the shipped folders, observable only by third-party folders overriding just those hooks.
- **Correction from verification:** Still present, but the title's "shift" is over-broad and the scope is understated in two ways. Accurate statement: llvmkit's `int_add` and `int_sub` consult the plain `fold_int_bin_op` hook where upstream `CreateAdd`/`CreateSub` funnel through `FoldNoWrapBinOp(.., false, false)`;
- **Fix:** Route the flagless emitters through the no-wrap / exact hooks with the flags set false, matching `FoldNoWrapBinOp` / `FoldExactBinOp`, and add a test folder that overrides only those hooks to witness the dispatch — the divergence is unobservable without one.

### 62. The phi known-bits arm recurses deeper than upstream, and answers more precisely

*analysis* — crates/llvmkit-ir/src/value_tracking.rs:1715-1726 (the recorded decision on `phi_known_bits`)

- **LLVM:** `computeKnownBitsFromOperator`'s `Instruction::PHI` arm gates its incoming-value intersection on `Depth < MaxAnalysisRecursionDepth - 1` and then recurses at that fixed depth, capping the search under an incoming value at one level so it does not spin around loops.
- **llvmkit:** `phi_known_bits` recurses at `depth + 1`, so it can prove strictly more about a shallow phi than upstream does. `@test_udiv_neg` in the ported `recurrence-knownbits.ll` witnesses it: llvmkit proves 60 leading zeros where upstream proves none (the fixture's own claim, bit 2 unknown, is untouched).
- **Why:** Recorded and deliberate, with two reasons: llvmkit already terminates by a different mechanism — the `stack` set rejects re-entering a value mid-computation — and `compute_known_bits_inner` memoizes on `(slot, query)` with no depth component, so entering an incoming value at a fixed deep depth would cache the weak answer computed there an …
- **Correction from verification:** The code-level divergence is real and still present, but the witness sentence is wrong and should be dropped. Accurate half: upstream's `Instruction::PHI` arm in `computeKnownBitsFromOperator` (ValueTracking.cpp) guards the intersection with `if (Depth < MaxAnalysisRecursionDepth - 1 && Known.isUnknown())` and then calls `computeKnownBits …
- **Fix:** Leave it until the known-bits cache becomes depth-keyed. At that point add the depth component to the memo key, adopt upstream's `MaxAnalysisRecursionDepth - 1` gate and fixed-depth recursion, and re-run `recurrence-knownbits.ll` — `@test_udiv_neg`'s llvmkit-specific extra precision is the assertion that will move.

### 63. A shift by a non-constant but provably in-range amount is treated as non-poison

*analysis* — crates/llvmkit-ir/src/value_tracking.rs:5694-5710 (the refinement), :5569-5572 (the function's doc statement of it), :4611-4620 (`shift_amount_known_in_range`, the faithf …

- **LLVM:** `shiftAmountKnownInRange` is syntactic — it requires a literal constant shift amount below the bit width — so `isGuaranteedNotToBeUndefOrPoison` treats a shift by any non-constant amount as able to create poison and skips the operand walk.
- **llvmkit:** `is_guaranteed_not_to_be_undef_or_poison` adds an arm with no upstream counterpart: if known bits prove the amount in range and the operands are well defined, the shift is not poison. It answers `true` strictly more often than upstream.
- **Why:** Recorded at the site as a deliberate llvmkit refinement, on the ground that a shift whose amount is in range and whose operands are well defined provably is not poison — so the extra `true` is sound, never unsound. `shift_amount_known_in_range` itself is a faithful port of the syntactic predicate;
- **Fix:** Nothing to close unless strict parity is wanted; if it is, delete the arm and the divergence note together, and expect the ported poison fixtures to keep passing (the arm only strengthens answers). Any consumer that relies on the stronger answer must be checked first — the refinement is reachable from every `isGuaranteedNotToBePoison` caller.

### 64. `returned_arg_operand` reads only the call site's argument attributes

*analysis* — crates/llvmkit-ir/src/value_tracking.rs:965-974 (`returned_arg_operand`), :976 (`returned_attr`); correct twin in crates/llvmkit-ir/src/pointer_analysis.rs:1316

- **LLVM:** `CallBase::getReturnedArgOperand` consults the *callee's* parameter attributes as well as the call site's, so `declare ptr @f(ptr returned)` makes a call that does not repeat `returned` still return its argument.
- **llvmkit:** `returned_arg_operand` in `value_tracking.rs` scans only the `arg_attrs` slice its caller hands it — the call site's — so the declaration-side `returned` is missed and the `returned` arm of `call_known_bits` is weaker than upstream's. `pointer_analysis.rs` ports both halves correctly; this is the same shortfall in a different function.
- **Why:** Recorded as a tranche-5 finding, with the sibling explicitly named: an upstream fixture caught the missing half in `pointer_analysis.rs`, and the `value_tracking.rs` twin was left as-is. No reason for leaving it is recorded.
- **Fix:** Give `returned_arg_operand` the callee value as well as the call-site attributes and check the callee's parameter attributes when the call site has none, mirroring what `pointer_analysis.rs` already does; take the upstream fixture that caught the pointer-analysis half in the same commit.

### 65. Two min/max matcher arms decline matches upstream accepts, rather than minting a constant

*analysis* — crates/llvmkit-ir/src/select_pattern.rs:1210-1216 (`not_value`), :1449-1456 (`look_through_cast_arm`)

- **LLVM:** `getNotValue`'s second arm folds `~C` for a constant operand via `ConstantInt::get(V->getType(), ~*C)`, and `lookThroughCastConst` builds a casted constant with `ConstantExpr::getTrunc` / `ConstantFoldCastOperand` and checks it round-trips. Both let `matchSelectPattern` recognise shapes llvmkit's ports do not.
- **llvmkit:** `not_value` reports only the `xor X, -1` form, and `look_through_cast_arm` ports only the two arrangements that need no new value. A `not` written as a folded constant, and the cast arrangement needing a materialised constant, are not recognised — each forgoes a match rather than inventing one, so the select-pattern result is weaker than upstream's.
- **Why:** Recorded at both sites and in the backlog: minting a constant is a module mutation, which an analysis has no business performing. The same ground is why `getFlippedStrictnessPredicateAndConstant` is left unported entirely rather than split into a caller-builds-the-constant variant, which the entry argues would be a different function.
- **Fix:** Only closable by deciding that a constant-minting analysis is acceptable, or by giving these matchers a mutation-capable variant taking a module token — at which point `getFlippedStrictnessPredicateAndConstant` becomes portable too and the three should land together. Until then the divergence is the price of the no-mutation rule, and the sites already say so.

### 66. Phi `remove_incoming` never self-erases an emptied phi

*IR model / Instructions* — crates/llvmkit-ir/src/instructions.rs:1430 (helper), crates/llvmkit-ir/src/instructions.rs:1637 (public `PhiInstDyn::remove_incoming` doc), crates/llvmkit-ir/src/phi_raw_ …

- **LLVM:** `PHINode::removeIncomingValue(unsigned Idx, bool DeletePHIIfEmpty = true)` (`lib/IR/Instructions.cpp`) defaults to `true`: after `swap_remove`ing the entry, if the phi has no operands left it is `replaceAllUsesWith(PoisonValue)`d and `eraseFromParent()`ed. Callers that drop a predecessor rely on that.
- **llvmkit:** `phi_remove_incoming` (the shared body of all four phi handles' `remove_incoming`) mirrors the `swap_remove` exactly but stops there. Removing the last incoming leaves a live phi with zero incomings — a node that prints as `%p = phi i32` with no `[ … ]` pairs, which `LLParser::parsePHI` cannot re-read. The caller is expected to finish the job.
- **Why:** Recorded at `crates/llvmkit-ir/src/instructions.rs:1417`: erasure in llvmkit goes through `Instruction::erase_from_parent`, which *consumes* the linear lifecycle handle so use-after-erase is a compile error;
- **Correction from verification:** The core divergence is accurate and unchanged. One supporting detail is wrong: the claim (and llvmkit's own comments) say a bracket-less phi is something "LLParser::parsePHI cannot re-read".
- **Fix:** Either (a) give the erased phi surface a linear, consuming variant — `remove_incoming_or_erase(self, …) -> Either<Value, ErasedPhi>` taking the phi by value so the handle cannot outlive the erase; or (b) return a `PhiEmptied` marker the caller must destructure, making the leftover unignorable. (b) is cheaper and preserves the `Copy` handle for the common case.

### 67. `DIExpression` elements stored as source spelling, not DWARF encodings

*IR model / metadata, parser* — crates/llvmkit-ir/src/metadata.rs:2100-2107 (model), crates/llvmkit-asmparser/src/ll_parser.rs:5440-5478 (parser), stale reasons at crates/llvmkit-asmparser/src/ll_parser …

- **LLVM:** `LLParser::parseDIExpressionBody` maps each `DW_OP_*` / `DW_ATE_*` through `dwarf::getOperationEncoding` / `getAttributeEncoding` and stores a `uint64_t` in `DIExpression::Elements`. Downstream, `DIExpression::isValid()` walks those encodings and checks each operation's operand count and shape; `AsmWriter::writeDIExpression` prints them back by name.
- **llvmkit:** `DwarfExpressionOperand::Operation(String)` keeps the written spelling. The parser does validate the spelling against `llvmkit_ir::dwarf::operation_encoding` / `attribute_encoding` and rejects an unknown one, so the *parse* path matches upstream — but the model itself carries no encodings, so nothing performs `DIExpression::isValid()`'s operand-arity checking, and a `DIExpression` built through the IR API (rather than parsed) can hold an arbitrary `Operation(String)` that prints straight back out.
- **Why:** Recorded at `metadata.rs:2092-2099` and `ll_parser.rs:5433-5439`: the `Dwarf.def` tables were unmodelled when this landed, and `writeDIExpression` prints a known op back by name regardless, so the written form is what round-trips.
- **Correction from verification:** Accurate as written; two refinements. (1) The "stale reasons" charge applies only to the two in-code doc comments (crates/llvmkit-asmparser/src/ll_parser.rs:5433-5439 and crates/llvmkit-ir/src/metadata.rs:2092-2099), both of which assert the Dwarf.def tables are unmodelled and that an unknown DW_OP round-trips rather than being rejected — …
- **Fix:** Two steps. First, correct the two stale comments — the parser no longer accepts unknown ops, and `dwarf.rs` is a drift-locked transcription of `Dwarf.def`, so the recorded premise no longer holds. Second, change `DwarfExpressionOperand::Operation(String)` to carry the resolved `u32` encoding alongside (or instead of) the spelling, and port `DIExpression::isValid()` on top of it so operand-arity errors are caught;

### 68. `match_select_pattern` ignores fast-math flags written on the `select`

*analysis / SelectPattern* — crates/llvmkit-ir/src/select_pattern.rs:395 (definition), reason at crates/llvmkit-ir/src/select_pattern.rs:389-394

- **LLVM:** `llvm::matchSelectPattern` reads `SI->getFastMathFlags()` when the `select` is an `FPMathOperator`, and uses `nnan` / `nsz` from the select itself to admit float min/max idioms that the `fcmp`'s own flags do not justify.
- **llvmkit:** llvmkit's `select` instruction carries no fast-math flag word, so those flags cannot be consulted. Flags on the `fcmp` are read. Some float min/max patterns upstream recognises are declined here.
- **Why:** Recorded inline at `select_pattern.rs:389-394`: "llvmkit's `select` carries no flag word, so `nnan` / `nsz` written on the select cannot be consulted. Flags on the `fcmp` *are* read, which is where they normally sit. Some float patterns upstream accepts are therefore declined here — never the reverse."
- **Correction from verification:** The behavioral divergence is real and still present, but the stated reason is stale and wrong. `match_select_pattern` does still discard the select's own fast-math flags — at C:/Users/olegg/Desktop/llvmkit/crates/llvmkit-ir/src/select_pattern.rs:419-427 it hands `FastMathFlags::empty()` to `match_decomposed_select_pattern` where upstream …
- **Fix:** The blocker is the IR model, not the analysis: add a `FastMathFlags` field to `SelectInstData` (every other `FPMathOperator` in llvmkit already carries one), have the parser and `IrBuilder::select*` set it, print it in `AsmWriter`, and then read it here exactly as upstream does.

### 69. Shuffle mask transforms model the IR alphabet only

*analysis / VectorUtils* — crates/llvmkit-ir/src/vector_utils.rs:34-43 (module header), crates/llvmkit-ir/src/vector_utils.rs:208-213

- **LLVM:** `widenShuffleMaskElts`, `narrowShuffleMaskElts` and friends read mask elements as raw `int`s, so the same functions serve both the IR alphabet `{lane, poison}` and the wider one SelectionDAG and the X86 backend use, where `SM_SentinelZero` is `-2`. The widening rule is "negatives must be *equal* across a widened group", which distinguishes `-1` from `-2`.
- **llvmkit:** The transforms take `&[ShuffleMaskElem]`, which has only `Lane(n)` and `Poison`, so the equal-negatives rule collapses to "all poison". Three upstream assertions in `VectorUtilsTest.cpp` have no llvmkit spelling as a result.
- **Why:** Recorded in the module header as permanent, not pending: "code generation and target backends are out of scope", so no mask llvmkit can hold distinguishes the two negatives and the difference is unobservable in-tree.
- **Correction from verification:** Accurate as described; only the second line citation is stale. The site note is on `widen_shuffle_mask_elements` (crates/llvmkit-ir/src/vector_utils.rs:851-860), not :208-213 — that range is now inside `is_splat_value`'s doc comment. The module-header citation (:34-43) is exact.
- **Fix:** No fix warranted — the narrowing follows from the project charter, and the recorded reason is verified: `ShuffleMaskElem` genuinely has no sentinel-zero variant. The only action is to keep the three unportable upstream assertions visible: they are already listed in `tests/vector_utils_masks.rs`, and they should carry `UPSTREAM.md` rows marked "no llvmkit spelling" so the coverage gap reads as deliberate rather than m …

### 70. `simplifyPHINode`'s undef blending not mirrored

*passes / InstSimplify* — crates/llvmkit-ir/src/inst_simplify.rs:78 (definition), reason at crates/llvmkit-ir/src/inst_simplify.rs:72-77

- **LLVM:** `llvm::simplifyPHINode` folds a phi whose incomings are one common value *blended with* `undef` — `[X, undef]` simplifies to `X` — in addition to tolerating self-references.
- **llvmkit:** `uniform_phi_value` ignores only self-referencing incomings; an `undef` incoming makes the phi mixed, so the fold is declined. llvmkit's InstSimplify pass simplifies strictly fewer phis.
- **Why:** Recorded inline at `inst_simplify.rs:75-77`: "Undef blending (upstream folds `[X, undef]` to `X`) is deliberately not mirrored here; it is documented as out of scope." The recorded reason names a scope decision but not what blocks it — llvmkit does model `undef` constants, so the blocker is not obviously representational.
- **Correction from verification:** Real and still present, but narrower than described. Correct statement: upstream `llvm::simplifyPHINode` (InstructionSimplify.cpp) skips self-referencing, `PoisonValue`, and `Q.isUndefValue` incomings, so `[X, undef]` and `[X, poison]` fold to `X` (guarded by `valueDominatesPHI`, plus `isGuaranteedNotToBePoison` when an undef input is pre …
- **Fix:** Verify the recorded premise before extending — `undef` is representable (`ConstantData::Undef`), so this looks closable rather than blocked. Extend `uniform_phi_value` to skip incomings that are `undef` alongside self-references, returning `None` only when two distinct non-undef, non-self values appear, and return the common value when at least one non-undef incoming exists.

### 71. getVScaleRange gap row cites a blocker that no longer exists

*ValueTracking parity ledger* — crates/llvmkit-ir/tests/value_tracking_parity.rs:495-498; contradicted by crates/llvmkit-asmparser/tests/attribute_td_drift.rs:34-38, crates/llvmkit-ir/src/attributes.rs: …

- **LLVM:** `llvm::getVScaleRange` (`ValueTracking.h`) reads the `vscale_range` function attribute's (min, max) pair and returns a `ConstantRange`.
- **llvmkit:** Listed in `VALUE_TRACKING_GAPS` as "blocked on the `vscale_range` attribute itself, which attribute_td_drift.rs lists as NOT_YET_MODELED: upstream reads a packed (min, max) pair and llvmkit's payload is a single u64, so porting it would mean inventing the second half." **Both halves of that reason are false today.** `attribute_td_drift.rs`'s `NOT_YET_MODELED` is `&[]` with a doc saying "**The list is empty**", and the payload is not a u64 — `Attribute::VScaleRange { min: u32, max: Option<u32> }` is a structured pai …
- **Why:** Unrecorded — the row was never revised after `vscale_range` landed. This is the same failure mode `vector_utils_parity.rs` documents for its own table ("An earlier revision of this table recorded eight of these as blocked on 'needs `Intrinsic::ID`', and that reason was wrong").
- **Fix:** Port `getVScaleRange` against `Attribute::VScaleRange { min, max }` — `max: None` maps to upstream's packed-`0` unbounded case — move the symbol from `VALUE_TRACKING_GAPS` to the modeled table (the surface-accounting assertion in `value_tracking_surface_is_accounted_for` keeps the counts honest), and port upstream's `ValueTrackingTest` vscale fixtures.

### 72. SqrtNszSignBit %A3/%A4 unported behind a nofpclass blocker that is closed

*ValueTracking / known-FP-class* — crates/llvmkit-asmparser/tests/known_fp_class.rs:547-607; contradicted by crates/llvmkit-asmparser/tests/parser_nofpclass.rs, crates/llvmkit-ir/src/known_fp_class.rs:150- …

- **LLVM:** `ComputeKnownFPClassTest.SqrtNszSignBit` (`llvm/unittests/Analysis/ValueTrackingTest.cpp`) declares `float nofpclass(nan) %arg.nnan` and adds two more blocks: `%A3 = call float @llvm.sqrt.f32(float %arg.nnan)` expecting `fcPosInf | fcPosNormal | fcZero | fcQNan` and `%A4` the `nsz` variant.
- **llvmkit:** Only `%A` and `%A2` are ported. The doc says the rest "need a `nofpclass(nan)` parameter attribute, which llvmkit does not model". **That is stale.** `nofpclass` is fully modeled: every component keyword round-trips in `parser_nofpclass.rs`, `Attribute::NoFpClass(mask)` prints in `NoFPClassName` order, and `known_fp_class.rs`'s `no_fp_class_of` explicitly ports "`CallBase::getRetNoFPClass` for a call, and `Argument::getNoFPClass` for a parameter" as the opening read of `KnownNotFromFlags`.
- **Why:** Unrecorded — the reason predates the `nofpclass` work and was not revisited. `docs/future-work.md` is cited as holding it, so the entry there is stale too.
- **Fix:** Restore the parameter `float nofpclass(nan) %arg.nnan` to the fixture IR and add the `%A3`/`%A4` blocks with upstream's two exact masks (`fcPosInf|fcPosNormal|fcZero|fcQNan` for both the with-flags and without-flags reads of `%A3`). Then drop the stale claim from the doc comment and from `docs/future-work.md`.

### 73. flags.ll vector trunc functions unported behind a closed blocker

*parser (instruction modifiers)* — crates/llvmkit-asmparser/tests/parser_modifiers.rs:76-98; UPSTREAM.md:882-883; contradicted by crates/llvmkit-asmparser/src/ll_parser.rs:11600-11615 and crates/llvmkit-as …

- **LLVM:** `test/Assembler/flags.ll` carries `@test_trunc_signed_vector`, `@test_trunc_unsigned_vector`, `@test_trunc_both_vector` and `@test_trunc_both_reversed_vector`, whose CHECK lines pin `trunc nuw nsw <2 x i64> %a to <2 x i32>` and the reversed spelling printing canonically.
- **llvmkit:** Only the scalar `@test_trunc_both` / `@test_trunc_both_reversed` are ported, with the reason "the upstream vector form needs vector int-cast support, which parse_int_cast lacks". **`parse_int_cast` has that support.** Its vector branch builds `IntCastFlags` carrying `nuw`/`nsw`/`nneg` and routes through `int_cast_erased`, and `parser_vector_casts.rs` already round-trips `%t = trunc nuw nsw <2 x i32> %x to <2 x i16>`.
- **Why:** Unrecorded — the reason survived the vector-cast work unchanged, and is mirrored verbatim into two `UPSTREAM.md` rows.
- **Correction from verification:** Accurate, but understated. The four vector trunc functions from test/Assembler/flags.ll (@test_trunc_signed_vector, @test_trunc_unsigned_vector, @test_trunc_both_vector, @test_trunc_both_reversed_vector) are indeed unported behind a blocker reason that is false: parse_int_cast fully supports vector integer casts with nuw/nsw/nneg.
- **Fix:** Vendor the four vector functions into the two `fixtures/upstream/flags/*.ll` files as upstream spells them, extend the `assert_check_lines` lists, and correct the doc comments and both `UPSTREAM.md` rows.

### 74. opaque-ptr-invalid-forward-ref.ll vendored but wired to nothing

*parser corpus / forward references* — crates/llvmkit-asmparser/tests/fixtures/upstream/opaque-ptr-invalid-forward-ref.ll (unreferenced); docs/future-work.md:137-141

- **LLVM:** `test/Assembler/opaque-ptr-invalid-forward-ref.ll` CHECKs `invalid forward reference to function 'f' with wrong type: expected 'ptr' but was 'ptr addrspace(1)'` for `@a = alias void (), ptr addrspace(1) @f` against a `define void @f()`.
- **llvmkit:** The fixture file is checked in but referenced by **no test and no manifest entry** — it neither runs nor xfails. `docs/future-work.md` names it as "vendored and waiting" on three per-site forward-reference texts left over from W2.5.
- **Why:** Recorded, in `docs/future-work.md`: it needs `invalid forward reference to function '<n>' with wrong type: expected 'T' but was 'U'` plus `type of definition and forward reference of '@N' disagree` and the global/alias twins — all comparing types at the forward-reference site.
- **Fix:** Implement the per-site type comparison in the forward-reference resolution path so the three texts are produced verbatim, then add the fixture to `parser_corpus_manifest.txt` (or to a `parser_forward_refs.rs` case asserting its CHECK line). Until then, give it a manifest row with an explicit status rather than leaving it inert.

### 75. dbg-record-invalid-5.ll vendored with no reference and no recorded blocker

*parser corpus / debug records* — crates/llvmkit-asmparser/tests/fixtures/upstream/dbg-record-invalid/dbg-record-invalid-5.ll (unreferenced);

- **LLVM:** `test/Assembler/dbg-record-invalid-5.ll` tests that a basic block containing *only* a debug record is a parse error, CHECKing `expected instruction opcode` at the closing brace.
- **llvmkit:** The fixture is checked in and referenced **nowhere** — no test, no manifest row, no `UPSTREAM.md` row, and no entry in `docs/future-work.md`. Its siblings `-1/-2/-3/-4/-6/-7/-8` all have tests, and `-0` is exercised via a manifest copy (`corpus_dbg_record_after_terminator_invalid.ll`, `status=xfail-parse`).
- **Why:** Unrecorded. Nothing in the tree states why this one split was skipped while its seven siblings were ported.
- **Correction from verification:** The coverage gap is real and still present, but the title's "no recorded blocker" is misleading: there is no blocker. Accurate statement: `crates/llvmkit-asmparser/tests/fixtures/upstream/dbg-record-invalid/dbg-record-invalid-5.ll` is vendored verbatim from upstream and git-tracked (committed in f68bee0, LLParser parity W11) but reference …
- **Fix:** Add a `DBG_RECORD_INVALID_5` const beside the others in `parser_debug_metadata.rs` and assert `expected instruction opcode`; if llvmkit answers something else, that is the finding to record. Also fold the now-duplicate `upstream/dbg-record-invalid/dbg-record-invalid-0.ll` (unreferenced) into the manifest row that currently points at a private copy.

### 76. Unported APInt/APFloat upstream tests for unmodeled surface

*ADT ports* — crates/llvmkit-ir/tests/ap_int_upstream.rs:5-9,390-396; crates/llvmkit-ir/tests/ap_int_upstream_ops.rs:82-87;

- **LLVM:** `llvm/unittests/ADT/APIntTest.cpp` covers `GCD`, `SolveQuadraticEquationWrap`, `clmul`, the rotate family and the `tc*` word-level primitives; `APFloatTest.cpp` covers `Float8*`, `Float6*`, `Float4E2M1FN` and `FloatTF32` semantics and asserts through `classify() -> FPClassTest`.
- **llvmkit:** Those tests are not ported. `ap_int_upstream.rs` states the APIs do not exist; `ap_float_upstream_predicates.rs` omits the unmodeled semantics and notes each `classify()` line as unmodeled rather than approximating it with the coarser `ApFloatCategory`. `TEST(APIntTest, nearestLogBase2)`'s final `APInt(UINT32_MAX, 0)` row is deliberately dropped (half a gigabyte to re-check an answer an adjacent row already checks).
- **Why:** Recorded in every case, at the module-doc level, with the missing API or semantics named. `docs/future-work.md` holds the APFloat-string remainder.
- **Correction from verification:** A residue of unported ADT tests is still real, but the claim's specifics are substantially stale on both halves. STILL TRUE: - `ApFloatSemantics` (crates/llvmkit-ir/src/ap_float.rs:14-22) has exactly seven variants (IeeeHalf, Bfloat, IeeeSingle, IeeeDouble, IeeeQuad, X87DoubleExtended, PpcDoubleDouble).
- **Fix:** Lowest-cost first: add `FpClassTest`-grained `classify()` to `ApFloat` (the mask type already exists in `llvmkit-ir` for `nofpclass`/known-FP-class) and restore the omitted `classify()` assertions. Then port the three `APFloat` string fixtures verbatim. The `Float8*`/`Float6*`/`FloatTF32` semantics and the `APInt` primitives are genuine model additions, each independent.

### 77. `DIFlags` / `DISPFlags` are stored as joined source text, not bitfields

*metadata model* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_metadata_field_value`), crates/llvmkit-ir/src/metadata.rs

- **LLVM:** `DINode::DIFlags` and `DISubprogram::DISPFlags` are `uint32_t` bitfields with `getFlag`/`getFlagString`/`splitFlags`; `AsmWriter::printDIFlags` emits them with a `ListSeparator(" | ")`.
- **llvmkit:** The parsing half landed (the `|`-joined form is read, mirroring upstream's repeated `lltok::bar` loop) but the written disjunction is kept as one `Enum(String)` field value, so `DIFlagPublic | DIFlagPrototyped` round-trips as text and never becomes a set. Printed bytes agree today because the separator matches.
- **Why:** Recorded in docs/future-work.md: same milestone and same reason as the DWARF tables — the bitflag type is only worth its keep once something reads it.
- **Correction from verification:** The claim is accurate as to the model divergence, but its last sentence over-claims. "Printed bytes agree today because the separator matches" holds only for input that is already in upstream's canonical form.
- **Fix:** Introduce the two bitflag types with `split_flags`/`flag_string` ports and store the decoded set; printing then derives the `|` list instead of echoing source text.

### 78. `TempDIAssignIDAttachments` RAUW machinery is absent

*metadata parser* — crates/llvmkit-asmparser/src/ll_parser.rs:5344-5350 (the only DIAssignID site)

- **LLVM:** `LLParser` collects instructions carrying a forward-referenced `!DIAssignID` attachment in `TempDIAssignIDAttachments` and RAUWs the temporary nodes when the real node is defined (`validateEndOfModule`).
- **llvmkit:** Only the `missing 'distinct', required for !DIAssignID()` rejection exists (landed W7 part 5); no DIAssignID-specific attachment RAUW appears in the parser — the generic metadata forward-reference map is all there is. Verified by grep: `DIAssignID` occurs in ll_parser.rs only around the distinct check.
- **Why:** Recorded as still open at W7 ("the `TempDIAssignIDAttachments` RAUW machinery … is metadata-layer work and belongs with W11"). W11's own completion notes do not record it landing, so the reason it is still open is **unrecorded** — treat the routing as a hypothesis, not a finding.
- **Correction from verification:** Still present, with two corrections to the description. (1) Upstream's RAUW drain lives in `LLParser::parseStandaloneMetadata`, not `validateEndOfModule` — `TempDIAssignIDAttachments` has exactly three sites in LLVM 22.1.4 (LLParser.h declaration, push in `parseInstructionMetadata`, drain in `parseStandaloneMetadata`) and `validateEndOfMo …
- **Fix:** First verify whether llvmkit's generic metadata forward-reference resolution already covers the attachment case; if not, add the pending-attachment map drained at end of module (law 8c: the map owns linear handles, draining consumes them).


### 80. No `ParserConfig`: `AllowIncompleteIR`, the DataLayout callback and the UpgradeDebugInfo flag are unmodelled

*parser — entry points* — crates/llvmkit-asmparser/src/parser.rs:209,:230,:252,:288

- **LLVM:** `parseAssembly*` takes a `DataLayoutCallbackTy` and an `UpgradeDebugInfo` flag, and `AllowIncompleteIR` (a `cl::opt`) enables auto-declaration of non-intrinsic undefineds, `dropUnknownMetadataReferences` and TBAA-drop tolerance.
- **llvmkit:** No `ParserConfig` / `DataLayoutCallback` / `allow_incomplete_ir` symbol exists anywhere in `crates/llvmkit-asmparser/src` (verified by grep); the entry points are `parse_assembly`, `parse_assembly_file`, `parse_assembly_with_index` (landed W10) and `parse_assembly_with_context`, all with fixed behaviour.
- **Why:** Recorded as W13 items with annex A7 giving the exact shape (`ParserConfig` with `*_with_config` twins, plain forms keeping today's defaults). Deferred because both options only become meaningful once the end-of-module machinery and AutoUpgrade exist.
- **Fix:** Add `ParserConfig` per annex A7 plus `*_with_config` twins, thread the DataLayout callback into the target-definitions phase (W3's restructure), and implement `AllowIncompleteIR`'s three tolerances faithfully, off by default.

### 81. `attr_kind_for_keyword` is a hand-written table, not generated from `Attributes.td`

*parser — attributes* — crates/llvmkit-asmparser/src/ll_parser.rs (`attr_kind_for_keyword`), crates/llvmkit-ir/src/attributes.rs, crates/llvmkit-asmparser/tests/attribute_td_drift.rs, crates/llv …

- **LLVM:** `LLParser::tokenToAttribute` is generated from `llvm/IR/Attributes.td`, so a new attribute cannot be silently missing.
- **llvmkit:** The keyword→`AttrKind` mapping is a hand list. `attribute_td_drift.rs` stands in for generation and has caught five separate holes, but the table itself is still transcribed.
- **Why:** Recorded as carried out of W5: "Given how much that guard has caught, generating the table would be strictly better — carry it." Also note the guard's own reader had four bugs; a measurement taken with a broken reader looks authoritative and is not.
- **Fix:** Extend `llvmkit-tablegen` to emit the keyword→kind table from the vendored `Attributes.td` and keep the drift test as the cross-check (five-step method, steps 1 and 5).

### 82. `inrange` bounds have a second, parallel APSInt reader

*parser — constants* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_inrange_bound` and helpers)

- **LLVM:** `LLLexer` has one APSInt rule — the `[us]0x` active-bit truncation and the signed widening — and every consumer widens from it.
- **llvmkit:** `Parser::parse_inrange_bound` and its six helpers (`ParsedInRangeBound`, `inrange_bound_to_apint_words`, `signed_magnitude_to_apint_words`, `apsint_to_apint_words`, `hex_apsint_bit_width`, `decimal_digits_to_words`, `hex_digits_to_words`) implement the same rule that `parse_int_literal` implements since W5. Both are currently correct.
- **Why:** Recorded in docs/future-work.md (W5): two implementations of one lexer rule is the exact shape this program keeps finding bugs in (three private copies of the scalable-vector walk, none matching `Type::isScalableTy`). Not done in W5 because it sits on the GEP constant-expression path — routed to W9a, which did not take it.
- **Correction from verification:** The claim is accurate in substance but understates the scope and misplaces the root cause. Two refinements: (1) the parallel machinery is larger than "six helpers" — `sign_extend_apint_words`, `negate_apint_words`, `mask_apint_top_word`, `apint_active_bits`, `mul_add_words`, `low_u64`, plus `signed_apint_cmp`/`unsigned_apint_cmp`/`apint_s …
- **Fix:** Collapse `parse_inrange_bound` onto `parse_int_literal` + `ParsedApsInt::extend_or_truncate`; the only real work is that `ConstantExprInRange::new` takes `Box<[u64]>` rather than an `ApInt`. Keep `parser_constants.rs::constant_expr_gep_inrange_signed_hex_active_bits_are_preserved` green.

### 83. Three copies of the aggregate index walk

*IR model* — crates/llvmkit-ir/src/instructions.rs (`indexed_aggregate_type`), crates/llvmkit-ir/src/ir_builder.rs (`walk_aggregate_for_builder`), crates/llvmkit-ir/src/verifier.rs (` …

- **LLVM:** `ExtractValueInst::getIndexedType` is one routine, used by the parser, the builder and the verifier alike.
- **llvmkit:** Three implementations: the public port `llvmkit_ir::indexed_aggregate_type` (added W9c for `invalid indices for {extract,insert}value`), `ir_builder.rs::walk_aggregate_for_builder`, and `verifier.rs::walk_aggregate_path` (which additionally distinguishes *why* the walk failed via `AggWalkErr`). All three currently agree.
- **Why:** Recorded in docs/future-work.md (W9c): not urgent, but "a predicate with three implementations is one diagnostic away from having three behaviours". Consolidation is not a pure deletion because the verifier needs a richer return type than `Option`.
- **Correction from verification:** The divergence is real and still present as described, with one refinement: the closing sub-claim "All three currently agree" has a reachable counterexample. `verifier.rs::walk_aggregate_path` narrows the array length with `u32::try_from(*n).unwrap_or(u32::MAX)` before comparing, while `indexed_aggregate_type` and `walk_aggregate_for_buil …
- **Fix:** Give the public port a result type that carries the failure reason, then delete the two private near-copies and re-point their callers.

### 84. Private `Type`-predicate duplicates were never consolidated onto the public ports

*IR model* — crates/llvmkit-asmparser/src/ll_parser.rs; crates/llvmkit-ir/src/{constants.rs, intrinsics.rs, ir_builder/constant_folder.rs, assumptions.rs, implied_conditions.rs}

- **LLVM:** `Type::isSized`, `Type::isScalableTy`, `CastInst::castIsValid` and the `isValid*Type` family are each one routine in `llvm/IR/Type.cpp` / `Instructions.cpp`.
- **llvmkit:** W4 (`a8b619c`) landed seven public `Type` predicates plus `cast_is_valid`, but private near-copies remain: `is_int_or_int_vector_type` and `is_ptr_or_ptr_vector_type` local in `ll_parser.rs`, plus duplicates recorded in `constants.rs`, `intrinsics.rs`, `ir_builder/constant_folder.rs`, `assumptions.rs`, `implied_conditions.rs`.
- **Why:** Recorded under "follow-ups deliberately not folded into parity commits". W4.5 had to fix the three `type_contains_scalable_vector` copies because they were all wrong (missing the target-extension arm, the cycle guard, or recursing where upstream does not) — the remaining copies were left because they were not load-bearing for a diagnostic …
- **Correction from verification:** Still present, but the upstream framing is wrong and the file list needs two corrections. The four upstream routines the claim names as the point of comparison are precisely the ones llvmkit did NOT duplicate. Each has exactly one definition in the tree and no private near-copies: Type::isSized -> Type::is_sized (type.rs:604);
- **Fix:** Delete each private copy in favour of the public predicate, checking each against its upstream counterpart first — the W4 lesson is that a predicate llvmkit already has may be answering a different question.

### 85. `ConstantRangeList` set operations are not ported

*IR model* — crates/llvmkit-ir/src/constant_range_list.rs

- **LLVM:** `llvm/IR/ConstantRangeList.h` provides `subtract`, `unionWith` and `intersectWith` alongside `insert`.
- **llvmkit:** Only `isOrderedRanges`, `getConstantRangeList`, `print` and `insert` (with its `int64_t` overload) are ported. Three of the six `unittests/IR/ConstantRangeListTest.cpp` cases (`Subtract`, `Union`, `Intersect`) are therefore unportable.
- **Why:** Recorded in docs/future-work.md (W5, decided 2026-08-12): no consumer anywhere in llvmkit — upstream's callers are Attributor- and `MemoryLocation`-style passes this tree has not ported — so porting would add public API with no in-tree user and no way to be sure it stays right.
- **Fix:** Land the three methods together with their first real caller, taking the three unit tests in the same commit. One detail to carry: `insert`'s no-op check uses `ConstantRange::contains` (unsigned) while everything else in the class compares signed; llvmkit reproduces the inconsistency, so read a `subtract` port against upstream's comment, not against `insert`.

### 86. The per-operand `elementtype` half of `verifyInlineAsmCall` is unported

*verifier / call surface* — crates/llvmkit-ir/src/inline_asm.rs, crates/llvmkit-ir/src/verifier.rs, crates/llvmkit-ir/src/ir_builder.rs (call surface)

- **LLVM:** `Verifier::verifyInlineAsmCall` checks per-operand `elementtype` attributes against the constraint records, in addition to the label rules W4 ported.
- **llvmkit:** The label rules and all nine `InlineAsm::verify` messages are reachable, but the `elementtype` half is absent.
- **Why:** Recorded in docs/future-work.md (W4): the call surface cannot spell per-operand `elementtype` attributes yet, so the check has nothing to read. The `Flag` / `ConstraintCode` bit encodings from the same header are recorded as backend serialization and deliberately out of scope.
- **Fix:** Grow the call-building surface to carry per-operand attribute sets, then port the `elementtype` arm of `verifyInlineAsmCall`.

### 87. `AllocationType` models the four keyword values, not upstream's OR-able set

*IR model — summary index* — crates/llvmkit-ir/src/module_summary_index.rs

- **LLVM:** `enum class AllocationType : uint8_t` has `None = 0`, `NotCold = 1`, `Cold = 2`, `Hot = 4`, `All = 7`, powers of two precisely so a context reaching an allocation more than one way can OR them; `AllocInfo::Versions` is a `SmallVector<uint8_t>` for that reason.
- **llvmkit:** The enum models exactly the four keywords `LLParser::parseAllocType` reads.
- **Why:** Recorded in docs/future-work.md (W10) as deliberate: `AssemblyWriter::printFunctionSummary`'s `AllocTypeName` lambda handles those four and `llvm_unreachable`s on anything else, so an ORed value can only come from bitcode, which llvmkit does not have. "The enum is the `.ll` surface, exactly."
- **Fix:** Nothing until llvmkit gains bitcode; revisit with the bitcode reader, at which point `AllocationType` becomes a masked newtype rather than an enum.

### 88. No token-set drift test outside attributes, calling conventions and summary keywords

*lexer* — crates/llvmkit-asmparser/src/ll_lexer/keywords.rs, crates/llvmkit-asmparser/src/ll_token.rs, crates/llvmkit-asmparser/tests/

- **LLVM:** `LLLexer.cpp`'s keyword and token tables define the accepted vocabulary; a keyword llvmkit does not know is silently mis-parsed rather than reported.
- **llvmkit:** `attribute_td_drift.rs` and `calling_conv_drift.rs` cross-check two families and the summary keywords were verified by inventory; the instruction keywords and the misc `kw_` families have never been mechanically diffed against upstream.
- **Why:** Recorded as a W14 item. The program's own W6 lesson is the reason it matters: "a table nothing cross-checks is wrong" — 28 calling conventions parsed as `ccc` and printed back wrong, invisibly, because every test used a convention that already worked.
- **Correction from verification:** Core gap is REAL and still present: nothing in the tree mechanically diffs the lexer's instruction (`INSTKEYWORD`) or misc `KEYWORD` families against upstream. Three details in the claim are wrong or undercounted, though: (1) "two families" undercounts the existing guards.
- **Fix:** Mechanically diff `LLLexer.cpp`'s tables against `keywords.rs` + `ll_token.rs` for every remaining family, and add a drift test wherever a vendorable source exists (note `orig_cpp/` is gitignored — anything a test reads must be vendored under `crates/llvmkit-asmparser/tablegen/llvm-22.1.4/`).

### 89. The corpus manifest covers 9 fixtures against 500 upstream `test/Assembler` files

*tests / corpus* — crates/llvmkit-asmparser/tests/parser_corpus_manifest.txt, crates/llvmkit-asmparser/tests/parser_corpus.rs, crates/llvmkit-asmparser/tests/fixtures/upstream/

- **LLVM:** n/a — this is llvmkit's completeness proof, not an upstream behaviour. Upstream's `test/Assembler` holds ~500 `.ll` files, 257 of them negatives (170 pinning message text) and ~175 round-trips.
- **llvmkit:** `parser_corpus_manifest.txt` carries 9 entries; the inventory counted 124 fixtures on disk with 115 unmanaged. Individual waves have ported their own clusters into per-wave test files.
- **Why:** Recorded as W14's "mass fixture port — the proof": classify every upstream fixture as `ported` / `blocked-model` (must be empty by then, else the gap goes back to a wave) / `N/A` with a one-line rationale.
- **Correction from verification:** The headline divergence is REAL and STILL PRESENT, but the cited path is wrong and the on-disk counts are stale. Corrected statement: "The corpus manifest at `crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt` (NOT `tests/parser_corpus_manifest.txt` as cited) carries exactly 9 entries, and has not been touched since commi …
- **Fix:** Expand the manifest to the full round-trip set and port the remaining negatives with exact `to_string()` assertions plus UPSTREAM.md rows. Two standing traps: an upstream `CHECK` block is a *pipeline's* output (check the `RUN` line before treating a mismatch as a bug), and an `xfail` reason is a hypothesis (unblocking one has twice revealed an unrelated defect).

## Coverage, tooling and provenance

Nothing changes for a well-formed module; these are gaps in what is measured, guarded or recorded.

### 90. `parse_optional_refs` sorts stably where upstream's sort is unstable

*parser (LLParser) / module summary index* — crates/llvmkit-asmparser/src/ll_parser.rs:3726, reason at crates/llvmkit-asmparser/src/ll_parser.rs:3706-3711

- **LLVM:** `LLParser::parseOptionalRefs` sorts the parsed references by access specifier with `llvm::sort`, which is `std::sort` — unstable, and deliberately shuffled first under `EXPENSIVE_CHECKS`. Ties (references sharing an access class) therefore end up in an unspecified order, and `FunctionSummary::specialRefCounts` and the printer only rely on read-only/write-only sitting at the end.
- **llvmkit:** `parsed.sort_by_key(|(reference, _)| reference.value.access)` — Rust's stable sort — so ties keep source order. For a `refs:` list with several references of one access class, llvmkit's printed order is deterministic where upstream's is not.
- **Why:** Recorded inline at `ll_parser.rs:3706-3711`. No reason is given for preferring stability beyond it being what `sort_by_key` does; the note frames it as an observation rather than a decision.
- **Fix:** Low-stakes but worth resolving explicitly. Since upstream's order is *unspecified* rather than different, llvmkit's stable order is a legal refinement — so the fix is documentation, not code: restate the comment as "upstream leaves ties unspecified; llvmkit pins source order, which is one of the orders `llvm::sort` may produce", and add the `UPSTREAM.md` row.

### 91. DIExpression oversized-element test asserts nothing, on a stale message claim

*parser (debug metadata)* — crates/llvmkit-asmparser/tests/parser_debug_metadata.rs:372-387 (stale doc + discarding assertion);

- **LLVM:** `test/Assembler/invalid-diexpression-large.ll` accepts an element of exactly `UINT64_MAX` (`CHECK-NOT: error:`) and rejects one above it with `element too large, limit is 18446744073709551615` from `LLParser::parseDIExpressionBody`.
- **llvmkit:** The test's doc says "Same logic as upstream, different diagnostic: … llvmkit reports the structured `Expected` error its parser uses throughout, so this asserts on the accept/reject behaviour rather than on message text", and the body discards the error entirely: `let _ = parse_err("…18446744073709551616…");`. **The message exists.** `ll_parser.rs` emits `format!("element too large, limit is {}", u64::MAX)`, and a *different* test in the same file already asserts it verbatim.
- **Why:** Unrecorded — the message landed later and this doc/body pair was not revisited; the divergence excuse outlived the divergence.
- **Correction from verification:** Still present, with one wording fix: the test does not "assert nothing". `parse_err` (crates/llvmkit-asmparser/tests/parser_debug_metadata.rs:152-158) ends in `.expect_err("parse must fail")`, so `let _ = parse_err("...18446744073709551616...")` at :386 does assert the input is rejected — it discards only the error *value*, i.e.
- **Fix:** Replace `let _ = parse_err(...)` with `assert_eq!(parse_err(...).to_string(), "element too large, limit is 18446744073709551615")` and delete the "different diagnostic" paragraph, so the port asserts the fixture's actual CHECK line.

### 92. return-self-good.ll described as blocked while the manifest runs it as passing

*parser corpus / provenance ledger* — crates/llvmkit-asmparser/tests/parser_constants.rs:859-867; UPSTREAM.md:1170; contradicted by crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt

- **LLVM:** `test/Bitcode/blockaddress-addrspace.ll::return-self-good.ll` uses `target datalayout = "P2"` with `@take_self_prog_as` declaring no address space, so the program address space supplies it.
- **llvmkit:** Two records disagree. `parser_corpus_manifest.txt` lists `upstream/blockaddress-addrspace/return_self_good.ll … status=pass` (and `CHANGELOG.md` records it moving `xfail-parse` -> `pass`), while `parser_constants.rs` and its `UPSTREAM.md` row still say "that fixture is still blocked on the *program* address space … which is W3 work" and route around it by dropping the address spaces.
- **Why:** Unrecorded — the doc was not updated when the fixture started passing.
- **Fix:** Confirm the corpus entry really exercises the whole fixture, then rewrite `same_function_forward_blockaddress_resolves_by_name` to use `upstream/blockaddress-addrspace/return_self_good.ll` directly (or state plainly that the address-space-stripped shape isolates the `BlockAddressPFS` rule), and correct the `UPSTREAM.md` classification from `llvmkit-specific subset` to `port`.

### 93. Bare riscv_vls_cc swallows the following token (upstream bug, reproduced deliberately)

*parser (calling conventions)* — crates/llvmkit-asmparser/tests/calling_conv_drift.rs:172-195

- **LLVM:** `parseOptionalCallingConv`'s `kw_riscv_vls_cc` arm consumes its own keyword and, finding no `(`, `break`s to the switch tail — which consumes a second token. So `declare riscv_vls_cc void @f()` loses its return type. This is upstream's actual behaviour, not its intent.
- **llvmkit:** Reproduced exactly and pinned: `declare riscv_vls_cc void @f()` is an error, and `declare riscv_vls_cc void void @f()` parses at the default ABI_VLEN of 128.
- **Why:** Recorded, and the reasoning is explicit: unreachable from printed IR (`printCallingConv` always writes the parameterised form), reproduced because "the contract is upstream's behaviour", noted in `docs/future-work.md`, and "if upstream fixes it, this test is what says so".
- **Fix:** No action while tracking LLVM 22.1.4 — this is the parity-correct answer. Re-check on the next vendored-tree bump; the test is designed to fail if upstream drops the fallthrough.

### 94. `printCallingConv`'s numeric fallback is deliberately not reproduced

*printer (AsmWriter)* — crates/llvmkit-ir/src/asm_writer.rs (calling-convention printing); crates/llvmkit-asmparser/tests/calling_conv_drift.rs

- **LLVM:** `printCallingConv`'s default arm is `Out << "cc" << cc`, so an unnamed convention prints as `cc11` — which `LLLexer` reads as one unknown identifier, meaning `llvm-as` cannot re-parse `llvm-dis`'s own output for `HiPE`, `AVR_BUILTIN`, `MSP430_BUILTIN`, `WASM_EmscriptenInvoke`, `M68k_INTR` or the two ARM64EC thunks.
- **llvmkit:** llvmkit prints `cc 11` — the spelling upstream's *parser* accepts — so its output round-trips here and remains valid input to `llvm-as`.
- **Why:** Recorded in docs/future-work.md (W6) as a considered decision: "the one place the byte-for-byte printer rule is deliberately broken, and it is broken in the safe direction". Recorded here for completeness because it is a live deviation from `AsmWriter.cpp`'s bytes.
- **Correction from verification:** The divergence itself is real and still present — but its stated justification is false, and the claim should be rewritten as an unjustified byte-level break rather than a "safe-direction" fix. What is true: llvmkit prints the numeric fallback as `cc <N>` (with a space) where upstream prints `cc<N>` (no space).
- **Fix:** Nothing to do unless upstream fixes its printer; revisit if it does. Keep `calling_conv_drift.rs` as the round-trip lock that found it.

### 95. `Can't read textual IR with a Context that discards named Values` is structurally unreachable

*parser — entry points* — crates/llvmkit-asmparser/src/parser.rs; ~/.claude/plans/llparser-parity-ledger.md

- **LLVM:** `LLParser::Run` refuses to parse when the `LLVMContext` has `shouldDiscardValueNames()` set.
- **llvmkit:** llvmkit has no discard-names mode at all, so the condition cannot arise and the message has no home.
- **Why:** Recorded as a W13 decision: classify as N/A-with-rationale in the ledger rather than inventing a trigger. Listed here so the final ledger's `MISSING = 0` target is not chased through this row.
- **Fix:** Mark the ledger row `N/A(structurally-impossible)` with the rationale, in the W14 ledger-v3 pass.

### 96. Two upstream diagnostics are unreachable upstream and are recorded, not invented

*parser — function header/body* — crates/llvmkit-asmparser/src/ll_parser.rs (`parse_argument_list`, `define_numbered_label`); ~/.claude/plans/llparser-parity-ledger.md

- **LLVM:** `argument can not have void type` sits behind `parseArgumentList`'s `AllowVoid = false`, so `parseType` already refuses a literal `void` with `void type only allowed for function results`; `unable to create block numbered '<N>'` sits behind `defineBB`, which runs `checkValueID` first, so a colliding numbered label has already failed.
- **llvmkit:** Neither message exists, matching upstream's reachable behaviour exactly.
- **Why:** Recorded in W8: "recorded rather than given invented triggers". Listed so the ledger's MISSING rows for these two are not treated as gaps.
- **Correction from verification:** Still present as a code fact — neither message exists in llvmkit — but the claim's rationale is half wrong, so this is one accurate recording plus one genuine unclosed gap, not two faithful matches. ACCURATE: `argument can not have void type` (LLParser.cpp:3401) is genuinely dead.
- **Fix:** Mark both ledger rows `N/A(unreachable-upstream)` in the W14 ledger-v3 pass.

### 97. `test/Assembler/thinlto-vtable-summary2.ll` and `invalid-name*.ll` are unportable fixtures

*tests / fixtures* — crates/llvmkit-asmparser/tests/parser_summary.rs, crates/llvmkit-asmparser/tests/fixtures/upstream/

- **LLVM:** `thinlto-vtable-summary2.ll` runs `opt %s -S -module-summary`, which *generates* a summary index from the module's type metadata; there is no `^N` block in the input. `invalid-name*.ll` are binary files with embedded NUL bytes pinning lexer name handling.
- **llvmkit:** Neither is ported. The other sixteen summary fixtures are ported in `parser_summary.rs`; the metadata negatives around `invalid-name*.ll` are ported.
- **Why:** Recorded in docs/future-work.md and in W7 part 6 respectively: the first is the module-summary *analysis*, not the parser, and llvmkit has neither; the second cannot be expressed as a Rust string literal, and it was recorded rather than skipped silently.
- **Correction from verification:** Substantially accurate and still present, with three wording corrections. (a) The `thinlto-vtable-summary2.ll` half is exact. Upstream's RUN line is `opt %s -S -module-summary | FileCheck %s`;
- **Fix:** Leave the first as N/A until a module-summary analysis exists. For the second, load the fixture from a checked-in binary file (`include_bytes!`) rather than a literal if the coverage is judged worth it; otherwise keep the N/A row with its rationale.

### 98. The ledger's `present` column is a string proxy and its Twine column undercounts by construction

*measurement / ledger* — ~/.claude/plans/llparser-parity-ledger.md, ~/.claude/plans/llparser-tools/ledger_v2.py

- **LLVM:** n/a — measurement infrastructure for the parity program. The denominator is 516 exact messages (`error` 221, `parseToken` 200, `tokError` 90, `checkValueID` 7, `parseValueAsMetadata` 2) plus 84 Twine templates and 11 lexer messages.
- **llvmkit:** `ledger_v2.py` globs every string literal under `crates/llvmkit-asmparser/src`, so `present` proves the text exists in the sources — never that a code path reaches it (W7's lesson: `redefinition of global '@x'` had a variant, a `Display` and a unit test and never fired). The Twine-fragment column matches fragments literally while llvmkit builds those texts with `format!`, so correctly-ported messages read unticked.
- **Why:** Recorded in both documents. Current standing: 411 present / 105 missing of 516 at the W11 boundary. W14's ledger v3 targets `MISSING = 0`, every row EXACT-with-a-citing-test or `N/A(rationale)`.
- **Correction from verification:** Substantially accurate; two refinements, one of which makes the defect worse than claimed. (1) Scope imprecision: `ledger_v2.py` does not glob only `crates/llvmkit-asmparser/src`.
- **Fix:** Regenerate before quoting any count — `python ledger_v2.py <orig_cpp/llvm-project-llvmorg-22.1.4> <workspace root, NOT src> <ledger path>`; passing `src` yields `present=0` silently. Read `present` as a ceiling on parity, and treat only the 516 exact-literal column as a scoreboard.

### 99. UPSTREAM.md provenance debt: 469 tests with no row

*tests / provenance* — UPSTREAM.md, crates/llvmkit-ir/tests/, crates/llvmkit-ir/src/

- **LLVM:** n/a — D11 house law: every `#[test]` cites its upstream source and gets an `UPSTREAM.md` row in the same commit.
- **llvmkit:** 2435 tests against 1994 rows, leaving 469 tests unrowed (measured at the W7 boundary as 470 tests / 1930 rows, plus 26 rows naming tests deleted long ago, removed in that recount).
- **Why:** Recorded in both documents and in `UPSTREAM.md`'s own header: the debt is inherited from the type-safety and pass-API programs — it sits in `llvmkit-ir` (`verifier_module_flags.rs`, `analysis_preservation.rs`, `module_brands.rs`, `id_roundtrip.rs`, `phi_raw_tests/`, `src/pass_context.rs`, `src/fp_class.rs`), not in the parser crates.
- **Correction from verification:** Substantively accurate and still present, with two precision fixes. (1) "1994 rows" is the distinct-reference count, not the literal row count: UPSTREAM.md carries 1995 data rows, one of which -- the file-scoped `crates/llvmkit-ir/tests/verifier_module_flags.rs` row -- appears twice verbatim. Moreover only 1961 rows name a test function;
- **Fix:** W14's recount item: enforce the per-wave rule (a row in the same commit) and clear the backlog file by file. A missing row means missing *provenance*, never "no upstream counterpart" — so each backfill has to name a real source or say explicitly that the test is llvmkit-specific.

---

## How to use this file

- Adding a row: verify it first, cite the upstream symbol, and give a fix
  sketch concrete enough to act on.
- Closing a row: delete it, and say in the commit which fixture now passes.
- A row with no fixture behind it is a hypothesis; say so in the entry.
