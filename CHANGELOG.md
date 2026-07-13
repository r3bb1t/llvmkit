# Changelog

Notable, user-visible changes to `llvmkit`. The format follows
[Keep a Changelog](https://keepachangelog.com/); the project is pre-1.0
(`0.0.x`), so breaking changes are expected and are flagged inline. Until a
tagged release is cut, entries accumulate under **Unreleased**.

## [Unreleased]

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
- `FnReshape::remove_edge` / `redirect_edge` (`switch`-terminator scope this
  release) drop or retarget a CFG edge and mechanically maintain the affected
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

- **Breaking:** the six `IRBuilder::build_*_phi` builders and the
  `PhiInst` / `FpPhiInst` / `PointerPhiInst` open-phi `add_incoming` / `finish`
  mutators are no longer public (`pub(crate)`). Author phis with block
  arguments instead — the edge and its incomings move together, so desync is
  unrepresentable rather than deferred to `verify()`:

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
