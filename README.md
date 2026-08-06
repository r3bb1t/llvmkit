# llvmkit

[![crates.io](https://img.shields.io/crates/v/llvmkit.svg)](https://crates.io/crates/llvmkit)
[![docs.rs](https://docs.rs/llvmkit/badge.svg)](https://docs.rs/llvmkit)
[![License](https://img.shields.io/crates/l/llvmkit.svg)](https://github.com/r3bb1t/llvmkit#license)

A from-scratch Rust reimplementation of [LLVM](https://llvm.org/) IR APIs.
Today `llvmkit` can lex, parse, build, verify, analyze, and print LLVM IR
without linking against `libLLVM`; bitcode support is still ahead.

## Status

Tracking **LLVM 22.1.4** (`llvmorg-22.1.4`, released 2026-04-21).

> **The crates.io badge above shows 0.0.3, which is the last published
> release. The list below describes 0.0.4, which is unreleased** — so
> `cargo add llvmkit` today gets the older closure-scoped API
> (`Module::with_new`), not the owned modules and storable ids described here.
> Track `master` for the 0.0.4 surface. The project is pre-1.0 and, under
> Cargo's pre-1.0 rules, every `0.0.x` is mutually incompatible; see
> [ROADMAP.md](ROADMAP.md) for the release sequence.

Shipped today:

- **Owned modules and storable ids** — the 0.0.4 handle model. `Module<B, S>`
  has no lifetime parameter, owns its storage, and is `Send`, so it can be
  returned, stored in a struct, collected into a `Vec`, and moved between
  threads. Declarations and value-producing builder calls return a
  `Copy + Send` **id** (`IntValueId<W, B>`, `FunctionId<R, B>`, `GlobalId<B>`,
  …) that carries the module's identity without borrowing it. Blocks are
  minted as linear, `!Copy` handles instead (`append_basic_block` returns a
  `BasicBlock<..>`, `append_block_with_params` a `BlockWithParams<..>`);
  calling `.id()` on one gives the storable `BlockId<R, B, Params>`.
  Terminator builders (`br`, `cond_br`, `ret`, …) consume
  the builder and hand back the terminated block alongside the new
  instruction, not an id. The borrowing handles themselves are minted per
  operation from `m.view(id)` / `m.try_view(id)`. A module's identity is the
  `B: ModuleBrand` *type*, in
  three rungs — `module_new!` (unnameable, generated at the expansion site),
  `Module::branded::<B>` (a named brand, at most one live module per brand),
  and `Module::dynamic` (`DynBrand`, unlimited live modules, separated by the
  runtime tag). See [Same-module safety](#same-module-safety).
- **`.ll` lexer** — done. `llvmkit-asmparser` ports
  `llvm/lib/AsmParser/LLLexer.cpp` and borrows directly from the source slice,
  allocating only when escape decoding actually changes bytes.
- **`.ll` parser** — parses ordinary compiler output. Parses module-level
  directives (target datalayout/triple, module asm, type definitions, globals,
  function declarations and definitions), all instruction opcodes, metadata
  (standalone numbered nodes, named metadata, instruction trailing attachments),
  and value forms (integer/float literals, undef, poison, null,
  zeroinitializer, global/function references, and represented `ConstantExpr`
  forms for parser-needed opcodes, including upstream vector GEP, bitcast, cast,
  and select folding fixtures). Round-trip tested via `format!("{module}")`.
  Attribute coverage spans the function, parameter, and return attributes real
  compiler output uses — the typed `byval(T)` / `sret(T)` family, `uwtable`'s
  kind grammar, both `dereferenceable` forms — plus `dso_local` on every global
  object and `c"..."` string constants. **`clang -O0` and `-O2` output parses,
  verifies, and round-trips**, asserted on whole programs in
  `tests/parser_attribute_matrix.rs`. A companion guard parses the vendored
  `Attributes.td` and fails CI if an upstream attribute is neither accepted nor
  listed as deliberately unmodeled, so the keyword table cannot silently drift
  from LLVM again. Not yet modeled: bitcode, and the 42 attributes named in
  that guard's `NOT_YET_MODELED` list.
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
- **KnownBits — complete.** `KnownBits.h`'s public surface is fully modeled,
  compiler-verified: the parity ledger
  (`crates/llvmkit-ir/tests/value_tracking_parity.rs`) asserts an *empty* gap
  list, so a regression or a newly-synced upstream method has to be
  acknowledged rather than absorbed.
- **ValueTracking — 93 of 101 entry points**, tracked symbol-by-symbol in the
  same ledger, which asserts that modeled plus gaps equals the audited surface
  so a symbol cannot be silently neither. Beyond `compute_known_bits` itself
  (with `KnownBitsAnalysis`, `ValueTrackingQuery`, recursion budgeting,
  dominator-tree hooks and a reusable per-analysis cache) this covers the
  select-pattern vocabulary and matching (`select_pattern.rs`), pointer and
  object analysis (`pointer_analysis.rs`), speculation safety and UB
  reachability (`speculation.rs`), `@llvm.assume` with its dominating-condition
  cache (`assumptions.rs`, `implied_conditions.rs`), and floating-point
  classification — the `FPClassTest` / `KnownFPClass` lattice (`fp_class.rs`),
  `computeKnownFPClass` and its predicates (`known_fp_class.rs`), and
  `fcmpImpliesClass` (`fp_predicate.rs`). `computeKnownFPClass`'s opcode
  dispatch is deliberately partial; its module header names every arm that is
  not yet consulted, and an unconsulted arm only ever weakens an answer.
  The remaining eight gaps each carry a recorded reason —
  see [`docs/future-work.md`](docs/future-work.md).
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
- **Full ValueTracking / DemandedBits / SimplifyDemandedBits parity** — the
  ledger is closed for `KnownBits.h` and open for the eight remaining
  `ValueTracking.h` entry points, some `ValueTracking.cpp` operator arms
  (notably `computeKnownFPClass`'s dispatch), demanded-bit rules, and
  `InstCombineSimplifyDemanded` transforms.
- **Additional or currently unrepresented `llvm.*` intrinsic IDs, signatures,
  and facts** — new IDs and verifier signatures must land before analysis facts
  are added.
- **Full built-in optimization transform library and pipeline builders**
  (`PassBuilder`, loop PM, CGSCC PM, legacy PM, *runnable* textual pipelines —
  `pass_pipeline.rs` parses a pipeline string into typed, data-only recipe
  values, but there is no NAME → constructor registry, so nothing can run one)

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
use llvmkit_ir::{IrBuilder, IrError, Linkage, module_new};

fn build() -> Result<(), IrError> {
    let m = module_new!("demo")?;
    let f = m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let b = IrBuilder::at_end(entry);
    let (lhs, rhs) = m.view(f).params();
    let sum = b.int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
    b.ret(sum)?;

    print!("{m}");
    Ok(())
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

`IrBuilder::call` takes a `TypedFunctionValue` callee and an argument
tuple typed against its parameter schema. Wrong arity, a wrong-typed argument,
or misusing a void call's result are all compile errors instead of runtime
`IrError`s or verifier failures, and the result narrows to the callee's real
return type with no `try_into`:

```rust
use llvmkit_ir::{IrBuilder, IrError, Linkage, module_new};

fn build_typed_call() -> Result<(), IrError> {
    let m = module_new!("demo")?;
    let callee = m.add_typed_function::<i32, (i32, i32), _>("add_inner", Linkage::External)?;
    let entry = m.view(callee).append_basic_block(&m, "entry");
    let b = IrBuilder::at_end(entry);
    let (lhs, rhs) = m.view(callee).params();
    let sum = b.int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
    b.ret(sum)?;

    let caller = m.add_typed_function::<i32, (i32, i32), _>("caller", Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let b = IrBuilder::at_end(entry);
    let (x, y) = m.view(caller).params();

    // `call` hands back a storable `TypedCallInstId`; `b.view(..)` reaches
    // the handle, whose `result()` is already `IntValue<i32>` -- no `try_into`.
    let call = b.call(m.view(callee), (x, y), "r")?;
    b.ret(b.view(call).result())?;

    print!("{m}");
    Ok(())
}
```

A callee whose signature is only known at runtime (parsed IR, an `extern`
declaration built from user input) keeps using the `_dyn` counterparts —
`call_dyn`, `indirect_call_dyn`, `invoke_dyn` — which take a
plain `FunctionValue` and an iterable of pre-widened `Value`s, and reject a
wrong argument count or type with `IrError::CallArgumentCountMismatch` /
`CallArgumentTypeMismatch` at build time rather than deferring to the verifier.
`indirect_call::<Sig>` derives the callee's function type from a Rust
function-pointer schema `Sig` instead of taking it by hand; `varargs_call`
lowers a fixed, schema-typed prefix the same way `call` does and appends
an erased `...` tail, matching LLVM's own no-static-check contract on variadic
arguments.

Derived struct schemas let you derive the schema on a plain Rust struct, use the
generated `<Struct>Value<'ctx, B>` wrapper in IR, and call field
accessors/builders instead of indexing aggregates manually:

```rust
use llvmkit_ir::{IrBuilder, IrStruct, Linkage, module_new};

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

let m = module_new!("window")?;
let f = m.add_typed_function_of::<Normalize, _>("normalize", Linkage::External)?;
let entry = m.view(f).append_basic_block(&m, "entry");
let b = IrBuilder::new_for_return::<Normalize>(&m).position_at_end(entry);
let (placement,) = m.view(f).params();
// `normal_position` returns `RectValue<'ctx, B>`, and `min` returns
// `PointValue<'ctx, B>`; nested structs keep their generated wrapper type.
let rect = placement.normal_position(&b)?;
let _min = rect.min(&b)?;
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
powerful as passing a type to `load` today; a mis-assertion produces
wrong IR that the verifier catches, never memory-unsafe behavior).
`typed_alloca::<T>`, `typed_load`, and `typed_store` skip
the runtime type-narrowing that the erased path needs, and
`field_gep::<S, I>` projects the field type at compile time straight
from a `#[derive(IrStruct)]` schema -- an out-of-range field index is a
missing trait impl, not a runtime bounds check.

### Typed vectors and arrays

`VectorValue<'ctx, E, L, B>` and `ArrayValue<'ctx, E, L, B>` carry the element
type (`E` -- a scalar marker like `i64`/`f64`) and the length (`L` -- `Len<N>`
for vectors, `ArrLen<N>` for arrays) in the type system, so a `<N x T>` /
`[N x T]` length mismatch or a wrong-element `insertelement` / `insertvalue` is
a compile error rather than a `verify()` diagnostic -- the vector/array analog
of `IntValue<'ctx, W>`. The bare `VectorValue<'ctx>` / `ArrayValue<'ctx>` is the
fully-erased (`Dyn`) form that parsed IR, scalable vectors, and runtime lengths
land in; it narrows to the typed form with `TryFrom`, which checks both element
and length.

```rust
use llvmkit_ir::{IrBuilder, IrError, Len, Linkage, VectorValue, module_new};

fn typed_vec() -> Result<(), IrError> {
    let m = module_new!("demo")?;
    let v4i32 = m.vector_type_n::<i32, 4>(); // VectorType<'_, i32, Len<4>>
    let fn_ty = m.function_type(m.i32_type().as_type(), [v4i32.as_type(), v4i32.as_type()]);
    let f = m.add_function_dyn("vadd", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::at_end(entry);

    // `try_into` checks element (i32) AND lane count (4) before stamping the markers.
    let a: VectorValue<'_, i32, Len<4>, _> =
        m.view(f).param(0).unwrap().as_erased().try_into().unwrap();
    let c: VectorValue<'_, i32, Len<4>, _> =
        m.view(f).param(1).unwrap().as_erased().try_into().unwrap();

    // Both operands are pinned to `<4 x i32>`; a length/element mismatch would not compile.
    let sum = b.vector_int_add(a, c, "sum")?;
    // Extract returns the element as its typed scalar handle -- `IntValue<i32>`, inferred.
    let lane0 = b.vector_extract(sum, m.i32_type().const_int(0_i32), "lane0")?;
    b.ret(lane0)?;
    Ok(())
}
```

The full runnable version (vectors and arrays) is
`crates/llvmkit-ir/examples/typed_vector_array.rs`.

### Auto-SSA: typed local variables instead of manual phi wiring

`SsaBuilder` (`crates/llvmkit-ir/src/ssa_builder.rs`) sits on top of the
typed `IrBuilder` and implements Braun et al.'s 2013 on-the-fly SSA
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
// Explicit phis, via block parameters (examples/factorial.rs). A loop header's
// parameters ARE its head-phis: you declare them on the block, and each branch
// into it carries its incomings as block arguments. There is no phi to
// pre-declare and no incoming-edge list to patch after the fact -- an edge that
// forgets an argument, or carries the wrong type or arity, does not compile.
let (loop_bb, params) = bwp.append_block_with_named_params(
    m.view(f).as_function(),
    &[(i32_ty.as_type(), "acc"), (i32_ty.as_type(), "i")],
    "loop",
)?;
let loop_label = loop_bb.id();       // storable `BlockId` — the branch currency

// entry: enter the loop carrying the initial values `[ acc = 1, i = %n ]`.
b.cond_br_with_args(is_zero, base_label, &[], loop_label,
    &[i32_ty.const_int(1_i32).as_erased(), n.as_erased()])?;

// loop: read the header params, compute, re-enter carrying the back-edge values.
let acc: IntValue<'_, i32, _> = params[0].try_into()?;
let i: IntValue<'_, i32, _> = params[1].try_into()?;
let next_acc = b.int_mul(acc, i, "next_acc")?;
let next_i = b.int_sub(i, 1_i32, "next_i")?;
b.cond_br_with_args(done, exit_label, &[], loop_label,
    &[m.view(next_acc).as_erased(), m.view(next_i).as_erased()])?;
```

(The raw `int_phi` / `add_incoming` pair that predates block parameters is
`pub(crate)` and cannot be called from outside the crate — that is deliberate,
and a compile-fail fixture pins it. Block parameters and `SsaBuilder` are the
two public ways to author a phi; `FnReshape::insert_phi` is the third, for
passes editing existing IR.)

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
let next_acc = b.ins()?.int_mul(acc, i, "next_acc")?;
let next_i = b.ins()?.int_sub(i, 1_i32, "next_i")?;
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

Migrating an existing inkwell codebase? [docs/inkwell-migration.md](docs/inkwell-migration.md)
is a side-by-side guide: the API mapping table, the three structural differences
to read first (no `Context` lifetime, owned modules, ids rather than handles),
and the ledger of what each migration buys you at compile time.

### Where llvmkit improves on upstream LLVM

`llvmkit` models a subset of LLVM and stops at IR construction, analysis, and
verification (see "Out of scope"). Within that subset there are three places
where its API makes a guarantee upstream's cannot, and they are worth stating
concretely rather than as a slogan.

**1. A module is an owned value, and every handle has a storable id.**
`Module<B, S>` owns its storage, has no lifetime parameter, and is `Send`, so it
can be returned from a function, held in a struct field, collected into a `Vec`,
and moved to another thread. Handles like `IntValue<'ctx, W, B>` borrow the
module, but each one also has an `id()` — `IntValueId<W, B>`, `BlockId<..>`,
`FunctionId<R, B>`, `GlobalId<B>` — that is `Copy + Send` and carries the brand
*without* the borrow. That is what lets a binary lifter keep its own
`HashMap<u64, BlockId<..>>` from guest address to block, suspend in the middle
of a function, move to a worker thread, and resume there
(`crates/llvmkit-ir/examples/lifter_session.rs` does exactly this).

Upstream's equivalent of a stored id is a raw `Value *` / `BasicBlock *`, and
keeping it valid is the client's job. LLVM ships `WeakVH`, `AssertingVH`, and
`CallbackVH` (`llvm/include/llvm/IR/ValueHandle.h`) specifically for
"catching dangling pointer bugs", and the Programmers Manual warns that the
weak form can still leave a dangling pointer. An llvmkit id is not a pointer:
resolving it re-checks the module tag and the arena slot, so a stale or foreign
id becomes `IrError::ForeignValueId`, `None`, or a panic — never a read of freed
memory. With `#![forbid(unsafe_code)]` on every workspace crate there is no
unsafe path available for it to take.

**2. Several error classes are unrepresentable rather than diagnosed.** An
integer width is a type parameter (`IntValue<'ctx, i32, B>`), a vector's element
and lane count are type parameters (`VectorValue<'ctx, i32, Len<4>, B>`), a
function's signature is a type parameter, and the owning module is a brand type.
So a mismatched `add`, a `<4 x i32>` mixed with a `<8 x i32>`, a call with the
wrong arity, a `ret` in a `void` function, and an operand borrowed from another
module are all *compile* errors — there is no program text that expresses them
and no runtime check to reach. Upstream accepts each of these as `Value *` and
reports them from `Verifier.cpp`, later, if verification runs at all. The
mapping from each upstream verifier message to the llvmkit type that forecloses
it is tabulated in [Type Safety: llvmkit vs. LLVM C++](docs/type-safety-vs-llvm.md),
and 86 compile-fail fixtures lock the guarantees.

**3. Verification is a typestate, not a function you must remember to call.**
`Module::verify(self)` consumes `Module<B, Unverified>` and returns
`Module<B, Verified>`; APIs that are only sound on verified IR demand the
`Verified` token, and any mutating pass *derives* `Module<B, Unverified>` from
its capability rung, so the re-verify is enforced by the type checker rather
than by a convention. Upstream's `verifyModule` is a free function returning a
bool sentinel that a caller can simply not call — and its pass-manager form
reports preservation through a hand-written `PreservedAnalyses`, where
over-claiming leaves stale analyses for a later pass to miscompile against. In
llvmkit that claim is derived from the rung and is unspellable.

The honest limits: these guarantees cover the *modeled* surface only, the
erased `Dyn` forms deliberately trade them back for runtime checks so parsed and
dynamic IR still works, and none of it helps if you need codegen — upstream is
the only option there.

### Bindings

Python and Java bindings are planned. They have not been written yet for one
reason: the API was not stable enough to wrap. Wrapping a moving surface means
rewriting the wrapper on every break, which is the failure mode
[llvmlite](https://github.com/numba/llvmlite) describes chasing upstream LLVM's
unstable C++ API. 0.0.4 is where that surface stops moving week to week — not
a stability promise (the crate is pre-1.0 and every `0.0.x` is mutually
incompatible), but settled enough that a wrapper is worth writing.

Keeping the surface *wrappable* has been a standing constraint on every API
decision along the way, which is why the shape below fell out rather than
having to be retrofitted:

- **Nothing is reachable only from inside a closure.** `Module::branded::<B>`,
  `Module::dynamic`, and `module_new!` all return an owned module. A binding's
  `Module.__init__` can call one and store the result; there is no
  `with_new(|m| ...)` scope for a foreign call stack to sit inside.
- **No lifetime appears in a storable type.** Every id is `Copy + Send` and
  `'static`; a wrapper object can hold one in a field for as long as it likes.
  The borrowing views (`IntValue<'ctx, ..>`) are minted per operation from
  `m.view(id)` and never need to cross the boundary.
- **`DynBrand` is the rung a binding uses.** A dynamic language has no place to
  put a brand type, and `Module::dynamic` asks for none: it is exempt from the
  uniqueness registry, so many live modules are legal, and separation falls
  back to the runtime `ModuleId` tag with `IrError::ForeignValueId` as the
  verdict — an error a wrapper can raise as an exception, not UB.
- **Misuse of a *handle or id* is an `IrError` or a deterministic panic, never
  a dangling read.** `#![forbid(unsafe_code)]` holds across the workspace, so
  the worst a forged or stale handle can do is get rejected. That holds without
  exception, metadata included: a metadata node is named by a `MetadataId<B>`
  carrying the owning module's `ModuleId`, and a foreign one is
  `IrError::ForeignMetadataId` at the arena boundary.

What a wrapper will still build itself: an id table. Ids are opaque — their
`(ModuleId, slot)` payload is private, and there is deliberately no
`from_raw_parts` — so a binding keeps its own `Vec`/`HashMap` of live ids and
hands the host language an index into it, which is what
[wgpu](https://github.com/gfx-rs/wgpu)'s and MLIR's C APIs do anyway.

### Same-module safety

A module's identity is a **type**. `Module<B, S>` is an owned, `Send`,
lifetime-free value; the `B: ModuleBrand` parameter rides on every handle, id,
and builder minted from it, and it is what separates one module from another.
Normal code never names the brand — values, constants, basic blocks, globals,
and builders infer it from the `Module` or type receiver they came from. Generic
extension code names `B: ModuleBrand` explicitly when it must accept any module.

There are three ways to obtain a brand, trading ergonomics against how much of
the separation is static:

| Constructor | Brand | Separation |
|---|---|---|
| `module_new!("name")` | a fresh, **unnameable** type per expansion site | compile-time |
| `Module::branded::<MyBrand, _>("name")` | a `'static` type you declare and can name | compile-time |
| `Module::dynamic("name")` | `DynBrand`, shared by every such module | run-time only |

`module_new!` is the default: it declares a brand inside its own block scope, so
no two expansion sites can ever collide and nothing outside can name the type.
Name a brand yourself when a module must appear in a struct field, a function
signature, or a return type — somewhere the type has to be written down.
`Module::dynamic` is for a module *count* that is a run-time decision (a loop
over translation units, a worker pool, a `Vec<Module<DynBrand>>`), where no
single static type could name each module individually.

**What is compile-time.** A process-global registry admits at most one live
`Module` per brand type, so a brand names exactly one module; a second claim is
`IrError::BrandInUse`, and `Module::branded_once` retires its brand permanently
on drop (`IrError::BrandRetired`) so a stale `'static` id can never be replayed
against fresh storage. Because of that lock, two **distinct** brand types are
two distinct modules, and handing a handle or id from one to the other's
builders, mutators, or resolvers is a type error — no runtime check is involved,
and there is no `IrError` variant for it to return. `DynBrand` is exempt from
the registry, which is precisely why it buys no compile-time separation.

**What is run-time.** Ids are storable: they carry the brand but not the borrow,
so they outlive the handle they came from — and therefore they also carry a
`ModuleId` tag, checked whenever an id is resolved back to a handle. That tag is
the backstop for the two cases the type system cannot see: two `DynBrand`
modules (the same type by construction), and a named brand re-issued to a fresh
module after the previous one dropped. Neither can become a silent miscompile.
Which form the rejection takes depends on the surface: the fallible id-taking
APIs — builders, pass mutators, `try_from_id`-style resolvers — return
`IrError::ForeignValueId`; `Module::try_view(id)` returns `None`; and
`Module::view(id)` *panics*, treating a foreign id as a deterministic contract
violation in the same way indexing a slice out of bounds is.

Compile-time separation is the guarantee llvmkit leads with; the runtime tag is
what keeps the deliberately-erased case sound instead of undefined.

### Instruction lifecycle safety

`Instruction<'ctx, state::Attached, B>` is the lifecycle authority for erase,
detach, move, and RAUW operations. Those methods consume the handle, so a used
lifecycle capability cannot be reused. Copyable discovery APIs return
`InstructionView` instead: blocks, value use-lists, and per-opcode handles expose
read-only inspection without minting a new mutation handle. Cursor-driven
mutation uses `BlockCursor::step` on an unterminated block.

Run the examples:

```bash
# Lex a file from disk
cargo run -p llvmkit-asmparser --example lex_file -- crates/llvmkit-asmparser/examples/demo.ll

# Build and print IR programmatically
cargo run -p llvmkit-ir --example build_add_function
cargo run -p llvmkit-ir --example cpu_state_add
cargo run -p llvmkit-ir --example factorial
cargo run -p llvmkit-ir --example factorial_auto_ssa
cargo run -p llvmkit-ir --example concurrent_counter
cargo run -p llvmkit-ir --example derived_struct_function

# Typed vectors and arrays: length/element mismatches become compile errors
cargo run -p llvmkit-ir --example typed_vector_array

# Build IR, run a built-in analysis, and drive custom passes
cargo run -p llvmkit-ir --example pass_manager_demo
cargo run -p llvmkit-ir --example authored_pass
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
        // ... locate a dead instruction, then narrow it to a `NonTerminator`:
        //     if let Some(dead) = view.as_non_terminator() { patch.erase(&dead); }
        // `erase` accepts *only* a `NonTerminator`, so erasing a terminator —
        // which would break this rung's CFG-preserved floor — is a compile
        // error, not a runtime rejection. It is infallible: no `?`.
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

expands to exactly this hand-written impl — the `<B>` header, the
associated-item block, and `run`'s higher-ranked `'m` / `'ctx` regions and their
where-clause are all supplied for you:

```rust
impl<B: ModuleBrand> FunctionPass<B> for EntryReachable {
    type Access = Inspect;
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = "entry-reachable";

    fn run<'m, 'ctx>(
        &mut self,
        cx: FnCx<'m, '_, 'ctx, B, Inspect, (DominatorTreeAnalysis,)>,
    ) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    { /* body */ }
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
    Analyses, DcePass, Dyn, FunctionId, InstSimplifyPass, IrResult, Module, ModuleBrand,
    Unverified, Verified, function_pipeline, run_function_pass,
};

fn cleanup<'ctx, B: ModuleBrand + 'ctx>(
    verified: Module<B, Verified>,
    f: FunctionId<Dyn, B>,
) -> IrResult<()> {
    let mut analyses = Analyses::new();

    // 1. A single pass. `InstSimplifyPass` is `PatchBody`, so the driver
    //    returns `Module<Unverified>` and the re-verify is enforced by the type.
    //    The driver is handed an **id**, never a view: it consumes the module
    //    token, and a view would be a borrow of the token about to move.
    let simplified: Module<B, Unverified> =
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
| mutating IR in a pass | declare a mutating rung, call the consuming `cx.mutate()`, receive a mutator; the driver returns `Module<B, Unverified>` |
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
├── docs/                            # see docs/README.md for the index
│   ├── type-safety-vs-llvm.md       #   current: the main technical reference
│   ├── ir-struct-derive.md          #   current: IrStruct user guide
│   ├── future-work.md               #   current: the live backlog
│   └── design/                      #   dated records of shipped subsystems
└── crates/
    ├── llvmkit-support/             # Span, Spanned<T>, SourceMap
    ├── llvmkit-asmparser/           # Lexer + .ll parser
    ├── llvmkit-macros/              # IrStruct derive, #[function_pass]/#[module_pass]
    ├── llvmkit-tablegen/            # TableGen front end + intrinsic emitter
    └── llvmkit-ir/                  # Typed IR model, builder, verifier, passes
```

`docs/` sits at the workspace root, so it is not part of the published `.crate`
and does not appear on docs.rs — it is a repository-facing tree. API
documentation ships as rustdoc.

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
- **D7. Cross-module mixing is rejected.** Every handle, id, and builder carries
  the owning module's brand — a `'static` *type* parameter `B`. Two modules with
  **distinct** brand types cannot exchange operands: that is a type error, caught
  at compile time, with no runtime check involved. A process-global registry
  admits at most one live `Module` per brand (`IrError::BrandInUse` /
  `BrandRetired`), which is what makes a brand name *one* module unambiguously.
  Where two modules deliberately **share** a brand type — every
  `Module::dynamic` module is `DynBrand`, and a named brand is re-issued after
  the previous module drops — the compile-time half cannot apply, and a mix-up
  is instead caught at the arena boundary by the runtime `ModuleId` tag every id
  carries (`IrError::ForeignValueId`). Compile-time separation is the guarantee;
  the runtime tag is the backstop that keeps the erased case sound rather than
  silently miscompiling. See [Same-module safety](#same-module-safety).
  **Metadata is included.** It used to be the one currency outside D7 — a bare
  arena index with neither a brand nor a tag, so an in-range node from another
  module mis-resolved silently. Since 0.0.4 the public currency is
  `MetadataId<B>`, which carries both halves like every other id: a mix-up
  across named brands is a compile error, and within one brand it is
  `IrError::ForeignMetadataId`. `IrError::UnknownMetadataSlot` now reports only
  a *native* id whose slot is out of range.
- **D8. Verified guarantees are explicit.** Verification consumes an
  unverified token and produces `Module<B, Verified>`. A pass pipeline's
  output typestate is *derived* from its members' capability rungs: an
  all-read-only (`Inspect`) run preserves that verified state at the type level,
  while any mutating pass returns `Module<B, Unverified>`, so their output
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
- [Writing an LLVM New PM Pass](https://releases.llvm.org/22.1.0/docs/WritingAnLLVMNewPMPass.html)
- [LLVM 22.1.4 release](https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.4)

## License

This project is a derivative work of the [LLVM Project](https://llvm.org/) and
is licensed under the [Apache License v2.0 with LLVM Exceptions](LICENSE)
(`Apache-2.0 WITH LLVM-exception`) — the same license LLVM ships under.
