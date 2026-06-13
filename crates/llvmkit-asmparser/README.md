# llvmkit-asmparser

[![crates.io](https://img.shields.io/crates/v/llvmkit-asmparser.svg)](https://crates.io/crates/llvmkit-asmparser)
[![docs.rs](https://docs.rs/llvmkit-asmparser/badge.svg)](https://docs.rs/llvmkit-asmparser)
[![License](https://img.shields.io/crates/l/llvmkit-asmparser.svg)](https://github.com/r3bb1t/llvmkit#license)

Lexer (and eventually parser) for LLVM textual IR (`.ll`).

The Rust files mirror `llvm/lib/AsmParser/` and `llvm/include/llvm/AsmParser/`
file-for-file: `LLLexer.{h,cpp}` → `ll_lexer.rs`, `LLToken.h` → `ll_token.rs`.
Tracking **LLVM 22.1.4**.

## What's implemented

- **`.ll` lexer** — borrows from a pre-loaded `&[u8]`; tokens carry `Cow<[u8]>`
  payloads that allocate only when an escape sequence actually changes bytes.
- **`.ll` parser** — recursive-descent, one-token lookahead. Builds into an
  existing `llvmkit_ir::Module<'ctx>`; the `'ctx` brand prevents cross-module
  mixing at compile time. Covers `target datalayout` / `target triple`,
  `source_filename`, `module asm`, type definitions, globals, declarations,
  definitions, function attributes/comdats/header operands, metadata and
  debug records, use-list directives, summaries, and every shipped opcode.
  Remaining parser work is tracked by the workspace roadmap rather than by
  empty parser stubs.

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


## Parser corpus

`tests/parser_corpus.rs` reads `tests/fixtures/parser_corpus_manifest.txt`.
Each row names an llvmkit fixture, its upstream provenance, optional
`expect=<file>` canonical AsmWriter output, and `status=pass|xfail-parse|xfail-verify`.
The `xfail-*` rows are the explicit allowlist for upstream-negative or
not-yet-supported shapes; passing rows must parse, verify, and match the
checked-in expected output when one is listed. Fixture comments cite upstream
inputs as provenance, not as a claim that upstream's FileCheck text is identical
to llvmkit's current expected output.

## License

Apache-2.0 WITH LLVM-exception (same as upstream LLVM). See the workspace
[LICENSE](../../LICENSE).
