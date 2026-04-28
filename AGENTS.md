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
- Not a code generator and not a target backend. `llvmkit` doesn't lower IR to machine code or link objects — use upstream LLVM (`llvm-sys`, `inkwell`) for that. Optimization / transform / analysis passes are *planned* future work, not excluded; they will land once the IR data model, builder, parser, and verifier are stable.

## Project Status

The repo is a Cargo workspace at `C:/Users/Aslan/llvmkit/`. Implemented today:

- The `.ll` lexer (`llvmkit-asmparser/src/ll_lexer.rs`).
- The IR data model with **width-typed integers** (`IntType<'ctx, W>`, `W in { bool, i8, i16, i32, i64, i128, IntDyn, Width<const N: u32> }`) and **kind-typed floats** (`FloatType<'ctx, K>`, `K in { f32, f64, Half, BFloat, Fp128, X86Fp80, PpcFp128, FloatDyn }`).
- Sealed marker traits: `IntWidth`, `StaticIntWidth`, `FloatKind`, `StaticFloatKind`, `WiderThan`, `FloatWiderThan`, `ReturnMarker`, `SelectArm`.
- Multi-source operand traits: `IntoIntValue<W>`, `IntoFloatValue<K>`, `IntoPointerValue`, `IntoConstantInt<W>`, `IntoConstantFloat<K>`, `IntoReturnValue<R>`.
- The full medium IRBuilder: every integer binop (`add`/`sub`/`mul`/`udiv`/`sdiv`/`urem`/`srem`/`shl`/`lshr`/`ashr`/`and`/`or`/`xor`) plus per-opcode flag types (`AddFlags`/`UDivFlags`/...), every float binop (`fadd`/`fsub`/`fmul`/`fdiv`/`frem`), every cast (`trunc`/`zext`/`sext`/`bitcast`/`ptrtoint`/`inttoptr`/`fptrunc`/`fpext`/`fptosi`/`fptoui`/`sitofp`/`uitofp`/`addrspacecast`), `icmp`/`fcmp`, control flow (`br`/`cond_br`/`unreachable`), `phi` (chainable `add_incoming`), memory (`alloca`/`load`/`store` with optional `Align`), `getelementptr` (`build_gep`/`build_inbounds_gep`/`build_struct_gep`), `call` (flat + chainable `CallBuilder`), and `select` (sealed `SelectArm` for int/float/pointer arms).
- AsmWriter producing real `.ll` output via `format!("{module}")` for every shipped opcode.
- **Verifier** (`crates/llvmkit-ir/src/verifier.rs`): `Module::verify_borrowed(&self) -> IrResult<()>` for diagnostic-only validation and `Module::verify(self) -> IrResult<VerifiedModule<'ctx>>` for the typestate path that brands a module as well-formed. Implements the constructive subset of `llvm/lib/IR/Verifier.cpp::visit*` for every shipped opcode (binary int/float, icmp/fcmp, all casts, alloca/load/store, GEP, call, select, phi, ret/br/unreachable). Catches type mismatches, terminator placement, phi-predecessor coherence, ambiguous phi, in-block use-before-def, and self-reference. Cross-block dominance is deferred to Session 4 (DominatorTree).
- **Mutation API (T1)**: full instruction-lifecycle typestate. `Instruction<'ctx, S = state::Attached>` is parameterised by `S: state::InstructionState` (sealed; variants `Attached` / `Detached`). `Instruction` is intentionally **`!Copy` and `!Clone`** (Doctrine D2): the linear-typed handle prevents use-after-erase / double-erase at compile time. Per-opcode handles (`AddInst`, `LoadInst`, ...) stay `Copy` and now hold `(ValueId, ModuleRef, TypeId)` directly; their `as_instruction(self)` materialises a fresh `Instruction<Attached>` on demand. Every operand slot in `instr_types.rs` is wrapped in `core::cell::Cell<ValueId>` (and `Cell<Option<ValueId>>` for the optional `alloca` / `ret` operands) so RAUW can rewrite the operand wiring through `&self`. The shipped lifecycle methods are:
    - `Instruction<Attached>::replace_all_uses_with(self, replacement)` --- `Value::replaceAllUsesWith` in `lib/IR/Value.cpp`.
    - `Instruction<Attached>::erase_from_parent(self)` --- `Instruction::eraseFromParent`.
    - `Instruction<Attached>::detach_from_parent(self) -> Instruction<Detached>` --- `Instruction::removeFromParent`.
    - `Instruction<Attached>::move_before(self, other)` / `move_after` --- `Instruction::moveBefore` / `moveAfter`.
    - `Instruction<Detached>::insert_before(self, other)` / `insert_after` / `append_to(block)` --- `Instruction::insertBefore` / `insertAfter` / `insertInto`.
    - `Instruction<Detached>::drop_detached(self)` --- discard a detached instruction without inserting it (deregisters its operands' use-list entries).
    - `BasicBlock::splice_into(self, dest)` --- `BasicBlock::splice`.
    - `BasicBlock::split_at(self, before, name) -> BasicBlock<R>` --- `BasicBlock::splitBasicBlock`.
    - `iter::BlockCursor::at_start(block)` and `cursor.next() -> Option<(Instruction<Attached>, BlockCursor)>` --- the canonical advance-then-mutate iteration helper (Doctrine D9). Yields each instruction by value while consuming the cursor; the next call sees the precomputed snapshot, so erasing the yielded handle does not invalidate the iteration.
  Note: `IsValue` keeps its `Copy` supertrait bound (every other implementer is a thin `Copy` handle); `Instruction<Attached>` is therefore *not* an `IsValue` impl. Callers that need an erased view use [`Instruction::as_value(&self)`] (inherent, `&self`).
- **Test provenance registry (T0)**: `UPSTREAM.md` (repo root) is the authoritative answer to "where does this llvmkit test come from?". Every `#[test]` in the workspace ships with a per-test doc comment citing the upstream `unittests/IR/*Test.cpp::TEST(...)` or `test/{Assembler,Verifier}/*.ll` fixture it ports (Doctrine D11).
- **Construction Lifecycle Typestate (T2)**: [`crate::BasicBlock<'ctx, R, Seal>`] gains a `Seal: BlockSealState` parameter (default [`Unsealed`]). The terminator-emitting builds (`build_br` / `build_cond_br` / `build_unreachable` / `build_ret` / `build_ret_void`) consume the `IRBuilder` by value and return `(BasicBlock<'ctx, R, Sealed>, Instruction<'ctx>)`. [`crate::IRBuilder::position_at_end`] only accepts `BasicBlock<'ctx, R, Unsealed>`, so a builder cannot be re-positioned at a sealed block. [`crate::PhiInst<'ctx, W, P>`] gains a `P: PhiState` parameter (default [`Open`]). `add_incoming` is gated to `Open`; [`PhiInst::finish`] consumes `Open` and produces a [`Closed`] view that exposes only read accessors.
- **CallInst Typed Return (T3)**: [`crate::CallInst<'ctx, R>`] gains a `R: ReturnMarker` parameter (default [`Dyn`]) that propagates the callee's return shape. `IRBuilder::build_call(callee: FunctionValue<'ctx, R>, ...)` returns `CallInst<'ctx, R>`; per-marker accessors (`return_int_value`, `return_float_value`, `return_pointer_value`) are gated to the matching marker so `CallInst<'ctx, ()>` exposes neither -- the runtime narrowing on `return_value() -> Option<Value>` is gone for typed call sites.
- **Aggregate Typing scaffolding (T4)**: [`crate::StructType<'ctx, B>`] gains a `B: StructBodyState` parameter (default [`StructBodyDyn`]) with `Opaque` / `BodySet` / `StructBodyDyn` markers. [`Module::opaque_struct(name) -> StructType<Opaque>`] and [`Module::set_struct_body_typed(opaque) -> StructType<BodySet>`] gate the body-set transition at the type level; the existing runtime-checked `set_struct_body` path stays for parsed / legacy code. Sealed-trait scaffolding for [`crate::VectorElement`] (`vector_element.rs` -- mirrors `VectorType::isValidElementType`) and [`crate::SizedElement`] (`sized_element.rs` -- mirrors `ArrayType::isValidElementType`) is in place; the const-generic parameterisation of [`crate::VectorType`] / [`crate::ArrayType`] (`<E, const N>`) is deferred to a later session because it touches every `m.array_type(elem, n)` / `m.vector_type(elem, n, scalable)` call site.

- **Parser-1: full instruction set (Session 1)**. Every opcode the `.ll` parser will need is shipped end-to-end (handle struct, payload, exhaustive operand walker arm per Doctrine D5, IRBuilder method, AsmWriter byte-for-byte parity, verifier `visit_*` arm). The 21 new instruction families: `fneg` (with FMF), `freeze`, `va_arg`; `extractvalue`/`insertvalue` (compile-time `u32` index lists); `extractelement`/`insertelement`/`shufflevector` (`shufflevector` mask is `Box<[i32]>` with `POISON_MASK_ELEM = -1`); `fence`/`cmpxchg`/`atomicrmw` (with new `AtomicOrdering`/`SyncScope`/`AtomicRMWBinOp` support modules and `AtomicCmpXchgConfig`/`AtomicRMWConfig` flag-bag structs); `switch`/`indirectbr` (Open/Closed typestate via new `term_open_state` mod, mirrors `phi_state`); `invoke<R>`/`callbr` (`InvokeInst<R>` mirrors `CallInst<R>` typed-return); `landingpad` (Open/Closed; `add_catch_clause`/`add_filter_clause`/`set_cleanup`)/`resume`; `cleanuppad`/`catchpad`/`catchret`/`cleanupret`/`catchswitch` (funclet pads with `Option<ValueId>` parent-pad slot for `within none`).
- **Builder-A1: atomic load/store + typed bitcast**. `LoadInstData` / `StoreInstData` carry `ordering: AtomicOrdering` and `sync_scope: SyncScope` slots that mirror the `OrderingField` / `SSID` bitfields on the upstream `LoadInst` / `StoreInst` classes (`Instructions.h`). The IRBuilder ships `build_int_load_atomic` / `build_load_atomic` / `build_store_atomic` keyed on the new `AtomicLoadConfig` / `AtomicStoreConfig` config bags (parallel to the existing `AtomicCmpXchgConfig` / `AtomicRMWConfig` shape), porting the upstream 5-arg `LoadInst::LoadInst(Type*, Value*, Twine&, bool, Align, AtomicOrdering, SyncScope::ID)` and 6-arg `StoreInst` constructors. AsmWriter emits the canonical `load atomic [volatile] <ty>, ptr <p> [syncscope("...")] <ordering>, align N` form (mirrors `printInstruction` in `lib/IR/AsmWriter.cpp`). Verifier ports the atomic-rule arm of `Verifier::visit{Load,Store}Inst` and `checkAtomicMemAccessSize` (rejects Release on load, Acquire on store, non-power-of-two operand size, non-default sync scope on non-atomic ops). New typed bitcast methods (`build_bitcast_int_to_int`, `build_bitcast_int_to_fp`, `build_bitcast_fp_to_int`, `build_bitcast_fp_to_fp`) port `IRBuilder::CreateBitCast`'s `CreateCast(Instruction::BitCast, V, DestTy)` arm with width equality enforced statically through new `STATIC_BITS: u32` associated constants on `StaticIntWidth` / `StaticFloatKind` (compile-time `const { assert!(...) }`).
- **Builder-A2: positioning + integer convenience**. The `IRBuilder<F, S, R>` gains an `insert_before: Option<ValueId>` slot (mirrors upstream `InsertPt`'s before-instruction iterator state) so `position_before(anchor)` and `position_past_allocas(f)` correctly thread `BasicBlock::insert_instruction_before` instead of always appending. `save_insert_point()` / `restore_insert_point(snapshot)` round-trip an [`InsertPoint<'ctx, R>`] (parallel to `IRBuilderBase::InsertPoint` in `IRBuilder.h`). Integer convenience methods `build_int_neg` / `build_int_neg_nsw` / `build_int_not` (mirror `IRBuilder::CreateNeg` / `CreateNSWNeg` / `CreateNot`). Pointer convenience methods `build_pointer_cast` (`CreatePointerBitCastOrAddrSpaceCast`), `build_pointer_cmp`, `build_is_null`, `build_is_not_null`. New `IntType::const_zero` / `IntType::const_all_ones` ports of `Constant::getNullValue` / `Constant::getAllOnesValue`. The `FastMathFlags` slot on the IRBuilder ships now (default `empty()`); the per-op auto-application lands in A3.
- **Builder-A3: builder-context FMF + per-predicate fcmp + non-int phi**. The `IRBuilder<F, S, R>` carries an `fmf: FastMathFlags` slot (default `empty()`); `with_fast_math_flags(fmf) -> Self`, `clear_fast_math_flags() -> Self`, and `fast_math_flags() -> FastMathFlags` mirror `IRBuilderBase::setFastMathFlags` / `clearFastMathFlags` / `getFastMathFlags`. `BinaryOpData` and `FCmpInstData` gain a `fmf` slot (printed by `fmt_binop` / `fmt_fcmp`) so the builder context auto-propagates to `fadd`/`fsub`/`fmul`/`fdiv`/`frem`/`fcmp` (and `fneg` via `build_float_neg`). Fourteen named per-predicate fcmp wrappers (`build_fcmp_oeq` ... `build_fcmp_une`) mirror `IRBuilder::CreateFCmpOEQ` ... `CreateFCmpUNE`. New non-int phi handles: `FpPhiInst<'ctx, K, P>` (port of `PHINode` with `IntoFloatValue<K>` operands) and `PointerPhiInst<'ctx, P>` (pointer phi); IRBuilder methods `build_fp_phi::<K>` / `build_fp_phi_dyn` / `build_pointer_phi` / `build_pointer_phi_in_addrspace`.
- **Builder-A4: vector splat + ptr_add convenience**. `IRBuilder::build_vector_splat(count, scalar, name)` mirrors `IRBuilderBase::CreateVectorSplat(unsigned, Value*, const Twine&)` (`lib/IR/IRBuilder.cpp` lines 1141-1158): inserts `scalar` at lane 0 of a poison `<count x T>` vector, then shufflevectors with a zero-mask. The intermediate `insertelement` is named `<name>.splatinsert` and the result `<name>.splat`, byte-for-byte matching upstream. `build_ptr_add(ptr, offset, name)` and `build_inbounds_ptr_add` mirror `IRBuilder::CreatePtrAdd` / `CreateInBoundsPtrAdd` (lines 2039 / 2044) -- thin wrappers around `CreateGEP(getInt8Ty(), Ptr, Offset, ...)`. New `VectorValue::from_value_unchecked` crate-internal constructor parallels the existing `PointerValue` / `IntValue` / `FloatValue` shapes for builder-result narrowing. `CreateAggregateRet` is deferred -- upstream lacks a dedicated `TEST_F` and the construct is genuinely niche (multi-value-return through poison + insertvalue chain).
- **Globals-B1: GlobalVariable + Comdat + AsmWriter / Verifier hookup**. New [`crate::global_variable::GlobalVariable<'ctx>`] handle (Copy, ValueId-keyed, mirrors `class GlobalVariable` in `IR/GlobalVariable.h`) carries the full upstream slot set: `value_type`, `address_space`, `is_constant`, `externally_initialized`, optional `initializer`, [`Linkage`], [`Visibility`], [`DllStorageClass`], [`ThreadLocalMode`], [`UnnamedAddr`], [`MaybeAlign`], `section`, `partition`, optional comdat reference. Construction goes through [`Module::add_global`] / [`Module::add_global_constant`] / [`Module::add_external_global`] (one-shot ctors mirroring upstream's two `GlobalVariable::GlobalVariable` overloads) or the chainable [`crate::global_variable::GlobalBuilder`]. Per-module storage mirrors the function shape (`globals: RefCell<Vec<ValueId>>` + `global_by_name: RefCell<HashMap<...>>`), and a new `ValueKindData::GlobalVariable(GlobalVariableData)` arm closes the value-kind enum (Doctrine D5 -- exhaustive operand/category matches updated). Comdat support ports `IR/Comdat.h`: [`crate::comdat::SelectionKind`] (`Any` / `ExactMatch` / `Largest` / `NoDeduplicate` / `SameSize`), [`crate::comdat::ComdatRef<'ctx>`] backed by a `boxcar::Vec<ComdatData>` arena (stable `&ComdatData` borrows under `&self`), and [`Module::get_or_insert_comdat`] / [`Module::get_comdat`] / [`Module::iter_comdats`] mirroring `Module::getOrInsertComdat`. AsmWriter ships byte-for-byte parity with `lib/IR/AsmWriter.cpp::printGlobal` (linkage / visibility / DLL / TLS / unnamed-addr / addrspace / external_initialised / global-vs-constant / initializer / section / partition / comdat / align) and `Comdat::print` (`$<name> = comdat <kind>`). c-string detection in `fmt_aggregate_constant` mirrors `ConstantDataArray::isString`: `[N x i8]` constants of all `ConstantInt` elements emit as `c"..."` (with `printEscapedString` semantics). Verifier ships [`Verifier::visit_global_variable`] for the constructive subset (initializer-type-matches-value-type, initializer-must-be-sized, common-linkage zero-init/not-constant/no-comdat, scalable-vector rejection); new [`VerifierRule`] variants `GlobalInitializerTypeMismatch` / `GlobalInitializerUnsized` / `CommonLinkageInvariantViolated` / `GlobalScalableType` and a new [`ValueCategoryLabel::GlobalVariable`] arm complete the diagnostic surface. 43 new ported tests under `crates/llvmkit-ir/tests/globals_basic.rs`, all anchored on `test/Bitcode/compatibility.ll` line citations or `unittests/IR/{Module,Constants}Test.cpp::TEST(...)` blocks (Doctrine D11).





Still ahead: parser, const-generic `VectorType<E, const N: u32, Scalable>` / `ArrayType<E, const N: u64>` (T4 follow-up; the sealed-trait scaffolding ships today), brand-based module identity (Doctrine D7 follow-up), DominatorTree + cross-block dominance verifier, pass infrastructure, transforms, bitcode, debug info, intrinsics.

Workspace shape (see each crate's `Cargo.toml` for details):

- Root `Cargo.toml` carries only `[workspace]` metadata.
- `llvmkit/` — the public umbrella crate; re-exports `llvmkit-support` and `llvmkit-asmparser`. `default-members` points at it so plain `cargo run` / `cargo doc` resolve here.
- `crates/llvmkit-support/` — shared helpers (`Span`, `Spanned<T>`, `SourceMap`).
- `crates/llvmkit-asmparser/` — textual IR lexer (parser later).

The crate name `llvmkit` was chosen because `llvmkit` is taken on crates.io. The
repo directory is still `llvmkit/` to avoid churn; rename whenever convenient.

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

Current shape (the lexer plus the IR data-model + value layer +
minimal IRBuilder are implemented; the rest are listed for parity
with `lib/IR/` and `lib/AsmParser/` and will land in subsequent
sessions):

```
<repo root>/
├── Cargo.toml                       # [workspace] only
├── README.md
├── LICENSE
├── AGENTS.md
├── INKWELL_MIGRATION.md
├── llvmkit/                         # umbrella crate
│   ├── Cargo.toml
│   └── src/lib.rs
└── crates/
    ├── llvmkit-support/
    │   └── src/
    │       ├── lib.rs
    │       ├── span.rs              # Span + Spanned<T>
    │       └── source_map.rs        # byte-offset → (line, col)
    ├── llvmkit-ir/                  # IR data model
    │   ├── Cargo.toml
    │   ├── src/
    │   │   ├── lib.rs
    │   │   ├── type.rs              # Type + TypeData + IrType / TypeKind
    │   │   ├── derived_types.rs     # IntType/FloatType/.. + refinement enums
    │   │   ├── typed_pointer_type.rs # TypedPointerType
    │   │   ├── module.rs            # Module + ModuleId + ModuleRef
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
    │   │   ├── basic_block.rs       # BasicBlock handle
    │   │   ├── value_symbol_table.rs # per-function name lookup
    │   │   ├── constant.rs          # Constant + IsConstant
    │   │   ├── constants.rs         # ConstantInt/Float/… refinements + ctors
    │   │   ├── global_value.rs      # Linkage / Visibility / DllStorageClass / ThreadLocalMode
    │   │   ├── global_variable.rs   # GlobalVariable + GlobalBuilder + GlobalVariableData
    │   │   ├── comdat.rs            # SelectionKind + Comdat + ComdatRef + ComdatId
    │   │   ├── unnamed_addr.rs      # GlobalValue::UnnamedAddr
    │   │   ├── argument.rs          # Argument handle
    │   │   ├── function.rs          # FunctionValue<'ctx, R> + FunctionBuilder<R>
    │   │   ├── marker.rs            # ReturnMarker + Dyn + Ptr (top-level)
    │   │   ├── align.rs             # Align + MaybeAlign (Support/Alignment.h)
    │   │   ├── instruction.rs       # Instruction + InstructionKind/TerminatorKind
    │   │   ├── instr_types.rs       # BinaryOpData / CastOpData / CastOpcode / ReturnOpData payloads
    │   │   ├── instructions.rs      # AddInst/SubInst/MulInst/CastInst/RetInst handles
    │   │   ├── operator.rs          # OverflowingBinaryOperator view
    │   │   ├── ir_builder.rs        # IRBuilder<'ctx, F, S, R> typestate
    │   │   └── ir_builder/
    │   │       ├── folder.rs        # IRBuilderFolder trait
    │   │       ├── constant_folder.rs # default folder
    │   │       └── no_folder.rs     # no-op folder
    │   ├── examples/
    │   │   ├── build_add_function.rs # cargo run --example build_add_function
    │   │   ├── cpu_state_add.rs     # multi-fn / params / unnamed_addr / trunc demo
    │   │   ├── factorial.rs         # phi + br + icmp + mul + sub loop demo
    │   │   └── concurrent_counter.rs # fence + atomicrmw + switch (Open/Closed) demo
    │   └── tests/
    │       ├── asm_writer_basic.rs
    │       ├── cpu_state_add_example.rs
    │       ├── factorial_example.rs
    │       ├── medium_builder_cast.rs
    │       ├── medium_builder_cmp.rs
    │       ├── medium_builder_control_flow.rs
    │       ├── medium_builder_int.rs
    │       ├── medium_builder_phi.rs
    │       ├── globals_basic.rs
    │       ├── parameter_attributes.rs
    │       ├── phase_a_types.rs
    │       ├── unnamed_addr.rs
    │       └── vertical_slice.rs
    └── llvmkit-asmparser/
        ├── README.md
        ├── src/
        │   ├── lib.rs
        │   ├── ll_lexer.rs          # LLLexer.h + LLLexer.cpp
        │   ├── ll_lexer/            # private impl-details for ll_lexer.rs
        │   │   ├── escape.rs        # mirrors UnEscapeLexed
        │   │   └── keywords.rs      # mirrors the keyword switch in LexIdentifier
        │   ├── ll_lexer_tests.rs    # unit tests, included via #[path]
        │   └── ll_token.rs          # LLToken.h
        ├── examples/
        │   ├── demo.ll              # tiny fixture used by the example
        │   └── lex_file.rs          # cargo run --example lex_file -- file.ll
        └── tests/
            ├── fixtures/demo.ll
            └── lexer_integration.rs
```

Future work — each entry pairs to a single LLVM C++ file:

| Future Rust file                                  | LLVM source                          |
|---------------------------------------------------|--------------------------------------|
| `llvmkit-asmparser/src/ll_parser.rs`              | `LLParser.{h,cpp}`                   |
| `llvmkit-asmparser/src/parser.rs`                 | `Parser.{h,cpp}`                     |
| `llvmkit-asmparser/src/asm_parser_context.rs`     | `AsmParserContext.{h,cpp}`           |
| `llvmkit-asmparser/src/file_loc.rs`               | `FileLoc.h`                          |
| `llvmkit-asmparser/src/slot_mapping.rs`           | `SlotMapping.h`                      |
| `llvmkit-asmparser/src/numbered_values.rs`        | `NumberedValues.h`                   |
| `crates/llvmkit-ir/src/global_alias.rs`           | `IR/GlobalAlias.h` + `Globals.cpp`   |
| `crates/llvmkit-ir/src/global_ifunc.rs`           | `IR/GlobalIFunc.h` + `Globals.cpp`   |
| `crates/llvmkit-ir/src/data_layout.rs`            | `IR/DataLayout.{h,cpp}`              |
| `crates/llvmkit-ir/src/intrinsic_inst.rs`         | `IR/IntrinsicInst.{h,cpp}`           |
| `crates/llvmkit-ir/src/inline_asm.rs`             | `IR/InlineAsm.{h,cpp}`               |
| `crates/llvmkit-ir/src/intrinsics.rs`             | `IR/Intrinsics.h`                    |
| `crates/llvmkit-ir/src/metadata.rs`               | `IR/Metadata.{h,cpp}`                |
| `crates/llvmkit-ir/src/assembly_annotation_writer.rs` | `IR/AssemblyAnnotationWriter.h` |
| `crates/llvmkit-ir/src/verifier.rs`               | `IR/Verifier.{h,cpp}`                |
| `crates/llvmkit-ir/src/asm_writer.rs` (extensions) | `IR/AsmWriter.cpp` (full opcode set) |
| `crates/llvmkit-bitcode/` (new crate)             | `lib/Bitcode/`, `lib/Bitstream/`     |

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
4. **D4.** Result types reflect operand types (`build_int_add::<i32, _, _>` returns `IntValue<i32>`).
5. **D5.** Operand registration is structural (one place per primitive; exhaustive `match`).
6. **D6.** Aggregate types parameterise over element shape (`VectorType<E, N>`, etc.).
7. **D7.** Cross-module mixing is statically rejected (`'ctx` brand + `ModuleRef` check).
8. **D8.** Verified guarantees flow through references (analyses bound to `&VerifiedModule<'ctx>`).
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

- `impl AsRef<Path>` for filesystem entry points (`Module::from_ll_file`).
- `impl Read` / `impl BufRead` for streaming readers (`Module::from_ll_reader`).
- `impl Write` for printers (`module.write_ll(&mut out)`).
- `&str` / `&[u8]` for in-memory variants (`Module::from_ll_str`).

This mirrors `serde_json::from_reader` / `from_slice` / `from_str`. **Default to streaming**; load into a `Vec<u8>` only when the parser genuinely requires random access.

### Conversions via `From` / `TryFrom`

- Infallible widening (`i32 → ConstantInt`) → `From`.
- Fallible narrowing (`Type → IntegerType`) → `TryFrom` returning `Result<_, TypeMismatch>`.

Avoid bespoke `as_int_type()` / `is_int_type()` pairs when `TryFrom` covers the same intent.

### Interning and identity

LLVM's `LLVMContext` interns types and constants so pointer equality means semantic equality. The Rust analog is a `Context` owning hash tables. Two reasonable shapes:

- Handle-based: `TypeId(u32)` indices into context-owned slabs. Cheap `Copy`, no lifetime, allocator-friendly.
- Reference-based: `&'ctx Type<'ctx>` with the context as a lifetime parent. Borrow-checker enforces "no dangling type references".

Pick one and apply it consistently across `Type`, `Constant`, and the metadata system. Mixing the two within a single subsystem is a smell.

### No `unsafe` without justification

LLVM C++ uses tagged pointers, hung-off operands, intrusive lists, and `union`-via-bitfields. **Do not** transcribe these tricks. Use safe Rust equivalents (`Vec`, `Box`, `Option`, `enum`) until profiling proves they are too slow. If `unsafe` is genuinely needed, isolate it behind a small module with a documented invariant.

### No FFI, no `bindgen`, no `llvm-sys`

If a problem feels solvable only by linking against `libLLVM`, the answer is "read the C++ and reimplement it." This is the explicit point of the project.

### Multi-source operand traits

For every operand slot in the IR builder, the trait should accept every reasonable statically-known source: the typed handle itself, the matching constant handle, every Rust scalar literal that lifts, `Argument<'ctx>`, the erased `Value<'ctx>`, `Instruction<'ctx>`, and the dyn-marker variant of the same family (`IntValue<IntDyn>` -> `IntValue<W>`). The `try_into()?` boilerplate disappears at the call site. Each impl is a *concrete-type* impl - no overlap with the identity blanket, no `dyn` dispatch. Cross-module rejection lives inside the lift trait's `into_*_value(module)` method, not at every IRBuilder call site - one check, reused everywhere.

### Typed forms paired with `_dyn` fallbacks

Every method that returns a typed handle ships in two shapes:

- the static-marker form (`build_int_load::<W>(...)` returns `IntValue<W>`);
- the erased fallback (`build_int_load_dyn(ty, ...)` takes an explicit `IntType<'ctx, IntDyn>` and returns `IntValue<IntDyn>`).

Same posture for trunc / zext / sext / phi / fp loads. The dyn form keeps the runtime check; the typed form pins the invariant at compile time.

### Builder pattern for variable-shape ops

Calls, GEPs, allocas, and any op with several optional knobs ship a chainable builder alongside the flat method:

- `b.build_call(callee, args, name)?` for homogeneous-arg construction.
- `b.call_builder(callee).arg(a).arg(b).tail().calling_conv(cc).name("r").build()?` for mixed-type / mixed-flag construction. `.arg<V: IsValue<'ctx>>(value)` is generic per call so heterogeneous argument lists work without trait objects.

The builder is a plain struct that accumulates state into a `Vec<ValueId>`; `.build()` performs cross-module checks once and emits the instruction.

### Sealed sum-of-categories traits

Where a single concept (`select`, future `freeze`) accepts any of int / float / pointer arms, define **one** sealed trait with an associated `Output` and ship **one** method:

```rust
pub trait SelectArm<'ctx>: Sized + sealed::Sealed {
    type Output;
    fn from_select_value(v: Value<'ctx>) -> Self::Output;
    fn arm_value(self) -> Value<'ctx>;
}
impl<'ctx, W: IntWidth>  SelectArm<'ctx> for IntValue<'ctx, W>   { type Output = IntValue<'ctx, W>;   ... }
impl<'ctx, K: FloatKind> SelectArm<'ctx> for FloatValue<'ctx, K> { type Output = FloatValue<'ctx, K>; ... }
impl<'ctx>               SelectArm<'ctx> for PointerValue<'ctx>  { type Output = PointerValue<'ctx>;   ... }
```

Each impl is concrete; the method monomorphises per arm category. Beats N per-category overload methods.

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
- **Modules**: one concept per file; let modules grow before splitting them. `Instructions.h` is 5k lines because it pays for itself; do not pre-split into 40 stub files.
- **Errors**: one crate-level `enum Error` (or a small per-subsystem enum that flattens into it). Avoid `Box<dyn std::error::Error>` in public signatures.
- **Comments**: explain *why*, not *what*. When porting a non-obvious C++ trick, link the source file and the symbol — never the line number, which drifts between LLVM versions: `// Mirrors LLParser::parseTopLevelEntities (LLParser.cpp)`.
- **Public API**: re-export from `lib.rs`. Keep internal modules `pub(crate)` until an external use case appears.
- **No `as` casts.** Use `From`/`Into` for infallible widening, `TryFrom`/`TryInto` for fallible narrowing, and method-style conversions (e.g. `u32::from(x)`, `usize::try_from(x)`) elsewhere. The `as` keyword silently truncates, changes signedness, and loses precision — every site is a footgun. If a conversion has no idiomatic counterpart (rare, e.g. deliberate truncation), wrap it in a small named helper with a one-line invariant comment.
- **No pointer-based identity in our code.** Identity flows through typed integer indices (`TypeId`, `ValueId`, `ModuleId`, ...). No `core::ptr::eq`, no `&T as *const T`, no address hashing in user-written code. Library internals like `boxcar` may use raw pointers safely behind their `unsafe` boundaries — we do not. Identity comparisons derive from `PartialEq`/`Hash` on the index types.
- **No runtime panics in production code.** `expect`, `unwrap`, `panic!`, `unimplemented!`, `todo!` are forbidden in non-test paths. Real failures use `IrError` returned via `IrResult<T>`. `unreachable!("…invariant…")` is permitted **only** when the branch is provably dead by construction *and* there is no reasonable way to remove it via the type system; the message names the invariant in plain English. Test code (`#[cfg(test)]`, `tests/`, `examples/`) is exempt.
- **Rust scalar types double as IR markers** where natural. The integer-width markers are the Rust types themselves (`bool` for i1, `i8`/`i16`/`i32`/`i64`/`i128`); the IEEE binary32/binary64 float kinds are `f32`/`f64`. Marker structs (`int_width::IntDyn`, `float_kind::{Half, BFloat, Fp128, X86Fp80, PpcFp128, FloatDyn}`) cover only the cases without a Rust counterpart. `IntDyn` and `FloatDyn` are distinct types so trait coherence stays sane (a single shared `Dyn` would simultaneously implement `IntWidth` and `FloatKind`). The top-level [`marker::Dyn`] / [`marker::Ptr`] / `()` mark fully-erased / pointer / void return shapes respectively; the bare type acts as the [`ReturnMarker`] (no wrapper structs).
- **Function and IRBuilder name parameters take `impl AsRef<str>`**; module / function / block / parameter names take `impl Into<String>`. The empty-name fast path stays allocation-free.
- **Compile-time invariants on cast widths** flow through sealed marker traits. `int_width::WiderThan<W>` is implemented for every `(Wider, Narrower)` pair of static markers, and the IR builder uses it as a bound on `build_trunc<Src: WiderThan<Dst>, Dst>` (and inversely on `build_zext` / `build_sext`). The `_dyn`-flavoured fallbacks keep the runtime check for the genuinely-erased path.

- **No `#[allow(...)]` attributes anywhere.** Not `#[allow(dead_code)]`, not `#[allow(clippy::...)]`, not `#[allow(unused_imports)]`, not `#[allow(non_upper_case_globals)]`, not anything else. The compiler / clippy is a teammate; silencing it is silencing the codebase. If a lint fires, fix the code (rename the symbol, drop the dead code, restructure the type) instead of suppressing the lint. The only exception is per-bullet `#[deny(...)]` and `#![forbid(unsafe_code)]` which *strengthen* lints.
- **Static dispatch only in the public IR / builder surface.** All polymorphism flows through monomorphised generics: `<T: Trait>` turbofish-form, `impl Trait` in argument position (which is just turbofish spelled differently - same monomorphisation, no vtable), or sealed-trait blanket impls. **No `dyn Trait`, no `Box<dyn>`, no `&dyn`** in any IR-builder, value, type, or instruction surface. Where homogeneity forces a slice (e.g. `args: &[Value<'ctx>]`), the slice element is a concrete handle - never a trait object.
- **Per-opcode flag types over shared flag bags.** When LLVM's `Operator.h` distinguishes flag classes per opcode (`OverflowingBinaryOperator`, `PossiblyExactOperator`, ...), each flag class gets its own Rust struct exposing only the flags LLVM permits for it (`AddFlags { nuw, nsw }`, `UDivFlags { exact }`, ...). Don't ship a single `BinopFlags` requiring runtime validation against the opcode - the type system should make invalid combinations *unspellable*.
- **AsmWriter print form matches `lib/IR/AsmWriter.cpp` byte-for-byte.** Read the matching `printInstruction` arm before adding an opcode formatter, and lock at least one fixture against the upstream-canonical text via `assert_eq!(format!("{m}"), expected)`. Flag print order, whitespace, and trailing punctuation all match the C++ reference.

- **No emojis**, no decorative comments, no boilerplate `mod tests` blocks unless they contain real tests.

## Development Commands

Run from the repository root (`C:/Users/Aslan/llvmkit` on the current host).

```bash
cargo build                  # compile
cargo build --release        # optimized build
cargo test                   # run all tests
cargo test <name>            # run tests matching a name
cargo check                  # type-check without codegen (fastest feedback)
cargo clippy --all-targets   # lint
cargo fmt                    # format
cargo fmt -- --check         # CI-style format check
cargo doc --no-deps --open   # render rustdoc
```

There is no `build.rs`, no Make/CMake, no submodules. `orig_cpp/` is **not** built — never run `cmake` or `ninja` against it.

## Testing & QA

The crate ships a substantial test suite (250+ tests across `crates/llvmkit-ir/tests/` plus per-module `#[cfg(test)]` blocks). The categories:

**Tests are ported, not invented.** Every new opcode, predicate, or instruction lands with tests sourced from one of the upstream LLVM trees:

1. **`orig_cpp/.../llvm/test/Assembler/*.ll`** - the canonical round-trip / format fixtures. Each `.ll` is `RUN: llvm-as | llvm-dis | FileCheck %s` upstream; the `; CHECK:` directives spell the canonical AsmWriter output. We can't run `llvm-as` (no parser yet), but we can build an equivalent module programmatically and assert `format!("{m}")` against the fixture body byte-for-byte. Copy the constructive subset to `tests/fixtures/llvm/<topic>.ll` with a leading comment block citing the upstream path.
2. **`orig_cpp/.../llvm/unittests/IR/*Test.cpp`** - GoogleTest-flavoured unit tests for `IRBuilder::Create*`, `ConstantInt::get`, etc. Each `TEST_F(*, Foo)` translates to a Rust `#[test]` mirroring the structural assertions (operand wiring, flag bits, result types).
3. **`orig_cpp/.../llvm/test/Verifier/*.ll`** - negative tests: malformed IR that LLVM's verifier rejects. Useful for `IrError` coverage on builder methods that surface domain rules.

**Do not invent `.ll` strings or test scenarios** unless upstream genuinely lacks coverage for the construct. When that happens, document the gap inline and cite the closest upstream test family (e.g. `IRBuilderTest::CreateStepVectorI3` for arbitrary-width tests).

**Test provenance registry.** Every `#[test]` in the workspace ships with a doc comment citing the upstream LLVM file, fixture, or `TEST(...)` it ports. The complete registry lives at `UPSTREAM.md` (repo root) and is the authoritative answer to "where does this test come from?". After adding a new test, append the row. Doctrine D11 (see `local://RLLVM_TYPE_SAFETY_SWEEP.md`) makes this rule mechanical: a test without a citation is a defect, not a stylistic gap.

Categories below are the *shape* of testing; their content always sources from the upstream tree above.

- **Unit tests** (`#[cfg(test)] mod tests` in each module) for type interning, constant folding, instruction construction.
- **Round-trip tests** (`tests/roundtrip.rs`) that read a `.ll` file, print it back, and assert the canonical form is stable. Use small `.ll` snippets as fixtures under `tests/fixtures/`. The LLVM repo's own `llvm/test/Assembler/*.ll` files are good seed material — copy specific files in as needed; do not pull the whole `test/` tree.
- **Conformance tests** for the parser by comparing against the C++ behavior described in `LLParser.cpp`. When the Rust parser disagrees with the reference, the reference wins unless the disagreement is a deliberate, documented Rust-side improvement.
- **Property tests** (`proptest` / `quickcheck`) for `IRBuilder` once it can produce a non-trivial subset of instructions: build a random valid module, print it, parse it, assert structural equality.

Do not commit code that breaks `cargo test`, `cargo clippy --all-targets -- -D warnings`, or `cargo fmt -- --check`.

## Important Files

- Root `Cargo.toml` — workspace definition (`[workspace]` + shared profile + dep versions).
- `crates/<crate>/Cargo.toml` — per-crate manifest; pulls shared values via `workspace = true`.
- `crates/<crate>/src/lib.rs` — crate root. Each crate begins with `#![forbid(unsafe_code)]`.
- `.gitignore` — ignores `/target`, `/orig_cpp/`, `/build/`. `Cargo.lock` is **committed** (the workspace ships binaries / examples).
- `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/` — read-only LLVM 22.1.4 reference. Treat as documentation, not as code.
- `build/llvm/compile_commands.json` — generated for clangd cross-navigation; do not commit.

## What an AI Assistant Should Do First

1. **Read the reference before editing.** When asked to port `Foo`, open the C++ header *and* the matching `.cpp` listed in the table above. The `.cpp` files contain invariants the `.h` doesn't show.
2. **Search before inventing.** If you need a utility (e.g. small-string optimization, bit reader), check whether `std` or a well-known crate already provides it before writing it.
3. **Prefer one well-modeled subsystem over many half-modeled ones.** A complete, idiomatic `Type` + `Context` pair is more valuable than stubs for `Type`, `Value`, `Module`, `Function`, and `IRBuilder` simultaneously.
4. **Surface uncertainty.** If a C++ behavior is ambiguous (e.g. silent overflow vs. assertion), state the choice and the rationale in a comment. Do not silently pick.
5. **Do not import LLVM via FFI** to "validate" Rust output. The Rust implementation must stand on its own; cross-checking against `llc` / `opt` is fine as an external manual step but must not be a build dependency.
6. **LSP-first for cross-file refactors.** Use `lsp rename` for symbol renames (cross-crate, cross-module), `lsp references` to find every consumer before changing a public signature, `lsp diagnostics file:"*"` after substantive edits to catch stragglers, and `lsp code_actions` for missing-trait-impl / missing-import suggestions. Regex / `sed` is the right tool only for doc-comment markdown links and similar non-symbol text - never for code-symbol changes.
