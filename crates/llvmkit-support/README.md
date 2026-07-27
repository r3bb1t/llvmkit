# llvmkit-support

[![crates.io](https://img.shields.io/crates/v/llvmkit-support.svg)](https://crates.io/crates/llvmkit-support)
[![docs.rs](https://docs.rs/llvmkit-support/badge.svg)](https://docs.rs/llvmkit-support)
[![License](https://img.shields.io/crates/l/llvmkit-support.svg)](https://github.com/r3bb1t/llvmkit#license)

Shared source-location utilities for the `llvmkit` family.

The whole crate is three types: `Span` (a byte range), `Spanned<T>` (a value
carrying one), and `SourceMap` (byte offset → line/column). It has no
dependencies of its own. `llvmkit-asmparser` uses it for token spans and
diagnostics; the umbrella `llvmkit` crate re-exports it as `llvmkit::support`.

Future cross-crate helpers belong here when more than one crate needs them.
IR-specific numeric cores such as `ApInt` / `ApFloat` do not — they live in
`llvmkit-ir`, alongside the IR types they serve.

## License

Apache-2.0 WITH LLVM-exception. See the workspace
[`LICENSE`](https://github.com/r3bb1t/llvmkit/blob/main/LICENSE).
