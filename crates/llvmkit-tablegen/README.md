# llvmkit-tablegen

[![crates.io](https://img.shields.io/crates/v/llvmkit-tablegen.svg)](https://crates.io/crates/llvmkit-tablegen)
[![docs.rs](https://docs.rs/llvmkit-tablegen/badge.svg)](https://docs.rs/llvmkit-tablegen)
[![License](https://img.shields.io/crates/l/llvmkit-tablegen.svg)](LICENSE)

TableGen front end and intrinsic emitter for the
[`llvmkit`](https://crates.io/crates/llvmkit) family, in pure safe Rust.

You almost certainly want `llvmkit` or `llvmkit-ir` instead. This crate is the
*generator* behind `llvmkit-ir`'s intrinsic tables, not something you call at
run time — it is a **build**-dependency of `llvmkit-ir` and does not appear in
the runtime dependency graph.

## What it ports

Two upstream subsystems, which is why it is a crate rather than a build script:

- **The TableGen language front end** — `llvm/lib/TableGen/`: the lexer
  (`TGLexer.cpp`), the parser (`TGParser.cpp`), the record/value model
  (`Record.cpp`), errors (`Error.cpp`), and the driver (`Main.cpp`).
- **The intrinsic emitter** — `llvm/utils/TableGen/Basic/`:
  `CodeGenIntrinsics.cpp`'s per-intrinsic model, and `IntrinsicEmitter.cpp`'s
  IIT type-signature encoding and table output.

Tracking **LLVM 22.1.4**.

## Why a crate and not a build script

It began as `llvmkit-ir/tools/gen_intrinsics.rs`, pulled into `build.rs` with
`#[path]`. That shape cost two things. A build script is not a test target, so
its `#[test]` functions **never ran** under `cargo test` even though each
carried an `UPSTREAM.md` provenance row. And a single 4,000-line file mirrored
no upstream file, in a project whose organising rule is that it mirrors LLVM's.

## Library API

Upstream ships a library plus an `llvm-tblgen` binary; this does the same.

```rust
use llvmkit_tablegen::{generate, vendored_llvm_root, GENERATED_FILE_NAME};

let out = std::path::Path::new(&std::env::var("OUT_DIR")?).join(GENERATED_FILE_NAME);
let generated = generate(&vendored_llvm_root(), &out)?;

// Replay the inputs as build-script dependencies.
for input in generated.inputs() {
    println!("cargo:rerun-if-changed={}", input.display());
}
```

- `generate` expands the `.td` tree into the generated Rust and reports the
  files it read, which `llvmkit-ir`'s `build.rs` replays as
  `cargo:rerun-if-changed` lines.
- `verify_generated` is the `--check` twin: it reports a stale generated file
  without rewriting it. A separate function rather than a `bool` parameter, so
  the two intents cannot be confused at a call site.
- `vendored_llvm_root` resolves `VENDORED_LLVM_ROOT` against *this* crate's
  manifest directory, so the path holds wherever the workspace is checked out.
- Every failure is a `TableGenError`.

## Command line

Mirrors `llvm-tblgen`, for manual regeneration and for CI staleness checks:

```text
llvmkit-tablegen [--check] <generated-file>
llvmkit-tablegen [--check] <llvm-root> <generated-file>
```

## The vendored input

`tablegen/llvm-22.1.4/` holds the 27 `.td` files this expands — `Intrinsics.td`,
the per-target `IntrinsicsAArch64.td` / `IntrinsicsAMDGPU.td` / … set, and the
`CodeGen` support tables they include. They live beside the generator that reads
them.

They are kept because the `.td` input is roughly six times smaller than the Rust
it expands to, and because regenerating from a vendored tree is reproducible in
a way that checking in only the output is not. As with `orig_cpp/`, the vendored
tree is **read-only reference material** — it is never built as C++.

## License

Apache-2.0 WITH LLVM-exception (the same license LLVM ships under). See
[`LICENSE`](LICENSE).
