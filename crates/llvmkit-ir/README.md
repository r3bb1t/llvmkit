# llvmkit-ir

[![crates.io](https://img.shields.io/crates/v/llvmkit-ir.svg)](https://crates.io/crates/llvmkit-ir)
[![docs.rs](https://docs.rs/llvmkit-ir/badge.svg)](https://docs.rs/llvmkit-ir)
[![License](https://img.shields.io/crates/l/llvmkit-ir.svg)](https://github.com/r3bb1t/llvmkit#license)

LLVM IR data model in pure safe Rust.

This crate mirrors the relevant `llvm/lib/IR/` and `llvm/include/llvm/IR/`
surfaces from LLVM 22.1.4. The shipped layer includes typed IR construction,
LLVM-style function-local value-name uniquing, AsmWriter support, represented
ConstantExpr construction/folding, the default ConstantFolder matching
`ConstantFolder.h` for the modeled IR surface, target-independent pure-constant
folds ported from `ConstantFold.cpp`, structural verification, shared CFG
queries, recompute-on-demand dominance, and a capability-graded
new-pass-manager-inspired analysis / pass layer: a pass declares a capability
*rung* and the pipeline's output typestate is derived from it — an all-read-only
(`Inspect`) run preserves `Module<Verified>`, while any mutating pass yields
`Module<Unverified>`, so over-claiming what a pass preserves is a compile error.
Raw `ModuleCore` storage stays crate-private; public APIs use branded `Module`
tokens and gate saved-handle mutators on `&Module<Unverified>`.

DataLayout / TLI-dependent folds stay in the analysis-only `constant_folding`
APIs; full optimization-pipeline, bitcode, broad transform-library, and full
KnownBits / ValueTracking parity are not claimed here.

Instruction lifecycle mutation uses linear `Instruction<Attached>` handles:
erase, detach, move, and RAUW consume the handle. Copyable rediscovery paths
return `InstructionView`, and cursor-driven mutation goes through
`BlockCursor::next` on an unterminated block.


Every `Module::with_new` session carries a fresh compile-time module brand.
Normal users do not write that brand: builder, type, constant, global, and block
APIs infer it from the `Module` or type receiver. Cross-module operands are
therefore rejected by Rust's type checker instead of by a runtime "foreign
value" error. Advanced extension APIs, such as generic pass or folder helpers,
may name `B: ModuleBrand` when they intentionally abstract over any module
brand.

Use the umbrella `llvmkit` crate when you want one dependency that also exposes
the textual IR parser and shared support utilities.

## License

Apache-2.0 WITH LLVM-exception. See the workspace
[`LICENSE`](https://github.com/r3bb1t/llvmkit/blob/main/LICENSE).
