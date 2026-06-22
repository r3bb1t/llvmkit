# llvmkit

[![crates.io](https://img.shields.io/crates/v/llvmkit.svg)](https://crates.io/crates/llvmkit)
[![docs.rs](https://docs.rs/llvmkit/badge.svg)](https://docs.rs/llvmkit)
[![License](https://img.shields.io/crates/l/llvmkit.svg)](https://github.com/r3bb1t/llvmkit#license)

Public umbrella crate for the `llvmkit` workspace, tracking LLVM 22.1.4.

`llvmkit` groups the implementation crates under stable module names so users
can depend on one crate:

- `llvmkit::ir` — typed LLVM IR data model, builder, verifier, AsmWriter, CFG,
  dominance, and value-tracking queries.
- `llvmkit::asmparser` — textual LLVM IR (`.ll`) lexer and parser.
- `llvmkit::support` — shared source-location utilities.

See the workspace [`README`](https://github.com/r3bb1t/llvmkit#readme) for the
full status and roadmap.

## License

Apache-2.0 WITH LLVM-exception. See the workspace
[`LICENSE`](https://github.com/r3bb1t/llvmkit/blob/main/LICENSE).
