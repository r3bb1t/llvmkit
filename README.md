# llvmkit

[![crates.io](https://img.shields.io/crates/v/llvmkit.svg)](https://crates.io/crates/llvmkit)
[![docs.rs](https://docs.rs/llvmkit/badge.svg)](https://docs.rs/llvmkit)
[![License](https://img.shields.io/crates/l/llvmkit.svg)](https://github.com/r3bb1t/llvmkit#license)

A from-scratch Rust reimplementation of [LLVM](https://llvm.org/) IR APIs.
Today `llvmkit` can lex, parse, build, verify, analyze, and print LLVM IR
without linking against `libLLVM`; bitcode support is still ahead.

## Status

Tracking **LLVM 22.1.4** (`llvmorg-22.1.4`, released 2026-04-21).

Shipped today:

- **`.ll` lexer** — done. `llvmkit-asmparser` ports
  `llvm/lib/AsmParser/LLLexer.cpp` and borrows directly from the source slice,
  allocating only when escape decoding actually changes bytes.
- **`.ll` parser** — done for the constructive subset. Parses module-level
  directives (target datalayout/triple, module asm, type definitions, globals,
  function declarations and definitions), all 42 instruction opcodes, metadata
  (standalone numbered nodes, named metadata, instruction trailing attachments),
  and value forms (integer/float literals, undef, poison, null,
  zeroinitializer, global/function references). Round-trip tested via
  `format!("{module}")`.
- **Typed IR data model** — done. `llvmkit-ir` ships interned types, typed
  values, typed constants, functions, basic blocks, globals, comdats, data
  layout, target triple, and module asm directives.
- **IR construction** — done for the currently modeled instruction families.
  The builder covers integer and floating-point arithmetic, comparisons,
  casts, memory ops, GEP, calls, select, phi, the Parser-1 terminator / EH /
  atomic families, and the associated typed-return / typestate surfaces.
- **AsmWriter** — done for the shipped surface. `format!("{module}")`
  produces real textual LLVM IR.
- **Verifier** — done for the shipped surface, including CFG-backed PHI checks
  and cross-block SSA dominance checks through a recomputed dominator tree.
- **CFG and dominance queries** — done. `FunctionCfg`, `BasicBlockEdge`,
  `BasicBlock::successors()`, and `DominatorTree` are available as reusable IR
  queries.
- **Minimal new-PM-inspired pass substrate** — done.
  `PreservedAnalyses`, `FunctionAnalysisManager`, `ModuleAnalysisManager`,
  `FunctionPassManager`, `ModulePassManager`,
  `ModuleToFunctionPassAdaptor`, and `PassInstrumentationCallbacks` are
  shipped.

Not shipped yet:

- **Metadata propagation to instructions** (parsed and accepted, not yet stored
  per-instruction)
- **Bitcode reader / writer**
- **Built-in optimization transforms and pipeline builders** (`PassBuilder`,
  loop PM, CGSCC PM, legacy PM, textual pipelines)

Out of scope:

- code generation
- target backends
- linking / object emission
- any dependency on `llvm-sys`, `inkwell`, or `libLLVM`

## Quick Start

Lex a `.ll` string:

```rust
use llvmkit_asmparser::ll_lexer::Lexer;

let mut lex = Lexer::from("@x = i32 42");
while let Some(tok) = lex.next() {
    let spanned = tok.expect("lex error");
    println!("{:?}", spanned);
}
```

Build IR programmatically:

```rust
use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module};

fn build() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("demo", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32>("add", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let lhs: IntValue<i32> = f.param(0)?.try_into()?;
        let rhs: IntValue<i32> = f.param(1)?.try_into()?;
        let sum = b.build_int_add(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        print!("{m}");
        Ok(())
    })
}
```

Run the examples:

```bash
# Lex a file from disk
cargo run -p llvmkit-asmparser --example lex_file -- crates/llvmkit-asmparser/examples/demo.ll

# Build and print IR programmatically
cargo run -p llvmkit-ir --example build_add_function
cargo run -p llvmkit-ir --example cpu_state_add
cargo run -p llvmkit-ir --example factorial
cargo run -p llvmkit-ir --example concurrent_counter

# Build IR, run a built-in analysis, and register custom passes
cargo run -p llvmkit-ir --example pass_manager_demo
```

## Built-in Analyses and Custom Passes

`llvmkit-ir` ships a branded, verified-state pass layer for querying analyses
and running LLVM-like passes over the modeled IR. Fresh modules are created
with `Module::with_new`, whose closure carries the generative module brand;
read-only pass pipelines preserve `Module<'ctx, B, Verified>`, while transform
pipelines return `Module<'ctx, B, Unverified>`.

Built-in analysis available today:

- `DominatorTreeAnalysis`

Core pass / analysis infrastructure available today:

- `FunctionAnalysisManager`
- `ModuleAnalysisManager`
- `FunctionPassContext`
- `ModulePassContext`
- `FunctionPassManager`
- `ModulePassManager`
- `ModuleToFunctionPassAdaptor`
- `PreservedAnalyses`
- `PassInstrumentationCallbacks`

Register a built-in analysis and a custom function pass:

```rust
use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionPassManager, IrResult, Module,
    ModuleAnalysisManager, ModulePassManager, ModuleToFunctionPassAdaptor, PreservedAnalyses,
    PreservesVerification, ReadOnlyFunctionPass, ReadOnlyFunctionPassContext,
};

struct MyFunctionPass;

impl<'ctx> ReadOnlyFunctionPass<'ctx> for MyFunctionPass {
    fn run(
        &mut self,
        cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        let function = cx.function();
        let dt = cx.analysis::<DominatorTreeAnalysis>()?;
        let entry = function.entry_block().expect("function body");
        assert!(dt.is_reachable_from_entry(entry));
        Ok(PreservedAnalyses::all())
    }
}

fn run_passes() -> IrResult<()> {
    Module::with_new::<_, _, _>("passes", |m| {
        // Build or parse functions into `m` here.
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DominatorTreeAnalysis);

        let mut fpm = FunctionPassManager::<_, PreservesVerification>::new_read_only();
        fpm.add_pass(MyFunctionPass);

        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));

        let mut mam = ModuleAnalysisManager::new();
        let _verified = mpm.run(m.verify()?, &mut mam, &mut fam)?;
        Ok(())
    })
}
```

For a runnable end-to-end version, see
`crates/llvmkit-ir/examples/pass_manager_demo.rs`.

| LLVM new PM concept | llvmkit API |
|---|---|
| `FunctionPass::run(Function &, FunctionAnalysisManager &)` | `FunctionPass::run(&mut FunctionPassContext)` |
| `ModulePass::run(Module &, ModuleAnalysisManager &)` | `ModulePass::run(&mut ModulePassContext)` |
| `PreservedAnalyses::all()` / `none()` | same names |
| `FAM.getResult<A>(F)` | `cx.analysis::<A>()` inside a function pass, `fam.get_result::<A>(FunctionView)` outside |
| `ModuleToFunctionPassAdaptor` | same name; function passes read cached module analyses only |
| mutating a module pass | use a `MutatesIr` manager, call `cx.module_mut()`, receive `Module<'ctx, B, Unverified>` |

Important boundary: the crate currently ships **pass infrastructure and one
built-in analysis**, not a full optimization pipeline. There is no public
`PassBuilder`, no loop / CGSCC manager surface, and no library of built-in IR
transform passes yet.

## Project Structure

```text
<repo root>/
├── Cargo.toml                       # [workspace] only
├── llvmkit/                         # umbrella crate
└── crates/
    ├── llvmkit-support/             # Span, Spanned<T>, SourceMap
    ├── llvmkit-asmparser/           # Lexer + .ll parser
    └── llvmkit-ir/                  # Typed IR model, builder, verifier, passes
```

Every Rust file pairs to a specific upstream LLVM concept. See
[AGENTS.md](AGENTS.md) for the detailed source-tree map and the current
port-status ledger, and [UPSTREAM.md](UPSTREAM.md) for the per-test provenance
registry.

## Design Principles

- **Track LLVM's behavior.** The Rust port aims to match upstream observable
  behavior on a per-file basis. Disagreements are bugs unless explicitly
  documented as Rust-side improvements.
- **Make invalid IR unrepresentable.** LLVM often relies on runtime checks;
  `llvmkit` pushes those distinctions into the Rust type system whenever the
  modeled surface can support it.
- **No FFI, no `bindgen`, no `llvm-sys`.** All functionality is implemented in
  Rust. Nothing links against `libLLVM`.
- **Safe Rust only.** Every workspace crate uses `#![forbid(unsafe_code)]`.
- **One concept, one representation.** CFG queries, dominance, analysis
  invalidation, and pass sequencing live in dedicated modules instead of being
  reimplemented ad hoc in verifier or examples.

## Type-Safety Doctrine

Type safety is `llvmkit`'s main differentiator. Eleven rules govern the public
surface; cite them by id (`D1`-`D11`) in reviews and commit messages.

- **D1. State machines are typestates.** If a value has more than one
  operational state, those states are distinct types.
- **D2. Linear-typed handles for irreversible operations.** Irreversible
  actions consume `self` on non-`Copy` handles.
- **D3. Erased forms are explicitly opt-in.** Typed handles default to the
  strongest static shape available; runtime-erased `Dyn` forms are explicit.
- **D4. Result types reflect operand types.** Builder return types preserve the
  operand category and width/kind information whenever the call site knows it.
- **D5. Operand registration is structural.** Use-list and operand traversal
  updates live in one exhaustive place per construction / mutation primitive.
- **D6. Aggregate types preserve element shape.** Aggregate typing is modeled
  directly rather than flattened into weak runtime predicates.
- **D7. Cross-module mixing is rejected.** Handles carry a generative module
  brand plus the owning `ModuleRef`, so values from one `Module::with_new`
  closure cannot be used with another module's builders.
- **D8. Verified guarantees are explicit.** Verification consumes an
  unverified token and produces `Module<'ctx, B, Verified>`. Read-only pass
  managers preserve that verified state at the type level; transform managers
  return `Module<'ctx, B, Unverified>`, so their output must be verified again
  before another verified-only pipeline can consume it.
- **D9. Iteration safety is structural.** Mutating-while-iterating uses
  dedicated cursor APIs rather than relying on caller discipline.
- **D10. No undefined behavior, by design.** Legal API calls must produce
  defined IR behavior; deferred traps and invalid combinations surface as typed
  errors or explicit IR states, not silent UB.
- **D11. Tests are ported, not invented.** Every `#[test]` in the workspace is
  traced in [UPSTREAM.md](UPSTREAM.md) to an upstream unit test, verifier
  fixture, assembler fixture, or an explicitly-labeled example lock.

## References

- [LLVM Project](https://llvm.org/)
- [LLVM Language Reference](https://llvm.org/docs/LangRef.html)
- [Using the New Pass Manager](https://llvm.org/docs/NewPassManager.html)
- [Writing an LLVM New PM Pass](https://releases.llvm.org/21.1.0/docs/WritingAnLLVMNewPMPass.html)
- [LLVM 22.1.4 release](https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.4)

## License

This project is a derivative work of the [LLVM Project](https://llvm.org/) and
is licensed under the [Apache License v2.0 with LLVM Exceptions](LICENSE)
(`Apache-2.0 WITH LLVM-exception`) — the same license LLVM ships under.
