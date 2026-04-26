# llvmkit

A from-scratch Rust reimplementation of [LLVM](https://llvm.org/) IR APIs — read, write, and programmatically build LLVM textual IR (`.ll`) and bitcode without linking against `libLLVM`.

## Status

Tracking **LLVM 22.1.4** (`llvmorg-22.1.4`, released 2026-04-21).

- **`.ll` lexer** — done. 1:1 port of `llvm/lib/AsmParser/LLLexer.cpp`.
  Borrows from the source slice, allocates only when escape-decoding actually changes bytes.
- **`.ll` parser** — not yet.
- **IR data model (`Type`, `Value`, `Module`, …)** — not yet.
- **`IRBuilder`** — not yet (in scope; needed for programmatic IR construction).
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

```bash
# Lex any .ll file from disk:
cargo run -p llvmkit-asmparser --example lex_file -- crates/llvmkit-asmparser/examples/demo.ll

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
