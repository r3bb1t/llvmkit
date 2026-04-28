# llvmkit

A from-scratch Rust reimplementation of [LLVM](https://llvm.org/) IR APIs — read, write, and programmatically build LLVM textual IR (`.ll`) and bitcode without linking against `libLLVM`.

## Status

Tracking **LLVM 22.1.4** (`llvmorg-22.1.4`, released 2026-04-21).

- **`.ll` lexer** — done. 1:1 port of `llvm/lib/AsmParser/LLLexer.cpp`.
  Borrows from the source slice, allocates only when escape-decoding actually changes bytes.
- **IR data model foundation** — done. `llvmkit-ir` ships:
  - **Type system** (Phase A): `Module<'ctx>`, every LLVM `TypeID`, per-kind
    typed handles (`IntType<'ctx, W>` width-typed, `FloatType<'ctx, K>`
    kind-typed; pointer / array / struct / vector / function), refinement
    enums (`SizedType`, `BasicTypeEnum`, `AggregateType`,
    `BasicMetadataTypeEnum`, `AnyTypeEnum`), interning via per-kind maps
    mirroring `LLVMContextImpl`, sealed `IrType` trait, derive-only handle
    identity via `(TypeId, ModuleRef<'ctx>)` and process-global `ModuleId`.
    Width and kind markers are **Rust scalar types** — `bool` / `i8` /
    `i16` / `i32` / `i64` / `i128` for integers and `f32` / `f64` for
    IEEE binary32/64 — plus struct markers (`Half`, `BFloat`, `Fp128`,
    `X86Fp80`, `PpcFp128`) for kinds without a Rust counterpart, and a
    single `Dyn` for both the width-erased and kind-erased shapes.
  - **Attributes** (Phase B): `AttrKind`, `Attribute`, `AttributeSet`,
    `AttributeList` (subset: enum / int / type / string), `AttributeMask`.
  - **Value layer foundation** (Phase C): `Value<'ctx>`, `Constant<'ctx>`,
    `BasicBlock<'ctx>`, per-kind value refinements (`IntValue<'ctx, W>`,
    `FloatValue<'ctx, K>`, `PointerValue`, `ArrayValue`, `StructValue`,
    `VectorValue`, `FunctionTypedValue`), sealed `IsValue` / `Typed` /
    `HasName` / `HasDebugLoc` traits, `Use` / `User` skeletons, opaque
    `DebugLoc`, per-function `ValueSymbolTable`.
  - **Constants** (Phase B continued): `ConstantIntValue<'ctx, W>`,
    `ConstantFloatValue<'ctx, K>`, `ConstantPointerNull`,
    `ConstantAggregate`, `UndefValue`, `PoisonValue`. Constructors live
    on the typed handle and dispatch on the Rust input type —
    `i32_ty.const_int(42_i32)` sign-extends; `i32_ty.const_int(42_u32)`
    zero-extends — via the `IntoConstantInt<'ctx, W>` trait.
  - **Function & basic block** (Phase D minimum + A3): `FunctionValue<'ctx, R>`
    where `R: ReturnMarker` is `RVoid` / `RInt<W>` / `RFloat<K>` / `RPtr` /
    `RDyn`. `BasicBlock<'ctx, R>` and `IRBuilder<'ctx, F, S, R>` propagate
    the marker so `build_ret` is statically typed. `Argument`, `Linkage`,
    `UnnamedAddr` (`local_unnamed_addr` / `unnamed_addr`),
    `FunctionBuilder<R>` (chainable: `.linkage()` / `.calling_conv()` /
    `.unnamed_addr()` / `.attribute()` / `.return_attribute()` /
    `.param_attribute()` / `.param_name()` / `.build()?`),
    `Module::add_function::<R>` / `Module::function_builder::<R>` /
    `function_by_name` / `function_by_name_typed::<R>` / `iter_functions`.
  - **Instructions** (Phase E + Phase C `trunc`/`zext`/`sext`/`icmp`/
    `br`/`cond_br`/`unreachable`/`phi`): `add`, `sub`, `mul`, integer
    casts, integer compares, branches, unreachable, and phi nodes,
    with per-opcode `AddInst` / `SubInst` / `MulInst` / `CastInst` /
    `ICmpInst` / `BranchInst` / `UnreachableInst` / `PhiInst` /
    `RetInst` handles, `Instruction::kind` / `terminator_kind` analysis
    enums, the `OverflowingBinaryOperator` view, and
    `PhiInst::add_incoming` mirroring `PHINode::addIncoming`.
  - **`IRBuilder`** (Phase G + A3 + Phase R-cast):
    `IRBuilder<'ctx, F, S, R>` with `Unpositioned` / `Positioned`
    typestate (compile-time insertion-point invariant) and a `R:
    ReturnMarker` parameter (compile-time return-type invariant).
    `IRBuilderFolder` trait + default `ConstantFolder` + `NoFolder`,
    `build_int_add` / `_sub` / `_mul` / `_cmp` parametrised by
    `W: IntWidth` so mismatched-width operands are a compile error,
    `build_trunc<Src, Dst>` requiring `Src: WiderThan<Dst>` (the
    inverse bound on `build_zext` / `build_sext`) so cast widths are a
    compile-time invariant for static markers,
    `build_trunc_dyn` / `_zext_dyn` / `_sext_dyn` keep the runtime
    check for `IntValue<Dyn>` paths,
    `build_br` / `build_cond_br` / `build_unreachable` /
    `build_int_phi`, `IntoIntValue<'ctx, W>` lifts already-typed
    `IntValue`, `ConstantIntValue`, and Rust scalar literals
    (`5_i32`, `true`, ...) at the same call site, and per-marker
    `build_ret` (`RInt<W>` accepts any `IntoIntValue<W>`,
    `RFloat<K>` requires `FloatValue<'ctx, K>`, `RPtr` requires
    `PointerValue<'ctx>`, `RVoid` exposes only `build_ret_void()`).
  - **AsmWriter** (Phase B + D-lite + C): `Display` impls on `Module`,
    `FunctionValue`, `BasicBlock`, `Instruction`, `Value`. `format!("{m}")`
    produces real `.ll` covering integer arithmetic + integer casts +
    integer compares + branches + phi + ret plus every constant kind,
    `local_unnamed_addr` / `unnamed_addr`, and parameter / return
    attribute slots (`define noundef i32 @main() {`).
- **`.ll` parser** — not yet.
- **Full instruction set (Parser-1)**: every opcode the `.ll` parser will
  need ships end-to-end. Beyond the medium-builder set, this adds
  `fneg` (with FMF), `freeze`, `va_arg`; `extractvalue` / `insertvalue`
  with compile-time index lists; `extractelement` / `insertelement` /
  `shufflevector`; `fence` / `cmpxchg` / `atomicrmw` plus
  `AtomicOrdering` / `SyncScope` / `AtomicRMWBinOp` support; `switch` /
  `indirectbr` with Open/Closed case-list typestate; `invoke<R>` /
  `callbr` (typed-return mirroring `CallInst<R>`); `landingpad` /
  `resume`; and the funclet family `cleanuppad` / `catchpad` /
  `catchret` / `cleanupret` / `catchswitch`.
- **Bitcode** — not yet.
- **Optimizations / passes** — not yet, but planned. The IR data model is
  designed to support pass-style transforms; concrete passes will land once
  the parser, builder, and verifier are stable.

What's *out of scope*: code generation and target backends. `llvmkit` is an
IR toolkit, not a compiler — nothing here lowers IR to machine code or links
object files. Use upstream LLVM (or `llvm-sys` / `inkwell`) for that.

## Quick Start

```rust
use llvmkit_asmparser::ll_lexer::Lexer;
use llvmkit_asmparser::ll_token::Token;

let mut lex = Lexer::from("@x = i32 42");
while let Some(tok) = lex.next() {
    let spanned = tok.expect("lex error");
    println!("{:?}", spanned);
}
```

Build IR programmatically with the type-state `IRBuilder`:

```rust
use llvmkit_ir::{
    IRBuilder, IntPredicate, IntValue, IrError, Linkage, Module, RInt,
};

fn build() -> Result<(), IrError> {
    let m = Module::new("factorial");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m
        .function_builder::<RInt<i32>>("factorial", fn_ty)
        .linkage(Linkage::External)
        .param_name(0, "n")
        .build()?;
    let entry = f.append_basic_block("entry");
    let base = f.append_basic_block("base");
    let loop_bb = f.append_basic_block("loop");
    let exit = f.append_basic_block("exit");
    let n: IntValue<i32> = f.param(0)?.try_into()?;

    let b = IRBuilder::new_for::<RInt<i32>>(&m).position_at_end(entry);
    let is_zero = b.build_int_cmp::<i32, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
    b.build_cond_br(is_zero, base, loop_bb)?;

    let b = IRBuilder::new_for::<RInt<i32>>(&m).position_at_end(base);
    b.build_ret(1_i32)?; // Rust scalar lifts through IntoIntValue
    // ... loop body builds with build_int_phi / build_int_mul /
    // build_int_sub / build_int_cmp / build_cond_br; see
    // `examples/factorial.rs` for the full version.
    let _ = (loop_bb, exit);
    print!("{m}");
    Ok(())
}
```


```bash
# Lex any .ll file from disk:
cargo run -p llvmkit-asmparser --example lex_file -- crates/llvmkit-asmparser/examples/demo.ll

# Build a tiny IR module programmatically and print real .ll:
cargo run -p llvmkit-ir --example build_add_function
cargo run -p llvmkit-ir --example cpu_state_add
cargo run -p llvmkit-ir --example factorial

# Run the test suite:
cargo test --workspace --all-targets
```

## Project Structure

```
<repo root>/
+-- Cargo.toml              # [workspace] only
+-- llvmkit/                # umbrella crate (re-exports the rest)
+-- crates/
    +-- llvmkit-support/    # Span, Spanned<T>, SourceMap
    +-- llvmkit-asmparser/  # Lexer (parser later)
        +-- src/ll_lexer.rs # mirrors llvm/lib/AsmParser/LLLexer.cpp + .h
        +-- src/ll_token.rs # mirrors llvm/include/llvm/AsmParser/LLToken.h
```

Every Rust file in this tree pairs to a single LLVM C++ file (`Foo.{h,cpp}` →
`foo.rs`). See [AGENTS.md](AGENTS.md) for the full source-tree map and the
future-files roadmap.

## Design Principles

- **Track LLVM's behavior.** The Rust port aims to match the upstream C++
  observable behavior on a per-file basis. Discrepancies are bugs unless
  flagged as deliberate Rust-side improvements.
- **Make invalid IR unrepresentable.** Where C++ uses runtime checks
  (`v->getType()->isFloatTy()`), Rust prefers a sum type whose variants already
  encode the answer.
- **No FFI, no `bindgen`, no `llvm-sys`.** All Rust code; nothing links against `libLLVM` at build or runtime.
- **`#![forbid(unsafe_code)]`** in every workspace crate.
- **Stream borrowing, not buffering.** The lexer takes `&'src [u8]`; tokens
  carry `Cow<[u8]>` payloads that borrow whenever escape-decoding leaves bytes
  unchanged.

## Type-Safety Doctrine

Type safety is `llvmkit`'s killer feature. The Rust type system is pushed to its
absolute maximum to make invalid IR unrepresentable wherever LLVM C++ relies on
runtime checks or convention. Eleven rules govern every API in the crate; cite
them by id (`D1`-`D11`) in code reviews and commit messages.

- **D1. State machines are typestates.** If a value has more than one operational
  state, those states are distinct types. `Instruction<'ctx, S>` for `S in
  { Attached, Detached }`, future `BasicBlock<'ctx, R, Seal>` for `Seal in
  { Unsealed, Sealed }`, future `PhiInst<'ctx, W, P>` for `P in { Open, Closed }`,
  `Module` vs. `VerifiedModule`. There is no third state, no `is_attached()`
  runtime predicate.
- **D2. Linear-typed handles for irreversible operations.** Methods that consume a
  resource take `self` by value AND the handle is `!Copy`. `Instruction<Attached>`
  is `!Copy` and `!Clone`; `erase_from_parent(self)` consumes the binding so
  use-after-erase and double-erase are *compile* errors.
- **D3. Erased forms are explicitly opt-in.** Every typed handle has a `Dyn`
  companion (`IntDyn`, `FloatDyn`, etc.). The user must spell the runtime-checked
  form (`as_dyn()` / `IntValue<IntDyn>`); the default in builder return types is
  the strongest type the construction site allows.
- **D4. Result types reflect operand types.** `b.build_int_add::<i32, _, _>(...)`
  returns `IntValue<i32>`. `b.build_int_cmp(...)` returns `IntValue<bool>`.
  `Option<Value>` accessors disappear wherever the presence is statically known.
- **D5. Operand registration is structural.** Use-list maintenance happens in
  exactly one place per construction primitive (`IRBuilder::append_instruction`,
  `PhiInst::add_incoming`) and exactly one place per mutation primitive
  (`replace_all_uses_with`, `erase_from_parent`). Adding a new opcode adds an arm
  to the operand walker; the compiler enforces exhaustiveness via `match`.
- **D6. Aggregate types parameterise over element shape.**
  `VectorType<'ctx, E, const N: u32>` (planned), `ArrayType<'ctx, E, const N: u64>`
  (planned), `StructType<'ctx>` with `Opaque` / `BodySet` typestate (planned).
- **D7. Cross-module mixing is statically rejected.** The `'ctx` lifetime brand
  catches the common case; the `ModuleRef` runtime check covers `'static`
  constants. Both layers stay; an invariant-lifetime brand for parser-loaded
  modules is planned.
- **D8. Verified guarantees flow through references.** Analyses returned by the
  future `AnalysisManager` are bound to the lifetime of the borrowed
  `&VerifiedModule<'ctx>` -- they cannot outlive the verified state. Mutating the
  module strips the brand, dropping every analysis result.
- **D9. Iteration safety is structural.** Mutating-while-iterating uses
  [`iter::BlockCursor`](crates/llvmkit-ir/src/iter.rs), which encodes
  "advance before mutate" via consume-on-step semantics. Direct mutation of an
  instruction iterator is a compile error.
- **D10. No undefined behaviour, by design.** Adopted from
  [Cranelift](https://cranelift.dev/): every legal API call produces well-defined
  IR. Where LLVM allows a runtime trap (unsigned overflow without `nuw`, division
  by zero, dereferencing null), `llvmkit`'s types either prevent the construction
  or surface the deferred-trap semantics through an `IrError` / `poison` value
  rather than silent bad-codegen. The `nuw` / `nsw` / `exact` flags on
  `AddFlags` / `SDivFlags` / etc. are the precedent.
- **D11. Tests are ported, not invented.** Every `#[test]` in the workspace cites
  its upstream `unittests/IR/*Test.cpp::TEST(...)` or `test/{Assembler,Verifier}/*.ll`
  fixture in a doc comment. The full registry lives at [UPSTREAM.md](UPSTREAM.md).
  When upstream genuinely lacks coverage (typestate compile-fail, AsmWriter parity
  for an opcode without a dedicated upstream test), the test is marked
  `llvmkit-specific:` with the closest functional reference cited.

## References

- [LLVM Project](https://llvm.org/) — source license and design.
- [LLVM Language Reference](https://llvm.org/docs/LangRef.html).
- LLVM source release notes — <https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.4>.

## License

This project is a derivative work of the [LLVM Project](https://llvm.org/) and
is licensed under the [Apache License v2.0 with LLVM Exceptions](LICENSE)
(`Apache-2.0 WITH LLVM-exception`) — the same license LLVM ships under.
