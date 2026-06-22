# llvmkit-ir

[![crates.io](https://img.shields.io/crates/v/llvmkit-ir.svg)](https://crates.io/crates/llvmkit-ir)
[![docs.rs](https://docs.rs/llvmkit-ir/badge.svg)](https://docs.rs/llvmkit-ir)
[![License](https://img.shields.io/crates/l/llvmkit-ir.svg)](https://github.com/r3bb1t/llvmkit#license)

LLVM IR data model in pure safe Rust.

This crate mirrors the relevant `llvm/lib/IR/` and `llvm/include/llvm/IR/`
surfaces from LLVM 22.1.4. The shipped layer includes typed IR construction,
LLVM-style function-local value-name uniquing, AsmWriter support, structural
verification, shared CFG queries, recompute-on-demand dominance, and
effect-typed new-pass-manager-inspired analysis / pass managers: read-only
pipelines preserve `Module<Verified>`, while transform pipelines return
`Module<Unverified>`. Raw `ModuleCore` storage stays crate-private; public APIs
use branded `Module` tokens and gate saved-handle mutators on
`&Module<Unverified>`.

Use the umbrella `llvmkit` crate when you want one dependency that also exposes
the textual IR parser and shared support utilities.

## License

Apache-2.0 WITH LLVM-exception. See the workspace
[`LICENSE`](https://github.com/r3bb1t/llvmkit/blob/main/LICENSE).
