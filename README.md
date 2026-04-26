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
    on the typed handle and dispatch on the Rust input type ""
    `i32_ty.const_int(42_i32)` sign-extends; `i32_ty.const_int(42_u32)`
    zero-extends "" via the `IntoConstantInt<'ctx, W>` trait.
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
  - **Instructions** (Phase E minimum + Phase C `trunc`): `add`, `sub`,
    `mul`, `trunc`, `ret`, with per-opcode `AddInst` / `SubInst` /
    `MulInst` / `CastInst` / `RetInst` handles, `Instruction::kind` /
    `terminator_kind` analysis enums, and the `OverflowingBinaryOperator`
    view.
  - **`IRBuilder`** (Phase G minimum + A3 + `trunc`):
    `IRBuilder<'ctx, F, S, R>` with `Unpositioned` / `Positioned`
    typestate (compile-time insertion-point invariant) and a `R:
    ReturnMarker` parameter (compile-time return-type invariant).
    `IRBuilderFolder` trait + default `ConstantFolder` + `NoFolder`,
    `build_int_add` / `_sub` / `_mul` parametrised by `W: IntWidth` so
    mismatched-width operands are a compile error,
    `build_trunc::<WSrc, WDst>` for narrowing casts, and per-marker
    `build_ret` (`RInt<W>` requires `IntValue<'ctx, W>`, `RFloat<K>`
    requires `FloatValue<'ctx, K>`, `RPtr` requires `PointerValue<'ctx>`,
    `RVoid` exposes only `build_ret_void()`).
  - **AsmWriter** (Phase B + D-lite): `Display` impls on `Module`,
    `FunctionValue`, `BasicBlock`, `Instruction`, `Value`. `format!("{m}")`
    produces real `.ll` covering integer arithmetic + `trunc` + `ret` plus
    every constant kind, `local_unnamed_addr` / `unnamed_addr`, and
    parameter / return attribute slots (`define noundef i32 @main() {`).
- **`.ll` parser** "" not yet.
- **Full instruction set** (`Br`/`CondBr`/`Switch`/`Phi`/`Call`/`GEP`/
  `Load`/`Store`/casts/cmps), full constant set (`ConstantExpr`,
  `BlockAddress`, `TokenNone`, ...), full attribute machinery,
  globals (Phase D rest), metadata / intrinsics (Phase F),
  AsmWriter, full Verifier \u2014 not yet; each scheduled as its own
  focused session.
- **Bitcode** \u2014 not yet.
- **Optimizations / passes** \u2014 not yet, but planned. The IR data model is
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
    AttrKind, B32, B64, IRBuilder, IntValue, IrError, Linkage, Module, RInt, UnnamedAddr,
};

fn build() -> Result<(), IrError> {
    let m = Module::new("cpu_state_add");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();

    // Multi-arg function with named parameters and `local_unnamed_addr`.
    let add_sig = m.fn_type(
        i32_ty,
        [i64_ty.as_type(), i64_ty.as_type(), i64_ty.as_type(), i64_ty.as_type()],
        false,
    );
    let add_fn = m
        .function_builder::<RInt<B32>>("add", add_sig)
        .linkage(Linkage::External)
        .unnamed_addr(UnnamedAddr::Local)
        .param_name(0, "rax").param_name(1, "rbx")
        .param_name(2, "rcx").param_name(3, "rdx")
        .build()?;
    let entry = add_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let rax: IntValue<B64> = add_fn.param(0)?.try_into()?;
    let rbx: IntValue<B64> = add_fn.param(1)?.try_into()?;
    let rcx: IntValue<B64> = add_fn.param(2)?.try_into()?;
    let _rdx: IntValue<B64> = add_fn.param(3)?.try_into()?;
    let t0 = b.build_trunc(rax, i32_ty, "")?;
    let t1 = b.build_trunc(rbx, i32_ty, "")?;
    let t2 = b.build_trunc(rcx, i32_ty, "")?;
    let s1 = b.build_int_add(t0, t1, "add1")?;
    let s2 = b.build_int_add(s1, t2, "add2")?;
    b.build_ret(s2)?;       // typed RInt<B32>: width-checked at compile time

    // Function with `noundef` return-attribute.
    let main_sig = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let main_fn = m
        .function_builder::<RInt<B32>>("main", main_sig)
        .linkage(Linkage::External)
        .unnamed_addr(UnnamedAddr::Local)
        .return_attribute(AttrKind::NoUndef)
        .build()?;
    let entry = main_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let one = i32_ty.const_int(1_i32);
    let one_v = IntValue::<B32>::try_from(one.as_value())?;
    b.build_ret(one_v)?;

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

## References

- [LLVM Project](https://llvm.org/) — source license and design.
- [LLVM Language Reference](https://llvm.org/docs/LangRef.html).
- LLVM source release notes — <https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.4>.

## License

This project is a derivative work of the [LLVM Project](https://llvm.org/) and
is licensed under the [Apache License v2.0 with LLVM Exceptions](LICENSE)
(`Apache-2.0 WITH LLVM-exception`) — the same license LLVM ships under.
