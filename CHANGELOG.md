# Changelog

Notable, user-visible changes to `llvmkit`. The format follows
[Keep a Changelog](https://keepachangelog.com/); the project is pre-1.0
(`0.0.x`), so breaking changes are expected and are flagged inline. Until a
tagged release is cut, entries accumulate under **Unreleased**.

## [Unreleased]

### No silent erasure — the strict cut

An erased `Value` / `Argument` / `Instruction` can no longer *silently* satisfy a
typed operand position, and a Rust numeric literal maps to exactly one IR width.
Erasure is still available, but it must be **spelled**.

#### Breaking

- Removed `Module::add_function::<R>(name, fn_ty, linkage)` — the constructor
  that paired an erased runtime signature with a static return marker, the
  one place a declaration could silently claim a return type its signature
  did not have (the marker check caught cross-kind lies at runtime, but the
  API's shape invited them). Declarations now split honestly:
  `add_typed_function::<Ret, Params, _>` derives the signature *from* the
  markers (a mismatch is unrepresentable; parameters come back typed), and
  `add_function_dyn` takes a runtime `FunctionType` and returns
  `FunctionValue<Dyn>`. To re-type a function declared erased, use the
  checked `function_by_name::<R>` lookup. One deliberate escape hatch
  remains: `function_builder::<R>(name, fn_ty)` (the attribute/linkage-rich
  declaration path) still pairs a user-supplied signature with a chosen
  marker and keeps the runtime `ReturnTypeMismatch` gate at `.build()`.
  Locked by `tests/compile_fail/add_function_removed.rs`.
- Removed the erased-handle lifts from `IntoIntValue`, `IntoFloatValue`, and
  `IntoPointerValue`. An erased `Value` / `Argument` / `Instruction` no longer
  fills a typed operand slot on its own; narrow it explicitly first — e.g.
  `let p: PointerValue = v.try_into()?;` (or `IntValue::<W>::try_from` /
  `FloatValue::<K>::try_from`) — or use the erased `_dyn` builder family. The four
  conversion traits (`IntoIntValue`, `IntoFloatValue`, `IntoPointerValue`, and,
  transitively, `IntoCallArg`) are now **sealed**: their set of accepted operand
  sources is closed and cannot be extended downstream.
- Removed the implicit literal-widening impls. A Rust integer literal now maps to
  exactly one IR width (`2i32` is `i32`; `2i64` is `i64`) and a Rust float to
  exactly one kind (`f32` / `f64`), with no silent widening (`i8 -> i32`,
  `f32 -> f64`). A literal in a wider slot must name its width, e.g. `2_i64`. The
  Rust-scalar → `Width<N>` lifts were removed for the same reason; a `Width<N>`
  slot takes a typed `IntValue<Width<N>>` / `ConstantIntValue<Width<N>>`, not a
  bare literal.

#### Improved

- As a direct consequence of the above, `build_int_add(2i32, 3i32, "n")` now
  infers its width with **no turbofish** and no annotation: with a single width
  per literal, the operand marker `W` has exactly one solution.
- The bitcast builders (`build_bitcast_int_to_int`, `build_bitcast_int_to_fp`,
  `build_bitcast_fp_to_int`, `build_bitcast_fp_to_fp`), `build_atomic_cmpxchg`,
  and `build_ui_to_fp_with_flags` drop their now-redundant operand-lift generic:
  with one-literal-one-width and sealed conversions, the lift bought only
  "accept a bare literal in place of a typed handle", dead weight for these
  computed-SSA operands. The methods now take the concrete typed operand
  directly, so e.g. `build_bitcast_int_to_int(v, i8_ty, "bc")` needs no
  turbofish. Printed IR is unchanged.

### Pass surface (cycle D)

#### Breaking

- Removed `FnCx::unchanged` / `ModCx::unchanged` — verbatim duplicates of the
  `done()` on the same contexts (identical bodies, identical semantics, two
  names for one operation). Migration: `cx.unchanged()` → `cx.done()`. The
  honesty lock is unchanged: `done()` also takes `self` by value, so calling
  it after `mutate()` is the same use-of-moved-value error.
- The function-rung mutators no longer hand out the module's *declaration*
  capability: `FnPatch::module_mut` is crate-internal and
  `FnReshape::module_mut` is removed. Through them, a pass declared at
  `PatchBody`/`ReshapeCfg` could `add_global` / `add_function_dyn` /
  `set_struct_body` while still reporting only its body-level preservation
  floor — the one rung-honesty leak left in the surface. The boundary that
  replaces them: **type construction is preservation-neutral** (it only interns
  into the context; no function, global, or CFG changes), so the read-only
  view reached by `FnPatch::module()` / `FnReshape::module()` now carries the
  type-constructor surface — while *declarations* stay exclusive to
  `ModRewrite::module_mut`, whose `RewriteModule` floor is already `none()`.
  Locked by `tests/compile_fail/function_rung_cannot_declare_globals.rs`,
  which pins the boundary, not a ban: in one fixture, minting a type through
  `patch.module()` compiles and `patch.module_mut()` is private.
- `FnReshape::insert_phi` is now the **typed** phi inserter and
  `insert_phi_dyn` is the erased twin (the naming law: bare = typed, `_dyn` =
  erased type). The typed `insert_phi<V>(block, incomings: &[(V, label)])`
  takes same-typed incomings (`IntValue<i32>`, `PointerValue`, …) and returns
  that same handle `V` — so a wrong-typed incoming is a compile error and the
  type is derived from the incomings rather than restated as a `Type` argument
  (it needs ≥1 incoming for that; the zero-incoming case stays on
  `insert_phi_dyn`). `insert_phi_dyn(block, ty, incomings: &[(Value, label)])`
  is the previous erased signature verbatim, renamed. The completeness and
  dominance obligations are witnessed at the call by both, exactly as before —
  only the per-incoming type-agreement moves to compile time.

- `ModRewrite::for_each_function::<FnA>(visitor)` is replaced by two
  rung-named **iterators**: `patch_functions()` (yields `FnPatch`) and
  `reshape_functions()` (yields `FnReshape`). External iteration is the
  idiomatic shape the closure visitor could not be: `?`, `continue`, and
  `break` just work, the rung is the method name instead of a turbofished
  access marker, and the iterator borrows nothing from the mutator, so
  `module_mut()` stays callable mid-loop (a global per patched function is
  the sanitizer shape). Same semantics otherwise: definitions in module
  order, declarations skipped, per-function `Requires` still `()`. The
  doc-hidden `MutatingFn::mutator_over_module` plumbing (sealed trait) went
  with it. The *pipeline adaptor* `for_each_function(function_pipeline((..)))`
  is a different item and is unchanged.

#### Added

- `FnPatch::builder_at(ip)` / `FnReshape::builder_at(ip)` — a positioned
  `IRBuilder` over the mutator's function, replacing the one legitimate use
  the removed `module_mut` escape had.
- `PatchFunctions` / `ReshapeFunctions` — the named iterator types behind
  `patch_functions()` / `reshape_functions()`, public like the other pass-API
  iterators (`ModuleFunctionViews` precedent).

### Idiomatic surface (cycle C)

#### Breaking

- `IsValue::as_value` is renamed **`into_erased`**, and the 20 inherent
  by-reference wideners on the linear (`!Copy`) handles — `BasicBlock`,
  `BasicBlockLabel`, `Instruction`, and the typed instruction handles — become
  **`to_erased`**. Erasure is the subject of this release, so the ~1500 sites
  that perform it now spell it; the `into_`/`to_` split follows the Rust
  convention that `into_*` consumes (owned → owned) while `to_*` widens from a
  borrow, which matters here because those handles are deliberately non-`Copy`
  so that their *lifecycle* methods consume. Migration is mechanical:
  `x.as_value()` → `x.into_erased()`, or `x.to_erased()` if `x` is one of the
  linear handles (the compiler names the right one).
- `Module::function_by_name` (erased; returns `Option<FunctionValue<Dyn>>`) is
  renamed `function_by_name_dyn`, and the checked marker-narrowing
  `function_by_name_typed::<R>` takes over the bare name as
  `function_by_name::<R>` — the naming law: typed variant bare, erased variant
  `_dyn`.
- `Module::set_struct_body` (erased; takes `StructType<StructBodyDyn>`) is
  renamed `set_struct_body_dyn`, and the typestate `set_struct_body_typed`
  (consumes `Opaque`, yields `BodySet`) takes over the bare name as
  `set_struct_body` — the naming law: typed variant bare, erased variant
  `_dyn`.
- `ModuleView::iter_functions` / `iter_globals` / `iter_aliases` /
  `iter_ifuncs` / `iter_comdats` and `Module::iter_globals` become
  `functions()` / `globals()` / `aliases()` / `ifuncs()` / `comdats()` —
  `iter_` prefixes dropped for idiomatic Rust names.

#### Added

- `IsValue::id()` — the arena id of any value handle, previously reachable only
  by widening first (`x.as_value().id`). Every value handle now answers `id()`
  directly, including the seven linear handles that cannot implement `IsValue`
  (`Instruction`, `NonTerminator`, `BasicBlock`, `BasicBlockLabel`, `PhiInst`,
  `FpPhiInst`, `PointerPhiInst`), which carry it as an inherent method.
- `IntoIterator` for `ModuleView`, `FunctionView`, `BasicBlockView` and
  `FunctionValue`, so a nest of `for` loops walks the IR directly:
  `for f in module_view { for bb in f { for inst in bb { .. } } }`. `ModuleView`
  iterates *functions*, mirroring LLVM's `for (Function &F : M)`. The named
  methods (`functions()`, `basic_blocks()`, `instructions()`) remain — the trait
  is sugar beside them. Their iterator types are public:
  `FunctionBasicBlocks`, `FunctionBasicBlockViews`, `BlockInstructionViews`.
- `PhiKind::incomings()` plus mirrors on the four typed phi handles — an
  iterator of `(value, block)` pairs, matching the shape `SwitchInst::cases()`
  already had. The indexed `incoming_count()` / `incoming(i)` remain.
- Around 30 iterator-returning methods now also promise `DoubleEndedIterator`
  and `FusedIterator`. The bodies always supported both; the opaque return type
  was hiding it, so reverse iteration over blocks and instructions now works.
  (`ModuleView`'s `IntoIterator` sugar is the one exception — its boxed inner
  iterator cannot offer `DoubleEndedIterator`; use `functions().rev()`.)
- `Display` for 18 public value handles and for `ApInt`. Value handles print
  their operand form and agree with the erased path by construction; the
  module-level globals (`FunctionValue`, `GlobalVariable`) print their
  *definition* line instead, following the existing `GlobalAlias` /
  `GlobalIFunc` precedent. Each impl documents which form it prints.
- `BasicBlockView` is now `Copy`, matching its sibling `FunctionView`.

#### Deliberate law exceptions

- `add_typed_function` keeps its name: `add_function` stays vacated as the
  migration tombstone — a removed method's E0599 with a did-you-mean beats
  confusing arity errors on a reused name (locked by
  `tests/compile_fail/add_function_removed.rs`).
- The `const_*` constructor family is conformant as-is: witness-generic
  constructors, not typed/erased pairs.
- The `append_block` family has no bare-named erased sibling — no violation.
- `build_bitcast_dyn` / `build_phi_dyn` are by-design `_dyn` orphans.

### Declaration surface — globals derive their type from the initializer

`Module::add_global` / `add_global_constant` no longer take a separate
`value_type`: the global's type is derived from its initializer, and the
initializer is now any `IntoConstantValue` — an existing constant handle **or a
Rust scalar literal**. The motivating call `add_global("marker", 0i32)` now
compiles with no type handle and no `.as_type()`.

#### Added

- `IntoConstantValue<'ctx, B>` — a value usable as a constant initializer: a
  blanket impl over every `IsConstant` handle, plus one impl per exact Rust
  scalar width (`bool`, `i8`..=`i128`, `u8`..=`u128`, `f32`, `f64`). One literal
  maps to exactly one IR width (no widening): `0i32` is an `i32`, `0i64` an
  `i64`. The scalar impls reuse the existing `IntoConstantInt` /
  `IntoConstantFloat` machinery.
- `Module::add_global_uninitialized(name, value_type)` — the declaration-only
  case (no initializer to derive from), using the module's default linkage.
  Accepts `impl Into<Type>`, so a typed handle needn't be widened via
  `.as_type()`; `add_external_global` gains the same ergonomic.
- `IrError::DuplicateGlobalName` — installing a global variable, alias, or ifunc
  whose name is already bound at module scope now reports this instead of the
  misused `DuplicateFunctionName`. One variant covers all three global-scope
  symbol kinds (they share the module's global namespace).
- `IRBuilder::at_end(bb)` and `BasicBlock::builder()` — a builder positioned at a
  block with the return marker inferred from the block, so
  `IRBuilder::new_for::<R>(&m).position_at_end(bb)` collapses to
  `IRBuilder::at_end(bb)` (no turbofish). `new_for` retained for building blocks
  before positioning.
- `Module::fn_type_no_params(ret, is_var_arg)` — a no-parameter function type
  without the empty-`Vec::<Type>::new()` inference cliff of `fn_type` (with an
  empty iterator the element type can't be inferred). It is exactly
  `fn_type(ret, [], is_var_arg)` with the element type pinned.
- `Module::add_function_dyn(name, signature, linkage)` — the honest *erased*
  function-declaration path: it takes a runtime `FunctionType` and returns a
  `FunctionValue<Dyn>`, carrying no static return marker and running no
  return-marker check (`Dyn` matches every signature by definition). This is the
  path for the `.ll` parser and other runtime-schema-driven tooling. For
  statically-typed authoring, prefer the typed primary
  `add_typed_function::<Ret, Params>(name, linkage)`: its turbofish *is* the
  schema (no separately built `FunctionType`), and the parameters come back
  already typed through `f.params()`. The erased
  `add_function::<R>(name, fn_ty, linkage)` — erased signature, typed return —
  stays; migrating its remaining call sites is deferred to the strict-cut cycle.

#### Changed

- **Breaking:** `add_global` / `add_global_constant` drop the `value_type`
  parameter and take `initializer: impl IntoConstantValue`. Migrate
  `add_global("g", ty.as_type(), init)` to `add_global("g", init)`. The
  redundant creation-time `TypeMismatch` (initializer type vs declared type) is
  gone — it is now unrepresentable, since the type *is* the initializer's.
  `GlobalVariable::set_initializer` keeps its type check: a *replacement*
  initializer must still match the global's frozen type. On the low-level
  `global_builder(name, ty).initializer(c)` escape hatch — where `ty` and `c`
  remain independent — a mismatch now surfaces at `verify()`
  (`GlobalInitializerTypeMismatch`) rather than eagerly at `build()`.
- Aggregate constant constructors `ArrayType::const_array` /
  `StructType::const_struct` / `VectorType::const_vector` now accept
  `impl IntoConstantValue` elements, so Rust literals work
  (`const_array([1i32, 2, 3])`). The blanket `IntoConstantValue for IsConstant`
  impl keeps existing constant-handle callers unchanged. They stay **fallible**
  (`IrResult`): the element-vs-container type check is still needed because the
  receivers are erased (`ArrayType<ElemDyn, ArrLenDyn>`, etc.).

### Unforgeable markers — the builder's typed-append family (internal)

Internal refactor of *how* an int / float / pointer marker is attached to a
freshly-appended instruction; **no public API change and byte-identical printed
IR**. Marker attachment across the builder's append surface now flows through a
typed-append constructor family — `append_int_like` / `append_int_at` /
`append_int_load`, the `append_fp_*` trio, and `append_ptr` / `append_ptr_load` —
each of which appends the instruction *at* a typed type-handle and re-wraps the
result, so the width / kind / pointer-ness matches the runtime type **by
construction** rather than by an implicit proof beside each call. This collapses
~40 scattered `from_value_unchecked` wraps (casts, comparisons, loads, alloca /
GEP, scalar arithmetic) onto the family.

#### Changed

- `from_value_unchecked`'s in-crate callers in `ir_builder.rs` drop from ~40
  scattered wraps to the 8 constructor-family bodies plus a legible residual
  (runtime-checked fold seams, the select-arm re-wrap, the `ptrtoaddr` `IntDyn`
  re-wrap, and the vector / array append wraps that have no typed constructor
  yet). The Cycle-1 runtime re-checks (`accept_folded_*` / `narrow_folded_*` /
  `def_*_var`) stay as defense in depth.
- **Audited, not sealed.** `from_value_unchecked` remains `pub(crate)` — a hard
  compile-time seal is infeasible (`value` and `ir_builder` are sibling modules
  and the constructors need `ir_builder`-private helpers), so the confinement is
  documented and locally proven, not compiler-enforced. `IntDyn` / `FloatDyn`
  markers still name no width / kind by design (erasure); the family proves
  integer- / float-ness structurally, and the width is simply not part of what
  the erased marker claims.

### Phi guarantees — wave 1

Pushes the *local*, statically- or parse-time-knowable phi invariants into
construction and parsing, so many malformed-phi shapes are rejected before
`Module::verify()` ever runs. Whole-graph facts — dominance and phi-incoming
completeness against the final predecessor set for builder-constructed IR —
remain owned by `Module::verify()` as the final gate (defense in depth).

#### Added

- `IrError::AmbiguousPhiIncoming` — all four phi edge-add paths now reject a
  second incoming for the same predecessor block that carries a *different*
  value. Same-value duplicates stay legal, since a `switch` with several edges
  from one predecessor relies on them. **Stricter:** this conflict was
  previously deferred to `verify()`. In the same change, the untyped
  `phi_add_incoming_from_value` (parser / SSA-builder path) now type-checks the
  incoming value at the call site instead of deferring the type mismatch to
  `verify()`.
- `m_phi()` matcher (binds `PhiKind`), and an InstSimplify fold that rewrites a
  uniform phi — every incoming a single value, self-references permitted — to
  that value.

#### Changed

- **Behavior change:** the six `build_*_phi` builders now insert at the block's
  PHI head regardless of the builder's cursor position, so phi misplacement is
  unrepresentable through the builder (the verifier's `PhiNotAtTop` check stays
  as defense in depth). *Side effect:* the auto-SSA builder's header-phi
  emission order for blocks with two or more header phis changed from
  reverse-creation order to creation order. This is cosmetic — all IR still
  verifies — but any consumer byte-locking auto-SSA output will observe the new
  order.
- **Stricter parsing:** the `.ll` parser now rejects a `phi` that appears after
  a non-phi instruction with the parse error *"phi must be grouped at the top of
  its basic block"*, instead of silently letting the auto-hoisting builder
  reorder ill-formed input.
- **Stricter parsing:** the `.ll` parser now checks phi *completeness* at
  end-of-function parse — once all predecessors are known — and reports
  incomplete or incoherent phis as source-located parse errors. The parser and
  the verifier share one `check_phi_incoming` helper, so parse-time and
  verify-time diagnostics cannot drift apart. Previously these were deferred to
  `verify()`.

#### Fixed

- `FnReshape::split_block` now rewrites successor-block phi incomings as part of
  the split. Previously a correct `ReshapeCfg` pass that split a block with a
  phi successor produced IR that failed `verify()` with `PhiPredecessorMismatch`;
  the split now maintains successor phis itself.

### Phi authoring — block arguments and pass-side edits

A Swift-SIL / MLIR-style block-argument authoring surface where a branch
carries the values for its successor's parameters, so the edge and its phi
incomings move together and can never desync. Plus dominance-witnessed
pass-side phi creation and edge edits that maintain successor phis
mechanically. (Wave-2 additions; the raw phi builders were subsequently made
internal — see "Phi authoring — raw builders internal" below.)

#### Added

- Block-argument authoring: `IRBuilder::append_block_with_params(function,
  &[Type], name)` creates a block whose parameters are operandless head-phis
  and returns the block plus one `Value` per parameter. `build_br_with_args` /
  `build_cond_br_with_args` build the terminator *and* seed each successor
  parameter with the value the branch carries, from the current block —
  arity-checked (`IrError::PhiArgArityMismatch`) and type-checked at the call
  site — those two validations are all-or-nothing (run up front, before any
  incoming is recorded). Printed IR is ordinary phis; storage/parser/printer
  are unchanged.
- `FnReshape::insert_phi(block, ty, incomings)` — pass-side phi creation that
  *witnesses* everything at the call: completeness against the block's
  predecessors, incoming types, differing-duplicate rejection (via the shared
  `check_phi_incoming`), and SSA dominance of each instruction incoming over
  its edge, read from the pass's dominator tree
  (`analysis_repaired::<DominatorTreeAnalysis>`). `IrError::PhiIncomingNotDominating`
  on a dominance failure.
- `FnReshape::remove_edge` / `redirect_edge` drop or retarget a CFG edge and
  mechanically maintain the affected
  successors' phis as part of the op — `remove_edge` drops the predecessor's
  incomings, `redirect_edge` takes the new target's per-parameter values as a
  required, type-checked argument, so "forgot the target's phis" cannot occur.
  Both record `CfgUpdate`s for the analysis-preservation machinery.

#### Changed

- **Wider parsing:** the `.ll` parser now accepts vector and aggregate phi
  result types (`phi <4 x i32>`, `phi {i32, i8}`) — previously rejected as
  "must be int, float, or pointer". Non-data first-class types (`label`,
  `metadata`, `token`) are still rejected, so no invalid IR slips through.

### Phi authoring — raw builders internal (breaking)

Completes the block-argument transition: block arguments are now the *only*
public way to author a phi, so an incomplete or predecessor-desynced phi is
unrepresentable through the public API rather than merely rejected at
`Module::verify()`.

#### Added

- `IRBuilder::append_block_with_named_params(function, &[(Type, &str)], name)`
  names each block parameter's head-phi, so block-argument authoring reproduces
  named-phi output byte-for-byte (e.g. the hand-written factorial's `%acc`/`%i`
  loop-header phis, which keep byte-parity with the auto-SSA factorial).

#### Changed

- **Breaking:** the three marker-form builders `IRBuilder::build_int_phi` /
  `build_fp_phi` / `build_pointer_phi` and the `PhiInst` / `FpPhiInst` /
  `PointerPhiInst` open-phi `add_incoming` / `finish` mutators are no longer
  public (`pub(crate)`). (The runtime-typed `build_int_phi_dyn` /
  `build_fp_phi_dyn` / `build_pointer_phi_in_addrspace` forms and the untyped
  `phi_add_incoming_from_value` stay reachable, but only as `#[doc(hidden)]`
  internal-contract items for the `.ll` parser — not supported public API.)
  Author phis with block arguments instead — the edge and its incomings move
  together, so desync is unrepresentable rather than deferred to `verify()`:

  | Was (no longer public) | Now (public) |
  | --- | --- |
  | `let p = b.build_int_phi::<i32, _>("p")?;` then `p.add_incoming(v0, pred0)?.add_incoming(v1, pred1)?;` | `let (blk, params) = b.append_block_with_params(f, &[i32_ty], "join")?;` then from each predecessor `b.build_br_with_args(blk.label(), &[v])?;`; the phi is `params[0]` |
  | naming the phi: `build_int_phi::<i32, _>("acc")` | `append_block_with_named_params(f, &[(i32_ty, "acc")], "join")` |
  | pass-side phi creation | `FnReshape::insert_phi(block, ty, incomings)` (unchanged) |

  The read surface (`PhiKind`, `incoming`, `incoming_count`, the `m_phi`
  matcher) is unchanged, and the `.ll` parser is unaffected (it reaches the
  builders through `#[doc(hidden)]` internal-contract entry points). The phi
  storage, printer, and verifier are unchanged — printed IR is still ordinary
  phis.

### Phi — verifier result-type rule and branch edge ops

The last two deferred phi-authoring items.

#### Added

- `VerifierRule::PhiInvalidResultType` — `Module::verify()` now rejects a phi
  whose *result* type is not a first-class **data** type (int, float, pointer —
  the opaque `ptr` and the legacy typed `i32*` — vector, array, non-opaque
  struct). Previously only the `.ll` parser enforced this, so a phi with a
  `token` / `label` / `metadata` / `void` result built through another path (the
  internal erased phi builders take an arbitrary `Type`) verified clean. Defense
  in depth: the guarantee now holds regardless of construction path. **Stricter
  `verify()`**, though only for IR that was already invalid. `VerifierRule` is
  `#[non_exhaustive]`, so the new variant is not a breaking change. Adds
  `Type::is_typed_pointer` alongside `Type::is_pointer` (which matches only the
  opaque `ptr`).
- `FnReshape::remove_edge` / `redirect_edge` now operate on **`br` and `cond_br`**,
  not just `switch`. `redirect_edge` retargets the unconditional `br` target or
  the matching arm of a `cond_br`; `remove_edge` collapses a `cond_br` to
  `br <surviving>` when one of its two edges is dropped, deregistering the
  now-dead condition operand. `BranchInstData.kind` became interior-mutable (a
  `RefCell<BranchKind>`, mirroring `SwitchInstData`'s `Cell`/`RefCell`), so the
  reshape mutator — which reaches instructions only through the arena's shared
  `&ValueData` — can edit branch targets and the branch *kind*. Removing the sole
  edge of an unconditional `br` is rejected (no successor would remain).
  `invoke`/`callbr` edges remain uneditable — see `docs/future-work.md`.

#### Changed

- **Stricter parsing:** the `.ll` parser now rejects a phi whose result type is
  an **opaque struct** (`phi %opaque`). It previously accepted it — contradicting
  its own comment — and produced IR that then failed `Module::verify()`. The
  parser and the verifier now accept exactly the same set of phi result types.

#### Fixed

- `FnReshape::remove_edge` / `redirect_edge` no longer leave a **zero-incoming
  phi** behind. When the removed edge was a block's *only* incoming edge, its
  head phis lost their last incoming and were left as `%p = phi i32` with no
  `[ … ]` pairs — a form LLVM's own LL parser rejects, so the module no longer
  round-tripped (even though `Module::verify()` accepted it, the count matching
  a now-zero-predecessor block). Both ops now mirror LLVM
  `BasicBlock::removePredecessor`: an emptied phi is replaced with poison (of
  its own result type) and erased, so the result round-trips. (A companion
  *defensive* verifier rule — a phi in a reachable block must carry at least one
  incoming — is tracked separately in `docs/future-work.md`.)

### Phi — zero-incoming verifier backstop

The companion defensive verifier rule to the round-trip fix above.

#### Added

- `VerifierRule::PhiEmptyInReachableBlock` — `Module::verify()` now rejects a
  phi that carries **zero** incoming values in a block **reachable from entry**,
  however the phi arose. Such a phi prints as `%p = phi i32` with no `[ … ]`
  pairs — a form `LLParser::parsePHI` rejects, so the module no longer
  round-trips. The shared `check_phi_incoming` count guard misses this: a
  zero-incoming phi in a zero-predecessor block passes on `0 == 0` (the same gap
  LLVM's `Verifier::visitPHINode` shares). The new check runs before that
  delegation and gates on `DominatorTree::is_reachable_from_entry` — an
  unreachable block may legitimately have no predecessors, so its phis are not
  forced to carry incomings. The public mutation path (the typed edge-edit ops —
  see the breaking entry below) already erases such phis; this backstop catches
  any other construction path. **Stricter `verify()`**, though only for IR that
  has no
  legal textual form. `VerifierRule` is `#[non_exhaustive]`, so the new variant
  is not a breaking change.

### Phi — typed terminator edit surface (breaking)

Replaces the dynamic CFG-edge ops with a typed edit surface whose method set
encodes which edits are legal, so a structurally-invalid edge edit is a compile
error instead of a runtime rejection. Same single-validated phi/edge maintenance
underneath.

#### Added

- `FnReshape::edit_terminator(from)` narrows a block's terminator into a typed
  edit handle whose *type* fixes the legal edge ops, plus the `dyn_cast`-style
  narrows `edit_switch` / `edit_cond_br` / `edit_br` / `edit_invoke` /
  `edit_callbr`:
  - `SwitchEdit`: `redirect_successor` / `redirect_default` / `remove_successor`
  - `CondBrEdit`: `redirect_then` / `redirect_else` / `remove_then` / `remove_else`
  - `BrEdit`: `redirect`
  - `InvokeEdit`: `redirect_normal` / `redirect_unwind`
  - `CallBrEdit`: `redirect_default` / `redirect_indirect`

  `edit_terminator` returns the `TermEdit` enum (with an `Uneditable` arm for
  `ret` / `unreachable` / `indirectbr` and the EH terminators). Each op runs
  through the same single-validated path as before: successor phis are maintained
  mechanically, and an emptied phi is poison-erased for LLVM `removePredecessor`
  parity.
- First-class `invoke` / `callbr` edge redirects (`redirect_normal` /
  `redirect_unwind`, `redirect_default` / `redirect_indirect`) retarget those
  mandatory successor edges in place — the last deferred phi follow-up, now
  shipped.

#### Removed

- **Breaking:** the dynamic `FnReshape::remove_edge` / `redirect_edge` are gone;
  use the typed narrows above. The migration is mechanical:
  `remove_edge(from, to)` → `edit_switch(&from)?.remove_successor(&to)` (switch)
  or `edit_cond_br(&from)?.remove_then()` / `.remove_else()` (cond_br, pick the
  arm whose target is `to`); `redirect_edge(from, old, new, vals)` →
  `edit_switch(&from)?.redirect_successor(&old, &new, vals)` /
  `.redirect_default(&new, vals)` (switch),
  `edit_cond_br(&from)?.redirect_then` / `.redirect_else(&new, vals)` (cond_br),
  or `edit_br(&from)?.redirect(&new, vals)` (unconditional `br`).

#### Changed

- **Structurally-invalid edge edits are now compile errors, not runtime
  rejections.** Removing an `invoke` / `callbr` edge, the sole edge of an
  unconditional `br`, or a `switch` default is unspellable — the method simply
  does not exist on the corresponding handle (`E0599`). A second `cond_br`
  collapse is a use-after-move, since `remove_then` / `remove_else` consume the
  handle (`E0382`).
- **Semantic change:** collapsing a `cond_br` whose *both* arms target the same
  block is now valid. The old `remove_edge` rejected it as ambiguous; the
  role-named `remove_then` / `remove_else` name the arm, so the collapse to
  `br <survivor>` is unambiguous.

### Phi authoring — typed block parameters

Lifts a block's *parameter shape* into the Rust type system, so a branch that
carries the wrong number of block-arguments — or a right-count-but-wrong-typed
argument — is a **compile error** rather than an `IrError::PhiArgArityMismatch`
/ type mismatch surfaced at the call site. The block analog of the const-generic
vector/array retrofit below: typing is **opt-in** through a defaulted marker, so
every existing erased branch/edge call keeps compiling and printing identical IR.

#### Added

- `BlockParams` sealed marker trait and its erased inhabitant `BlockParamsDyn`
  (`block_params.rs`), plus a **last, defaulted** `Params` type parameter on
  `BasicBlockLabel` and `BasicBlock` (`…, Params: BlockParams = BlockParamsDyn`).
  Because the new parameter defaults to the erased marker, every existing handle
  spelling is unchanged; a label recovered from an untyped `Value` still lands on
  `BlockParamsDyn`.
- `IRBuilder::append_block_typed::<Params>(function, name)` — the typed sibling
  of `append_block_with_params`. `Params` is a `FunctionParamList` tuple (the
  same schema that types a function's parameter list, e.g. `(i32, Ptr)`); the
  call returns the block *stamped* with `Params` plus a typed tuple of parameter
  handles sourced from the block's operandless head-phis (`Params` position `i`
  is parameter `i`'s handle and carries its IR type). The parameter IR types are
  built before the block is appended, so a construction failure leaves no
  half-built block behind.
- `BlockCall<'ctx, R, B, Params>` — a typed branch edge bundling a typed target
  label with the block-arguments that seed its head-phis, built via
  `head.call(args)` (on a typed `BasicBlockLabel` or `BasicBlock`) where `args`
  satisfies `CallArgs<Params>`. A **wrong arity or a wrong-typed argument
  position is a compile error**, reusing the exact machinery of a typed
  `build_call`. `IRBuilder::build_br_call` / `build_cond_br_call` consume a
  `BlockCall` (the latter one per arm — the two arms may carry different
  schemas), seed the target's head-phis with the compile-checked arguments, and
  emit the branch. Any *value-level* lowering failure (e.g. a cross-module
  constant) is deferred into the `BlockCall` and surfaced as `IrResult` at build
  time.
- Typed parameter tuples are capped at **arity 12**: `BlockParams` carries a
  `Debug` supertrait and the standard library stops deriving `Debug` on tuples
  past arity 12, so a `>12`-arity tuple is rejected with a `BlockParams`-
  unsatisfied bound error. Beyond twelve parameters, author the block with the
  erased `append_block_with_params` (`BlockParamsDyn`) form. The whole erased
  authoring surface — `append_block_with_params` /
  `append_block_with_named_params`, `build_br` / `build_br_with_args` /
  `build_cond_br_with_args` — is **unchanged** and still produces `BlockParamsDyn`
  handles.

### No silent erasure — marker-generic narrowing and type checks at every marker

Closes a gap between llvmkit's typed handles and the checks behind them. A typed
handle (`IntValue<'ctx, i32, B>`, `FloatValue<'ctx, f64, B>`) is a *claim* about
a value's runtime type; several seams that consume such handles trusted the
claim instead of checking it, but only when the marker was static — exactly the
case where a wrong claim is invisible. They now check unconditionally, and where
a check does fire, the error names what actually differs.

#### Added

- `IntWidth::narrow` / `FloatKind::narrow` — narrow an erased `Value` to a typed
  `IntValue<'ctx, W, B>` / `FloatValue<'ctx, K, B>` behind a **bare**
  `W: IntWidth` / `K: FloatKind` bound, returning `IrResult`. Every impl
  delegates to the matching per-marker `TryFrom<Value>`, so the error split is
  inherited, not restated (right kind + wrong width → `OperandWidthMismatch`;
  wrong kind → `TypeMismatch`; `IntDyn` / `FloatDyn` accept any width / kind).

  What is new is the *bound*, not the capability: those `TryFrom` impls could
  already be reached from generic code by propagating a
  `where IntValue<'ctx, W, B>: TryFrom<Value<'ctx, B>>` clause through every
  downstream signature. `narrow` makes the same narrowing callable from a bare
  marker bound, and is expressible where that route is not — namely inside a
  trait impl, whose signature is fixed for you and cannot take the extra clause.
- `IrError::AddressSpaceMismatch { expected, got }` — a pointer-vs-pointer type
  drift now names both address spaces. `IrError` is `#[non_exhaustive]`, so this
  is **not** a breaking addition.

#### Fixed

- **Behaviour change:** four fold-result acceptors (`accept_folded_int` /
  `accept_folded_fp` / `accept_folded_cast_int` / `accept_folded_cast_fp`) and
  two auto-SSA variable-def seams (`SsaBuilder::def_int_var` /
  `def_float_var`) compared the value's runtime type against the expected type
  **only for the erased `IntDyn` / `FloatDyn` markers**. At every static marker
  the compare was skipped, on the rationale that the handle's type already
  proved the width — which is circular, since `from_value_unchecked` exists
  precisely to mint that claim *without* consulting the payload. A wrong-typed
  fold result or variable def was therefore **silently accepted** at a static
  width: an `IntValue<'_, i32>` really carrying an `i64` could escape to a
  caller, or land in an `i32`-pinned variable that a later `use_int_var` reads
  back at the wrong type. All six now compare at every marker.

  **No released behaviour was wrong.** `from_value_unchecked` is
  crate-internal, so only in-crate code could mint the contradicting handle:
  an externally-authored folder cannot reach the hole at all (its typed hooks
  are compile-time barred — `tests/compile_fail/folder_typed_wrong_width.rs`),
  and the shipped `ConstantFolder` produced correct types throughout. This
  closes a **latent in-crate channel**, not an active miscompile. The cost is
  one interned-`TypeId` compare per accepted fold or def, and no correct
  program is newly rejected: types are interned by width / kind / address
  space, so equality of `TypeId` *is* structural type equality.

  Two supporting changes make that claim checkable rather than asserted:
  `ConstantFolder`'s nine typed hooks now `narrow` the results of their erased
  siblings instead of re-wrapping them unchecked behind a prose audit of the
  fold kernel, turning the audit into a proof at the point of construction; and
  the four hand-rolled "is this the type it claims?" compares collapsed into a
  single core, `Type::require_match`, which carries the comparison, the error
  shape and the rationale in one place.

#### Changed

- **Breaking:** `IRBuilder::build_switch` is now the **typed** builder (was
  `build_switch_typed`), and the width-erased one is `build_switch_dyn` (was
  `build_switch`). Every other typed/erased pair in the crate suffixes the
  *erased* variant `_dyn` — `build_call` / `build_call_dyn`, `build_invoke` /
  `build_invoke_dyn`, `build_int_phi` / `build_int_phi_dyn` — and `switch` was
  the sole inversion. Migration is a rename: `build_switch` → `build_switch_dyn`,
  `build_switch_typed` → `build_switch`. Behaviour and return types are
  unchanged, and the erased form is still what the `.ll` parser and the auto-SSA
  builder land on. The *Typed terminator operands* section below, which
  introduced the pair, is written in the new names.
- Integer type drift at the fold and variable-def seams now reports
  `IrError::OperandWidthMismatch { lhs, rhs }` where it previously reported
  `IrError::TypeMismatch { expected: Integer, got: Integer }` — true, and silent
  about the only fact separating the two sides, since `TypeKindLabel` has a
  single width-less `Integer` variant. A drift to a wrong *kind* still reports
  `TypeMismatch`. Float seams are unaffected: `TypeKindLabel` has a distinct
  variant per float kind, so their `TypeMismatch` already named both sides.
- Pointer type drift at the fold and variable-def seams now reports
  `IrError::AddressSpaceMismatch { expected, got }`, for the identical reason
  applied to the single, address-space-less `Pointer` variant — an
  `addrspace(0)`-vs-`addrspace(1)` def used to report "expected pointer, got
  pointer". It is a separate error rather than a reuse of
  `OperandWidthMismatch` because an address space is not a width.
  `SsaBuilder::def_pointer_var` is the variable-def seam. The fold seams are
  every pointer-typed destination a custom folder can answer — among them
  `build_pointer_cast`, and `build_bitcast_dyn` / `build_select` when the
  destination or the arms are pointers — all of which funnel through the
  builder's `checked_folded_value`, hence through the same
  `Type::require_match`.

  Both are **breaking for error matching**: code keying on
  `IrError::TypeMismatch` to catch an integer-width or address-space drift at
  these seams must now match the new variants.

### Typed terminator operands — switch condition width and indirectbr address

Extends the branching type-safety program from a terminator's *edges* to its
*operands*: the `switch` condition/case integer width and the `indirectbr`
address type now live in the Rust type system, so a wrong-width case value or a
non-pointer jump address is a **compile error** rather than a runtime
`TypeMismatch` / verifier rejection. Typing is **opt-in** — the erased authoring
surface (`build_switch_dyn`, `build_indirectbr` with a runtime-checked `Value`
address) is untouched and keeps its existing runtime checks.

#### Added

- `SwitchInst<'ctx, P, B, W: IntWidth = IntDyn>` now threads the condition's
  integer width `W` as a **last, defaulted** type parameter, plus
  `IRBuilder::build_switch::<W>(cond, default, name)` — the typed sibling
  of `build_switch_dyn` — which pins `W` from the typed condition and returns a
  `SwitchInst<…, W>`. On such a switch, `SwitchInst::add_case` carries an
  `IntoIntValue<'ctx, W, B>` bound, so a **wrong-width case value is a compile
  error** (an `i64` case on a `W = i32` switch has no `IntoIntValue<'_, i32, _>`
  impl — never narrows). The erased `build_switch_dyn` still yields a
  `SwitchInst<…, IntDyn>` whose `add_case` keeps the runtime `TypeMismatch`
  check, and the parser / SSA-builder paths are unchanged (they land on the
  erased form).
- `IRBuilder::build_indirectbr` tightened its address bound from `IsValue` to
  `IntoPointerValue<'ctx, B>`, so a **typed non-pointer address is a compile
  error** (an `IntValue<i32>` has no `IntoPointerValue` impl) — the
  pointer-ness check moves from `verify()` to build time. An erased `Value`
  address still works and is pointer-checked at build time as before.

### Const-generic vector and array types (breaking)

Fixed vectors and arrays now carry their **element type** and **length** in the
Rust type system, so `<N x T>` / `[N x T]` length mismatches and wrong-element
`insertelement` / `insertvalue` — previously caught only by `Module::verify()` —
become **compile errors**. This is the vector/array analog of the scalar
`IntValue<'ctx, W: IntWidth, B>`. Erased (`Dyn`) markers are the defaults, so a
bare `VectorValue<'ctx>` / `ArrayValue<'ctx>` is the fully-erased form, and
parsed `.ll`, scalable vectors, and runtime lengths land there unchanged.

#### Added

- Element markers `VecElem` (base) and `StaticVecElem<'ctx, B>` (projection) in
  `element.rs`, spelled by the scalar markers themselves (`i64`, `f64`, `bool`,
  the int-width and float-kind markers); `ElemDyn` is the erased element.
- Length markers `Len<const N: u32>` / `LenDyn` (+ `StaticVecLen`) for vectors
  and `ArrLen<const N: u64>` / `ArrLenDyn` (+ `StaticArrayLen`) for arrays —
  separate families because vector lengths are `u32` and array lengths `u64`.
- Const-generic constructors `Module::vector_type_n::<E, const N: u32>()` and
  `array_type_n::<E, const N: u64>()`. `vector_type_n` rejects `N == 0` at
  monomorphisation (a `const {}` assert); `[0 x T]` arrays stay legal.
- Typed value narrowing — `TryFrom<Value>` for `VectorValue<E, Len<N>>` and
  `ArrayValue<E, ArrLen<N>>` checks element **and** length before stamping the
  markers (`OperandWidthMismatch` / `IrError::ArrayLengthMismatch` for length,
  `TypeMismatch` for element), mirroring the scalar `IntValue` narrowing.
- Typed op builders that lower into the existing erased builders (byte-identical
  IR): `build_vec_int_{add,sub,mul,xor,and,or,shl,lshr,ashr}` (both operands
  pinned to the same `E`,`N`, so a length/element mismatch has no matching impl),
  `build_vec_extract` / `build_vec_insert` / `build_vec_splat`, and the array
  `build_arr_extract` / `build_arr_insert`. `build_alloca` accepts a typed array
  type directly (its result stays an erased `PointerValue`).
- `IrError::ArrayLengthMismatch { expected: u64, got: u64 }` — a statically
  lengthed array handle narrowed from an array of a different length.
- `WrapWitness` — an unforgeable in-crate token gating `StaticVecElem::wrap_value`
  (the sole unchecked `Value` → typed-scalar-handle wrap) to callers that already
  hold an element-type proof; every external `Value` → typed-handle path stays the
  checked `TryFrom`.
- Example `crates/llvmkit-ir/examples/typed_vector_array.rs` and three new table
  rows in `docs/type-safety-vs-llvm.md`.

#### Changed

- **Breaking:** `VectorType` / `VectorValue` and `ArrayType` / `ArrayValue` each
  gained two defaulted generic parameters — element and length. The bare handles
  (`VectorValue<'ctx>`, `ArrayType<'ctx>`, …) still name the fully-erased form and
  behave exactly as before; only code that spelled these handles with an explicit
  brand-only generic list must now also spell the `Dyn` markers.
- **Breaking:** the unwired element-as-type-handle scaffolds `VectorElement` /
  `SizedElement` (`vector_element.rs` / `sized_element.rs`) are removed, replaced
  by the scalar-marker `VecElem` / `ElemDyn` in `element.rs`. They had no
  consumers.

Still erased by design (runtime/verifier-checked, unchanged): scalable vectors,
pointer-element vectors (blocked on address-space markers), composite-element
arrays, and length-relating ops (`shufflevector` output length, concat `N1+N2`,
compile-time index-in-bounds) that need `generic_const_exprs` on nightly. See
`docs/future-work.md`.
