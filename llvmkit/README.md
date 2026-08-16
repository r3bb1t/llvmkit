# llvmkit

[![crates.io](https://img.shields.io/crates/v/llvmkit.svg)](https://crates.io/crates/llvmkit)
[![docs.rs](https://docs.rs/llvmkit/badge.svg)](https://docs.rs/llvmkit)
[![License](https://img.shields.io/crates/l/llvmkit.svg)](https://github.com/r3bb1t/llvmkit#license)

Public umbrella crate for the `llvmkit` workspace, tracking LLVM 22.1.4.

The crate is intentionally thin: it groups the implementation crates under
stable module names so users can depend on one crate. Nothing is reimplemented
here, and every name below is a plain re-export.

- `llvmkit::ir` — typed LLVM IR data model, builder, verifier, AsmWriter,
  ConstantExpr construction/folding, the default ConstantFolder for the modeled
  LLVM 22.1.4 IR surface, `ApInt` / `ApFloat`, `DataLayout`, CFG, dominance,
  value-tracking queries, the auto-SSA frontend, the capability-graded
  analysis / pass layer, and the `#[derive(IrStruct)]` / `#[function_pass]` /
  `#[module_pass]` macros.
- `llvmkit::asmparser` — textual LLVM IR (`.ll`) lexer and parser.
- `llvmkit::support` — shared source-location utilities (`Span`, `Spanned`,
  `SourceMap`).

The macros arrive through `llvmkit-ir`'s `macros` feature, forwarded by this
crate's own default `macros` feature. Turn them off with
`default-features = false`; the proc-macro crate is compiled either way, so the
feature gates only the re-export surface.

Not here, and not planned in this crate: code generation, target backends,
linking, and object emission. Bitcode and debug info are future work.

See the workspace [`README`](https://github.com/r3bb1t/llvmkit#readme) for the
full status and roadmap.

## License

Apache-2.0 WITH LLVM-exception. See [`LICENSE`](LICENSE).
