# llvmkit

[![crates.io](https://img.shields.io/crates/v/llvmkit.svg)](https://crates.io/crates/llvmkit)
[![docs.rs](https://docs.rs/llvmkit/badge.svg)](https://docs.rs/llvmkit)
[![License](https://img.shields.io/crates/l/llvmkit.svg)](https://github.com/r3bb1t/llvmkit#license)

Public umbrella crate for the `llvmkit` workspace.

`llvmkit` groups the implementation crates under stable module names so users can depend on one crate:

- `llvmkit::ir` — typed LLVM IR data model, builder, verifier, AsmWriter, CFG and dominance queries.
- `llvmkit::asmparser` — textual LLVM IR (`.ll`) lexer and parser.
- `llvmkit::support` — shared source-location utilities.

## License

Apache-2.0 WITH LLVM-exception. See the workspace [LICENSE](../LICENSE).
