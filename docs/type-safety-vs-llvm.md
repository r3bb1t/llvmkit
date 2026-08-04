# Type Safety: llvmkit vs. LLVM C++

`llvmkit` tracks LLVM's IR semantics, printer forms, verifier rules, and source
layout. The deliberate difference is the public API shape: where upstream LLVM C++
often lets callers build malformed IR and asks a later verifier pass to reject it,
`llvmkit` pushes many local, statically knowable invariants into Rust types.

This is not a claim that LLVM C++ is poorly designed. LLVM is a mature C++
compiler infrastructure optimized around pointer identity, intrusive lists,
mutation-heavy passes, and late verification. `llvmkit` has a different advantage:
its API can use Rust ownership, typestate, sealed traits, and per-module brand
types to make many invalid states unspellable.

## The short version

The Doctrine column references the D1-D11 rules in the README. This page focuses
on user-visible API failure modes; D11's test-provenance rule is tracked in
`UPSTREAM.md` rather than repeated here.

| Problem shape | Doctrine anchor | Upstream LLVM C++ | llvmkit |
| --- | --- | --- | --- |
| Value from another module used as an operand | D7 | Builder accepts `Value *`; verifier later reports `"Referencing ... in another module!"` | Operand type carries the owning module's brand type; wrong module is a compile error |
| Branch to a block from another module | D7 | Builder accepts `BasicBlock *`; verifier later rejects malformed control flow | Branch target carries the builder module's brand |
| Global initializer expression tied to another module | D7 | Constructor accepts `Constant *`; type is asserted, module provenance is not statically represented | `add_global` requires an `IntoConstantValue<'ctx, B>` initializer with the same `B` (the global's value type is derived from it) |
| Custom folder returns a value from the wrong module | D7 | Folder hooks return raw `Value *` | Folder hooks return `IrResult<Option<Value<'ctx, B>>>` |
| Custom folder returns a wrong-*width* typed fold result | D4 | Folder hooks return raw `Value *`; the builder inserts whatever comes back, and the mistyped constant surfaces later as malformed IR | The typed hook's signature pins the return type, so an external folder cannot spell a **concrete** wrong width (`E0308`, locked by `tests/compile_fail/folder_typed_wrong_width.rs`). Where a signature *cannot* pin it — `W = IntDyn` names no width, and the crate-internal `from_value_unchecked` can mint any marker — the builder re-checks each fold result's runtime type against the operand / cast destination, for **every** marker (see §4; the check was previously skipped for static markers, which was circular) |
| Insert after a terminated block | D1 | Insertion point is a mutable iterator into a `BasicBlock *` | Terminator builders consume the builder (`self` by value) and return a `Terminated` view. `BasicBlock` is `!Copy`, so `position_at_end` *moves* it: a retained `Unterminated` handle cannot be re-positioned into either (`E0382`, locked by `tests/compile_fail/retained_unterminated_block_cannot_reposition.rs` and `builder_cannot_terminate_twice.rs`) |
| Mutate instruction metadata on a module already verified, or from a read-only pass | D8, D2 | `Instruction::setMetadata` is a plain non-const method; `verifyModule` is a free function returning a `bool` the caller may ignore, and nothing connects the two | `InstructionView::set_metadata` / `push_debug_record` (and their `Instruction` twins) demand a `&Module<B, Unverified>` token, which a `Verified` module cannot supply and an `Inspect`-rung pass never holds (`E0308`, locked by `tests/compile_fail/verified_module_metadata_is_immutable.rs`) |
| Metadata node from another module attached, referenced, or looked up | D7 | `MDNode *` records no owning module; the attachment is accepted and surfaces (if at all) as a corrupt dump | The metadata currency is `MetadataId<B>` — brand statically, `ModuleId` tag at run time. Wrong brand is a compile error (`tests/compile_fail/cross_module_metadata_attachment.rs`); wrong module under one brand is `IrError::ForeignMetadataId` at a single arena choke point (`tests/module_ownership.rs`) |
| Keep a handle to a block or value after its module is gone | D2, D7 | A `BasicBlock *` outliving its `Module` is a dangling pointer, diagnosed at no stage | A module is an owned value, so every borrowing handle (`BasicBlock`, `FunctionValue`, `Value`, every `*View`) carries a `'ctx` borrow of it and cannot escape its scope (`E0597`, locked by `tests/compile_fail/view_cannot_outlive_its_module.rs`). The `.id()` form of the same program compiles — which is why a stale *id* is a run-time rejection while a stale *view* is unconstructible |
| Return a value from a `void` function, or `ret void` from a value-returning function | D1, D4 | `CreateRet(Value *)` / `CreateRetVoid()` are just methods; mismatch is verifier/runtime state | `IRBuilder<..., R>` exposes return methods according to the function return marker |
| Read a typed result from a `void` call | D3, D4 | Caller must inspect the call/function type | `CallInst<'ctx, ()>` exposes no typed result accessor |
| Use an instruction handle after erase | D2 | Raw pointer discipline | Lifecycle methods consume a non-`Copy`, non-`Clone` `Instruction` handle |
| Recover lifecycle authority from a copyable value, block, or use-list | D2, D9 | Any retained `Instruction *` can be reused for mutation | Copyable rediscovery APIs return `InstructionView`; only builder output, `BlockCursor`, and detached reinsertion produce `Instruction<Attached>` |
| Add more incoming edges or destinations after a variable-arity instruction is finalized | D1, D2 | Caller discipline plus verifier | `PhiInst<Open>` / `SwitchInst<Open>` / `IndirectBrInst<Open>` / `LandingPadInst<Open>` / `CatchSwitchInst<Open>` are linear; `finish()` returns closed views without mutators |
| Misplace a phi, mistype a phi incoming, or give one predecessor two different incoming values | D1, D4 | Builder accepts all three; the verifier later reports `PhiNotAtTop` or the type / predecessor mismatch | `build_*_phi` always insert at the block's PHI head (placement correct by construction); `add_incoming` — the typed path *and* the untyped parser/SSA-builder path — type-checks the incoming and rejects a differing duplicate for one predecessor as `IrError::AmbiguousPhiIncoming`; whole-graph incoming-vs-predecessor completeness stays in `Module::verify()` |
| Branch carries the wrong number of, or wrong-typed, values for its successor's block parameters | D1, D4 | The successor's head-phis are filled `PHINode`-by-`PHINode`; a miscounted or mistyped incoming is an `assert` / verifier concern | A typed successor label carries a `Params` schema; `head.call(args)` (a `BlockCall`) requires `args: CallArgs<Params>`, so a wrong arity or a wrong-typed block-argument position is a compile error, reusing the typed-`build_call` machinery. The erased `append_block_with_params` / `build_*_with_args` path stays call-site-checked (`IrError::PhiArgArityMismatch` / type mismatch). **The plain-`br` door is shut at run time, not at compile time**: `build_br`/`build_cond_br` still take any `IntoBasicBlockLabel`, whose `BlockId` impl erases `Params`, so a plain branch to a parameterised block still *compiles* — but every plain terminator edge (`br`, `cond_br`, `switch` default and cases, both `invoke` edges, `callbr`, `indirectbr`) now rejects a parameterised target with `IrError::PhiArgArityMismatch` before the terminator is emitted, so the incomplete phi is no longer reachable (§9) |
| Add a wrong-width case value to a `switch` | D4 | Builder accepts any `ConstantInt *`; a case whose integer width ≠ the condition is caught later by `Verifier::visitSwitchInst` (`"Switch constants must all be same type as switch value!"`) | `build_switch::<W>` pins the condition width `W`, and `SwitchInst::add_case` then carries an `IntoIntValue<'ctx, W, B>` bound, so a wrong-width case is a compile error; the erased `build_switch_dyn` (`IntDyn`) keeps the same rule as a runtime `IrError::TypeMismatch` check for parsed / SSA-builder input |
| Jump through a non-pointer `indirectbr` address | D4 | `CreateIndirectBr` accepts any `Value *`; a non-pointer address is caught later by `Verifier::visitIndirectBrInst` (`"Indirectbr operand must have pointer type!"`) | `build_indirectbr`'s address is bound `IntoPointerValue<'ctx, B>`, so a typed non-pointer address is a compile error. There is no erased overload: `IntoPointerValue` has no impl for a bare `Value`, so a parsed address must first be narrowed by `TryFrom` (`ll_parser.rs::parse_indirectbr` does exactly that), which is where the pointer check lands. Either way the rule is out of `Module::verify()` |
| Make a structurally-invalid CFG edge edit — remove an `invoke`/`callbr` edge or the sole edge of an unconditional `br`, remove a `switch` default, or collapse a `cond_br` twice | D1, D2 | Edge edits are raw pointer manipulation (`setSuccessor` / `removePredecessor` / branch replacement); an edit that orphans a mandatory edge yields malformed IR the verifier catches later, if at all | `FnReshape::edit_terminator` narrows the terminator into a per-kind typed handle (`BrEdit` / `CondBrEdit` / `SwitchEdit` / `InvokeEdit` / `CallBrEdit`) whose method set fixes the legal edits: a removal that would orphan a mandatory edge has no method to spell (`E0599`), and `remove_then` / `remove_else` consume the handle, so a double collapse is use-after-move (`E0382`). Each redirect/remove maintains the successors' phis mechanically, poison-erasing an emptied phi for `BasicBlock::removePredecessor` parity |
| Insert a wrong-typed element into a vector or aggregate (`insertelement` / `insertvalue`) | D4, D6 | Builder accepts `Value *`; the verifier later reports the element/field type mismatch | `build_vec_insert` / `build_arr_insert` take a value typed by the handle's element marker `E`, so a wrong element type is a compile error (typed handle); the erased `VectorValue<'ctx>` / `ArrayValue<'ctx>` (`Dyn`) path stays verifier-checked as the escape hatch |
| Elementwise vector binop on mismatched lane count or element type | D4, D6 | Builder accepts two `Value *`; the verifier later reports the operand type mismatch | `build_vec_int_{add,sub,mul,xor,and,or,shl,lshr,ashr}` take two `VectorValue<E, Len<N>>` with the *same* `E`,`N`, so a mismatched length or element has no matching impl (compile error, typed handle); the erased `_dyn` / `VectorValue<'ctx>` path stays verifier-checked |
| `<N x T>` / `[N x T]` length mismatch at a typed vector/array op | D6 | Builder accepts the mis-sized operand; the verifier later reports the length/type mismatch | The length marker (`Len<N>` for vectors, `ArrLen<N>` for arrays) is part of the handle type, so a typed op on a wrong-length value is a compile error; the all-`Dyn` `VectorValue<'ctx>` / `ArrayValue<'ctx>` form narrows via `TryFrom` (`OperandWidthMismatch` / `IrError::ArrayLengthMismatch`) as the escape hatch |
| Run verified-only analyses after a transform | D8 | Verifier pass convention | A pass pipeline's output is `Module<B, Unverified>` whenever any member mutates (derived from the members' rungs), so verified-only analyses require an explicit `verify()` first |
| Pass mutates IR but reports everything preserved | D8, D1 | Pass returns a hand-written `PreservedAnalyses`; over-claiming leaves stale analyses that later passes miscompile against, caught only if a verifier/analysis-checker pass is opted in | Preservation is *derived* from the pass's capability rung, so over-claiming is a compile error: a mutating rung's `done()` floor is fixed by the rung, and `Access = Inspect` has no `mutate()` at all |
| Declare an analysis dependency | D8, D1 | Fallible `getResult` / `getCachedResult`; querying an undeclared or uncomputed analysis returns null and is undefined behavior | `type Requires` is prefetched, then read through the infallible `cx.analysis::<A, _>()`; an undeclared analysis has no `AnalysisSelector` impl, so the access is a compile error |
| External crate authoring a module pass | D8, D1 | `PassInfoMixin` plus manual plugin registration wiring | Implement `ModulePass` (or the `#[module_pass]` sugar), symmetric with function passes — no registration step |
| Author a pass with the wrong rung, no name, or an undeclared analysis | D1, D8 | `PassInfoMixin` + plugin registration; a wrong rung, missing name, or typo'd pipeline entry fails at plugin-load or run time, if at all | `#[function_pass]` / `#[module_pass]` expand to the trait impl and make each slip a pinpointed compile error (a module-only rung fails the `FnAccess` bound, a missing `name` is a `syn::Error`, an undeclared analysis fails a `#[diagnostic::on_unimplemented]` bound) |

### The D7 rows have one carve-out (and one that closed)

The D7 rows above say "compile error". That is exact only when the two modules
carry **different named brands**. One thing still falls outside that, and one
that used to — metadata — no longer does:

**(a) Same-brand pairs fall back to the runtime tag.** Two `Module::dynamic`
modules share `DynBrand`, and a re-issued `branded` brand names two generations
of the same type, so in both cases the handles have the *same* Rust type and
nothing is rejected at compile time. What catches them is the `ModuleId` tag:
`IrError::ForeignValueId` on the fallible paths, a deterministic panic on the
infallible `m.view(id)`, `None` from `try_view`. The guarantee is real but it
is a checked rejection, not a type error. Read every "wrong module is a compile
error" row below as "wrong *brand* is a compile error; wrong module under the
same brand is a checked run-time rejection".

**(b) Metadata carries both halves as of the 0.0.4 polish freeze.** Through
0.0.4 it carried neither: `metadata.rs::MetadataSlot` was a bare `usize`
arena index, and the `ValueSlot` inside `DebugMetadataOperand::Value` was
likewise bare, so there was no `B` for two modules' handles to differ in and no
tag for an arena boundary to check. An in-range handle minted by module A and
attached in module B resolved against B's arena and printed the wrong node,
silently.

The polish cycle split that currency the way cycle A split the value currency.
`MetadataSlot` is now crate-internal, and the public currency is
**`MetadataId<B>`** — `{ tag: ModuleId, slot: MetadataSlot }`, `Copy`, `Send`,
`'static`, brand-invariant. Two named brands make a mix-up a type error
(`tests/compile_fail/cross_module_metadata_attachment.rs`); within one brand the
tag catches it as `IrError::ForeignMetadataId`, at a single choke point every
metadata API routes through
(`tests/module_ownership.rs::a_metadata_id_from_another_module_is_refused_everywhere`).
`IrError::UnknownMetadataSlot { index, len }` remains for the *other* case — a
native id whose slot is past the end of the arena.

So: "cross-module mixing is caught" is now true of metadata on the same terms as
values, blocks, and constants — statically across named brands, by the tag
within one. Only carve-out (a) still applies.

## Runtime errors, fatal verifier passes, and assertions in LLVM C++

LLVM exposes several failure modes for invalid IR:

1. **Builder APIs usually have no error channel.** They return raw pointers such
   as `Value *`, `BranchInst *`, or `CallInst *`.
2. **Standalone verification is sentinel-style.** `verifyModule` returns `true`
   when the module is broken and optionally writes diagnostics.
3. **Verifier passes may be fatal.** The default verifier pass can call
   `report_fatal_error("Broken module found, compilation aborted!")`.
4. **Some constructors/mutators use `assert`.** In assertion-enabled builds that
   aborts; in assertion-disabled builds the check disappears.

Upstream verifier API:

```cpp
/// If there are no errors, the function returns false. If an error is
/// found, a message describing the error is written to OS and true is returned.
LLVM_ABI bool verifyModule(const Module &M, raw_ostream *OS = nullptr,
                           bool *BrokenDebugInfo = nullptr);
```

Fatal verifier pass path:

```cpp
if (FatalErrors && (Res.IRBroken || Res.DebugInfoBroken))
  report_fatal_error("Broken module found, compilation aborted!");
```

`llvmkit` still has a verifier because some IR facts are inherently whole-module
or CFG-dependent. The difference is that many local invariants never reach the
verifier: the type checker rejects them first.

## 1. Cross-module operands

LLVM C++ builder surface:

```cpp
Value *CreateAdd(Value *LHS, Value *RHS, const Twine &Name = "",
                 bool HasNUW = false, bool HasNSW = false) {
  if (Value *V =
          Folder.FoldNoWrapBinOp(Instruction::Add, LHS, RHS, HasNUW, HasNSW))
    return V;
  return CreateInsertNUWNSWBinOp(Instruction::Add, LHS, RHS, Name, HasNUW,
                                 HasNSW);
}
```

That signature cannot say which module owns `LHS` and `RHS`. The verifier catches
foreign references later:

```cpp
Check(F->getParent() == &M, "Referencing function in another module!", &I,
      &M, F, F->getParent());

Check(GV->getParent() == &M, "Referencing global in another module!", &I,
      &M, GV, GV->getParent());

Check(OpInst->getFunction() == BB->getParent(),
      "Referring to an instruction in another function!", &I);
```

In `llvmkit` a module's identity is a **type**. Any `'static` type may be a
brand — the trait demands nothing else, so a bare unit struct qualifies — and
for a *named* brand a process-global registry keeps at most one live module at
a time, so at any given instant a named brand points at exactly one module:

```rust
pub trait ModuleBrand: 'static {}

impl Module<DynBrand, Unverified> {
    // At most one live module per brand; the brand is freed on drop.
    pub fn branded<B: ModuleBrand>(name: impl Into<String>) -> IrResult<Module<B, Unverified>>;
    // ...or retired permanently on drop, so no successor can ever claim it.
    pub fn branded_once<B: ModuleBrand>(name: impl Into<String>) -> IrResult<Module<B, Unverified>>;
    // Registry-exempt: arbitrarily many live at once, separated by the runtime tag alone.
    pub fn dynamic(name: impl Into<String>) -> Module<DynBrand, Unverified>;
}
```

`module_new!("name")` wraps `branded` with a brand declared at the macro's
expansion site, so the brand is unnameable from anywhere else — the ergonomic
descendant of the generative lifetime brand this crate used to mint, but on an
owned, movable token rather than one pinned to a callback's frame.

Two properties of that registry qualify every "different module is a compile
error" claim on this page, and both are load-bearing rather than footnotes:

- **`DynBrand` is registry-exempt.** Arbitrarily many `Module<DynBrand>` values
  may be live at once, and handles from two of them have the *same* type. The
  compile-time half of identity is deliberately traded away there; a mix-up is
  a run-time `IrError::ForeignValueId`, not a type error.
- **`branded` frees the brand on drop.** A later module may re-claim the same
  brand type, so a stale id minted by the dead generation still *type-checks*
  against its successor. `module_ownership.rs::a_stale_id_from_a_dead_generation_is_refused_by_its_successor`
  locks that this is caught at run time by the tag.
  `Module::branded_once` retires the brand permanently instead, so no successor
  can ever exist.

Both are why every id also carries a runtime `ModuleId` tag: the type separates
*distinct named brands*, and the tag separates everything else.

Values carry that brand:

```rust
pub struct Value<'ctx, B: ModuleBrand> {
    id: ValueSlot,
    module: ModuleRef<'ctx, B>,
    ty: TypeSlot,
}
```

The `'ctx` is the borrow of the module the handle came from; the brand `B` is
what separates modules.

That borrow is load-bearing, and it is the second half of the story. A module is
an ordinary owned value that can be dropped, so a `Value`, `BasicBlock`,
`FunctionValue`, or any `*View` minted from one **cannot outlive it** — rustc
rejects the escape with the stable `E0597`:

```rust
let escaped = {
    let m = Module::dynamic("m");
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    // Borrows `m`. Replacing this with `.id()` would compile.
    m.view(f).append_basic_block(&m, "entry")
};
```

Result: compile error — `` `m` does not live long enough ``, locked by
`tests/compile_fail/view_cannot_outlive_its_module.rs`. Upstream has neither
half: a `BasicBlock *` outliving its `Module` is a dangling pointer with no
diagnostic at any stage.

The comment in that snippet is the law that makes the whole storable-id family
necessary rather than merely convenient. Ids (`ValueId`, `FunctionId`,
`BlockId`, …) are `Copy + Send + 'static`: they carry the brand *without* the
borrow, so they may be stored in structs, sent across threads, and outlive the
module — which is exactly why they must also carry the runtime `ModuleId` tag.
A stale id is therefore a **run-time** rejection (`IrError::ForeignValueId`, or
a deterministic panic on the infallible `m.view(id)`), while a stale *view* is
not constructible at all. The two mechanisms are complements, not alternatives.

The integer-add builder requires both operands to match the builder's brand `B`,
and hands back a storable id:

```rust
pub fn build_int_add<W, Lhs, Rhs, Name>(
    &self,
    lhs: Lhs,
    rhs: Rhs,
    name: Name,
) -> IrResult<IntValueId<W, B>>
where
    Name: AsRef<str>,
    W: IntWidth,
    Lhs: IntoIntValue<'ctx, W, B>,
    Rhs: IntoIntValue<'ctx, W, B>,
```

Bad Rust program, from `tests/compile_fail/cross_module_value_brand.rs`:

```rust
let left = Module::branded::<Left, _>("left").unwrap();
let left_value = left.i64_type().const_int(1_i64);

let right = Module::branded::<Right, _>("right").unwrap();
let function = right
    .add_typed_function::<i64, (), _>("f", Linkage::External)
    .unwrap()
    .as_function();
let entry = right.view(function).append_basic_block(&right, "entry");
let builder = IRBuilder::new_for::<i64>(&right).position_at_end(entry);

let _ = builder.build_int_add(left_value, left_value, "bad");
```

Result: compile error — `ConstantIntValue<'_, i64, Left>` does not implement
`IntoIntValue<'_, _, Right>`, and rustc says so in as many words: *for that
trait implementation, expected `Left`, found `Right`*. No verifier pass, no
fatal abort, no delayed broken module.

The scope of that *static* guarantee is values, blocks, constants, and (since
the 0.0.4 polish freeze) metadata nodes under **distinct named brands**. Two
`DynBrand` modules, or two generations of one re-issued `branded` brand, share a
type and are separated by the runtime tag instead — see carve-out (a) under the
summary table.

## 2. Cross-module branch targets

LLVM C++ accepts a raw block pointer:

```cpp
BranchInst *CreateBr(BasicBlock *Dest) {
  return Insert(BranchInst::Create(Dest));
}
```

The verifier later rejects blocks from the wrong function/module:

```cpp
Check(OpBB->getParent() == BB->getParent(),
      "Referring to a basic block in another function!", &I);
```

`llvmkit` requires the target block to carry the builder's brand:

```rust
pub fn build_br<T>(self, target: T) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
where
    T: IntoBasicBlockLabel<'ctx, R, B>,
```

`IntoBasicBlockLabel<'ctx, R, B>` (`basic_block.rs::IntoBasicBlockLabel`) is the
*accepting* bound at every branch-target position, and it follows the same
id/view split as the rest of 2.0:

- `BlockId<R, B, Params>` is the **storable** currency — `Copy + Send +
  'static`, what a producer hands back and what a struct keeps. Its impl
  resolves through the module and is fallible, so a `BlockId` minted in another
  module *of the same brand* yields `IrError::ForeignValueId` rather than
  silently naming a same-numbered slot here.
- `BasicBlockLabel<'ctx, R, B>` is the **borrowing view** a `BlockId` resolves
  to through `Module::view`; its impl is the identity.
- `BasicBlock<'ctx, R, Term, B>` (and `&BasicBlock`) at any termination state,
  so an in-scope block can name itself as a target without a round trip through
  the module, and `SsaBlock<R, B>` for the SSA layer.

The trait is sealed (`basic_block.rs::block_label_sealed::Sealed`), and every
impl is parameterised over the SAME `B` as the builder, so a target block minted
under a *different named brand* has no impl to satisfy the bound at all — the
rejection is static, not a resolve failure. Under the *same* brand (two
`DynBrand` modules, or two generations of one `branded` brand) the impl does
resolve and the `BlockId` bullet above is what catches it, at run time.

Note what the trait deliberately does **not** carry: `Params`. Its `BlockId`
impl calls `BasicBlockLabel::erase_params`, so a parameterised block is an
ordinary branch target *to the type system* at a plain `build_br`. The parameter
schema is honoured at compile time by the `BlockCall` edge only; the plain
builders catch a parameterised target at run time instead, rejecting it with
`IrError::PhiArgArityMismatch` — see §9.

Bad Rust program, from `tests/compile_fail/cross_module_branch_target.rs`:

```rust
let left = Module::branded::<Left, _>("left").unwrap();
let f = left
    .add_typed_function::<(), (), _>("left_f", Linkage::External)
    .unwrap()
    .as_function();
let left_target = left.view(f).append_basic_block(&left, "target");

let right = Module::branded::<Right, _>("right").unwrap();
let f = right
    .add_typed_function::<(), (), _>("right_f", Linkage::External)
    .unwrap()
    .as_function();
let entry = right.view(f).append_basic_block(&right, "entry");
let builder = IRBuilder::new_for::<()>(&right).position_at_end(entry);

let _ = builder.build_br(left_target);
```

Result: compile error — `` `BasicBlock<'_, (), Unterminated, Left>:
IntoBasicBlockLabel<'_, (), Right>` is not satisfied `` (`E0277`). The branch
target is not from the same branded module.

Limit: same-module CFG facts that depend on the *complete* graph — dominance,
and phi-incoming completeness against the final predecessor set for
builder-constructed IR — still belong in `Module::verify()`. The *local* phi
facts are witnessed earlier now: placement is correct by construction, each
`add_incoming` checks the incoming value's type and rejects a conflicting
duplicate for one predecessor, and the `.ll` parser checks phi completeness once
all predecessors are known (see section 9). `Module::verify()` remains the final
gate over the whole-graph coherence.

## 3. Global initializer operands from the wrong module

LLVM C++ global construction asserts type compatibility. The assertion does not
encode module provenance; if an initializer expression references a global from a
different module, that is a verifier concern rather than a constructor type
constraint:

```cpp
if (InitVal) {
  assert(InitVal->getType() == Ty &&
         "Initializer should be the same type as the GlobalVariable!");
  Op<0>() = InitVal;
}
```

`setInitializer` has the same assertion shape:

```cpp
assert(InitVal->getType() == getValueType() &&
       "Initializer type must match GlobalVariable type");
```

`llvmkit` requires the value type and initializer to carry the destination module
brand. This is deliberately stricter than upstream for simple literal constants:
constants created through one branded module cannot be reused in another branded
module, because richer constants can also carry symbol references and operand
wiring.

```rust
pub fn add_global<N, C>(&'ctx self, name: N, initializer: C) -> IrResult<GlobalId<B>>
where
    N: AsRef<str>,
    C: IntoConstantValue<'ctx, B>,
```

The global's value type is *derived* from the initializer rather than passed
alongside it, so the "initializer type must match the global type" assertion
upstream needs has no call site to guard. What remains is the module-provenance
rule, which the brand carries.

Bad Rust program, from
`tests/compile_fail/cross_module_global_initializer_brand.rs`:

```rust
let left = Module::branded::<Left, _>("left").unwrap();
let left_init = left.i32_type().const_int(1_i32);

let right = Module::branded::<Right, _>("right").unwrap();
let _ = right.add_global("g", left_init);
```

Result: compile error. A constant produced by `left` cannot initialize a global
owned by `right`.

## 4. Custom constant folders cannot smuggle foreign values

LLVM C++ folder hooks return raw `Value *`, with `nullptr` meaning "no fold":

```cpp
virtual Value *FoldBinOp(Instruction::BinaryOps Opc, Value *LHS,
                         Value *RHS) const = 0;

virtual Value *FoldSelect(Value *C, Value *True, Value *False) const = 0;
```

A custom folder can accidentally return a value owned by another module; LLVM can
only catch the resulting broken IR later.

`llvmkit` folders are branded:

```rust
pub trait IRBuilderFolder<'ctx, B: ModuleBrand + 'ctx> {
    fn fold_bin_op_dyn(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
    ) -> IrResult<Option<Value<'ctx, B>>> {
        let _ = (opcode, lhs, rhs);
        Ok(None) // decline to fold
    }
}
```

The typed hooks go further: `fold_int_bin_op<W>` returns
`IrResult<Option<IntValue<'ctx, W, B>>>`, so an *external* folder cannot spell a
**concrete** wrong width -- returning an `IntValue<'ctx, i64, B>` where
`IntValue<'ctx, W, B>` is required is an ordinary `E0308`, locked by
`tests/compile_fail/folder_typed_wrong_width.rs`.

The signature is not the whole story, though, and this page does not pretend it
is. Two gaps survive it:

- At `W = IntDyn` the marker names no width at all, so a fold result typed
  `IntValue<'ctx, IntDyn, B>` can still carry any integer width. `IntWidth::narrow`
  is public and proves only "some integer" there, so an external folder *can*
  hand back a wrong-width result.
- In-crate, the `pub(crate)` `IntValue::from_value_unchecked` mints an
  `IntValue<'ctx, W, B>` without consulting the payload's real type, so a static
  `W` is only as honest as whoever built the handle.

So the builder does **not** take the marker on trust. Its `accept_folded_*`
helpers re-check every fold result's runtime type against the operand's (or the
cast's destination), for *every* marker, static ones included, and
`ConstantFolder`'s own typed overrides re-type their erased results through
`W::narrow` / `K::narrow` at the point of construction rather than rewrapping on
the authority of a prose invariant.

Beyond folds, the builder's own marker attachment is likewise no longer implicit.
An int / float / pointer marker reaches a freshly-appended instruction only through
the typed-append constructor family — `append_int_like` / `_at` / `_load`,
`append_fp_*`, `append_ptr` / `append_ptr_load` — each of which appends the
instruction *at* a typed type-handle (`IntType<W>` / `FloatType<K>` / `PointerType`,
or a `W`-typed operand) and re-wraps the result, so the marker matches the runtime
type **by construction** rather than by a proof a reader must reconstruct. This
confinement is *audited*, not a compile-time seal: `from_value_unchecked` stays
`pub(crate)` (a hard seal is impossible — `value` and `ir_builder` are sibling
modules, and the constructors depend on `ir_builder`-private helpers), so its
in-crate callers are now a legible handful — the constructor family, the runtime-
checked fold seams, and a documented residual (result accessors, arena/param lifts,
the vector/array append wraps, the select-arm re-wrap, and the `ptrtoaddr` `IntDyn`
re-wrap) — rather than a hundred scattered wraps, and the fold re-checks above remain
the backstop.

That check used to be keyed on the marker being erased -- which was circular: it
trusted precisely the claim it existed to verify, so at any static width a
wrong-typed fold result was silently accepted. See the CHANGELOG's *"No silent
erasure"* entry; the seam is locked from both sides by
`hostile_native_typed_override_wrong_width_rejected_at_static_width`
(in-crate, via `from_value_unchecked`) and
`external_narrow_override_wrong_width_rejected_by_accept_folded_int`
(external, via `narrow` at `IntDyn`).

Bad Rust helper, from `tests/compile_fail/custom_folder_wrong_brand.rs`:

```rust
struct Foreign;
impl ModuleBrand for Foreign {}

fn return_foreign_folder_value<'ctx, B: ModuleBrand>(
    foreign: Value<'ctx, Foreign>,
) -> Value<'ctx, B> {
    foreign
}
```

Result: compile error. A value carrying the concrete brand `Foreign` is not the
caller's `B`, so it cannot be returned at a hook's brand-generic return
position. The fixture is deliberately brand-*specific* and stays that way:
generalising `foreign` to a brand-agnostic value would prove nothing.

## 5. Terminator builders return terminated block views

LLVM C++ insertion points are mutable positions in raw IR lists:

```cpp
void SetInsertPoint(BasicBlock *TheBB) {
  BB = TheBB;
  InsertPt = BB->end();
}

ReturnInst *CreateRet(Value *V) {
  return Insert(ReturnInst::Create(Context, V));
}
```

That API shape cannot statically prevent code from appending more instructions
after a terminator. LLVM's verifier rejects malformed blocks later.

`llvmkit` models the common construction path with a termination-state marker.
Positioning only accepts an unterminated block:

```rust
pub fn position_at_end<Params>(
    self,
    bb: BasicBlock<'ctx, R, Unterminated, B, Params>,
) -> IRBuilder<'m, 'ctx, B, F, Positioned, R>
where
    Params: BlockParams,
```

Terminator builders consume the positioned builder and return a terminated view
of the insertion block:

```rust
pub fn build_ret<V>(self, value: V) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
where
    V: IntoReturnValue<'ctx, R, B>,
```

```rust
pub fn build_ret_void(self) -> VoidReturnInst<'ctx, B> {
    let inst = self.append_ret(None);
    let bb = self.into_insert_block();
    (bb.retag_termination::<Terminated>(), inst)
}
```

(`VoidReturnInst<'ctx, B>` is an alias for `TerminatedBlockInst<'ctx, (), B>`,
itself a `(BasicBlock, Instruction)` pair — the terminator builders return
borrowing handles, not ids.)

Two separate facts close this, and the compile-fail suite locks each:

1. **The builder is consumed.** Every terminator-emitting build takes `self` by
   value, so a second `build_ret_void()` on the same builder is a use of a moved
   value (`E0382`, `tests/compile_fail/builder_cannot_terminate_twice.rs`).
   Upstream, `IRBuilder` keeps its insertion point after `CreateRetVoid()`, so
   the second call silently appends a second terminator.
2. **The block handle is linear.** `BasicBlock` is deliberately **not** `Copy` —
   it is an insertion token, not a reference. `position_at_end` therefore *moves*
   it, so retaining an earlier `Unterminated` handle and positioning a second
   builder into it is also `E0382`
   (`tests/compile_fail/retained_unterminated_block_cannot_reposition.rs`). Code
   that follows the returned handle instead sees `Term = Terminated`, which
   `position_at_end` does not accept
   (`tests/compile_fail/position_at_end_terminated_block.rs`).

The copyable cross-block reference is `BasicBlockLabel` (or the storable
`BlockId`), and neither is an insertion capability: they can name a branch target
or a phi predecessor, but they cannot be passed to `position_at_end`. Re-entering
a block by id goes through the checked `position_at_end_dyn`, which rejects a
foreign or absent block with `IrError::ForeignValueId`.

## 6. Return type mismatches are rejected by the builder type

LLVM C++ exposes both return builders independently:

```cpp
ReturnInst *CreateRetVoid() {
  return Insert(ReturnInst::Create(Context));
}

ReturnInst *CreateRet(Value *V) {
  return Insert(ReturnInst::Create(Context, V));
}
```

The function's return type is not part of the C++ builder type. A mismatch is a
runtime/verifier concern.

`llvmkit` carries the parent function's return shape in the builder marker `R`:

```rust
pub fn build_ret<V>(self, value: V) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
where
    V: IntoReturnValue<'ctx, R, B>,
```

For a `void` builder, value return is not available through trait resolution; the
only direct return operation is:

```rust
pub fn build_ret_void(self) -> VoidReturnInst<'ctx, B>
```

For typed integer/float/pointer builders, the return value must implement the
matching `IntoReturnValue<'ctx, R, B>` conversion. The dynamic `Dyn` fallback
still exists for parsed/erased code and performs a runtime type-equality check.

## 7. Call results know whether they exist

LLVM C++ call construction returns `CallInst *` regardless of callee return
shape:

```cpp
CallInst *CreateCall(FunctionType *FTy, Value *Callee,
                     ArrayRef<Value *> Args = {}, const Twine &Name = "",
                     MDNode *FPMathTag = nullptr) {
  CallInst *CI = CallInst::Create(FTy, Callee, Args, DefaultOperandBundles);
  return Insert(CI, Name);
}
```

The caller must inspect types to know whether a usable result exists.

`llvmkit` carries the callee return marker into the instruction handle:

```rust
pub struct CallInst<'ctx, R: ReturnMarker, B: ModuleBrand> {
    /* fields omitted */
}
```

Typed accessors are gated by `R`:

```rust
impl<'ctx, B: ModuleBrand + 'ctx> CallInst<'ctx, i32, B> {
    /// Typed result handle for an integer-returning call.
    pub fn return_int_value(self) -> IntValue<'ctx, i32, B> {
        /* construct typed value handle */
    }
}
```

The accessor family is generated per return marker (`return_int_value` for each
integer width including `IntDyn`, plus `return_float_value` /
`return_pointer_value`), so a `CallInst<'ctx, (), B>` has no typed result
accessor at all — the `impl` block that would carry one does not exist for `()`.
The generic `return_value()` method still exists and returns `None` for a `void`
return type, so a void call cannot be mistaken for a typed value.

The call builders themselves hand back a storable `CallInstId<R, B>` (or
`TypedCallInstId<Ret>`), which `Module::view` resolves back into the
`CallInst<'ctx, R, B>` handle above with the return marker intact.

## 8. Instruction lifecycle is linear

LLVM C++ exposes mutating lifecycle operations through raw pointers:

```cpp
/// This method unlinks 'this' from the containing basic block, but does not
/// delete it.
LLVM_ABI void removeFromParent();

/// This method unlinks 'this' from the containing basic block and deletes it.
LLVM_ABI InstListType::iterator eraseFromParent();
```

RAUW also relies on assertions for several invariants:

```cpp
assert(New && "Value::replaceAllUsesWith(<null>) is invalid!");
assert(!contains(New, this) &&
       "this->replaceAllUsesWith(expr(this)) is NOT valid!");
assert(New->getType() == getType() &&
       "replaceAllUses of value with new value of different type!");
```

`llvmkit` makes irreversible operations consume a linear handle:

```rust
pub struct Instruction<'ctx, S: state::InstructionState, B: ModuleBrand> {
    /* fields omitted */
}
```

The handle is intentionally not `Copy` or `Clone`; lifecycle methods take `self`:

```rust
pub fn replace_all_uses_with<V: IsValue<'ctx, B>>(
    self,
    module_token: &'ctx Module<B, Unverified>,
    replacement: V,
) -> IrResult<()>
```

```rust
pub fn erase_from_parent(self, module_token: &'ctx Module<B, Unverified>)
```

Once an instruction handle is erased or detached, the consumed binding cannot be
used again. The replacement value also carries the same module brand `B`.

Copyable discovery paths such as `BasicBlock::instructions`,
`BasicBlock::terminator`, `Value::users`, and per-opcode `as_view` return
`InstructionView`. The view can inspect, name, and attach metadata, but it has no
lifecycle methods. The public paths that produce `Instruction<Attached>` are the
builder result, `BlockCursor::next` over an unsealed block, and detached
reinsertion.

## 9. Open/closed views for variable-shape instructions

LLVM C++ phi construction returns a mutable `PHINode *` that the caller fills in
incoming-by-incoming:

```cpp
PHINode *CreatePHI(Type *Ty, unsigned NumReservedValues,
                   const Twine &Name = "") {
  PHINode *Phi = PHINode::Create(Ty, NumReservedValues);
  return Insert(Phi, Name);
}
```

That incremental fill-in is where predecessor/incoming *desync* is born — a CFG
edit that forgets to update a successor phi, or a phi left one incoming short.
`llvmkit`'s **recommended** phi authoring removes that shape entirely: you never
build a bare phi and bolt edges onto it. Instead you give a block *parameters*
and let each branch carry the values — the Swift-SIL / MLIR block-argument
shape:

```rust
// The block's parameters ARE its head-phis; the branch carries the incomings.
let (hdr, params) = builder.append_block_with_params(m.view(f), &[i32_ty.as_type()], "hdr")?;
let hdr_target = hdr.id();                 // storable, Copy branch target
// ... then, positioned in each predecessor (`build_br_with_args` takes erased
// `Value`s, hence the `into_erased()`):
builder.build_br_with_args(hdr_target, &[m.view(x).into_erased()])?;
```

`build_br_with_args` / `build_cond_br_with_args` append the terminator *and* seed
each successor parameter in one call, arity- and type-checked at the call site
(`IrError::PhiArgArityMismatch` / `IrError::TypeMismatch`), all-or-nothing.
The edge and its incomings move together, so *along this path* an incomplete or
desynced phi cannot be built. Printed IR is ordinary phis; storage, parser,
printer, and verifier are unchanged.

**The honest limit, because it is the sort of thing this page exists to state.**
The block-argument surface is a *better door*, not a sealed one. Nothing in the
type system forces a caller through it, and two public spellings still land an
incomplete or desynced phi at `Module::verify()` rather than earlier:

- **`remove_incoming` is public** (see below) and can empty or desync a phi
  outright; `phi_raw_tests/remove_incoming.rs::remove_incoming_leaves_the_verifier_to_flag_the_missing_edge`
  locks precisely that outcome.
- **The `#[doc(hidden)] pub` parser contract** (`build_int_phi_dyn`,
  `build_fp_phi_dyn`, `build_pointer_phi_in_addrspace`, `build_phi_dyn`,
  `phi_add_incoming_from_value`) is unsupported but reachable from outside the
  crate, and it *is* the bare-phi-then-bolt-edges shape.

The third spelling that used to belong on that list — **a plain `br` to a
parameterised block** — was closed in `feature-34/polish-freeze`. It still
*compiles* (the label positions erase `Params`, by design), but every plain
terminator edge now checks its target before emitting and rejects a
parameterised one with `IrError::PhiArgArityMismatch`: `build_br`,
`build_cond_br`, `build_switch` / `build_switch_dyn` and `SwitchInst::add_case`,
all four `invoke` entry points, `build_callbr*`, and
`IndirectBrInst::add_destination`. `switch` and `invoke` also gained the
argument-carrying forms they were designed with and never got
(`build_switch_with_args` / `build_switch_dyn_with_args`,
`build_invoke_with_args` / `build_invoke_dyn_with_args`), so every terminator
that can reach a parameterised block either carries its arguments or does not
build. The guard keys on *block parameters*, not on "the block contains phis",
so the parser's back-edges, `SsaBuilder`'s unsealed loop headers, and
pass-inserted phis are unaffected.

So the accurate claim is: block arguments make the desync **unrepresentable in
the code that uses them**, the plain-branch door is shut at build time, and the
raw typed builders that used to be the easy way to get it wrong are gone from
the public surface (`E0599`, locked by
`tests/compile_fail/raw_phi_builder_is_unnameable.rs`). Whole-graph
incoming-vs-predecessor coherence remains `Module::verify()`'s job, for
builder-constructed IR as much as for parsed IR.

The block-argument surface above is width/type-erased: arity and per-argument
types are checked at the *call site* (runtime `IrError`). A **typed** variant
lifts the block's *parameter shape* into the type system so those checks move to
*compile* time. `append_block_typed::<(i32, Ptr), _>(m.view(f), "hdr")` returns
the block stamped with that schema plus a typed tuple of parameter handles
(`(IntValue<'_, i32, _>, PointerValue<'_, _>)`). The edge is then bundled
separately: `hdr.call((a, b))` mints a `BlockCall`, consumed by `build_br_call` /
`build_cond_br_call`, and `BasicBlockLabel::call` carries a `CallArgs<Params>`
bound (the same machinery a typed `build_call` uses), so a wrong-arity or
wrong-typed block-argument is a compile error rather than the erased path's
call-site `IrError::PhiArgArityMismatch` / type mismatch — locked by
`tests/compile_fail/block_call_wrong_arity.rs` and
`block_call_wrong_arg_type.rs`. What the `Params` marker does **not** do is
force a caller to spell the edge as a `BlockCall` at all; the plain-`br` escape
above stays open. Both surfaces lower to the identical ordinary phis; the schema is
an opt-in, last, defaulted `Params` marker (`BlockParamsDyn` by default), so every
erased spelling is unchanged. In the same spirit, a `switch`'s condition width and
an `indirectbr`'s address pointer-ness can be pinned in the type
(`build_switch::<W>` / the `IntoPointerValue`-bound `build_indirectbr`), so a
wrong-width case or a non-pointer jump address is a compile error too — see the
summary table above.

Underneath, the incremental editing window still exists — the `PhiInst` handle
*type* and its read accessors stay public (a `_dyn` builder returns the matching
`PhiInstId`, which views back into one, and rediscovery yields it), but
*authoring through it* is off the supported surface: the marker-form
`build_*_phi` builders and the `add_incoming` mutator are `pub(crate)` — a hard
`E0624` — and the few entry points the separate parser crate needs are
`#[doc(hidden)] pub` "internal contract" items. Be exact about the difference:
`pub(crate)` is a compiler-enforced seal, `#[doc(hidden)] pub` is a convention
an external caller can ignore. So the visibility keeps a phi unobservable
mid-construction from *ordinary* outside code, not from determined outside code:

```rust
pub struct PhiInst<'ctx, W: IntWidth, B: ModuleBrand> {
    /* fields omitted */
}
```

The handle carried an `Open`/`Closed` construction typestate until cycle B
(slice B1g) retired it. Cycle B's builders hand back `Copy` ids, and a view
minted from a `Copy` id is re-mintable, so a linear "only one open capability"
marker could no longer be *true* — and the public `remove_incoming` added
alongside it mutates exactly the rediscovered handles the marker called
finalised. A marker that gates nothing is worse than none, so it went; the
`pub(crate)` seal on the raw builders is the guarantee that survives.

`add_incoming` witnesses the *local*
phi facts at the call site rather than at `verify()` time: the incoming value's
type is checked against the phi (the untyped parser / SSA-builder path
`phi_add_incoming_from_value` included), and a second incoming for a predecessor
already recorded with a *different* value is rejected as
`IrError::AmbiguousPhiIncoming` — same-value duplicates stay legal, so a `switch`
with several edges from one predecessor still builds. Placement is correct by
construction too: the internal builders insert at the block's PHI head regardless
of cursor position (the verifier's `PhiNotAtTop` check stays as defense in
depth). What these local checks do *not* cover — phi-incoming completeness against
the final predecessor set, and dominance — remains `Module::verify()`'s job; the
`.ll` parser additionally checks that completeness once all predecessors are
known.

Edge *removal* is public: `remove_incoming` (on all four phi handles and on the
variant-independent `PhiKind`) mirrors `PHINode::removeIncomingValue`, including
its backfill-from-the-end ordering, and takes an `Unverified` module token as
its mutation-capability witness. It deliberately does **not** mirror upstream's
`DeletePHIIfEmpty`: llvmkit erases through `Instruction::erase_from_parent`,
which consumes the linear lifecycle handle so use-after-erase is a compile
error, and a `Copy` opcode handle cannot express that consumption. Auto-erasing
an emptied phi ships where it can be sound instead — on the `ReshapeCfg` edge
edits, which RAUW it with poison and erase it (LLVM `removePredecessor`).

The linear open/closed pattern remains **public** for the variable-arity
terminators — `switch`, `indirectbr`, `landingpad`, and `catchswitch`: open
handles are not `Copy`, mutators consume `self`, and `finish()` returns a closed
view. Rediscovery through `InstructionKind` / `TerminatorKind` also returns closed
variants, so it cannot reopen a finalized variable-arity instruction.

The closed views are still fully inspectable: each variable-arity terminator
exposes a reader for its entries — `SwitchInst::cases()` yields
`(case_value, target)` pairs, and `IndirectBrInst::destinations()`,
`LandingPadInst::clauses()`, and `CatchSwitchInst::handlers()` yield their
respective lists. Reading the entries never risks reopening the instruction.

## 10. Verification state is part of module type

LLVM C++ verification is a convention: a caller chooses whether to run
`verifyModule`, a verifier pass, or no verification at all.

`llvmkit` encodes the state:

```rust
Module<B, Unverified>
Module<B, Verified>
```

Verification consumes mutation capability and returns a verified token on
success. A pass pipeline's output typestate is *derived* from its members'
capability rungs: an all-read-only (`Inspect`) run preserves `Verified`, while
any mutating pass returns `Unverified`, forcing an explicit re-verification
before verified-only analyses or pass pipelines can consume the result (see
section 11).

### The `Unverified` token — now including instruction metadata

The typestate only means something if mutation routes do not bypass it. The rule
is that a mutator takes a `&Module<B, Unverified>` capability token, which
`verify(self)` consumes: once the module has become `Module<B, Verified>` there
is nothing left to hand one, so the re-verify obligation is enforced by the type
checker rather than by a convention. This holds across the IR-shaped mutators —
every `set_*` on `FunctionValue`, `GlobalVariable`, `GlobalAlias`,
`GlobalIFunc`, and `Instruction`/`InstructionView`, plus `set_name` and the
whole instruction lifecycle (`erase_from_parent`, `replace_all_uses_with`).

`ComdatRef::set_selection_kind` was the last escape and now takes the token too.
It was reachable because `Module::get_comdat` is state-generic, so a `Verified`
module hands out a `ComdatRef` — and the selection kind is printed
(`$name = comdat <kind>`), so rewriting it changed verified IR. (The pass-API
leg was never open: `ModuleView::comdats()` yields `ComdatView`, which has
`module` / `name` / `selection_kind` and no setter, so an `Inspect`-rung pass
could not reach it.)

**How far "every mutator" has actually been checked.** A scan over every `pub
fn` in the workspace taking `self`/`&self`, mutating interior state
(`Cell::set` / `RefCell::borrow_mut` / `replace`), and *not* naming
`Unverified` returns 53 candidates. They fall into four groups, none of which is
an escape:

- **Value types** (`ap_int.rs`, `known_bits.rs`) — `ApInt::set_sign_bit` and
  friends mutate a local numeric value, not module storage.
- **`ModuleCore` internals** — crate-private; the public `Module` wrappers over
  them either take the token or live in the `Unverified` impl.
- **Builders** (`ir_builder.rs`) — an `IRBuilder` can only be positioned on an
  unverified module, so the token is upstream of every `build_*`.
- **Variable-arity terminator handles** — `IndirectBrInst::add_destination`,
  `LandingPadInst::set_cleanup`, `CatchSwitchInst::add_handler` are gated on the
  `term_open_state::Open` typestate instead. `Open` is a unit struct with a
  private field, mintable only by the builder that just created the terminator;
  every reading path yields `Closed`. That is a stronger guarantee than the
  token, not a weaker one.

The scan is mechanical and its triage is by category, so read this as "no escape
found by a systematic sweep", not as a proof.

Instruction *metadata* was the most damaging escape from this rule, and cycle E
closed it. `InstructionView::set_metadata` and `InstructionView::push_debug_record`
(and their `Instruction` twins) took no token, which left two real holes: a `Verified`
module's printed IR could be changed through a read-only view with the typestate
still claiming it had been verified, and an `Inspect`-rung pass — which is handed
only views, never a token — could rewrite `!dbg` attachments while the driver
derived `Module<B, Verified>` and reported everything preserved. The metadata
setters on `FunctionValue` and `GlobalVariable`, and `set_name`, already required
the token; only the instruction pair did not. All four now take it:

```rust
let verified = m.verify().unwrap();          // consumes the Unverified token

let view = verified.as_view();
let inst = /* ... a read-only InstructionView reached through `view` ... */;

// No `&Module<B, Unverified>` is left in scope, and the `Verified` module
// cannot supply one — so this call cannot be written.
inst.set_metadata(&verified, MetadataAttachmentKind::Dbg, node);
```

Result: compile error `E0308` — *expected `&Module<DynBrand>`, found
`&Module<DynBrand, Verified>`*, with the `note:` spelling out the elided default:
*expected reference `&Module<DynBrand, Unverified>` / found reference
`&Module<DynBrand, Verified>`*. Locked by
`tests/compile_fail/verified_module_metadata_is_immutable.rs`. Upstream has no
analogue: `Instruction::setMetadata` is a plain non-const method, `verifyModule`
is a free function returning a `bool` a caller may ignore, and nothing connects
the two.

That bounds *reachability* of the metadata arena — mutating it requires code
that already holds the target module's token. The polish freeze then made the
metadata **currency** module-safe as well: `set_metadata` and
`push_debug_record` take `MetadataId<B>` rather than a bare slot, and the token
they already demanded is what supplies the `ModuleId` the id's tag is compared
against, so both calls now return `IrResult<()>` and reject a foreign node with
`IrError::ForeignMetadataId`. See carve-out (b) under the summary table.

This does not remove the verifier. It makes the verifier's result impossible to
forget in typed APIs.

## 11. Passes cannot lie about what they preserve

This is `llvmkit`'s pass-authoring headline, and it has no upstream equivalent.

In LLVM's new pass manager a pass hand-writes what it preserved, and the manager
trusts it:

```cpp
PreservedAnalyses run(Function &F, FunctionAnalysisManager &AM) {
  // ... mutate F ...
  return PreservedAnalyses::all();   // a lie: F changed, nothing is invalidated
}
```

A wrong `PreservedAnalyses` is the highest-impact pass bug there is: the manager
keeps a now-stale cached analysis and a later pass miscompiles against it. LLVM
catches it only if you opt into verification instrumentation (`-verify-each`);
the type system offers no defense, because `run` can mutate `F` and still return
`all()`.

`llvmkit` removes the hand-written claim entirely. A pass declares a *capability
rung* — how much it may mutate — and the driver *derives* the preservation set
from that rung. The author never writes a `PreservedAnalyses` value:

```rust
pub trait FunctionPass<B: ModuleBrand> {
    type Access: FnAccess; // Inspect | PatchBody | ReshapeCfg
    type Requires;
    const NAME: &'static str;
    const REQUIRED: bool = false;

    fn run<'m, 'ctx>(
        &mut self,
        cx: FnCx<'m, '_, 'ctx, B, Self::Access, Self::Requires>,
    ) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
        Self::Requires: FunctionAnalysisList<'ctx, B>;
}
```

`run` is higher-ranked over **both** context regions — `'m`, the driver's borrow
of the module and everything minted from it, and `'ctx`, the region the
prefetched analyses were collected at — so the driver picks both and may hand the
pass a module borrow rooted in its own frame. `Requires` carries its
`FunctionAnalysisList<'ctx, B>` bound on `run` rather than on the associated
type, for the same reason.

Two structural facts make over-claiming unspellable.

**(a) A read-only rung has no mutation door.** `Inspect` deliberately does not
implement `MutatingFn`, and `FnCx::mutate` exists only where `A: MutatingFn`. So
an `Inspect` context has no `mutate()` method at all — a pass declared read-only
cannot mutate, whatever its body attempts:

```rust
impl<B: ModuleBrand> FunctionPass<B> for InspectMutates {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "inspect-mutates";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let patch = cx.mutate(); // no such method on an Inspect context
        Ok(patch.done())
    }
}
```

Result: compile error `E0599` — ``the method `mutate` exists for struct
`FnCx<..., Inspect, ()>`, but its trait bounds were not satisfied`` — with rustc
naming the missing bound outright: `` `Inspect: MutatingFn` ``.

**(b) Reaching a mutator consumes the all-preserved report.** `FnCx::mutate`
takes `self` **by value**. Once a mutating pass has stepped into its mutator the
context is moved, so the all-preserved `cx.done()` is gone.
The only report left is the mutator's own `done()`, which carries the rung's
derived floor. "Mutated, then claimed everything preserved" has no spelling:

```rust
    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let _patch = cx.mutate(); // moves `cx` into the mutator
        Ok(cx.done())             // use of moved value
    }
```

Result: compile error `E0382` — ``use of moved value: `cx` ``.

The floor is always a safe under-approximation: under-claiming only costs a
recompute, while over-claiming is the miscompile — and over-claiming is exactly
what has no representation. The rung ladder:

| Rung | May mutate | Derived floor |
| --- | --- | --- |
| `Inspect` | nothing (read-only) | all preserved |
| `PatchBody` | instructions inside existing blocks | CFG-shaped analyses preserved |
| `ReshapeCfg` | the whole CFG | nothing preserved |
| `RewriteModule` (module level) | globals, functions, bodies | nothing preserved |

Both bad programs are locked in the compile-fail suite
(`tests/compile_fail/inspect_pass_cannot_mutate.rs`,
`tests/compile_fail/claim_preserved_after_mutate.rs`).

Analysis dependencies are the same story from the read side: a pass lists what
it needs in `type Requires`, the driver prefetches it, and the pass reads it
through the infallible `cx.analysis::<A, _>()`. Upstream's `getResult<A>` /
`getCachedResult<A>` are fallible and return null (undefined behavior) when the
analysis was never declared or computed; here an undeclared analysis has no
`AnalysisSelector` impl, so the access does not compile.

### Authoring mistakes are compile errors too (`#[function_pass]` / `#[module_pass]`)

A pass is one `impl` block plus the `#[function_pass]` / `#[module_pass]`
attribute — no plugin entry point, no registration callback, no `PassInfoMixin`.
The macro is zero-cost sugar (it expands to exactly the trait impl above), but it
also turns the usual authoring slips into pinpointed compile errors instead of
plugin-load or run-time failures. Declaring a module-only rung on a function pass
is rejected by the rung bound:

```rust
#[function_pass(name = "oops", access = RewriteModule)]
impl Oops {
    fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> { Ok(cx.done()) }
}
```

Result: compile error `E0277` — ``the trait bound `RewriteModule: FnAccess` is
not satisfied`` — reported at the attribute, with rustc listing the three rungs
that do implement it. A function pass cannot even spell a module rung. In the
same way, omitting `name` is a `syn::Error` at the attribute (``missing `name =
"..."`; a pass must declare its `NAME` ``), and reading an analysis the pass
never listed in `requires` fails its `#[diagnostic::on_unimplemented]` bound
(``analysis `DominatorTreeAnalysis` is not in this pass's `Requires` list `()` ``,
with a note pointing at the fix). Upstream, the analogous mistakes — a malformed `PassInfoMixin`,
a typo'd pipeline name, a missing `llvmGetPassPluginInfo` registration — surface
at plugin-load or run time, if at all. Each of these is locked in the
compile-fail suite (`function_pass_wrong_level_access.rs`,
`function_pass_missing_name.rs`, `undeclared_analysis_in_pass_body.rs`), and a
mutating pass pushed into a read-only runtime pipeline is rejected the same way
(`mutating_pass_cannot_enter_readonly_dyn.rs`).

### `type Requires` (analysis deps) vs. `const REQUIRED` (a must-run pass)

Two similarly-named knobs are easy to conflate, so to be explicit they are
different things:

- **`type Requires`** is the pass's *analysis dependency list* — the analyses it
  reads (covered just above). The driver prefetches them, and
  `cx.analysis::<A, _>()` returns them infallibly.
- **`const REQUIRED`** marks the *pass itself* as one that must always run — a
  pass that pass-instrumentation is not allowed to skip. It defaults to `false`
  and is set declaratively, or with the bare `required` flag on the macro:
  `#[function_pass(name = "...", access = ..., required)]`.

LLVM expresses "always run this pass" with the `RequiredPassInfoMixin` CRTP
marker plus a virtual `isRequired()` that the pipeline consults at run time.
llvmkit makes it a compile-time associated constant (`const REQUIRED: bool`)
instead — no inheritance, no virtual — which the runtime `Dyn` pipelines surface
through `has_required_pass()`. Honest scope, since this page documents what
actually ships: the flag and its accessor exist today, but the pass
instrumentation that would consult them to *skip* non-required passes is not yet
wired (see `docs/future-work.md`). So `const REQUIRED` currently records the
author's intent at the type level; today every queued pass runs regardless,
because nothing skips any pass yet.

### A tuple pipeline derives its output's verified state from its members

The single-pass rule (section 10: verification state is part of the module type)
extends to a *tuple of passes*, and this is where the typestate does the most
work. `function_pipeline((A, B, C))` / `module_pipeline((...))` run their members
in written order, and the module type that `.run(...)` hands back is computed at
compile time from the members' rungs: if every member is `Inspect` (read-only)
the output is `Module<B, Verified>`; if any member mutates, it is
`Module<B, Unverified>`. It is a type-level fold — `StaysVerified` is the identity
and `Downgrades` (any mutating rung) is absorbing — so the verdict is a property
of the tuple, never a value anyone writes:

```rust
// Two read-only passes → the pipeline hands back a still-verified module.
let mut pipe = function_pipeline((CountBlocks, EntryReachable));
let m: Module<_, Verified> = pipe.run(verified, f, &mut analyses)?;

// Swap in one mutating (`PatchBody`) pass and the SAME `.run(...)` call now
// returns `Module<_, Unverified>` — the `Verified` annotation above stops
// compiling.
let mut pipe = function_pipeline((CountBlocks, InstSimplifyPass));
let m: Module<_, Unverified> = pipe.run(verified, f, &mut analyses)?;
let _ = m.verify()?; // required before the next verified-only stage
```

`run` takes the `Module<B, Verified>` **by value**, so the input token is
consumed and the only module left to work with is whichever one the fold
produced — there is no stale `Verified` binding lying around to reach for.

There is no way to pull a `Module<B, Verified>` out of a pipeline that contains a
mutating pass, and no way to forget the re-verify: the return type carries the
answer. LLVM's pipelines leave "is the IR still verified after this?" to
convention. The runtime `Dyn` containers can't run this fold (their member list
is only known at run time), so they commit at construction instead —
`DynReadOnlyFunctionPipeline` accepts only `Inspect` passes and always yields
`Module<B, Verified>`, while `DynFunctionPipeline` accepts any pass and always
yields `Module<B, Unverified>`.

## 12. Instruction inspection is exhaustive and precisely typed

In LLVM C++ a pass inspects an instruction with `isa`/`dyn_cast` plus
`getOpcode()`, and operand accessors hand back `Value *`:

```cpp
if (auto *LI = dyn_cast<LoadInst>(&I)) {
  Value *Ptr = LI->getPointerOperand();   // Value*, re-check the type yourself
}
switch (cast<CastInst>(&I)->getOpcode()) { /* runtime opcode dispatch */ }
```

`llvmkit` makes the same inspection a `match` over an exhaustive enum, and it
carries the types the IR grammar already guarantees:

```rust
match view.classify() {
    // Total: every instruction is a non-terminator or a terminator, so there
    // is no overloaded `None` to forget an `is_terminator()` guard for.
    Classified::Inst(InstructionKind::Load(load)) => {
        let ptr: PointerValue<'_, _> = load.pointer();  // typed, not an erased Value
    }
    Classified::Inst(InstructionKind::Cast(CastKind::PtrToInt(c))) => {
        let src: PointerValue<'_, _> = c.src();         // one handle per cast opcode
    }
    Classified::Term(TerminatorKind::Switch(sw)) => {
        for (case_value, target) in sw.cases() { /* ... */ }
    }
    _ => {}
}
```

`InstructionKind` and `TerminatorKind` are `#[non_exhaustive]`-free on purpose:
a new opcode is a compile error in every `match` that has not considered it,
which is the point — a silent `_` fallthrough would let a new opcode inherit
whatever the wildcard does. Casts split into `CastKind` (one handle per opcode,
mirroring LLVM's `TruncInst`/`ZExtInst`/… classes), and phis into
`PhiKind { Int, Fp, Ptr, Other }` chosen from the phi's *result type* — so the
narrowing accessor is always sound and there is no integer-flavored handle whose
`as_int_value()` would lie on an `f64` or pointer phi.

Where a single-opcode `match` is too fine, grouped views recover the C++
`dyn_cast<BinaryOperator>` / `dyn_cast<CmpInst>` ergonomics without losing the
opcode: `InstructionKind::as_binary_op()` exposes `lhs`/`rhs`/`opcode`/`nuw`/
`nsw`/`exact`/`is_commutative` across all eighteen arithmetic opcodes, and
`as_cmp()` exposes `lhs`/`rhs` and a unified `CmpPredicate` over `icmp`/`fcmp`.
The flag overlays `OverflowingBinaryOperator` (add/sub/mul/shl) and
`PossiblyExactOperator` (udiv/sdiv/lshr/ashr) mirror LLVM's `Operator.h` split.

## What llvmkit still verifies at runtime

`llvmkit` intentionally does not pretend every LLVM rule is local enough for the
type system. Runtime verification still owns:

- parsed or otherwise erased `Dyn` forms;
- **a custom folder's typed fold result** — the hook's signature keeps a
  *concrete* wrong width out (§4), but `IntDyn` names no width and the
  crate-internal `from_value_unchecked` can mint any marker, so a marker is only
  as honest as whoever built the handle. The builder re-checks each fold result's
  runtime type against the operand / cast destination, for every marker;
- **an `SsaBuilder` variable definition** — same reason, same shape:
  `def_int_var` / `def_float_var` re-check the incoming value's runtime type
  against the variable's, for every marker;
- dominance and cross-block SSA use checks;
- phi-incoming completeness against the *complete* CFG predecessor set — the
  whole-graph check, and **not only for parsed input**. Wave 1 moved the *local*
  phi facts earlier (placement is correct by construction; `add_incoming` checks
  the incoming value's type and rejects a differing duplicate for one
  predecessor; the `.ll` parser runs `check_function_phi_coherence` at
  end-of-function parse once all predecessors are known; and `split_block`
  maintains its successors' phi incomings itself). But a plain `build_br` /
  `build_cond_br` into a block that has parameters seeds nothing and is not
  arity-checked, so builder-constructed IR reaches `verifier.rs::check_phi` the
  same way parsed IR does. `Module::verify()` is the gate here, not a backstop —
  see §9;
- complete terminator and reachability invariants after parser/pass mutation;
- data-layout-dependent size/alignment rules;
- verifier rules for attributes, globals, atomics, calls, EH pads, and metadata
  that depend on whole-instruction or whole-module context;
- metadata *content* rules — that a `!range` tuple has an even, non-zero
  operand count of integer constants, that `!absolute_symbol` is not the empty
  range, that an attachment sits on an opcode that accepts it. Metadata
  **provenance** is no longer on this list: since the 0.0.4 polish freeze a
  metadata handle is a tagged, branded `MetadataId<B>`, so a foreign node is
  `IrError::ForeignMetadataId` at the attach site rather than a silent
  mis-resolve at print time.

The rule of thumb is simple: if Rust can know the invariant from the types at the
call site, `llvmkit` makes it a type error. If the invariant depends on the whole
module, CFG, data layout, or erased parser input, `Module::verify()` reports it.

## Deliberate divergences from upstream defaults

llvmkit's *semantics* track upstream LLVM; a small number of API *defaults*
deliberately do not. These change what the equivalent construction sequence
emits, so they are documented here rather than left to surprise a diff:

- **Call sites default to the callee's calling convention.** Upstream
  `IRBuilder::CreateCall` leaves every new call site at `ccc` even when the
  callee declares `fastcc`; making them agree is the frontend's job
  (`CallBase::setCallingConv`), and a mismatch is undefined behavior at run
  time. llvmkit's call builders (`build_call`, `build_varargs_call`,
  `call_builder`, `typed_call_builder`) instead default the call site to the
  callee's own convention, so the same construction sequence against a
  `fastcc` callee prints `call fastcc ...` where upstream prints `call ...`.
  To override, pass a `CallSiteConfig` to `build_call_with_config` (its
  `CallSiteConfig::new` starts at `CallingConv::C`, so a bare config *is* the
  upstream default), or set `.calling_conv(..)` on the fluent `call_builder` /
  `typed_call_builder`. Parsed IR is unaffected: the parser stores exactly the
  convention the input spells.

## Proof in the repository

The compile-fail suite locks these guarantees with `trybuild`. As of 0.0.4 it
registers **85 fixtures — 84 `t.compile_fail` plus 1 `t.pass` — and the baseline
is 0 failures** on the pinned toolchain (`cargo +1.96.0`; a newer rustc rewords
diagnostics and produces `.stderr` mismatches that are not regressions). All of
them live in `crates/llvmkit-ir/tests/compile_fail/` and are registered in
`crates/llvmkit-ir/tests/typestate_compile_fail.rs`, which is the count's
source of truth — `CLAUDE.md` tracks the same figure.

The sketch below is **abridged and regrouped by section of this page** — it is
not the file's registration order, and the grouping comments are this page's,
not the file's. Read the real list in `typestate_compile_fail.rs`:

```rust
#[test]
fn typestate_compile_fail() {
    let t = trybuild::TestCases::new();
    // The single `pass` case flips trybuild's `has_pass` switch from `cargo
    // check` to `cargo build`, which is load-bearing: `extract_value_empty_
    // indices.rs` fails with a monomorphisation-time `E0080` that a `check`
    // never reaches.
    t.pass("tests/compile_fail/extract_value_dyn_empty_slice_compiles.rs");
    // Brand / typestate locks (sections 1-3):
    t.compile_fail("tests/compile_fail/cross_module_value_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_global_initializer_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_branch_target.rs");
    t.compile_fail("tests/compile_fail/cross_module_select_arm.rs");
    t.compile_fail("tests/compile_fail/custom_folder_wrong_brand.rs");
    t.compile_fail("tests/compile_fail/cross_named_brand_id_view.rs");
    // Owned-module / handle-lifetime locks (sections 1, 5, 10):
    t.compile_fail("tests/compile_fail/view_cannot_outlive_its_module.rs");
    t.compile_fail("tests/compile_fail/verified_module_metadata_is_immutable.rs");
    t.compile_fail("tests/compile_fail/builder_cannot_terminate_twice.rs");
    t.compile_fail("tests/compile_fail/retained_unterminated_block_cannot_reposition.rs");
    t.compile_fail("tests/compile_fail/position_at_end_terminated_block.rs");
    // Capability-graded pass API locks (section 11):
    t.compile_fail("tests/compile_fail/inspect_pass_cannot_mutate.rs");
    t.compile_fail("tests/compile_fail/claim_preserved_after_mutate.rs");
    t.compile_fail("tests/compile_fail/undeclared_analysis_in_pass_body.rs");
    t.compile_fail("tests/compile_fail/function_pass_wrong_level_access.rs");
    t.compile_fail("tests/compile_fail/function_pass_missing_name.rs");
    t.compile_fail("tests/compile_fail/mutating_pass_cannot_enter_readonly_dyn.rs");
    // Phi authoring: raw builders unnameable, typed block-call edge (section 9):
    t.compile_fail("tests/compile_fail/raw_phi_builder_is_unnameable.rs");
    t.compile_fail("tests/compile_fail/block_call_wrong_arity.rs");
    t.compile_fail("tests/compile_fail/block_call_wrong_arg_type.rs");
    /* 60-odd further fixtures omitted; see the file for the full list */
}
```

Run the focused proof:

```bash
cargo +1.96.0 test -p llvmkit-ir typestate_compile_fail
```

The pinned toolchain is not decoration. A `.stderr` file records one rustc's
exact diagnostic text, and CI pins **1.96.0**, so the suite must be run at that
version — a mismatch on a *newer* rustc is a toolchain difference, not a
finding, and blessing it would corrupt the baseline for everyone on the pin.

Where a fixture's primary error is one of llvmkit's *own* messages — an `E0599`
absent-method, an `E0382` use-after-move, a `#[diagnostic::on_unimplemented]`
note, or a `syn::Error` — that text does not drift across rustc versions, and
most fixtures are deliberately written to land on such an error rather than on
an inference-failure message.

Those tests are intentionally not one-to-one ports of LLVM C++ tests. They are
`llvmkit`-specific type-safety locks for invariants that upstream LLVM represents
through raw pointers plus assertions, verifier diagnostics, or fatal verifier
passes.
