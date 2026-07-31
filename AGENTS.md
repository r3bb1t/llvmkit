# Repository Guidelines

## Project Overview

`llvmkit` is a from-scratch Rust reimplementation of LLVM IR APIs. It is **not** an FFI binding to `libLLVM` — the build and runtime never depend on `libLLVM` or `llvm-sys`.

Goals, in priority order:

1. **Read and write LLVM IR** (textual `.ll` first, bitcode later) with idiomatic Rust I/O traits.
2. **Provide an `IRBuilder` analog** for programmatic IR construction.
3. **Mirror LLVM's logic exactly**, using the C++ source under `orig_cpp/` as the canonical reference for behavior.
4. **Make invalid IR unrepresentable** at the type level wherever LLVM uses runtime checks. Where C++ forces `if (v->getType()->isFloatTy())`, Rust should expose a sum type whose variants already encode the answer.

What `llvmkit` is *not*:

- Not a binding crate (`llvm-sys`, `inkwell`, `llvm-ir`-style wrappers are all out of scope).
- Not a code generator and not a target backend. `llvmkit` doesn't lower IR to machine code or link objects — use upstream LLVM (`llvm-sys`, `inkwell`) for that. Pass and analysis infrastructure plus a first set of built-in transforms (`DcePass`, `InstSimplifyPass`, `SimplifyDemandedBitsPass`) are in-tree; a broader optimization-transform library, `PassBuilder`-style pipeline builders, loop PM, and CGSCC PM remain future work.

Note the direction of that first bullet: it means llvmkit does not *wrap* LLVM. It says nothing about wrapping llvmkit. **Python and Java bindings to `llvmkit` are planned** — they were blocked on an API stable enough to wrap, which is what 0.0.4 delivers. Never describe them as "out of scope" in a user-facing doc; that phrasing came from an internal plan and meant only "not part of this cycle". The design constraint the 2.0 surface was built under still holds and should be honoured by new API: nothing reachable only from inside a closure, no lifetime in any storable type, `DynBrand` as the rung a wrapper uses, and misuse of a handle or id an `IrError` or a deterministic panic rather than a dangling read.

## Project Status

The repo is a Cargo workspace at the repo root, at **0.0.4** (unreleased; see
`CHANGELOG.md`, `[0.0.4]`), tracking LLVM 22.1.4.

### The 2.0 handle model — read this before writing new API

The per-workstream summaries further down are a *historical* record, and parts
of them predate 2.0. Where the two disagree, this subsection wins.

- `Module<B: ModuleBrand, S = Unverified>` has **no lifetime parameter**. It
  owns its storage (`Box<ModuleCore>`), is `Send` — the brand rides as
  `PhantomData<fn(B) -> B>`, so even a `!Send` brand type leaves the module
  `Send` — and can be moved into a struct, a `Vec`, or another thread. `S` is
  the verification typestate (`Unverified` / `Verified`).
- A module's identity is the brand *type*:
  `pub trait ModuleBrand: 'static {}`. It is **unsealed** — users declare their
  own — and demands nothing of the type: a brand is a bare unit struct.
  `'static` is the only bound, and only because the uniqueness registry keys
  brands by `TypeId`. A user-declared brand is two lines:

  ```rust
  struct LiftedBin;
  impl llvmkit_ir::ModuleBrand for LiftedBin {}
  ```

  It carried `Copy + Debug + Eq + Hash` until the 0.0.4 freeze, not because
  anything called those methods on a brand but because the brand-generic
  containers used std `#[derive]`, which bounds every type parameter whether or
  not a field uses it. **Those containers now use `#[derive(Branded)]`** from
  `llvmkit-macros`: identical impls with the item's generics copied verbatim
  and no inferred bounds. When you add a branded type, derive `Branded`, not
  the std traits — a std derive silently reintroduces the bounds the brand no
  longer satisfies, and the compiler will point at the *use* site, not yours.

- Three ways to make a module, all returning an owned value:
  `module_new!("name")` (emits the struct above under an unnameable name, one
  fresh brand per expansion site), `Module::branded::<B, _>(name) ->
  IrResult<Module<B, Unverified>>` (named brand; a process-global registry
  admits at most one live module per brand — `IrError::BrandInUse`, and
  `branded_once::<B>` retires it permanently — `IrError::BrandRetired`), and
  `Module::dynamic(name) -> Module<DynBrand, Unverified>` (infallible,
  registry-exempt, many live modules legal). There is **no** closure-scoped
  construction anywhere.
- **Ids** are the storable currency: `Copy + Send + 'static`, payload private,
  carrying `(ModuleId, slot)`. `ValueId<B>`, `IntValueId<W, B>`,
  `FloatValueId<K, B>`, `PointerValueId<B>`, `FunctionId<R, B>`, `GlobalId<B>`,
  `BlockId<R, B, Params>`, and the instruction ids.
- **Views** are the ephemeral borrowing layer: `m.view(id)` (deterministic
  panic on a foreign tag or absent slot) and `m.try_view(id)` (`None`). Views
  are never stored.
- Declarations and **value-producing** builders return ids —
  `Module::add_global -> IrResult<GlobalId<B>>`,
  `build_int_add -> IrResult<IntValueId<W, B>>`,
  `build_call -> IrResult<TypedCallInstId<Ret, B>>`. **Not every builder does:**
  block appenders mint linear `!Copy` handles
  (`FunctionValue::append_basic_block -> BasicBlock<'ctx, R, Unterminated, B>`;
  `IRBuilder::append_block_with_params -> IrResult<BlockWithParams<'ctx, R, B>>`,
  a `(block, params)` pair) — reach the storable `BlockId` via `.id()`, since no
  API hands one back directly. Terminator builders consume the builder by value
  and return borrowing handles, not ids: `build_br` / `build_cond_br` /
  `build_ret` yield `TerminatedBlockInst<'ctx, R, B>`, the type alias for
  `(BasicBlock<'ctx, R, Terminated, B>, Instruction<'ctx, Attached, B>)`.
- Each `get_*` returns the same currency its `add_*` twin does:
  `get_global -> Option<GlobalId<B>>`, likewise `get_alias` / `get_ifunc`,
  `function_by_name_dyn -> Option<FunctionId<Dyn, B>>`, and
  `function_by_name::<R> -> IrResult<Option<FunctionId<R, B>>>` (a signature
  mismatch stays `IrError::ReturnTypeMismatch`, never a widened id). All are
  state-generic and take `&self`. `get_comdat -> Option<ComdatRef<'ctx, B>>` is
  the one deliberate exception: a comdat is not a `Value`, `ComdatId` carries
  neither tag nor brand, and `view` cannot resolve it — compare comdats with
  `ComdatRef`'s `PartialEq`.
- `BlockId<R, B, Params>` is the storable branch-target / phi-predecessor
  currency; `BasicBlockLabel` is the borrowing view it resolves to, and
  `IntoBasicBlockLabel` accepts either (plus a `BasicBlock`).
- Internal arena indices are named `*Slot` (`ValueSlot`, `TypeSlot`,
  `MetadataSlot`) — **never** `*Id`. `handle.slot()` is the internal index;
  `handle.id()` mints the public tagged id.
- **Phis are not authored by hand.** `IRBuilder::build_int_phi` /
  `build_fp_phi` / `build_pointer_phi` and every `PhiInst::add_incoming` are
  `pub(crate)` and unnameable from outside the crate (locked by
  `tests/compile_fail/raw_phi_builder_is_unnameable.rs`). There are exactly
  three public routes: **block arguments** — `append_block_with_params` /
  `append_block_with_named_params` / `append_block_typed`, branched to with
  `build_br_with_args` / `build_cond_br_with_args` / `build_switch_with_args` /
  `build_switch_dyn_with_args` / `build_invoke_with_args` /
  `build_invoke_dyn_with_args`, or the compile-checked `build_br_call` /
  `build_cond_br_call` — which is the one to reach for. A plain terminator
  aimed at a block that was *created with* parameters is rejected up front
  (`IrError::PhiArgArityMismatch`), so an edge cannot silently skip its
  incomings; `indirectbr`, `callbr`, and the indirect-callee `invoke` shapes
  are reject-only, since their edges are selected at run time. Then
  `SsaBuilder`, which discovers the phis for you; and
  `FnReshape::insert_phi` inside a pass. Only the erased escape hatches
  (`build_phi_dyn`, `build_int_phi_dyn`, `build_fp_phi_dyn`,
  `build_pointer_phi_in_addrspace`) are public builder methods.

Implemented today:

- The `.ll` lexer (`llvmkit-asmparser/src/ll_lexer.rs`).
- The `.ll` parser (`llvmkit-asmparser/src/ll_parser.rs` and `parser.rs`) for the constructive module/function/instruction subset: target datalayout/triple, module asm, types, globals, declarations/definitions, metadata records, use-list directives, summaries, and every shipped opcode.
- The IR data model with **width-typed integers** (`IntType<'ctx, W, B>`, `W in { bool, i8, i16, i32, i64, i128, IntDyn, Width<const N: u32> }`) and **kind-typed floats** (`FloatType<'ctx, K, B>`, `K in { f32, f64, Half, BFloat, Fp128, X86Fp80, PpcFp128, FloatDyn }`). Every borrowing handle carries the module brand `B` as its last type parameter; call sites infer it from the `Module` or type receiver and never spell it.
- Sealed marker traits: `IntWidth`, `StaticIntWidth`, `FloatKind`, `StaticFloatKind`, `WiderThan`, `FloatWiderThan`, `ReturnMarker`, `SelectArm`.
- Multi-source operand traits: `IntoIntValue<W>`, `IntoFloatValue<K>`, `IntoPointerValue`, `IntoConstantInt<W>`, `IntoConstantFloat<K>`, `IntoReturnValue<R>`.
- The full medium IRBuilder: every integer binop (`add`/`sub`/`mul`/`udiv`/`sdiv`/`urem`/`srem`/`shl`/`lshr`/`ashr`/`and`/`or`/`xor`) plus per-opcode flag types (`AddFlags`/`UDivFlags`/...), every float binop (`fadd`/`fsub`/`fmul`/`fdiv`/`frem`), every cast (`trunc`/`zext`/`sext`/`bitcast`/`ptrtoint`/`inttoptr`/`fptrunc`/`fpext`/`fptosi`/`fptoui`/`sitofp`/`uitofp`/`addrspacecast`), `icmp`/`fcmp`, control flow (`br`/`cond_br`/`unreachable`), `phi` (chainable `add_incoming`), memory (`alloca`/`load`/`store` with optional `Align`), `getelementptr` (`build_gep`/`build_inbounds_gep`/`build_struct_gep`), `call` (flat + chainable `CallBuilder`), and `select` (sealed `SelectArm` for int/float/pointer arms).
- AsmWriter producing real `.ll` output via `format!("{module}")` for every shipped opcode.
- **Verifier** (`crates/llvmkit-ir/src/verifier.rs`): `Module::verify_borrowed(&self) -> IrResult<()>` validates without consuming, and `Module::verify(self) -> IrResult<Module<B, Verified>>` consumes an unverified token into the verified typestate. The verifier covers the shipped opcode surface, including CFG-backed PHI checks and cross-block SSA dominance through `DominatorTree`.
- **Mutation API (T1)**: full instruction-lifecycle typestate. `Instruction<'ctx, S, B>` is parameterised by `S: state::InstructionState` (sealed; variants `Attached` / `Detached`) — there is no default for `S`. `Instruction` is intentionally **`!Copy` and `!Clone`** (Doctrine D2): the linear-typed handle prevents use-after-erase / double-erase at compile time. Copyable per-opcode handles (`AddInst`, `LoadInst`, ...) expose `as_view(&self) -> InstructionView` for read-only inspection; lifecycle mutation requires a builder-produced `Instruction<Attached>` or `iter::BlockCursor` rediscovery. Open variable-arity handles (`SwitchInst<Open>`, `IndirectBrInst<Open>`, `LandingPadInst<Open>`, `CatchSwitchInst<Open>`) are also linear and consumed by their mutators / `finish`, so retained open handles cannot mutate after finalisation. Every operand slot in `instr_types.rs` is wrapped in `core::cell::Cell<ValueSlot>` (and `Cell<Option<ValueSlot>>` for the optional `alloca` / `ret` operands) so RAUW can rewrite the operand wiring through `&self`. The shipped lifecycle methods are:
    - `Instruction<Attached>::replace_all_uses_with(self, &Module<B, Unverified>, replacement)` --- `Value::replaceAllUsesWith` in `lib/IR/Value.cpp`.
    - `Instruction<Attached>::erase_from_parent(self, &Module<B, Unverified>)` --- `Instruction::eraseFromParent`.
    - `Instruction<Attached>::detach_from_parent(self, &Module<B, Unverified>) -> Instruction<'ctx, Detached, B>` --- `Instruction::removeFromParent`.
    - `Instruction<Attached>::move_before(self, &Module<B, Unverified>, &InstructionView)` / `move_after` --- `Instruction::moveBefore` / `moveAfter`.
    - `Instruction<Detached>::insert_before(self, &Module<B, Unverified>, &InstructionView)` / `insert_after` / `append_to(&Module<B, Unverified>, block)` --- `Instruction::insertBefore` / `insertAfter` / `insertInto`.
    - `Instruction<Detached>::drop_detached(self, &Module<B, Unverified>)` --- discard a detached instruction without inserting it (deregisters its operands' use-list entries).
    - `BasicBlock::splice_into(self, &Module<B, Unverified>, dest)` --- `BasicBlock::splice`.
    - `BasicBlock::split_at(self, &Module<B, Unverified>, &InstructionView, name) -> IrResult<BasicBlock<'ctx, R, Unterminated, B>>` --- `BasicBlock::splitBasicBlock`.
    - `iter::BlockCursor::at_start(block)` and `cursor.next() -> Option<(Instruction<Attached>, BlockCursor)>` --- the canonical advance-then-mutate iteration helper (Doctrine D9). Yields each instruction by value while consuming the cursor; the next call sees the precomputed snapshot, so erasing the yielded handle does not invalidate the iteration.
  Note: `IsValue` keeps its `Copy` supertrait bound (every other implementer is a thin `Copy` handle); `Instruction<Attached>` is therefore *not* an `IsValue` impl. Callers that need an erased view use [`Instruction::to_erased`] (inherent, `&self` — it borrows, so the linear handle survives).
- **Test provenance registry (T0)**: `UPSTREAM.md` (repo root) is the authoritative answer to "where does this llvmkit test come from?". Every `#[test]` in the workspace ships with a per-test doc comment citing the upstream `unittests/IR/*Test.cpp::TEST(...)` or `test/{Assembler,Verifier}/*.ll` fixture it ports (Doctrine D11).
- **Construction Lifecycle Typestate (T2)**: [`crate::BasicBlock`] gains a `Term: BlockTerminationState` parameter; today the handle reads `BasicBlock<'ctx, R, Term, B, Params = BlockParamsDyn>` and `Term` has no default. The terminator-emitting builds (`build_br` / `build_cond_br` / `build_unreachable` / `build_ret` / `build_ret_void`) consume the `IRBuilder` by value and return the borrowing [`crate::TerminatedBlockInst`] pair `(BasicBlock<'ctx, R, Terminated, B>, Instruction<'ctx, Attached, B>)` — **not** an id. [`crate::IRBuilder::position_at_end`] only accepts an `Unterminated` block, so a builder cannot be re-positioned at a terminated block, and because each terminator takes `self` by value a second terminator on one block is a borrow-check error (locked by `tests/compile_fail/builder_cannot_terminate_twice.rs`). [`crate::PhiInst`] carried the same shape (`P: PhiState`, `add_incoming` gated to `Open`, `finish` producing a `Closed` view) until cycle B slice B1g retired it: the phi builders now return `Copy` ids, a view minted from a `Copy` id is re-mintable, and a linear open-capability marker on a re-mintable view guarantees nothing. Phi authoring is kept unobservable from outside the crate by *visibility* instead (`build_*_phi` and `add_incoming` are `pub(crate)`); the variable-arity terminators (`switch`/`indirectbr`/`landingpad`/`catchswitch`) keep their own `term_open_state` Open/Closed typestate, which is still linear and still public.
- **CallInst Typed Return (T3)**: [`crate::CallInst`] gains a `R: ReturnMarker` parameter that propagates the callee's return shape; today the handle reads `CallInst<'ctx, R, B>` and `R` has no default. Per-marker accessors (`return_int_value`, `return_float_value`, `return_pointer_value`) are gated to the matching marker so a `()`-marked call exposes none of them -- the runtime narrowing on `return_value() -> Option<Value>` is gone for typed call sites. **Superseded (0.0.4):** the builder methods return *ids*, not handles -- `build_call_dyn` (the erased, `FunctionValue`-callee form) returns `IrResult<CallInstId<R, B>>` and `build_call` (the schema-typed form) returns `IrResult<TypedCallInstId<Ret, B>>`. `m.view(id)` mints the `CallInst` / `TypedCallInst` handle when you need the accessors.
- **Aggregate Typing scaffolding (T4)**: [`crate::StructType`] gains a body-state parameter with `Opaque` / `BodySet` / `StructBodyDyn` markers; today the handle reads `StructType<'ctx, Body: StructBodyState, B>` and `Body` has no default. `Module::opaque_struct(name) -> IrResult<StructType<'ctx, Opaque, B>>` and `Module::set_struct_body(opaque, elements, packed) -> IrResult<StructType<'ctx, BodySet, B>>` gate the body-set transition at the type level; the existing runtime-checked `set_struct_body_dyn` path stays for parsed / legacy code. The element/length scaffolding plus the const-generic parameterisation of [`crate::VectorType`] / [`crate::ArrayType`] have since **shipped** (`feature-17/const-generic-vec-array`, S1–S6): the old unwired `VectorElement` / `SizedElement` markers were replaced by [`crate::VecElem`] / [`crate::StaticVecElem`] / `ElemDyn` (`element.rs`), and the types now read `VectorType<'ctx, E, L, B>` / `ArrayType<'ctx, E, L, B>` with a scalar element marker `E` and a length marker `L` (`Len<const N: u32>` / `LenDyn` in `vec_len.rs`; `ArrLen<const N: u64>` / `ArrLenDyn` in `array_len.rs`). The bare `<'ctx>` forms stay all-`Dyn`, so `m.array_type(elem, n)` / `m.vector_type(elem, n, scalable)` still yield erased handles; `Module::vector_type_n::<E, const N>()` / `array_type_n::<E, const N>()` mint the const-generic ones. See the roadmap below for the residual non-goals that stay erased.

- **Parser-1: full instruction set (Session 1)**. Every opcode the `.ll` parser will need is shipped end-to-end (handle struct, payload, exhaustive operand walker arm per Doctrine D5, IRBuilder method, AsmWriter byte-for-byte parity, verifier `visit_*` arm). The 21 new instruction families: `fneg` (with FMF), `freeze`, `va_arg`; `extractvalue`/`insertvalue` (compile-time `u32` index lists); `extractelement`/`insertelement`/`shufflevector` (`shufflevector` mask is `Box<[i32]>` with `POISON_MASK_ELEM = -1`); `fence`/`cmpxchg`/`atomicrmw` (with new `AtomicOrdering`/`SyncScope`/`AtomicRMWBinOp` support modules and `AtomicCmpXchgConfig`/`AtomicRMWConfig` flag-bag structs); `switch`/`indirectbr` (Open/Closed typestate via new `term_open_state` mod, which mirrored the since-retired `phi_state`); `invoke<R>`/`callbr` (`InvokeInst<R>` mirrors `CallInst<R>` typed-return); `landingpad` (Open/Closed; `add_catch_clause`/`add_filter_clause`/`set_cleanup`)/`resume`; `cleanuppad`/`catchpad`/`catchret`/`cleanupret`/`catchswitch` (funclet pads with `Option<ValueId>` parent-pad slot for `within none`).
- **Builder-A1: atomic load/store + typed bitcast**. `LoadInstData` / `StoreInstData` carry `ordering: AtomicOrdering` and `sync_scope: SyncScope` slots that mirror the `OrderingField` / `SSID` bitfields on the upstream `LoadInst` / `StoreInst` classes (`Instructions.h`). The IRBuilder ships `build_int_load_atomic` / `build_load_atomic` / `build_store_atomic` keyed on the new `AtomicLoadConfig` / `AtomicStoreConfig` config bags (parallel to the existing `AtomicCmpXchgConfig` / `AtomicRMWConfig` shape), porting the upstream 5-arg `LoadInst::LoadInst(Type*, Value*, Twine&, bool, Align, AtomicOrdering, SyncScope::ID)` and 6-arg `StoreInst` constructors. AsmWriter emits the canonical `load atomic [volatile] <ty>, ptr <p> [syncscope("...")] <ordering>, align N` form (mirrors `printInstruction` in `lib/IR/AsmWriter.cpp`). Verifier ports the atomic-rule arm of `Verifier::visit{Load,Store}Inst` and `checkAtomicMemAccessSize` (rejects Release on load, Acquire on store, non-power-of-two operand size, non-default sync scope on non-atomic ops). New typed bitcast methods (`build_bitcast_int_to_int`, `build_bitcast_int_to_fp`, `build_bitcast_fp_to_int`, `build_bitcast_fp_to_fp`) port `IRBuilder::CreateBitCast`'s `CreateCast(Instruction::BitCast, V, DestTy)` arm with width equality enforced statically through new `STATIC_BITS: u32` associated constants on `StaticIntWidth` / `StaticFloatKind` (compile-time `const { assert!(...) }`).
- **Builder-A2: positioning + integer convenience**. The `IRBuilder` (today `IRBuilder<'m, 'ctx, B, F, S, R>`) gains an `insert_before: Option<ValueSlot>` slot (mirrors upstream `InsertPt`'s before-instruction iterator state) so `position_before(anchor)` and `position_past_allocas(f)` correctly thread `BasicBlock::insert_instruction_before` instead of always appending. `save_insert_point()` / `restore_insert_point(snapshot)` round-trip an [`InsertPoint<'ctx, R, B>`] (parallel to `IRBuilderBase::InsertPoint` in `IRBuilder.h`). Integer convenience methods `build_int_neg` / `build_int_neg_nsw` / `build_int_not` (mirror `IRBuilder::CreateNeg` / `CreateNSWNeg` / `CreateNot`). Pointer convenience methods `build_pointer_cast` (`CreatePointerBitCastOrAddrSpaceCast`), `build_pointer_cmp`, `build_is_null`, `build_is_not_null`. New `IntType::const_zero` / `IntType::const_all_ones` ports of `Constant::getNullValue` / `Constant::getAllOnesValue`. The `FastMathFlags` slot on the IRBuilder ships now (default `empty()`); the per-op auto-application lands in A3.
- **Builder-A3: builder-context FMF + per-predicate fcmp + non-int phi**. The `IRBuilder` carries an `fmf: FastMathFlags` slot (default `empty()`); `with_fast_math_flags(fmf) -> Self`, `clear_fast_math_flags() -> Self`, and `fast_math_flags() -> FastMathFlags` mirror `IRBuilderBase::setFastMathFlags` / `clearFastMathFlags` / `getFastMathFlags`. `BinaryOpData` and `FCmpInstData` gain a `fmf` slot (printed by `fmt_binop` / `fmt_fcmp`) so the builder context auto-propagates to `fadd`/`fsub`/`fmul`/`fdiv`/`frem`/`fcmp` (and `fneg` via `build_float_neg`). Fourteen named per-predicate fcmp wrappers (`build_fcmp_oeq` ... `build_fcmp_une`) mirror `IRBuilder::CreateFCmpOEQ` ... `CreateFCmpUNE`. New non-int phi handles: `FpPhiInst<'ctx, K, B>` (port of `PHINode` with `IntoFloatValue<K>` operands) and `PointerPhiInst<'ctx, B>` (pointer phi); IRBuilder methods `build_fp_phi::<K>` / `build_fp_phi_dyn` / `build_pointer_phi` / `build_pointer_phi_in_addrspace`. (The open/closed phi typestate these once carried was retired in cycle B slice B1g, as described above. The typed constructors `build_int_phi` / `build_fp_phi` / `build_pointer_phi` are `pub(crate)` and unnameable from outside the crate; the `_dyn` forms and `build_pointer_phi_in_addrspace` are public. All of them return ids -- `PhiInstId<W, B>`, `FpPhiInstId<K, B>`, `PointerPhiInstId<B>`. For the three public ways to author a phi today, see **The 2.0 handle model**.)
- **Builder-A4: vector splat + ptr_add convenience**. `IRBuilder::build_vector_splat(count, scalar, name)` mirrors `IRBuilderBase::CreateVectorSplat(unsigned, Value*, const Twine&)` (`lib/IR/IRBuilder.cpp`): inserts `scalar` at lane 0 of a poison `<count x T>` vector, then shufflevectors with a zero-mask. The intermediate `insertelement` is named `<name>.splatinsert` and the result `<name>.splat`, byte-for-byte matching upstream. `build_ptr_add(ptr, offset, name)` and `build_inbounds_ptr_add` mirror `IRBuilder::CreatePtrAdd` / `CreateInBoundsPtrAdd` (`IRBuilder.h`) -- thin wrappers around `CreateGEP(getInt8Ty(), Ptr, Offset, ...)`. New `VectorValue::from_value_unchecked` crate-internal constructor parallels the existing `PointerValue` / `IntValue` / `FloatValue` shapes for builder-result narrowing. `CreateAggregateRet` is deferred -- upstream lacks a dedicated `TEST_F` and the construct is genuinely niche (multi-value-return through poison + insertvalue chain).
- **Globals-B1: GlobalVariable + Comdat + AsmWriter / Verifier hookup**. New [`crate::global_variable::GlobalVariable<'ctx, B>`] handle (Copy, slot-keyed, mirrors `class GlobalVariable` in `IR/GlobalVariable.h`) carries the full upstream slot set: `value_type`, `address_space`, `is_constant`, `externally_initialized`, optional `initializer`, [`Linkage`], [`Visibility`], [`DllStorageClass`], [`ThreadLocalMode`], [`UnnamedAddr`], [`MaybeAlign`], `section`, `partition`, optional comdat reference. Construction goes through [`Module::add_global`] / [`Module::add_global_constant`] / [`Module::add_external_global`] (one-shot ctors mirroring upstream's two `GlobalVariable::GlobalVariable` overloads) or the chainable [`crate::global_variable::GlobalBuilder`]. **Superseded (0.0.4):** all three ctors return `IrResult<GlobalId<B>>`, and [`Module::get_global`] returns `Option<GlobalId<B>>`; `m.view(id)` mints the `GlobalVariable` handle. Per-module storage mirrors the function shape (`globals: RefCell<Vec<ValueSlot>>` + `global_by_name: RefCell<HashMap<...>>`), and a new `ValueKindData::GlobalVariable(GlobalVariableData)` arm closes the value-kind enum (Doctrine D5 -- exhaustive operand/category matches updated). Comdat support ports `IR/Comdat.h`: [`crate::comdat::SelectionKind`] (`Any` / `ExactMatch` / `Largest` / `NoDeduplicate` / `SameSize`), [`crate::comdat::ComdatRef<'ctx, B>`] backed by a `boxcar::Vec<ComdatData>` arena (stable `&ComdatData` borrows under `&self`), and [`Module::get_or_insert_comdat`] / [`Module::get_comdat`] / [`ModuleView::comdats`] mirroring `Module::getOrInsertComdat`. Comdats are the one currency that stayed handle-shaped: `get_comdat` returns `Option<ComdatRef>`, `ComdatRef::id` was removed in cycle E, and comdat identity is compared through `ComdatRef`'s `PartialEq`. AsmWriter ships byte-for-byte parity with `lib/IR/AsmWriter.cpp::printGlobal` (linkage / visibility / DLL / TLS / unnamed-addr / addrspace / external_initialised / global-vs-constant / initializer / section / partition / comdat / align) and `Comdat::print` (`$<name> = comdat <kind>`). c-string detection in `fmt_aggregate_constant` mirrors `ConstantDataArray::isString`: `[N x i8]` constants of all `ConstantInt` elements emit as `c"..."` (with `printEscapedString` semantics). Verifier ships [`Verifier::visit_global_variable`] for the constructive subset (initializer-type-matches-value-type, initializer-must-be-sized, common-linkage zero-init/not-constant/no-comdat, scalable-vector rejection); new [`VerifierRule`] variants `GlobalInitializerTypeMismatch` / `GlobalInitializerUnsized` / `CommonLinkageInvariantViolated` / `GlobalScalableType` and a new [`ValueCategoryLabel::GlobalVariable`] arm complete the diagnostic surface. 43 new ported tests under `crates/llvmkit-ir/tests/globals_basic.rs`, all anchored on `test/Bitcode/compatibility.ll` or `unittests/IR/{Module,Constants}Test.cpp::TEST(...)` provenance (Doctrine D11).
- **DataLayout-B2: target-datalayout / target-triple / module-asm directives**. New [`crate::data_layout::DataLayout`] ports `class DataLayout` from `IR/DataLayout.h` + the parser slice of `lib/IR/DataLayout.cpp`. The parser accepts every upstream specifier: endianness (`e` / `E`), mangling (`m:e` / `m:o` / `m:w` / `m:x` / `m:l` / `m:m` / `m:a`), per-bit-width primitive alignment (`i<N>:<abi>:<pref>` / `f<...>` / `v<...>`), aggregate alignment (`a<...>`), pointer specs with optional flags + symbolic name (`p[<flags>][<as>][(<name>)]:<size>:<abi>[:<pref>[:<idx>]]`), native integer widths (`n<...>`), stack-natural alignment (`S<n>`), function-pointer alignment + kind (`F[in]<n>`), program / alloca / globals address spaces (`P<n>` / `A<n>` / `G<n>`), and the trailing non-integral-AS post-pass (`ni:<as>...`). Error messages mirror upstream byte-for-byte ("unknown specifier '...'", "address space must be a 24-bit integer", "preferred alignment cannot be less than the ABI alignment", etc.). Accessor surface: [`DataLayout::is_little_endian`] / `is_big_endian`, [`mangling_mode`], [`stack_alignment`], [`function_ptr_align`] / [`function_ptr_align_type`], [`alloca_addr_space`] / [`program_addr_space`] / [`default_globals_addr_space`], [`is_legal_integer`] / [`is_illegal_integer`] / [`fits_in_legal_integer`] / [`largest_legal_int_type_size_in_bits`], [`address_space_name`] / [`named_address_space`] / [`is_non_integral_address_space`] / [`has_unstable_representation`] / [`has_external_state`], [`pointer_size`] / [`pointer_size_in_bits`] / [`index_size`] / [`index_size_in_bits`], [`pointer_abi_align`] / [`pointer_pref_align`], type-walking accessors [`type_size_in_bits`] / [`type_store_size`] / [`type_store_size_in_bits`] / [`type_alloc_size`] / [`type_alloc_size_in_bits`] / [`abi_type_align`] / [`pref_type_align`] / [`abi_integer_type_align`] / [`value_or_abi_type_align`], and a [`StructLayoutInfo`] returned from [`struct_layout`] (mirrors `StructLayout::StructLayout`'s field-placement walk including padding flags and per-field offsets). Target-extension layout-type table ports `getTargetTypeInfo` from `lib/IR/Type.cpp` end-to-end (SPIR-V image / typed / padding / IntegralConstant / Literal, AArch64 `svcount`, RISC-V `riscv.vector.tuple`, DirectX `dx.*`, AMDGPU `amdgcn.named.barrier`, the test extension `llvm.test.vectorelement`, void default). [`Module`] gains [`data_layout`] / [`set_data_layout`] / [`set_data_layout_value`] / [`target_triple`] / [`set_target_triple`] / [`module_asm`] / [`set_module_asm`] / [`append_module_asm`] mirroring the matching `Module::*` methods. AsmWriter emits `target datalayout = "..."` (when non-default), `target triple = "..."` (when set), and one `module asm "..."` line per newline-split entry, mirroring `lib/IR/AsmWriter.cpp::printModule`. New [`IrError::InvalidDataLayout { reason }`] for parse failures. 48 new ported tests under `crates/llvmkit-ir/tests/data_layout_round_trip.rs`, one per upstream `TEST(DataLayout*, ...)` block plus standard-target round-trip cases for x86_64-linux / aarch64-darwin / wasm32 (Doctrine D11).
- **Pass-C1: capability-graded pass authoring + raw-core closure**. Public raw `ModuleCore` access is closed: `Module` exposes direct constructors/mutators, public handles return read-only `ModuleView`, and IR construction requires `&Module<B, Unverified>`. Saved-handle mutators (`FunctionValue`, globals, instruction lifecycle, block splice/split, names) require the matching unverified module token, so `verify(self)` consumes mutation capability. As of cycle E this holds without exception: `InstructionView::set_metadata`, `InstructionView::push_debug_record`, and their `Instruction` twins gained the leading `&Module<B, Unverified>` parameter they had been missing, which had let a `Verified` module be mutated through a read-only view and let an `Inspect`-rung pass rewrite `!dbg` while the driver reported everything preserved (locked by `tests/compile_fail/verified_module_metadata_is_immutable.rs`). **Authoring a pass** (`FunctionPass`, or the symmetric `ModulePass`): declare `type Access` — the pass's *capability rung* — plus `type Requires` (a tuple of analysis markers, prefetched before the run) and `const NAME`, then write one `fn run(cx) -> IrResult<FnReport>` (a module pass returns `IrResult<ModReport>`). The rung is `Inspect` (read-only; the only rung valid at both function and module level), `PatchBody` (edit instructions, CFG intact), `ReshapeCfg` (rewire the CFG), or `RewriteModule` (module-level rewrite). **Preservation is derived, never declared** — over-claiming is *unspellable*: the report's constructor is `pub(crate)`, so the only report an author can obtain carries the floor the driver derives from the rung. An `Inspect` context has no `cx.mutate()` at all (`Inspect` is not `MutatingFn`); each mutating rung's `cx.mutate()` yields a mutator whose `done()` reports exactly that rung's floor (`PatchBody` ⇒ CFG analyses preserved; `ReshapeCfg`/`RewriteModule` ⇒ nothing). Declared analyses are read infallibly through `cx.analysis::<A, _>()`; an undeclared analysis has no `AnalysisSelector` impl and is a compile error. **Three run modes**, each threading one `&mut Analyses` (the bundled function + module analysis managers): a single pass via `run_function_pass` / `run_module_pass`; a compile-time tuple via `function_pipeline` / `module_pipeline` (with the `for_each_function` adaptor and nested pipelines) whose `Module<B, Verified>`-vs-`Module<B, Unverified>` output typestate is *derived* from the members' rungs — any mutating member downgrades it (D8), never a hand-written claim; and runtime-assembled `Dyn` containers — `DynFunctionPipeline` / `DynModulePipeline` (transform; always `Unverified` out) and `DynReadOnlyFunctionPipeline` / `DynReadOnlyModulePipeline` (read-only; `push` bounded to `Inspect`, so a mutating pass cannot enter and the module threads through `Verified`). The `#[function_pass]` / `#[module_pass]` attribute macros are zero-cost sugar expanding to exactly the raw trait impl (`name`/`access`/`requires`/`required` → `NAME`/`Access`/`Requires`/`REQUIRED`). Note two names that moved: a module pass walks its functions with `ModRewrite::patch_functions()` / `reshape_functions()` (the old `ModRewrite::for_each_function::<FnA>(visitor)` is gone), while the *pipeline adaptor* `for_each_function(function_pipeline((..)))` is very much alive — different thing, same words.
- **Block-state rename (Workstream 0)**. `BlockSealState` (LLVM-terminator sense) is renamed `BlockTerminationState` end-to-end -- `Sealed`/`Unsealed` become [`Terminated`]/[`Unterminated`], `IS_SEALED` becomes `IS_TERMINATED`, `BasicBlock::retag_seal` becomes `retag_termination`, and the generic parameter conventionally named `Seal` is now `Term`. The `ir_builder.rs` aliases (`SealedBlockInst` and its `Switch`/`IndirectBr`/`Invoke`/`CatchSwitch` siblings) become `TerminatedBlockInst` etc. The private builder-positioning seal is promoted to `pub trait BuilderPositionState: state_sealed::Sealed` so the auto-SSA layer below can name it. This frees the word "seal" for its one remaining meaning: Braun-style predecessor-completeness sealing (`SsaBuilder::seal_block`) -- the unrelated `sealed::Sealed` privacy idiom is the only other surviving use of the word. Purely mechanical (LSP rename across ~111 occurrences in `llvmkit-ir` + 13 in `llvmkit-asmparser`); no behavior change, no printed-IR change.
- **Hardening sweep (Workstream 2)**. Closes runtime-check seams in the IRBuilder/folder path. The [`crate::ir_builder::folder::IRBuilderFolder`] trait's 15 erased hooks are renamed `fold_*_dyn` (16 `_dyn` hooks total after the `fold_binary_intrinsic` split adds `fold_binary_intrinsic_with_fmf_source_dyn`; the two `create_pointer_cast`/`create_pointer_bitcast_or_addrspace_cast` hooks keep their pre-existing names) and gain default `Ok(None)` bodies, so `NoFolder` shrinks to a one-line `impl IRBuilderFolder for NoFolder {}`; new typed hooks (`fold_int_bin_op<W>`, `fold_fp_bin_op<K>`, `fold_int_cmp<W>`, `fold_cast_to_int<W>`, ...) let a custom folder return a statically-typed `IntValue<W>`/`FloatValue<K>` instead of an erased `Value`. The builder does **not** take that static marker on trust: its `accept_folded_*` helpers re-check the fold result's runtime type against the operand's (or the cast's destination) for *every* marker, static ones included -- a static `W` is only as honest as whoever built the handle, and the crate-internal `IntValue::from_value_unchecked` mints an `IntValue<W>` without consulting the payload's real type. `ConstantFolder`'s typed overrides delegate to the erased `*_dyn` hooks and re-type the result through `W::narrow`/`K::narrow` (checked at the point of construction), rather than rewrapping unchecked on the authority of a prose invariant audit. New `typed_pointer_value.rs` ships [`crate::TypedPointerValue<'ctx, T: IrField, B>`] (Rust-side pointee-schema overlay on a plain opaque pointer; `build_typed_alloca`/`build_typed_load`/`build_typed_store`/`build_field_gep::<S, I>` skip the erased path's runtime narrow entirely) -- distinct from the pre-existing IR-level [`crate::TypedPointerType`] (GPU-only typed-pointer *type*, which changes printed IR; the two now cross-link in their module docs). Fixed a real GEP address-space bug (`build_gep_inner` was hard-coding address space 0 instead of preserving the base pointer's address space) and a `SelectArm` forging hole (`SelectArm::from_select_value` now requires a `pub(crate)`-constructed evidence token). Flag parity additions: construction-time `samesign` (icmp), `nneg` (zext/uitofp), and `nuw`/`nsw` (trunc, never silently dropped per Doctrine D10) as typed variants alongside the pre-existing `_dyn` forms.
- **End-to-end typed calls (Workstream 1)**. `build_call` previously accepted erased arguments with no compile-time arity or type checking, and `CallInst<R>`'s return marker was caller-asserted rather than derived. [`crate::IntoCallArg<'ctx, P: FunctionParam, B>`] is a deliberately **open** per-position lift trait (macro-generated blanket impls over the existing `IntoIntValue`/`IntoFloatValue`/`IntoPointerValue` families plus derive-emitted impls for struct-schema slots, `#[diagnostic::on_unimplemented]`-annotated); only [`crate::CallArgs<'ctx, Params: FunctionParamList, B>`] is sealed -- a tuple trait covering arities 0..=16, so a wrong argument count has no impl (compile error) and a wrong-typed position fails its `IntoCallArg` bound (compile error). `FunctionReturn` gained an associated `CallResult` type so the callee's schema derives the call's return shape; the new [`crate::TypedCallInst<'ctx, Ret: FunctionReturn, B>`] handle wraps `CallInst<'ctx, Ret::Marker, B>` with an infallible `result()` accessor -- no `try_into`. (Since 2.0 `build_call` hands back a `TypedCallInstId`, so the call site reads `m.view(call).result()`.) `build_call`/`build_call_with_config`/`build_varargs_call`/`build_indirect_call::<Sig>`/`build_invoke` all take the typed path by default; the `_dyn` fallbacks (`build_call_dyn`, `build_indirect_call_dyn`, `build_invoke_dyn`) keep the erased path but now run a new `validate_call_site_args` check at build time instead of deferring wrong-arity/wrong-type calls to the verifier.
- **Auto-SSA frontend (Workstream 3)**. New `crates/llvmkit-ir/src/ssa_builder.rs` implements Braun et al.'s 2013 on-the-fly SSA construction on top of the typed `IRBuilder` (nearest LLVM relative: `lib/Transforms/Utils/SSAUpdater*.cpp`; nearest Rust relative: cranelift-frontend's `FunctionBuilder`). [`crate::SsaBuilder`] exposes `Copy` typed-variable handles (`IntVariable<W>`, `FloatVariable<K>`, `PointerVariable`) that `def_*_var`/`use_*_var` like mutable locals -- no manual `build_int_phi` + `add_incoming` wiring. `create_block` auto-seals the entry block; `seal_block` completes a block's incomplete phis once its predecessor set is fully known; `finish()` is the always-correct seal-everything fallback. Since 0.0.4 cycle D the builder is **one type whose insertion point is data** -- `switch_to_block` and every terminator take `&mut self`, and an operation without an insertion point is `SsaUnpositioned` rather than an absent method (the crate's `_dyn` convention, applied to the one tool whose job is a CFG discovered at run time; the plain `IRBuilder`'s linear block token and terminator-consuming cursor are untouched, and `SsaBuilder::ins()` still returns a *borrow*, so the plain builder's consuming terminators stay unreachable through this layer). The Braun bookkeeping lives in an owned, `Send`, `Clone`, lifetime-free `SsaState<B>` a caller stores in a struct field and snapshots per branch; working builders are minted from `(&module, function, &mut state)` — two steps, both spelled `for_function`: `SsaState::for_function(&m, m.view(f))?` opens the state, then `SsaBuilder::for_function(&m, m.view(f), &mut state)?` mints a builder over it (worked form in `examples/factorial_auto_ssa.rs`). `examples/lifter_session.rs` is the consumer proof: a suspend/resume lifter as a plain movable value. Braun sealing remains runtime state with typed errors (`SsaBlockAlreadySealed`, `SsaBranchToSealedBlock`, `SsaUseOfUndefinedVariable`, ...) because "all predecessors known" is inherently a property of future calls, not a type. `examples/factorial_auto_ssa.rs` is byte-parity locked against the pre-existing manual-phi `examples/factorial.rs` -- same printed `.ll`, no phi/label plumbing. Currently ships int/float/pointer variables and `br`/`cond_br`/`switch`/`ret`/`ret_void`/`unreachable` terminators (aggregate variables and invoke/EH terminators are future work, see `docs/future-work.md`).

- **Pass-facing type safety** ([`docs/design/pass-facing-type-safety.md`](docs/design/pass-facing-type-safety.md)). Total `InstructionView::classify() -> Classified{Inst,Term}` over exhaustive `InstructionKind`/`TerminatorKind` with `CastKind`/`PhiKind` sub-enums and grammar-typed operands (`load.pointer() -> PointerValue`, `CallInst::classify_callee() -> Callee`). A `PatternMatch.h`-style combinator DSL whose matchers RETURN their bindings as a flat tuple (`matchers.rs`: `m_add`/`m_c_add`/`m_one_use`/`m_all_ones`/...). Pass ergonomics: a *witnessed* dirty-bit (`done()` reports the rung floor only when a mutation actually happened), `FnPatch::erase` accepts only a `NonTerminator` (terminator-erase is a compile error), and a mutation-driven worklist + erase-safe cursor that put `DcePass`/`InstSimplifyPass` on an O(n)-amortized seed-then-drain loop (byte-identical output). Framework-*witnessed* analysis preservation: `FnReshape` records `CfgUpdate`s, `CfgIncremental` analyses repair, the driver marks preserved only what it watched repair, and reading a stale CFG analysis mid-reshape is a compile error (`FnReshape::analysis_repaired`); a `Requires` list no longer needs `Default` (`PrefetchableAnalysis`).

The const-generic `VectorType<E, Len<N>>` / `ArrayType<E, ArrLen<N>>` retrofit (T4 follow-up) **shipped** for fixed-length vectors and arrays (`feature-17`; element + length markers, `build_vec_int_*` / `build_vec_extract` / `build_vec_insert` / `build_vec_splat` / `build_arr_extract` / `build_arr_insert` making element/length mismatch a compile error). Its residual stays erased / `Dyn`: **scalable vectors** (always `Dyn`), **pointer-element vectors** (blocked on address-space markers), **composite-element arrays** (`[N x {..}]` / `[N x [..]]` / `[N x <..>]`), **float / div / rem vector binops and vector `icmp`/`fcmp`** (no erased `_dyn` lowering to reuse), and the length-relating ops (shufflevector output length, concat `N1+N2`, compile-time index-in-bounds, cross-`Len` widen/narrow) that are **blocked on `generic_const_exprs` (unstable)**. Still ahead beyond that: a broader optimization-transform library (a fold-to-constant `InstSimplifyPass` and `DcePass` ship today), bitcode, debug info, and fuller intrinsics coverage. See [`docs/future-work.md`](docs/future-work.md) for the full session-end roadmap (killer-feature designs, upstream coverage gaps, ergonomics backlog, and this session's own punted items).

Workspace shape (see each crate's `Cargo.toml` for details):

- Root `Cargo.toml` carries `[workspace]` metadata plus the shared
  `[workspace.package]` / `[workspace.dependencies]` tables. Every member
  inherits `version` / `edition` / `rust-version` from it.
- `llvmkit/` — the public umbrella crate; re-exports `llvmkit-ir`, `llvmkit-support`, and `llvmkit-asmparser` as `llvmkit::ir` / `llvmkit::support` / `llvmkit::asmparser`. `default-members` points at it so plain `cargo run` / `cargo doc` resolve here.
- `crates/llvmkit-ir/` — the IR data model, builder, verifier, AsmWriter, analyses, and pass layer. The bulk of the workspace.
- `crates/llvmkit-support/` — shared helpers (`Span`, `Spanned<T>`, `SourceMap`).
- `crates/llvmkit-asmparser/` — textual IR lexer and `.ll` parser.
- `crates/llvmkit-macros/` — the proc-macro crate behind `#[derive(Branded)]`, `#[derive(IrStruct)]`, `#[function_pass]`, and `#[module_pass]`. A **required** dependency of `llvmkit-ir` and `llvmkit-asmparser` (proc-macro crates are build-time only — they contribute nothing to the built artifact); the `macros` feature now gates only the *user-facing* re-exports `IrStruct` / `function_pass` / `module_pass`. `Branded` is re-exported crate-internally as `pub(crate) use llvmkit_macros::Branded` so every use site reads `use crate::Branded;`.

Reference C++ tree at `orig_cpp/llvm-project-llvmorg-22.1.4/` is **read-only**:
never modified, never built, never shipped. `compile_commands.json` for clangd
navigation is generated under `build/llvm/` (also gitignored).

## Reference C++ Tree (`orig_cpp/`)

The canonical implementation lives at:

```
orig_cpp/llvm-project-llvmorg-22.1.4/llvm/
```

Only the `llvm/` subdirectory matters. `clang/`, `mlir/`, `lld/`, `lldb/`, `flang/`, `polly/`, `bolt/`, `compiler-rt/`, `libc*/`, `libcxx*/`, `runtimes/`, and friends are **out of scope** — do not read them when porting features.

When porting, anchor the work on these files:

### IR core data model

| Concept | Headers | Implementation |
|---|---|---|
| `LLVMContext` (interning, uniquing) | `llvm/include/llvm/IR/LLVMContext.h` | `llvm/lib/IR/LLVMContext.cpp` |
| `Type`, `IntegerType`, `FunctionType`, `StructType`, `ArrayType`, `VectorType`, `PointerType` | `llvm/include/llvm/IR/Type.h`, `DerivedTypes.h` | `llvm/lib/IR/Type.cpp` |
| `Value` / `User` / `Use` | `llvm/include/llvm/IR/{Value,User,Use}.h` | `llvm/lib/IR/{Value,User,Use}.cpp` |
| `Constant`, `ConstantInt`, `ConstantFP`, `ConstantExpr`, `ConstantData*` | `llvm/include/llvm/IR/{Constant,Constants}.h` | `llvm/lib/IR/Constants.cpp` |
| `Module`, `Function`, `BasicBlock`, `Argument` | `llvm/include/llvm/IR/{Module,Function,BasicBlock,Argument}.h` | `llvm/lib/IR/{Module,Function,BasicBlock}.cpp` |
| `GlobalValue`, `GlobalObject`, `GlobalVariable`, `GlobalAlias`, `GlobalIFunc` | `llvm/include/llvm/IR/Global*.h` | `llvm/lib/IR/Globals.cpp` |
| `Instruction` (base) | `llvm/include/llvm/IR/Instruction.h`, `InstrTypes.h` | `llvm/lib/IR/Instruction.cpp` |
| Concrete instructions (Load, Store, Alloca, Br, Phi, Switch, Call, …) | `llvm/include/llvm/IR/Instructions.h` (~5k lines) | `llvm/lib/IR/Instructions.cpp` |
| Operator wrappers (`Operator`, `OverflowingBinaryOperator`, …) | `llvm/include/llvm/IR/Operator.h` | `llvm/lib/IR/Operator.cpp` |
| `IntrinsicInst` (memcpy, dbg.value, …) | `llvm/include/llvm/IR/IntrinsicInst.h` | `llvm/lib/IR/IntrinsicInst.cpp` |
| `IRBuilder` + folders | `llvm/include/llvm/IR/IRBuilder.h`, `IRBuilderFolder.h`, `ConstantFolder.h`, `NoFolder.h` | `llvm/lib/IR/IRBuilder.cpp` |
| `Verifier` | `llvm/include/llvm/IR/Verifier.h` | `llvm/lib/IR/Verifier.cpp` |

### Textual IR (`.ll`)

| Concept | Headers | Implementation |
|---|---|---|
| Lexer | `llvm/include/llvm/AsmParser/{LLLexer,LLToken}.h` | `llvm/lib/AsmParser/LLLexer.cpp` |
| Parser (entry: `parseAssembly*`) | `llvm/include/llvm/AsmParser/{LLParser,Parser}.h` | `llvm/lib/AsmParser/{LLParser,Parser}.cpp` (LLParser.cpp is ~11k lines) |
| Slot numbering / mapping | `llvm/include/llvm/AsmParser/{SlotMapping,NumberedValues}.h`, `llvm/include/llvm/IR/ModuleSlotTracker.h` | `SlotTracker` lives **inside** `llvm/lib/IR/AsmWriter.cpp` (not exported) |
| Optional source-location capture | `llvm/include/llvm/AsmParser/{AsmParserContext,FileLoc}.h` | `llvm/lib/AsmParser/AsmParserContext.cpp` |
| Printer / `Module::print` / `Value::print` | `llvm/include/llvm/IR/AssemblyAnnotationWriter.h` | `llvm/lib/IR/AsmWriter.cpp` (~5.5k lines) |
| Format-dispatch wrapper (sniffs bitcode magic, falls back to `.ll`) | `llvm/include/llvm/IRReader/IRReader.h` | `llvm/lib/IRReader/IRReader.cpp` |

Bitcode magic is detected via `isBitcode()` in `llvm/include/llvm/Bitcode/BitcodeReader.h`. Wrapper magic: `0x0B 0x17 0xC0 0xDE`; raw magic: `0xBC`.

### Bitcode (deferred, but mapped)

| Concept | Headers | Implementation |
|---|---|---|
| Bitstream framing | `llvm/include/llvm/Bitstream/{BitstreamReader,BitstreamWriter,BitCodeEnums,BitCodes}.h` | `llvm/lib/Bitstream/Reader/BitstreamReader.cpp` |
| Bitcode reader | `llvm/include/llvm/Bitcode/BitcodeReader.h` | `llvm/lib/Bitcode/Reader/{BitcodeReader,MetadataLoader,ValueList}.cpp` |
| Bitcode writer | `llvm/include/llvm/Bitcode/BitcodeWriter.h` | `llvm/lib/Bitcode/Writer/{BitcodeWriter,ValueEnumerator}.cpp` |
| Record / block IDs | `llvm/include/llvm/Bitcode/{LLVMBitCodes,BitcodeCommon,BitcodeConvenience}.h` | — |

### Support utilities (cherry-pick only what IR/AsmParser needs)

Do **not** port the whole of `llvm/Support/`. The narrow slice that matters:

| Header | Purpose | Rust counterpart |
|---|---|---|
| `Support/MemoryBuffer.h`, `MemoryBufferRef.h` | File / buffer loading | `std::io::Read` / `BufRead`, `&[u8]`, `Cow<[u8]>` |
| `Support/raw_ostream.h` | Buffered text output | `std::io::Write`, `std::fmt::Write` |
| `Support/Error.h`, `ErrorOr.h`, `ErrorHandling.h` | Recoverable + fatal errors | `Result<T, E>` with crate-level error enum (`thiserror` is acceptable but not required) |
| `Support/SourceMgr.h`, `SMDiagnostic` | Diagnostic locations | Custom `Diagnostic { span, severity, message }` struct |
| `Support/Casting.h` (`isa`/`cast`/`dyn_cast`) | RTTI-free polymorphism | `match` on the relevant Rust enum — usually unnecessary because variants are explicit |
| `Support/Endian.h`, `MathExtras.h` | Bit twiddling for bitstream | `u*::from_le_bytes`, `byteorder` crate (or hand-rolled) |
| `ADT/StringRef.h`, `ArrayRef.h`, `SmallVector.h` | Borrowed / small-buffer collections | `&str`, `&[T]`, `Vec<T>`, `smallvec::SmallVec` |

## Workspace Layout

Each implementation crate's `src/` directory mirrors the matching LLVM C++
tree **file-for-file**: `Foo.h` + `Foo.cpp` collapse into `foo.rs` (snake_case).
If a translation unit genuinely benefits from a split, use the modern Rust
2018 module form: `foo.rs` at the parent level **plus** a `foo/` directory
containing private helper files — the parent `foo.rs` stays the canonical
navigation entry-point.

Current shape (abridged; files are listed when they are useful navigation
anchors, not as an exhaustive inventory):

```
<repo root>/
├── Cargo.toml                       # [workspace] + shared package/dependency tables
├── README.md                        # user-facing docs + the Type-Safety Doctrine prose
├── CHANGELOG.md                     # Keep a Changelog; user-visible changes
├── ROADMAP.md
├── UPSTREAM.md                      # per-test provenance registry (Doctrine D11)
├── LICENSE
├── AGENTS.md
├── INKWELL_MIGRATION.md
├── docs/                            # design notes: future-work, pass-facing type safety, …
├── llvmkit/                         # umbrella crate
│   ├── Cargo.toml
│   └── src/lib.rs
└── crates/
    ├── llvmkit-support/
    │   └── src/
    │       ├── lib.rs
    │       ├── span.rs              # Span + Spanned<T>
    │       └── source_map.rs        # byte-offset → (line, col)
    ├── llvmkit-macros/              # proc macros: Branded, IrStruct, function_pass, module_pass
    │   └── src/
    │       ├── lib.rs
    │       ├── ir_struct.rs
    │       ├── function_pass.rs
    │       ├── module_pass.rs
    │       └── pass_macro_shared.rs
    ├── llvmkit-ir/                  # IR data model
    │   ├── Cargo.toml
    │   ├── src/
    │   │   ├── lib.rs
    │   │   ├── type.rs              # Type + TypeData + IrType / TypeKind
    │   │   ├── derived_types.rs     # IntType/FloatType/StructType/VectorType/ArrayType
    │   │   ├── typed_pointer_type.rs # TypedPointerType (IR-level typed pointer)
    │   │   ├── typed_pointer_value.rs # TypedPointerValue (Rust-side pointee schema)
    │   │   ├── module.rs            # Module + ModuleId + ModuleRef + ModuleView + module_new!
    │   │   ├── value_id.rs          # the storable id family + ViewIn (sealed)
    │   │   ├── llvm_context.rs      # type/value arenas + intern maps
    │   │   ├── calling_conv.rs      # CallingConv newtype
    │   │   ├── cmp_predicate.rs     # IntPredicate + FloatPredicate
    │   │   ├── attributes.rs        # AttrKind / Attribute / AttributeList / AttributeStorage
    │   │   ├── attribute_mask.rs    # AttributeMask bitflags
    │   │   ├── fmf.rs               # FastMathFlags
    │   │   ├── gep_no_wrap_flags.rs # GepNoWrapFlags
    │   │   ├── error.rs             # IrError + TypeKindLabel + ValueCategoryLabel
    │   │   ├── value.rs             # Value + IntValue/FloatValue/… + sealed traits
    │   │   ├── use.rs               # Use (transient view)
    │   │   ├── user.rs              # sealed User trait
    │   │   ├── debug_loc.rs         # opaque DebugLoc placeholder
    │   │   ├── metadata.rs          # metadata arena (crate-internal MetadataSlot) + the public MetadataId<B>
    │   │   ├── basic_block.rs       # BasicBlock + BasicBlockLabel + IntoBasicBlockLabel
    │   │   ├── block_params.rs      # BlockParams schema markers
    │   │   ├── value_symbol_table.rs # per-function name lookup
    │   │   ├── constant.rs          # Constant + IsConstant
    │   │   ├── constants.rs         # ConstantInt/Float/… refinements + ctors
    │   │   ├── constant_fold.rs     # ConstantFold.cpp pure folds
    │   │   ├── global_value.rs      # Linkage / Visibility / DllStorageClass / ThreadLocalMode
    │   │   ├── global_variable.rs   # GlobalVariable + GlobalBuilder + GlobalVariableData
    │   │   ├── global_alias.rs      # GlobalAlias
    │   │   ├── global_ifunc.rs      # GlobalIFunc
    │   │   ├── comdat.rs            # SelectionKind + ComdatRef + ComdatData + ComdatId
    │   │   ├── unnamed_addr.rs      # GlobalValue::UnnamedAddr
    │   │   ├── argument.rs          # Argument handle
    │   │   ├── function.rs          # FunctionValue<'ctx, R, B> + FunctionBuilder<'ctx, R, B>
    │   │   ├── function_signature.rs # FunctionParam(List) / FunctionReturn / CallArgs
    │   │   ├── marker.rs            # ReturnMarker + Dyn + Ptr (top-level)
    │   │   ├── align.rs             # Align + MaybeAlign (Support/Alignment.h)
    │   │   ├── instruction.rs       # Instruction + InstructionView + Kind/TerminatorKind
    │   │   ├── instr_types.rs       # BinaryOpData / CastOpData / CastOpcode / ReturnOpData payloads
    │   │   ├── instructions.rs      # AddInst/SubInst/MulInst/CastInst/RetInst handles
    │   │   ├── operator.rs          # OverflowingBinaryOperator view
    │   │   ├── ir_builder.rs        # IRBuilder<'m, 'ctx, B, F, S, R> typestate
    │   │   ├── ir_builder/
    │   │   │   ├── folder.rs        # IRBuilderFolder trait
    │   │   │   ├── constant_folder.rs # default folder
    │   │   │   └── no_folder.rs     # no-op folder
    │   │   ├── ssa_builder.rs       # Braun-style auto-SSA over the typed IRBuilder
    │   │   ├── asm_writer.rs        # AsmWriter.cpp — byte-identical printing
    │   │   ├── verifier.rs          # Verifier.cpp
    │   │   ├── cfg.rs               # predecessor/successor queries
    │   │   ├── dominator_tree.rs    # recompute-on-demand dominance
    │   │   ├── analysis.rs          # analysis managers + markers
    │   │   ├── pass_access.rs       # the capability rungs
    │   │   ├── pass_context.rs      # FnPatch / FnReshape / ModRewrite contexts
    │   │   ├── pass_manager.rs      # run_*_pass, tuple pipelines, Dyn containers
    │   │   ├── pass_pipeline.rs     # data-only textual pipeline names / recipes
    │   │   ├── matchers.rs          # PatternMatch.h-style combinators
    │   │   ├── dce.rs               # DcePass
    │   │   └── inst_simplify.rs     # InstSimplifyPass
    │   ├── examples/                # the printed-IR ones are byte-locked by a test
    │   │   ├── build_add_function.rs # cargo run --example build_add_function
    │   │   ├── cpu_state_add.rs     # multi-fn / params / unnamed_addr / trunc demo
    │   │   ├── factorial.rs         # phi + br + icmp + mul + sub loop demo
    │   │   ├── factorial_auto_ssa.rs # SsaBuilder twin, byte-locked against factorial.rs
    │   │   ├── lifter_session.rs    # suspend/resume lifter — the owned-module proof
    │   │   ├── module_per_batch.rs  # many modules, one brand rung
    │   │   ├── concurrent_counter.rs # fence + atomicrmw + switch (Open/Closed) demo
    │   │   ├── derived_struct_function.rs # #[derive(IrStruct)] schema demo
    │   │   ├── typed_vector_array.rs # const-generic vector / array ops
    │   │   ├── pass_manager_demo.rs  # capability-graded single-pass driver demo (DominatorTreeAnalysis + Inspect/PatchBody rungs)
    │   │   └── authored_pass.rs      # #[function_pass] / #[module_pass] authoring-sugar demo
    │   └── tests/                   # ~100 integration files; a few anchors:
    │       ├── asm_writer_basic.rs
    │       ├── vertical_slice.rs
    │       ├── module_brands.rs     # the three brand rungs + registry errors
    │       ├── module_ownership.rs  # owned / movable / Send module
    │       ├── id_roundtrip.rs      # id → view → id
    │       ├── globals_basic.rs
    │       ├── typestate_compile_fail.rs # the trybuild driver
    │       └── compile_fail/        # 83 trybuild fixtures (82 compile_fail + 1 pass)
    └── llvmkit-asmparser/
        ├── README.md
        ├── src/
        │   ├── lib.rs
        │   ├── ll_lexer.rs          # LLLexer.h + LLLexer.cpp
        │   ├── ll_parser.rs         # LLParser.h + LLParser.cpp
        │   ├── parser.rs            # Parser.h + Parser.cpp facade
        │   ├── asm_parser_context.rs # AsmParserContext.{h,cpp}
        │   ├── file_loc.rs          # FileLoc.h
        │   ├── slot_mapping.rs      # SlotMapping.h
        │   ├── numbered_values.rs   # NumberedValues.h
        │   ├── module_summary.rs    # summary/index record placeholders parsed from .ll
        │   ├── parse_error.rs       # parser diagnostics
        │   ├── ll_token.rs          # LLToken.h
        │   └── ll_lexer/            # private lexer helpers
        ├── examples/
        │   ├── demo.ll
        │   ├── parser_demo.ll
        │   ├── lex_file.rs          # cargo run --example lex_file -- file.ll
        │   └── parse_file.rs        # cargo run --example parse_file -- file.ll
        └── tests/
            ├── lexer_integration.rs
            ├── parser_*.rs          # facade, module, function-body, metadata, EH, summary coverage
            ├── parser_corpus.rs     # manifest-driven round-trip corpus
            └── fixtures/            # parser corpus inputs and expected output
```

Future work — each entry pairs to a single LLVM C++ file or subsystem:

| Future Rust file / subsystem                         | LLVM source                          |
|------------------------------------------------------|--------------------------------------|
| `crates/llvmkit-ir/src/intrinsic_inst.rs`            | `IR/IntrinsicInst.{h,cpp}`           |
| `crates/llvmkit-ir/src/assembly_annotation_writer.rs` | `IR/AssemblyAnnotationWriter.h` |
| `crates/llvmkit-bitcode/` (new crate)                | `lib/Bitcode/`, `lib/Bitstream/`     |
| optimization transforms / pipeline builders          | `lib/Transforms/`, new PM builders   |

**Do not add empty stub files.** A file in the tree should reflect existing
behavior; placeholders that pretend to do work are a smell. The future-files
list above is the authoritative roadmap; consult it before introducing a new
Rust filename.

## Lexer API at a glance

```rust
use llvmkit_asmparser::ll_lexer::{Lexer, LexError};
use llvmkit_asmparser::ll_token::Token;
use llvmkit_asmparser::read_to_owned;

// In-memory string — the most ergonomic shape:
let mut lex = Lexer::from("@x = i32 42");
while let Some(tok) = lex.next() { /* Result<Spanned<Token>, LexError> */ }

// Borrowed byte slice — the canonical constructor:
let bytes: Vec<u8> = std::fs::read("foo.ll")?;
let lex = Lexer::new(&bytes);

// Any `Read` source via the documented helper:
let bytes = read_to_owned(some_reader)?;
let lex = Lexer::new(&bytes);
```

Token payloads borrow from the source via `Cow<[u8]>`; quoted forms with
`\\xx` escapes are the only path that allocates.

## Type-Safety Doctrine (D1-D11)

Eleven rules govern every API in `llvmkit`. They are NOT optional and are NOT graded by API. Cite them by id (`D1`-`D11`) in code reviews and commit messages. The full prose lives in `README.md`; the short version:

1. **D1.** State machines are typestates (no `is_attached()` predicates).
2. **D2.** Linear-typed handles for irreversible operations (`!Copy` + consume `self`).
3. **D3.** Erased forms are explicitly opt-in (`Dyn` companion only on request).
4. **D4.** Result types reflect operand types (`build_int_add::<i32, _, _, _>` yields an `IntValueId<i32, B>`).
5. **D5.** Operand registration is structural (one place per primitive; exhaustive `match`).
6. **D6.** Aggregate types parameterise over element shape (`VectorType<E, L, B>`, etc.).
7. **D7.** Cross-module mixing is rejected. Every handle and id carries the owning module's brand — a `'static` *type* `B` — so two modules with **distinct** brand types cannot exchange operands: a type error, no runtime check. Where two modules deliberately **share** a brand (every `Module::dynamic` is `DynBrand`; a named brand is re-issued after the previous module drops), the compile-time half cannot apply and the `ModuleId` tag on every id is the backstop — `IrError::ForeignValueId`, `None` from `try_view`, or a deterministic `view` panic. **Metadata is in scope too**, as of the 0.0.4 freeze: the public currency is `MetadataId<B>` (`{ tag: ModuleId, slot: MetadataSlot }`), so a mix-up across named brands is a type error and within one brand it is `IrError::ForeignMetadataId`, raised at a single arena choke point (`MetadataId::into_stored` / `ModuleCore::metadata_slot_of`). `IrError::UnknownMetadataSlot` now reports only a *native* id whose slot is out of range. The remaining carve-out is the same-brand one above: two `DynBrand` modules are separated by the tag, not by the type.
8. **D8.** Verified guarantees flow through typestate: verified-only analyses require `Module<B, Verified>`, and a pass pipeline's output module is Verified iff every member ran at the read-only (`Inspect`) rung — any mutating pass derives `Module<B, Unverified>` until `verify()` runs again. The output typestate is derived from the passes' capability rungs, never hand-declared.
9. **D9.** Iteration safety is structural (`BlockCursor`'s consume-on-step).
10. **D10.** No undefined behaviour, by design (Cranelift-style; no silent bad-codegen).
11. **D11.** Tests are ported, not invented (every `#[test]` cites upstream; registry at `UPSTREAM.md`).

Violating any of these is a defect, not a stylistic gap. New code that introduces a new state machine, a new operand-bearing instruction, or a new aggregate type **MUST** check itself against the relevant doctrine bullet before landing.


## Rust Idioms & Translation Patterns

These are the rules that turn a literal C++ port into idiomatic Rust. Apply them consistently.

### Make invalid states unrepresentable

C++:

```cpp
llvm::Value *v = ...;
if (v->getType()->isFloatTy()) {
    // user must remember to check
}
```

Rust:

```rust
match value.ty() {
    Type::Float(FloatKind::F32) => { /* the check IS the match arm */ }
    _ => { /* every other case forced into existence by the compiler */ }
}
```

When LLVM uses `getOpcode()` + downcasting, prefer a single `enum Instruction { Add(BinOp), Load(LoadInst), ... }` over a trait-object hierarchy. Reach for `Box<dyn Trait>` only when an open set of plugins is genuinely required — IR opcodes are a closed set.

### `Result` instead of `bool` + out-params

C++ patterns like `bool parseFoo(Foo &out, SMDiagnostic &err)` become:

```rust
fn parse_foo(input: &mut impl BufRead) -> Result<Foo, ParseError>;
```

A single crate-level `enum Error` with variants per failure mode is preferred. Wrap third-party errors with `#[from]` so `?` works.

### Generic I/O via traits, not file paths

C++ takes `const char *Filename` or `MemoryBufferRef`. Rust takes:

- `impl AsRef<Path>` for filesystem entry points (`parser::parse_file_branded` / `parse_file_dynamic` / `parse_assembly_file`).
- `impl AsRef<[u8]>` for in-memory parser variants (`parse_branded` / `parse_dynamic` / `parse_assembly`), with a `&str` convenience twin where it pays (`parse_assembly_string`).
- `impl Read` helpers should read once into owned bytes before handing a borrowed slice to the lexer / parser (`llvmkit_asmparser::read_to_owned`).
- `fmt::Display` (`format!("{module}")`) for printers until a dedicated `Write` facade exists.

Prefer the closure-free entry points: `parse_branded::<B>` / `parse_dynamic` / `parse_file_*` / `parse_into` all return the **owned** `Module<B, Unverified>`. The `parse_assembly*` family still takes a closure, and the reason is not the brand — `ParsedModule` holds borrowing handles into the module, so handing both back would be a self-reference.

This mirrors `serde_json::from_reader` / `from_slice` / `from_str`. **Default to streaming**; load into a `Vec<u8>` only when the parser genuinely requires random access.

### Conversions via `From` / `TryFrom`

- Infallible widening (`i32 → ConstantInt`) → `From`.
- Fallible narrowing (`Type → IntegerType`) → `TryFrom` returning `Result<_, TypeMismatch>`.

Avoid bespoke `as_int_type()` / `is_int_type()` pairs when `TryFrom` covers the same intent.

### Interning and identity

LLVM's `LLVMContext` interns types and constants so pointer equality means semantic equality. This is settled, not an open choice: the arenas live in a crate-private `ModuleCore`, identity is an arena index, and the public surface is the id/view split described under **Project Status**.

- Internally: a `*Slot` index (`TypeSlot`, `ValueSlot`, `MetadataSlot`) into a module-owned arena. Cheap `Copy`, no lifetime.
- Borrowing handle: `(slot, ModuleRef<'ctx, B>)` — `Type<'ctx, B>`, `IntValue<'ctx, W, B>`, and friends. The borrow checker enforces "no handle outliving its module" (locked by `tests/compile_fail/view_cannot_outlive_its_module.rs`).
- Storable id: `(ModuleId, ValueSlot)` plus the brand — `ValueId<B>` and family. `Copy + Send + 'static`, resolved back through `m.view(id)`.

Apply the same three-layer shape to any new subsystem. Mixing the layers within one subsystem is a smell. Metadata was the last subsystem that had not finished the job — it was slot-only, with neither a tag nor a brand — and the 0.0.4 freeze closed it: `MetadataSlot` is crate-internal, `MetadataId<B>` is the public currency, and the arena holds the same vocabulary types at a crate-private `StoredBrand` so there is one definition per concept rather than a public type and a private twin. New metadata work keeps that shape.

### No `unsafe`, ever

Every workspace crate opens with `#![forbid(unsafe_code)]`. This is a hard rule, not a default to weigh against performance. LLVM C++ uses tagged pointers, hung-off operands, intrusive lists, and `union`-via-bitfields; **do not** transcribe these tricks — use safe Rust equivalents (`Vec`, `Box`, `Option`, `enum`, `Cell`). If a design appears to need `unsafe`, the design is wrong; dependencies (`boxcar`) may use it behind their own boundaries, we do not.

### No FFI, no `bindgen`, no `llvm-sys`

If a problem feels solvable only by linking against `libLLVM`, the answer is "read the C++ and reimplement it." This is the explicit point of the project.

### Multi-source operand traits

For every operand slot in the IR builder, the lift trait accepts every source that is *statically* the right shape, and only those. For `IntoIntValue<'ctx, W, B>` that is: the typed handle `IntValue<'ctx, W, B>` (identity), its storable id `IntValueId<W, B>`, the matching constant handle `ConstantIntValue<'ctx, W, B>`, and the Rust scalar literals that lift to `W`. `IntoFloatValue` / `IntoPointerValue` mirror that shape. The `try_into()?` boilerplate disappears at the call site, and the handle and id spellings are interchangeable there.

**What the trait deliberately does *not* accept** (the "no silent erasure" cut): an erased `Value`, an `Argument`, or an `Instruction` cannot fill a typed operand slot on its own — narrow it first (`let p: PointerValue = v.try_into()?;`, `IntValue::<W>::try_from`) or use the erased `_dyn` builder family. There is likewise no `IntValue<IntDyn> -> IntValue<W>` lift, and no implicit literal widening: `2i32` is `i32`, `2i64` is `i64`. These three traits are **sealed** — the accepted set is closed and cannot be extended downstream. `IntoCallArg` stays *open* (derive-emitted impls for struct-schema slots need it), but inherits the cut transitively: its int / float / pointer impls are blanket impls bounded by the sealed lift traits.

Each impl is a *concrete-type* impl - no overlap with the identity blanket, no `dyn` dispatch. Cross-module rejection lives inside the lift trait's `into_*_value(module)` method, not at every IRBuilder call site - one check, reused everywhere; a foreign id is `IrError::ForeignValueId`.

### Typed forms paired with `_dyn` fallbacks

Every method that produces a typed result ships in two shapes:

- the static-marker form (`build_int_load::<W>(ptr, name) -> IrResult<IntValueId<W, B>>`);
- the erased fallback (`build_int_load_dyn(ty, ptr, name)` takes an explicit `IntType<'ctx, IntDyn, B>` and returns `IrResult<IntValueId<IntDyn, B>>`).

Same posture for trunc / zext / sext / fp loads. (Phi is the exception: the pair exists, but only the `_dyn` half is public — see the phi bullet under **The 2.0 handle model**.) The dyn form keeps the runtime check; the typed form pins the invariant at compile time. The `_dyn` suffix marks the **erased** member of such a pair and nothing else — never abbreviate an unrelated name to `_dyn`, and never suffix the typed form.

### Builder pattern for variable-shape ops

Calls, GEPs, allocas, and any op with several optional knobs ship a chainable builder alongside the flat method:

- `b.build_call(typed_callee, (a, b), "r")?` for schema-typed construction — the argument tuple is compile-checked position-by-position against the callee's parameter schema.
- `b.build_call_dyn(callee, args, name)?` for homogeneous pre-widened arguments on runtime-shaped callees (parsed IR); validates arity/types at build time.
- `b.call_builder(callee).arg(a).arg(b).tail().calling_conv(cc).name("r").build()?` for mixed-type / mixed-flag construction on the erased path. `.arg<V: IntoErasedValue<'ctx, B>>(value)` is generic per call so heterogeneous argument lists work without trait objects.

The builder is a plain struct that accumulates state into a `Vec<ValueSlot>`; `.build()` performs cross-module checks once and emits the instruction.

### Sealed sum-of-categories traits

Where a single concept (`select`, future `freeze`) accepts any of int / float / pointer arms, define **one** sealed trait with an associated `Output` and ship **one** method:

```rust
pub trait SelectArm<'ctx, B: ModuleBrand>: Sized + select_arm_sealed::Sealed {
    type Output;
    #[doc(hidden)]
    fn from_select_value(v: Value<'ctx, B>, narrow: &SelectNarrow<'_>) -> Self::Output;
    #[doc(hidden)]
    fn arm_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}
// Each category is implemented for BOTH the borrowing handle and the storable
// id, and `Output` is the *id* either way, so the two spellings are
// interchangeable at the call site and at the binding.
impl<'ctx, W: IntWidth,  B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for IntValue<'ctx, W, B>   { type Output = IntValueId<W, B>;     ... }
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for FloatValue<'ctx, K, B> { type Output = FloatValueId<K, B>;   ... }
impl<'ctx,               B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for PointerValue<'ctx, B>  { type Output = PointerValueId<B>;    ... }
impl<'ctx, W: IntWidth,  B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for IntValueId<W, B>       { type Output = IntValueId<W, B>;     ... }
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for FloatValueId<K, B>     { type Output = FloatValueId<K, B>;   ... }
impl<'ctx,               B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for PointerValueId<B>      { type Output = PointerValueId<B>;    ... }
```

Each impl is concrete; the method monomorphises per arm category. Beats N per-category overload methods. `arm_value` takes a `ModuleRef` and returns `IrResult` for the same reason `IntoBasicBlockLabel::into_basic_block_label` does: an *id* arm has to be module-checked, and a foreign id is `IrError::ForeignValueId` rather than a silently same-numbered slot.

### Compile-time invariants via `const { assert!(...) }`

Stable Rust does not allow const-evaluated bounds in `where` clauses (`{ M > N }` needs unstable `generic_const_exprs`). The stable analogue is `const { assert!(...) }` inside the trait method body - monomorphisation evaluates the assertion at instantiation time. Under-spec'd instantiations are *compile* errors.

Used today by `Width<const N: u32>` for arbitrary integer widths:

```rust
impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for i32 {
    type Error = Infallible;
    fn into_constant_int(self, ty: IntType<'ctx, Width<N>>) -> Result<...> {
        const { assert!(N >= 32, "i32 lift to Width<N> requires N >= 32"); }
        // ...
    }
}
```

And by `Module::int_type_n::<N>()` for the range check (`MIN_INT_BITS..=MAX_INT_BITS`). Prefer this over runtime `IrError::InvalidIntegerWidth` when `N` is statically known.

## Code Conventions

- **Edition**: 2024. Use 2024-only features (e.g. `let chains` in stable form) when they help.
- **Naming**: standard Rust (`snake_case` items, `PascalCase` types, `SCREAMING_SNAKE_CASE` consts). Drop the `LLVM` prefix from ported names — `LLVMContext` becomes `Context`, `LLVMModule` becomes `Module`. The crate name already namespaces them.
- **Naming law: full words, no abbreviations.** Spell it out — `instruction`, not `inst`; `predecessor`, not `pred`; `metadata`, not `md` — in every new public and internal name. Upstream's abbreviated spellings are not a licence to copy them. The **one** sanctioned suffix is `_dyn`, and it means exactly one thing: this is the runtime-erased member of a typed/erased pair. The typed form never carries a suffix. Internal arena indices are `*Slot` (`ValueSlot`, `TypeSlot`, `MetadataSlot`); the public tagged ids are `*Id`. Do not blur the two.
- **Be honest about fallibility.** A signature's return type is a claim. Do not return `IrResult<T>` from an operation that cannot fail (`Module::dynamic` is infallible and says so), and never make a real failure disappear — no silent no-op, no swallowed error, no `Option` standing in for a diagnosable one. Cycle E fixed two of these: `Module::metadata_set` used to no-op on a bad slot and `named_metadata_add_operand` used to panic; both now return `IrError::UnknownMetadataSlot { index, len }`. The 0.0.4 freeze applied the same rule to the tag: every metadata API that *accepts* a `MetadataId` became fallible rather than silently resolving a foreign id (`IrError::ForeignMetadataId`). When a function's contract genuinely permits a panic, it is documented under a `# Panics` heading and paired with a fallible twin.
- **No silent erasure.** A typed handle or id never widens to an erased one implicitly, at an operand position or a return position. Erasure is spelled: `as_dyn()`, or a call to the `_dyn` method. If a generic narrow is involved, re-check the runtime type at the point of construction rather than trusting a caller-supplied marker.
- **Modules**: one concept per file; let modules grow before splitting them. `Instructions.h` is 5k lines because it pays for itself; do not pre-split into 40 stub files.
- **Errors**: one crate-level `enum Error` (or a small per-subsystem enum that flattens into it). Avoid `Box<dyn std::error::Error>` in public signatures.
- **Comments**: explain *why*, not *what*. When porting a non-obvious C++ trick, link the source file and the symbol — never the line number, which drifts between LLVM versions: `// Mirrors LLParser::parseTopLevelEntities (LLParser.cpp)`.
- **Public API**: re-export from `lib.rs`. Keep internal modules non-public until an external use case appears. Prefer the narrowest visibility that compiles: private first, then `pub(in super::some_module)` for a specific parent scope, then `pub(super)`, then `pub(crate)`, and plain `pub` only for real public API. Do not use `pub(crate)` as the default for intra-module sharing.
- **Public API shape**: user-facing alternate operations are separate methods, not `Option<T>` inputs (`set_*`/`clear_*`, `*_within_none`, `*_to_caller`). Public config/result structs keep fields private and expose constructors plus Rust API Guidelines C-GETTER accessors (`field()`, never `get_field()`). Internal ids and fields may stay direct when they are not user-facing; user-facing ids and other public data use idiomatic getters such as `id()`. Public signatures use imported type names instead of `crate::...` / `super::...`.
- **Prefer imports over fully qualified paths, and prefer precise parent-relative imports.** Import types, traits, and helper functions at the top of the module (`use super::metadata::MetadataKind;` from same-crate modules) instead of spelling `crate::metadata::MetadataKind` inline. For same-crate code, prefer direct `super::<sibling>` imports over `crate::...`, especially in `use` items; avoid broad `super::super::...` hops unless the file is genuinely nested two module levels below the target and there is no clearer local re-export/import path. Reserve `crate::...` for crate-root re-exports, doctest/user-facing examples, or paths that would become unclear or invalid parent-relative imports. Fully qualified paths are reserved for one-off disambiguation, macro hygiene, macro-generated code where imports would be misleading, or cases where an import would create a real name conflict. When editing code, do not introduce new fully qualified `crate::...` / `super::...` paths in signatures or ordinary expressions; add or extend a `use` item and use the imported name instead.
- **No `as` casts.** Use `From`/`Into` for infallible widening, `TryFrom`/`TryInto` for fallible narrowing, and method-style conversions (e.g. `u32::from(x)`, `usize::try_from(x)`) elsewhere. The `as` keyword silently truncates, changes signedness, and loses precision — every site is a footgun. If a conversion has no idiomatic counterpart (rare, e.g. deliberate truncation), wrap it in a small named helper with a one-line invariant comment.
- **No pointer-based identity in our code.** Identity flows through typed integer indices (`TypeSlot`, `ValueSlot`, `ModuleId`, and the public `*Id` family). No `core::ptr::eq`, no `&T as *const T`, no address hashing in user-written code. Library internals like `boxcar` may use raw pointers safely behind their `unsafe` boundaries — we do not. Identity comparisons derive from `PartialEq`/`Hash` on the index types.
- **No runtime panics in production code.** `expect`, `unwrap`, `panic!`, `unimplemented!`, `todo!` are forbidden in non-test paths. Real failures use `IrError` returned via `IrResult<T>`. `unreachable!("…invariant…")` is permitted **only** when the branch is provably dead by construction *and* there is no reasonable way to remove it via the type system; the message names the invariant in plain English. Test code (`#[cfg(test)]`, `tests/`, `examples/`) is exempt. The one deliberate exception in the public surface is `Module::view` / `ModuleView::view`, whose contract *is* a deterministic panic on a foreign tag or absent slot; it carries a `# Panics` section and `try_view` is its fallible twin. Do not add a second exception without the same pairing.
- **Rust scalar types double as IR markers** where natural. The integer-width markers are the Rust types themselves (`bool` for i1, `i8`/`i16`/`i32`/`i64`/`i128`); the IEEE binary32/binary64 float kinds are `f32`/`f64`. Marker structs (`int_width::IntDyn`, `float_kind::{Half, BFloat, Fp128, X86Fp80, PpcFp128, FloatDyn}`) cover only the cases without a Rust counterpart. `IntDyn` and `FloatDyn` are distinct types so trait coherence stays sane (a single shared `Dyn` would simultaneously implement `IntWidth` and `FloatKind`). The top-level [`marker::Dyn`] / [`marker::Ptr`] / `()` mark fully-erased / pointer / void return shapes respectively; the bare type acts as the [`ReturnMarker`] (no wrapper structs).
- **Name parameters use named generics**: function and IRBuilder instruction names are `Name: AsRef<str>` when an empty-name fast path can avoid allocation; module / function / block / parameter names are `Name: Into<String>` when stored unconditionally. Public argument-position polymorphism is spelled with named generics so explicit turbofish call sites remain possible.
- **Compile-time invariants on cast widths** flow through sealed marker traits. `int_width::WiderThan<W>` is implemented for every `(Wider, Narrower)` pair of static markers, and the IR builder uses it as a bound on `build_trunc<Src: WiderThan<Dst>, Dst>` (and inversely on `build_zext` / `build_sext`). The `_dyn`-flavoured fallbacks keep the runtime check for the genuinely-erased path.

- **No `#[allow(...)]` attributes anywhere.** Not `#[allow(dead_code)]`, not `#[allow(clippy::...)]`, not `#[allow(unused_imports)]`, not `#[allow(non_upper_case_globals)]`, not anything else. The compiler / clippy is a teammate; silencing it is silencing the codebase. If a lint fires, fix the code (rename the symbol, drop the dead code, restructure the type) instead of suppressing the lint. The only exception is per-bullet `#[deny(...)]` and `#![forbid(unsafe_code)]` which *strengthen* lints.
- **Static dispatch only in the public IR / builder surface.** All polymorphism flows through monomorphised named generics (`<T: Trait>` / `where T: Trait`) or sealed-trait blanket impls. **No `dyn Trait`, no `Box<dyn>`, no `&dyn`** in any IR-builder, value, type, or instruction surface. Where homogeneity forces a slice (e.g. `param_types: &[Type<'ctx, B>]`), the slice element is a concrete handle - never a trait object.
- **Per-opcode flag types over shared flag bags.** When LLVM's `Operator.h` distinguishes flag classes per opcode (`OverflowingBinaryOperator`, `PossiblyExactOperator`, ...), each flag class gets its own Rust struct exposing only the flags LLVM permits for it (`AddFlags { nuw, nsw }`, `UDivFlags { exact }`, ...). Don't ship a single `BinopFlags` requiring runtime validation against the opcode - the type system should make invalid combinations *unspellable*.
- **AsmWriter print form matches `lib/IR/AsmWriter.cpp` byte-for-byte.** Read the matching `printInstruction` arm before adding an opcode formatter, and lock at least one fixture against the upstream-canonical text via `assert_eq!(format!("{m}"), expected)`. Flag print order, whitespace, and trailing punctuation all match the C++ reference.

- **No emojis**, no decorative comments, no boilerplate `mod tests` blocks unless they contain real tests.

## Development Commands

Run from the repository root.

**Pin the toolchain: CI installs rustc 1.96.0 (`dtolnay/rust-toolchain@1.96.0`
in `.github/workflows/ci.yml`), so every gate runs as `cargo +1.96.0`.** This is
not optional hygiene. The trybuild `.stderr` fixtures under
`crates/llvmkit-ir/tests/compile_fail/` are blessed against that compiler, and a
newer rustc rewords diagnostics — running the suite unpinned produces mismatches
that look like real regressions and are not. If you ever see a `.stderr` diff,
re-run on `+1.96.0` before touching a fixture. There is **no** category of
"environmental" trybuild drift; that claim was investigated and disproved.

```bash
cargo +1.96.0 build                                                   # compile
cargo +1.96.0 build --release                                         # optimized build
cargo +1.96.0 test --workspace --all-targets --all-features           # run all tests
cargo +1.96.0 test <name>                                             # run tests matching a name
cargo +1.96.0 check --workspace --examples                            # type-check (fastest feedback)
cargo +1.96.0 clippy --workspace --all-targets --all-features -- -D warnings
cargo +1.96.0 fmt                                                     # format
cargo +1.96.0 fmt -- --check                                          # CI-style format check
cargo +1.96.0 test --workspace --doc --all-features                   # doctests
cargo +1.96.0 doc --workspace --no-deps --all-features                # rustdoc (CI runs with RUSTDOCFLAGS=-D warnings)
```

The full CI gate is exactly that list plus `cargo audit`; reproduce it locally
before pushing. Baseline on the pin: **0 trybuild failures of 83 registered
fixtures** (82 `compile_fail` + 1 `pass`).

There is no `build.rs`, no Make/CMake, no submodules. `orig_cpp/` is **not** built — never run `cmake` or `ninja` against it.

## Commits

- Conventional Commits: `type(scope): summary` — `feat`, `fix`, `docs`, `test`, `refactor`, `chore`. Append `!` after the scope for a breaking change (`feat(lookups)!: get_* returns ids`). The scope names the workstream or cycle slice.
- Cite the doctrine id (`D1`–`D11`) in the body whenever a change turns on one.
- Every user-visible change gets a `CHANGELOG.md` entry (Keep a Changelog format; the project is pre-1.0, so breaking changes are expected and are flagged inline).
- Every new `#[test]` gets its `UPSTREAM.md` row in the same commit (Doctrine D11).

## Testing & QA

The workspace ships a substantial test suite (1,500+ tests across `crates/*/tests/` plus per-module `#[cfg(test)]` blocks; the exact attribute-anchored count and per-test provenance live in `UPSTREAM.md`). The categories:

**Tests are ported, not invented.** Every new opcode, predicate, or instruction lands with tests sourced from one of the upstream LLVM trees:

1. **`orig_cpp/.../llvm/test/Assembler/*.ll`** - the canonical round-trip / format fixtures. Each `.ll` is `RUN: llvm-as | llvm-dis | FileCheck %s` upstream; the `; CHECK:` directives spell the canonical AsmWriter output. We can't run `llvm-as` (no parser yet), but we can build an equivalent module programmatically and assert `format!("{m}")` against the fixture body byte-for-byte. Copy the constructive subset to `tests/fixtures/llvm/<topic>.ll` with a leading comment block citing the upstream path.
2. **`orig_cpp/.../llvm/unittests/IR/*Test.cpp`** - GoogleTest-flavoured unit tests for `IRBuilder::Create*`, `ConstantInt::get`, etc. Each `TEST_F(*, Foo)` translates to a Rust `#[test]` mirroring the structural assertions (operand wiring, flag bits, result types).
3. **`orig_cpp/.../llvm/test/Verifier/*.ll`** - negative tests: malformed IR that LLVM's verifier rejects. Useful for `IrError` coverage on builder methods that surface domain rules.

**Do not invent `.ll` strings or test scenarios** unless upstream genuinely lacks coverage for the construct. When that happens, document the gap inline and cite the closest upstream test family (e.g. `IRBuilderTest::CreateStepVectorI3` for arbitrary-width tests).

**Test provenance registry.** Every `#[test]` in the workspace ships with a doc comment citing the upstream LLVM file, fixture, or `TEST(...)` it ports. The complete registry lives at `UPSTREAM.md` (repo root) and is the authoritative answer to "where does this test come from?". After adding a new test, append the row. Doctrine D11 (full prose in `README.md`, worked examples in `docs/type-safety-vs-llvm.md`) makes this rule mechanical: a test without a citation is a defect, not a stylistic gap.

Categories below are the *shape* of testing; their content always sources from the upstream tree above.

- **Unit tests** (`#[cfg(test)] mod tests` in each module) for type interning, constant folding, instruction construction.
- **Round-trip tests** — `crates/llvmkit-asmparser/tests/parser_corpus.rs` drives `tests/fixtures/parser_corpus_manifest.txt`: each row names a fixture, its upstream provenance, an optional `expect=<file>` of canonical AsmWriter output, and `status=pass|xfail-parse|xfail-verify`. Passing rows must parse, verify, and match the checked-in expected output byte-for-byte. The LLVM repo's own `llvm/test/Assembler/*.ll` files are good seed material — copy specific files in as needed; do not pull the whole `test/` tree.
- **Compile-fail tests** — `crates/llvmkit-ir/tests/typestate_compile_fail.rs` drives the trybuild fixtures in `tests/compile_fail/`. A new type-level law lands with a fixture that proves the *wrong* program does not compile; the blessed `.stderr` is part of the lock. Bless only on `+1.96.0`.
- **Byte-lock tests** — a printed-IR example under `examples/` is paired with a test asserting `format!("{m}")` against the expected text, so a formatting regression cannot land silently.
- **Conformance tests** for the parser by comparing against the C++ behavior described in `LLParser.cpp`. When the Rust parser disagrees with the reference, the reference wins unless the disagreement is a deliberate, documented Rust-side improvement.
- **Property tests** (`proptest`, a dev-dependency of `llvmkit-ir`; used today by `tests/ssa_builder.rs`) for generative coverage: build a random valid module, print it, parse it, assert structural equality.

Do not commit code that breaks any gate in **Development Commands** above — run them as `cargo +1.96.0`.

## Important Files

- Root `Cargo.toml` — workspace definition: `[workspace]` members + `[workspace.package]` (version, edition 2024, `rust-version = "1.96"`) + `[workspace.dependencies]`.
- `crates/<crate>/Cargo.toml` — per-crate manifest; pulls shared values via `workspace = true`.
- `crates/<crate>/src/lib.rs` — crate root. Each crate begins with `#![forbid(unsafe_code)]`. No workspace crate uses FFI or `llvm-sys`, and there is no `unsafe` block anywhere in our code.
- `.github/workflows/ci.yml` — the gate. Pins rustc **1.96.0**; run everything locally as `cargo +1.96.0`.
- `README.md` — user-facing docs and the authoritative prose for Doctrine D1–D11.
- `CHANGELOG.md` / `UPSTREAM.md` / `ROADMAP.md` / `docs/` — release history, per-test provenance, roadmap, design notes.
- `.gitignore` — ignores `/target`, `/orig_cpp/`, `/build/`. `Cargo.lock` is **committed** (the workspace ships binaries / examples).
- `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/` — read-only LLVM 22.1.4 reference. Treat as documentation, not as code.
- `build/llvm/compile_commands.json` — generated for clangd cross-navigation; do not commit.

## What an AI Assistant Should Do First

1. **Read the reference before editing.** When asked to port `Foo`, open the C++ header *and* the matching `.cpp` listed in the table above. The `.cpp` files contain invariants the `.h` doesn't show.
2. **Read the current signature, not a doc's memory of it.** The API froze at 0.0.4 after a broad reshape; any prose — including this file's historical workstream summaries — can lag the tree. `grep` the `pub fn` before you call it, and check **The 2.0 handle model** above before choosing between an id, a view, and a borrowing handle.
3. **Run gates as `cargo +1.96.0`.** A `.stderr` mismatch on an unpinned toolchain is not a regression.
4. **Search before inventing.** If you need a utility (e.g. small-string optimization, bit reader), check whether `std` or a well-known crate already provides it before writing it.
5. **Prefer one well-modeled subsystem over many half-modeled ones.** A complete, idiomatic `Type` + `Context` pair is more valuable than stubs for `Type`, `Value`, `Module`, `Function`, and `IRBuilder` simultaneously.
6. **Surface uncertainty.** If a C++ behavior is ambiguous (e.g. silent overflow vs. assertion), state the choice and the rationale in a comment. Do not silently pick.
7. **Do not import LLVM via FFI** to "validate" Rust output. The Rust implementation must stand on its own; cross-checking against `llc` / `opt` is fine as an external manual step but must not be a build dependency.
8. **LSP-first for cross-file refactors.** Use `lsp rename` for symbol renames (cross-crate, cross-module), `lsp references` to find every consumer before changing a public signature, `lsp diagnostics file:"*"` after substantive edits to catch stragglers, and `lsp code_actions` for missing-trait-impl / missing-import suggestions. Regex / `sed` is the right tool only for doc-comment markdown links and similar non-symbol text - never for code-symbol changes.
