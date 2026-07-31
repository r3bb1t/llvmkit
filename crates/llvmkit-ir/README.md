# llvmkit-ir

[![crates.io](https://img.shields.io/crates/v/llvmkit-ir.svg)](https://crates.io/crates/llvmkit-ir)
[![docs.rs](https://docs.rs/llvmkit-ir/badge.svg)](https://docs.rs/llvmkit-ir)
[![License](https://img.shields.io/crates/l/llvmkit-ir.svg)](https://github.com/r3bb1t/llvmkit#license)

LLVM IR data model in pure safe Rust.

This crate mirrors the relevant `llvm/lib/IR/` and `llvm/include/llvm/IR/`
surfaces from LLVM 22.1.4. The shipped layer includes typed IR construction,
LLVM-style function-local value-name uniquing, AsmWriter support, `ApInt` /
`ApFloat` arithmetic, `DataLayout` parsing and type-layout queries, represented
ConstantExpr construction/folding, the default ConstantFolder matching
`ConstantFolder.h` for the modeled IR surface, target-independent pure-constant
folds ported from `ConstantFold.cpp`, structural verification, shared CFG
queries, recompute-on-demand dominance, a Braun-style auto-SSA frontend
(`SsaBuilder`), a `PatternMatch.h`-style matcher DSL, and a capability-graded
new-pass-manager-inspired analysis / pass layer: a pass declares a capability
*rung* and the pipeline's output typestate is derived from it — an all-read-only
(`Inspect`) run preserves `Module<B, Verified>`, while any mutating pass yields
`Module<B, Unverified>`, so over-claiming what a pass preserves is a compile
error. A first set of built-in transforms (`DcePass`, `InstSimplifyPass`,
`SimplifyDemandedBitsPass`) ships on top of it. Raw `ModuleCore` storage stays
crate-private; public APIs use branded `Module` tokens and gate saved-handle
mutators on `&Module<B, Unverified>`.

DataLayout / TLI-dependent folds stay in the analysis-only `constant_folding`
APIs; full optimization-pipeline, bitcode, debug info, broad transform-library,
and full KnownBits / ValueTracking parity are not claimed here.

## Ids, views, and handles

A `Module<B, S>` has no lifetime parameter. It owns its storage, is `Send`, and
can be moved into a struct, a `Vec`, or another thread — so a lifter can suspend,
change threads, and resume.

What you store are **ids**: `Copy + Send + 'static` values carrying a module tag
and an arena slot (`ValueId<B>`, `IntValueId<W, B>`, `FunctionId<R, B>`,
`GlobalId<B>`, `BlockId<R, B, Params>`, …). Declarations and value-producing
builders hand them back, and each `get_*` lookup returns the same currency its
`add_*` twin does. Resolve one into a short-lived borrowing **view** with
`m.view(id)`, or `m.try_view(id)` for the fallible form.

Not everything is an id, by design. Appending a block gives you a linear,
non-`Copy` `BasicBlock` insertion token (call `.id()` for the storable
`BlockId`), and the terminator builders consume the builder by value and return
a borrowing `(BasicBlock<Terminated>, Instruction)` pair — which is what makes
a second terminator on one block a compile error.

Instruction lifecycle mutation likewise uses linear `Instruction<Attached>`
handles: erase, detach, move, and RAUW consume the handle. Copyable rediscovery
paths return `InstructionView`, and cursor-driven mutation goes through
`BlockCursor::next` on an unterminated block.

Phis are not written by hand. The raw `build_*_phi` + `add_incoming` pair is
crate-private; author merges with **block arguments** instead
(`append_block_with_params` / `append_block_typed`, branched to with
`build_br_with_args` / `build_cond_br_with_args`), or let `SsaBuilder` discover
them for you. Inside a pass, `FnReshape::insert_phi` is the third route.

## Module brands

Every module carries a compile-time brand — a `'static` type that names it.
`module_new!("name")` mints a fresh one per expansion site,
`Module::branded::<B>` takes a brand you name, and `Module::dynamic` opts out in
favour of the runtime module tag alone. A brand you declare yourself is a
bare unit struct plus an empty impl — `ModuleBrand` requires nothing but
`'static`:

```rust
struct LiftedBin;
impl llvmkit_ir::ModuleBrand for LiftedBin {}
```

Normal users do not write the brand at handle types: builder, type, constant,
global, and block APIs infer it from the `Module` or type receiver. Two modules
with **distinct** brand types cannot exchange operands — that is a type error,
caught by Rust with no runtime check involved. Where two modules deliberately
share a brand (every `Module::dynamic` module is `DynBrand`, and a named brand
is re-issued once the previous module drops), the compile-time half cannot
apply, and the runtime module tag every id carries is the backstop:
`IrError::ForeignValueId`, `None` from `try_view`, or a deterministic panic from
`view`. Metadata is the one currency this does not cover — a metadata slot is a
bare arena index with neither brand nor tag, so an in-range slot from another
module still mis-resolves. Tracked in the workspace's `docs/future-work.md`.

Advanced extension APIs, such as generic pass or folder helpers, may name
`B: ModuleBrand` when they intentionally abstract over any module brand.

Use the umbrella `llvmkit` crate when you want one dependency that also exposes
the textual IR parser and shared support utilities.

## License

Apache-2.0 WITH LLVM-exception. See the workspace
[`LICENSE`](https://github.com/r3bb1t/llvmkit/blob/main/LICENSE).
