# llvmkit-macros

[![crates.io](https://img.shields.io/crates/v/llvmkit-macros.svg)](https://crates.io/crates/llvmkit-macros)
[![docs.rs](https://docs.rs/llvmkit-macros/badge.svg)](https://docs.rs/llvmkit-macros)
[![License](https://img.shields.io/crates/l/llvmkit-macros.svg)](https://github.com/r3bb1t/llvmkit#license)

Procedural macros for the [`llvmkit`](https://crates.io/crates/llvmkit)
family. You almost certainly want `llvmkit` or `llvmkit-ir` instead — this
crate exists because a proc-macro must live in its own crate, and it is
re-exported from there.

Being a proc-macro crate, it runs inside `rustc` at build time and contributes
nothing to the built artifact.

## What it provides

- **`#[derive(Branded)]`** — std-trait impls *without* the per-parameter bounds
  std `derive` infers. Deriving `Clone` on `struct V<'ctx, B: ModuleBrand>`
  normally emits `impl<'ctx, B: ModuleBrand + Clone> Clone`, bounding every
  type parameter whether or not a field uses it. llvmkit's view types carry
  their brand and marker parameters only as `PhantomData`, so those bounds are
  always spurious — and requiring them is what forced four supertraits onto
  `ModuleBrand` before 0.0.4. `Branded` copies the item's generics verbatim and
  adds nothing, which is what lets a brand be a bare `struct LiftedBin;`.

  The default set is `Clone, Copy, Debug, PartialEq, Eq, Hash`;
  `#[branded(…)]` names an explicit subset and may add `Default`. `PartialEq`
  and `Hash` are generated from one shared field walk, so their contract cannot
  drift; `Debug` skips phantom fields; a `Copy` request on a type with a
  non-`Copy` field is still rejected by the compiler (`E0204`).

- **`#[derive(IrStruct)]`** — maps a non-generic named-field Rust struct to an
  llvmkit named-struct schema and generates the matching `<Struct>Value`
  wrapper. Helper attributes: `#[llvmkit(name = "...")]`, `#[llvmkit(packed)]`,
  `#[llvmkit(crate = path::to::ir)]`.

- **`#[function_pass]` / `#[module_pass]`** — ergonomics sugar for the
  capability-graded pass API. Each expands an inherent `impl Pass { fn run(..) }`
  block into exactly the `FunctionPass` / `ModulePass` trait impl you could
  hand-write, hiding the impl header, the associated-item block, and the `run`
  lifetimes. Zero runtime cost: the output *is* that impl.

## Documentation

See the [`llvmkit-ir` docs](https://docs.rs/llvmkit-ir) for the surrounding
API, and the repository's `AGENTS.md` for the conventions each macro serves.

## License

Apache License v2.0 with LLVM Exceptions (`Apache-2.0 WITH LLVM-exception`) —
the same license LLVM ships under. See [LICENSE](LICENSE).
