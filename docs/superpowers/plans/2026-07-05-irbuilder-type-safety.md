# IRBuilder Type-Safety Hardening + Typed Calls + Auto-SSA Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close every closable runtime-check seam in the IRBuilder path (typed folder, typed memory, flag parity, doctrine fixes), ship end-to-end typed calls (compile-checked arity/types/derived return markers), and ship the Braun-style auto-SSA frontend.

**Architecture:** Four sequential workstreams on branch `irbuilder-type-safety`: (0) mechanical block-state rename freeing the word "seal" for Braun-SSA; (1) hardening sweep over `ir_builder/folder.rs` + `ir_builder.rs`; (2) typed calls extending the existing `function_signature.rs` schema machinery; (3) new `ssa_builder.rs` layer on top of the typed IRBuilder. Spec: `docs/superpowers/specs/2026-07-05-irbuilder-typed-calls-autossa-design.md`.

**Tech Stack:** Rust 2024 (MSRV 1.96 — `cast_unsigned` and `#[diagnostic::on_unimplemented]` both available), cargo workspace, trybuild compile-fail tests, proptest, rust-analyzer LSP.

## Global Constraints

- `#![forbid(unsafe_code)]` in every crate; no `dyn Trait` in public IR/builder surface; sealed traits for closed sets.
- No `as` casts; no `#[allow(...)]`; no `unwrap`/`expect`/`panic!` in production paths (`unreachable!("<invariant>")` only for provably-dead branches).
- No `bool` or `Option` parameters in public APIs — separate methods instead.
- Every typed form keeps an explicitly-named `_dyn` fallback (Doctrine D3).
- AsmWriter output byte-for-byte stable. SOLE exception: Task 2's GEP address-space bug fix, which moves output TO upstream parity.
- Every new `#[test]` carries an upstream-citation doc comment and an UPSTREAM.md row; Rust-only features use explicitly-labeled example-locks (D11).
- Prefer imports over qualified paths; new public items re-export from `lib.rs`.
- Commit messages cite doctrine IDs (e.g. "D1", "D4") and end with the Co-Authored-By trailer.
- Gates: `cargo check` after each step-group; per-task `cargo test -p llvmkit-ir`; per-workstream `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt -- --check`.
- Use the LSP tool: `lsp references` before changing any public signature; `lsp rename` for symbol renames; `lsp diagnostics` after substantive edits. Never regex-rename code symbols.
- The C++ reference tree is at `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/` — read the cited upstream file before porting behavior.

---

### Task 1: Workstream 0 — Block-state rename (`Sealed`→`Terminated`, `Unsealed`→`Unterminated`)

**Files:**
- Modify: `crates/llvmkit-ir/src/block_state.rs` (full rewrite below)
- Modify: `crates/llvmkit-ir/src/basic_block.rs` (generic param `Seal`→`Term`, `retag_seal`→`retag_termination`)
- Modify: `crates/llvmkit-ir/src/ir_builder.rs` (type aliases + new `BuilderPositionState`)
- Modify: ~20 llvmkit-ir files + 2 llvmkit-asmparser files (mechanical fallout, via LSP rename)
- Rename: `crates/llvmkit-ir/tests/compile_fail/position_at_end_sealed_block.rs` → `position_at_end_terminated_block.rs` (+`.stderr`); `retained_unsealed_block_cannot_reposition.rs` → `retained_unterminated_block_cannot_reposition.rs` (+`.stderr`)

**Interfaces:**
- Produces: `pub trait BlockTerminationState { const IS_TERMINATED: bool; }`, `pub struct Terminated;`, `pub struct Unterminated;`, `pub trait BuilderPositionState: state_sealed::Sealed + 'static {}` (impls: `Positioned`, `Unpositioned`), aliases `TerminatedBlockInst` / `TerminatedBlockSwitch` / `TerminatedBlockIndirectBr` / `TerminatedBlockInvoke` / `TerminatedBlockCatchSwitch`. Every later task uses these names.

- [ ] **Step 1: Rename the three state symbols with LSP**

Run (each is a workspace-wide symbol rename; do NOT use regex):
```
lsp rename crates/llvmkit-ir/src/block_state.rs symbol:"Sealed" (the struct at line 37) -> "Terminated"
lsp rename crates/llvmkit-ir/src/block_state.rs symbol:"Unsealed" (the struct at line 32) -> "Unterminated"
lsp rename crates/llvmkit-ir/src/block_state.rs symbol:"BlockSealState" -> "BlockTerminationState"
lsp rename crates/llvmkit-ir/src/block_state.rs symbol:"IS_SEALED" -> "IS_TERMINATED"
lsp rename (basic_block.rs) symbol:"retag_seal" -> "retag_termination"
```
Expected: ~111 edits across ~20 llvmkit-ir files and 13 sites in llvmkit-asmparser. CAUTION: the unrelated privacy idiom `value::sealed::Sealed` must NOT be touched — it is a different symbol; LSP rename keeps them apart (this is why regex is forbidden here).

- [ ] **Step 2: Rewrite `block_state.rs` doc comments (termination vocabulary)**

Replace the module docs and marker docs so "seal" no longer appears. Final file content:

```rust
//! Construction-lifecycle typestate for [`crate::BasicBlock`]
//! (Doctrine D1 — make invalid states unrepresentable).
//!
//! LLVM enforces "every well-formed basic block ends in exactly one
//! terminator and contains nothing after that terminator" at runtime
//! in `Verifier::visitBasicBlock` (`lib/IR/Verifier.cpp`). llvmkit
//! models the common builder path with a termination-state view: an
//! [`Unterminated`] block handle can be positioned for appends, and
//! terminator builders return a [`Terminated`] view of the insertion
//! block. Code that follows that returned view cannot append via
//! [`crate::IRBuilder::position_at_end`].
//!
//! Cranelift calls this state "filled"; llvmkit uses LLVM's own
//! "terminator" vocabulary. The word "sealed" is reserved for the
//! Braun-SSA predecessor-set sense used by [`crate::SsaBuilder`].
//!
//! `BasicBlock` is a linear insertion-capability handle (`!Copy` / `!Clone`).
//! Copyable cross-block references use [`crate::BasicBlockLabel`], so a
//! retained label can still name a predecessor after termination without
//! being accepted by [`crate::IRBuilder::position_at_end`].

use crate::value::sealed;

/// Sealed marker trait for the [`crate::BasicBlock`] termination-state
/// type parameter. The two implementors are [`Unterminated`] and
/// [`Terminated`]; external crates cannot invent new states.
pub trait BlockTerminationState: sealed::Sealed + 'static {
    /// `true` for [`Terminated`], `false` for [`Unterminated`]. Useful for
    /// formatter / diagnostic helpers that need to distinguish at
    /// runtime without per-state generics.
    const IS_TERMINATED: bool;
}

/// Marker: the block has no terminator yet. `IRBuilder` may be
/// positioned at this block and emit instructions into it.
#[derive(Debug, Clone, Copy)]
pub struct Unterminated;

/// Marker: the block has a terminator. No further instructions can
/// be appended via `IRBuilder`.
#[derive(Debug, Clone, Copy)]
pub struct Terminated;

impl sealed::Sealed for Unterminated {}
impl sealed::Sealed for Terminated {}

impl BlockTerminationState for Unterminated {
    const IS_TERMINATED: bool = false;
}
impl BlockTerminationState for Terminated {
    const IS_TERMINATED: bool = true;
}
```

(Note: the `SsaBuilder` doc link goes stale until Task 16 — use plain text "the Braun-SSA predecessor-set sense" without the intra-doc link until then, then upgrade the link in Task 16.)

- [ ] **Step 3: Rename the five ir_builder.rs type aliases with LSP**

```
lsp rename symbol:"SealedBlockInst" -> "TerminatedBlockInst"
lsp rename symbol:"SealedBlockSwitch" -> "TerminatedBlockSwitch"
lsp rename symbol:"SealedBlockIndirectBr" -> "TerminatedBlockIndirectBr"
lsp rename symbol:"SealedBlockInvoke" -> "TerminatedBlockInvoke"
lsp rename symbol:"SealedBlockCatchSwitch" -> "TerminatedBlockCatchSwitch"
```
Also rename the generic parameter `Seal` to `Term` on `BasicBlock` in basic_block.rs (LSP rename on the type parameter; if rust-analyzer refuses generic-param rename, edit the declaration and let `lsp diagnostics` list the fallout).

- [ ] **Step 4: Add `BuilderPositionState` and retarget the `S` bound**

In `crates/llvmkit-ir/src/ir_builder.rs`, next to the existing `state_sealed` module (near the `Positioned`/`Unpositioned` markers, ~line 120-130), add:

```rust
/// Sealed marker trait for the [`IRBuilder`] positioning typestate.
/// The two implementors are [`Unpositioned`] and [`Positioned`];
/// external crates cannot invent new states. Public so higher layers
/// (e.g. [`crate::SsaBuilder`]) can be generic over the same states.
pub trait BuilderPositionState: state_sealed::Sealed + 'static {}

impl BuilderPositionState for Unpositioned {}
impl BuilderPositionState for Positioned {}
```

Then change the `IRBuilder` struct's `S: state_sealed::Sealed` bound (~line 206) and every impl-block bound that spells `S: state_sealed::Sealed` to `S: BuilderPositionState` (find them with `lsp references` on `state_sealed::Sealed`). Re-export `BuilderPositionState` from `crates/llvmkit-ir/src/lib.rs` alongside the existing `Positioned`/`Unpositioned` re-exports (keep the `SsaBuilder` doc link as plain text until Task 16).

- [ ] **Step 5: Rename the two compile-fail fixtures and regenerate goldens**

```bash
git mv crates/llvmkit-ir/tests/compile_fail/position_at_end_sealed_block.rs crates/llvmkit-ir/tests/compile_fail/position_at_end_terminated_block.rs
git mv crates/llvmkit-ir/tests/compile_fail/position_at_end_sealed_block.stderr crates/llvmkit-ir/tests/compile_fail/position_at_end_terminated_block.stderr
git mv crates/llvmkit-ir/tests/compile_fail/retained_unsealed_block_cannot_reposition.rs crates/llvmkit-ir/tests/compile_fail/retained_unterminated_block_cannot_reposition.rs
git mv crates/llvmkit-ir/tests/compile_fail/retained_unsealed_block_cannot_reposition.stderr crates/llvmkit-ir/tests/compile_fail/retained_unterminated_block_cannot_reposition.stderr
```
(If the exact fixture filenames differ, list the directory first: `ls crates/llvmkit-ir/tests/compile_fail/ | grep -i seal`.) Update the harness file that references them (grep for the old stem in `crates/llvmkit-ir/tests/`), update the fixture contents' renamed symbols, then regenerate goldens:

```bash
TRYBUILD=overwrite cargo test -p llvmkit-ir --test typestate_compile_fail
```
(If the harness test file has a different name, find it: `grep -rl "compile_fail" crates/llvmkit-ir/tests/*.rs`.) Inspect the regenerated `.stderr` diffs — they must only show the renamed symbols, no new error classes.

- [ ] **Step 6: Full gate + grep audit**

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt
```
Expected: all green. Then audit that "seal" vocabulary is gone from block-state contexts:
```bash
grep -rn "Unsealed\|BlockSealState\|IS_SEALED\|SealedBlock\|retag_seal" crates/ llvmkit/
```
Expected: zero hits. (`sealed::Sealed` / `mod sealed` privacy-idiom hits are fine and expected — the grep above deliberately excludes the lowercase forms.) Update AGENTS.md prose that mentions `Sealed`/`Unsealed` block states and `SealedBlock*` aliases (Project Status bullets T2/Parser-1) to the new names.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "Rename block seal typestate to termination state (D1)

Frees the word 'seal' for the upcoming Braun-SSA layer: BlockSealState
-> BlockTerminationState, Sealed -> Terminated, Unsealed ->
Unterminated, IS_SEALED -> IS_TERMINATED, SealedBlock* aliases ->
TerminatedBlock*, retag_seal -> retag_termination. Adds public sealed
BuilderPositionState over Positioned/Unpositioned. Mechanical; no
behavior or printed-IR change.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: GEP address-space preservation bug fix (2f.1, P0)

**Files:**
- Modify: `crates/llvmkit-ir/src/ir_builder.rs:3703` (in `build_gep_inner`)
- Test: `crates/llvmkit-ir/tests/builder_gep_addrspace.rs` (new)
- Modify: `UPSTREAM.md` (register the new test)

**Interfaces:**
- Consumes: existing `build_gep`/`build_inbounds_gep` public methods (signatures unchanged).
- Produces: GEP results typed `ptr addrspace(N)` matching the base pointer. Upstream: `GetElementPtrInst::getGEPReturnType` (`llvm/include/llvm/IR/Instructions.h`) — read it first.

- [ ] **Step 1: Write the failing test**

Create `crates/llvmkit-ir/tests/builder_gep_addrspace.rs`:

```rust
//! GEP result-type address-space preservation.
//!
//! Ports the constructive subset of
//! `llvm/test/Assembler/2007-12-11-AddressSpaces.ll` (GEP through
//! `ptr addrspace(N)` operands): the GEP result pointer must live in
//! the SAME address space as its base pointer, mirroring
//! `GetElementPtrInst::getGEPReturnType` (`IR/Instructions.h`).

use llvmkit_ir::{IrResult, Linkage, Module};

#[test]
fn gep_result_preserves_base_pointer_address_space() -> IrResult<()> {
    Module::with_new("gep_addrspace", |m| {
        let fn_ty = m.function_type(m.void_type().as_type(), &[m.ptr_type(33).as_type()], false)?;
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let p = f.param(0)?;
        let gep = b.build_inbounds_gep(m.i32_type(), p, [1_i64], "q")?;
        // The result type must be ptr addrspace(33), not plain ptr.
        let printed = format!("{m}");
        assert!(
            printed.contains("getelementptr inbounds i32, ptr addrspace(33) %0"),
            "GEP must keep the base address space; got:\n{printed}"
        );
        b.build_ret_void()?;
        Ok(())
    })
}
```

Adjust the construction calls to the real public API if they differ (check with `lsp hover` on `function_type` / `add_function` / `build_inbounds_gep`; the existing `tests/globals_basic.rs` and `tests/medium_builder_*.rs` show the canonical construction idioms). The assertion is the point: `ptr addrspace(33)` in the printed GEP.

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p llvmkit-ir --test builder_gep_addrspace
```
Expected: FAIL — the printed GEP says `ptr %0`-typed result (result type interned as addrspace 0).

- [ ] **Step 3: Fix `build_gep_inner`**

At `crates/llvmkit-ir/src/ir_builder.rs:3703`, replace the hard-coded result type:

```rust
// BEFORE:
let result_ty = self.module.ptr_type(0).as_type().id();

// AFTER — mirror GetElementPtrInst::getGEPReturnType (IR/Instructions.h):
// the result pointer lives in the base pointer's address space.
let base_addr_space = match self.module.context().type_data(ptr_value.ty) {
    TypeData::Pointer { addr_space } => *addr_space,
    _ => 0, // ptr operand was already validated as a pointer upstream of here
};
let result_ty = self.module.ptr_type(base_addr_space).as_type().id();
```

Match the real `TypeData::Pointer` variant shape (check `crates/llvmkit-ir/src/type.rs` — if the field is a different name or the variant is tuple-shaped, adapt; if `ptr_value` at this point is a `Value` with a `.ty` field vs a method, adapt). If the folder call at ~3704-3709 compares the folded result against `result_ty`, it now automatically checks against the correct address space — verify by reading the 10 lines below the edit.

- [ ] **Step 4: Run tests**

```bash
cargo test -p llvmkit-ir --test builder_gep_addrspace && cargo test -p llvmkit-ir
```
Expected: new test PASSES; full suite green (no existing fixture locks a non-zero-AS GEP result — if one fails, it was locking the bug; fix the fixture and note it in the commit).

- [ ] **Step 5: Register in UPSTREAM.md and commit**

Append the UPSTREAM.md row citing `test/Assembler/2007-12-11-AddressSpaces.ll` + `GetElementPtrInst::getGEPReturnType`. Then:

```bash
git add -A
git commit -m "Fix GEP result type to preserve base address space (D10)

build_gep_inner hard-coded ptr_type(0); upstream
GetElementPtrInst::getGEPReturnType keeps the source pointer's address
space. Locks test/Assembler/2007-12-11-AddressSpaces.ll subset.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Close the `SelectArm::from_select_value` forging hole (2f.2, P0)

**Files:**
- Modify: `crates/llvmkit-ir/src/ir_builder.rs:~6211-6308` (SelectArm trait + impls + `build_select`)
- Test: `crates/llvmkit-ir/tests/compile_fail/select_arm_forge.rs` (+`.stderr`) (new)
- Modify: `UPSTREAM.md`

**Interfaces:**
- Consumes: existing `SelectArm` sealed trait (`type Output; fn from_select_value(v: Value) -> Self::Output; fn arm_value(self) -> Value;`).
- Produces: `pub struct SelectNarrow<'a>` evidence token (crate-mintable only); `SelectArm::from_select_value(v: Value<'ctx, B>, narrow: &SelectNarrow<'_>) -> Self::Output`. Task 5 relies on this hole being closed for its no-recheck soundness argument.

- [ ] **Step 1: Write the compile-fail test**

Create `crates/llvmkit-ir/tests/compile_fail/select_arm_forge.rs` (example-lock — C++ has no analog; nearest family is the crate's existing capability-token fixtures):

```rust
//! A downstream crate must NOT be able to forge a typed handle through
//! SelectArm::from_select_value — the SelectNarrow evidence token has no
//! public constructor.
use llvmkit_ir::{Module, SelectArm};

fn main() {
    Module::with_new("forge", |m| {
        let f64_val = m.f64_type().const_float(1.0).as_value();
        // ERROR: no way to mint a SelectNarrow token outside llvmkit-ir.
        let _forged = <llvmkit_ir::IntValue<'_, i32, _> as SelectArm<_>>::from_select_value(
            f64_val,
            todo!("SelectNarrow cannot be constructed here"),
        );
        Ok(())
    })
    .unwrap();
}
```

Register it in the trybuild harness (same file found in Task 1 Step 5).

- [ ] **Step 2: Add the token and change the trait**

In `ir_builder.rs`, above the `SelectArm` trait (~6205):

```rust
/// Evidence that a select fold/result value has already been checked
/// against the arm type. Only this crate can mint it (private field,
/// `pub(crate)` constructor), so downstream code can *name* the type in
/// trait impls but cannot call `from_select_value` with a forged value.
/// Follows the `ValidatedStructValue` capability-token precedent
/// (`struct_schema.rs`).
#[derive(Debug)]
pub struct SelectNarrow<'a> {
    _private: PhantomData<&'a ()>,
}

impl<'a> SelectNarrow<'a> {
    #[inline]
    pub(crate) fn new() -> Self {
        Self { _private: PhantomData }
    }
}
```

Change the trait method to `fn from_select_value(v: Value<'ctx, B>, narrow: &SelectNarrow<'_>) -> Self::Output;` and add the `_narrow` parameter to every impl (IntValue, FloatValue, PointerValue, VectorValue, and the generic IsValue impl — the bodies don't use it). Update the in-crate call sites inside `build_select` (find them: `lsp references` on `from_select_value`; expected 1-2 sites) to pass `&SelectNarrow::new()`. Re-export `SelectNarrow` from `lib.rs`.

- [ ] **Step 3: Regenerate the compile-fail golden and run the suite**

```bash
TRYBUILD=overwrite cargo test -p llvmkit-ir --test typestate_compile_fail
cargo test -p llvmkit-ir
```
Expected: `select_arm_forge.stderr` shows the private-constructor / unmintable-token error; full suite green.

- [ ] **Step 4: Register in UPSTREAM.md and commit**

```bash
git add -A
git commit -m "Gate SelectArm::from_select_value behind SelectNarrow token (D2)

The doc-hidden trait method wrapped unchecked, letting downstream code
forge typed handles from mistyped values. Evidence-token parameter
follows the ValidatedStructValue precedent.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `OverflowFlags` carrier type

**Files:**
- Modify: `crates/llvmkit-ir/src/instr_types.rs` (add next to the existing per-opcode flag structs `AddFlags`/`SubFlags`/...)
- Modify: `crates/llvmkit-ir/src/lib.rs` (re-export)

**Interfaces:**
- Produces: `OverflowFlags` with `pub const fn none()`, chainable `pub const fn nuw(self)` / `pub const fn nsw(self)`, accessors `has_nuw()` / `has_nsw()`, and `pub(crate) fn from_parts(nuw: bool, nsw: bool)` for internal call sites that hold runtime bools. Task 5's `fold_int_bin_op_no_wrap` and the folder trait consume it.

- [ ] **Step 1: Add the type**

```rust
/// nuw/nsw pair for overflowing binary operators. Mirrors the flag
/// pair on `OverflowingBinaryOperator` (`IR/Operator.h`). Public
/// construction is chainable (`OverflowFlags::none().nuw().nsw()`);
/// the bool-pair constructor is crate-internal per the no-bool-params
/// convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct OverflowFlags {
    nuw: bool,
    nsw: bool,
}

impl OverflowFlags {
    /// No wrap flags set.
    #[inline]
    pub const fn none() -> Self {
        Self { nuw: false, nsw: false }
    }

    /// Set `nuw` (no unsigned wrap).
    #[inline]
    pub const fn nuw(self) -> Self {
        Self { nuw: true, ..self }
    }

    /// Set `nsw` (no signed wrap).
    #[inline]
    pub const fn nsw(self) -> Self {
        Self { nsw: true, ..self }
    }

    #[inline]
    pub const fn has_nuw(self) -> bool {
        self.nuw
    }

    #[inline]
    pub const fn has_nsw(self) -> bool {
        self.nsw
    }

    #[inline]
    pub(crate) const fn from_parts(nuw: bool, nsw: bool) -> Self {
        Self { nuw, nsw }
    }
}
```

- [ ] **Step 2: Re-export, check, commit**

Re-export `OverflowFlags` from `crates/llvmkit-ir/src/lib.rs` (same `pub use` group as the other instr_types flag structs) and from the umbrella `llvmkit/src/lib.rs` if flag types are re-exported there (grep for `AddFlags` to see).

```bash
cargo check && git add -A && git commit -m "Add OverflowFlags carrier for folder no-wrap hooks

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Typed folder (2a, P0) — trait rewrite, NoFolder one-liner, builder rewiring

**Files:**
- Modify: `crates/llvmkit-ir/src/ir_builder/folder.rs` (full trait replacement below)
- Modify: `crates/llvmkit-ir/src/ir_builder/no_folder.rs` (shrinks to an empty impl)
- Modify: `crates/llvmkit-ir/src/ir_builder/constant_folder.rs` (method renames + typed overrides)
- Modify: `crates/llvmkit-ir/src/ir_builder.rs` (~45 fold call sites; new `accept_folded_int/fp` helpers)
- Test: `crates/llvmkit-ir/tests/compile_fail/folder_typed_wrong_width.rs` (+`.stderr`), additions to the existing custom-folder test file (find it: `grep -rl "IRBuilderFolder" crates/llvmkit-ir/tests/`)
- Modify: `UPSTREAM.md`

**Interfaces:**
- Consumes: `OverflowFlags` (Task 4), `SelectNarrow`-closed handle set (Task 3).
- Produces: trait `IRBuilderFolder` with 17 renamed `*_dyn` hooks (all with default `Ok(None)` bodies) + typed hooks `fold_int_bin_op<W>` / `fold_int_bin_op_no_wrap<W>` / `fold_int_bin_op_exact<W>` / `fold_fp_bin_op<K>` / `fold_fp_un_op<K>` / `fold_int_cmp<W>` / `fold_fp_cmp<K>` / `fold_cast_to_int<W>` / `fold_cast_to_fp<K>` (defaults delegate to `_dyn` + TypeId re-narrow). Tasks 6/16 build on the final folder names; the parser and all `_dyn` builder methods keep erased folding.

- [ ] **Step 1: Rename the 17 trait methods with LSP**

One `lsp rename` per method on the trait definition in `folder.rs` (renaming the trait method updates `ConstantFolder`, `NoFolder`, every builder call site, and any test folder in one shot):

`fold_bin_op`→`fold_bin_op_dyn`, `fold_exact_bin_op`→`fold_exact_bin_op_dyn`, `fold_no_wrap_bin_op`→`fold_no_wrap_bin_op_dyn`, `fold_bin_op_fmf`→`fold_bin_op_fmf_dyn`, `fold_un_op_fmf`→`fold_un_op_fmf_dyn`, `fold_cmp`→`fold_cmp_dyn`, `fold_gep`→`fold_gep_dyn`, `fold_select`→`fold_select_dyn`, `fold_extract_value`→`fold_extract_value_dyn`, `fold_insert_value`→`fold_insert_value_dyn`, `fold_extract_element`→`fold_extract_element_dyn`, `fold_insert_element`→`fold_insert_element_dyn`, `fold_shuffle_vector`→`fold_shuffle_vector_dyn`, `fold_cast`→`fold_cast_dyn`, `fold_binary_intrinsic`→`fold_binary_intrinsic_dyn`. (`create_pointer_cast` / `create_pointer_bitcast_or_addrspace_cast` keep their names — they are constant-materialization hooks, not fold hooks, matching upstream's `CreatePointerCast` naming.)

Run `cargo check` — still green (pure rename).

- [ ] **Step 2: Change the three doctrine-violating signatures**

In `folder.rs` (and mirror in `constant_folder.rs`'s impl + all call sites, which `lsp diagnostics` will list):

```rust
// fold_exact_bin_op_dyn: DROP the trailing `is_exact: bool` — the
// builder only ever passed `true` (exactness is implied by the method).
fn fold_exact_bin_op_dyn(
    &self,
    opcode: BinaryOpcode,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
) -> IrResult<Option<Value<'ctx, B>>> { let _ = (opcode, lhs, rhs); Ok(None) }

// fold_no_wrap_bin_op_dyn: `has_nuw: bool, has_nsw: bool` -> OverflowFlags.
fn fold_no_wrap_bin_op_dyn(
    &self,
    opcode: BinaryOpcode,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    flags: OverflowFlags,
) -> IrResult<Option<Value<'ctx, B>>> { let _ = (opcode, lhs, rhs, flags); Ok(None) }

// fold_binary_intrinsic_dyn: split on the Option param.
fn fold_binary_intrinsic_dyn(
    &self,
    id: BinaryIntrinsic,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    ty: Type<'ctx, B>,
) -> IrResult<Option<Value<'ctx, B>>> { let _ = (id, lhs, rhs, ty); Ok(None) }

fn fold_binary_intrinsic_with_fmf_source_dyn(
    &self,
    id: BinaryIntrinsic,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    ty: Type<'ctx, B>,
    fmf_source: &InstructionView<'ctx, B>,
) -> IrResult<Option<Value<'ctx, B>>> {
    let _ = fmf_source;
    self.fold_binary_intrinsic_dyn(id, lhs, rhs, ty)
}
```

Builder call-site migration rules: `fold_exact_bin_op(op, l, r, true)` → drop the `true`; `fold_no_wrap_bin_op(op, l, r, nuw, nsw)` → `..., OverflowFlags::from_parts(nuw, nsw)` (or the chainable form where the flags are literal); `fold_binary_intrinsic(id, l, r, ty, None)` → `fold_binary_intrinsic_dyn(id, l, r, ty)`; `..., Some(view))` → `fold_binary_intrinsic_with_fmf_source_dyn(id, l, r, ty, view)`.

- [ ] **Step 3: Give every `_dyn` hook a default `Ok(None)` body; shrink NoFolder**

Every one of the 17 hooks gets `{ let _ = (<params>); Ok(None) }` as its default body (as shown above; imports: extend the `use super::{...}` list — every needed name is already imported by `ir_builder.rs`). Then replace the entire body of `no_folder.rs`'s impl:

```rust
/// Folder that never folds: every hook keeps its default
/// "decline to fold" body, so the builder materializes a real
/// instruction for every operation. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
impl<'ctx, B: ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B> for NoFolder {}
```

Keep `NoFolder`'s struct definition, derives, and doc comment; delete only the 17 stub methods (and any now-unused imports/helpers in that file — `lsp diagnostics` flags them).

- [ ] **Step 4: Add the typed hooks with delegating defaults**

Append to the trait in `folder.rs` (after the `_dyn` hooks), plus the two `pub(super)` narrowing helpers at file scope:

```rust
    // ---- Typed hooks. Called by the statically-typed build_* paths;
    //      results are typed handles the builder accepts without a
    //      runtime re-check for static markers. Defaults delegate to
    //      the matching _dyn hook and re-narrow by TypeId, so a folder
    //      that only overrides the erased surface keeps today's
    //      semantics. Pointer-, vector-, and aggregate-result folds
    //      (fold_gep_dyn, fold_select_dyn, ...) deliberately stay
    //      erased: PointerValue does not statically pin the address
    //      space and vector element typing is deferred (T4). ----

    fn fold_int_bin_op<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())?;
        narrow_folded_int(folded, lhs)
    }

    fn fold_int_bin_op_no_wrap<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
        flags: OverflowFlags,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_no_wrap_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value(), flags)?;
        narrow_folded_int(folded, lhs)
    }

    fn fold_int_bin_op_exact<W: IntWidth>(
        &self,
        opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_exact_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())?;
        narrow_folded_int(folded, lhs)
    }

    fn fold_fp_bin_op<K: FloatKind>(
        &self,
        opcode: BinaryOpcode,
        lhs: FloatValue<'ctx, K, B>,
        rhs: FloatValue<'ctx, K, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        let folded = self.fold_bin_op_fmf_dyn(opcode, lhs.as_value(), rhs.as_value(), fmf)?;
        narrow_folded_fp(folded, lhs)
    }

    fn fold_fp_un_op<K: FloatKind>(
        &self,
        opcode: UnaryOpcode,
        value: FloatValue<'ctx, K, B>,
        fmf: FastMathFlags,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        let folded = self.fold_un_op_fmf_dyn(opcode, value.as_value(), fmf)?;
        narrow_folded_fp(folded, value)
    }

    fn fold_int_cmp<W: IntWidth>(
        &self,
        predicate: IntPredicate,
        lhs: IntValue<'ctx, W, B>,
        rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, bool, B>>> {
        let folded = self.fold_cmp_dyn(predicate.into(), lhs.as_value(), rhs.as_value())?;
        narrow_folded_bool(folded)
    }

    fn fold_fp_cmp<K: FloatKind>(
        &self,
        predicate: FloatPredicate,
        lhs: FloatValue<'ctx, K, B>,
        rhs: FloatValue<'ctx, K, B>,
    ) -> IrResult<Option<IntValue<'ctx, bool, B>>> {
        let folded = self.fold_cmp_dyn(predicate.into(), lhs.as_value(), rhs.as_value())?;
        narrow_folded_bool(folded)
    }

    fn fold_cast_to_int<W: IntWidth>(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: IntType<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let folded = self.fold_cast_dyn(opcode, value, dest_ty.as_type())?;
        narrow_folded_cast_int(folded, dest_ty)
    }

    fn fold_cast_to_fp<K: FloatKind>(
        &self,
        opcode: CastOpcode,
        value: Value<'ctx, B>,
        dest_ty: FloatType<'ctx, K, B>,
    ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
        let folded = self.fold_cast_dyn(opcode, value, dest_ty.as_type())?;
        narrow_folded_cast_fp(folded, dest_ty)
    }
```

File-scope helpers (same file, below the trait):

```rust
/// Re-narrow an erased fold result to the operand's int width by TypeId
/// equality. Used by the typed hooks' delegating default bodies; native
/// typed overrides (ConstantFolder) skip this entirely.
pub(super) fn narrow_folded_int<'ctx, W, B>(
    folded: Option<Value<'ctx, B>>,
    like: IntValue<'ctx, W, B>,
) -> IrResult<Option<IntValue<'ctx, W, B>>>
where
    W: IntWidth,
    B: ModuleBrand + 'ctx,
{
    let Some(v) = folded else { return Ok(None) };
    if v.ty().id() != like.as_value().ty().id() {
        return Err(IrError::TypeMismatch {
            expected: like.as_value().ty().kind_label(),
            got: v.ty().kind_label(),
        });
    }
    Ok(Some(IntValue::from_value_unchecked(v)))
}
// narrow_folded_fp mirrors narrow_folded_int over FloatValue<K>.
// narrow_folded_bool checks `matches!(v.ty().kind(), TypeKind::Integer { bits: 1 })`
//   and wraps IntValue::<bool, B>::from_value_unchecked.
// narrow_folded_cast_int / narrow_folded_cast_fp compare against
//   dest_ty.as_type().id() instead of an operand.
```

Write the three commented siblings out in full (same shape, different check). Adapt small API details to reality via `lsp hover`: the `IntPredicate -> CmpPredicate` conversion (`predicate.into()`) — check `cmp_predicate.rs` for the real `From` impl or constructor and use it; `IrError::TypeMismatch` field spelling; `Value::ty()` / `Type::id()` / `kind_label()` accessor names (all used widely in `ir_builder.rs` — copy the local idiom).

- [ ] **Step 5: ConstantFolder native typed overrides**

In `constant_folder.rs`, add overrides for all 9 typed hooks. Pattern (the binop case; the others follow the same shape against their kernel):

```rust
fn fold_int_bin_op<W: IntWidth>(
    &self,
    opcode: BinaryOpcode,
    lhs: IntValue<'ctx, W, B>,
    rhs: IntValue<'ctx, W, B>,
) -> IrResult<Option<IntValue<'ctx, W, B>>> {
    // Kernel invariant: binary folds preserve the LHS operand type
    // (mirrors BinaryOperator result typing, lib/IR/Instructions.cpp),
    // so the unchecked wrap cannot mistype.
    Ok(self
        .fold_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())?
        .map(IntValue::from_value_unchecked))
}
```

REQUIRED sub-step — kernel-invariant audit: before writing each override, read the kernel the `_dyn` body dispatches to (`constant_fold_binary_instruction`, `constant_fold_cast_instruction`, `constant_expr` constructors in `crates/llvmkit-ir/src/constant_fold*.rs` / `module.rs`) and confirm in one comment line per override that the result type is structurally pinned (binops: operand type; casts: exactly `dest_ty`; cmps: i1). If any kernel path can produce a different type, DO NOT write the unchecked override for that hook — keep the inherited checked default and note why in a comment. The cmp overrides wrap with `IntValue::<bool, B>::from_value_unchecked` only after confirming the cmp kernel always produces i1 for scalar operands.

- [ ] **Step 6: Rewire the typed builder paths**

In `ir_builder.rs`, add two private helpers on the Positioned impl block (near `checked_folded_value` ~5782):

```rust
/// Accept a typed fold result. For static markers this is the identity —
/// the type system already guarantees the width/kind. For dyn markers
/// (IntDyn) the marker doesn't pin the width, so keep a TypeId check.
/// The branch monomorphizes away for static W.
fn accept_folded_int<W: IntWidth>(
    &self,
    folded: IntValue<'ctx, W, B>,
    like: IntValue<'ctx, W, B>,
) -> IrResult<IntValue<'ctx, W, B>> {
    if W::static_bits().is_none()
        && folded.as_value().ty().id() != like.as_value().ty().id()
    {
        return Err(IrError::TypeMismatch {
            expected: like.as_value().ty().kind_label(),
            got: folded.as_value().ty().kind_label(),
        });
    }
    Ok(folded)
}
// accept_folded_fp<K: FloatKind> mirrors this, keyed on K::ieee_label().is_none().
```

Then rewire each typed path from erased hook + `checked_folded_value` + `from_value_unchecked` to typed hook + `accept_folded_*`. The typed int binops all funnel through the `build_int_binop` / `build_int_binop_flagged` helpers (~1145-1221) — rewire THERE (before/after):

```rust
// BEFORE (inside build_int_binop, exact shape per the current file):
if let Some(folded) = self.folder.fold_bin_op_dyn(opcode, lhs.as_value(), rhs.as_value())? {
    let folded = self.checked_folded_value(folded, lhs.ty().as_type().id())?;
    return Ok(IntValue::<W, B>::from_value_unchecked(folded));
}
// AFTER:
if let Some(folded) = self.folder.fold_int_bin_op(opcode, lhs, rhs)? {
    return Ok(self.accept_folded_int(folded, lhs)?);
}
```

Sweep the remaining typed families the same way — find every erased-hook call site with `lsp references` on each `fold_*_dyn` method and classify: calls made from a method whose operands are typed handles (`IntValue<W>` / `FloatValue<K>`) and whose result re-narrows via `from_value_unchecked` switch to the typed hook; calls from `_dyn` builder methods stay on the `_dyn` hook + `checked_folded_value` (unchanged semantics). The typed families to convert: int binops (via the two helpers), flagged int binops (`fold_int_bin_op_no_wrap` / `_exact`), fp binops + fneg (`fold_fp_bin_op` / `fold_fp_un_op`), `build_int_cmp` + per-predicate icmp wrappers (`fold_int_cmp`), `build_fp_cmp*` + per-predicate fcmp wrappers (`fold_fp_cmp`), typed casts with int dest (`build_trunc`/`build_zext`/`build_sext`/`build_fp_to_si`/`build_fp_to_ui`/`build_ptr_to_int` → `fold_cast_to_int`), typed casts with fp dest (`build_fp_ext`/`build_fp_trunc`/`build_si_to_fp`/`build_ui_to_fp` → `fold_cast_to_fp`). GEP/select/vector/aggregate/pointer-dest-cast sites stay erased (documented in Step 4's trait comment).

- [ ] **Step 7: Tests**

(a) Compile-fail example-lock `crates/llvmkit-ir/tests/compile_fail/folder_typed_wrong_width.rs` — a custom folder whose `fold_int_bin_op<i32>` tries to return `IntValue<'ctx, i64, B>`:

```rust
//! A custom folder cannot return a wrong-width typed fold result:
//! the typed hook's signature pins the width. Example-lock (C++ has
//! no static analog; nearest family: IRBuilderFolder.h contract).
use llvmkit_ir::{BinaryOpcode, IRBuilderFolder, IntValue, IrResult, ModuleBrand};

struct BadFolder;

impl<'ctx, B: ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B> for BadFolder {
    fn fold_int_bin_op<W: llvmkit_ir::IntWidth>(
        &self,
        _opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        _rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let wide: IntValue<'ctx, i64, B> = todo!();
        Ok(Some(wide)) //~ ERROR mismatched types
    }
}

fn main() {}
```

(b) Typed-vs-dyn parity: in the existing custom-folder/constant-folder test file, build the same `add i32 7, 9` through `build_int_add::<i32, _, _, _>` (typed hook path) and through `build_int_add_dyn` (erased path) with `ConstantFolder`; assert both produce the identical folded constant and identical printed module. Cite the same upstream `ConstantsTest.cpp` anchor rows the file already uses.

(c) NoFolder still materializes instructions and preserves names — extend the existing NoFolder coverage (port anchor: `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`, at `orig_cpp/.../llvm/unittests/IR/IRBuilderTest.cpp`): with `NoFolder`, `build_int_add::<i32>(const 1, const 2, "x")` must produce an `add` instruction named `%x`, not a constant.

(d) Dyn-marker fold keeps its runtime check: a test folder whose `fold_int_bin_op<IntDyn>` override returns a 64-bit value for 32-bit operands must yield `IrError::TypeMismatch` through `build_int_add_dyn`... (route the value through the `IntValue<IntDyn>` identity lift). This locks the `accept_folded_int` dyn seam.

- [ ] **Step 8: Gate, UPSTREAM.md, commit**

```bash
TRYBUILD=overwrite cargo test -p llvmkit-ir --test typestate_compile_fail
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt
```
Register all new tests in UPSTREAM.md. Commit:

```bash
git add -A
git commit -m "Rebuild IRBuilderFolder around typed fold hooks (D3, D4)

17 erased hooks renamed *_dyn with default decline-to-fold bodies
(NoFolder is now an empty impl); is_exact bool dropped; nuw/nsw bools
-> OverflowFlags; fold_binary_intrinsic split on its Option param. New
typed hooks fold_int_bin_op<W>/fold_fp_bin_op<K>/fold_int_cmp<W>/
fold_cast_to_int<W>/... let typed build paths skip runtime
re-narrowing; ConstantFolder overrides them natively under audited
kernel invariants; dyn markers keep a TypeId check that monomorphizes
away for static widths.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Typed memory overlay — `TypedPointerValue<T>` + compile-time field GEPs (2b, P1)

**Files:**
- Create: `crates/llvmkit-ir/src/typed_pointer_value.rs`
- Modify: `crates/llvmkit-ir/src/struct_schema.rs` (add `StructFieldAt` + `FieldOf`)
- Modify: `crates/llvmkit-ir/src/ir_builder.rs` (six new methods)
- Modify: `crates/llvmkit-ir/src/lib.rs` (+umbrella crate re-exports)
- Test: `crates/llvmkit-ir/tests/builder_typed_memory.rs` (new), `crates/llvmkit-ir/tests/compile_fail/typed_gep_bad_index.rs` (+`.stderr`)
- Modify: `UPSTREAM.md`

**Interfaces:**
- Consumes: `IrField` / `IntoIrField` / `StructSchema` (struct_schema.rs, read in full above), existing `build_alloca` / `build_load` / `build_store` / `build_struct_gep` / `build_gep` public methods.
- Produces: `TypedPointerValue<'ctx, T: IrField, B>` with `as_pointer_value()` / `as_value()` / `IntoPointerValue` impl; `PointerValue::with_pointee::<T>()`; `StructFieldAt<const I: u32>` + `type FieldOf<S, I>`; builder methods `build_typed_alloca` / `build_typed_load` / `build_typed_load_with_align` / `build_typed_store` / `build_typed_store_with_align` / `build_field_gep` / `build_element_gep` / `build_inbounds_element_gep`. Task 20's docs advertise these.

- [ ] **Step 1: Write the failing round-trip test**

`crates/llvmkit-ir/tests/builder_typed_memory.rs`:

```rust
//! TypedPointerValue: Rust-side static pointee overlay on opaque `ptr`.
//! Example-locks (opaque pointers have no upstream typed analog); the IR
//! shape is anchored on the existing alloca/load/store fixtures and
//! `test/Assembler/getelementptr_struct.ll` for the field-GEP form.

use llvmkit_ir::{IrResult, IrStruct, Linkage, Module};

#[derive(IrStruct)]
struct CpuState {
    flags: i32,
    pc: i64,
}

#[test]
fn typed_alloca_load_store_round_trip_prints_identically_to_erased() -> IrResult<()> {
    let typed = Module::with_new("m", |m| {
        let f = m.add_typed_function::<i32, (i32,), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let (x,) = f.params();
        let slot = b.build_typed_alloca::<i32, _>("slot")?;
        b.build_typed_store(x, slot)?;
        let v = b.build_typed_load(slot, "v")?; // IntValue<'_, i32, _> — no try_into
        b.build_ret(v)?;
        Ok(format!("{m}"))
    })?;
    let erased = Module::with_new("m", |m| {
        let f = m.add_typed_function::<i32, (i32,), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let (x,) = f.params();
        let slot = b.build_alloca(m.i32_type(), "slot")?;
        b.build_store(x, slot)?;
        let v = b.build_int_load::<i32, _, _>(slot, "v")?;
        b.build_ret(v)?;
        Ok(format!("{m}"))
    })?;
    assert_eq!(typed, erased, "typed overlay must not change printed IR");
    Ok(())
}

#[test]
fn field_gep_projects_field_type_at_compile_time() -> IrResult<()> {
    Module::with_new("m", |m| {
        let f = m.add_typed_function::<i64, (), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.builder(&m).position_at_end(entry);
        let cpu = b.build_typed_alloca::<CpuState, _>("cpu")?;
        let pc_ptr = b.build_field_gep::<CpuState, 1, _>(cpu, "pc.ptr")?; // TypedPointerValue<i64>
        let pc = b.build_typed_load(pc_ptr, "pc")?; // IntValue<'_, i64, _>
        b.build_ret(pc)?;
        let printed = format!("{m}");
        assert!(printed.contains("getelementptr inbounds %CpuState, ptr %cpu, i32 0, i32 1"));
        Ok(())
    })
}
```

Adapt construction idioms (`add_typed_function`, `build_int_load` arg order, the exact GEP print form — check what `build_struct_gep` currently prints in an existing fixture) with `lsp hover` / existing tests. Run: `cargo test -p llvmkit-ir --test builder_typed_memory` — expected: FAIL (methods don't exist).

- [ ] **Step 2: Create `typed_pointer_value.rs`**

```rust
//! Rust-side static pointee overlay on opaque pointers.
//!
//! [`TypedPointerValue`] wraps a plain [`PointerValue`] and remembers a
//! pointee schema `T: IrField` at the type level. It is compile-time
//! bookkeeping only: the wrapped value's IR type is a plain opaque
//! pointer and printed IR is byte-identical to the erased path.
//! Unrelated to [`crate::TypedPointerType`], which is the IR-level
//! (GPU-only) typed-pointer *type* and prints differently.

use core::marker::PhantomData;

use crate::error::IrResult;
use crate::module::{Brand, ModuleBrand, ModuleRef};
use crate::struct_schema::IrField;
use crate::value::{IntoPointerValue, PointerValue, Value};

/// Opaque `ptr` value plus a phantom pointee schema `T`.
pub struct TypedPointerValue<'ctx, T: IrField, B: ModuleBrand = Brand<'ctx>> {
    ptr: PointerValue<'ctx, B>,
    _pointee: PhantomData<fn() -> T>,
}

impl<'ctx, T: IrField, B: ModuleBrand> Clone for TypedPointerValue<'ctx, T, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, T: IrField, B: ModuleBrand> Copy for TypedPointerValue<'ctx, T, B> {}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx> TypedPointerValue<'ctx, T, B> {
    #[inline]
    pub(crate) fn from_pointer(ptr: PointerValue<'ctx, B>) -> Self {
        Self { ptr, _pointee: PhantomData }
    }

    /// Erase the pointee schema (D3 opt-out).
    #[inline]
    pub fn as_pointer_value(self) -> PointerValue<'ctx, B> {
        self.ptr
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        self.ptr.as_value()
    }
}

impl<'ctx, T: IrField, B: ModuleBrand + 'ctx> IntoPointerValue<'ctx, B>
    for TypedPointerValue<'ctx, T, B>
{
    #[inline]
    fn into_pointer_value(self, module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        self.ptr.into_pointer_value(module)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> PointerValue<'ctx, B> {
    /// Attach a pointee schema. This is an *assertion*, not a checked
    /// conversion — opaque pointers carry nothing to check against. A
    /// mis-assertion is exactly as (un)safe as passing the wrong type
    /// to `build_load(ty, ptr, ..)` today: wrong IR, caught by the
    /// verifier, never memory-unsafe (D10).
    #[inline]
    pub fn with_pointee<T: IrField>(self) -> TypedPointerValue<'ctx, T, B> {
        TypedPointerValue::from_pointer(self)
    }
}
```

Add manual `Debug`/`PartialEq`/`Eq`/`Hash` impls delegating to `ptr` (PhantomData pattern, as `TypedFunctionValue` does). Register the module in `lib.rs` (`mod typed_pointer_value;` + `pub use`). If `IntoPointerValue`'s method has a different receiver idiom, mirror the `PointerValue` identity impl at `value.rs:1533`.

- [ ] **Step 3: Add `StructFieldAt` to struct_schema.rs**

```rust
/// The `I`-th top-level field schema of a field tuple. Implemented for
/// tuple arities 1..=16, one impl per (arity, index) pair, so an
/// out-of-range index is "no impl" — a compile error at the
/// `build_field_gep::<S, I>` call site.
pub trait StructFieldAt<const I: u32> {
    type Field: IrField;
}

/// Field schema of `S` at index `I`.
pub type FieldOf<S, const I: u32> =
    <<S as StructSchema>::FieldParams as StructFieldAt<I>>::Field;

macro_rules! impl_struct_field_at {
    ($( ($($f:ident),+) => [ $( $idx:literal : $pick:ident ),+ ] );+ $(;)?) => {$($(
        impl<$($f: IrField),+> StructFieldAt<$idx> for ($($f,)+) {
            type Field = $pick;
        }
    )+)+};
}

impl_struct_field_at! {
    (F0) => [0: F0];
    (F0, F1) => [0: F0, 1: F1];
    (F0, F1, F2) => [0: F0, 1: F1, 2: F2];
    (F0, F1, F2, F3) => [0: F0, 1: F1, 2: F2, 3: F3];
    (F0, F1, F2, F3, F4) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4];
    (F0, F1, F2, F3, F4, F5) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5];
    (F0, F1, F2, F3, F4, F5, F6) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6];
    (F0, F1, F2, F3, F4, F5, F6, F7) => [0: F0, 1: F1, 2: F2, 3: F3, 4: F4, 5: F5, 6: F6, 7: F7];
}
```

Extend the macro invocations up to arity 16 following the same shape (write all rows out — they are mechanical but must be present). Note the elements of `FieldParams` tuples for derive-generated schemas are field schema tokens, all of which implement `IrField` (verified: `impl_int_field!` / `impl_float_field!` / `IrField for Ptr` / blanket `IrField for S: StructSchema` in this file).

- [ ] **Step 4: Builder methods**

In `ir_builder.rs`, Positioned impl (near the memory section ~2784). The builder already stores `module: &'m Module<'ctx, B, Unverified>` (verified — `self.module.ptr_type(...)` is used throughout), so `T::ir_type(self.module)` is directly callable:

```rust
/// `alloca` for schema `T`, returning a pointee-typed pointer.
/// Mirrors `IRBuilder::CreateAlloca` + the Rust-side overlay.
pub fn build_typed_alloca<T, Name>(
    &self,
    name: Name,
) -> IrResult<TypedPointerValue<'ctx, T, B>>
where
    T: IrField,
    Name: AsRef<str>,
{
    let ty = T::ir_type(self.module)?;
    let ptr = self.build_alloca(ty, name)?;
    Ok(ptr.with_pointee::<T>())
}

/// Typed `load`: the result type is derived from the pointer's schema.
pub fn build_typed_load<T, Name>(
    &self,
    ptr: TypedPointerValue<'ctx, T, B>,
    name: Name,
) -> IrResult<T::Value<'ctx, B>>
where
    T: IrField,
    Name: AsRef<str>,
{
    let ty = T::ir_type(self.module)?;
    let raw = self.build_load(ty, ptr.as_pointer_value(), name)?;
    T::value_from_ir_value(raw)
}

/// Typed `store`: the value lifts through the schema's IntoIrField.
pub fn build_typed_store<T, V>(
    &self,
    value: V,
    ptr: TypedPointerValue<'ctx, T, B>,
) -> IrResult<StoreInst<'ctx, B>>
where
    T: IrField,
    V: IntoIrField<'ctx, T, B>,
{
    let v = value.into_ir_field(ModuleRef::new(self.module))?;
    self.build_store(v, ptr.as_pointer_value())
}

/// `getelementptr inbounds %S, ptr %p, i32 0, i32 I` with the field
/// type projected at compile time. Out-of-range `I` fails to compile.
/// Mirrors `IRBuilder::CreateStructGEP`.
pub fn build_field_gep<S, const I: u32, Name>(
    &self,
    ptr: TypedPointerValue<'ctx, S, B>,
    name: Name,
) -> IrResult<TypedPointerValue<'ctx, FieldOf<S, I>, B>>
where
    S: StructSchema,
    S::FieldParams: StructFieldAt<I>,
    Name: AsRef<str>,
{
    let struct_ty = S::ir_type(self.module)?;
    let raw = self.build_struct_gep(struct_ty, ptr.as_pointer_value(), I, name)?;
    let raw_ptr = PointerValue::try_from(raw)?;
    Ok(raw_ptr.with_pointee::<FieldOf<S, I>>())
}

/// `getelementptr T, ptr %p, <idx>` — element-stride arithmetic; the
/// pointee schema is preserved. Mirrors the 1-index `IRBuilder::CreateGEP`.
pub fn build_element_gep<T, W, Idx, Name>(
    &self,
    ptr: TypedPointerValue<'ctx, T, B>,
    index: Idx,
    name: Name,
) -> IrResult<TypedPointerValue<'ctx, T, B>>
where
    T: IrField,
    W: IntWidth,
    Idx: IntoIntValue<'ctx, W, B>,
    Name: AsRef<str>,
{
    let elem_ty = T::ir_type(self.module)?;
    let idx = index.into_int_value(ModuleRef::new(self.module))?;
    let raw = self.build_gep(elem_ty, ptr.as_pointer_value(), [idx], name)?;
    let raw_ptr = PointerValue::try_from(raw)?;
    Ok(raw_ptr.with_pointee::<T>())
}
```

Add `build_typed_load_with_align` / `build_typed_store_with_align` (same bodies via the `_with_align` inner methods) and `build_inbounds_element_gep` (via `build_inbounds_gep`). Adapt inner-call signatures to reality with `lsp hover` on `build_alloca` / `build_load` / `build_store` / `build_struct_gep` / `build_gep` — if `build_alloca`'s type parameter refuses an erased `Type<'ctx, B>`, route through the crate-internal `build_alloca_inner` (~2834) the way `build_alloca` itself does. If `build_struct_gep` returns `PointerValue` directly, drop the `try_from`.

- [ ] **Step 5: Compile-fail lock for bad field index**

`crates/llvmkit-ir/tests/compile_fail/typed_gep_bad_index.rs`: `b.build_field_gep::<CpuState, 7, _>(cpu, "x")` on the 2-field `CpuState` → expected error: `StructFieldAt<7>` is not implemented. Regenerate golden via `TRYBUILD=overwrite`.

- [ ] **Step 6: Run, register, commit**

```bash
cargo test -p llvmkit-ir --test builder_typed_memory && cargo test -p llvmkit-ir
```
UPSTREAM.md rows (example-locks + `getelementptr_struct.ll` anchor). Commit:

```bash
git add -A
git commit -m "Add TypedPointerValue overlay + compile-time field GEPs (D4, D6)

Opaque-ptr values can carry a Rust-side pointee schema: typed
alloca/load/store skip runtime narrowing, and build_field_gep::<S, I>
projects the field type from the IrStruct schema at compile time
(out-of-range index = no StructFieldAt impl = compile error). Printed
IR is byte-identical to the erased path.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Typed flag-parity variants (2d, P1)

**Files:**
- Modify: `crates/llvmkit-ir/src/ir_builder.rs` (four new typed methods; reorder one `_dyn` signature)
- Modify: `crates/llvmkit-asmparser/src/ll_parser.rs:~6332` (param-order migration)
- Test: extend `crates/llvmkit-ir/tests/medium_builder_cmp.rs` and `medium_builder_cast.rs`
- Modify: `UPSTREAM.md`

**Interfaces:**
- Consumes: existing flag structs `ICmpFlags` / `ZExtFlags` / `TruncFlags` / `UIToFpFlags` (instr_types.rs — confirm exact names via `lsp references` on `build_int_cmp_with_flags_dyn` / `build_zext_with_flags_dyn` / `build_trunc_with_flags_dyn` / `build_ui_to_fp_with_flags_dyn` and reuse whatever those take), `WiderThan` / `FloatWiderThan` bounds, Task 5's typed fold hooks.
- Produces: `build_int_cmp_with_flags`, `build_zext_with_flags`, `build_trunc_with_flags`, `build_ui_to_fp_with_flags` (typed); `build_int_cmp_with_flags_dyn` reordered to `(pred, lhs, rhs, flags, name)`.

- [ ] **Step 1: Write failing print-parity tests**

Extend the cmp/cast test files (anchor: `test/Assembler/flags.ll` — the extracted tree has 6 `samesign` and 4 `nneg` occurrences; copy the constructive lines):

```rust
/// Ports test/Assembler/flags.ll (samesign line): typed operands, no
/// dyn erasure needed to spell the flag.
#[test]
fn typed_icmp_samesign_prints_flag() -> IrResult<()> {
    Module::with_new("m", |m| {
        /* build i32 params a, b as in the file's existing tests */
        let c = b.build_int_cmp_with_flags::<i32, _, _, _>(
            IntPredicate::Slt, a, b_, ICmpFlags::new().samesign(), "c")?;
        let _ = c;
        assert!(format!("{m}").contains("icmp samesign slt i32"));
        Ok(())
    })
}
```

Mirror for `zext nneg` / `trunc nuw nsw` / `uitofp nneg` against the fixture's exact printed forms. Match the flag-struct constructor spelling to whatever the existing `_dyn` tests use (grep the test tree for `samesign`). Run — expected FAIL (methods missing).

- [ ] **Step 2: Add the four typed methods**

Each is the existing typed method's signature plus the flags parameter, emitting through the same payload path as its `_dyn` twin (read the `_dyn` body and reuse its payload construction; only operand acquisition differs — typed lifts instead of erased values). Signatures:

```rust
pub fn build_int_cmp_with_flags<W, Lhs, Rhs, Name>(
    &self, predicate: IntPredicate, lhs: Lhs, rhs: Rhs, flags: ICmpFlags, name: Name,
) -> IrResult<IntValue<'ctx, bool, B>>
where W: IntWidth, Lhs: IntoIntValue<'ctx, W, B>, Rhs: IntoIntValue<'ctx, W, B>, Name: AsRef<str>;

pub fn build_zext_with_flags<Src, Dst, Name>(
    &self, value: IntValue<'ctx, Src, B>, dst_ty: IntType<'ctx, Dst, B>, flags: ZExtFlags, name: Name,
) -> IrResult<IntValue<'ctx, Dst, B>>
where Src: IntWidth, Dst: IntWidth + WiderThan<Src>, Name: AsRef<str>;

pub fn build_trunc_with_flags<Src, Dst, Name>(
    &self, value: IntValue<'ctx, Src, B>, dst_ty: IntType<'ctx, Dst, B>, flags: TruncFlags, name: Name,
) -> IrResult<IntValue<'ctx, Dst, B>>
where Src: IntWidth + WiderThan<Dst>, Dst: IntWidth, Name: AsRef<str>;

pub fn build_ui_to_fp_with_flags<W, K, V, Name>(
    &self, value: V, dst_ty: FloatType<'ctx, K, B>, flags: UIToFpFlags, name: Name,
) -> IrResult<FloatValue<'ctx, K, B>>
where W: IntWidth, K: FloatKind, V: IntoIntValue<'ctx, W, B>, Name: AsRef<str>;
```

Copy the `WiderThan` direction from the existing `build_zext`/`build_trunc` bounds (verify with `lsp hover` — the plan spells zext as `Dst: WiderThan<Src>` and trunc as `Src: WiderThan<Dst>`; whichever way the existing methods spell it, match them exactly). Doc comments cite the verified upstream facts: samesign is post-hoc `ICmpInst::setSameSign` upstream (llvmkit's construction-time flag is the deliberate improvement); trunc flags are NEVER silently dropped here (D10) — where upstream `CreateTrunc` returns V unchanged for same-type casts and loses the flags, our `WiderThan` bound makes same-type trunc unspellable, so the case cannot arise; document that in the method doc.

- [ ] **Step 3: Reorder `build_int_cmp_with_flags_dyn` and migrate the parser**

Change its parameter order to `(predicate, lhs, rhs, flags, name)` to match the rest of the `*_with_flags*` family. `lsp references` lists the call sites (parser at `ll_parser.rs:~6332` + any tests); reorder the arguments at each.

- [ ] **Step 4: Run, register, commit**

```bash
cargo test && cargo clippy --all-targets -- -D warnings
```
UPSTREAM.md rows citing `test/Assembler/flags.ll` line numbers from the local tree. Commit: `"Add typed flag variants: samesign icmp, nneg zext/uitofp, trunc nuw/nsw (D3, D4)"` with trailer.

---

### Task 8: `as`-cast sweep in int_width.rs (2e.1, P2)

**Files:**
- Modify: `crates/llvmkit-ir/src/int_width.rs` (~20 sites: lines ~174, 208, 226-228, 240-241, 257, 293-295, 310-312, 367, 421, 451-453, 468-469, 482, 503)

**Interfaces:** none new — behavior-neutral. Covered by the existing `ConstantsTest` ports (`Integer_i1`, `IntSigns` rows in UPSTREAM.md).

- [ ] **Step 1: Replace sign-reinterpreting casts**

MSRV is 1.96, so `cast_unsigned()` (stable 1.87) is available. Mechanical rules:
- `x as u64` where `x: i64` (bit-pattern reinterpret) → `x.cast_unsigned()`; same per width (`i8→u8` etc.). Where the cast ALSO widens (`i32 as u64`), split: `i64::from(x).cast_unsigned()`.
- `x as u64` where `x: u128` (deliberate truncation to a half) → the named helper below.
- Pure widening (`u32 as u64`) → `u64::from(x)`.

Add once, near the top of the file's helper section:

```rust
/// Split into (lo, hi) 64-bit halves. Invariant: mask/shift bound each
/// half to 64 bits, so the narrowing conversions cannot fail.
fn u128_halves(bits: u128) -> (u64, u64) {
    let lo = u64::try_from(bits & u128::from(u64::MAX))
        .unwrap_or_else(|_| unreachable!("masked to 64 bits"));
    let hi = u64::try_from(bits >> 64)
        .unwrap_or_else(|_| unreachable!("shifted to 64 bits"));
    (lo, hi)
}
```

Find every site: `grep -n " as u\| as i" crates/llvmkit-ir/src/int_width.rs`. Convert ALL of them; zero `as` casts remain in the file.

- [ ] **Step 2: Verify + commit**

```bash
grep -cn " as u\| as i" crates/llvmkit-ir/src/int_width.rs   # expected: 0 code hits (doc-comment prose hits OK)
cargo test -p llvmkit-ir
```
Existing constant tests are the behavior lock. Commit: `"Remove as-casts from int_width lift chain (conventions)"` with trailer.

---

### Task 9: Builder sentinel / masking fixes (2e.2, 2e.3, 2e.4, 2f.3, 2f.5)

**Files:**
- Modify: `crates/llvmkit-ir/src/ir_builder.rs` (~4478 splat; ~5119-5125 + ~5272-5278 invoke/callbr; ~6317-6359 walk_aggregate; `build_load_inner` unused param)
- Modify: `crates/llvmkit-ir/src/typed_pointer_type.rs:52,61` (expect → invariant unreachable)
- Modify: `crates/llvmkit-ir/src/error.rs` (new variant)
- Test: extend the aggregate-op test file (find: `grep -rl "extract_value" crates/llvmkit-ir/tests/`)

**Interfaces:**
- Produces: `IrError::AggregateIndexOutOfRange { index: u32, count: u64 }` (error.rs; follow the exact `#[error(...)]`/Display style of the neighboring variants).

- [ ] **Step 1: e.2 — splat sentinel (~4478)**

```rust
// BEFORE: let n = usize::try_from(count).unwrap_or(usize::MAX);
// AFTER:
let n = usize::try_from(count).map_err(|_| IrError::InvalidOperation {
    message: "vector splat lane count exceeds the platform address range",
})?;
```

- [ ] **Step 2: e.3 — invoke/callbr return-type extraction (~5119-5125, ~5272-5278)**

Replace the `unwrap_or(fn_ty)` decomposition with the direct accessors, mirroring `build_call`'s arm (`callee.signature()` returns `FunctionType<'ctx, B>` — verified at `call_builder`, ir_builder.rs:3449):

```rust
let sig = callee.signature();
let fn_ty = sig.as_type().id();
let ret_ty = sig.return_type().id();
```

- [ ] **Step 3: e.4 — walk_aggregate_for_builder u64 domain (~6317-6359)**

Replace both `unwrap_or(u32::MAX)` sentinels: compare in the u64 domain (`if u64::from(idx) >= count_u64`), index the elements slice via `usize::try_from(idx)` with the out-of-range error, and report `IrError::AggregateIndexOutOfRange { index: idx, count: count_u64 }` instead of clamping. Mirrors `ExtractValueInst::getIndexedType` (`orig_cpp/.../llvm/lib/IR/Instructions.cpp`) — read it first. Add the error variant to error.rs. Update any test matching the old `ArgumentIndexOutOfRange` on aggregate paths (`lsp references` / grep) to the new variant.

- [ ] **Step 4: f.3 + f.5**

`typed_pointer_type.rs:52,61`: `expect("...")` → `unwrap_or_else(|| unreachable!("TypedPointerType invariant: constructor verified TypeData::TypedPointer"))` (adjust to `match` if the receiver is an Option pattern that reads better). `build_load_inner`: delete the unused `_ptr` parameter and update its (private) callers.

- [ ] **Step 4b: 2c — document the no-parameterization decision + `LoadInst::loaded_ty`**

Add to the `instructions.rs` module doc (top of file), after the existing prose:

```rust
//! ## Why arithmetic/memory handles carry no type parameters
//!
//! `CallInst<R>` / `PhiInst<W, P>` carry markers because the builder
//! returns them typed and the marker gates real accessors. `AddInst`,
//! `LoadInst`, and the other per-opcode handles do not: the typed
//! information already lives on the value handles the builder returns
//! (D4 — `build_int_add::<W>` returns `IntValue<W>`), and the handles'
//! reachable constructors are rediscovery paths (`BlockCursor`,
//! `InstructionView`, `TryFrom`) which are inherently dyn-shaped — a
//! marker there would instantiate as `AddInst<IntDyn>` everywhere and
//! gate nothing.
```

If `LoadInst` lacks a loaded-type accessor (grep `fn loaded_ty` / read the LoadInst impl), add:

```rust
/// The loaded type (the instruction's result type).
#[inline]
pub fn loaded_ty(self) -> Type<'ctx, B> {
    Type::new(self.ty, self.module)
}
```
(match the field/constructor idiom of the neighboring accessors).

- [ ] **Step 5: Test + commit**

Add one aggregate out-of-range test asserting the new variant (anchor: `test/Assembler/extractvalue-invalid-idx.ll` in the local tree). `cargo test -p llvmkit-ir`. Commit: `"Replace sentinel/masking fallbacks with typed errors (D10)"` + trailer.

---

### Task 10: `FloatWiderThan` rows for X86Fp80 / Fp128 / PpcFp128 (2e.5, P2)

**Files:**
- Modify: `crates/llvmkit-ir/src/float_kind.rs` (~121-130, the `FloatWiderThan` impl block)
- Test: extend `crates/llvmkit-ir/tests/medium_builder_cast.rs`; new compile-fail `fp_ext_equal_width.rs` (+`.stderr`)

**Interfaces:**
- Produces: `X86Fp80: FloatWiderThan<Half|BFloat|f32|f64>`, `Fp128: FloatWiderThan<... + X86Fp80>`, `PpcFp128: FloatWiderThan<... + X86Fp80>` so `build_fp_ext::<f64, X86Fp80>` etc. compile.

- [ ] **Step 1: Read the upstream rule + the existing impl shape**

Read `CastInst::castIsValid`'s FPExt/FPTrunc arm in `orig_cpp/.../llvm/lib/IR/Instructions.cpp` — it legalizes on a STRICT `getPrimitiveSizeInBits` inequality. Sizes: Half=16, BFloat=16, f32=32, f64=64, X86Fp80=80, Fp128=128, PpcFp128=128. Then read `float_kind.rs:121-130` for the existing impl idiom (direct impls or a macro).

- [ ] **Step 2: Add the rows (follow the existing idiom exactly)**

```rust
// New wider-than rows: castIsValid is a strict size comparison, so the
// non-IEEE layouts participate. 80 > 64/32/16; 128 > 80.
impl FloatWiderThan<Half> for X86Fp80 {}
impl FloatWiderThan<BFloat> for X86Fp80 {}
impl FloatWiderThan<f32> for X86Fp80 {}
impl FloatWiderThan<f64> for X86Fp80 {}
impl FloatWiderThan<X86Fp80> for Fp128 {}
impl FloatWiderThan<X86Fp80> for PpcFp128 {}
impl FloatWiderThan<Half> for PpcFp128 {}
impl FloatWiderThan<BFloat> for PpcFp128 {}
impl FloatWiderThan<f32> for PpcFp128 {}
impl FloatWiderThan<f64> for PpcFp128 {}
// Deliberately absent: Fp128 <-> PpcFp128 (both 128 bits) and
// Half <-> BFloat (both 16 bits) — castIsValid requires a STRICT
// size inequality, so neither direction is a valid fpext/fptrunc.
```

Check which of these rows already exist (Fp128 over f64 etc. probably do) and add only the missing ones; if a macro generates the block, extend the macro invocation instead.

- [ ] **Step 3: Tests + commit**

Positive: `build_fp_ext::<f64, X86Fp80>` compiles and prints `fpext double %x to x86_fp80` (anchor: `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` matrix — read it in the local tree and cite the exact assertions ported). Negative compile-fail `fp_ext_equal_width.rs`: `build_fp_ext::<Fp128, PpcFp128>` → no `FloatWiderThan` impl. `TRYBUILD=overwrite` regen; UPSTREAM.md rows; commit `"Extend FloatWiderThan to non-IEEE layouts per castIsValid (D4)"` + trailer.

---

### Task 11: Non-empty extract/insertvalue index lists (2e.6, P1)

**Files:**
- Modify: `crates/llvmkit-ir/src/ir_builder.rs` (`build_extract_value` / `build_insert_value` + new `_dyn` twins)
- Modify: `crates/llvmkit-asmparser/src/ll_parser.rs` (~2 call sites → `_dyn`)
- Test: aggregate test file + compile-fail `extract_value_empty_indices.rs` (+`.stderr`)

**Interfaces:**
- Produces: `build_extract_value<V, const N: usize, Name>(aggregate: V, indices: [u32; N], name)` with `const { assert!(N > 0) }`; `build_extract_value_dyn(..., indices: &[u32], ...)` keeping the runtime empty-check; same pair for insert_value (which also keeps its element-value parameter).

- [ ] **Step 1: Split the methods**

Current methods take `indices: IntoIterator<Item = u32>`-ish (read the exact signatures at ~2167-2231 first). New shapes:

```rust
pub fn build_extract_value<V, const N: usize, Name>(
    &self,
    aggregate: V,
    indices: [u32; N],
    name: Name,
) -> IrResult<Value<'ctx, B>>
where
    V: IsValue<'ctx, B>,
    Name: AsRef<str>,
{
    const {
        assert!(N > 0, "extractvalue requires at least one index");
    }
    self.build_extract_value_dyn(aggregate, &indices, name)
}

pub fn build_extract_value_dyn<V, Name>(
    &self,
    aggregate: V,
    indices: &[u32],
    name: Name,
) -> IrResult<Value<'ctx, B>>
where
    V: IsValue<'ctx, B>,
    Name: AsRef<str>,
{
    // former build_extract_value body, keeping the runtime
    // empty-indices IrError (ports test/Assembler/extractvalue-no-idx.ll)
}
```

Mirror for `build_insert_value` / `build_insert_value_dyn`. Existing array-literal call sites (`[0]`, `[1, 2]`) compile unchanged; `lsp references` finds iterator-style callers — migrate the parser's extractvalue/insertvalue arms (~7174 region) and any tests passing `Vec`/slices to `_dyn`.

- [ ] **Step 2: Tests + commit**

Compile-fail: `build_extract_value(agg, [], "x")` → const-assert failure (example-lock; the runtime `_dyn` twin's empty-slice rejection ports `test/Assembler/extractvalue-no-idx.ll`). Full gate; UPSTREAM.md; commit `"Const-assert non-empty aggregate index lists; _dyn keeps runtime check (D3)"` + trailer.

---

### Task 12: Hardening parity checks from LLVM 21/22 release notes

**Files:**
- Verify (and fix only if wrong): `build_struct_gep` no-wrap flags, `build_ptr_to_addr` result-type derivation, `AtomicRMWBinOp` fmaximum/fminimum, constant folder `mul` constexpr

**Interfaces:** none new — this is a verification checklist with fixes only on divergence.

- [ ] **Step 1: struct GEP flags** — upstream `CreateStructGEP` passes `GEPNoWrapFlags::inBounds() | GEPNoWrapFlags::noUnsignedWrap()` (verified in IRBuilder.h @ 22.1.4). Read llvmkit's `build_struct_gep` (~3650s): if it emits only `inbounds`, fix to include `nuw` and lock the printed form (`getelementptr inbounds nuw` — check the AsmWriter's GepNoWrapFlags printing + `test/Assembler/getelementptr_struct.ll` expected text first; if our parser/writer don't support `nuw` on GEP yet, record the gap in docs/future-work.md instead of half-fixing).
- [ ] **Step 2: ptrtoaddr** — `CreatePtrToAddr` derives the result from `DataLayout.getAddressType(V->getType())`. Read `build_ptr_to_addr`: confirm it derives from DataLayout's index/address size for the pointer's address space rather than taking a caller type; fix to match if not.
- [ ] **Step 3: atomicrmw fmaximum/fminimum** — `grep -n "fmaximum\|fminimum" crates/llvmkit-ir/src/` — if the `AtomicRMWBinOp` enum lacks them, add variants + AsmWriter arm + verifier acceptance + parser keyword (they are in LLVM 21's `AtomicRMWInst::BinOp`; read `Instructions.h`); port an `test/Assembler/atomic*.ll` line as the lock. If present, no-op.
- [ ] **Step 4: mul constexpr** — `grep -n "Mul" crates/llvmkit-ir/src/constant*.rs module.rs` fold kernels: confirm no `ConstantExpr` mul is EMITTED (folding to a literal constant is fine; a `mul` constant-expression node is not — removed upstream in LLVM 21). Fix + lock if violated.
- [ ] **Step 5: Commit** (only if fixes were needed): `"Align struct-GEP flags / ptrtoaddr typing / atomicrmw ops with LLVM 22 (parity)"` + trailer. If everything already matched, note that in the workstream summary instead.

---

**WORKSTREAM GATE (hardening complete):**

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt -- --check
```
All green before starting Task 13.

---

### Task 13: Typed-call argument traits — `IntoCallArg<P>` + `CallArgs<Params>`

**Files:**
- Modify: `crates/llvmkit-ir/src/function_signature.rs` (traits + int/float/Width/Ptr impls)
- Modify: `crates/llvmkit-ir/src/struct_schema.rs` (StructSchema-source impls + `StructFields` delegation)
- Modify: `crates/llvmkit-macros/src/ir_struct.rs` (derive emits the wrapper impl)
- Modify: `crates/llvmkit-ir/src/lib.rs` (re-exports)
- Test: `crates/llvmkit-ir/tests/function_signature.rs` (compile-and-lower unit additions)

**Interfaces:**
- Consumes: `IntoIntValue<W>` / `IntoFloatValue<K>` / `IntoPointerValue` lift traits; `FunctionParam` / `FunctionParamList` (read in full — tuple macro at function_signature.rs:653-713); `StructFields<S>` (struct_schema.rs:500).
- Produces (Tasks 14-16 depend on these exact names):
  - `pub trait IntoCallArg<'ctx, P: FunctionParam, B: ModuleBrand = Brand<'ctx>>: Sized { #[doc(hidden)] fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>; }`
  - `pub trait CallArgs<'ctx, Params: FunctionParamList, B: ModuleBrand = Brand<'ctx>>: Sized { #[doc(hidden)] fn lower(self, module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>>; }` (sealed)

- [ ] **Step 1: Add `IntoCallArg` and its impls to function_signature.rs**

New imports needed at the top: `ModuleRef`, `Value`, `ValueId`, `IntoIntValue`, `IntoFloatValue`, `IntoPointerValue`, `IntDyn` (mirror the import style of struct_schema.rs). Then:

```rust
/// Inputs that can fill the call-argument slot described by schema
/// token `P` in a typed call. Mirrors the multi-source posture of
/// [`IntoIrField`]: typed handles, constants, Rust literals,
/// `Argument`, and erased `Value` all lift through the underlying
/// operand traits. Cross-module rejection lives inside those traits'
/// `into_*_value(module)` methods (D7), not at the call site.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot fill a call-argument slot of schema `{P}`",
    label = "wrong argument type for this parameter position"
)]
pub trait IntoCallArg<'ctx, P: FunctionParam, B: ModuleBrand = Brand<'ctx>>: Sized {
    #[doc(hidden)]
    fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

macro_rules! impl_into_call_arg_int {
    ($($w:ty),+ $(,)?) => {$(
        impl<'ctx, B, V> IntoCallArg<'ctx, $w, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoIntValue<'ctx, $w, B>,
        {
            #[inline]
            fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_int_value(module)?.as_value())
            }
        }
    )+};
}
impl_into_call_arg_int!(bool, i8, i16, i32, i64, i128);

impl<'ctx, B, V, const N: u32> IntoCallArg<'ctx, Width<N>, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoIntValue<'ctx, Width<N>, B>,
{
    #[inline]
    fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_int_value(module)?.as_value())
    }
}

macro_rules! impl_into_call_arg_float {
    ($($k:ty),+ $(,)?) => {$(
        impl<'ctx, B, V> IntoCallArg<'ctx, $k, B> for V
        where
            B: ModuleBrand + 'ctx,
            V: IntoFloatValue<'ctx, $k, B>,
        {
            #[inline]
            fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(self.into_float_value(module)?.as_value())
            }
        }
    )+};
}
impl_into_call_arg_float!(f32, f64, Half, BFloat, Fp128, X86Fp80, PpcFp128);

impl<'ctx, B, V> IntoCallArg<'ctx, Ptr, B> for V
where
    B: ModuleBrand + 'ctx,
    V: IntoPointerValue<'ctx, B>,
{
    #[inline]
    fn into_call_arg(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.into_pointer_value(module)?.as_value())
    }
}
```

Coherence note (why this compiles): this is byte-for-byte the `IntoIrField` impl topology from struct_schema.rs:265-496, which ships on stable today. The int/float/Ptr blankets never overlap each other (distinct `P`), and the Step 2 StructSchema impls can't unify with them because `i32: StructSchema` etc. have no impl and the trait is crate-local.

- [ ] **Step 2: StructSchema sources + `CallArgs` in struct_schema.rs / function_signature.rs**

In struct_schema.rs (mirrors `impl_struct_into_field!` at :479):

```rust
macro_rules! impl_struct_into_call_arg {
    ($source:ty) => {
        impl<'ctx, S, B> IntoCallArg<'ctx, S, B> for $source
        where
            S: StructSchema,
            B: ModuleBrand + 'ctx,
        {
            fn into_call_arg(self, _module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
                Ok(S::try_value_from_ir(self)?.as_struct_value().as_value())
            }
        }
    };
}
impl_struct_into_call_arg!(Value<'ctx, B>);
impl_struct_into_call_arg!(Argument<'ctx, B>);
impl_struct_into_call_arg!(Constant<'ctx, B>);
impl_struct_into_call_arg!(Instruction<'ctx, Attached, B>);
```

Back in function_signature.rs, the sealed tuple trait:

```rust
mod call_args_sealed {
    pub trait Sealed {}
}

/// Argument tuple for a typed call site: arity must equal
/// `Params::ARITY` and position `i` must satisfy `IntoCallArg<P_i>`.
/// Wrong arity has no impl (compile error); a wrong-typed position
/// fails its `IntoCallArg` bound (compile error).
#[diagnostic::on_unimplemented(
    message = "argument tuple `{Self}` does not match the callee's parameter schema `{Params}`",
    note = "argument count and per-position types must match the callee's typed signature"
)]
pub trait CallArgs<'ctx, Params: FunctionParamList, B: ModuleBrand = Brand<'ctx>>:
    Sized + call_args_sealed::Sealed
{
    #[doc(hidden)]
    fn lower(self, module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>>;
}

impl call_args_sealed::Sealed for () {}
impl<'ctx, B: ModuleBrand + 'ctx> CallArgs<'ctx, (), B> for () {
    #[inline]
    fn lower(self, _module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>> {
        Ok(Box::new([]))
    }
}

macro_rules! impl_call_args_tuple {
    ($($p:ident / $v:ident / $x:ident),+) => {
        impl<$($v),+> call_args_sealed::Sealed for ($($v,)+) {}

        impl<'ctx, B, $($p,)+ $($v,)+> CallArgs<'ctx, ($($p,)+), B> for ($($v,)+)
        where
            B: ModuleBrand + 'ctx,
            $($p: FunctionParam,)+
            $($v: IntoCallArg<'ctx, $p, B>,)+
        {
            fn lower(self, module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>> {
                let ($($x,)+) = self;
                Ok(Box::new([$( $x.into_call_arg(module)?.id, )+]))
            }
        }
    };
}
impl_call_args_tuple!(P0 / V0 / v0);
impl_call_args_tuple!(P0 / V0 / v0, P1 / V1 / v1);
impl_call_args_tuple!(P0 / V0 / v0, P1 / V1 / v1, P2 / V2 / v2);
// ... continue mechanically to arity 16, matching impl_param_list_tuple's ceiling.
```

Write ALL arities out to 16. If `Value`'s `id` field is not directly readable here, use the accessor the crate favors (`lsp hover` on an existing `.id` use in this file's neighbors; `as_value().id` is used crate-wide in instructions.rs). Then in struct_schema.rs, the exploded-fields delegation:

```rust
impl<'ctx, B, S, A> CallArgs<'ctx, StructFields<S>, B> for A
where
    B: ModuleBrand + 'ctx,
    S: StructSchema,
    A: CallArgs<'ctx, S::FieldParams, B>,
{
    fn lower(self, module: ModuleRef<'ctx, B>) -> IrResult<Box<[ValueId]>> {
        <A as CallArgs<'ctx, S::FieldParams, B>>::lower(self, module)
    }
}
```

(`StructFields<S>` and `(P0, ..., Pn)` are distinct type constructors — no overlap.)

- [ ] **Step 3: Derive emits the wrapper impl**

In `crates/llvmkit-macros/src/ir_struct.rs`, alongside the generated `IntoIrField`-adjacent impls, emit (concrete self type — no coherence question):

```rust
impl<'ctx, B: #krate::ModuleBrand + 'ctx> #krate::IntoCallArg<'ctx, #schema_ident, B>
    for #wrapper_ident<'ctx, B>
{
    fn into_call_arg(
        self,
        _module: #krate::ModuleRef<'ctx, B>,
    ) -> #krate::IrResult<#krate::Value<'ctx, B>> {
        Ok(self.as_struct_value().as_value())
    }
}
```

(Read the derive's existing `quote!` blocks first and copy its crate-path/ident hygiene exactly — `#krate` is whatever alias the file already uses.)

- [ ] **Step 4: Compile-and-lower unit test, commit**

Add to `tests/function_signature.rs`: lower `(5_i32, x)` against `Params = (i32, i32)` via `CallArgs::lower` and assert two ValueIds come back (anchor: same rows the file already cites). Run `cargo test -p llvmkit-ir --test function_signature`. Re-export `IntoCallArg`, `CallArgs` from lib.rs. Commit: `"Add IntoCallArg/CallArgs typed-call argument traits (D4, D7)"` + trailer.

---

### Task 14: Derived return — `FunctionReturn::CallResult` GAT + `TypedCallInst`

**Files:**
- Modify: `crates/llvmkit-ir/src/function_signature.rs` (trait + all in-file impls + token)
- Modify: `crates/llvmkit-ir/src/struct_schema.rs` (`FunctionReturn for S` impl)
- Modify: `crates/llvmkit-ir/src/instructions.rs` (`TypedCallInst`; brand-generalize the return-accessor macros)
- Modify: `crates/llvmkit-ir/src/lib.rs`

**Interfaces:**
- Produces: `FunctionReturn` gains `type CallResult<'ctx, B: ModuleBrand + 'ctx>;` and `fn call_result_from_value<'ctx, B>(value: Value<'ctx, B>, validated: &token::ValidatedCallResult<'_>) -> Self::CallResult<'ctx, B>`; `TypedCallInst<'ctx, Ret, B>` with `result()` / `as_call_inst()` / `as_dyn()` / `as_value()`; `CallInst<'ctx, $w, B>` per-marker accessors for all brands. Task 15 constructs `TypedCallInst` via `TypedCallInst::from_call`.

- [ ] **Step 1: Token + trait additions**

In the existing `token` mod (function_signature.rs:28):

```rust
/// Capability proving that a call result's type was validated when the
/// typed callee facade was constructed. Only this crate mints it.
#[derive(Debug)]
pub struct ValidatedCallResult<'a> {
    _private: core::marker::PhantomData<&'a ()>,
}

impl<'a> ValidatedCallResult<'a> {
    pub(crate) fn new() -> Self {
        Self { _private: core::marker::PhantomData }
    }
}
```

Trait additions to `FunctionReturn` (after `expected_kind_label`):

```rust
    /// Branded result handle of a typed call to a callee with this
    /// return schema: `()` for void, `IntValue<'ctx, i32, B>` for
    /// `i32`, `S::Value<'ctx, B>` for a struct schema, etc.
    type CallResult<'ctx, B: ModuleBrand + 'ctx>;

    /// Wrap a raw call result. The token is only minted by this crate
    /// after the callee schema was validated
    /// (`TypedFunctionValue::try_from_function`), so the unchecked
    /// wraps below cannot mistype.
    fn call_result_from_value<'ctx, B>(
        value: Value<'ctx, B>,
        validated: &token::ValidatedCallResult<'_>,
    ) -> Self::CallResult<'ctx, B>
    where
        B: ModuleBrand + 'ctx;
```

- [ ] **Step 2: Update every `FunctionReturn` impl**

Per-impl additions (each is two short items — the associated type and the constructor):

- `()` (line 283): `type CallResult<'ctx, B: ModuleBrand + 'ctx> = ();` constructor body: `let _ = (value, validated);` (returns unit).
- `Ptr` (line 308): `= PointerValue<'ctx, B>;` body: `PointerValue::from_value_unchecked(value)`.
- `impl_int_signature_marker!` macro (line 377): inside the `FunctionReturn` half add `type CallResult<'ctx, B: ModuleBrand + 'ctx> = IntValue<'ctx, $marker, B>;` and body `IntValue::<$marker, B>::from_value_unchecked(value)`.
- `Width<N>` (line 457): `= IntValue<'ctx, Width<N>, B>;` same shape.
- `impl_float_signature_marker!` (line 526): `= FloatValue<'ctx, $marker, B>;` body `FloatValue::<$marker, B>::from_value_unchecked(value)`.
- struct_schema.rs `impl<S: StructSchema> FunctionReturn for S` (line 540): `type CallResult<'ctx, B: ModuleBrand + 'ctx> = S::Value<'ctx, B>;` body:

```rust
fn call_result_from_value<'ctx, B>(
    value: Value<'ctx, B>,
    _validated: &token::ValidatedCallResult<'_>,
) -> Self::CallResult<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    let validated = ValidatedStructValue::new();
    S::Value::from_struct_value(StructValue::from_value_unchecked(value), &validated)
}
```

`from_value_unchecked` visibility: these constructors are crate-internal (verified — used throughout instructions.rs / function_signature.rs); the token gates outside callers.

- [ ] **Step 3: `TypedCallInst` in instructions.rs (below `CallInst`, ~line 604)**

```rust
/// Call handle whose full return schema is carried at the type level.
/// The marker on the inner [`CallInst`] is `Ret::Marker` — derived from
/// the callee by [`crate::IRBuilder::build_call`], never caller-asserted.
pub struct TypedCallInst<'ctx, Ret, B: ModuleBrand = Brand<'ctx>>
where
    Ret: FunctionReturn,
{
    inner: CallInst<'ctx, Ret::Marker, B>,
    _ret: core::marker::PhantomData<Ret>,
}

impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> Clone for TypedCallInst<'ctx, Ret, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, Ret: FunctionReturn, B: ModuleBrand> Copy for TypedCallInst<'ctx, Ret, B> {}
// + manual Debug / PartialEq / Eq / Hash delegating to `inner`
// (PhantomData pattern — copy the TypedFunctionValue block shapes).

impl<'ctx, Ret: FunctionReturn, B: ModuleBrand + 'ctx> TypedCallInst<'ctx, Ret, B> {
    #[inline]
    pub(super) fn from_call(inner: CallInst<'ctx, Ret::Marker, B>) -> Self {
        Self { inner, _ret: core::marker::PhantomData }
    }

    /// Typed result. Infallible: the schema was validated when the
    /// typed callee facade was constructed. `()` for a void callee.
    #[inline]
    pub fn result(self) -> Ret::CallResult<'ctx, B> {
        let validated = crate::function_signature::token::ValidatedCallResult::new();
        let value = Value::from_parts(self.inner.id, self.inner.module, self.inner.ty);
        Ret::call_result_from_value(value, &validated)
    }

    /// Marker-typed handle (keeps `Ret::Marker`, drops the schema).
    #[inline]
    pub fn as_call_inst(self) -> CallInst<'ctx, Ret::Marker, B> {
        self.inner
    }

    /// Fully-erased handle (D3).
    #[inline]
    pub fn as_dyn(self) -> CallInst<'ctx, Dyn, B> {
        self.inner.as_dyn()
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        self.inner.as_value()
    }
}
```

(Use an import for `ValidatedCallResult` per conventions rather than the qualified path.) Then brand-generalize the accessor macros at instructions.rs:556-591: change `impl<'ctx> CallInst<'ctx, $w>` to `impl<'ctx, B: ModuleBrand + 'ctx> CallInst<'ctx, $w, B>` in both `call_inst_int_return!` and `call_inst_float_return!` (bodies unchanged — pure widening; do the same for the matching `InvokeInst` accessor macros if they share the gap — grep `impl<'ctx> InvokeInst`).

- [ ] **Step 4: Check, re-export, commit**

`cargo check`; re-export `TypedCallInst` (+ `ValidatedCallResult` stays doc-hidden inside `token`). Commit: `"Derive call results from callee schemas: CallResult GAT + TypedCallInst (D4)"` + trailer.

---

### Task 15: Typed builder call surface + hardened dyn fallbacks

**Files:**
- Modify: `crates/llvmkit-ir/src/ir_builder.rs` (renames; typed `build_call` family; `validate_call_site_args`; `TypedCallBuilder`)
- Modify: `crates/llvmkit-ir/src/marker.rs` (`marker_kind_label`)
- Modify: `crates/llvmkit-ir/src/error.rs` (4 new variants)
- Modify: `crates/llvmkit-ir/src/function_signature.rs` + `module.rs` (varargs facade + method split)
- Modify: `crates/llvmkit-asmparser/src/ll_parser.rs` (~7489, ~8262)
- Modify: tests/examples that call the renamed methods (mechanical `_dyn` suffix)

**Interfaces:**
- Consumes: `CallArgs` / `IntoCallArg` (Task 13), `TypedCallInst` (Task 14), `TerminatedBlockInvoke` naming (Task 1).
- Produces: typed `build_call` / `build_call_with_config` / `typed_call_builder` / `build_varargs_call` / `build_indirect_call::<Sig>` / `build_invoke` / `build_invoke_with_config`; renamed `build_call_dyn` / `build_indirect_call_dyn` / `build_invoke_dyn` / `build_invoke_dyn_with_config`; `TypedVarArgsFunctionValue`; errors `CallArgumentCountMismatch` / `CallArgumentTypeMismatch` / `UnexpectedVarArgsSignature` / `MissingVarArgsSignature`; alias `TerminatedBlockTypedInvoke`.

- [ ] **Step 1: LSP-rename the dyn fallbacks**

`build_call`→`build_call_dyn`, `build_indirect_call`→`build_indirect_call_dyn`, `build_invoke`→`build_invoke_dyn`, `build_invoke_with_config`→`build_invoke_dyn_with_config`. This mechanically migrates the parser (~7489, ~8262) and every test/example call site. `cargo check` green.

- [ ] **Step 2: Error variants + `marker_kind_label`**

error.rs (copy the neighboring `#[error]`/Display formatting style):

```rust
CallArgumentCountMismatch { expected: u32, got: u32 },
CallArgumentTypeMismatch { index: u32, expected: TypeKindLabel, got: TypeKindLabel },
UnexpectedVarArgsSignature,
MissingVarArgsSignature,
```

marker.rs:

```rust
/// Diagnostic label for a return marker's expected type kind. `None`
/// for [`Dyn`], which matches every signature. Crate-internal — used
/// to fix the expected/got duplication in ReturnTypeMismatch reports.
pub(crate) fn marker_kind_label<R: ReturnMarker>() -> Option<TypeKindLabel> {
    match R::expected_kind() {
        ExpectedRetKind::Void => Some(TypeKindLabel::Void),
        ExpectedRetKind::Ptr => Some(TypeKindLabel::Pointer),
        ExpectedRetKind::IntStatic(_) | ExpectedRetKind::IntDyn => Some(TypeKindLabel::Integer),
        ExpectedRetKind::FloatStatic(label) => Some(float_label_to_kind(label)),
        ExpectedRetKind::FloatDyn => Some(TypeKindLabel::Float),
        ExpectedRetKind::Dyn => None,
    }
}
```

(Write `float_label_to_kind` as a match from the ieee label strings `"half" | "bfloat" | "float" | "double" | "fp128" | "x86_fp80" | "ppc_fp128"` to the corresponding `TypeKindLabel` variants — check the exact label spellings in float_kind.rs and the exact `ExpectedRetKind` variant shapes in marker.rs before writing; adapt names to reality.) Fix the three `ReturnTypeMismatch { expected: X, got: X }` duplication sites (build_indirect_call_dyn at ~3487, build_inline_asm_call at ~3546, and the invoke config twin — `grep -n "kind_label()," crates/llvmkit-ir/src/ir_builder.rs` around those regions) to:

```rust
return Err(IrError::ReturnTypeMismatch {
    expected: marker_kind_label::<R2>()
        .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
    got: fn_ty.return_type().kind_label(),
});
```

- [ ] **Step 3: `validate_call_site_args` + wire into every dyn path**

Private helper on the Positioned impl (near the call section):

```rust
/// Ports the `CallInst::init` assertions ("Calling a function with a
/// bad signature!", lib/IR/Instructions.cpp): argument count must
/// equal the parameter count (or exceed it for varargs), and each
/// fixed argument's type must equal the parameter type exactly.
fn validate_call_site_args(
    &self,
    fn_ty: FunctionType<'ctx, B>,
    args: &[ValueId],
) -> IrResult<()> {
    let params: Vec<TypeId> = fn_ty.params().collect();
    let expected = u32::try_from(params.len())
        .unwrap_or_else(|_| unreachable!("parameter count bounded by u32"));
    let got = u32::try_from(args.len())
        .unwrap_or_else(|_| unreachable!("argument count bounded by u32"));
    let count_ok = if fn_ty.is_var_arg() { got >= expected } else { got == expected };
    if !count_ok {
        return Err(IrError::CallArgumentCountMismatch { expected, got });
    }
    for (i, (arg, param_ty)) in args.iter().zip(params.iter()).enumerate() {
        let arg_ty = self.module.context().value_data(*arg).ty;
        if arg_ty != *param_ty {
            return Err(IrError::CallArgumentTypeMismatch {
                index: u32::try_from(i)
                    .unwrap_or_else(|_| unreachable!("argument index bounded by u32")),
                expected: Type::new(*param_ty, /* module ref as the file spells it */).kind_label(),
                got: Type::new(arg_ty, /* likewise */).kind_label(),
            });
        }
    }
    Ok(())
}
```

(Adapt `fn_ty.params()` iterator shape and the `Type::new(...)`-from-TypeId idiom to what derived_types.rs actually exposes — `lsp hover`; the value-type read `self.module.context().value_data(id).ty` is the idiom from build_indirect_call at :3516.) Wire it in: `CallBuilder::build()` (before payload construction — this covers `build_call_dyn` and the intrinsic builders, which flow through it), `build_indirect_call_dyn`, `build_inline_asm_call`, `build_invoke_dyn` / `build_invoke_dyn_with_config`, `build_inline_asm_invoke(_with_config)`, `build_callbr(_with_config)` and inline-asm callbr twins. Each site already holds the arg-id list and the `FunctionType` — insert the call just before the payload is built. Tests that deliberately built mismatched calls to exercise verifier rules will now fail at build time: re-point them at the new `IrError::CallArgument*` (build-time) OR construct via parsed `.ll` where the verifier-level lock must survive.

- [ ] **Step 4: Typed call methods**

```rust
/// TYPED flat call — the primary form. Wrong arity / wrong argument
/// types / wrong result use are compile errors; the return marker is
/// derived from the callee. Mirrors `IRBuilder::CreateCall(FunctionCallee, ...)`.
pub fn build_call<Ret, Params, A, Name>(
    &self,
    callee: TypedFunctionValue<'ctx, Ret, Params, B>,
    args: A,
    name: Name,
) -> IrResult<TypedCallInst<'ctx, Ret, B>>
where
    Ret: FunctionReturn,
    Params: FunctionParamList,
    A: CallArgs<'ctx, Params, B>,
    Name: AsRef<str>,
{
    let f = callee.as_function();
    let arg_ids = args.lower(ModuleRef::new(self.module))?;
    let payload = CallInstData::new(
        f.as_value().id,
        f.signature().as_type().id(),
        arg_ids,
        f.calling_conv(),
        TailCallKind::None,
    );
    let inst = self.append_instruction(
        f.return_type().id(),
        InstructionKindData::Call(payload),
        name,
    );
    Ok(TypedCallInst::from_call(CallInst::from_raw(
        inst.as_value().id,
        ModuleRef::new(self.module),
        inst.ty().id(),
    )))
}
```

(Payload/append idiom copied from `build_indirect_call` at :3497-3513 — reuse its exact import spellings.) Add `build_call_with_config(callee, args, config: CallSiteConfig)` (same body, threading cc/attrs/name from the config the way `CallBuilder::build` does — read it) and the typed chainable twin:

```rust
pub struct TypedCallBuilder<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
where /* bounds as build_call + RP: ReturnMarker (parent), F: IRBuilderFolder<'ctx, B> */
{
    parent: &'a IRBuilder<'m, 'ctx, B, F, Positioned, RP>,
    callee: TypedFunctionValue<'ctx, Ret, Params, B>,
    args: A,
    tail_kind: TailCallKind,
    calling_conv: Option<CallingConv>,
    attrs: CallAttributeData,
    name: String,
}
// Chainables: tail()/must_tail()/no_tail()/calling_conv(cc)/call_attributes(attrs)/name(n);
// build() lowers `args` then emits exactly as build_call, applying the overrides.
pub fn typed_call_builder<Ret, Params, A>(&self, callee: ..., args: A) -> TypedCallBuilder<...>;
```

Typed indirect call (function type constructed from the schema, never spelled by hand):

```rust
/// Typed indirect call through a function-pointer value.
/// Spell as: b.build_indirect_call::<fn(i32) -> i32, _, _>(fp, (x,), "r")?
pub fn build_indirect_call<Sig, A, Name>(
    &self,
    callee: PointerValue<'ctx, B>,
    args: A,
    name: Name,
) -> IrResult<TypedCallInst<'ctx, Sig::Ret, B>>
where
    Sig: FunctionSignature,
    A: CallArgs<'ctx, Sig::Params, B>,
    Name: AsRef<str>,
{
    let ret = <Sig::Ret as FunctionReturn>::ir_type(self.module)?;
    let params = <Sig::Params as FunctionParamList>::ir_types(self.module)?;
    let fn_ty = self.module.function_type(ret, &params, false)?;
    let arg_ids = args.lower(ModuleRef::new(self.module))?;
    /* payload + append as build_indirect_call_dyn, then
       TypedCallInst::from_call(CallInst::from_raw(...)) */
}
```

(Check `Module::function_type`'s real signature — the parser constructs function types somewhere; mirror it.) Typed invoke: clone `build_invoke_dyn`'s body shape but take `TypedFunctionValue` + `A: CallArgs`, return `type TerminatedBlockTypedInvoke<'ctx, R, Ret, B = Brand<'ctx>> = (BasicBlock<'ctx, R, Terminated, B>, InvokeInst<'ctx, <Ret as FunctionReturn>::Marker, B>);` — with `build_invoke_with_config` twin.

- [ ] **Step 5: Varargs facades**

function_signature.rs — `TypedFunctionValue::try_from_function` gains the fixed-arity gate as its FIRST check:

```rust
if function.signature().is_var_arg() {
    return Err(IrError::UnexpectedVarArgsSignature);
}
```

New `TypedVarArgsFunctionValue<'ctx, Ret, Params, B>` — copy the `TypedFunctionValue` struct + manual-derive block wholesale (same fields), with `try_from_function` requiring `is_var_arg()` (else `MissingVarArgsSignature`) and arity meaning "fixed-prefix arity" (`arg_count() != Params::ARITY` check unchanged — confirm how the crate stores varargs params: if `arg_count` includes only fixed params, keep; read `FunctionValue::arg_count`). Same `params()` / `append_basic_block` / `builder` / `as_function` surface. module.rs — split the bool (find `typed_function_type` via `lsp references`): `typed_function_type::<Ret, Params>()` (fixed) + `typed_varargs_function_type::<Ret, Params>()`, same for `_of` twins, plus `add_typed_varargs_function` / `add_typed_varargs_function_of` mirroring the existing `add_typed_function(_of)` bodies with the varargs facade + `true` flag. Builder method:

```rust
pub fn build_varargs_call<Ret, Params, A, I, V, Name>(
    &self,
    callee: TypedVarArgsFunctionValue<'ctx, Ret, Params, B>,
    fixed_args: A,
    varargs: I,
    name: Name,
) -> IrResult<TypedCallInst<'ctx, Ret, B>>
where
    Ret: FunctionReturn, Params: FunctionParamList,
    A: CallArgs<'ctx, Params, B>,
    I: IntoIterator<Item = V>, V: IsValue<'ctx, B>, Name: AsRef<str>,
{
    // lower fixed prefix, extend with erased varargs ids, emit as build_call.
}
```

- [ ] **Step 6: Gate + commit**

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt
```
Commit: `"Ship end-to-end typed calls; harden dyn call paths (D1-D4, D7)"` + body listing the renames + the `ReturnTypeMismatch` expected/got bug fix + trailer.

---

### Task 16: Typed-call test suite

**Files:**
- Create: `crates/llvmkit-ir/tests/builder_typed_call.rs`
- Create: `crates/llvmkit-ir/tests/compile_fail/typed_call_wrong_arity.rs`, `typed_call_wrong_arg_type.rs`, `typed_call_void_result_use.rs`, `typed_call_cross_module_arg.rs` (+`.stderr` each)
- Modify: `UPSTREAM.md`, `README.md` (typed-call section), `INKWELL_MIGRATION.md` (call row)

**Interfaces:** consumes everything from Tasks 13-15.

- [ ] **Step 1: Positive tests** (`builder_typed_call.rs`)

Anchor: `orig_cpp/.../llvm/unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)` — read it and port its operand-wiring assertions through the typed path. Cases: (1) typed direct call `build_call(callee, (x, 1_i32), "r")` printing the exact `%r = call i32 @callee(i32 %0, i32 1)` text the existing builder_call fixture locks; (2) `r.result()` feeding `build_int_add` and `build_ret` with no `try_into`; (3) void callee — `result()` is `()`; ptr + f64 variants; (4) typed invoke (anchor `InvokeInst` TEST_F); (5) varargs `printf`-shape (anchor: varargs call in `test/Bitcode/compatibility.ll` — cite the line from the local tree); (6) typed indirect call printing identical text to the dyn form; (7) dyn-path build-time rejection: `build_call_dyn` with 1-of-2 args → `CallArgumentCountMismatch`; wrong-typed arg → `CallArgumentTypeMismatch` (example-locks citing `CallInst::init`); (8) `build_indirect_call_dyn::<i32>` against a void fn type → `ReturnTypeMismatch` with `expected: Integer, got: Void` (locks the bug fix).

- [ ] **Step 2: Compile-fail fixtures**

`typed_call_wrong_arity.rs`: 2-param callee, `(x,)` tuple → `CallArgs` unimplemented (golden locks the `#[diagnostic::on_unimplemented]` message). `typed_call_wrong_arg_type.rs`: f64 value into an i32 slot → `IntoCallArg` bound failure. `typed_call_void_result_use.rs`: `let n: i32 = ...; b.build_int_add::<i32,_,_,_>(void_call.result(), n, "x")` → `()` is not `IntoIntValue`. `typed_call_cross_module_arg.rs`: value from module A into module B's typed call → brand mismatch (mirror the existing cross-module compile-fail fixture's shape). `TRYBUILD=overwrite` regen.

- [ ] **Step 3: Register + docs + commit**

UPSTREAM.md rows for every test; README gains the flagship snippet (the spec §Workstream-1 usage block); INKWELL_MIGRATION.md maps `build_call` → typed/dyn split. Commit: `"Lock typed-call surface with ported + compile-fail tests (D11)"` + trailer.

---

**WORKSTREAM GATE (typed calls complete):** full gate green.

---

### Task 17: Auto-SSA core — `ssa_builder.rs` variables, blocks, state, Braun engine

**Files:**
- Create: `crates/llvmkit-ir/src/ssa_builder.rs`
- Modify: `crates/llvmkit-ir/src/module.rs` (crate-internal `next_ssa_builder_id()` counter)
- Modify: `crates/llvmkit-ir/src/error.rs` (8 new variants)
- Modify: `crates/llvmkit-ir/src/lib.rs` (module + re-exports)
- Test: `crates/llvmkit-ir/tests/ssa_builder.rs` (started here, completed in Task 19)

**Interfaces:**
- Consumes: `BuilderPositionState` (Task 1), `IRBuilder` positioning + `build_int_phi_dyn` / `build_fp_phi_dyn` / `build_pointer_phi_in_addrspace`, `Instruction<'ctx, Attached, B>::{replace_all_uses_with, erase_from_parent}` (instruction.rs:~802/~867), `Instruction::from_parts` (instruction.rs:~770, crate-root visible), `Value::users()` (value.rs:~386), `PhiInst::add_incoming` dyn-shaped path (`phi_add_incoming_from_value`, ir_builder.rs:~460).
- Produces (Task 18/19 depend on these exact names): `SsaBuilder<'m, 'ctx, B, F, S, R>`, `SsaBlock<'ctx, R, B>`, `IntVariable<'ctx, W, B>` / `FloatVariable<'ctx, K, B>` / `PointerVariable<'ctx, B>`, `SsaBuilderId`; errors `SsaUseOfUndefinedVariable { variable: String, block: String }`, `SsaBranchToSealedBlock { block: String }`, `SsaBlockAlreadySealed { block: String }`, `SsaBlockAlreadyFilled { block: String }`, `SsaUnfilledBlock { block: String }`, `SsaForeignVariable`, `SsaForeignBlock`, `SsaFunctionHasBlocks`.

- [ ] **Step 1: Module scaffolding, ids, variables, block handle**

File header cites Braun et al. 2013 ("Simple and Efficient Construction of Static Single Assignment Form"), cranelift-frontend `FunctionBuilder`, and `llvm/lib/Transforms/Utils/SSAUpdater.cpp` as the nearest LLVM relative. Core data (write in full; `HashMap`/`HashSet` from std):

```rust
/// Per-module monotonic id for an [`SsaBuilder`]; foreign-variable /
/// foreign-block use is a typed runtime error (a generative per-builder
/// brand was rejected: it would force nested closures per function body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SsaBuilderId(u32);

/// Typed SSA variable of integer width `W`. Cranelift analogue:
/// `cranelift_frontend::Variable`, specialised per category per llvmkit
/// convention (cf. PhiInst / FpPhiInst / PointerPhiInst).
pub struct IntVariable<'ctx, W: IntWidth, B: ModuleBrand = Brand<'ctx>> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeId,
    module: ModuleRef<'ctx, B>,
    _w: PhantomData<fn() -> W>,
}
// FloatVariable<'ctx, K: FloatKind, B> / PointerVariable<'ctx, B>: same
// four fields with their own phantom (pointer variant has no phantom).
// All three: manual Copy/Clone/Debug/PartialEq/Eq/Hash (PhantomData pattern).

/// Copyable reference to a block managed by an SsaBuilder. NOT an
/// insertion capability — the linear BasicBlock handles stay inside the
/// SsaBuilder; this implements IntoBasicBlockLabel as the escape hatch.
pub struct SsaBlock<'ctx, R: ReturnMarker, B: ModuleBrand = Brand<'ctx>> {
    label: BasicBlockLabel<'ctx, R, B>,
    owner: SsaBuilderId,
}
// Copy/Clone/Debug/PartialEq/Eq/Hash manual impls; `pub fn label(&self)`;
// impl IntoBasicBlockLabel<'ctx, R, B> for SsaBlock<'ctx, R, B> delegating
// to `label` (copy the impl shape from an existing IntoBasicBlockLabel impl
// in basic_block.rs).
```

module.rs gets (next to the other ModuleCore counters — find them via the existing id-allocation methods):

```rust
pub(crate) fn next_ssa_builder_id(&self) -> u32 { /* fetch-and-increment a Cell<u32>, as the other counters do */ }
```

- [ ] **Step 2: `SsaState` + `SsaBuilder` struct + constructors**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarCategory { Int, Float, Pointer }

struct VarData {
    ty: TypeId,
    category: VarCategory,
    name: String,
    poison_on_undef: bool,
}

struct SsaState<'ctx, R: ReturnMarker, B: ModuleBrand> {
    vars: Vec<VarData>,
    /// Braun currentDef: (block, var) -> definition value.
    current_def: HashMap<(ValueId, u32), ValueId>,
    /// Trivial-phi forwarding (path-compressed on read).
    resolved: RefCell<HashMap<ValueId, ValueId>>,
    /// Recorded CFG edges, duplicates preserved (phi operand order).
    preds: HashMap<ValueId, Vec<ValueId>>,
    sealed: HashSet<ValueId>,
    filled: HashSet<ValueId>,
    /// Braun incompletePhis: block -> [(var index, phi value)].
    incomplete_phis: HashMap<ValueId, Vec<(u32, ValueId)>>,
    /// Linear insertion capabilities for not-yet-current blocks.
    open_blocks: HashMap<ValueId, BasicBlock<'ctx, R, Unterminated, B>>,
    /// Linear lifecycle handles for layer-created phis (RAUW / erase).
    created_phis: HashMap<ValueId, Instruction<'ctx, Attached, B>>,
    /// Deterministic iteration for finish().
    block_order: Vec<ValueId>,
}

pub struct SsaBuilder<'m, 'ctx, B, F = ConstantFolder, S = Unpositioned, R = Dyn>
where
    B: ModuleBrand,
    F: IRBuilderFolder<'ctx, B> + Clone,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    module: &'m Module<'ctx, B, Unverified>,
    function: FunctionValue<'ctx, R, B>,
    id: SsaBuilderId,
    folder: F,
    /// Some iff S = Positioned (mirrors the `insert_block()` Option
    /// precedent at ir_builder.rs:~539).
    inner: Option<IRBuilder<'m, 'ctx, B, F, Positioned, R>>,
    state: SsaState<'ctx, R, B>,
    _s: PhantomData<S>,
}

impl<'m, 'ctx, B: ModuleBrand + 'ctx> SsaBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, Dyn> {
    /// Errors with SsaFunctionHasBlocks if `function` already has a
    /// body — the layer must observe every CFG edge from birth.
    pub fn for_function<R: ReturnMarker>(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionValue<'ctx, R, B>,
    ) -> IrResult<SsaBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, R>> {
        Self::with_folder_for_function(module, function, ConstantFolder)
    }

    pub fn with_folder_for_function<F, R>(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionValue<'ctx, R, B>,
        folder: F,
    ) -> IrResult<SsaBuilder<'m, 'ctx, B, F, Unpositioned, R>>
    where
        F: IRBuilderFolder<'ctx, B> + Clone,
        R: ReturnMarker,
    {
        if function.entry_block().is_some() {
            return Err(IrError::SsaFunctionHasBlocks);
        }
        Ok(SsaBuilder {
            module,
            function,
            id: SsaBuilderId(module.next_ssa_builder_id()),
            folder,
            inner: None,
            state: SsaState::new(),
            _s: PhantomData,
        })
    }
}
```

(`function.entry_block()` — the accessor exists per README's pass example; confirm spelling.)

- [ ] **Step 3: Any-state surface — `create_block`, variable declarations, `seal_block`**

```rust
impl<'m, 'ctx, B, F, S, R> SsaBuilder<'m, 'ctx, B, F, S, R>
where /* standard bounds */
{
    /// Append a block. The FIRST created block is the entry block and
    /// is auto-Braun-sealed: entry has no predecessors by definition
    /// (Verifier::visitFunction), so a later branch TO it errors.
    pub fn create_block<Name: Into<String>>(&mut self, name: Name) -> SsaBlock<'ctx, R, B> {
        let block = self.function.append_basic_block(self.module, name);
        let label = block.label(); // confirm accessor: the copyable label of a linear block handle
        let block_id = label_value_id(&label); // helper: the block's ValueId — find the real accessor
        if self.state.block_order.is_empty() {
            self.state.sealed.insert(block_id);
        }
        self.state.block_order.push(block_id);
        self.state.preds.entry(block_id).or_default();
        self.state.open_blocks.insert(block_id, block);
        SsaBlock { label, owner: self.id }
    }

    /// Declare a strict int variable: reading it on a def-less path is
    /// a typed error (D10).
    pub fn declare_int_var<W: StaticIntWidth, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> IntVariable<'ctx, W, B> {
        self.declare_var_raw::<W>(name, VarCategory::Int, false)
    }

    /// Poison twin: reading on a def-less path yields `poison`
    /// (explicit opt-in, separate method per the no-bool-params rule).
    pub fn declare_int_var_poison<W: StaticIntWidth, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> IntVariable<'ctx, W, B> {
        self.declare_var_raw::<W>(name, VarCategory::Int, true)
    }
    // declare_int_var_dyn(_poison)(ty: IntType<'ctx, IntDyn, B>, name),
    // declare_float_var(_poison)<K: StaticFloatKind>, declare_float_var_dyn(_poison)(ty),
    // declare_pointer_var(_poison)() [addrspace 0],
    // declare_pointer_var(_poison)_in_addrspace(ty: PointerType<'ctx, B>):
    // all funnel into a private declare-slot helper that pushes VarData
    // { ty, category, name, poison_on_undef } and returns the handle with
    // index = vars.len()-1, owner = self.id, module = ModuleRef::new(self.module).
    // The static forms get `ty` from W::ir_type / K::ir_type-style
    // constructors (StaticIntWidth::ir_type exists per AGENTS.md).

    /// Braun sealBlock: the predecessor set is complete; complete this
    /// block's incomplete phis.
    pub fn seal_block(&mut self, block: SsaBlock<'ctx, R, B>) -> IrResult<()> {
        self.check_owner_block(&block)?;
        let block_id = label_value_id(&block.label);
        if self.state.sealed.contains(&block_id) {
            return Err(IrError::SsaBlockAlreadySealed { block: block_name(block_id) });
        }
        let pending = self.state.incomplete_phis.remove(&block_id).unwrap_or_default();
        self.state.sealed.insert(block_id);
        for (var, phi_id) in pending {
            self.add_phi_operands(var, phi_id, block_id)?;
        }
        Ok(())
    }
}
```

`label_value_id` / `block_name` are small private helpers over whatever `BasicBlockLabel` actually exposes (`lsp hover` — the CFG module resolves labels to blocks somewhere; reuse that path). `check_owner_block` compares `block.owner != self.id` → `SsaForeignBlock`.

- [ ] **Step 4: The Braun engine (private)**

Faithful port of the paper's four procedures. `read_variable_in` chases single-pred chains iteratively; phi-operand filling recurses (bounded: each phi is created and filled once, so depth ≤ block count):

```rust
impl /* same bounds */ SsaBuilder<...> {
    fn write_variable(&mut self, var: u32, block: ValueId, value: ValueId) {
        self.state.current_def.insert((block, var), value);
    }

    /// Braun readVariable + readVariableRecursive.
    fn read_variable_in(&mut self, var: u32, mut block: ValueId) -> IrResult<ValueId> {
        loop {
            if let Some(v) = self.state.current_def.get(&(block, var)) {
                return Ok(self.resolve(*v));
            }
            if !self.state.sealed.contains(&block) {
                // Incomplete CFG: operandless phi at the head, complete at seal.
                let phi = self.emit_operandless_phi(var, block)?;
                self.state.incomplete_phis.entry(block).or_default().push((var, phi));
                self.write_variable(var, block, phi);
                return Ok(phi);
            }
            let preds = self.state.preds.get(&block).cloned().unwrap_or_default();
            match preds.len() {
                0 => return self.undefined_read(var, block), // entry reached: strict error or poison
                1 => block = preds[0], // single-pred chase: NO phi
                _ => {
                    let phi = self.emit_operandless_phi(var, block)?;
                    self.write_variable(var, block, phi); // break cycles
                    return self.add_phi_operands(var, phi, block);
                }
            }
        }
    }

    /// Braun addPhiOperands + tryRemoveTrivialPhi.
    fn add_phi_operands(&mut self, var: u32, phi: ValueId, block: ValueId) -> IrResult<ValueId> {
        let preds = self.state.preds.get(&block).cloned().unwrap_or_default();
        for pred in preds {
            let operand = self.read_variable_in(var, pred)?;
            self.phi_add_incoming_raw(phi, operand, pred)?; // dyn phi add_incoming via ir_builder's phi_add_incoming_from_value idiom
        }
        self.try_remove_trivial_phi(phi)
    }

    fn try_remove_trivial_phi(&mut self, phi: ValueId) -> IrResult<ValueId> {
        let mut same: Option<ValueId> = None;
        for op in self.phi_incoming_values(phi) {   // read the incoming list via the phi payload accessors
            let op = self.resolve(op);
            if op == phi || Some(op) == same { continue; }
            if same.is_some() { return Ok(phi); }    // merges >= 2 distinct values: not trivial
            same = Some(op);
        }
        let same = match same {
            Some(v) => v,
            None => return self.undefined_phi_replacement(phi), // unreachable-or-self phi: strict error / poison per the var
        };
        // Snapshot users BEFORE mutation; only recurse into layer-created phis.
        let users: Vec<ValueId> = self.phi_user_ids(phi);
        let handle = self.state.created_phis.remove(&phi)
            .unwrap_or_else(|| unreachable!("SsaBuilder invariant: trivial-phi candidates are layer-created"));
        let replacement = /* Value from `same` id via Value::from_parts idiom */;
        handle.replace_all_uses_with(self.module, replacement)?; // rewrites phi incoming Cells (instruction.rs:1134)
        // NOTE: replace_all_uses_with consumes the Attached handle. Check its real
        // signature: if it does not also erase, re-materialize a handle via
        // Instruction::from_parts(...) and erase_from_parent(self.module)?.
        self.state.resolved.borrow_mut().insert(phi, same);
        for user in users {
            if self.state.created_phis.contains_key(&user) {
                self.try_remove_trivial_phi(user)?;
            }
        }
        Ok(self.resolve(same))
    }

    /// Path-compressed forwarding lookup.
    fn resolve(&self, mut v: ValueId) -> ValueId {
        let map = self.state.resolved.borrow();
        while let Some(next) = map.get(&v) { v = *next; }
        v
    }
}
```

`emit_operandless_phi` — the three insertion cases:
1. Block is the CURRENT block and empty → emit through `self.inner` (append == head-insert in an empty block).
2. Block is open (not current, unfilled): take its linear handle from `open_blocks`, `IRBuilder::with_folder(self.module, self.folder.clone()).position_at_end(handle)`, if the block already has instructions reposition with `position_before(&first_instruction_view)`, emit, then `into_insert_block()` the handle back into the map.
3. Block is filled (Terminated — handle consumed by its terminator): `position_before(&first_instruction_view)` — a filled block always has ≥1 instruction. First-instruction views come from the block-iteration API (`crates/llvmkit-ir/src/iter.rs` — `BlockCursor`/`InstructionView` per AGENTS.md; find the "first instruction of block" accessor there).
Emit with the category-matching dyn phi builder (`build_int_phi_dyn(ty, name)` / `build_fp_phi_dyn` / `build_pointer_phi_in_addrspace`), name = the variable's declared name, and store `Instruction::from_parts(phi_value_id, ...)` into `created_phis`. `undefined_read`: `poison_on_undef` vars return `ty.get_poison()`-equivalent (constants.rs:~1380 — find `get_poison`/`poison_value` spelling); strict vars return `Err(IrError::SsaUseOfUndefinedVariable { variable, block })`.

- [ ] **Step 5: Error variants + compile check**

Add the 8 `IrError` variants (error.rs, matching neighboring style). `cargo check` — the module compiles standalone (Positioned-only API lands in Task 18; keep `inner` unused-warning-free by having `emit_operandless_phi` use it). Commit: `"Add SsaBuilder core: typed variables + Braun on-the-fly SSA engine (D1, D9)"` + trailer citing Braun et al. 2013.

---

### Task 18: Auto-SSA positioned/unpositioned API — def/use, terminators, `finish`

**Files:**
- Modify: `crates/llvmkit-ir/src/ssa_builder.rs`

**Interfaces:**
- Produces the full public lifecycle: `switch_to_block`, `ins()`, `current_block()`, `def_int_var` / `def_float_var` / `def_pointer_var`, `use_int_var` / `use_float_var` / `use_pointer_var`, `br` / `cond_br` / `switch` / `ret` / `ret_void` / `unreachable`, `finish()`.

- [ ] **Step 1: Unpositioned-only surface**

```rust
impl<'m, 'ctx, B, F, R> SsaBuilder<'m, 'ctx, B, F, Unpositioned, R>
where /* bounds */
{
    /// Position at the end of `block`. "Terminate the current block
    /// before switching" is a COMPILE error — this method does not
    /// exist on the Positioned state.
    pub fn switch_to_block(
        mut self,
        block: SsaBlock<'ctx, R, B>,
    ) -> IrResult<SsaBuilder<'m, 'ctx, B, F, Positioned, R>> {
        self.check_owner_block(&block)?;
        let block_id = label_value_id(&block.label);
        if self.state.filled.contains(&block_id) {
            return Err(IrError::SsaBlockAlreadyFilled { block: block_name(block_id) });
        }
        let handle = self.state.open_blocks.remove(&block_id)
            .unwrap_or_else(|| unreachable!("SsaBuilder invariant: unfilled block retains its linear handle"));
        let inner = IRBuilder::with_folder(self.module, self.folder.clone()).position_at_end(handle);
        Ok(SsaBuilder { module: self.module, function: self.function, id: self.id,
            folder: self.folder, inner: Some(inner), state: self.state, _s: PhantomData })
    }

    /// Seal every remaining unsealed block, then require every created
    /// block to be filled. Consuming self on Unpositioned gives two
    /// static guarantees: no def/use after finish, and no block is
    /// mid-construction.
    pub fn finish(mut self) -> IrResult<()> {
        for block_id in self.state.block_order.clone() {
            if !self.state.sealed.contains(&block_id) {
                let pending = self.state.incomplete_phis.remove(&block_id).unwrap_or_default();
                self.state.sealed.insert(block_id);
                for (var, phi) in pending {
                    self.add_phi_operands(var, phi, block_id)?;
                }
            }
        }
        for block_id in &self.state.block_order {
            if !self.state.filled.contains(block_id) {
                return Err(IrError::SsaUnfilledBlock { block: block_name(*block_id) });
            }
        }
        Ok(())
    }
}
```

(`IRBuilder::with_folder` constructor — confirm the real name/arg order from ir_builder.rs's constructor set: `new`, `new_for`, `new_for_return`, `with_folder`.)

- [ ] **Step 2: Positioned-only surface**

```rust
impl<'m, 'ctx, B, F, R> SsaBuilder<'m, 'ctx, B, F, Positioned, R>
where /* bounds */
{
    /// The full existing typed instruction surface, cranelift-style:
    /// `c.ins().build_int_mul(a, b, "x")?`. The &-return makes the
    /// IRBuilder's self-consuming methods (terminators, repositioning)
    /// structurally unreachable — the SsaBuilder never surrenders the
    /// inner builder, which keeps its CFG bookkeeping complete.
    pub fn ins(&self) -> &IRBuilder<'m, 'ctx, B, F, Positioned, R> {
        self.inner.as_ref()
            .unwrap_or_else(|| unreachable!("SsaBuilder invariant: Positioned state holds the inner builder"))
    }

    pub fn current_block(&self) -> SsaBlock<'ctx, R, B> { /* label from ins().insert_block() */ }

    /// Braun writeVariable — pure bookkeeping, no IR emitted.
    pub fn def_int_var<W: IntWidth, V>(
        &mut self,
        var: IntVariable<'ctx, W, B>,
        value: V,
    ) -> IrResult<()>
    where
        V: IntoIntValue<'ctx, W, B>,
    {
        self.check_owner_var(var.owner)?;
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        // _dyn-declared vars pin only "integer"; check the width against var.ty
        // when W is IntDyn (mirror accept_folded_int's dyn seam).
        let block = self.current_block_id();
        self.write_variable(var.index, block, v.as_value().id);
        Ok(())
    }

    /// Braun readVariable; the result type reflects the variable (D4).
    pub fn use_int_var<W: IntWidth>(
        &mut self,
        var: IntVariable<'ctx, W, B>,
    ) -> IrResult<IntValue<'ctx, W, B>> {
        self.check_owner_var(var.owner)?;
        let block = self.current_block_id();
        let id = self.read_variable_in(var.index, block)?;
        let value = /* Value::from_parts(id, ModuleRef::new(self.module), <ty of id>) idiom */;
        Ok(IntValue::from_value_unchecked(value)) // sound: var.ty pinned at declaration
    }
    // def_float_var / use_float_var / def_pointer_var / use_pointer_var mirror these.
}
```

Terminators — each consumes self, delegates to the inner builder's terminator (which consumes IT), records edges, marks filled, and returns the Unpositioned builder. The `br` case in full (the others follow it):

```rust
    pub fn br(
        mut self,
        dest: SsaBlock<'ctx, R, B>,
    ) -> IrResult<SsaBuilder<'m, 'ctx, B, F, Unpositioned, R>> {
        self.check_owner_block(&dest)?;
        let dest_id = label_value_id(&dest.label);
        // ANY edge into a Braun-sealed block is an error; entry is
        // auto-sealed, so this also enforces "entry has no
        // predecessors" (Verifier::visitFunction) at construction time.
        if self.state.sealed.contains(&dest_id) {
            return Err(IrError::SsaBranchToSealedBlock { block: block_name(dest_id) });
        }
        let src_id = self.current_block_id();
        let inner = self.inner.take()
            .unwrap_or_else(|| unreachable!("SsaBuilder invariant: Positioned state holds the inner builder"));
        let (_terminated, _inst) = inner.build_br(dest.label)?;
        self.state.preds.entry(dest_id).or_default().push(src_id);
        self.state.filled.insert(src_id);
        Ok(SsaBuilder { module: self.module, function: self.function, id: self.id,
            folder: self.folder, inner: None, state: self.state, _s: PhantomData })
    }
```

`cond_br(cond: C: IntoIntValue<'ctx, bool, B>, then_dest, else_dest)` records two edges (then-edge first — phi incoming order follows edge-recording order); `switch<W, C, V, Cases>(cond, default_dest, cases: IntoIterator<Item = (C, SsaBlock)>)` collects cases up front (closed form — every edge observed) and lowers through the builder's switch (Open/Closed) API, sealing-check + edge-record per unique destination occurrence; `ret<V: IntoReturnValue<'ctx, R, B>>` / `unreachable()` record no edges; `ret_void` is gated on `R = ()`:

```rust
impl<'m, 'ctx, B, F> SsaBuilder<'m, 'ctx, B, F, Positioned, ()> where /* bounds */ {
    pub fn ret_void(mut self) -> IrResult<SsaBuilder<'m, 'ctx, B, F, Unpositioned, ()>> { /* as br, via build_ret_void */ }
}
```

Match the inner terminators' real return shapes (`build_br` returns `(BasicBlock<R, Terminated, B>, Instruction)` per AGENTS.md T2; the switch path returns an Open handle — call `.finish()` on it after adding all cases, per the Open/Closed typestate).

- [ ] **Step 3: First runtime tests**

Start `tests/ssa_builder.rs` (anchors per the spec's D11 table — cite `SSAUpdater::GetValueInMiddleOfBlock` for single-pred, Braun §2/§3 as documented-gap example-locks, and the crate's own verifier as oracle):
- `single_pred_read_emits_no_phi`: entry defines x, second block reads it → printed IR has NO phi, the read resolves to the def.
- `diamond_merge_places_single_phi_at_join`: def x differently in both arms → exactly one `phi` in the join block with both incomings.
- `loop_backedge_completes_incomplete_phi_on_seal`: factorial-shaped loop; before `seal_block` the phi is operandless; after, it has both incomings.
- `strict_use_before_def_is_typed_error` / `poison_variable_reads_poison_on_undef_path` (locks the `poison` token in printed IR).
- `branch_to_sealed_block_rejected`, `double_seal_rejected`, `foreign_variable_rejected`, `finish_reports_unfilled_block`, `for_function_rejects_nonempty_function`.
- `every_auto_ssa_module_verifies`: every CFG shape above must pass `verify_borrowed()`.

Run: `cargo test -p llvmkit-ir --test ssa_builder`. Commit: `"Add SsaBuilder def/use + terminators + finish (D1, D2, D4, D10)"` + trailer.

---

### Task 19: Auto-SSA flagship example + byte-parity lock + property tests

**Files:**
- Create: `crates/llvmkit-ir/examples/factorial_auto_ssa.rs`
- Create: `crates/llvmkit-ir/tests/factorial_auto_ssa_example.rs`
- Modify: `crates/llvmkit-ir/tests/ssa_builder.rs` (proptest)
- Create: `crates/llvmkit-ir/tests/compile_fail/ssa_def_unpositioned.rs`, `ssa_use_after_terminator.rs`, `ssa_def_wrong_width.rs`, `ssa_ret_value_in_void_fn.rs`, `ssa_finish_positioned.rs` (+`.stderr` each)
- Modify: `UPSTREAM.md`

- [ ] **Step 1: Port factorial to the SSA layer**

Read `examples/factorial.rs` and `tests/factorial_example.rs` first — the manual example's EXPECTED PRINTED IR STRING is the single source of truth. The example (shape; adapt names/blocks to make the output byte-identical to the manual one — same block names, same value names, same order):

```rust
let mut c = SsaBuilder::for_function(&m, f.as_function())?;
let acc = c.declare_int_var::<i32, _>("acc");
let i = c.declare_int_var::<i32, _>("i");
let entry = c.create_block("entry"); // auto-sealed
/* base/loop/exit blocks; entry: def acc=1, def i=n, cond_br;
   loop: use acc/i, mul, sub, def both, cond_br backedge, seal loop AFTER the backedge;
   exit: use acc -> single-pred resolution, ret. c.finish()? */
```

The exit-block read MUST resolve without a phi (`ret i32 %next_acc`) and the loop phis must print with the same names/incoming order as the manual example (`%acc = phi i32 [ 1, %entry ], [ %next_acc, %loop ]`). If operand order differs, adjust edge-recording order (preds are recorded in terminator order — `cond_br(then, else)` records then-edge first; pick argument order to match).

- [ ] **Step 2: Byte-parity lock**

`tests/factorial_auto_ssa_example.rs`: build via the example's code path and `assert_eq!(format!("{m}"), EXPECTED)` where `EXPECTED` is the exact string from `tests/factorial_example.rs` (duplicate with a cross-reference comment, or extract to a shared include — prefer whichever the manual test's structure makes cleaner). This is the flagship D11 example-lock: auto-SSA and manual construction print IDENTICAL `.ll`.

- [ ] **Step 3: Property test**

In `tests/ssa_builder.rs` (proptest is already a workspace dev-dependency — verify in Cargo.toml, add if missing): generate random reducible CFGs (bounded: ≤8 blocks, ≤4 vars, random def/use/seal schedules with seal-after-all-preds discipline; always `finish()`), assert `verify_borrowed()` is `Ok` for every generated module, and that strict-undef schedules yield `SsaUseOfUndefinedVariable`, never invalid IR.

- [ ] **Step 4: Typestate compile-fail locks**

`ssa_def_unpositioned.rs` (def before switch_to_block — method missing on Unpositioned), `ssa_use_after_terminator.rs` (use after `br` consumed the builder — moved value), `ssa_def_wrong_width.rs` (i64 value into `IntVariable<i32>` — IntoIntValue bound), `ssa_ret_value_in_void_fn.rs` (`ret(v)` on `R = ()` — IntoReturnValue bound), `ssa_finish_positioned.rs` (finish on Positioned — method missing). `TRYBUILD=overwrite` regen.

- [ ] **Step 5: Register + workstream gate + commit**

UPSTREAM.md rows for every test (all `llvmkit-specific` / example-locks with the citations from Task 18 Step 3). Full gate:

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt -- --check
cargo run -p llvmkit-ir --example factorial_auto_ssa
```
Commit: `"Add auto-SSA factorial example locked byte-identical to manual phis (D11)"` + trailer.

---

### Task 20: Docs — future-work.md, README, AGENTS.md status, migration notes

**Files:**
- Create: `docs/future-work.md`
- Modify: `README.md`, `AGENTS.md`, `INKWELL_MIGRATION.md`, `UPSTREAM.md` (final audit)

- [ ] **Step 1: `docs/future-work.md`**

Write it from the approved plan's "Future-work document" section (`C:\Users\olegg\.claude\plans\i-want-you-to-enumerated-storm.md`) — it is fully drafted there: killer-feature designs (ir! macro DSL in `crates/llvmkit-macros/` as sibling module `ir.rs`; rustc-quality diagnostics in llvmkit-support), upstream coverage gaps with the LLVM-22-verified signatures (lifetime intrinsics without size args, CreateGlobalString, min/max + intrinsic-helper families, FMF-variant completions, const-GEP shortcuts, debug-loc/operand bundles, insert-point guards as scoped closures), ergonomics backlog (atomic builders, load/store builder consolidation, per-flag wrappers), inspiration-derived candidates (no-panic positioning vs inkwell with the wasmer#962 citation, egg/egglog e-graph pass family, Alive2-style refinement checking), type-system follow-ups (T4 const-generic aggregates, Width relations, auto-SSA aggregate variables, address-space-typed pointers, typed fold_gep/fold_select), plus everything individual tasks punted here (Task 12 items if skipped, `[F; N]` IrField arrays, vector-of-pointer GEP bases, derive-generated field-index consts, `TypedInvokeInst`, typed callbr/intrinsics, varargs invoke).

- [ ] **Step 2: README + AGENTS.md**

README: typed-calls flagship snippet (spec Workstream-1 usage block), auto-SSA section with the factorial before/after teaser, `TypedPointerValue` bullet, and a short "why llvmkit vs inkwell" positioning paragraph (compile-time conversion safety, no C strings, forbid(unsafe_code)). AGENTS.md: add Project Status bullets for this session's four workstreams (follow the existing bullet style: name, one-paragraph summary, key types), update the block-state names in older bullets (done in Task 1 — re-verify).

- [ ] **Step 3: Final audit + commit**

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt -- --check
grep -rn "TODO\|FIXME" crates/llvmkit-ir/src/ssa_builder.rs crates/llvmkit-ir/src/typed_pointer_value.rs  # expected: none
```
Every new test has an UPSTREAM.md row (cross-check: `grep -c "#\[test\]"` on new test files vs new rows). Commit: `"Document session: future-work roadmap + feature docs"` + trailer. Then use superpowers:finishing-a-development-branch to decide merge/PR.

