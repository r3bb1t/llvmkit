# llvmkit-asmparser

Lexer (and eventually parser) for LLVM textual IR (`.ll`).

The Rust files mirror `llvm/lib/AsmParser/` and `llvm/include/llvm/AsmParser/`
file-for-file: `LLLexer.{h,cpp}` → `ll_lexer.rs`, `LLToken.h` → `ll_token.rs`.
Tracking **LLVM 22.1.4**.

## What's implemented

- **`.ll` lexer** — borrows from a pre-loaded `&[u8]`; tokens carry `Cow<[u8]>`
  payloads that allocate only when an escape sequence actually changes bytes.
- **`.ll` parser (module-level slice)** — recursive-descent, one-token
  lookahead. Builds into an existing `llvmkit_ir::Module<'ctx>`; the `'ctx`
  brand prevents cross-module mixing at compile time. Covers `target
  datalayout` / `target triple`, `source_filename`, `module asm`, named /
  numbered struct type definitions, simple `@name = global TY CONST` /
  `constant` globals, `declare` declarations, and `define` function bodies
  for every shipped opcode. Function-body details, attribute groups,
  metadata, comdats, aliases, and use-list directives land in later
  sessions.

Bitcode is out of scope for this crate today; see
the workspace [`README`](../../README.md) for the roadmap.

## Usage

```rust
use llvmkit_asmparser::ll_lexer::Lexer;

let mut lex = Lexer::from("@x = i32 42");
while let Some(tok) = lex.next() {
    println!("{:?}", tok.expect("lex error"));
}
```

For a `Read`-based entry point, use the `read_to_owned` helper:

```rust
use llvmkit_asmparser::{ll_lexer::Lexer, read_to_owned};
use std::fs::File;

let bytes = read_to_owned(File::open("foo.ll")?)?;
let lex   = Lexer::new(&bytes);
# Ok::<_, std::io::Error>(())
```

End-to-end examples:

- `cargo run --example lex_file   -- examples/demo.ll`
- `cargo run --example parse_file -- examples/parser_demo.ll`

The parser example prints the round-tripped module via the AsmWriter on
success and a `(line:col)` diagnostic with a caret underline on failure.

## License

Apache-2.0 WITH LLVM-exception (same as upstream LLVM). See the workspace
[LICENSE](../../LICENSE).
