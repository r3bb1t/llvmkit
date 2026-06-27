# llvmkit-support

[![crates.io](https://img.shields.io/crates/v/llvmkit-support.svg)](https://crates.io/crates/llvmkit-support)
[![docs.rs](https://docs.rs/llvmkit-support/badge.svg)](https://docs.rs/llvmkit-support)
[![License](https://img.shields.io/crates/l/llvmkit-support.svg)](https://github.com/r3bb1t/llvmkit#license)

Shared support utilities for the `llvmkit` family.

Currently exposes `Span`, `Spanned`, and `SourceMap`. Future cross-crate helpers
belong here when multiple crates need them; IR-specific numeric cores such as
`ApInt` / `ApFloat` live in `llvmkit-ir`.

## License

Apache-2.0 WITH LLVM-exception. See the workspace
[`LICENSE`](https://github.com/r3bb1t/llvmkit/blob/main/LICENSE).
