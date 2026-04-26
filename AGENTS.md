# Repository Guidelines

## Project Overview

`rllvm` is a from-scratch Rust reimplementation of LLVM IR APIs. It is **not** an FFI binding to `libLLVM` — the build and runtime never depend on `libLLVM` or `llvm-sys`.

Goals, in priority order:

1. **Read and write LLVM IR** (textual `.ll` first, bitcode later) with idiomatic Rust I/O traits.
2. **Provide an `IRBuilder` analog** for programmatic IR construction.
3. **Mirror LLVM's logic exactly**, using the C++ source under `orig_cpp/` as the canonical reference for behavior.
4. **Make invalid IR unrepresentable** at the type level wherever LLVM uses runtime checks. Where C++ forces `if (v->getType()->isFloatTy())`, Rust should expose a sum type whose variants already encode the answer.

What `rllvm` is *not*:

- Not a binding crate (`llvm-sys`, `inkwell`, `llvm-ir`-style wrappers are all out of scope).
- Not a code generator and not a target backend. `llvmkit` doesn't lower IR to machine code or link objects — use upstream LLVM (`llvm-sys`, `inkwell`) for that. Optimization / transform / analysis passes are *planned* future work, not excluded; they will land once the IR data model, builder, parser, and verifier are stable.

## Project Status

The repo is a Cargo workspace at `C:/Users/Aslan/rllvm/`. The first subsystem
(the `.ll` lexer) is implemented; the parser, IR data model, and bitcode
layers will land in subsequent sessions.

Workspace shape (see each crate's `Cargo.toml` for details):

- Root `Cargo.toml` carries only `[workspace]` metadata.
- `llvmkit/` — the public umbrella crate; re-exports `llvmkit-support` and `llvmkit-asmparser`. `default-members` points at it so plain `cargo run` / `cargo doc` resolve here.
- `crates/llvmkit-support/` — shared helpers (`Span`, `Spanned<T>`, `SourceMap`).
- `crates/llvmkit-asmparser/` — textual IR lexer (parser later).

The crate name `llvmkit` was chosen because `rllvm` is taken on crates.io. The
repo directory is still `rllvm/` to avoid churn; rename whenever convenient.

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

Current shape (only the lexer is implemented; the rest are listed for parity
with `lib/AsmParser/` and will land in subsequent sessions):

```
<repo root>/
├── Cargo.toml                       # [workspace] only
├── README.md
├── LICENSE
├── AGENTS.md
├── llvmkit/                         # umbrella crate
│   ├── Cargo.toml
│   └── src/lib.rs
└── crates/
    ├── llvmkit-support/
    │   └── src/
    │       ├── lib.rs
    │       ├── span.rs              # Span + Spanned<T>
    │       └── source_map.rs        # byte-offset → (line, col)
    └── llvmkit-asmparser/
        ├── README.md
        ├── src/
        │   ├── lib.rs
        │   ├── ll_lexer.rs          # LLLexer.h + LLLexer.cpp
        │   ├── ll_lexer/            # private impl-details for ll_lexer.rs
        │   │   ├── escape.rs        # mirrors UnEscapeLexed (LLLexer.cpp:124)
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
| `llvmkit-ir/` (new crate)                         | `lib/IR/` — Type, Value, Module, … |
| `llvmkit-irbuilder/` (new crate)                  | `IRBuilder.h` + folders              |
| `llvmkit-bitcode/` (new crate)                    | `lib/Bitcode/`, `lib/Bitstream/`     |

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

## Code Conventions

- **Edition**: 2024. Use 2024-only features (e.g. `let chains` in stable form) when they help.
- **Naming**: standard Rust (`snake_case` items, `PascalCase` types, `SCREAMING_SNAKE_CASE` consts). Drop the `LLVM` prefix from ported names — `LLVMContext` becomes `Context`, `LLVMModule` becomes `Module`. The crate name already namespaces them.
- **Modules**: one concept per file; let modules grow before splitting them. `Instructions.h` is 5k lines because it pays for itself; do not pre-split into 40 stub files.
- **Errors**: one crate-level `enum Error` (or a small per-subsystem enum that flattens into it). Avoid `Box<dyn std::error::Error>` in public signatures.
- **Comments**: explain *why*, not *what*. When porting a non-obvious C++ trick, link the source file and line: `// Mirrors LLParser::ParseTopLevelEntities (LLParser.cpp:412)`.
- **Public API**: re-export from `lib.rs`. Keep internal modules `pub(crate)` until an external use case appears.
- **No emojis**, no decorative comments, no boilerplate `mod tests` blocks unless they contain real tests.

## Development Commands

Run from the repository root (`C:/Users/Aslan/rllvm` on the current host).

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

No tests exist beyond the placeholder. The recommended testing strategy as code lands:

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
