# llvmkit

A from-scratch Rust reimplementation of [LLVM](https://llvm.org/) IR APIs.
Today `llvmkit` can lex, build, verify, analyze, and print LLVM IR without
linking against `libLLVM`; the `.ll` parser and bitcode support are still
ahead.

## Status

Tracking **LLVM 22.1.4** (`llvmorg-22.1.4`, released 2026-04-21).

Shipped today:

- **`.ll` lexer** — done. `llvmkit-asmparser` ports
  `llvm/lib/AsmParser/LLLexer.cpp` and borrows directly from the source slice,
  allocating only when escape decoding actually changes bytes.
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

- **`.ll` parser**
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
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<i32>("add", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let lhs: IntValue<i32> = f.param(0)?.try_into()?;
    let rhs: IntValue<i32> = f.param(1)?.try_into()?;
    let sum = b.build_int_add(lhs, rhs, "sum")?;
    b.build_ret(sum)?;

    print!("{m}");
    Ok(())
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

`llvmkit-ir` now ships the minimum pass-readiness layer needed to query and run
LLVM-like analyses over the modeled IR.

Built-in analysis available today:

- `DominatorTreeAnalysis`

Core pass / analysis infrastructure available today:

- `FunctionAnalysisManager`
- `ModuleAnalysisManager`
- `FunctionPassManager`
- `ModulePassManager`
- `ModuleToFunctionPassAdaptor`
- `PreservedAnalyses`
- `PassInstrumentationCallbacks`

Planned next tightening: keep the pass / analysis substrate but distinguish
**verified** and **unverified** modules explicitly at pass entry points. The
current infrastructure is intentionally a stepping stone toward a
`VerifiedModule<'ctx>`-aware surface, not a replacement for that typestate
boundary.

Register a built-in analysis and a custom function pass:

```rust
use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionPass, FunctionPassManager,
    Module, ModuleAnalysisManager, ModulePassManager, ModuleToFunctionPassAdaptor,
    PreservedAnalyses,
};

struct MyFunctionPass;

impl<'ctx> FunctionPass<'ctx> for MyFunctionPass {
    fn run(
        &mut self,
        function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        let dt = fam.get_result::<DominatorTreeAnalysis>(function)?;
        let entry = function.entry_block().expect("function body");
        assert!(dt.is_reachable_from_entry(entry));
        Ok(PreservedAnalyses::all())
    }
}

fn run_passes(m: &Module<'_>) -> llvmkit_ir::IrResult<()> {
    let function = m.function_by_name("my_function").expect("function exists");

    let mut fam = FunctionAnalysisManager::new();
    fam.register_pass(DominatorTreeAnalysis);
    let _ = fam.get_result::<DominatorTreeAnalysis>(function)?;

    let mut fpm = FunctionPassManager::new();
    fpm.add_pass(MyFunctionPass);

    let mut mpm = ModulePassManager::new();
    mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));

    let mut mam = ModuleAnalysisManager::new();
    mpm.run(m, &mut mam, &mut fam)?;
    Ok(())
}
```

For a runnable end-to-end version, see
`crates/llvmkit-ir/examples/pass_manager_demo.rs`.

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
    ├── llvmkit-asmparser/           # Lexer today, parser later
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
- **D7. Cross-module mixing is rejected.** Handles carry both a `'ctx` brand
  and a `ModuleRef` runtime identity check.
- **D8. Verified guarantees are explicit.** `Module::verify()` produces a
  `VerifiedModule<'ctx>` brand. The minimal pass infrastructure is shipped, and
  the planned next cutover is to make analysis / transform entry points reflect
  the verified-vs-unverified distinction directly instead of collapsing both
  states into the same module-facing surface.
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
