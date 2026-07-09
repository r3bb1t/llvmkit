# Type Safety: llvmkit vs. LLVM C++

`llvmkit` tracks LLVM's IR semantics, printer forms, verifier rules, and source
layout. The deliberate difference is the public API shape: where upstream LLVM C++
often lets callers build malformed IR and asks a later verifier pass to reject it,
`llvmkit` pushes many local, statically knowable invariants into Rust types.

This is not a claim that LLVM C++ is poorly designed. LLVM is a mature C++
compiler infrastructure optimized around pointer identity, intrusive lists,
mutation-heavy passes, and late verification. `llvmkit` has a different advantage:
its API can use Rust ownership, typestate, sealed traits, and generative lifetimes
to make many invalid states unspellable.

## The short version

The Doctrine column references the D1-D11 rules in the README. This page focuses
on user-visible API failure modes; D11's test-provenance rule is tracked in
`UPSTREAM.md` rather than repeated here.

| Problem shape | Doctrine anchor | Upstream LLVM C++ | llvmkit |
| --- | --- | --- | --- |
| Value from another module used as an operand | D7 | Builder accepts `Value *`; verifier later reports `"Referencing ... in another module!"` | Operand type carries a generative module brand; wrong module is a compile error |
| Branch to a block from another module | D7 | Builder accepts `BasicBlock *`; verifier later rejects malformed control flow | Branch target carries the builder module's brand |
| Global initializer expression tied to another module | D7 | Constructor accepts `Constant *`; type is asserted, module provenance is not statically represented | `add_global` requires `Type<'ctx, B>` and `IsConstant<'ctx, B>` with the same `B` |
| Custom folder returns a value from the wrong module | D7 | Folder hooks return raw `Value *` | Folder hooks return `IrResult<Option<Value<'ctx, B>>>` |
| Insert after a terminated block | D1 | Insertion point is a mutable iterator into a `BasicBlock *` | Terminator builders consume the builder and return a `Terminated` view; retained `Unterminated` block copies remain verifier-backed |
| Return a value from a `void` function, or `ret void` from a value-returning function | D1, D4 | `CreateRet(Value *)` / `CreateRetVoid()` are just methods; mismatch is verifier/runtime state | `IRBuilder<..., R>` exposes return methods according to the function return marker |
| Read a typed result from a `void` call | D3, D4 | Caller must inspect the call/function type | `CallInst<'ctx, ()>` exposes no typed result accessor |
| Use an instruction handle after erase | D2 | Raw pointer discipline | Lifecycle methods consume a non-`Copy`, non-`Clone` `Instruction` handle |
| Recover lifecycle authority from a copyable value, block, or use-list | D2, D9 | Any retained `Instruction *` can be reused for mutation | Copyable rediscovery APIs return `InstructionView`; only builder output, `BlockCursor`, and detached reinsertion produce `Instruction<Attached>` |
| Add more incoming edges or destinations after a variable-arity instruction is finalized | D1, D2 | Caller discipline plus verifier | `PhiInst<Open>` / `SwitchInst<Open>` / `IndirectBrInst<Open>` / `LandingPadInst<Open>` / `CatchSwitchInst<Open>` are linear; `finish()` returns closed views without mutators |
| Run verified-only analyses after a transform | D8 | Verifier pass convention | A pass pipeline's output is `Module<Unverified>` whenever any member mutates (derived from the members' rungs), so verified-only analyses require an explicit `verify()` first |
| Pass mutates IR but reports everything preserved | D8, D1 | Pass returns a hand-written `PreservedAnalyses`; over-claiming leaves stale analyses that later passes miscompile against, caught only if a verifier/analysis-checker pass is opted in | Preservation is *derived* from the pass's capability rung, so over-claiming is a compile error: a mutating rung's `done()` floor is fixed by the rung, and `Access = Inspect` has no `mutate()` at all |
| Declare an analysis dependency | D8, D1 | Fallible `getResult` / `getCachedResult`; querying an undeclared or uncomputed analysis returns null and is undefined behavior | `type Requires` is prefetched, then read through the infallible `cx.analysis::<A, _>()`; an undeclared analysis has no `AnalysisSelector` impl, so the access is a compile error |
| External crate authoring a module pass | D8, D1 | `PassInfoMixin` plus manual plugin registration wiring | Implement `ModulePass` (or the `#[module_pass]` sugar), symmetric with function passes — no registration step |
| Author a pass with the wrong rung, no name, or an undeclared analysis | D1, D8 | `PassInfoMixin` + plugin registration; a wrong rung, missing name, or typo'd pipeline entry fails at plugin-load or run time, if at all | `#[function_pass]` / `#[module_pass]` expand to the trait impl and make each slip a pinpointed compile error (a module-only rung fails the `FnAccess` bound, a missing `name` is a `syn::Error`, an undeclared analysis fails a `#[diagnostic::on_unimplemented]` bound) |

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

`llvmkit` gives each `Module::with_new` session a fresh brand:

```rust
pub trait ModuleBrand: brand_sealed::Sealed + Copy + core::fmt::Debug + Eq + Hash {}

pub struct Brand<'id>(PhantomData<fn(&'id ()) -> &'id ()>);

pub fn with_new<N, R, F>(name: N, f: F) -> R
where
    N: Into<String>,
    F: for<'brand> FnOnce(Module<'brand, Brand<'brand>, Unverified>) -> R,
```

Values carry that brand:

```rust
pub struct Value<'ctx, B: ModuleBrand = Brand<'ctx>> {
    id: ValueId,
    module: ModuleRef<'ctx, B>,
    ty: TypeId,
}
```

The integer-add builder requires both operands to match the builder's brand `B`:

```rust
pub fn build_int_add<W, Lhs, Rhs, Name>(
    &self,
    lhs: Lhs,
    rhs: Rhs,
    name: Name,
) -> IrResult<IntValue<'ctx, W, B>>
where
    W: IntWidth,
    Lhs: IntoIntValue<'ctx, W, B>,
    Rhs: IntoIntValue<'ctx, W, B>,
```

Bad Rust program from the compile-fail suite:

```rust
Module::with_new::<_, _, _>("left", |left| {
    let left_value = left.i64_type().const_int(1_i64);

    Module::with_new::<_, _, _>("right", |right| {
        let i64_ty = right.i64_type();
        let fn_ty = right.fn_type(i64_ty.as_type(), Vec::<Type<'_, _>>::new(), false);
        let function = right.add_function::<i64, _>("f", fn_ty, Linkage::External).unwrap();
        let entry = function.append_basic_block(&right, "entry");
        let builder = IRBuilder::new_for::<i64>(&right).position_at_end(entry);

        let _ = builder.build_int_add(left_value, left_value, "bad");
    });
});
```

Result: compile error. The value from `left` cannot satisfy an operand bound for
`right`'s brand. No verifier pass, no fatal abort, no delayed broken module.

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

`IntoBasicBlockLabel<'ctx, R, B>` is implemented for both a bare
`BasicBlockLabel<'ctx, R, B>` and any `BasicBlock<'ctx, R, Term, B>`
(any termination state) -- but always parameterised over the SAME `B`
as the builder, so a target block minted under a different module's
brand has no impl to satisfy this bound at all.

Bad Rust program:

```rust
Module::with_new::<_, _, _>("left", |left| {
    let void_ty = left.void_type();
    let fn_ty = left.fn_type(void_ty.as_type(), Vec::<Type<'_, _>>::new(), false);
    let f = left.add_function::<(), _>("left_f", fn_ty, Linkage::External).unwrap();
    let left_target = f.append_basic_block(&left, "target");

    Module::with_new::<_, _, _>("right", |right| {
        let void_ty = right.void_type();
        let fn_ty = right.fn_type(void_ty.as_type(), Vec::<Type<'_, _>>::new(), false);
        let f = right.add_function::<(), _>("right_f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&right, "entry");
        let builder = IRBuilder::new_for::<()>(&right).position_at_end(entry);

        let _ = builder.build_br(left_target);
    });
});
```

Result: compile error. The branch target is not from the same branded module.

Limit: same-module CFG facts that depend on the complete graph, such as phi
predecessor completeness and dominance, still belong in `Module::verify()`.

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
pub fn add_global<N, C>(
    &self,
    name: N,
    value_type: Type<'ctx, B>,
    initializer: C,
) -> IrResult<GlobalVariable<'ctx, B>>
where
    C: IsConstant<'ctx, B>,
```

Bad Rust program:

```rust
Module::with_new::<_, _, _>("left", |left| {
    let left_init = left.i32_type().const_int(1_i32);

    Module::with_new::<_, _, _>("right", |right| {
        let i32_ty = right.i32_type();
        let _ = right.add_global("g", i32_ty.as_type(), left_init);
    });
});
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
pub trait IRBuilderFolder<'ctx, B: ModuleBrand = Brand<'ctx>> {
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
`IrResult<Option<IntValue<'ctx, W, B>>>`, so a custom folder cannot forge a
wrong *width* either -- the hook's signature pins it, and the builder accepts
typed fold results with no runtime re-check.

Bad Rust helper:

```rust
fn return_foreign_folder_value<'ctx, B: ModuleBrand>(
    foreign: Value<'ctx>,
) -> IrResult<Option<Value<'ctx, B>>> {
    Ok(Some(foreign))
}
```

Result: compile error. The unbranded/default-branded `foreign` value cannot be
returned as an arbitrary `Value<'ctx, B>`.

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
pub fn position_at_end(
    self,
    bb: BasicBlock<'ctx, R, Unterminated, B>,
) -> IRBuilder<'m, 'ctx, B, F, Positioned, R>
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
    let bb = self.insert_block();
    let inst = self.append_ret(None);
    (bb.retag_termination::<Terminated>(), inst)
}
```

After the terminator, the positioned builder has been consumed. Code that follows
the returned handle sees `Term = Terminated`, so `position_at_end` is not callable
on that handle. `BasicBlock` handles are `Copy` today; retaining an earlier
`Unterminated` copy can still spell a malformed append, and `Module::verify()`
remains the backstop for that escape hatch.

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
pub struct CallInst<'ctx, R: ReturnMarker = Dyn, B: ModuleBrand = Brand<'ctx>> {
    /* fields omitted */
}
```

Typed accessors are gated by `R`:

```rust
impl<'ctx> CallInst<'ctx, i32> {
    pub fn return_int_value(self) -> IntValue<'ctx, i32> {
        /* construct typed value handle */
    }
}
```

A `CallInst<'ctx, ()>` has no typed result accessor. The generic
`return_value()` method still exists and returns `None`, so a void call cannot be
mistaken for a typed value.

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
pub struct Instruction<'ctx, S: InstructionState = state::Attached, B: ModuleBrand = Brand<'ctx>> {
    /* fields omitted */
}
```

The handle is intentionally not `Copy` or `Clone`; lifecycle methods take `self`:

```rust
pub fn replace_all_uses_with<V: IsValue<'ctx, B>>(
    self,
    module_token: &Module<'ctx, B, Unverified>,
    replacement: V,
) -> IrResult<()>
```

```rust
pub fn erase_from_parent(self, module_token: &Module<'ctx, B, Unverified>)
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

LLVM C++ phi construction returns a mutable `PHINode *`:

```cpp
PHINode *CreatePHI(Type *Ty, unsigned NumReservedValues,
                   const Twine &Name = "") {
  PHINode *Phi = PHINode::Create(Ty, NumReservedValues);
  return Insert(Phi, Name);
}
```

`llvmkit` represents the editing window explicitly:

```rust
pub struct PhiInst<'ctx, W: IntWidth, P: PhiState = Open, B: ModuleBrand = Brand<'ctx>> {
    /* fields omitted */
}
```

Only an open phi accepts incoming edges:

```rust
pub fn add_incoming<V, R, S>(self, value: V, block: BasicBlock<'ctx, R, S>) -> IrResult<Self>
where
    V: IntoIntValue<'ctx, W, B>,
```

Calling `finish` returns a closed view:

```rust
pub fn finish(self) -> PhiInst<'ctx, W, Closed, B> {
    self.retag::<Closed>()
}
```

The closed view exposes read accessors, not `add_incoming`. The same linear
open/closed pattern is used for `switch`, `indirectbr`, `landingpad`, and
`catchswitch`: open handles are not `Copy`, mutators consume `self`, and
`finish()` returns a closed view. Rediscovery through `InstructionKind` /
`TerminatorKind` also returns closed variants, so it cannot reopen a finalized
variable-arity instruction.

## 10. Verification state is part of module type

LLVM C++ verification is a convention: a caller chooses whether to run
`verifyModule`, a verifier pass, or no verification at all.

`llvmkit` encodes the state:

```rust
Module<'ctx, B, Unverified>
Module<'ctx, B, Verified>
```

Verification consumes mutation capability and returns a verified token on
success. A pass pipeline's output typestate is *derived* from its members'
capability rungs: an all-read-only (`Inspect`) run preserves `Verified`, while
any mutating pass returns `Unverified`, forcing an explicit re-verification
before verified-only analyses or pass pipelines can consume the result (see
section 11).

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
pub trait FunctionPass<'ctx, B: ModuleBrand + 'ctx = Brand<'ctx>> {
    type Access: FnAccess; // Inspect | PatchBody | ReshapeCfg
    type Requires: FunctionAnalysisList<'ctx, B>;
    const NAME: &'static str;
    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, Self::Access, Self::Requires>)
        -> IrResult<FnReport>;
}
```

Two structural facts make over-claiming unspellable.

**(a) A read-only rung has no mutation door.** `Inspect` deliberately does not
implement `MutatingFn`, and `FnCx::mutate` exists only where `A: MutatingFn`. So
an `Inspect` context has no `mutate()` method at all — a pass declared read-only
cannot mutate, whatever its body attempts:

```rust
impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for InspectMutates {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "inspect-mutates";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport> {
        let patch = cx.mutate(); // no such method on an Inspect context
        Ok(patch.done())
    }
}
```

Result: compile error `error[E0599]: the method mutate exists ... but its trait
bounds were not satisfied: Inspect: MutatingFn`.

**(b) Reaching a mutator consumes the all-preserved report.** `FnCx::mutate`
takes `self` **by value**. Once a mutating pass has stepped into its mutator the
context is moved, so the all-preserved `cx.unchanged()` / `cx.done()` is gone.
The only report left is the mutator's own `done()`, which carries the rung's
derived floor. "Mutated, then claimed everything preserved" has no spelling:

```rust
    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        let _patch = cx.mutate(); // moves `cx` into the mutator
        Ok(cx.unchanged())        // use of moved value
    }
```

Result: compile error `error[E0382]: use of moved value: cx`.

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

Result: compile error — `RewriteModule` does not implement `FnAccess`, so a
function pass cannot even spell a module rung. In the same way, omitting `name`
is a `syn::Error` at the attribute ("missing name; a pass must declare its
NAME"), and reading an analysis the pass never listed in `requires` fails its
`#[diagnostic::on_unimplemented]` bound ("analysis ... is not in this pass's
Requires list"). Upstream, the analogous mistakes — a malformed `PassInfoMixin`,
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

## What llvmkit still verifies at runtime

`llvmkit` intentionally does not pretend every LLVM rule is local enough for the
type system. Runtime verification still owns:

- parsed or otherwise erased `Dyn` forms;
- dominance and cross-block SSA use checks;
- phi incoming set versus CFG predecessor set;
- complete terminator and reachability invariants after parser/pass mutation;
- data-layout-dependent size/alignment rules;
- verifier rules for attributes, globals, atomics, calls, EH pads, and metadata
  that depend on whole-instruction or whole-module context.

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
  Pass an explicit convention (`with_config` / `.calling_conv(..)`) to
  override. Parsed IR is unaffected: the parser stores exactly the convention
  the input spells.

## Proof in the repository

The compile-fail suite locks these guarantees with `trybuild`:

```rust
#[test]
fn typestate_compile_fail() {
    let t = trybuild::TestCases::new();
    // Brand / typestate locks:
    t.compile_fail("tests/compile_fail/cross_module_value_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_global_initializer_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_branch_target.rs");
    t.compile_fail("tests/compile_fail/cross_module_select_arm.rs");
    t.compile_fail("tests/compile_fail/custom_folder_wrong_brand.rs");
    // Pass API v2 locks (section 11):
    t.compile_fail("tests/compile_fail/inspect_pass_cannot_mutate.rs");
    t.compile_fail("tests/compile_fail/claim_preserved_after_mutate.rs");
    t.compile_fail("tests/compile_fail/undeclared_analysis_in_pass_body.rs");
    t.compile_fail("tests/compile_fail/function_pass_wrong_level_access.rs");
    t.compile_fail("tests/compile_fail/function_pass_missing_name.rs");
    t.compile_fail("tests/compile_fail/mutating_pass_cannot_enter_readonly_dyn.rs");
    /* more brand / typestate fixtures omitted */
}
```

Run the focused proof:

```bash
cargo test -p llvmkit-ir typestate_compile_fail
```

Those tests are intentionally not one-to-one ports of LLVM C++ tests. They are
`llvmkit`-specific type-safety locks for invariants that upstream LLVM represents
through raw pointers plus assertions, verifier diagnostics, or fatal verifier
passes.
