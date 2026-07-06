# IRBuilder Type-Safety Hardening + Typed Calls + Auto-SSA — Design Spec

Date: 2026-07-05. Status: approved.

## Context

llvmkit is a from-scratch Rust reimplementation of LLVM IR APIs whose main differentiator is type safety (Doctrine D1-D11 in AGENTS.md). This session pushes the IRBuilder — the crate's centerpiece — from "strongly typed" to "the most type-safe IR construction API possible", with breaking changes explicitly welcome, plus distinctive killer features that make llvmkit "LLVM 2.0 on steroids" without drifting from LLVM semantics. The long-term goal is replacing upstream LLVM for IR construction workflows.

Scope:

1. **Type-safety hardening sweep** of `ir_builder.rs` and the infrastructure it references (folder, lift traits, instruction handles).
2. **End-to-end typed calls** (flagship killer feature): callee signatures carried in the type system so wrong-arity/wrong-type/wrong-return calls become compile errors.
3. **Auto-SSA frontend** (second killer feature): Cranelift-style typed variables with Braun et al. on-the-fly phi construction, eliminating manual phi wiring.
4. A **future-work document** capturing everything found but not done this session (macro DSL, rustc-quality diagnostics, upstream coverage gaps, ergonomics).

**Organizing principle: the repo is built around the typestate pattern.** Every state machine in llvmkit is already a family of types, not a runtime flag: `IRBuilder<Positioned|Unpositioned>`, `BasicBlock<Sealed|Unsealed>`, `PhiInst<Open|Closed>`, `SwitchInst<Open>`, `Instruction<Attached|Detached>`, `Module<Verified|Unverified>`, `StructType<Opaque|BodySet>`. Every design below is judged by one question first: does it extend the typestate story or fight it? Typed calls carry the callee signature as a type parameter; the auto-SSA layer models Braun sealing as state (typestate where the predecessor set allows it, runtime-checked state with typed errors where it cannot — justified explicitly); the typed folder moves fold-result trust from a runtime re-check into the type of the fold method. New runtime predicates are accepted only where the checked property is genuinely dynamic (parsed IR, dyn paths), and each one must have a typed twin per D3.

Existing foundation to build on (not rebuild): `crates/llvmkit-macros/` already ships `#[derive(IrStruct)]` (struct schemas, generated `<Struct>Value<'ctx, B>` wrappers, typed field accessors via `StructSchema`/`IrField`), and `function_signature.rs` already ships `TypedFunctionValue` tuple-schema facades.

Constraints: stable Rust 2024, `#![forbid(unsafe_code)]`, static dispatch only (no `dyn`), sealed traits, no `as` casts, no panics in production paths, `_dyn` fallbacks for every typed form (D3), tests ported or explicitly example-locked (D11), AsmWriter output stays byte-for-byte LLVM-parity, rust-analyzer LSP for cross-file refactors.

## Audit findings driving the design

- `build_call` accepts erased args with no arity/type checking; `CallInst<R2>`'s return marker is caller-asserted, not derived from the callee (ir_builder.rs ~3321-3580). Biggest hole.
- `build_load` and `build_gep` return fully erased `Value` even when the caller supplied a typed element type; GEP's computed element type is discarded.
- All 17 `IRBuilderFolder` methods return erased `IrResult<Option<Value>>`, re-narrowed at runtime via `checked_folded_value` (ir_builder.rs ~5782); `NoFolder` must stub all 17 methods.
- Per-opcode handles are asymmetric: `CallInst<R>`, `PhiInst<W,P>`, `SwitchInst<Open>` are typed/typestated; `AddInst`, `LoadInst` carry nothing.
- Flags existing only on `_dyn` variants: `samesign` (icmp), `nneg` (uitofp/zext).
- Misc doctrine violations: `as` casts in the `const_int_raw` chain (int_width.rs), `unwrap_or(usize::MAX / u32::MAX)` sentinels (ir_builder.rs ~4478, ~6323, ~6336), `unwrap_or(fn_ty)` in build_invoke (~5125, ~5278).

### Verified upstream facts (web-grounded against llvmorg-22.1.4)

- `samesign` (icmp): upstream IRBuilder has NO samesign parameter — set post-hoc via `ICmpInst::setSameSign`; `CmpPredicate` bundles predicate+samesign for analysis APIs. llvmkit's construction-time flag is a deliberate Rust-side improvement; typed variants keep construction-time semantics.
- `nneg` (zext, uitofp): upstream is a trailing `bool IsNonNeg = false` on `CreateZExt`/`CreateUIToFP`, applied post-creation and silently dropped on the folding path.
- trunc `nuw`/`nsw`: trailing bools on `CreateTrunc`; upstream silently drops the flags when src type == dest type. llvmkit must NOT silently drop flags (D10).
- `or disjoint`: trailing `bool IsDisjoint` on `CreateOr` (no separate method upstream).
- GEP: `CreateGEP(..., GEPNoWrapFlags NW)`; `CreateStructGEP` uses `GEPNoWrapFlags::inBounds() | noUnsignedWrap()` — verify llvmkit's `build_struct_gep` emits the same flags.
- `ptrtoaddr` (new in LLVM 22): `CreatePtrToAddr` takes NO destination type — result derives from DataLayout's address type. Verify llvmkit's `build_ptr_to_addr` matches.
- FMF: upstream has `FMFSource` and an FMF-variant family incl. `CreateSelectFMF`, `CreateFPTruncFMF`, `CreateFPExtFMF` — future work for llvmkit.
- Folder: upstream `IRBuilderFolder.h` has exactly 17 pure-virtual methods matching llvmkit's trait 1:1; the typed-folder redesign is a deliberate, documented Rust-side divergence.
- LLVM 21/22 changes to double-check during hardening: `atomicrmw fmaximum/fminimum` (21), `mul` constant expression removed (21), `ConstantData` use-lists no longer inspectable upstream, lifetime intrinsics lost their size argument (22).

## Design: Workstream 0 — Block-state rename (prerequisite)

llvmkit's `BasicBlock` `Sealed` marker means "terminator emitted", which collides fatally with Braun-SSA's "sealed = all predecessors known". Resolve before anything else lands:

- `block_state.rs`: `BlockSealState` → `BlockTerminationState`, `Sealed` → `Terminated`, `Unsealed` → `Unterminated`, `IS_SEALED` → `IS_TERMINATED`; `BasicBlock::retag_seal` → `retag_termination`; generic param `Seal` → `Term`.
- `ir_builder.rs` aliases: `SealedBlockInst` → `TerminatedBlockInst` (+ `Switch`/`IndirectBr`/`Invoke`/`CatchSwitch` siblings).
- Promote the private builder-state seal to `pub trait BuilderPositionState: state_sealed::Sealed` (implementors `Positioned`/`Unpositioned` unchanged).
- ~111 occurrences across ~20 llvmkit-ir files + 13 in llvmkit-asmparser: LSP rename, not regex. Two compile-fail fixtures rename with regenerated `.stderr` goldens.
- `Terminated` chosen over Cranelift's "Filled" because llvmkit mirrors LLVM vocabulary; rustdoc notes the Cranelift synonym.
- No behavior change; no printed-IR change.

## Design: Workstream 1 — End-to-end typed calls

**Problem.** `build_call` accepts erased args with zero arity/type checking (verifier-only), and `CallInst<R2>`'s marker is caller-asserted. Also found: `build_indirect_call`/`build_inline_asm_call`/`build_inline_asm_invoke_with_config` construct `ReturnTypeMismatch { expected, got }` with the SAME value in both fields — a real bug fixed here.

**Core design** (in `function_signature.rs` unless noted):

1. **`IntoCallArg<'ctx, P: FunctionParam, B>`** — sealed per-position lift trait, `#[diagnostic::on_unimplemented]` for readable errors. Macro-generated impls per concrete marker over the existing lift traits, a `Width<N>` const blanket, a `Ptr` blanket, StructSchema concrete-source impls, and an IrStruct-derive-emitted impl for generated wrappers. Coherence proven by precedent: `IntoIrField` (struct_schema.rs:265-496) ships this exact impl topology today (verified).
2. **`CallArgs<'ctx, Params: FunctionParamList, B>`** — sealed tuple trait, macro arities 0..=16; position `i` bounded by `IntoCallArg<P_i>`; `lower(self, module) -> IrResult<Box<[ValueId]>>`. Wrong arity = no impl = compile error; wrong position type = failed bound = compile error. Plus a `StructFields<S>` delegation impl.
3. **Derived return**: `FunctionReturn` gains GAT `type CallResult<'ctx, B>` (`()` for void, `IntValue<'ctx, W, B>` for ints, `S::Value<'ctx, B>` for schemas) and a capability-token-gated `call_result_from_value` (follows the shipped `ValidatedStructValue` pattern). New `TypedCallInst<'ctx, Ret: FunctionReturn, B>` handle in instructions.rs wrapping `CallInst<'ctx, Ret::Marker, B>` with infallible `result() -> Ret::CallResult`, `as_call_inst()`, `as_dyn()`. Marker is DERIVED from the callee, never asserted.
4. **Builder surface** (ir_builder.rs): typed `build_call(callee: TypedFunctionValue<Ret, Params>, args: A: CallArgs<Params>, name)` → `TypedCallInst<Ret>`; `build_call_with_config`; `typed_call_builder` (args taken whole up front — chainables carry only flags); `build_varargs_call(callee: TypedVarArgsFunctionValue, fixed: A, varargs: I, name)`; typed `build_indirect_call::<Sig: FunctionSignature>(ptr, args, name)`; typed `build_invoke`/`build_invoke_with_config` returning `(BasicBlock<R, Terminated>, InvokeInst<Ret::Marker>)` via alias `TerminatedBlockTypedInvoke`.
5. **D3 dyn fallbacks, renamed + hardened**: `build_call`→`build_call_dyn`, `build_indirect_call`→`build_indirect_call_dyn`, `build_invoke(...)`→`build_invoke_dyn(...)`. New private `validate_call_site_args(fn_ty, args)` (ports the `CallInst::init` assertions from `lib/IR/Instructions.cpp`) applied to ALL dyn call/invoke/callbr/inline-asm paths — mismatched calls fail at build time (matching upstream parser strictness) instead of at verify.
6. **Varargs**: new `TypedVarArgsFunctionValue` facade (typed fixed prefix + explicit dyn tail); `TypedFunctionValue::try_from_function` newly REJECTS varargs signatures. `Module::typed_function_type(is_var_arg: bool)` loses its bool param → separate `typed_varargs_function_type` (+`_of`, `add_typed_varargs_function`, `add_typed_varargs_function_of`).
7. **Errors** (error.rs): `CallArgumentCountMismatch`, `CallArgumentTypeMismatch { index, expected, got }`, `UnexpectedVarArgsSignature`, `MissingVarArgsSignature`. Bug fix: `marker.rs` gains crate-internal `marker_kind_label::<R>()` so `ReturnTypeMismatch` reports real expected/got.
8. **Parser migration** (ll_parser.rs): ~7462 `call_builder` unchanged; ~7489 → `build_indirect_call_dyn`; ~8262 → `build_invoke_dyn_with_config`.

**Usage (flagship):**

```rust
let callee = m.add_typed_function::<i32, (i32, i32), _>("callee", Linkage::External)?;
let r = b.build_call(callee, (x, 1), "r")?;        // (x,) alone: COMPILE ERROR
let sum = b.build_int_add::<i32, _, _, _>(r.result(), 2, "sum")?;
```

**Deferred (mechanical later, reuses `CallArgs` unchanged)**: `TypedInvokeInst<Ret>` schema wrapper, typed callbr, typed intrinsic calls, varargs invoke.

**Test plan (D11)**: new `tests/builder_typed_call.rs` anchored on `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst/InvokeInst)`; varargs anchored on `test/Bitcode/compatibility.ll`; dyn build-time rejection = example-locks citing `CallInst::init` + `LLParser::parseCall`; trybuild compile-fail fixtures (`typed_call_wrong_arity/wrong_arg_type/void_result_use/cross_module_arg`) with `.stderr` goldens; existing verify-level `VerifierRule::CallArg*` tests stay. All registered in UPSTREAM.md.

**Review resolutions**: `#[diagnostic::on_unimplemented]` needs Rust ≥ 1.78 — check workspace `rust-version`, drop the attribute (not the design) if MSRV forbids. Bare literals into `i8`/`i16`/`Width<N<32>>` slots need suffixes — compile error, not miscompile. `result()` returning `()` for void callees accepted (documented).

## Design: Workstream 2 — Hardening sweep

Priorities: **P0** = 2a typed folder (L), 2f.1 GEP addrspace fix (S), 2f.2 SelectArm forging hole (S). **P1** = 2b typed memory (M), 2d flag parity (S), 2e.3 invoke sentinel (S), 2e.6 index lists (M). **P2** = 2e.1/2/4/5, 2f.3/5 (all S). 2c is a documented no-code decision.

**2a. Typed folder** (`ir_builder/folder.rs` full trait replacement; static dispatch preserved):
- The 17 erased hooks are renamed `fold_*_dyn`, ALL with default `Ok(None)` bodies → `NoFolder` shrinks to `impl IRBuilderFolder for NoFolder {}`. Doctrine cleanups: `fold_exact_bin_op` drops its always-`true` `is_exact: bool`; `fold_no_wrap_bin_op`'s two bools become a new `OverflowFlags` carrier (instr_types.rs, `none()/nuw()/nsw()`); `fold_binary_intrinsic`'s `Option<&InstructionView>` param splits into `fold_binary_intrinsic_dyn` + `fold_binary_intrinsic_with_fmf_source_dyn`.
- NEW typed hooks with delegating defaults: `fold_int_bin_op<W>` / `_no_wrap` / `_exact` (→ `Option<IntValue<W>>`), `fold_fp_bin_op<K>` / `fold_fp_un_op<K>`, `fold_int_cmp<W>` / `fold_fp_cmp<K>` (→ `Option<IntValue<bool>>`), `fold_cast_to_int<W>` / `fold_cast_to_fp<K>`. Defaults delegate to the `_dyn` hook + TypeId re-narrow.
- Soundness: with `from_value_unchecked` crate-internal, a custom folder cannot forge a wrong-typed `IntValue<W>` for static `W` — the builder drops `checked_folded_value` on those paths. Two honest seams: (i) dyn markers keep a TypeId check via `accept_folded_int/fp` whose branch monomorphizes away for static markers; (ii) pointer/vector/aggregate folds stay erased + checked (PointerValue doesn't pin address space; vector element typing deferred to T4) — documented in the trait rustdoc.
- `ConstantFolder` overrides both families; typed overrides wrap fold-kernel results with `from_value_unchecked` under per-kernel invariant comments. **Implementation must audit each kernel** (`constant_fold_binary_instruction`, `constant_fold_cast_instruction`, `constant_expr`); any kernel that can't guarantee its invariant falls back to the checked default.
- Builder rewiring: ~45 fold call sites.

**2b. Typed memory overlay** (new `typed_pointer_value.rs`; existing `typed_pointer_type.rs` is the IR-level GPU kind, evaluated and REJECTED — it changes printed IR; rustdoc cross-links the two):
- `TypedPointerValue<'ctx, T: IrField, B>` = plain opaque `PointerValue` + `PhantomData<fn() -> T>` pointee schema. Printed IR byte-identical to the erased path. `IsValue`/`Typed`/`IntoPointerValue` impls. `PointerValue::with_pointee::<T>()` is a documented unchecked assertion (as powerful as passing `ty` to `build_load` today; verifier catches mis-assertions — D10).
- Builder methods: `build_typed_alloca::<T>` → `TypedPointerValue<T>`; `build_typed_load(ptr)` → `T::Value<'ctx, B>` (no runtime narrow); `build_typed_store(value: V: IntoIrField<T>, ptr)`; `build_field_gep::<S, const I: u32>(ptr)` → `TypedPointerValue<FieldOf<S, I>>` via new `StructFieldAt<const I: u32>` trait (macro impls per (arity, index) pair) + `FieldOf<S, I>` alias — out-of-range index = compile error; `build_element_gep` preserves the schema; `_with_align` variants included.
- Enabler: IRBuilder's `_module: PhantomData` becomes a stored `module_token: &'m Module<'ctx, B, Unverified>`.

**2c. Per-opcode handles: NO parameterization** (documented in instructions.rs module doc). Typed info already lives on the value handles builder methods return (D4); rediscovery paths are inherently dyn-shaped. Compromise: `LoadInst::loaded_ty()` accessor if absent.

**2d. Dyn-flag parity**: typed `build_int_cmp_with_flags` (samesign — LLVM 20; construction-time flag is a deliberate improvement over upstream's post-hoc setter), `build_zext_with_flags` (nneg, `WiderThan` preserved), `build_trunc_with_flags` (nuw/nsw — never silently dropped, per D10), `build_ui_to_fp_with_flags` (nneg). Consistency fix: `build_int_cmp_with_flags_dyn` reorders to `(pred, lhs, rhs, flags, name)`.

**2e. Misc doctrine fixes:**
- e.1 `as`-cast sweep in int_width.rs (~20 sites): `cast_unsigned()` (stable 1.87; MSRV fallback `uN::from_ne_bytes(x.to_ne_bytes())`) + one `u128_halves` helper.
- e.2 splat sentinel (ir_builder.rs:4478): `unwrap_or(usize::MAX)` → typed `IrError::InvalidOperation`.
- e.3 invoke/callbr `unwrap_or(fn_ty)` (~5119-5125, ~5272-5278): use `callee.signature()` directly.
- e.4 `walk_aggregate_for_builder` (~6317-6359): u64-domain comparison, `usize::try_from` indexing, new `IrError::AggregateIndexOutOfRange { index: u32, count: u64 }`.
- e.5 `FloatWiderThan` rows ADDED for `X86Fp80`/`PpcFp128`/`Fp128` (upstream `castIsValid` legalizes on strict `getPrimitiveSizeInBits` inequality); absent pairs (Fp128↔PpcFp128, Half↔BFloat) documented — strict inequality required.
- e.6 `build_extract_value`/`build_insert_value` take `[u32; N]` with `const { assert!(N > 0) }`; new `_dyn` twins keep the runtime check for slices (parser migrates, 2 sites).

**2f. New findings (verified in code):**
- f.1 **GEP address-space bug (P0)**: `build_gep_inner` hard-codes `ptr_type(0)` (ir_builder.rs:3703); upstream `GetElementPtrInst::getGEPReturnType` preserves the source pointer's address space. Fix: intern `ptr_type(addr_space)` from the base pointer. The sweep's only printed-IR delta — moves output from wrong to upstream-parity (lock against `test/Assembler/2007-12-11-AddressSpaces.ll`).
- f.2 **SelectArm forging hole (P0)**: `SelectArm::from_select_value` (~6232-6257) wraps unchecked — downstream code could forge `IntValue<i32>` from an `f64`. Fix: evidence-token parameter `&SelectNarrow<'_>` with `pub(crate)` constructor (follows `ValidatedStructValue` precedent).
- f.3 `expect()` in typed_pointer_type.rs:52,61 → invariant-named `unreachable!`.
- f.5 drop `build_load_inner`'s unused `_ptr` param (private).
- Punted to future work: typed vector/aggregate op results (T4), typed `fold_gep`/`fold_select` hooks (address-space markers), `[F; N]` IrField array impls, vector-of-pointer GEP bases, derive-generated field-index consts.

**Test plan (D11)**: NoFolder one-liner via `IRBuilderTest.cpp::NoFolderNames` port; typed-vs-erased fold equality both routes; wrong-typed custom folder = compile-fail example-lock; dyn-marker fold runtime-check test; GEP addrspace byte-parity vs `2007-12-11-AddressSpaces.ll`; `build_field_gep` typed/dyn print-identical + bad-index compile-fail; samesign/nneg/trunc-flags print parity vs `test/Assembler/flags.ll`; FloatWiderThan matrix vs `InstructionsTest.cpp::CastInst`; empty-index rejection ports `test/Assembler/extractvalue-no-idx.ll`. All in UPSTREAM.md.

## Design: Workstream 3 — Auto-SSA frontend

**New file `crates/llvmkit-ir/src/ssa_builder.rs`** (Rust-only feature; header cites Braun et al. 2013, cranelift-frontend `FunctionBuilder`, and `lib/Transforms/Utils/SSAUpdater*.cpp` as nearest LLVM relatives). Sits ON TOP of the typed IRBuilder; all existing type safety threads through unchanged.

**Handles:**
- `IntVariable<'ctx, W, B>` / `FloatVariable<'ctx, K, B>` / `PointerVariable<'ctx, B>` — cheap `Copy` handles `{ index, owner: SsaBuilderId, ty: TypeId, module, PhantomData }`; IR type pinned at declaration. `SsaBuilderId` from a crate-internal counter on `ModuleCore` (generative per-builder brand rejected: forces nested closures; owner-id runtime check with typed errors is the honest fallback).
- `SsaBlock<'ctx, R, B>` — Copy block reference (NOT an insertion capability). Implements `IntoBasicBlockLabel` as the escape hatch.

**`SsaBuilder<'m, 'ctx, B, F: IRBuilderFolder + Clone, S: BuilderPositionState, R>`** holding the module token, function, folder prototype, `inner: Option<IRBuilder<Positioned>>`, and `SsaState`. Constructor `for_function(module, f)` errors `SsaFunctionHasBlocks` if the function already has a body.

- Any-state: `create_block(name)` (FIRST block = entry, auto-Braun-sealed — the verifier's "entry has no predecessors" becomes a construction-time error); `declare_int_var::<W>` / `_dyn` / float / pointer variants, each with a `_poison` twin (separate methods; strict variables error on undefined reads per D10, poison variables read `poison` by explicit opt-in); `seal_block(block)` — Braun seal, completes that block's incomplete phis.
- Positioned-only: `ins() -> &IRBuilder<..., Positioned, R>` (full typed instruction surface; `&`-return makes self-consuming builder methods structurally unreachable — the CFG bookkeeping stays complete by construction); `def_int_var(var, value: V: IntoIntValue<W>)` (+ float/pointer) — pure bookkeeping; `use_int_var(var) -> IntValue<'ctx, W, B>` (+ float/pointer) — Braun readVariable, result type reflects the variable (D4). Terminators `br`/`cond_br`/`switch`/`ret`/`ret_void` (void-gated)/`unreachable` consume self, record CFG edges, return the Unpositioned builder. Branching to a Braun-sealed block = `SsaBranchToSealedBlock`.
- Unpositioned-only: `switch_to_block(block)` ("terminate before switching" is a COMPILE error — the method doesn't exist on Positioned); `finish()` — seals remaining blocks, errors `SsaUnfilledBlock` for created-but-unfilled blocks.

**Braun mapping** (mutation primitives verified sufficient — no new primitives): `writeVariable` = `current_def[(block,var)]`; `readVariableRecursive` on unsealed block = operandless phi at block head in `incomplete_phis`; sealed 1-pred = recurse, NO phi; sealed n-pred = phi + `addPhiOperands`; `tryRemoveTrivialPhi` = existing `Instruction<Attached>::replace_all_uses_with` + `erase_from_parent` (phi-incoming Cell rewrite confirmed at instruction.rs:1134; operand deregistration at :188) with a path-compressed forwarding map + `Value::users()` recursion into layer-created phis. Phi head-insertion via `position_before(&first_instruction)` or plain append into provably-empty blocks. Worklist, not native recursion.

**Typestate honesty (D1)**: positioning/fill discipline IS typestate. Braun-sealing CANNOT be typestate — "all predecessors known" is a property of future calls, and `SsaBlock` must stay Copy — so it's runtime state with typed errors (`SsaBlockAlreadySealed`, `SsaBranchToSealedBlock`, `SsaUseOfUndefinedVariable`, `SsaForeignVariable/Block`, `SsaUnfilledBlock`, `SsaFunctionHasBlocks`), with `finish()` as the always-correct seal-everything fallback.

**Flagship example** `examples/factorial_auto_ssa.rs`: factorial with variables `acc`/`i` — no phis, no label plumbing; single-pred reads resolve to the def directly (exit block gets `ret i32 %next_acc`, no spurious phi). Locked byte-identical to the manual `factorial.rs` output.

**Scope v1**: int/float/pointer variables; `br`/`cond_br`/`switch`/`ret`/`ret_void`/`unreachable`. Future: invoke/callbr/EH terminators, vector variables, aggregate variables (per-field fan-out through `StructSchema`). Mixed manual phis via `ins()` are legal and verifier-checked. Known limit: non-minimal (correct but redundant) phis on irreducible CFGs — Braun §4; no cleanup pass in v1.

**Test plan (D11)**: byte-identical factorial lock (single source of truth with `tests/factorial_example.rs`); diamond-merge single-phi placement; single-pred no-phi; backedge phi completion + trivial-phi RAUW-and-erase observation; strict-undef typed error; poison-path print lock; sealed-branch/double-seal/foreign/unfilled/nonempty-function negatives; `every_auto_ssa_module_verifies` over all suite CFG shapes; proptest random reducible CFGs → always verifies; 5 typestate compile-fail fixtures. All registered in UPSTREAM.md as `llvmkit-specific`/example-locks per the documented-gap rule.

**Review resolutions**: `ins()` accessor over `Deref` (C-DEREF). Builder lost on `Err` from self-consuming methods follows the `restore_insert_point` precedent. `F: Clone` bound is new — shipped folders already derive it. `Instruction::from_parts` visibility reachable from the sibling module — verify with rust-analyzer at implementation.

## Verification

- `cargo check` after each file-level step; `cargo test` per workstream; full gate before completion: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt -- --check`.
- rust-analyzer LSP throughout: `lsp references` before public-signature changes, `lsp rename` for symbol renames, `lsp diagnostics file:"*"` after substantive edits.
- AsmWriter parity: every existing print fixture passes unchanged (sole exception: the GEP addrspace bug fix, which moves output TO upstream parity).
- Typed-calls negative locks: compile-fail assertions for wrong arity, wrong arg type, wrong return marker.
- Auto-SSA example-lock: byte-identical factorial.
- Every new `#[test]` gets an UPSTREAM.md row.

## Future work

Captured in `docs/future-work.md` (created in the docs step): the `ir!` macro DSL (in the existing `llvmkit-macros` crate), rustc-quality diagnostics, upstream coverage gaps (memcpy/memset/lifetime with LLVM-22 signatures, GlobalString, min/max + intrinsic helper families, FMF-variant completions, const-GEP shortcuts, debug-loc/operand bundles, insert-point guards), ergonomics backlog (atomic builders, load/store builder consolidation, per-flag wrappers), inspiration-derived candidates (no-panic positioning vs inkwell, e-graph optimization substrate, Alive2-style refinement checking), and type-system follow-ups (T4 const-generic aggregates, `Width<M>`/`Width<N>` relations, auto-SSA aggregate variables, address-space-typed pointers).
