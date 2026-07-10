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
  function declarations and definitions), all instruction opcodes, metadata
  (standalone numbered nodes, named metadata, instruction trailing attachments),
  and value forms (integer/float literals, undef, poison, null,
  zeroinitializer, global/function references, and represented `ConstantExpr`
  forms for parser-needed opcodes, including upstream vector GEP, bitcast, cast,
  and select folding fixtures). Round-trip tested via `format!("{module}")`.
- **Typed IR data model** — done. `llvmkit-ir` ships interned types, typed
  values, typed constants, functions, basic blocks, globals, comdats, data
  layout, target triple, module asm directives, and LLVM-style function-local
  value-name uniquing across arguments, blocks, and instructions.
- **IR construction** — done for the currently modeled instruction families.
  The builder covers integer and floating-point arithmetic, comparisons,
  casts, memory ops, GEP, calls, select, phi, the Parser-1 terminator / EH /
  atomic families, and the associated typed-return / typestate surfaces. The
  default `ConstantFolder` mirrors `llvm/include/llvm/IR/ConstantFolder.h` for
  the modeled IR surface and routes target-independent pure-constant folds
  through the LLVM 22.1.4 `ConstantFold.cpp`-derived helper layer.
- **AsmWriter** — done for the shipped surface. `format!("{module}")`
  produces real textual LLVM IR, including upstream folded `ConstantExpr` forms
  for vector GEP, bitcast, cast, and select fixtures.
- **Verifier** — done for the shipped surface, including CFG-backed PHI checks
  and cross-block SSA dominance checks through a recomputed dominator tree.
- **CFG and dominance queries** — done. `FunctionCfg`, `BasicBlockEdge`,
  `BasicBlock::successors()`, and `DominatorTree` are available as reusable IR
  queries.
- **Capability-graded pass API** — done, including explicit
  analysis invalidation. A pass declares a capability *rung*
  (`Inspect` / `PatchBody` / `ReshapeCfg` / `RewriteModule`) and its required
  analyses; the driver *derives* which analyses survive and whether the output
  module is still verified, so over-claiming what a pass preserves is a compile
  error rather than a stale-analysis miscompile. Ships the `FunctionPass` /
  `ModulePass` traits, single-pass drivers (`run_function_pass` /
  `run_module_pass`), compile-time tuple pipelines (`function_pipeline` /
  `module_pipeline` / `for_each_function`), runtime-assembled `Dyn` containers,
  the bundled `Analyses` manager, `PreservedAnalyses`,
  `PassInstrumentationCallbacks`, and the `#[function_pass]` / `#[module_pass]`
  authoring macros. See [Built-in Analyses and Custom Passes](#built-in-analyses-and-custom-passes).
- **KnownBits / ValueTracking subset** — shipped for represented integer,
  pointer, fixed-vector, and intrinsic facts; full LLVM parity is not claimed.
  The surface includes `KnownBits`, `compute_known_bits`,
  `KnownBitsAnalysis`, `ValueTrackingQuery`, recursion budgeting,
  dominator-tree hooks, and a reusable per-analysis cache.
- **Represented intrinsic signatures and facts** — shipped for the modeled
  `llvm.*` signature families listed in `ROADMAP.md`: `assume`; integer or
  fixed-vector overloads of `abs`, bit permutations, counts, funnel shifts,
  min/max, and saturating arithmetic; fixed-vector `vector.reduce.add`;
  `ptrmask`; `vscale`; and the represented lifetime, memory, trap,
  cycle-counter, and register helpers. KnownBits/DemandedBits facts are limited
  to the shipped subset (for example constant-amount funnel shifts, bit
  permutations, counts, saturation arithmetic, min/max, vector-reduce add, and
  `ptrmask`). Range metadata, range attributes on function/call returns, and
  `returned` call/invoke arguments feed known-bits queries. Unsupported ordinary
  calls stay unknown, and unsupported `llvm.*` intrinsics are rejected unless
  their IDs, signatures, and verifier rules are represented.
- **Demanded-bits and initial scalar cleanup transforms** — shipped for the
  modeled scalar-integer slice. `DemandedBitsAnalysis` covers the represented
  operator and intrinsic operand-mask subset; `SimplifyDemandedBitsPass`
  includes scalar-integer constant replacement, no-use dead instruction-chain
  erasure, and the upstream `assoc-cast-assoc.ll::AndZextAnd` demanded-mask
  transform. `InstSimplifyPass` and `DcePass` provide the first conservative
  runnable O1-style scalar cleanup passes.
- **Strict upstream fixture/provenance policy** — in force. Behavior is derived
  from LLVM 22.1.4 sources and in-tree fixtures with `UPSTREAM.md` anchors; no
  shipped analysis fact is a stub, and tests/runtime do not depend on
  `orig_cpp` or hidden C++ fixtures.

Not shipped yet:

- **Full analysis / optimization constant-folding parity beyond the represented
  target-independent `ConstantFolder` surface** — DataLayout / TLI-dependent
  folds remain in analysis-only APIs where represented; the default folder does
  not ship LLVM's full optimization pipeline or broad transform library.

- **Full metadata / attribute surface beyond the represented range,
  `absolute_symbol`, debug/use-list, and `returned` facts**
- **Bitcode reader / writer**
- **Full KnownBits / ValueTracking / DemandedBits / SimplifyDemandedBits
  parity** — the parity ledger remains open for remaining `KnownBits.cpp`
  formulas, `ValueTracking.cpp` operator arms, demanded-bit rules, and
  `InstCombineSimplifyDemanded` transforms.
- **Additional or currently unrepresented `llvm.*` intrinsic IDs, signatures,
  and facts** — new IDs and verifier signatures must land before analysis facts
  are added.
- **Full built-in optimization transform library and pipeline builders**
  (`PassBuilder`, loop PM, CGSCC PM, legacy PM, textual pipelines)

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
use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

fn build() -> Result<(), IrError> {
    Module::with_new("demo", |m| {
        let f = m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (lhs, rhs) = f.params();
        let sum = b.build_int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        print!("{m}");
        Ok(())
    })
}
```

Typed function facades are for signatures known in Rust. Parser or dynamic IR
code keeps using `FunctionValue::param` / `params`; a typed facade uses the tuple
parameter schema instead, so wrong typed access fails at compile time and
`TypedFunctionValue::params()` is infallible after construction.
`TypedFunctionValue::try_from_function` is the fallible boundary for wrapping an
existing raw function with a mismatched signature. For ordinary Rust function
pointer aliases, `m.add_typed_function_of::<fn(i32) -> i32, _>(...)` builds the
LLVM signature directly from the alias.

### Typed calls

`IRBuilder::build_call` takes a `TypedFunctionValue` callee and an argument
tuple typed against its parameter schema. Wrong arity, a wrong-typed argument,
or misusing a void call's result are all compile errors instead of runtime
`IrError`s or verifier failures, and the result narrows to the callee's real
return type with no `try_into`:

```rust
use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

fn build_typed_call() -> Result<(), IrError> {
    Module::with_new("demo", |m| {
        let callee = m.add_typed_function::<i32, (i32, i32), _>("add_inner", Linkage::External)?;
        let entry = callee.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (lhs, rhs) = callee.params();
        let sum = b.build_int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        let caller = m.add_typed_function::<i32, (i32, i32), _>("caller", Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (x, y) = caller.params();

        // `call.result()` is already `IntValue<i32>` -- no `try_into`.
        let call = b.build_call(callee, (x, y), "r")?;
        b.build_ret(call.result())?;

        print!("{m}");
        Ok(())
    })
}
```

A callee whose signature is only known at runtime (parsed IR, an `extern`
declaration built from user input) keeps using the `_dyn` counterparts —
`build_call_dyn`, `build_indirect_call_dyn`, `build_invoke_dyn` — which take a
plain `FunctionValue` and an iterable of pre-widened `Value`s, and reject a
wrong argument count or type with `IrError::CallArgumentCountMismatch` /
`CallArgumentTypeMismatch` at build time rather than deferring to the verifier.
`build_indirect_call::<Sig>` derives the callee's function type from a Rust
function-pointer schema `Sig` instead of taking it by hand; `build_varargs_call`
lowers a fixed, schema-typed prefix the same way `build_call` does and appends
an erased `...` tail, matching LLVM's own no-static-check contract on variadic
arguments.

Derived struct schemas let you derive the schema on a plain Rust struct, use the
generated `<Struct>Value<'ctx, B>` wrapper in IR, and call field
accessors/builders instead of indexing aggregates manually:

```rust
use llvmkit_ir::{IRBuilder, IrStruct, Linkage, Module};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(IrStruct)]
struct Rect {
    min: Point,
    max: Point,
}

#[derive(IrStruct)]
struct WindowPlacement {
    show_cmd: i32,
    normal_position: Rect,
}

type Normalize = fn(WindowPlacement) -> WindowPlacement;

Module::with_new("window", |m| {
    let f = m.add_typed_function_of::<Normalize, _>("normalize", Linkage::External)?;
    let entry = f.append_basic_block(&m, "entry");
    let b = IRBuilder::new_for_return::<Normalize>(&m).position_at_end(entry);
    let (placement,) = f.params();
    // `normal_position` returns `RectValue<'ctx, B>`, and `min` returns
    // `PointValue<'ctx, B>`; nested structs keep their generated wrapper type.
    let rect = placement.normal_position(&b)?;
    let _min = rect.min(&b)?;
    Ok(())
})?;
```

Existing IR can be checked back into a generated wrapper with
`WindowPlacementValue::try_from(raw)?`. When a function boundary should receive
top-level fields separately, `StructFields<WindowPlacement>` emits `i32, %Rect`
parameters while nested structs keep their generated wrapper values.

Helper attributes are intentionally small: `#[llvmkit(name = "...")]` overrides
the LLVM identified-struct name, `#[llvmkit(packed)]` emits a packed body, and
`#[llvmkit(crate = path::to::ir)]` overrides the generated crate path. Field
rename/skip/default helpers do not ship because LLVM struct layout is positional
and hiding field changes would obscure ABI/layout changes.

Detailed macro docs: [IrStruct derive macro](docs/ir-struct-derive.md).

### Typed pointers

`TypedPointerValue<'ctx, T: IrField, B>` overlays a compile-time pointee
schema on top of a plain opaque `ptr` value -- it is Rust-side bookkeeping
only, so printed IR is byte-identical to the erased path. `PointerValue::with_pointee::<T>()`
attaches the schema as an explicit, documented assertion (exactly as
powerful as passing a type to `build_load` today; a mis-assertion produces
wrong IR that the verifier catches, never memory-unsafe behavior).
`build_typed_alloca::<T>`, `build_typed_load`, and `build_typed_store` skip
the runtime type-narrowing that the erased path needs, and
`build_field_gep::<S, I>` projects the field type at compile time straight
from a `#[derive(IrStruct)]` schema -- an out-of-range field index is a
missing trait impl, not a runtime bounds check.

### Auto-SSA: typed local variables instead of manual phi wiring

`SsaBuilder` (`crates/llvmkit-ir/src/ssa_builder.rs`) sits on top of the
typed `IRBuilder` and implements Braun et al.'s 2013 on-the-fly SSA
construction algorithm (the same family of technique Cranelift's
`FunctionBuilder` uses). Instead of pre-declaring phi nodes and patching
their incoming edges by hand, you declare a typed variable once and then
`def_*_var`/`use_*_var` it like a mutable local; the engine inserts,
completes, and trivial-phi-eliminates the phis for you as blocks are sealed.

The point isn't fewer lines -- the auto-SSA version of a loop is not
shorter than its manually-phi-wired twin. It is **less error-prone and more
declarative**: there is no phi pre-declaration, no incoming-edge patching,
and no label plumbing to get wrong. Compare the loop body of
`examples/factorial.rs` (manual phis) and `examples/factorial_auto_ssa.rs`
(auto-SSA) -- both are byte-parity locked to print the identical `.ll`:

```rust
// Manual phi wiring (examples/factorial.rs): declare empty phis up front,
// build the loop body, then patch both incoming edges by hand.
let acc_phi = b.build_int_phi::<i32, _>("acc")?;
let i_phi = b.build_int_phi::<i32, _>("i")?;
let acc = acc_phi.as_int_value();
let i = i_phi.as_int_value();
let next_acc = b.build_int_mul(acc, i, "next_acc")?;
let next_i = b.build_int_sub(i, 1_i32, "next_i")?;
// ... build the rest of the loop body, then:
acc_phi.add_incoming(1_i32, entry_label)?.add_incoming(next_acc, loop_label)?.finish();
i_phi.add_incoming(n, entry_label)?.add_incoming(next_i, loop_label)?.finish();
```

```rust
// Auto-SSA (examples/factorial_auto_ssa.rs): declare typed variables once;
// def/use them like mutable locals. No phi, no incoming-edge bookkeeping,
// no label plumbing -- SsaBuilder inserts and completes the phis itself
// when `loop_bb` is sealed.
let acc_var = b.declare_int_var::<i32, _>("acc");
let i_var = b.declare_int_var::<i32, _>("i");
// entry block:
b.def_int_var(acc_var, 1_i32)?;
b.def_int_var(i_var, n)?;
// loop block:
let i = b.use_int_var(i_var)?;
let acc = b.use_int_var(acc_var)?;
let next_acc = b.ins().build_int_mul(acc, i, "next_acc")?;
let next_i = b.ins().build_int_sub(i, 1_i32, "next_i")?;
b.def_int_var(acc_var, next_acc)?;
b.def_int_var(i_var, next_i)?;
b.seal_block(loop_bb)?; // completes both phis from the now-known predecessor set
```

`SsaBuilder` also turns "branch to a block whose predecessors are already
fully known" (Braun sealing) and "read/write before the builder is
positioned" into typed errors rather than caller discipline: `create_block`
auto-seals the entry block, `seal_block` completes a block's incomplete
phis, and `finish()` is the always-correct seal-everything fallback that
also rejects any created-but-never-filled block. It currently covers int / float /
pointer variables and the `br` / `cond_br` / `switch` / `ret` / `ret_void` /
`unreachable` terminators; mixing in manual phis via `b.ins()` for anything
outside that scope is legal and verifier-checked. See
[`docs/future-work.md`](docs/future-work.md) for the planned scope (aggregate
variables, invoke/EH terminators).

### Why llvmkit instead of inkwell

Both `llvmkit` and [`inkwell`](https://github.com/TheDan64/inkwell) give
Rust a typed LLVM IR-construction API, but they take different positions on
where invalid usage is caught. inkwell wraps `libLLVM` through
`llvm-sys`, and several of its typed accessors resolve their type check at
runtime: calling `into_float_value()` on a value that isn't actually a
float panics rather than failing to compile (see
[wasmer#962](https://github.com/wasmerio/wasmer/issues/962) for a
production crate hitting exactly this), and inkwell's own
[README](https://github.com/TheDan64/inkwell) documents panics on
interior-NUL strings and the lack of a safe multithreaded mode.

`llvmkit` makes the corresponding class of bugs a compile error instead:
conversions between typed handles go through `TryFrom`/lift traits that
either resolve at the call site or fail to compile, module identifiers are
plain owned `String`s (there is no C-string boundary to panic on interior
NULs), and every workspace crate ships `#![forbid(unsafe_code)]` -- there is
no FFI boundary into `libLLVM` to begin with, because `llvmkit` is a
from-scratch reimplementation, not a binding. The tradeoff is real: `llvmkit`
does not generate code, link, or do anything past IR construction and
verification (see "Out of scope" above) -- pick `inkwell` when you need to
reach codegen through upstream LLVM, and `llvmkit` when the task is IR
construction / analysis and compile-time misuse safety matters more than
having `libLLVM`'s full backend behind it.

### Same-module safety

`Module::with_new` gives every module construction session a fresh compile-time
brand. Normal code does not name that brand: values, constants, basic blocks,
globals, and builders infer it from the `Module` or type receiver used to
create them. Builder and mutation APIs therefore reject cross-module operands at
compile time instead of returning a runtime "foreign value" error. Generic
extension code may name `B: ModuleBrand` explicitly when it needs to accept any
module brand; ordinary examples should stay inside the `with_new` closure and
let the receiver drive inference.

### Instruction lifecycle safety

`Instruction<'ctx, state::Attached>` is the lifecycle authority for erase,
detach, move, and RAUW operations. Those methods consume the handle, so a used
lifecycle capability cannot be reused. Copyable discovery APIs return
`InstructionView` instead: blocks, value use-lists, and per-opcode handles expose
read-only inspection without minting a new mutation handle. Cursor-driven
mutation uses `BlockCursor::next` on an unterminated block.

Run the examples:

```bash
# Lex a file from disk
cargo run -p llvmkit-asmparser --example lex_file -- crates/llvmkit-asmparser/examples/demo.ll

# Build and print IR programmatically
cargo run -p llvmkit-ir --example build_add_function
cargo run -p llvmkit-ir --example cpu_state_add
cargo run -p llvmkit-ir --example factorial
cargo run -p llvmkit-ir --example concurrent_counter
cargo run -p llvmkit-ir --example derived_struct_function

# Build IR, run a built-in analysis, and register custom passes
cargo run -p llvmkit-ir --example pass_manager_demo
```

## Built-in Analyses and Custom Passes

`llvmkit-ir` ships a **capability-graded** pass layer for querying
analyses and running LLVM-like passes over the modeled IR. A pass declares a
capability *rung* — how much of the IR it is allowed to touch — plus the
analyses it needs; the driver derives everything else, including which analyses
survive the run and whether the output module is still `Verified`. There is
**no pass-registration step**: a pass is a value you hand to a driver or drop
into a tuple.

| Rung | Level | May mutate | Analyses preserved after a run |
|---|---|---|---|
| `Inspect` | function or module | nothing (read-only) | all |
| `PatchBody` | function | instructions inside existing blocks | CFG-shaped analyses |
| `ReshapeCfg` | function | blocks, terminators, PHIs — the whole CFG | none |
| `RewriteModule` | module | globals, functions, bodies | none |

The rung is the *only* preservation knob, and it is structural: a `PatchBody`
mutator physically has no method that edits a terminator, so "CFG analyses
preserved" is true by construction — never a `PreservedAnalyses` value the
author hand-writes and might get wrong. A lying `PreservedAnalyses` (mutate the
IR, then report everything preserved, leaving stale analyses for a later pass to
miscompile against) is the class of bug LLVM catches only with opt-in
verification; here it is **unspellable**. The `none` for `ReshapeCfg` /
`RewriteModule` is the structural *floor*: a `ReshapeCfg` pass can still keep a
specific CFG analysis (e.g. the dominator tree) by opting into a witnessed
incremental-repair hook (`CfgIncremental` / `FnReshape::analysis_repaired`) —
the driver marks it preserved only after watching it repair, never on the
author's say-so. See
[Type Safety: llvmkit vs. LLVM C++](docs/type-safety-vs-llvm.md#11-passes-cannot-lie-about-what-they-preserve).

Built-in analyses available today:

- `DominatorTreeAnalysis`
- `KnownBitsAnalysis`
- `DemandedBitsAnalysis`

Initial built-in transforms available today:

- `SimplifyDemandedBitsPass`
- `InstSimplifyPass`
- `DcePass`

Core pass / analysis infrastructure available today:

- `FunctionPass` / `ModulePass` (the two authoring traits)
- `#[function_pass]` / `#[module_pass]` (zero-cost authoring macros)
- `Inspect` / `PatchBody` / `ReshapeCfg` / `RewriteModule` (capability rungs)
- `run_function_pass` / `run_module_pass` (single-pass drivers)
- `function_pipeline` / `module_pipeline` / `for_each_function` (compile-time tuple pipelines)
- `DynFunctionPipeline` / `DynModulePipeline` / `DynReadOnlyFunctionPipeline` / `DynReadOnlyModulePipeline` (runtime-assembled)
- `Analyses` (the bundled function + module analysis managers)
- `PreservedAnalyses`
- `PassInstrumentationCallbacks`
- `matchers` — a `PatternMatch.h`-style combinator DSL (`m_add` / `m_c_add` / `m_one_use` / `m_all_ones` / …); matchers *return* their bindings, so a partial match is `None` rather than a half-filled slot
- `InstructionView::classify()` → exhaustive `Classified { Inst, Term }` with `CastKind` / `PhiKind` sub-enums and grammar-typed operands (`load.pointer() -> PointerValue`, `CallInst::classify_callee() -> Callee`)
- `CfgIncremental` / `FnReshape::analysis_repaired` — witnessed CFG-analysis preservation across a `ReshapeCfg` pass

### Authoring a pass

A pass is one `impl` block. Declare `type Access` (the rung), `type Requires`
(a tuple of analysis markers, prefetched before the run), and `const NAME`, then
write `fn run(cx) -> IrResult<FnReport>` (a module pass returns
`IrResult<ModReport>`). The `#[function_pass]` / `#[module_pass]` macros are
zero-cost sugar that expand to exactly that trait impl — `FnCx<Self>` /
`FnReport` in the macro form are readability sentinels the macro rewrites, so
they are not imported:

```rust
use llvmkit_ir::{function_pass, DominatorTreeAnalysis, IrResult};

struct EntryReachable;

#[function_pass(name = "entry-reachable", access = Inspect, requires = [DominatorTreeAnalysis])]
impl EntryReachable {
    fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> {
        // `requires = [..]` was prefetched, so the accessor is infallible.
        let dt = cx.analysis::<DominatorTreeAnalysis, _>();
        let entry = cx.function().entry_block().expect("definition has an entry");
        let _reachable = dt.is_reachable_from_entry(entry);
        // `Inspect` has no `cx.mutate()`; the only report it can build is
        // all-preserved, and the module stays `Verified`.
        Ok(cx.done())
    }
}
```

A mutating rung reaches its mutator through the **consuming** `cx.mutate()`;
once you call it, the context is moved, so the all-preserved `cx.done()` is
gone — a pass that touches the IR cannot then claim it preserved everything.
The mutator's own `done()` reports exactly the rung's floor:

```rust
use llvmkit_ir::{function_pass, IrResult};

struct EraseDeadInstruction;

#[function_pass(name = "erase-dead", access = PatchBody)]
impl EraseDeadInstruction {
    fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> {
        let mut patch = cx.mutate();     // consumes `cx` — no all-preserved report left
        // ... locate a dead InstructionView `dead` and: patch.erase(&dead)?;
        Ok(patch.done())                 // floor = CFG analyses preserved (PatchBody rung)
    }
}
```

The equivalent raw `impl FunctionPass for ..` form (what the macro expands to) is
shown end-to-end in `crates/llvmkit-ir/examples/pass_manager_demo.rs`, and
`crates/llvmkit-ir/examples/authored_pass.rs` runs both a macro-authored
function pass and module pass.

### The `#[function_pass]` / `#[module_pass]` macros

Both macros take the same attribute grammar and turn a plain inherent `impl`
into the raw trait impl — no registration, no boilerplate header, and **zero
runtime cost** (the expansion *is* the impl you would have written):

| Attribute | Required? | Becomes | Notes |
|---|---|---|---|
| `name = "..."` | yes | `const NAME` | the instrumentation-facing pass name |
| `access = <Rung>` | yes | `type Access` | `Inspect` / `PatchBody` / `ReshapeCfg` for `#[function_pass]`; `Inspect` / `RewriteModule` for `#[module_pass]`. A wrong-level rung fails the `FnAccess` / `ModAccess` bound at compile time |
| `requires = [A, B]` | no (default `[]`) | `type Requires = (A, B,)` | analyses prefetched before `run`, read infallibly via `cx.analysis::<A, _>()` |
| `required` | no (bare flag) | `const REQUIRED: bool = true` | marks a pass that must always run |

So this macro form:

```rust
#[function_pass(name = "entry-reachable", access = Inspect, requires = [DominatorTreeAnalysis])]
impl EntryReachable {
    fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> { /* body */ }
}
```

expands to exactly this hand-written impl — the `<'ctx, B>` header, the
associated-item block, and the `run` lifetimes are all supplied for you:

```rust
impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for EntryReachable {
    type Access = Inspect;
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = "entry-reachable";

    fn run(
        &mut self,
        cx: FnCx<'_, '_, 'ctx, B, Inspect, (DominatorTreeAnalysis,)>,
    ) -> IrResult<FnReport> { /* body */ }
}
```

Misuse is caught at the offending token, not deep inside the expansion: a
missing `name`/`access`, an unknown key, a trait impl instead of an inherent
one, or a generic impl each produce a pinpointed `compile_error!`, and a
function pass that declares a module-only rung fails with
`RewriteModule: FnAccess` unsatisfied (all locked in the compile-fail suite).
The written `FnCx<Self>` / `FnReport` are readability sentinels the macro
rewrites, so they are never imported.

### The three run modes

Every mode threads one `&mut Analyses`. The verified-state of the returned
module is *derived* — any mutating pass downgrades it to `Module<Unverified>`,
forcing an explicit re-`verify()` before the next verified-only stage (D8):

```rust
use llvmkit_ir::{
    Analyses, Brand, DcePass, FunctionView, InstSimplifyPass, IrResult, Module, Unverified,
    Verified, function_pipeline, run_function_pass,
};

fn cleanup<'ctx>(
    verified: Module<'ctx, Brand<'ctx>, Verified>,
    f: FunctionView<'ctx>,
) -> IrResult<()> {
    let mut analyses = Analyses::new();

    // 1. A single pass. `InstSimplifyPass` is `PatchBody`, so the driver
    //    returns `Module<Unverified>` and the re-verify is enforced by the type.
    let simplified: Module<'_, _, Unverified> =
        run_function_pass(InstSimplifyPass, verified, f, &mut analyses)?;

    // 2. A compile-time tuple pipeline, run in written order. The output
    //    typestate folds the members' rungs: any mutator ⇒ Unverified.
    let cleaned = function_pipeline((InstSimplifyPass, DcePass))
        .run(simplified.verify()?, f, &mut analyses)?;
    let _reverified = cleaned.verify()?;
    Ok(())
}
```

The third mode is runtime assembly, for opt-style CLIs where the pass list is
not known until run time: `DynFunctionPipeline` / `DynModulePipeline` (transform;
always `Unverified` out) and `DynReadOnlyFunctionPipeline` /
`DynReadOnlyModulePipeline` (read-only; `push` is bounded to `Inspect`, so a
mutating pass *cannot* be added and the module threads through `Verified`).

For runnable end-to-end versions, see
`crates/llvmkit-ir/examples/pass_manager_demo.rs` and
`crates/llvmkit-ir/examples/authored_pass.rs`.

| LLVM new PM concept | llvmkit API |
|---|---|
| `FunctionPass::run(Function &, FunctionAnalysisManager &)` | `FunctionPass::run(cx: FnCx<..>)` — one consuming context |
| `ModulePass::run(Module &, ModuleAnalysisManager &)` | `ModulePass::run(cx: ModCx<..>)` |
| `PreservedAnalyses::all()` / `none()` hand-written by the pass | derived from the pass's `type Access` rung — never hand-written |
| `FAM.getResult<A>(F)` (fallible, null on undeclared) | `cx.analysis::<A, _>()` — infallible; declared in `type Requires`, prefetched |
| `ModuleToFunctionPassAdaptor` | `for_each_function(function_pipeline((..)))` as a module-pipeline member |
| mutating IR in a pass | declare a mutating rung, call the consuming `cx.mutate()`, receive a mutator; the driver returns `Module<'ctx, B, Unverified>` |
| plugin registration (`llvmGetPassPluginInfo`) | none — a pass is a plain value; no registration step |

Important boundary: the crate currently ships **the capability-graded pass API,
built-in analyses, initial scalar cleanup transforms (`SimplifyDemandedBitsPass`,
`InstSimplifyPass`, `DcePass`), optimization-level markers, scoped pass /
pipeline names, and data-only pass-pipeline recipe types**, not a full
optimization pipeline. There is no public LLVM-compatible `PassBuilder`, no
runnable `default<O1>` optimizer, no loop / CGSCC / legacy manager, no
instrumentation-driven skipping, and no broad transform library yet. See
[`docs/future-work.md`](docs/future-work.md) (the "Pass API — deferred"
section) for the scoped-out items.

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

Every Rust file that ports LLVM behavior pairs to a specific upstream LLVM
concept. See [AGENTS.md](AGENTS.md) for the detailed source-tree map and the
current port-status ledger, and [UPSTREAM.md](UPSTREAM.md) for the per-test and
fixture provenance registry. The in-tree fixture policy avoids generated stubs,
and the test and runtime paths do not depend on `orig_cpp`.

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
See [Type Safety: llvmkit vs. LLVM C++](docs/type-safety-vs-llvm.md) for worked
examples that map common LLVM C++ failure modes to Doctrine IDs and compile-fail
locks.

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
- **D7. Cross-module mixing is rejected.** Public construction and mutation
  APIs carry a generative module brand, so values from one `Module::with_new`
  closure cannot be passed to another module's builders or mutators. This is a
  type error, not a runtime same-module check.
- **D8. Verified guarantees are explicit.** Verification consumes an
  unverified token and produces `Module<'ctx, B, Verified>`. A pass pipeline's
  output typestate is *derived* from its members' capability rungs: an
  all-read-only (`Inspect`) run preserves that verified state at the type level,
  while any mutating pass returns `Module<'ctx, B, Unverified>`, so their output
  must be verified again before another verified-only pipeline can consume it.
- **D9. Iteration safety is structural.** Mutating-while-iterating uses
  dedicated cursor APIs rather than relying on caller discipline.
- **D10. No undefined behavior, by design.** Legal API calls must produce
  defined IR behavior; deferred traps and invalid combinations surface as typed
  errors or explicit IR states, not silent UB.
- **D11. Tests and fixtures are ported, not invented.** Every `#[test]` in the
  workspace is traced in [UPSTREAM.md](UPSTREAM.md) to an upstream unit test,
  verifier fixture, assembler fixture, or explicitly-labeled example lock; the
  fixture and runtime paths do not depend on `orig_cpp`.

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
